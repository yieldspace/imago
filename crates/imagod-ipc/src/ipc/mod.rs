//! Transport-agnostic IPC helpers for manager/runner coordination.

use std::{
    future::Future,
    path::Path,
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use imagod_common::ImagodError;
use imagod_spec::ipc::{
    ControlRequest, ControlResponse, InvocationTokenClaims, RunnerInboundRequest,
    RunnerInboundResponse,
};
use imagod_spec::wire::{ErrorCode, Validate, ValidationError};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// DBus-like peer-to-peer transport implementation over Unix domain sockets.
pub mod dbus_p2p;

const STAGE_IPC: &str = "ipc";
const STAGE_TOKEN: &str = "ipc.token";
const MAX_INVOCATION_TOKEN_CHARS: usize = 4096;
type HmacSha256 = Hmac<Sha256>;

fn validate_non_empty(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::empty(field));
    }

    Ok(())
}

fn validation_error(code: ErrorCode, stage: &str, error: ValidationError) -> ImagodError {
    ImagodError::new(code, stage, error.to_string())
}

#[allow(dead_code)]
/// Boxed future used by transport traits to avoid exposing concrete future types.
pub type BoxFutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, ImagodError>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Internal signed token envelope serialized into URL-safe base64.
struct SignedInvocationToken {
    claims: InvocationTokenClaims,
    signature: String,
}

impl Validate for SignedInvocationToken {
    fn validate(&self) -> Result<(), ValidationError> {
        self.claims.validate()?;
        validate_non_empty(&self.signature, "signature")
    }
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
    claims
        .validate()
        .map_err(|e| validation_error(ErrorCode::Internal, STAGE_TOKEN, e))?;
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
    envelope
        .validate()
        .map_err(|e| validation_error(ErrorCode::Unauthorized, STAGE_TOKEN, e))?;

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
    use std::{
        collections::BTreeMap,
        net::{IpAddr, Ipv4Addr},
        path::PathBuf,
    };

    use imagod_spec::{
        CapabilityPolicy, PluginComponent, PluginDependency, PluginKind, RunnerAppType,
        RunnerBootstrap, RunnerSocketConfig, RunnerSocketDirection, RunnerSocketProtocol,
        RunnerWasiMount, WasiHttpOutboundRule,
    };

    use super::*;

    fn valid_http_bootstrap() -> RunnerBootstrap {
        RunnerBootstrap {
            runner_id: "runner-a".to_string(),
            service_name: "svc-a".to_string(),
            release_hash: "release-a".to_string(),
            app_type: RunnerAppType::Http,
            http_port: Some(8080),
            http_max_body_bytes: Some(1024),
            http_worker_count: 2,
            http_worker_queue_capacity: 4,
            socket: None,
            component_path: PathBuf::from("/tmp/component.wasm"),
            args: vec![],
            envs: BTreeMap::new(),
            wasi_mounts: vec![],
            wasi_http_outbound: vec![],
            resources: BTreeMap::new(),
            bindings: vec![],
            plugin_dependencies: vec![],
            capabilities: CapabilityPolicy::default(),
            manager_control_endpoint: PathBuf::from("/tmp/manager.sock"),
            runner_endpoint: PathBuf::from("/tmp/runner.sock"),
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            epoch_tick_interval_ms: 50,
            wasm_memory_reservation_bytes: 64 * 1024 * 1024,
            wasm_memory_reservation_for_growth_bytes: 16 * 1024 * 1024,
            wasm_memory_guard_size_bytes: 64 * 1024,
            wasm_guard_before_linear_memory: false,
            wasm_parallel_compilation: false,
        }
    }

    #[test]
    fn runner_app_type_round_trip_via_cbor() {
        let encoded =
            imago_protocol::to_cbor(&RunnerAppType::Rpc).expect("app type encoding should work");
        let decoded = imago_protocol::from_cbor::<RunnerAppType>(&encoded)
            .expect("app type decoding should work");
        assert_eq!(decoded, RunnerAppType::Rpc);
    }

    #[test]
    fn runner_bootstrap_validate_accepts_rpc_without_http_or_socket() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.app_type = RunnerAppType::Rpc;
        bootstrap.http_port = None;
        bootstrap.http_max_body_bytes = None;
        bootstrap.socket = None;
        bootstrap
            .validate()
            .expect("rpc app type should be validated like cli");
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
            http_worker_count: 2,
            http_worker_queue_capacity: 4,
            socket: Some(RunnerSocketConfig {
                protocol: RunnerSocketProtocol::Udp,
                direction: RunnerSocketDirection::Inbound,
                listen_addr: "0.0.0.0".to_string(),
                listen_port: 514,
            }),
            component_path: PathBuf::from("/tmp/component.wasm"),
            args: vec!["--help".to_string()],
            envs: std::collections::BTreeMap::new(),
            wasi_mounts: vec![RunnerWasiMount {
                host_path: PathBuf::from("/tmp/assets"),
                guest_path: "/assets".to_string(),
                read_only: true,
            }],
            wasi_http_outbound: vec![WasiHttpOutboundRule::Host {
                host: "localhost".to_string(),
            }],
            resources: BTreeMap::from([(
                "i2c".to_string(),
                serde_json::json!({ "allowed_buses": ["/dev/i2c-1"] }),
            )]),
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
            wasm_memory_reservation_bytes: 64 * 1024 * 1024,
            wasm_memory_reservation_for_growth_bytes: 16 * 1024 * 1024,
            wasm_memory_guard_size_bytes: 64 * 1024,
            wasm_guard_before_linear_memory: false,
            wasm_parallel_compilation: false,
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
        assert_eq!(decoded.wasi_mounts.len(), 1);
        assert_eq!(decoded.wasi_mounts[0].guest_path, "/assets");
        assert!(decoded.wasi_mounts[0].read_only);
        assert_eq!(decoded.wasi_http_outbound.len(), 1);
        assert_eq!(
            decoded.wasi_http_outbound[0],
            WasiHttpOutboundRule::Host {
                host: "localhost".to_string()
            }
        );
        assert_eq!(
            decoded.resources.get("i2c"),
            Some(&serde_json::json!({ "allowed_buses": ["/dev/i2c-1"] }))
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
        assert_eq!(decoded.http_worker_count, 2);
        assert_eq!(decoded.http_worker_queue_capacity, 4);
        assert_eq!(decoded.wasm_memory_reservation_bytes, 64 * 1024 * 1024);
        assert_eq!(
            decoded.wasm_memory_reservation_for_growth_bytes,
            16 * 1024 * 1024
        );
        assert_eq!(decoded.wasm_memory_guard_size_bytes, 64 * 1024);
        assert!(!decoded.wasm_guard_before_linear_memory);
        assert!(!decoded.wasm_parallel_compilation);
    }

    #[test]
    fn runner_bootstrap_cbor_roundtrip_preserves_plugin_component_interfaces() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.plugin_dependencies = vec![PluginDependency {
            name: "yieldspace:plugin/example".to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Wasm,
            wit: "warg://yieldspace:plugin/example@0.1.0".to_string(),
            requires: vec![],
            component: Some(PluginComponent {
                path: PathBuf::from("/tmp/plugin-component.wasm"),
                sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                imports: Some(vec!["yieldspace:plugin/provider".to_string()]),
                exports: Some(vec!["yieldspace:plugin/example".to_string()]),
            }),
            capabilities: CapabilityPolicy::default(),
        }];

        let encoded = imago_protocol::to_cbor(&bootstrap).expect("bootstrap encoding should work");
        let decoded = imago_protocol::from_cbor::<RunnerBootstrap>(&encoded)
            .expect("bootstrap decoding should work");

        let decoded_component = decoded
            .plugin_dependencies
            .first()
            .and_then(|dep| dep.component.as_ref())
            .expect("decoded bootstrap should include plugin component metadata");
        assert_eq!(
            decoded_component.imports.as_ref(),
            Some(&vec!["yieldspace:plugin/provider".to_string()])
        );
        assert_eq!(
            decoded_component.exports.as_ref(),
            Some(&vec!["yieldspace:plugin/example".to_string()])
        );
    }

    #[test]
    fn plugin_component_validate_rejects_empty_import_or_export_name() {
        let invalid_import_component = PluginComponent {
            path: PathBuf::from("/tmp/plugin-component.wasm"),
            sha256: "abcdef".to_string(),
            imports: Some(vec!["".to_string()]),
            exports: None,
        };
        let err = invalid_import_component
            .validate()
            .expect_err("empty import should fail validation");
        assert!(err.to_string().contains("imports"));

        let invalid_export_component = PluginComponent {
            path: PathBuf::from("/tmp/plugin-component.wasm"),
            sha256: "abcdef".to_string(),
            imports: None,
            exports: Some(vec!["".to_string()]),
        };
        let err = invalid_export_component
            .validate()
            .expect_err("empty export should fail validation");
        assert!(err.to_string().contains("exports"));
    }

    #[test]
    fn runner_bootstrap_validate_rejects_http_worker_count_out_of_range() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.http_worker_count = 5;

        let err = bootstrap
            .validate()
            .expect_err("bootstrap should reject out-of-range http_worker_count");
        assert!(err.to_string().contains("http_worker_count"));
    }

    #[test]
    fn runner_bootstrap_validate_rejects_invalid_wasi_guest_path() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.wasi_mounts = vec![RunnerWasiMount {
            host_path: PathBuf::from("/tmp/assets"),
            guest_path: "../assets".to_string(),
            read_only: true,
        }];

        let err = bootstrap
            .validate()
            .expect_err("bootstrap should reject invalid wasi guest path");
        assert!(err.to_string().contains("guest_path"));
    }

    #[test]
    fn runner_bootstrap_validate_rejects_http_worker_queue_capacity_out_of_range() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.http_worker_queue_capacity = 17;

        let err = bootstrap
            .validate()
            .expect_err("bootstrap should reject out-of-range http_worker_queue_capacity");
        assert!(err.to_string().contains("http_worker_queue_capacity"));
    }

    #[test]
    fn runner_bootstrap_validate_rejects_zero_wasm_memory_reservation() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.wasm_memory_reservation_bytes = 0;

        let err = bootstrap
            .validate()
            .expect_err("bootstrap should reject zero wasm_memory_reservation_bytes");
        assert!(err.to_string().contains("wasm_memory_reservation_bytes"));
    }

    #[test]
    fn runner_bootstrap_validate_rejects_zero_wasm_memory_reservation_for_growth() {
        let mut bootstrap = valid_http_bootstrap();
        bootstrap.wasm_memory_reservation_for_growth_bytes = 0;

        let err = bootstrap
            .validate()
            .expect_err("bootstrap should reject zero wasm_memory_reservation_for_growth_bytes");
        assert!(
            err.to_string()
                .contains("wasm_memory_reservation_for_growth_bytes")
        );
    }

    #[test]
    fn wasi_http_outbound_rule_parse_normalizes_host_port_and_cidr() {
        let host_rule =
            WasiHttpOutboundRule::parse("LOCALHOST").expect("host rule should parse and normalize");
        assert_eq!(
            host_rule,
            WasiHttpOutboundRule::Host {
                host: "localhost".to_string()
            }
        );

        let host_port_rule = WasiHttpOutboundRule::parse("[::1]:443")
            .expect("host:port rule should parse and normalize");
        assert_eq!(
            host_port_rule,
            WasiHttpOutboundRule::HostPort {
                host: "::1".to_string(),
                port: 443
            }
        );

        let cidr_rule = WasiHttpOutboundRule::parse("10.1.2.3/8")
            .expect("CIDR rule should parse and normalize");
        assert_eq!(
            cidr_rule,
            WasiHttpOutboundRule::Cidr {
                network: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
                prefix_len: 8,
            }
        );
    }

    #[test]
    fn wasi_http_outbound_rule_parse_rejects_wildcard() {
        let err = WasiHttpOutboundRule::parse("*.example.com")
            .expect_err("wildcard rule should be rejected");
        assert!(err.contains("wildcard"));
    }

    #[test]
    fn wasi_http_outbound_rule_matches_authority() {
        let host = WasiHttpOutboundRule::Host {
            host: "localhost".to_string(),
        };
        assert!(host.matches_authority("LOCALHOST", 80));
        assert!(!host.matches_authority("example.com", 80));

        let host_port = WasiHttpOutboundRule::HostPort {
            host: "api.example.com".to_string(),
            port: 443,
        };
        assert!(host_port.matches_authority("API.EXAMPLE.COM", 443));
        assert!(!host_port.matches_authority("api.example.com", 80));

        let cidr = WasiHttpOutboundRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)),
            prefix_len: 8,
        };
        assert!(cidr.matches_authority("10.1.2.3", 443));
        assert!(!cidr.matches_authority("11.1.2.3", 443));
        assert!(
            !cidr.matches_authority("api.example.com", 443),
            "CIDR should not match non-IP hostnames"
        );
    }

    #[test]
    fn wasi_http_outbound_rule_cidr_cbor_round_trip() {
        let rule = WasiHttpOutboundRule::Cidr {
            network: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)),
            prefix_len: 8,
        };

        let encoded = imago_protocol::to_cbor(&rule).expect("CIDR rule encoding should work");
        let decoded = imago_protocol::from_cbor::<WasiHttpOutboundRule>(&encoded)
            .expect("CIDR rule decoding should work");

        assert_eq!(decoded, rule);
    }

    #[test]
    fn control_request_validate_accepts_remote_rpc_variants() {
        let connect = ControlRequest::RpcConnectRemote {
            runner_id: "runner-a".to_string(),
            manager_auth_proof: "proof".to_string(),
            authority: "rpc://node-a:4443".to_string(),
        };
        connect.validate().expect("connect request should be valid");

        let invoke = ControlRequest::RpcInvokeRemote {
            runner_id: "runner-a".to_string(),
            manager_auth_proof: "proof".to_string(),
            connection_id: "conn-1".to_string(),
            target_service: "svc-b".to_string(),
            interface_id: "pkg:iface/invoke".to_string(),
            function: "call".to_string(),
            args_cbor: vec![0x01],
        };
        invoke.validate().expect("invoke request should be valid");

        let disconnect = ControlRequest::RpcDisconnectRemote {
            runner_id: "runner-a".to_string(),
            manager_auth_proof: "proof".to_string(),
            connection_id: "conn-1".to_string(),
        };
        disconnect
            .validate()
            .expect("disconnect request should be valid");
    }

    #[test]
    fn control_request_validate_rejects_remote_rpc_with_empty_connection_id() {
        let request = ControlRequest::RpcInvokeRemote {
            runner_id: "runner-a".to_string(),
            manager_auth_proof: "proof".to_string(),
            connection_id: "".to_string(),
            target_service: "svc-b".to_string(),
            interface_id: "pkg:iface/invoke".to_string(),
            function: "call".to_string(),
            args_cbor: Vec::new(),
        };
        let err = request
            .validate()
            .expect_err("empty connection id should be rejected");
        assert!(err.to_string().contains("connection_id"));
    }

    #[test]
    fn control_response_validate_accepts_remote_rpc_variants() {
        ControlResponse::RpcRemoteConnected {
            connection_id: "conn-1".to_string(),
        }
        .validate()
        .expect("remote connected should be valid");

        ControlResponse::RpcRemoteInvokeResult {
            result_cbor: vec![0x01, 0x02],
        }
        .validate()
        .expect("remote invoke result should be valid");
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

    #[test]
    fn runner_socket_protocol_and_direction_allow_helpers_cover_all_variants() {
        assert!(RunnerSocketProtocol::Udp.allows_udp());
        assert!(!RunnerSocketProtocol::Udp.allows_tcp());
        assert!(!RunnerSocketProtocol::Tcp.allows_udp());
        assert!(RunnerSocketProtocol::Tcp.allows_tcp());
        assert!(RunnerSocketProtocol::Both.allows_udp());
        assert!(RunnerSocketProtocol::Both.allows_tcp());

        assert!(RunnerSocketDirection::Inbound.allows_inbound());
        assert!(!RunnerSocketDirection::Inbound.allows_outbound());
        assert!(!RunnerSocketDirection::Outbound.allows_inbound());
        assert!(RunnerSocketDirection::Outbound.allows_outbound());
        assert!(RunnerSocketDirection::Both.allows_inbound());
        assert!(RunnerSocketDirection::Both.allows_outbound());
    }

    #[test]
    fn runner_socket_config_validate_requires_port_only_when_inbound_is_enabled() {
        let inbound = RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Tcp,
            direction: RunnerSocketDirection::Inbound,
            listen_addr: "127.0.0.1".to_string(),
            listen_port: 0,
        };
        let err = inbound
            .validate()
            .expect_err("inbound socket config with port=0 should fail");
        assert!(err.to_string().contains("listen_port"));

        let outbound_only = RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Tcp,
            direction: RunnerSocketDirection::Outbound,
            listen_addr: "127.0.0.1".to_string(),
            listen_port: 0,
        };
        outbound_only
            .validate()
            .expect("outbound-only config may keep listen_port=0");
    }

    #[test]
    fn verify_manager_auth_proof_rejects_invalid_secret_encoding() {
        let err = verify_manager_auth_proof("not-hex", "runner-1", "deadbeef")
            .expect_err("invalid secret encoding should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, STAGE_TOKEN);
        assert!(err.message.contains("secret decode failed"));
    }

    #[test]
    fn issue_invocation_token_rejects_invalid_claims() {
        let secret = random_secret_hex();
        let claims = InvocationTokenClaims {
            source_service: "".to_string(),
            target_service: "svc-b".to_string(),
            wit: "pkg:iface/callable".to_string(),
            exp: now_unix_secs() + 60,
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let err =
            issue_invocation_token(&secret, claims).expect_err("invalid claims should fail issue");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, STAGE_TOKEN);
        assert!(err.message.contains("source_service"));
    }

    #[test]
    fn verify_invocation_token_rejects_malformed_signature_mismatch_and_expired() {
        let secret = random_secret_hex();
        let claims = InvocationTokenClaims {
            source_service: "svc-a".to_string(),
            target_service: "svc-b".to_string(),
            wit: "pkg:iface/callable".to_string(),
            exp: now_unix_secs() + 60,
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let token =
            issue_invocation_token(&secret, claims).expect("token issue should succeed first");

        let malformed = verify_invocation_token(&secret, "%%%not-base64%%%")
            .expect_err("malformed token should fail");
        assert_eq!(malformed.code, ErrorCode::Unauthorized);
        assert_eq!(malformed.stage, STAGE_TOKEN);
        assert!(malformed.message.contains("token decode failed"));

        let wrong_secret = random_secret_hex();
        let mismatch = verify_invocation_token(&wrong_secret, &token)
            .expect_err("signature mismatch should fail");
        assert_eq!(mismatch.code, ErrorCode::Unauthorized);
        assert_eq!(mismatch.stage, STAGE_TOKEN);
        assert!(mismatch.message.contains("token signature mismatch"));

        let expired_claims = InvocationTokenClaims {
            source_service: "svc-a".to_string(),
            target_service: "svc-b".to_string(),
            wit: "pkg:iface/callable".to_string(),
            exp: now_unix_secs().saturating_sub(1),
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let expired_token = issue_invocation_token(&secret, expired_claims)
            .expect("expired token payload still signs successfully");
        let expired = verify_invocation_token(&secret, &expired_token)
            .expect_err("expired token should fail verification");
        assert_eq!(expired.code, ErrorCode::Unauthorized);
        assert_eq!(expired.stage, STAGE_TOKEN);
        assert!(expired.message.contains("token expired"));
    }

    #[test]
    fn map_ipc_error_adds_ipc_stage_prefix() {
        let err = map_ipc_error("write", "io error");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, "ipc.write");
        assert_eq!(err.message, "io error");
    }
}
