use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushRequest,
    ArtifactStatus, ByteRange, DeployPrepareRequest, DeployPrepareResponse, ErrorCode,
};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};
use uuid::Uuid;

use crate::error::ImagodError;

const STAGE_PREPARE: &str = "deploy.prepare";
const STAGE_PUSH: &str = "artifact.push";
const STAGE_COMMIT: &str = "artifact.commit";
const STAGE_ORCHESTRATE: &str = "orchestration";

#[derive(Debug, Clone)]
pub struct CommittedArtifact {
    pub deploy_id: String,
    pub path: PathBuf,
    pub manifest_digest: String,
    pub artifact_digest: String,
}

#[derive(Debug, Clone)]
struct UploadSession {
    deploy_id: String,
    service_name: String,
    idempotency_key: String,
    fingerprint: String,
    artifact_digest: String,
    artifact_size: u64,
    manifest_digest: String,
    upload_token: String,
    file_path: PathBuf,
    received_ranges: Vec<ByteRange>,
    committed: bool,
    inflight_writes: usize,
    commit_in_progress: bool,
    updated_at_epoch_secs: u64,
}

#[derive(Default)]
struct StoreState {
    sessions: BTreeMap<String, UploadSession>,
    idempotency: BTreeMap<String, String>,
}

#[derive(Default)]
struct CleanupPlan {
    files: Vec<PathBuf>,
}

impl CleanupPlan {
    fn merge(&mut self, other: CleanupPlan) {
        self.files.extend(other.files);
    }
}

#[derive(Clone)]
pub struct ArtifactStore {
    root: Arc<PathBuf>,
    state: Arc<Mutex<StoreState>>,
    upload_session_ttl_secs: u64,
    max_chunk_size: usize,
    max_inflight_chunks: usize,
    max_artifact_size_bytes: u64,
}

impl ArtifactStore {
    pub async fn new(
        root: impl AsRef<Path>,
        upload_session_ttl_secs: u64,
        max_chunk_size: usize,
        max_inflight_chunks: usize,
        max_artifact_size_bytes: u64,
    ) -> Result<Self, ImagodError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("sessions"))
            .await
            .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;

        Ok(Self {
            root: Arc::new(root),
            state: Arc::new(Mutex::new(StoreState::default())),
            upload_session_ttl_secs: upload_session_ttl_secs.max(1),
            max_chunk_size: max_chunk_size.max(1),
            max_inflight_chunks: max_inflight_chunks.max(1),
            max_artifact_size_bytes: max_artifact_size_bytes.max(1),
        })
    }

    pub async fn prepare(
        &self,
        request: DeployPrepareRequest,
    ) -> Result<DeployPrepareResponse, ImagodError> {
        if request.artifact_size == 0 {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_PREPARE,
                "artifact_size must be > 0",
            ));
        }
        if request.artifact_size > self.max_artifact_size_bytes {
            return Err(ImagodError::new(
                ErrorCode::StorageQuota,
                STAGE_PREPARE,
                "artifact_size exceeds max_artifact_size_bytes",
            )
            .with_detail("artifact_size", request.artifact_size.to_string())
            .with_detail(
                "max_artifact_size_bytes",
                self.max_artifact_size_bytes.to_string(),
            ));
        }

        let fingerprint = fingerprint(&request);
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        enum PrepareDecision {
            Existing(DeployPrepareResponse),
            Create(UploadSession),
        }

        let decision: Result<PrepareDecision, ImagodError> = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(collect_expired_sessions_locked(
                &mut state,
                now,
                self.upload_session_ttl_secs,
            ));
            cleanup_orphan_idempotency_locked(&mut state);

            if let Some(existing_id) = state.idempotency.get(&request.idempotency_key).cloned()
                && let Some(existing) = state.sessions.get_mut(&existing_id)
            {
                if existing.fingerprint != fingerprint {
                    Err(ImagodError::new(
                        ErrorCode::IdempotencyConflict,
                        STAGE_PREPARE,
                        "idempotency_key is reused with different payload",
                    ))
                } else {
                    existing.updated_at_epoch_secs = now;
                    Ok(PrepareDecision::Existing(build_prepare_response(
                        existing,
                        self.upload_session_ttl_secs,
                        now,
                    )))
                }
            } else {
                let deploy_id = Uuid::new_v4().to_string();
                let upload_token = Uuid::new_v4().to_string();
                let file_path = self
                    .root
                    .join("sessions")
                    .join(format!("{deploy_id}.artifact"));

                let session = UploadSession {
                    deploy_id: deploy_id.clone(),
                    service_name: request.name.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    fingerprint,
                    artifact_digest: request.artifact_digest.clone(),
                    artifact_size: request.artifact_size,
                    manifest_digest: request.manifest_digest.clone(),
                    upload_token,
                    file_path,
                    received_ranges: Vec::new(),
                    committed: false,
                    inflight_writes: 0,
                    commit_in_progress: false,
                    updated_at_epoch_secs: now,
                };
                Ok(PrepareDecision::Create(session))
            }
        };

        let decision = match decision {
            Ok(decision) => decision,
            Err(err) => {
                apply_cleanup_plan(cleanup_plan).await;
                return Err(err);
            }
        };

        let result = match decision {
            PrepareDecision::Existing(response) => Ok(response),
            PrepareDecision::Create(session_candidate) => {
                if let Err(err) = create_preallocated_file(
                    &session_candidate.file_path,
                    session_candidate.artifact_size,
                )
                .await
                {
                    apply_cleanup_plan(cleanup_plan).await;
                    return Err(err);
                }

                let now_after_io = now_epoch_secs();
                let mut created_file_path_to_cleanup: Option<PathBuf> = None;
                let mut session_candidate = session_candidate;
                let result = {
                    let mut state = self.state.lock().await;
                    cleanup_plan.merge(collect_expired_sessions_locked(
                        &mut state,
                        now_after_io,
                        self.upload_session_ttl_secs,
                    ));
                    cleanup_orphan_idempotency_locked(&mut state);

                    if let Some(existing_id) = state
                        .idempotency
                        .get(&session_candidate.idempotency_key)
                        .cloned()
                        && let Some(existing) = state.sessions.get_mut(&existing_id)
                    {
                        created_file_path_to_cleanup = Some(session_candidate.file_path.clone());
                        if existing.fingerprint != session_candidate.fingerprint {
                            Err(ImagodError::new(
                                ErrorCode::IdempotencyConflict,
                                STAGE_PREPARE,
                                "idempotency_key is reused with different payload",
                            ))
                        } else {
                            existing.updated_at_epoch_secs = now_after_io;
                            Ok(build_prepare_response(
                                existing,
                                self.upload_session_ttl_secs,
                                now_after_io,
                            ))
                        }
                    } else {
                        session_candidate.updated_at_epoch_secs = now_after_io;
                        let deploy_id = session_candidate.deploy_id.clone();
                        let idempotency_key = session_candidate.idempotency_key.clone();
                        let response = build_prepare_response(
                            &session_candidate,
                            self.upload_session_ttl_secs,
                            now_after_io,
                        );
                        state.idempotency.insert(idempotency_key, deploy_id.clone());
                        state.sessions.insert(deploy_id, session_candidate);
                        Ok(response)
                    }
                };

                if let Some(path) = created_file_path_to_cleanup {
                    cleanup_plan.files.push(path);
                }
                result
            }
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    pub async fn push(&self, request: ArtifactPushRequest) -> Result<ArtifactPushAck, ImagodError> {
        let now = now_epoch_secs();
        let header = request.header;
        let max_chunk_size = u64::try_from(self.max_chunk_size).unwrap_or(u64::MAX);
        if header.length > max_chunk_size {
            return Err(ImagodError::new(
                ErrorCode::RangeInvalid,
                STAGE_PUSH,
                "chunk length exceeds configured chunk_size",
            )
            .with_detail("chunk_length", header.length.to_string())
            .with_detail("chunk_size", max_chunk_size.to_string()));
        }

        let max_chunk_b64_len = max_base64_len_for_decoded_len(header.length).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::RangeInvalid,
                STAGE_PUSH,
                "chunk length is too large to validate base64 payload",
            )
        })?;
        if request.chunk_b64.len() > max_chunk_b64_len {
            return Err(ImagodError::new(
                ErrorCode::RangeInvalid,
                STAGE_PUSH,
                "chunk_b64 length exceeds declared header.length",
            )
            .with_detail("chunk_b64_len", request.chunk_b64.len().to_string())
            .with_detail("max_chunk_b64_len", max_chunk_b64_len.to_string())
            .with_detail("chunk_length", header.length.to_string()));
        }

        let chunk = STANDARD
            .decode(request.chunk_b64.as_bytes())
            .map_err(|e| map_bad_request(STAGE_PUSH, format!("chunk_b64 decode failed: {e}")))?;
        if chunk.len() as u64 != header.length {
            return Err(ImagodError::new(
                ErrorCode::RangeInvalid,
                STAGE_PUSH,
                "chunk length does not match header.length",
            ));
        }

        let chunk_hash = hex::encode(Sha256::digest(&chunk));
        if chunk_hash != header.chunk_sha256 {
            return Err(ImagodError::new(
                ErrorCode::ChunkHashMismatch,
                STAGE_PUSH,
                "chunk sha256 mismatch",
            ));
        }

        let chunk_end = header.length.checked_add(header.offset).ok_or_else(|| {
            ImagodError::new(ErrorCode::RangeInvalid, STAGE_PUSH, "chunk overflow")
        })?;

        let mut cleanup_plan = CleanupPlan::default();
        let prepare_write = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(collect_expired_sessions_locked(
                &mut state,
                now,
                self.upload_session_ttl_secs,
            ));
            cleanup_orphan_idempotency_locked(&mut state);

            let session = state.sessions.get_mut(&header.deploy_id).ok_or_else(|| {
                ImagodError::new(ErrorCode::NotFound, STAGE_PUSH, "deploy_id is not found")
            })?;

            if session.upload_token != header.upload_token {
                return Err(ImagodError::new(
                    ErrorCode::Unauthorized,
                    STAGE_PUSH,
                    "upload_token mismatch",
                ));
            }

            if session.commit_in_progress {
                return Err(ImagodError::new(
                    ErrorCode::Busy,
                    STAGE_PUSH,
                    "artifact commit is in progress",
                ));
            }

            if session.committed {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_PUSH,
                    "artifact is already committed",
                ));
            }

            if chunk_end > session.artifact_size {
                return Err(ImagodError::new(
                    ErrorCode::RangeInvalid,
                    STAGE_PUSH,
                    "chunk range is outside artifact size",
                ));
            }

            if session.inflight_writes >= self.max_inflight_chunks {
                return Err(ImagodError::new(
                    ErrorCode::Busy,
                    STAGE_PUSH,
                    "max_inflight_chunks limit reached",
                ));
            }

            session.inflight_writes += 1;
            session.updated_at_epoch_secs = now;
            Ok(session.file_path.clone())
        };

        let file_path = match prepare_write {
            Ok(path) => path,
            Err(err) => {
                apply_cleanup_plan(cleanup_plan).await;
                return Err(err);
            }
        };

        let write_result = write_chunk_to_file(&file_path, header.offset, &chunk).await;

        let result = {
            let mut state = self.state.lock().await;
            let session = state.sessions.get_mut(&header.deploy_id).ok_or_else(|| {
                map_internal(
                    STAGE_PUSH,
                    "session disappeared during artifact.push".to_string(),
                )
            })?;

            if session.inflight_writes > 0 {
                session.inflight_writes -= 1;
            }

            write_result?;
            merge_range(
                &mut session.received_ranges,
                range_from_start_end(header.offset, chunk_end),
            );
            session.updated_at_epoch_secs = now;
            let next_missing = next_missing_range(&session.received_ranges, session.artifact_size);
            Ok(ArtifactPushAck {
                received_ranges: session.received_ranges.clone(),
                next_missing_range: next_missing,
                accepted_bytes: header.length,
            })
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    pub async fn commit(
        &self,
        request: ArtifactCommitRequest,
    ) -> Result<ArtifactCommitResponse, ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        let prepare_commit = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(collect_expired_sessions_locked(
                &mut state,
                now,
                self.upload_session_ttl_secs,
            ));
            cleanup_orphan_idempotency_locked(&mut state);

            let session = state.sessions.get_mut(&request.deploy_id).ok_or_else(|| {
                ImagodError::new(ErrorCode::NotFound, STAGE_COMMIT, "deploy_id is not found")
            })?;

            if request.artifact_digest != session.artifact_digest
                || request.artifact_size != session.artifact_size
                || request.manifest_digest != session.manifest_digest
            {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_COMMIT,
                    "artifact metadata mismatch",
                ));
            }

            if session.commit_in_progress {
                return Err(ImagodError::new(
                    ErrorCode::Busy,
                    STAGE_COMMIT,
                    "artifact commit is already in progress",
                ));
            }

            if session.inflight_writes > 0 {
                return Err(ImagodError::new(
                    ErrorCode::Busy,
                    STAGE_COMMIT,
                    "artifact has in-flight writes",
                ));
            }

            if !is_complete(&session.received_ranges, session.artifact_size) {
                return Err(ImagodError::new(
                    ErrorCode::ArtifactIncomplete,
                    STAGE_COMMIT,
                    "artifact is incomplete",
                ));
            }

            session.commit_in_progress = true;
            session.updated_at_epoch_secs = now;
            Ok((session.file_path.clone(), session.artifact_digest.clone()))
        };

        let (file_path, expected_digest) = match prepare_commit {
            Ok(values) => values,
            Err(err) => {
                apply_cleanup_plan(cleanup_plan).await;
                return Err(err);
            }
        };

        let digest_result = digest_file(&file_path).await;

        let result = {
            let mut state = self.state.lock().await;
            let session = state.sessions.get_mut(&request.deploy_id).ok_or_else(|| {
                map_internal(
                    STAGE_COMMIT,
                    "session disappeared during artifact.commit".to_string(),
                )
            })?;

            session.commit_in_progress = false;
            session.updated_at_epoch_secs = now;

            let digest = digest_result?;
            if digest != expected_digest {
                return Err(ImagodError::new(
                    ErrorCode::BadManifest,
                    STAGE_COMMIT,
                    "artifact digest mismatch",
                ));
            }

            session.committed = true;
            let service_name = session.service_name.clone();
            let current_deploy_id = session.deploy_id.clone();
            let artifact_id = session.artifact_digest.clone();

            cleanup_plan.merge(collect_old_committed_sessions_locked(
                &mut state,
                &service_name,
                &current_deploy_id,
            ));

            Ok(ArtifactCommitResponse {
                artifact_id,
                verified: true,
            })
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    pub async fn committed_artifact(
        &self,
        deploy_id: &str,
    ) -> Result<CommittedArtifact, ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        let result = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(collect_expired_sessions_locked(
                &mut state,
                now,
                self.upload_session_ttl_secs,
            ));
            cleanup_orphan_idempotency_locked(&mut state);

            let session = state.sessions.get_mut(deploy_id).ok_or_else(|| {
                ImagodError::new(
                    ErrorCode::NotFound,
                    STAGE_ORCHESTRATE,
                    "deploy_id is not found for command.start",
                )
            })?;

            if !session.committed {
                return Err(ImagodError::new(
                    ErrorCode::ArtifactIncomplete,
                    STAGE_ORCHESTRATE,
                    "artifact.commit has not been completed",
                ));
            }

            session.updated_at_epoch_secs = now;
            Ok(CommittedArtifact {
                deploy_id: session.deploy_id.clone(),
                path: session.file_path.clone(),
                manifest_digest: session.manifest_digest.clone(),
                artifact_digest: session.artifact_digest.clone(),
            })
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }
}

fn build_prepare_response(
    session: &UploadSession,
    upload_session_ttl_secs: u64,
    now_epoch_secs: u64,
) -> DeployPrepareResponse {
    let artifact_status =
        if session.committed || is_complete(&session.received_ranges, session.artifact_size) {
            ArtifactStatus::Complete
        } else if session.received_ranges.is_empty() {
            ArtifactStatus::Missing
        } else {
            ArtifactStatus::Partial
        };

    let missing_ranges = match artifact_status {
        ArtifactStatus::Complete => Vec::new(),
        _ => next_missing_range(&session.received_ranges, session.artifact_size)
            .map(|v| vec![v])
            .unwrap_or_default(),
    };

    DeployPrepareResponse {
        deploy_id: session.deploy_id.clone(),
        artifact_status,
        missing_ranges,
        upload_token: session.upload_token.clone(),
        session_expires_at: now_epoch_secs
            .saturating_add(upload_session_ttl_secs)
            .to_string(),
    }
}

fn collect_expired_sessions_locked(
    state: &mut StoreState,
    now_epoch_secs: u64,
    upload_session_ttl_secs: u64,
) -> CleanupPlan {
    let expired_ids = state
        .sessions
        .iter()
        .filter_map(|(deploy_id, session)| {
            if session.committed || session.inflight_writes > 0 || session.commit_in_progress {
                return None;
            }

            let age = now_epoch_secs.saturating_sub(session.updated_at_epoch_secs);
            if age >= upload_session_ttl_secs {
                Some(deploy_id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    collect_sessions_for_removal_locked(state, expired_ids)
}

fn collect_old_committed_sessions_locked(
    state: &mut StoreState,
    service_name: &str,
    keep_deploy_id: &str,
) -> CleanupPlan {
    let old_ids = state
        .sessions
        .iter()
        .filter_map(|(deploy_id, session)| {
            if session.committed
                && session.service_name == service_name
                && deploy_id.as_str() != keep_deploy_id
            {
                Some(deploy_id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    collect_sessions_for_removal_locked(state, old_ids)
}

fn collect_sessions_for_removal_locked(
    state: &mut StoreState,
    deploy_ids: Vec<String>,
) -> CleanupPlan {
    let mut plan = CleanupPlan::default();

    for deploy_id in deploy_ids {
        if let Some(session) = state.sessions.remove(&deploy_id) {
            if state
                .idempotency
                .get(&session.idempotency_key)
                .is_some_and(|mapped| mapped == &deploy_id)
            {
                state.idempotency.remove(&session.idempotency_key);
            }
            plan.files.push(session.file_path);
        }
    }

    plan
}

fn cleanup_orphan_idempotency_locked(state: &mut StoreState) {
    let orphan_keys = state
        .idempotency
        .iter()
        .filter_map(|(key, deploy_id)| {
            if state.sessions.contains_key(deploy_id) {
                None
            } else {
                Some(key.clone())
            }
        })
        .collect::<Vec<_>>();

    for key in orphan_keys {
        state.idempotency.remove(&key);
    }
}

async fn apply_cleanup_plan(plan: CleanupPlan) {
    for path in plan.files {
        match fs::remove_file(&path).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "artifact cleanup failed path={} error={}",
                    path.display(),
                    e
                );
            }
        }
    }
}

async fn create_preallocated_file(path: &Path, artifact_size: u64) -> Result<(), ImagodError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(path)
        .await
        .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;
    file.set_len(artifact_size)
        .await
        .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;
    file.flush()
        .await
        .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))
}

async fn write_chunk_to_file(path: &Path, offset: u64, chunk: &[u8]) -> Result<(), ImagodError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
    file.write_all(chunk)
        .await
        .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
    file.flush()
        .await
        .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))
}

fn fingerprint(request: &DeployPrepareRequest) -> String {
    let mut extra_hasher = Sha256::new();
    extra_hasher.update(map_fingerprint_fragment(&request.target).as_bytes());
    extra_hasher.update(b"|");
    extra_hasher.update(map_fingerprint_fragment(&request.policy).as_bytes());
    let extra_hash = hex::encode(extra_hasher.finalize());

    format!(
        "{}|{}|{}|{}|{}|{}",
        request.name,
        request.app_type,
        request.artifact_digest,
        request.artifact_size,
        request.manifest_digest,
        extra_hash
    )
}

fn map_fingerprint_fragment(map: &BTreeMap<String, String>) -> String {
    let mut fragment = String::new();
    for (key, value) in map {
        fragment.push_str(key);
        fragment.push('=');
        fragment.push_str(value);
        fragment.push('\n');
    }
    fragment
}

fn max_base64_len_for_decoded_len(decoded_len: u64) -> Option<usize> {
    let groups = decoded_len.checked_add(2)?.checked_div(3)?;
    let encoded_len = groups.checked_mul(4)?;
    usize::try_from(encoded_len).ok()
}

async fn digest_file(path: &Path) -> Result<String, ImagodError> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .map_err(|e| map_internal(STAGE_COMMIT, e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 64];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| map_internal(STAGE_COMMIT, e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_complete(ranges: &[ByteRange], total: u64) -> bool {
    if ranges.len() != 1 {
        return false;
    }
    let first = &ranges[0];
    first.offset == 0 && first.length == total
}

fn next_missing_range(ranges: &[ByteRange], total: u64) -> Option<ByteRange> {
    if total == 0 {
        return None;
    }
    if ranges.is_empty() {
        return Some(ByteRange {
            offset: 0,
            length: total,
        });
    }

    let mut cursor = 0u64;
    for range in ranges {
        let start = range.offset;
        let end = range.offset.saturating_add(range.length);
        if cursor < start {
            return Some(range_from_start_end(cursor, start));
        }
        cursor = end;
    }
    if cursor < total {
        return Some(range_from_start_end(cursor, total));
    }
    None
}

fn merge_range(ranges: &mut Vec<ByteRange>, incoming: ByteRange) {
    ranges.push(incoming);
    ranges.sort_by_key(|r| r.offset);

    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.offset <= last.offset.saturating_add(last.length) => {
                let current_end = range.offset.saturating_add(range.length);
                let merged_end = last.offset.saturating_add(last.length).max(current_end);
                last.length = merged_end.saturating_sub(last.offset);
            }
            _ => merged.push(range),
        }
    }

    *ranges = merged;
}

fn range_from_start_end(start: u64, end: u64) -> ByteRange {
    ByteRange {
        offset: start,
        length: end.saturating_sub(start),
    }
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_internal(stage: &str, message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, stage, message).with_retryable(true)
}

fn map_bad_request(stage: &str, message: String) -> ImagodError {
    ImagodError::new(ErrorCode::BadRequest, stage, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imago_protocol::ArtifactPushChunkHeader;
    use std::time::Duration;

    const TEST_CHUNK_SIZE: usize = 1024 * 1024;
    const TEST_MAX_INFLIGHT: usize = 16;
    const TEST_MAX_ARTIFACT_SIZE: u64 = 64 * 1024 * 1024;

    #[tokio::test]
    async fn expires_incomplete_sessions_and_deletes_files() {
        let (store, root) = new_store("expires_incomplete_sessions", 1).await;
        let artifact = b"artifact-a";
        let manifest_digest = hex::encode(Sha256::digest(b"manifest-a"));
        let artifact_digest = hex::encode(Sha256::digest(artifact));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-a".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-a".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        let stale_path = root
            .join("sessions")
            .join(format!("{}.artifact", prepare.deploy_id));
        assert!(stale_path.exists());

        tokio::time::sleep(Duration::from_secs(2)).await;

        let _ = store
            .prepare(DeployPrepareRequest {
                name: "svc-b".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: hex::encode(Sha256::digest(b"artifact-b")),
                artifact_size: b"artifact-b".len() as u64,
                manifest_digest: hex::encode(Sha256::digest(b"manifest-b")),
                idempotency_key: "idem-b".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("second prepare should trigger cleanup");

        assert!(!stale_path.exists());

        let err = store
            .committed_artifact(&prepare.deploy_id)
            .await
            .expect_err("expired deploy should be removed");
        assert_eq!(err.code, ErrorCode::NotFound);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn keeps_latest_committed_artifact_per_service() {
        let (store, root) = new_store("keeps_latest_committed_artifact_per_service", 60).await;

        let first = prepare_push_commit(&store, "svc-c", b"artifact-v1", "idem-v1").await;
        let first_path = root
            .join("sessions")
            .join(format!("{}.artifact", first.deploy_id));
        assert!(first_path.exists());

        let second = prepare_push_commit(&store, "svc-c", b"artifact-v2", "idem-v2").await;
        let second_path = root
            .join("sessions")
            .join(format!("{}.artifact", second.deploy_id));

        assert!(!first_path.exists());
        assert!(second_path.exists());

        let old = store.committed_artifact(&first.deploy_id).await;
        assert!(old.is_err());

        let latest = store
            .committed_artifact(&second.deploy_id)
            .await
            .expect("latest artifact should remain");
        assert_eq!(latest.deploy_id, second.deploy_id);

        cleanup_root(root);
    }

    #[test]
    fn fingerprint_changes_when_target_or_policy_content_changes() {
        let mut target_a = BTreeMap::new();
        target_a.insert("remote".to_string(), "127.0.0.1:4443".to_string());
        target_a.insert("region".to_string(), "local".to_string());

        let mut target_b = BTreeMap::new();
        target_b.insert("remote".to_string(), "127.0.0.1:4443".to_string());
        target_b.insert("region".to_string(), "edge".to_string());

        let mut policy_a = BTreeMap::new();
        policy_a.insert("rollback".to_string(), "true".to_string());

        let mut policy_b = BTreeMap::new();
        policy_b.insert("rollback".to_string(), "false".to_string());

        let mut request = DeployPrepareRequest {
            name: "svc".to_string(),
            app_type: "cli".to_string(),
            target: target_a,
            artifact_digest: "sha256:artifact".to_string(),
            artifact_size: 1024,
            manifest_digest: "sha256:manifest".to_string(),
            idempotency_key: "idem".to_string(),
            policy: policy_a,
        };

        let baseline = fingerprint(&request);

        request.target = target_b;
        let target_changed = fingerprint(&request);
        assert_ne!(baseline, target_changed);

        request.target = BTreeMap::from([
            ("remote".to_string(), "127.0.0.1:4443".to_string()),
            ("region".to_string(), "local".to_string()),
        ]);
        request.policy = policy_b;
        let policy_changed = fingerprint(&request);
        assert_ne!(baseline, policy_changed);
    }

    #[tokio::test]
    async fn prepare_idempotency_conflict_does_not_create_extra_session_file() {
        let (store, root) = new_store("prepare_idempotency_conflict_cleanup", 60).await;
        let artifact = b"artifact-idempotency";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest-idempotency"));

        let mut first_target = BTreeMap::new();
        first_target.insert("region".to_string(), "local".to_string());
        let first = store
            .prepare(DeployPrepareRequest {
                name: "svc-idem".to_string(),
                app_type: "cli".to_string(),
                target: first_target,
                artifact_digest: artifact_digest.clone(),
                artifact_size: artifact.len() as u64,
                manifest_digest: manifest_digest.clone(),
                idempotency_key: "idem-shared".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("first prepare should succeed");

        let second_same = store
            .prepare(DeployPrepareRequest {
                name: "svc-idem".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::from([("region".to_string(), "local".to_string())]),
                artifact_digest: artifact_digest.clone(),
                artifact_size: artifact.len() as u64,
                manifest_digest: manifest_digest.clone(),
                idempotency_key: "idem-shared".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("same fingerprint should reuse session");
        assert_eq!(first.deploy_id, second_same.deploy_id);

        let err = store
            .prepare(DeployPrepareRequest {
                name: "svc-idem".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::from([("region".to_string(), "edge".to_string())]),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-shared".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect_err("different fingerprint should be rejected");
        assert_eq!(err.code, ErrorCode::IdempotencyConflict);

        let session_files = std::fs::read_dir(root.join("sessions"))
            .expect("session dir should be readable")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "artifact")
            })
            .count();
        assert_eq!(session_files, 1, "conflict must not leave extra files");

        cleanup_root(root);
    }

    #[tokio::test]
    async fn prepare_rejects_artifact_size_over_limit() {
        let (store, root) = new_store_with_limits(
            "prepare_rejects_artifact_size_over_limit",
            60,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            4,
        )
        .await;

        let err = store
            .prepare(DeployPrepareRequest {
                name: "svc-limit".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: "sha256:artifact".to_string(),
                artifact_size: 5,
                manifest_digest: "sha256:manifest".to_string(),
                idempotency_key: "idem-limit".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect_err("prepare should reject artifact over configured limit");
        assert_eq!(err.code, ErrorCode::StorageQuota);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn push_rejects_chunks_over_configured_size() {
        let (store, root) = new_store_with_limits(
            "push_rejects_chunks_over_configured_size",
            60,
            2,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let artifact = b"abcd";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest"));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-chunk-limit".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-chunk-limit".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        let err = store
            .push(ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: prepare.deploy_id,
                    offset: 0,
                    length: artifact.len() as u64,
                    chunk_sha256: hex::encode(Sha256::digest(artifact)),
                    upload_token: prepare.upload_token,
                },
                chunk_b64: STANDARD.encode(artifact),
            })
            .await
            .expect_err("push should fail when chunk exceeds configured limit");
        assert_eq!(err.code, ErrorCode::RangeInvalid);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn push_rejects_when_max_inflight_reached() {
        let (store, root) = new_store_with_limits(
            "push_rejects_when_max_inflight_reached",
            60,
            TEST_CHUNK_SIZE,
            1,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let artifact = b"abcd";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest"));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-inflight-limit".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-inflight-limit".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        {
            let mut state = store.state.lock().await;
            let session = state
                .sessions
                .get_mut(&prepare.deploy_id)
                .expect("session should exist");
            session.inflight_writes = 1;
        }

        let err = store
            .push(ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: prepare.deploy_id,
                    offset: 0,
                    length: artifact.len() as u64,
                    chunk_sha256: hex::encode(Sha256::digest(artifact)),
                    upload_token: prepare.upload_token,
                },
                chunk_b64: STANDARD.encode(artifact),
            })
            .await
            .expect_err("push should fail when max_inflight_chunks is reached");
        assert_eq!(err.code, ErrorCode::Busy);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn push_rejects_oversized_base64_payload_before_decode() {
        let (store, root) = new_store_with_limits(
            "push_rejects_oversized_base64_payload_before_decode",
            60,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let artifact = b"abcd";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest"));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-b64-limit".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-b64-limit".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        let err = store
            .push(ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: prepare.deploy_id,
                    offset: 0,
                    length: 1,
                    chunk_sha256: "irrelevant".to_string(),
                    upload_token: prepare.upload_token,
                },
                chunk_b64: "A".repeat(1024),
            })
            .await
            .expect_err("oversized base64 payload should be rejected before decode");
        assert_eq!(err.code, ErrorCode::RangeInvalid);
        assert!(err.to_string().contains("chunk_b64"));

        cleanup_root(root);
    }

    #[tokio::test]
    async fn commit_rejects_when_inflight_writes_exist() {
        let (store, root) = new_store("commit_rejects_when_inflight_writes_exist", 60).await;
        let artifact = b"artifact-a";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest-a"));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-commit-busy".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: artifact_digest.clone(),
                artifact_size: artifact.len() as u64,
                manifest_digest: manifest_digest.clone(),
                idempotency_key: "idem-commit-busy".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        {
            let mut state = store.state.lock().await;
            let session = state
                .sessions
                .get_mut(&prepare.deploy_id)
                .expect("session should exist");
            session.inflight_writes = 1;
        }

        let err = store
            .commit(ArtifactCommitRequest {
                deploy_id: prepare.deploy_id,
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
            })
            .await
            .expect_err("commit should fail when artifact has in-flight writes");
        assert_eq!(err.code, ErrorCode::Busy);

        cleanup_root(root);
    }

    struct CommitResult {
        deploy_id: String,
    }

    async fn prepare_push_commit(
        store: &ArtifactStore,
        service_name: &str,
        artifact: &[u8],
        idempotency_key: &str,
    ) -> CommitResult {
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_seed = format!("manifest-{idempotency_key}");
        let manifest_digest = hex::encode(Sha256::digest(manifest_seed.as_bytes()));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: service_name.to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: artifact_digest.clone(),
                artifact_size: artifact.len() as u64,
                manifest_digest: manifest_digest.clone(),
                idempotency_key: idempotency_key.to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        let chunk_hash = hex::encode(Sha256::digest(artifact));
        store
            .push(ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: prepare.deploy_id.clone(),
                    offset: 0,
                    length: artifact.len() as u64,
                    chunk_sha256: chunk_hash,
                    upload_token: prepare.upload_token,
                },
                chunk_b64: STANDARD.encode(artifact),
            })
            .await
            .expect("push should succeed");

        let commit = store
            .commit(ArtifactCommitRequest {
                deploy_id: prepare.deploy_id.clone(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
            })
            .await
            .expect("commit should succeed");
        assert!(commit.verified);

        CommitResult {
            deploy_id: prepare.deploy_id,
        }
    }

    async fn new_store(test_name: &str, ttl_secs: u64) -> (ArtifactStore, PathBuf) {
        new_store_with_limits(
            test_name,
            ttl_secs,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await
    }

    async fn new_store_with_limits(
        test_name: &str,
        ttl_secs: u64,
        chunk_size: usize,
        max_inflight_chunks: usize,
        max_artifact_size_bytes: u64,
    ) -> (ArtifactStore, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "imagod-artifact-store-tests-{}-{}",
            test_name,
            now_epoch_secs()
        ));
        let store = ArtifactStore::new(
            &root,
            ttl_secs,
            chunk_size,
            max_inflight_chunks,
            max_artifact_size_bytes,
        )
        .await
        .expect("store init should succeed");
        (store, root)
    }

    fn cleanup_root(root: PathBuf) {
        let _ = std::fs::remove_dir_all(root);
    }
}
