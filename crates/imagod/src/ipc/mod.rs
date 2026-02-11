use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use imago_protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ImagodError;

pub mod dbus_p2p;

const STAGE_IPC: &str = "ipc";
const STAGE_TOKEN: &str = "ipc.token";

#[allow(dead_code)]
pub type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, ImagodError>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceBinding {
    pub target: String,
    pub wit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerBootstrap {
    pub runner_id: String,
    pub service_name: String,
    pub release_hash: String,
    pub component_path: PathBuf,
    pub args: Vec<String>,
    pub envs: std::collections::BTreeMap<String, String>,
    pub bindings: Vec<ServiceBinding>,
    pub manager_control_endpoint: PathBuf,
    pub runner_endpoint: PathBuf,
    pub manager_auth_secret: String,
    pub invocation_secret: String,
    pub epoch_tick_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlRequest {
    RegisterRunner {
        runner_id: String,
        service_name: String,
        release_hash: String,
        runner_endpoint: PathBuf,
        manager_auth_proof: String,
    },
    RunnerReady {
        runner_id: String,
        manager_auth_proof: String,
    },
    Heartbeat {
        runner_id: String,
        manager_auth_proof: String,
    },
    ResolveInvocationTarget {
        runner_id: String,
        manager_auth_proof: String,
        target_service: String,
        wit: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResponse {
    Ack,
    ResolvedInvocationTarget { endpoint: PathBuf, token: String },
    Error(IpcErrorPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunnerInboundRequest {
    ShutdownRunner,
    Invoke {
        interface_id: String,
        function: String,
        payload_cbor: Vec<u8>,
        token: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunnerInboundResponse {
    Ack,
    InvokeResult { payload_cbor: Vec<u8> },
    Error(IpcErrorPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcErrorPayload {
    pub code: ErrorCode,
    pub stage: String,
    pub message: String,
}

impl IpcErrorPayload {
    pub fn from_error(err: &ImagodError) -> Self {
        Self {
            code: err.code,
            stage: err.stage.clone(),
            message: err.message.clone(),
        }
    }

    pub fn to_error(&self) -> ImagodError {
        ImagodError::new(self.code, self.stage.clone(), self.message.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationTokenClaims {
    pub source_service: String,
    pub target_service: String,
    pub wit: String,
    pub exp: u64,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedInvocationToken {
    claims: InvocationTokenClaims,
    signature: String,
}

#[allow(dead_code)]
pub trait ControlPlaneTransport: Send + Sync {
    fn call_control<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a ControlRequest,
    ) -> BoxFutureResult<'a, ControlResponse>;
}

#[allow(dead_code)]
pub trait InvocationTransport: Send + Sync {
    fn call_runner<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a RunnerInboundRequest,
    ) -> BoxFutureResult<'a, RunnerInboundResponse>;
}

pub fn compute_manager_auth_proof(
    secret_hex: &str,
    runner_id: &str,
) -> Result<String, ImagodError> {
    let secret = decode_secret_hex(secret_hex)?;
    let mut hasher = Sha256::new();
    hasher.update(&secret);
    hasher.update(runner_id.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

pub fn issue_invocation_token(
    secret_hex: &str,
    claims: InvocationTokenClaims,
) -> Result<String, ImagodError> {
    let secret = decode_secret_hex(secret_hex)?;
    let signature = compute_token_signature(&secret, &claims)?;
    let envelope = SignedInvocationToken { claims, signature };
    let bytes = imago_protocol::to_cbor(&envelope).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TOKEN,
            format!("token encode failed: {e}"),
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn verify_invocation_token(
    secret_hex: &str,
    token: &str,
) -> Result<InvocationTokenClaims, ImagodError> {
    let secret = decode_secret_hex(secret_hex)?;
    let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|e| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_TOKEN,
            format!("token decode failed: {e}"),
        )
    })?;
    let envelope = imago_protocol::from_cbor::<SignedInvocationToken>(&bytes).map_err(|e| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_TOKEN,
            format!("token parse failed: {e}"),
        )
    })?;

    let expected = compute_token_signature(&secret, &envelope.claims)?;
    if envelope.signature != expected {
        return Err(ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_TOKEN,
            "token signature mismatch",
        ));
    }

    if envelope.claims.exp <= now_unix_secs() {
        return Err(ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_TOKEN,
            "token expired",
        ));
    }

    Ok(envelope.claims)
}

fn compute_token_signature(
    secret: &[u8],
    claims: &InvocationTokenClaims,
) -> Result<String, ImagodError> {
    let claims_bytes = imago_protocol::to_cbor(claims).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TOKEN,
            format!("claims encode failed: {e}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(&claims_bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn decode_secret_hex(secret_hex: &str) -> Result<Vec<u8>, ImagodError> {
    hex::decode(secret_hex).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TOKEN,
            format!("secret decode failed: {e}"),
        )
    })
}

pub fn random_secret_hex() -> String {
    let mut hasher = Sha256::new();
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.update(now_unix_secs().to_be_bytes());
    hex::encode(hasher.finalize())
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn map_ipc_error(stage: &str, message: impl Into<String>) -> ImagodError {
    ImagodError::new(
        ErrorCode::Internal,
        format!("{STAGE_IPC}.{stage}"),
        message.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_token_round_trip() {
        let secret = random_secret_hex();
        let claims = InvocationTokenClaims {
            source_service: "svc-a".to_string(),
            target_service: "svc-b".to_string(),
            wit: "pkg:iface/callable".to_string(),
            exp: now_unix_secs() + 60,
            nonce: uuid::Uuid::new_v4().to_string(),
        };

        let token =
            issue_invocation_token(&secret, claims.clone()).expect("token issue should succeed");
        let verified =
            verify_invocation_token(&secret, &token).expect("token verify should succeed");

        assert_eq!(verified.source_service, claims.source_service);
        assert_eq!(verified.target_service, claims.target_service);
        assert_eq!(verified.wit, claims.wit);
    }

    #[test]
    fn manager_auth_proof_is_stable_for_same_input() {
        let secret = random_secret_hex();
        let p1 = compute_manager_auth_proof(&secret, "runner-1").expect("proof should succeed");
        let p2 = compute_manager_auth_proof(&secret, "runner-1").expect("proof should succeed");
        assert_eq!(p1, p2);
    }
}
