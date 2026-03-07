//! Wasmtime runtime integration used by runner processes.

pub mod native_plugins;

mod capability_checker;
mod http_supervisor;
mod plugin_resolver;
pub mod rpc_bridge;
mod rpc_values;
mod runtime_entry;
mod wasi_nn;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{ResourceMap, RunnerAppType, WasiHttpOutboundRule};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx, WasiHttpView,
    bindings::http::types::ErrorCode as WasiHttpErrorCode,
    types::{HostFutureIncomingResponse, OutgoingRequestConfig, default_send_request},
};

pub use native_plugins::{NativePlugin, NativePluginRegistry, NativePluginRegistryBuilder};
pub use runtime_entry::{WasmEngineTuning, WasmRuntime};

pub(crate) const STAGE_RUNTIME: &str = "runtime.start";
pub(crate) const HTTP_REQUEST_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePluginContext {
    service_name: String,
    release_hash: String,
    runner_id: String,
    app_type: String,
    manager_control_endpoint: std::path::PathBuf,
    manager_auth_secret: String,
    resources: ResourceMap,
}

impl NativePluginContext {
    pub fn new(
        service_name: String,
        release_hash: String,
        runner_id: String,
        app_type: RunnerAppType,
        manager_control_endpoint: std::path::PathBuf,
        manager_auth_secret: String,
        resources: ResourceMap,
    ) -> Self {
        Self {
            service_name,
            release_hash,
            runner_id,
            app_type: app_type_text(app_type).to_string(),
            manager_control_endpoint,
            manager_auth_secret,
            resources,
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn release_hash(&self) -> &str {
        &self.release_hash
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    pub fn app_type(&self) -> &str {
        &self.app_type
    }

    pub fn manager_control_endpoint(&self) -> &std::path::Path {
        &self.manager_control_endpoint
    }

    pub fn manager_auth_secret(&self) -> &str {
        &self.manager_auth_secret
    }

    pub fn resources(&self) -> &ResourceMap {
        &self.resources
    }
}

pub fn app_type_text(app_type: RunnerAppType) -> &'static str {
    match app_type {
        RunnerAppType::Cli => "cli",
        RunnerAppType::Rpc => "rpc",
        RunnerAppType::Http => "http",
        RunnerAppType::Socket => "socket",
    }
}

/// Internal WASI host state stored in the Wasmtime store.
pub struct WasiState {
    pub(crate) table: ResourceTable,
    pub(crate) wasi: WasiCtx,
    pub(crate) http: WasiHttpCtx,
    pub(crate) wasi_nn: wasmtime_wasi_nn::wit::WasiNnCtx,
    pub(crate) wasi_http_outbound: Vec<WasiHttpOutboundRule>,
    pub(crate) native_plugin_context: NativePluginContext,
}

impl WasiState {
    pub(crate) fn new(
        wasi: WasiCtx,
        http: WasiHttpCtx,
        wasi_http_outbound: Vec<WasiHttpOutboundRule>,
        native_plugin_context: NativePluginContext,
    ) -> Self {
        Self {
            table: ResourceTable::new(),
            wasi,
            http,
            wasi_nn: wasi_nn::new_context(),
            wasi_http_outbound,
            native_plugin_context,
        }
    }

    pub fn native_plugin_context(&self) -> &NativePluginContext {
        &self.native_plugin_context
    }
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WasiState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<HostFutureIncomingResponse> {
        let Some(authority) = request.uri().authority() else {
            return Err(WasiHttpErrorCode::HttpRequestUriInvalid.into());
        };
        let request_host = authority.host();
        let request_port = authority
            .port_u16()
            .unwrap_or(if config.use_tls { 443 } else { 80 });
        if is_http_outbound_allowed(&self.wasi_http_outbound, request_host, request_port) {
            Ok(default_send_request(request, config))
        } else {
            Err(WasiHttpErrorCode::HttpRequestDenied.into())
        }
    }
}

fn is_http_outbound_allowed(
    rules: &[WasiHttpOutboundRule],
    request_host: &str,
    request_port: u16,
) -> bool {
    rules
        .iter()
        .any(|rule| rule.matches_authority(request_host, request_port))
}

pub(crate) fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}

pub(crate) fn map_runtime_unauthorized_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Unauthorized, STAGE_RUNTIME, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    use imagod_ipc::RunnerAppType;
    use std::time::Duration;
    use wasmtime_wasi_http::bindings::http::types::ErrorCode as WasiHttpErrorCode;

    #[test]
    fn native_plugin_app_type_text_is_stable() {
        assert_eq!(app_type_text(RunnerAppType::Cli), "cli");
        assert_eq!(app_type_text(RunnerAppType::Rpc), "rpc");
        assert_eq!(app_type_text(RunnerAppType::Http), "http");
        assert_eq!(app_type_text(RunnerAppType::Socket), "socket");
    }

    #[test]
    fn native_plugin_context_stores_runner_metadata() {
        let context = NativePluginContext::new(
            "svc-test".to_string(),
            "release-test".to_string(),
            "runner-test".to_string(),
            RunnerAppType::Http,
            std::path::PathBuf::from("/tmp/manager.sock"),
            "secret".to_string(),
            std::collections::BTreeMap::from([(
                "i2c".to_string(),
                serde_json::json!({ "allowed_buses": ["/dev/i2c-1"] }),
            )]),
        );
        assert_eq!(context.service_name(), "svc-test");
        assert_eq!(context.release_hash(), "release-test");
        assert_eq!(context.runner_id(), "runner-test");
        assert_eq!(context.app_type(), "http");
        assert_eq!(
            context.manager_control_endpoint(),
            std::path::Path::new("/tmp/manager.sock")
        );
        assert_eq!(context.manager_auth_secret(), "secret");
        assert_eq!(
            context.resources().get("i2c"),
            Some(&serde_json::json!({ "allowed_buses": ["/dev/i2c-1"] }))
        );
    }

    #[test]
    fn http_outbound_matcher_supports_host_host_port_and_cidr() {
        let rules = vec![
            WasiHttpOutboundRule::Host {
                host: "api.example.com".to_string(),
            },
            WasiHttpOutboundRule::HostPort {
                host: "secure.example.com".to_string(),
                port: 443,
            },
            WasiHttpOutboundRule::Cidr {
                network: "10.0.0.0".parse().expect("valid CIDR network"),
                prefix_len: 8,
            },
        ];
        assert!(is_http_outbound_allowed(&rules, "API.EXAMPLE.COM", 80));
        assert!(is_http_outbound_allowed(&rules, "secure.example.com", 443));
        assert!(!is_http_outbound_allowed(&rules, "secure.example.com", 80));
        assert!(is_http_outbound_allowed(&rules, "10.1.2.3", 8080));
        assert!(
            !is_http_outbound_allowed(&rules, "www.example.net", 8080),
            "CIDR rule must not match non-IP hosts"
        );
    }

    #[test]
    fn http_outbound_matcher_accepts_default_localhost_entries() {
        let rules = vec![
            WasiHttpOutboundRule::Host {
                host: "localhost".to_string(),
            },
            WasiHttpOutboundRule::Host {
                host: "127.0.0.1".to_string(),
            },
            WasiHttpOutboundRule::Host {
                host: "::1".to_string(),
            },
        ];
        assert!(is_http_outbound_allowed(&rules, "LOCALHOST", 80));
        assert!(is_http_outbound_allowed(&rules, "127.0.0.1", 80));
        assert!(is_http_outbound_allowed(&rules, "::1", 80));
    }

    #[test]
    fn send_request_rejects_non_allowlisted_authority_with_http_request_denied() {
        let mut state = WasiState::new(
            WasiCtx::builder().build(),
            WasiHttpCtx::new(),
            vec![WasiHttpOutboundRule::Host {
                host: "localhost".to_string(),
            }],
            NativePluginContext::new(
                "svc-test".to_string(),
                "release-test".to_string(),
                "runner-test".to_string(),
                RunnerAppType::Cli,
                std::path::PathBuf::from("/tmp/manager.sock"),
                "secret".to_string(),
                std::collections::BTreeMap::new(),
            ),
        );
        let request = hyper::Request::builder()
            .uri("http://example.com/")
            .body(
                Empty::<Bytes>::new()
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .expect("request should be built");
        let config = OutgoingRequestConfig {
            use_tls: false,
            connect_timeout: Duration::from_secs(1),
            first_byte_timeout: Duration::from_secs(1),
            between_bytes_timeout: Duration::from_secs(1),
        };

        let err = state
            .send_request(request, config)
            .expect_err("request must be denied by outbound allowlist");
        assert!(matches!(
            err.downcast_ref(),
            Some(code) if matches!(code, WasiHttpErrorCode::HttpRequestDenied)
        ));
    }
}
