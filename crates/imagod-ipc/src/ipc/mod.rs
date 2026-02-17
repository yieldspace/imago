//! Transport-agnostic IPC contracts for manager/runner coordination.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use imago_protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use imagod_common::ImagodError;

/// DBus-like peer-to-peer transport implementation over Unix domain sockets.
pub mod dbus_p2p;

const STAGE_IPC: &str = "ipc";
const STAGE_TOKEN: &str = "ipc.token";
const MAX_INVOCATION_TOKEN_CHARS: usize = 4096;
type HmacSha256 = Hmac<Sha256>;

#[allow(dead_code)]
/// Boxed future used by transport traits to avoid exposing concrete future types.
pub type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, ImagodError>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Binding rule that allows one service to invoke an interface on another service.
pub struct ServiceBinding {
    /// Name of the destination service.
    pub target: String,
    /// WIT interface identifier that is allowed for the target.
    pub wit: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
/// Runtime application type carried from manifest into runner bootstrap.
pub enum RunnerAppType {
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "socket")]
    Socket,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime protocol policy.
pub enum RunnerSocketProtocol {
    #[serde(rename = "udp")]
    Udp,
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "both")]
    Both,
}

impl RunnerSocketProtocol {
    /// Returns true when UDP is allowed by this protocol policy.
    pub fn allows_udp(self) -> bool {
        matches!(self, Self::Udp | Self::Both)
    }

    /// Returns true when TCP is allowed by this protocol policy.
    pub fn allows_tcp(self) -> bool {
        matches!(self, Self::Tcp | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime traffic direction policy.
pub enum RunnerSocketDirection {
    #[serde(rename = "inbound")]
    Inbound,
    #[serde(rename = "outbound")]
    Outbound,
    #[serde(rename = "both")]
    Both,
}

impl RunnerSocketDirection {
    /// Returns true when inbound socket operations are allowed.
    pub fn allows_inbound(self) -> bool {
        matches!(self, Self::Inbound | Self::Both)
    }

    /// Returns true when outbound socket operations are allowed.
    pub fn allows_outbound(self) -> bool {
        matches!(self, Self::Outbound | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Socket runtime execution settings used when `app_type=socket`.
pub struct RunnerSocketConfig {
    /// Allowed socket protocol set.
    pub protocol: RunnerSocketProtocol,
    /// Allowed socket traffic direction.
    pub direction: RunnerSocketDirection,
    /// Bind address required for inbound socket operations.
    pub listen_addr: String,
    /// Bind port required for inbound socket operations.
    pub listen_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
/// Plugin delivery kind used by manifest/bootstrap dependency definitions.
pub enum PluginKind {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Wasm plugin component descriptor.
pub struct PluginComponent {
    /// Component path. In manifest this is release-relative; in runner bootstrap this is absolute.
    pub path: PathBuf,
    /// Hex-encoded SHA-256 digest for component bytes.
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
/// Function-level capability policy used by app/plugin callers.
pub struct CapabilityPolicy {
    /// When true, all dependency and WASI calls are allowed.
    #[serde(default)]
    pub privileged: bool,
    /// Dependency plugin function permissions.
    #[serde(default)]
    pub deps: BTreeMap<String, Vec<String>>,
    /// WASI interface function permissions.
    #[serde(default)]
    pub wasi: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// One plugin dependency definition passed to runner runtime.
pub struct PluginDependency {
    /// Canonical package name.
    pub name: String,
    /// Dependency version string.
    pub version: String,
    /// Delivery kind.
    pub kind: PluginKind,
    /// WIT source identifier.
    pub wit: String,
    /// Additional plugin package dependencies required by this plugin.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Wasm component descriptor for `kind=wasm`.
    #[serde(default)]
    pub component: Option<PluginComponent>,
    /// Capability policy used when this plugin is the caller.
    #[serde(default)]
    pub capabilities: CapabilityPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Bootstrap payload sent from manager to runner via child stdin.
pub struct RunnerBootstrap {
    /// Ephemeral runner identifier generated by manager.
    pub runner_id: String,
    /// Service name owned by this runner.
    pub service_name: String,
    /// Release hash to be executed.
    pub release_hash: String,
    /// Runtime execution model selected from manifest `type`.
    pub app_type: RunnerAppType,
    /// TCP port used by local HTTP ingress when `app_type=http`.
    pub http_port: Option<u16>,
    /// Max accepted HTTP request body size in bytes when `app_type=http`.
    pub http_max_body_bytes: Option<u64>,
    /// Socket runtime configuration when `app_type=socket`.
    pub socket: Option<RunnerSocketConfig>,
    /// Absolute path to the component file.
    pub component_path: PathBuf,
    /// CLI arguments passed to WASI command.
    pub args: Vec<String>,
    /// Environment variables passed to the runtime.
    pub envs: std::collections::BTreeMap<String, String>,
    /// Allowed outbound bindings for this service.
    pub bindings: Vec<ServiceBinding>,
    /// Plugin dependencies available for this app/plugin execution context.
    #[serde(default)]
    pub plugin_dependencies: Vec<PluginDependency>,
    /// App-level capability policy.
    #[serde(default)]
    pub capabilities: CapabilityPolicy,
    /// Manager control socket endpoint.
    pub manager_control_endpoint: PathBuf,
    /// Runner inbound socket endpoint.
    pub runner_endpoint: PathBuf,
    /// Shared secret used for manager-auth proof generation.
    pub manager_auth_secret: String,
    /// Secret used by manager to sign invocation tokens for this runner.
    pub invocation_secret: String,
    /// Epoch tick interval used by the runner runtime loop.
    pub epoch_tick_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Control-plane request from runner to manager.
pub enum ControlRequest {
    /// Registers a newly spawned runner endpoint.
    RegisterRunner {
        /// Runner identifier.
        runner_id: String,
        /// Service name attached to runner.
        service_name: String,
        /// Release hash announced by runner.
        release_hash: String,
        /// Endpoint where runner accepts inbound requests.
        runner_endpoint: PathBuf,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Marks the runner as ready to execute workload.
    RunnerReady {
        /// Runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Sends runner liveness information.
    Heartbeat {
        /// Runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Resolves destination runner endpoint and invocation token.
    ResolveInvocationTarget {
        /// Caller runner identifier.
        runner_id: String,
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
        /// Destination service name.
        target_service: String,
        /// Requested WIT interface identifier.
        wit: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Control-plane response returned by manager.
pub enum ControlResponse {
    /// Generic success response for one-way commands.
    Ack,
    /// Resolved target endpoint plus short-lived authorization token.
    ResolvedInvocationTarget {
        /// Destination runner endpoint.
        endpoint: PathBuf,
        /// Authorization token scoped for one invocation target.
        token: String,
    },
    /// Structured control-plane failure.
    Error(IpcErrorPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runner inbound request issued by manager or other runners.
pub enum RunnerInboundRequest {
    /// Requests graceful runner shutdown.
    ShutdownRunner {
        /// Proof derived from manager secret and runner id.
        manager_auth_proof: String,
    },
    /// Requests interface function invocation on the runner.
    Invoke {
        /// Interface identifier to invoke.
        interface_id: String,
        /// Function name inside the interface.
        function: String,
        /// Raw CBOR payload for invocation arguments.
        payload_cbor: Vec<u8>,
        /// Manager-signed authorization token.
        token: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Runner inbound response.
pub enum RunnerInboundResponse {
    /// Generic success response.
    Ack,
    /// Invocation result payload encoded as CBOR.
    InvokeResult {
        /// Raw CBOR payload returned by invocation.
        payload_cbor: Vec<u8>,
    },
    /// Structured runner-side failure.
    Error(IpcErrorPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Wire-safe error payload for IPC responses.
pub struct IpcErrorPayload {
    /// Protocol error code.
    pub code: ErrorCode,
    /// Stage identifier for diagnostics.
    pub stage: String,
    /// Human-readable message.
    pub message: String,
}

impl IpcErrorPayload {
    /// Converts an internal error into wire payload form.
    pub fn from_error(err: &ImagodError) -> Self {
        Self {
            code: err.code,
            stage: err.stage.clone(),
            message: err.message.clone(),
        }
    }

    /// Converts wire payload form back into an internal error.
    pub fn to_error(&self) -> ImagodError {
        ImagodError::new(self.code, self.stage.clone(), self.message.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Claims embedded in manager-signed invocation tokens.
pub struct InvocationTokenClaims {
    /// Calling service name.
    pub source_service: String,
    /// Destination service name.
    pub target_service: String,
    /// Authorized WIT interface identifier.
    pub wit: String,
    /// Expiration epoch (seconds).
    pub exp: u64,
    /// Unique nonce to prevent token reuse patterns.
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Internal signed token envelope serialized into URL-safe base64.
struct SignedInvocationToken {
    claims: InvocationTokenClaims,
    signature: String,
}

#[allow(dead_code)]
/// Transport abstraction for runner-to-manager control requests.
pub trait ControlPlaneTransport: Send + Sync {
    /// Sends one control request to the manager endpoint.
    fn call_control<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a ControlRequest,
    ) -> BoxFutureResult<'a, ControlResponse>;
}

#[allow(dead_code)]
/// Transport abstraction for inbound runner invocations.
pub trait InvocationTransport: Send + Sync {
    /// Sends one request to a runner endpoint.
    fn call_runner<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a RunnerInboundRequest,
    ) -> BoxFutureResult<'a, RunnerInboundResponse>;
}

/// Computes a deterministic manager-auth proof from secret and runner id.
pub fn compute_manager_auth_proof(
    secret_hex: &str,
    runner_id: &str,
) -> Result<String, ImagodError> {
    let secret = decode_secret_hex(secret_hex)?;
    compute_hmac_signature(&secret, runner_id.as_bytes())
}

/// Verifies manager-auth proof from secret and runner id using constant-time comparison.
pub fn verify_manager_auth_proof(
    secret_hex: &str,
    runner_id: &str,
    proof_hex: &str,
) -> Result<(), ImagodError> {
    let secret = decode_secret_hex(secret_hex)?;
    verify_hmac_signature_hex(
        &secret,
        runner_id.as_bytes(),
        proof_hex,
        STAGE_IPC,
        "manager auth proof decode failed",
        "manager auth proof mismatch",
    )
}

/// Issues a short-lived invocation token signed by the given secret.
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

/// Verifies signature and expiration of a manager-issued invocation token.
pub fn verify_invocation_token(
    secret_hex: &str,
    token: &str,
) -> Result<InvocationTokenClaims, ImagodError> {
    if token.len() > MAX_INVOCATION_TOKEN_CHARS {
        return Err(ImagodError::new(
            ErrorCode::Unauthorized,
            STAGE_TOKEN,
            format!(
                "token is too large: {} chars (max {MAX_INVOCATION_TOKEN_CHARS})",
                token.len()
            ),
        ));
    }

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

    let claims_bytes = imago_protocol::to_cbor(&envelope.claims).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TOKEN,
            format!("claims encode failed: {e}"),
        )
    })?;
    verify_hmac_signature_hex(
        &secret,
        &claims_bytes,
        &envelope.signature,
        STAGE_TOKEN,
        "token signature decode failed",
        "token signature mismatch",
    )?;

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
    compute_hmac_signature(secret, &claims_bytes)
}

fn compute_hmac_signature(secret: &[u8], payload: &[u8]) -> Result<String, ImagodError> {
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TOKEN,
            format!("hmac initialization failed: {e}"),
        )
    })?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn verify_hmac_signature_hex(
    secret: &[u8],
    payload: &[u8],
    signature_hex: &str,
    stage: &str,
    decode_error_message: &str,
    mismatch_message: &str,
) -> Result<(), ImagodError> {
    let provided = hex::decode(signature_hex).map_err(|e| {
        ImagodError::new(
            ErrorCode::Unauthorized,
            stage,
            format!("{decode_error_message}: {e}"),
        )
    })?;
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            stage,
            format!("hmac initialization failed: {e}"),
        )
    })?;
    mac.update(payload);
    mac.verify_slice(&provided)
        .map_err(|_| ImagodError::new(ErrorCode::Unauthorized, stage, mismatch_message.to_string()))
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

/// Generates a pseudo-random hex secret for ephemeral IPC auth.
pub fn random_secret_hex() -> String {
    let mut secret = [0_u8; 32];
    secret[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    secret[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    hex::encode(secret)
}

/// Returns the current Unix time in seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Maps transport-local failures to a structured IPC internal error.
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
    fn runner_app_type_round_trip_via_cbor() {
        let encoded =
            imago_protocol::to_cbor(&RunnerAppType::Http).expect("app type encoding should work");
        let decoded = imago_protocol::from_cbor::<RunnerAppType>(&encoded)
            .expect("app type decoding should work");
        assert_eq!(decoded, RunnerAppType::Http);
    }

    #[test]
    fn runner_bootstrap_includes_app_type_in_cbor_round_trip() {
        let bootstrap = RunnerBootstrap {
            runner_id: "runner-a".to_string(),
            service_name: "svc-a".to_string(),
            release_hash: "release-a".to_string(),
            app_type: RunnerAppType::Socket,
            http_port: None,
            http_max_body_bytes: None,
            socket: Some(RunnerSocketConfig {
                protocol: RunnerSocketProtocol::Udp,
                direction: RunnerSocketDirection::Inbound,
                listen_addr: "0.0.0.0".to_string(),
                listen_port: 514,
            }),
            component_path: PathBuf::from("/tmp/component.wasm"),
            args: vec!["--help".to_string()],
            envs: std::collections::BTreeMap::new(),
            bindings: vec![],
            plugin_dependencies: vec![PluginDependency {
                name: "yieldspace:plugin/example".to_string(),
                version: "0.1.0".to_string(),
                kind: PluginKind::Native,
                wit: "warg://yieldspace:plugin/example@0.1.0".to_string(),
                requires: vec![],
                component: None,
                capabilities: CapabilityPolicy::default(),
            }],
            capabilities: CapabilityPolicy {
                privileged: false,
                deps: BTreeMap::from([(
                    "yieldspace:plugin/example".to_string(),
                    vec!["*".to_string()],
                )]),
                wasi: BTreeMap::new(),
            },
            manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
            runner_endpoint: PathBuf::from("/tmp/runner.sock"),
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            epoch_tick_interval_ms: 50,
        };
        let encoded = imago_protocol::to_cbor(&bootstrap).expect("bootstrap encoding should work");
        let decoded = imago_protocol::from_cbor::<RunnerBootstrap>(&encoded)
            .expect("bootstrap decoding should work");
        assert_eq!(decoded.app_type, RunnerAppType::Socket);
        assert_eq!(decoded.http_port, None);
        assert_eq!(decoded.http_max_body_bytes, None);
        assert_eq!(
            decoded.socket.as_ref().map(|cfg| cfg.listen_port),
            Some(514)
        );
        assert_eq!(
            decoded.socket.as_ref().map(|cfg| cfg.protocol),
            Some(RunnerSocketProtocol::Udp)
        );
        assert_eq!(decoded.plugin_dependencies.len(), 1);
        assert_eq!(
            decoded.plugin_dependencies[0].name,
            "yieldspace:plugin/example"
        );
        assert!(
            decoded
                .capabilities
                .deps
                .contains_key("yieldspace:plugin/example")
        );
    }

    #[test]
    fn random_secret_hex_has_32_bytes() {
        let secret_hex = random_secret_hex();
        assert_eq!(secret_hex.len(), 64);
        let secret = hex::decode(secret_hex).expect("secret should be valid hex");
        assert_eq!(secret.len(), 32);
    }

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

    #[test]
    fn verify_manager_auth_proof_accepts_valid_proof() {
        let secret = random_secret_hex();
        let proof = compute_manager_auth_proof(&secret, "runner-1").expect("proof should succeed");
        verify_manager_auth_proof(&secret, "runner-1", &proof).expect("valid proof should verify");
    }

    #[test]
    fn verify_manager_auth_proof_rejects_invalid_and_malformed_proof() {
        let secret = random_secret_hex();
        let mismatch = verify_manager_auth_proof(&secret, "runner-1", "deadbeef")
            .expect_err("mismatched proof should be rejected");
        assert_eq!(mismatch.code, ErrorCode::Unauthorized);

        let malformed = verify_manager_auth_proof(&secret, "runner-1", "not-hex")
            .expect_err("malformed proof should be rejected");
        assert_eq!(malformed.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn verify_invocation_token_rejects_oversized_token_before_decode() {
        let secret = random_secret_hex();
        let oversized = "a".repeat(MAX_INVOCATION_TOKEN_CHARS + 1);
        let err = verify_invocation_token(&secret, &oversized)
            .expect_err("oversized token should be rejected");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        assert!(err.message.contains("too large"));
    }
}
