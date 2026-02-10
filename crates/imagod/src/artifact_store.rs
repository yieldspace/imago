use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use imago_protocol::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushRequest,
    ArtifactRange, ArtifactStatus, DeployPrepareRequest, DeployPrepareResponse, ErrorCode,
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
    fingerprint: String,
    artifact_digest: String,
    artifact_size: u64,
    manifest_digest: String,
    upload_token: String,
    file_path: PathBuf,
    received_ranges: Vec<ArtifactRange>,
    committed: bool,
}

#[derive(Default)]
struct StoreState {
    sessions: BTreeMap<String, UploadSession>,
    idempotency: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct ArtifactStore {
    root: Arc<PathBuf>,
    state: Arc<Mutex<StoreState>>,
}

impl ArtifactStore {
    pub async fn new(root: impl AsRef<Path>) -> Result<Self, ImagodError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("sessions"))
            .await
            .map_err(|e| map_internal(STAGE_PREPARE, e.to_string()))?;

        Ok(Self {
            root: Arc::new(root),
            state: Arc::new(Mutex::new(StoreState::default())),
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
        let mut state = self.state.lock().await;

        if let Some(existing_id) = state.idempotency.get(&request.idempotency_key).cloned()
            && let Some(existing) = state.sessions.get(&existing_id)
        {
            if existing.fingerprint != fingerprint {
                return Err(ImagodError::new(
                    ErrorCode::IdempotencyConflict,
                    STAGE_PREPARE,
                    "idempotency_key is reused with different payload",
                ));
            }
            return Ok(build_prepare_response(existing));
        }

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
            fingerprint,
            artifact_digest: request.artifact_digest,
            artifact_size: request.artifact_size,
            manifest_digest: request.manifest_digest,
            upload_token,
            file_path,
            received_ranges: Vec::new(),
            committed: false,
        };

        let response = build_prepare_response(&session);
        state
            .idempotency
            .insert(request.idempotency_key, deploy_id.clone());
        state.sessions.insert(deploy_id, session);

        Ok(response)
    }

    pub async fn push(&self, request: ArtifactPushRequest) -> Result<ArtifactPushAck, ImagodError> {
        let mut state = self.state.lock().await;
        let session = state.sessions.get_mut(&request.deploy_id).ok_or_else(|| {
            ImagodError::new(ErrorCode::NotFound, STAGE_PUSH, "deploy_id is not found")
        })?;

        if session.upload_token != request.upload_token {
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

        let chunk = STANDARD
            .decode(request.chunk_b64.as_bytes())
            .map_err(|e| map_bad_request(STAGE_PUSH, format!("chunk_b64 decode failed: {e}")))?;

        if chunk.len() as u64 != request.length {
            return Err(ImagodError::new(
                ErrorCode::RangeInvalid,
                STAGE_PUSH,
                "chunk length does not match request.length",
            ));
        }

        let chunk_end = request.offset.checked_add(request.length).ok_or_else(|| {
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
        if chunk_hash != request.chunk_sha256 {
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
        file.seek(std::io::SeekFrom::Start(request.offset))
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
            ArtifactRange::new(request.offset, chunk_end),
        );
        let next_missing = next_missing_range(&session.received_ranges, session.artifact_size);

        Ok(ArtifactPushAck {
            received_ranges: session.received_ranges.clone(),
            next_missing_range: next_missing,
            accepted_bytes: request.length,
        })
    }

    pub async fn commit(
        &self,
        request: ArtifactCommitRequest,
    ) -> Result<ArtifactCommitResponse, ImagodError> {
        let mut state = self.state.lock().await;
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
        Ok(ArtifactCommitResponse {
            artifact_id: session.artifact_digest.clone(),
            verified: true,
        })
    }

    pub async fn committed_artifact(
        &self,
        deploy_id: &str,
    ) -> Result<CommittedArtifact, ImagodError> {
        let state = self.state.lock().await;
        let session = state.sessions.get(deploy_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "orchestration",
                "deploy_id is not found for command.start",
            )
        })?;

        if !session.committed {
            return Err(ImagodError::new(
                ErrorCode::ArtifactIncomplete,
                "orchestration",
                "artifact.commit has not been completed",
            ));
        }

        Ok(CommittedArtifact {
            deploy_id: session.deploy_id.clone(),
            path: session.file_path.clone(),
            manifest_digest: session.manifest_digest.clone(),
            artifact_digest: session.artifact_digest.clone(),
        })
    }
}

fn build_prepare_response(session: &UploadSession) -> DeployPrepareResponse {
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
        session_expires_at: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 900)
            .to_string(),
    }
}

fn fingerprint(request: &DeployPrepareRequest) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        request.name,
        request.service_type as u8,
        request.artifact_digest,
        request.artifact_size,
        request.manifest_digest,
        request.target.len()
    )
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

fn is_complete(ranges: &[ArtifactRange], total: u64) -> bool {
    if ranges.len() != 1 {
        return false;
    }
    let first = &ranges[0];
    first.start == 0 && first.end == total
}

fn next_missing_range(ranges: &[ArtifactRange], total: u64) -> Option<ArtifactRange> {
    if total == 0 {
        return None;
    }
    if ranges.is_empty() {
        return Some(ArtifactRange::new(0, total));
    }

    let mut cursor = 0;
    for range in ranges {
        if cursor < range.start {
            return Some(ArtifactRange::new(cursor, range.start));
        }
        cursor = range.end;
    }
    if cursor < total {
        return Some(ArtifactRange::new(cursor, total));
    }
    None
}

fn merge_range(ranges: &mut Vec<ArtifactRange>, incoming: ArtifactRange) {
    ranges.push(incoming);
    ranges.sort_by_key(|r| r.start);

    let mut merged: Vec<ArtifactRange> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }

    *ranges = merged;
}

fn map_internal(stage: &str, message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, stage, message).with_retryable(true)
}

fn map_bad_request(stage: &str, message: String) -> ImagodError {
    ImagodError::new(ErrorCode::BadRequest, stage, message)
}
