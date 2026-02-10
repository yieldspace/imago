use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
    ArtifactStatus, ByteRange, DeployPrepareRequest, DeployPrepareResponse, ErrorCode,
};
use serde::{Deserialize, Serialize};
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
    updated_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPushRequest {
    #[serde(flatten)]
    pub header: ArtifactPushChunkHeader,
    pub chunk_b64: String,
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
}

impl ArtifactStore {
    pub async fn new(
        root: impl AsRef<Path>,
        upload_session_ttl_secs: u64,
    ) -> Result<Self, ImagodError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("sessions"))
            .await
            .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;

        Ok(Self {
            root: Arc::new(root),
            state: Arc::new(Mutex::new(StoreState::default())),
            upload_session_ttl_secs: upload_session_ttl_secs.max(1),
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

        let fingerprint = fingerprint(&request);
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
                    Ok(build_prepare_response(
                        existing,
                        self.upload_session_ttl_secs,
                        now,
                    ))
                }
            } else {
                let deploy_id = Uuid::new_v4().to_string();
                let upload_token = Uuid::new_v4().to_string();
                let file_path = self
                    .root
                    .join("sessions")
                    .join(format!("{deploy_id}.artifact"));

                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .read(true)
                    .open(&file_path)
                    .await
                    .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;
                file.set_len(request.artifact_size)
                    .await
                    .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;
                file.flush()
                    .await
                    .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;

                let session = UploadSession {
                    deploy_id: deploy_id.clone(),
                    service_name: request.name,
                    idempotency_key: request.idempotency_key.clone(),
                    fingerprint,
                    artifact_digest: request.artifact_digest,
                    artifact_size: request.artifact_size,
                    manifest_digest: request.manifest_digest,
                    upload_token,
                    file_path,
                    received_ranges: Vec::new(),
                    committed: false,
                    updated_at_epoch_secs: now,
                };

                let response = build_prepare_response(&session, self.upload_session_ttl_secs, now);
                state
                    .idempotency
                    .insert(request.idempotency_key, deploy_id.clone());
                state.sessions.insert(deploy_id, session);
                Ok(response)
            }
        };

        apply_cleanup_plan(cleanup_plan).await;
        result
    }

    pub async fn push(&self, request: ArtifactPushRequest) -> Result<ArtifactPushAck, ImagodError> {
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

            let header = &request.header;
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

            if session.committed {
                return Err(ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_PUSH,
                    "artifact is already committed",
                ));
            }

            let chunk = STANDARD.decode(request.chunk_b64.as_bytes()).map_err(|e| {
                map_bad_request(STAGE_PUSH, format!("chunk_b64 decode failed: {e}"))
            })?;

            if chunk.len() as u64 != header.length {
                return Err(ImagodError::new(
                    ErrorCode::RangeInvalid,
                    STAGE_PUSH,
                    "chunk length does not match header.length",
                ));
            }

            let chunk_end = header.length.checked_add(header.offset).ok_or_else(|| {
                ImagodError::new(ErrorCode::RangeInvalid, STAGE_PUSH, "chunk overflow")
            })?;
            if chunk_end > session.artifact_size {
                return Err(ImagodError::new(
                    ErrorCode::RangeInvalid,
                    STAGE_PUSH,
                    "chunk range is outside artifact size",
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

            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&session.file_path)
                .await
                .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
            file.seek(std::io::SeekFrom::Start(header.offset))
                .await
                .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;
            file.flush()
                .await
                .map_err(|e| map_internal(STAGE_PUSH, e.to_string()))?;

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

        let result = {
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

            if !is_complete(&session.received_ranges, session.artifact_size) {
                return Err(ImagodError::new(
                    ErrorCode::ArtifactIncomplete,
                    STAGE_COMMIT,
                    "artifact is incomplete",
                ));
            }

            let digest = digest_file(&session.file_path).await?;
            if digest != session.artifact_digest {
                return Err(ImagodError::new(
                    ErrorCode::BadManifest,
                    STAGE_COMMIT,
                    "artifact digest mismatch",
                ));
            }

            session.committed = true;
            session.updated_at_epoch_secs = now;
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
            if session.committed {
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
    use std::time::Duration;

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
        let root = std::env::temp_dir().join(format!(
            "imagod-artifact-store-tests-{}-{}",
            test_name,
            now_epoch_secs()
        ));
        let store = ArtifactStore::new(&root, ttl_secs)
            .await
            .expect("store init should succeed");
        (store, root)
    }

    fn cleanup_root(root: PathBuf) {
        let _ = std::fs::remove_dir_all(root);
    }
}
