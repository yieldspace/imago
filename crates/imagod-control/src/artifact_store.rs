//! Artifact upload session management and commit verification logic.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::UNIX_EPOCH,
};

#[cfg(test)]
use imago_protocol::ArtifactStatus;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushRequest, ByteRange,
    DeployPrepareRequest, DeployPrepareResponse, ErrorCode,
};
use sha2::{Digest, Sha256};
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

use imagod_common::ImagodError;
use session_index::InMemoryUploadSessionStore;

use self::chunk_pipeline::FileChunkSink;

mod chunk_pipeline;
mod commit;
mod session_index;

const STAGE_PREPARE: &str = "deploy.prepare";
const STAGE_PUSH: &str = "artifact.push";
const STAGE_COMMIT: &str = "artifact.commit";
const STAGE_ORCHESTRATE: &str = "orchestration";

#[derive(Debug, Clone)]
/// A fully committed artifact resolved by `deploy_id`.
pub struct CommittedArtifact {
    /// Deployment identifier that owns this artifact.
    pub deploy_id: String,
    /// Path to the persisted artifact archive on local storage.
    pub path: PathBuf,
    /// Manifest digest recorded during commit.
    pub manifest_digest: String,
    /// Artifact digest recorded during commit.
    pub artifact_digest: String,
}

#[derive(Debug, Clone)]
/// Internal mutable state for one upload session.
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
/// In-memory index of active sessions and idempotency keys.
struct StoreState {
    sessions: BTreeMap<String, UploadSession>,
    idempotency: BTreeMap<String, String>,
}

#[derive(Default)]
/// Deferred filesystem cleanup actions collected while lock is held.
struct CleanupPlan {
    files: Vec<PathBuf>,
}

impl CleanupPlan {
    /// Merges file removal actions from another cleanup plan.
    fn merge(&mut self, other: CleanupPlan) {
        self.files.extend(other.files);
    }
}

#[derive(Clone)]
/// Artifact storage service for prepare/push/commit operations.
pub struct ArtifactStore {
    root: Arc<PathBuf>,
    state: Arc<Mutex<StoreState>>,
    pinned_deploy_ids: Arc<StdMutex<BTreeMap<String, usize>>>,
    session_store: InMemoryUploadSessionStore,
    chunk_sink: FileChunkSink,
    upload_session_ttl_secs: u64,
    committed_session_ttl_secs: u64,
    max_committed_sessions: usize,
    max_chunk_size: usize,
    max_inflight_chunks: usize,
    max_artifact_size_bytes: u64,
}

/// RAII guard for one pinned deploy session.
pub struct DeploySessionPinGuard {
    deploy_id: String,
    pinned_deploy_ids: Arc<StdMutex<BTreeMap<String, usize>>>,
}

impl Drop for DeploySessionPinGuard {
    fn drop(&mut self) {
        let mut pinned = match self.pinned_deploy_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(count) = pinned.get_mut(&self.deploy_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pinned.remove(&self.deploy_id);
            }
        }
    }
}

impl ArtifactStore {
    /// Creates a new artifact store rooted at `root`.
    pub async fn new(
        root: impl AsRef<Path>,
        upload_session_ttl_secs: u64,
        committed_session_ttl_secs: u64,
        max_committed_sessions: usize,
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
            pinned_deploy_ids: Arc::new(StdMutex::new(BTreeMap::new())),
            session_store: InMemoryUploadSessionStore,
            chunk_sink: FileChunkSink,
            upload_session_ttl_secs: upload_session_ttl_secs.max(1),
            committed_session_ttl_secs: committed_session_ttl_secs.max(1),
            max_committed_sessions: max_committed_sessions.max(1),
            max_chunk_size: max_chunk_size.max(1),
            max_inflight_chunks: max_inflight_chunks.max(1),
            max_artifact_size_bytes: max_artifact_size_bytes.max(1),
        })
    }

    /// Pins one deploy session from orphan cleanup while the returned guard is alive.
    pub fn pin_deploy_session(&self, deploy_id: &str) -> DeploySessionPinGuard {
        let mut pinned = match self.pinned_deploy_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let count = pinned.entry(deploy_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        DeploySessionPinGuard {
            deploy_id: deploy_id.to_string(),
            pinned_deploy_ids: self.pinned_deploy_ids.clone(),
        }
    }

    fn collect_session_cleanup_locked(
        &self,
        state: &mut StoreState,
        now_epoch_secs: u64,
        keep_deploy_id: Option<&str>,
    ) -> CleanupPlan {
        let mut protected_deploy_ids = self.pinned_deploy_ids_snapshot();
        if let Some(deploy_id) = keep_deploy_id.filter(|id| !id.is_empty()) {
            protected_deploy_ids.insert(deploy_id.to_string());
        }
        let mut cleanup_plan = CleanupPlan::default();
        cleanup_plan.merge(self.session_store.collect_expired_sessions(
            state,
            now_epoch_secs,
            self.upload_session_ttl_secs,
        ));
        cleanup_plan.merge(self.session_store.collect_orphan_committed_sessions(
            state,
            now_epoch_secs,
            self.committed_session_ttl_secs,
            self.max_committed_sessions,
            &protected_deploy_ids,
        ));
        self.session_store.cleanup_orphan_idempotency(state);
        cleanup_plan
    }

    fn pinned_deploy_ids_snapshot(&self) -> HashSet<String> {
        let pinned = match self.pinned_deploy_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        pinned.keys().cloned().collect()
    }

    /// Handles `deploy.prepare` by creating or resuming an upload session.
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
            cleanup_plan.merge(self.collect_session_cleanup_locked(&mut state, now, None));

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
                    Ok(PrepareDecision::Existing(
                        self.session_store.build_prepare_response(
                            existing,
                            self.upload_session_ttl_secs,
                            now,
                        ),
                    ))
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
                if let Err(err) = self
                    .chunk_sink
                    .create_preallocated_file(
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
                    cleanup_plan.merge(self.collect_session_cleanup_locked(
                        &mut state,
                        now_after_io,
                        None,
                    ));

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
                            Ok(self.session_store.build_prepare_response(
                                existing,
                                self.upload_session_ttl_secs,
                                now_after_io,
                            ))
                        }
                    } else {
                        session_candidate.updated_at_epoch_secs = now_after_io;
                        let deploy_id = session_candidate.deploy_id.clone();
                        let idempotency_key = session_candidate.idempotency_key.clone();
                        let response = self.session_store.build_prepare_response(
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

    /// Handles `artifact.push` by validating and writing one chunk.
    pub async fn push(&self, request: ArtifactPushRequest) -> Result<ArtifactPushAck, ImagodError> {
        let now = now_epoch_secs();
        let ArtifactPushRequest { header, chunk } = request;
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
            cleanup_plan.merge(self.collect_session_cleanup_locked(&mut state, now, None));

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

        let write_result = self
            .chunk_sink
            .write_chunk_to_file(&file_path, header.offset, &chunk)
            .await;

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
            commit::merge_range(
                &mut session.received_ranges,
                commit::range_from_start_end(header.offset, chunk_end),
            );
            session.updated_at_epoch_secs = now;
            let next_missing =
                commit::next_missing_range(&session.received_ranges, session.artifact_size);
            Ok(ArtifactPushAck {
                received_ranges: session.received_ranges.clone(),
                next_missing_range: next_missing,
                accepted_bytes: header.length,
            })
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    /// Handles `artifact.commit` by verifying digest and finalizing session state.
    pub async fn commit(
        &self,
        request: ArtifactCommitRequest,
    ) -> Result<ArtifactCommitResponse, ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        let prepare_commit = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(self.collect_session_cleanup_locked(&mut state, now, None));

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

            if !commit::is_complete(&session.received_ranges, session.artifact_size) {
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

        let digest_result = commit::digest_file(&file_path).await;

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
            let current_deploy_id = session.deploy_id.clone();
            let artifact_id = session.artifact_digest.clone();

            cleanup_plan.merge(self.collect_session_cleanup_locked(
                &mut state,
                now,
                Some(&current_deploy_id),
            ));

            Ok(ArtifactCommitResponse {
                artifact_id,
                verified: true,
            })
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    /// Returns committed artifact metadata for a completed deploy id.
    pub async fn committed_artifact(
        &self,
        deploy_id: &str,
    ) -> Result<CommittedArtifact, ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        let result = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(self.collect_session_cleanup_locked(
                &mut state,
                now,
                Some(deploy_id),
            ));

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

    /// Resolves service name for a completed deploy id used by `command.start`.
    pub async fn service_name_for_deploy(&self, deploy_id: &str) -> Result<String, ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        let result = {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(self.collect_session_cleanup_locked(
                &mut state,
                now,
                Some(deploy_id),
            ));

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
            Ok(session.service_name.clone())
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    /// Removes upload session state and temp file for one deploy id.
    pub async fn purge_deploy_session(&self, deploy_id: &str) -> Result<(), ImagodError> {
        let now = now_epoch_secs();
        let mut cleanup_plan = CleanupPlan::default();

        {
            let mut state = self.state.lock().await;
            cleanup_plan.merge(self.collect_session_cleanup_locked(
                &mut state,
                now,
                Some(deploy_id),
            ));
            cleanup_plan.merge(
                self.session_store
                    .collect_sessions_by_deploy_ids(&mut state, vec![deploy_id.to_string()]),
            );
        }

        apply_cleanup_plan(cleanup_plan).await;
        Ok(())
    }
}

/// Builds a prepare response from session progress and configured expiry.
#[cfg(test)]
fn build_prepare_response(
    session: &UploadSession,
    upload_session_ttl_secs: u64,
    now_epoch_secs: u64,
) -> DeployPrepareResponse {
    InMemoryUploadSessionStore.build_prepare_response(
        session,
        upload_session_ttl_secs,
        now_epoch_secs,
    )
}

/// Executes filesystem cleanup outside lock scope.
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

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_internal(stage: &str, message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, stage, message).with_retryable(true)
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
    async fn keeps_committed_artifacts_until_explicit_cleanup() {
        let (store, root) = new_store("keeps_committed_artifacts_until_explicit_cleanup", 60).await;

        let first = prepare_push_commit(&store, "svc-c", b"artifact-v1", "idem-v1").await;
        let first_path = root
            .join("sessions")
            .join(format!("{}.artifact", first.deploy_id));
        assert!(first_path.exists());

        let second = prepare_push_commit(&store, "svc-c", b"artifact-v2", "idem-v2").await;
        let second_path = root
            .join("sessions")
            .join(format!("{}.artifact", second.deploy_id));

        assert!(first_path.exists());
        assert!(second_path.exists());

        let old = store
            .committed_artifact(&first.deploy_id)
            .await
            .expect("first committed artifact should remain");
        assert_eq!(old.deploy_id, first.deploy_id);

        let latest = store
            .committed_artifact(&second.deploy_id)
            .await
            .expect("latest artifact should remain");
        assert_eq!(latest.deploy_id, second.deploy_id);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn purge_deploy_session_removes_committed_state_and_file() {
        let (store, root) =
            new_store("purge_deploy_session_removes_committed_state_and_file", 60).await;
        let commit = prepare_push_commit(&store, "svc-purge", b"artifact-v1", "idem-purge").await;
        let committed_path = root
            .join("sessions")
            .join(format!("{}.artifact", commit.deploy_id));
        assert!(
            committed_path.exists(),
            "committed artifact file should exist"
        );

        store
            .purge_deploy_session(&commit.deploy_id)
            .await
            .expect("purge should succeed");
        assert!(
            !committed_path.exists(),
            "purged artifact file should be removed"
        );

        let err = store
            .committed_artifact(&commit.deploy_id)
            .await
            .expect_err("purged deploy should be removed");
        assert_eq!(err.code, ErrorCode::NotFound);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn committed_orphan_cleanup_enforces_max_sessions() {
        let (store, root) = new_store_with_session_limits(
            "committed_orphan_cleanup_enforces_max_sessions",
            60,
            600,
            2,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let first = prepare_push_commit(&store, "svc-1", b"artifact-1", "idem-1").await;
        let second = prepare_push_commit(&store, "svc-2", b"artifact-2", "idem-2").await;
        let third = prepare_push_commit(&store, "svc-3", b"artifact-3", "idem-3").await;

        let first_artifact = store.committed_artifact(&first.deploy_id).await.ok();
        let second_artifact = store.committed_artifact(&second.deploy_id).await.ok();
        let third_artifact = store
            .committed_artifact(&third.deploy_id)
            .await
            .expect("newest committed session should remain");
        assert_eq!(third_artifact.deploy_id, third.deploy_id);
        let retained_count = usize::from(first_artifact.is_some())
            + usize::from(second_artifact.is_some())
            + usize::from(true);
        assert_eq!(
            retained_count, 2,
            "max_committed_sessions=2 should keep exactly two committed sessions"
        );

        cleanup_root(root);
    }

    #[tokio::test]
    async fn pinned_deploy_session_is_excluded_from_orphan_cleanup() {
        let (store, root) = new_store_with_session_limits(
            "pinned_deploy_session_is_excluded_from_orphan_cleanup",
            60,
            600,
            1,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let pinned =
            prepare_push_commit(&store, "svc-pinned", b"artifact-pinned", "idem-pinned").await;
        let _pin_guard = store.pin_deploy_session(&pinned.deploy_id);
        let unpinned = prepare_push_commit(
            &store,
            "svc-unpinned",
            b"artifact-unpinned",
            "idem-unpinned",
        )
        .await;

        let _ = store
            .prepare(DeployPrepareRequest {
                name: "svc-trigger".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: hex::encode(Sha256::digest(b"artifact-trigger")),
                artifact_size: b"artifact-trigger".len() as u64,
                manifest_digest: hex::encode(Sha256::digest(b"manifest-trigger")),
                idempotency_key: "idem-trigger".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should trigger orphan cleanup");

        let kept = store
            .committed_artifact(&pinned.deploy_id)
            .await
            .expect("pinned deploy should be protected from orphan cleanup");
        assert_eq!(kept.deploy_id, pinned.deploy_id);

        let removed = store
            .committed_artifact(&unpinned.deploy_id)
            .await
            .expect_err("unpinned deploy should be evicted by orphan cleanup");
        assert_eq!(removed.code, ErrorCode::NotFound);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn committed_orphan_cleanup_expires_sessions_after_ttl() {
        let (store, root) = new_store_with_session_limits(
            "committed_orphan_cleanup_expires_sessions_after_ttl",
            60,
            1,
            16,
            TEST_CHUNK_SIZE,
            TEST_MAX_INFLIGHT,
            TEST_MAX_ARTIFACT_SIZE,
        )
        .await;
        let first =
            prepare_push_commit(&store, "svc-expire", b"artifact-expire", "idem-expire").await;
        let first_path = root
            .join("sessions")
            .join(format!("{}.artifact", first.deploy_id));
        assert!(first_path.exists(), "committed artifact file should exist");

        tokio::time::sleep(Duration::from_secs(2)).await;

        let _ = store
            .prepare(DeployPrepareRequest {
                name: "svc-trigger".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest: hex::encode(Sha256::digest(b"artifact-trigger")),
                artifact_size: b"artifact-trigger".len() as u64,
                manifest_digest: hex::encode(Sha256::digest(b"manifest-trigger")),
                idempotency_key: "idem-trigger".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should trigger committed session cleanup");

        assert!(
            !first_path.exists(),
            "expired committed artifact file should be removed"
        );
        let err = store
            .committed_artifact(&first.deploy_id)
            .await
            .expect_err("expired committed session should be removed");
        assert_eq!(err.code, ErrorCode::NotFound);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn service_name_for_deploy_returns_committed_service_name() {
        let (store, root) = new_store("service_name_for_deploy_committed", 60).await;
        let commit = prepare_push_commit(&store, "svc-lookup", b"artifact-v1", "idem-lookup").await;

        let service_name = store
            .service_name_for_deploy(&commit.deploy_id)
            .await
            .expect("service_name_for_deploy should resolve committed service name");
        assert_eq!(service_name, "svc-lookup");

        cleanup_root(root);
    }

    #[tokio::test]
    async fn service_name_for_deploy_rejects_uncommitted_deploy() {
        let (store, root) = new_store("service_name_for_deploy_uncommitted", 60).await;
        let artifact = b"artifact-uncommitted";
        let artifact_digest = hex::encode(Sha256::digest(artifact));
        let manifest_digest = hex::encode(Sha256::digest(b"manifest-uncommitted"));

        let prepare = store
            .prepare(DeployPrepareRequest {
                name: "svc-uncommitted".to_string(),
                app_type: "cli".to_string(),
                target: BTreeMap::new(),
                artifact_digest,
                artifact_size: artifact.len() as u64,
                manifest_digest,
                idempotency_key: "idem-uncommitted".to_string(),
                policy: BTreeMap::new(),
            })
            .await
            .expect("prepare should succeed");

        let err = store
            .service_name_for_deploy(&prepare.deploy_id)
            .await
            .expect_err("uncommitted deploy should be rejected");
        assert_eq!(err.code, ErrorCode::ArtifactIncomplete);
        assert_eq!(err.stage, "orchestration");
        assert_eq!(err.message, "artifact.commit has not been completed");

        cleanup_root(root);
    }

    #[tokio::test]
    async fn service_name_for_deploy_rejects_missing_deploy() {
        let (store, root) = new_store("service_name_for_deploy_missing", 60).await;

        let err = store
            .service_name_for_deploy("deploy-missing")
            .await
            .expect_err("missing deploy_id should be rejected");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(err.stage, "orchestration");
        assert_eq!(err.message, "deploy_id is not found for command.start");

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
                chunk: artifact.to_vec(),
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
                chunk: artifact.to_vec(),
            })
            .await
            .expect_err("push should fail when max_inflight_chunks is reached");
        assert_eq!(err.code, ErrorCode::Busy);

        cleanup_root(root);
    }

    #[tokio::test]
    async fn push_rejects_chunk_length_mismatch() {
        let (store, root) = new_store_with_limits(
            "push_rejects_chunk_length_mismatch",
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
                chunk: artifact.to_vec(),
            })
            .await
            .expect_err("chunk length mismatch should be rejected");
        assert_eq!(err.code, ErrorCode::RangeInvalid);

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
                chunk: artifact.to_vec(),
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
        new_store_with_session_limits(
            test_name,
            ttl_secs,
            120,
            16,
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
        new_store_with_session_limits(
            test_name,
            ttl_secs,
            120,
            16,
            chunk_size,
            max_inflight_chunks,
            max_artifact_size_bytes,
        )
        .await
    }

    async fn new_store_with_session_limits(
        test_name: &str,
        ttl_secs: u64,
        committed_session_ttl_secs: u64,
        max_committed_sessions: usize,
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
            committed_session_ttl_secs,
            max_committed_sessions,
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

    fn test_session_with_ranges(ranges: Vec<ByteRange>, artifact_size: u64) -> UploadSession {
        UploadSession {
            deploy_id: "deploy-test".to_string(),
            service_name: "svc-test".to_string(),
            idempotency_key: "idem-test".to_string(),
            fingerprint: "fingerprint-test".to_string(),
            artifact_digest: "sha256:artifact".to_string(),
            artifact_size,
            manifest_digest: "sha256:manifest".to_string(),
            upload_token: "upload-token".to_string(),
            file_path: PathBuf::from("/tmp/imagod-artifact-store-test.artifact"),
            received_ranges: ranges,
            committed: false,
            inflight_writes: 0,
            commit_in_progress: false,
            updated_at_epoch_secs: 1,
        }
    }

    #[test]
    fn build_prepare_response_partial_returns_all_missing_ranges() {
        let session = test_session_with_ranges(
            vec![
                ByteRange {
                    offset: 0,
                    length: 10,
                },
                ByteRange {
                    offset: 20,
                    length: 10,
                },
                ByteRange {
                    offset: 40,
                    length: 10,
                },
            ],
            60,
        );

        let response = build_prepare_response(&session, 900, 100);
        assert_eq!(response.artifact_status, ArtifactStatus::Partial);
        assert_eq!(
            response.missing_ranges,
            vec![
                ByteRange {
                    offset: 10,
                    length: 10,
                },
                ByteRange {
                    offset: 30,
                    length: 10,
                },
                ByteRange {
                    offset: 50,
                    length: 10,
                },
            ]
        );
    }

    #[test]
    fn build_prepare_response_missing_and_complete_have_expected_ranges() {
        let missing_session = test_session_with_ranges(Vec::new(), 128);
        let missing = build_prepare_response(&missing_session, 900, 100);
        assert_eq!(missing.artifact_status, ArtifactStatus::Missing);
        assert_eq!(
            missing.missing_ranges,
            vec![ByteRange {
                offset: 0,
                length: 128,
            }]
        );

        let mut complete_session = test_session_with_ranges(
            vec![ByteRange {
                offset: 0,
                length: 128,
            }],
            128,
        );
        complete_session.committed = true;
        let complete = build_prepare_response(&complete_session, 900, 100);
        assert_eq!(complete.artifact_status, ArtifactStatus::Complete);
        assert!(complete.missing_ranges.is_empty());
    }
}
