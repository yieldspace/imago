//! Wasmtime runtime integration used by runner processes.

pub mod native_plugins;

mod capability_checker;
mod http_supervisor;
mod plugin_resolver;
pub mod rpc_bridge;
mod rpc_values;
mod runtime_entry;
mod wasi_nn;

use std::{collections::BTreeMap, error::Error as _, sync::Arc};

use http_body_util::BodyExt;
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::{ResourceMap, RunnerAppType, WasiHttpOutboundRule};
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;
use wasmtime::component::{ResourceAny, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    io::TokioIo,
    p2::{
        HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
        bindings::http::types::{DnsErrorPayload, ErrorCode as WasiHttpErrorCode},
        body::HyperOutgoingBody,
        types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
    },
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WasmDependencyResourceKey {
    pub(crate) dependency_name: String,
    pub(crate) interface_name: String,
    pub(crate) resource_name: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredWasmDependencyResource {
    pub(crate) type_id: u32,
    pub(crate) resource: ResourceAny,
}

#[derive(Debug)]
pub(crate) struct WasmDependencyResourceState {
    next_type_id: u32,
    next_rep: u32,
    types: BTreeMap<WasmDependencyResourceKey, u32>,
    resources: BTreeMap<u32, StoredWasmDependencyResource>,
}

impl Default for WasmDependencyResourceState {
    fn default() -> Self {
        Self {
            next_type_id: 1,
            next_rep: 1,
            types: BTreeMap::new(),
            resources: BTreeMap::new(),
        }
    }
}

impl WasmDependencyResourceState {
    fn type_id_for(&mut self, key: WasmDependencyResourceKey) -> u32 {
        if let Some(type_id) = self.types.get(&key) {
            return *type_id;
        }

        let type_id = self.next_type_id;
        self.next_type_id = self
            .next_type_id
            .checked_add(1)
            .expect("wasm dependency resource type ids exhausted");
        self.types.insert(key, type_id);
        type_id
    }

    fn insert_resource(&mut self, type_id: u32, resource: ResourceAny) -> Result<u32, ImagodError> {
        let rep = self.next_rep;
        self.next_rep = self.next_rep.checked_add(1).ok_or_else(|| {
            map_runtime_error("wasm dependency resource ids exhausted".to_string())
        })?;
        self.resources
            .insert(rep, StoredWasmDependencyResource { type_id, resource });
        Ok(rep)
    }

    fn resource(&self, rep: u32) -> Option<StoredWasmDependencyResource> {
        self.resources.get(&rep).copied()
    }

    fn remove_resource(&mut self, rep: u32) -> Option<StoredWasmDependencyResource> {
        self.resources.remove(&rep)
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
    pub(crate) http_hooks: WasiHttpOutboundHooks,
    pub(crate) native_plugin_context: NativePluginContext,
    pub(crate) wasm_dependency_resources: WasmDependencyResourceState,
}

#[derive(Debug)]
pub(crate) struct WasiHttpOutboundHooks {
    rules: Vec<WasiHttpOutboundRule>,
}

impl WasiHttpOutboundHooks {
    fn new(rules: Vec<WasiHttpOutboundRule>) -> Self {
        Self { rules }
    }
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
            http_hooks: WasiHttpOutboundHooks::new(wasi_http_outbound),
            native_plugin_context,
            wasm_dependency_resources: WasmDependencyResourceState::default(),
        }
    }

    pub fn native_plugin_context(&self) -> &NativePluginContext {
        &self.native_plugin_context
    }

    pub(crate) fn wasm_dependency_resource_type_id(
        &mut self,
        key: WasmDependencyResourceKey,
    ) -> u32 {
        self.wasm_dependency_resources.type_id_for(key)
    }

    pub(crate) fn store_wasm_dependency_resource(
        &mut self,
        type_id: u32,
        resource: ResourceAny,
    ) -> Result<u32, ImagodError> {
        self.wasm_dependency_resources
            .insert_resource(type_id, resource)
    }

    pub(crate) fn wasm_dependency_resource(
        &self,
        rep: u32,
    ) -> Option<StoredWasmDependencyResource> {
        self.wasm_dependency_resources.resource(rep)
    }

    pub(crate) fn remove_wasm_dependency_resource(
        &mut self,
        rep: u32,
    ) -> Option<StoredWasmDependencyResource> {
        self.wasm_dependency_resources.remove_resource(rep)
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
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl WasiHttpHooks for WasiHttpOutboundHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let Some(authority) = request.uri().authority() else {
            return Err(WasiHttpErrorCode::HttpRequestUriInvalid.into());
        };
        let request_host = authority.host();
        let request_port = authority
            .port_u16()
            .unwrap_or(if config.use_tls { 443 } else { 80 });
        if is_http_outbound_allowed(&self.rules, request_host, request_port) {
            Ok(spawn_outgoing_request(request, config))
        } else {
            Err(WasiHttpErrorCode::HttpRequestDenied.into())
        }
    }
}

fn spawn_outgoing_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HostFutureIncomingResponse {
    let handle =
        wasmtime_wasi::runtime::spawn(
            async move { Ok(send_outgoing_request(request, config).await) },
        );
    HostFutureIncomingResponse::pending(handle)
}

async fn send_outgoing_request(
    mut request: hyper::Request<HyperOutgoingBody>,
    OutgoingRequestConfig {
        use_tls,
        connect_timeout,
        first_byte_timeout,
        between_bytes_timeout,
    }: OutgoingRequestConfig,
) -> Result<IncomingResponse, WasiHttpErrorCode> {
    let authority = request
        .uri()
        .authority()
        .ok_or(WasiHttpErrorCode::HttpRequestUriInvalid)?;
    let host = authority.host().to_string();
    let connect_authority = if authority.port().is_some() {
        authority.as_str().to_string()
    } else {
        let port = if use_tls { 443 } else { 80 };
        if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        }
    };

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&connect_authority))
        .await
        .map_err(|_| WasiHttpErrorCode::ConnectionTimeout)?
        .map_err(|err| map_connect_error(&err))?;

    let (mut sender, worker) = if use_tls {
        let root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let domain = rustls::pki_types::ServerName::try_from(host.as_str())
            .map_err(|_| dns_error("invalid dns name", 0))?
            .to_owned();
        let stream = connector
            .connect(domain, tcp_stream)
            .await
            .map_err(|_| WasiHttpErrorCode::TlsProtocolError)?;
        let stream = TokioIo::new(stream);

        let (sender, conn) = timeout(
            connect_timeout,
            hyper::client::conn::http1::handshake(stream),
        )
        .await
        .map_err(|_| WasiHttpErrorCode::ConnectionTimeout)?
        .map_err(map_hyper_request_error)?;

        let worker = wasmtime_wasi::runtime::spawn(async move {
            let _ = conn.await;
        });

        (sender, worker)
    } else {
        let stream = TokioIo::new(tcp_stream);
        let (sender, conn) = timeout(
            connect_timeout,
            hyper::client::conn::http1::handshake(stream),
        )
        .await
        .map_err(|_| WasiHttpErrorCode::ConnectionTimeout)?
        .map_err(map_hyper_request_error)?;

        let worker = wasmtime_wasi::runtime::spawn(async move {
            let _ = conn.await;
        });

        (sender, worker)
    };

    *request.uri_mut() = hyper::Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|path| path.as_str())
                .unwrap_or("/"),
        )
        .build()
        .expect("comes from valid request");

    let resp = timeout(first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| WasiHttpErrorCode::ConnectionReadTimeout)?
        .map_err(map_hyper_request_error)?
        .map(|body| body.map_err(map_hyper_request_error).boxed_unsync());

    Ok(IncomingResponse {
        resp,
        worker: Some(worker),
        between_bytes_timeout,
    })
}

fn map_connect_error(err: &std::io::Error) -> WasiHttpErrorCode {
    match err.kind() {
        std::io::ErrorKind::AddrNotAvailable => dns_error("address not available", 0),
        _ => {
            if err
                .to_string()
                .starts_with("failed to lookup address information")
            {
                dns_error("address not available", 0)
            } else {
                WasiHttpErrorCode::ConnectionRefused
            }
        }
    }
}

fn dns_error(rcode: &str, info_code: u16) -> WasiHttpErrorCode {
    WasiHttpErrorCode::DnsError(DnsErrorPayload {
        rcode: Some(rcode.to_string()),
        info_code: Some(info_code),
    })
}

fn map_hyper_request_error(err: hyper::Error) -> WasiHttpErrorCode {
    if let Some(cause) = err.source()
        && let Some(code) = cause.downcast_ref::<WasiHttpErrorCode>()
    {
        return code.clone();
    }

    WasiHttpErrorCode::HttpProtocolError
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
    use wasmtime_wasi::p2::Pollable;
    use wasmtime_wasi_http::p2::{
        WasiHttpHooks, bindings::http::types::ErrorCode as WasiHttpErrorCode,
    };

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
            .http_hooks
            .send_request(request, config)
            .expect_err("request must be denied by outbound allowlist");
        assert!(matches!(
            err.downcast_ref(),
            Some(code) if matches!(code, WasiHttpErrorCode::HttpRequestDenied)
        ));
    }

    #[test]
    fn send_request_allows_allowlisted_http_authority() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("listener should bind");
                let addr = listener.local_addr().expect("listener addr");
                let server = tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};

                    let (mut stream, _) = listener.accept().await.expect("accept should succeed");
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await.expect("request should be read");
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await
                        .expect("response should be written");
                });

                let mut state = WasiState::new(
                    WasiCtx::builder().build(),
                    WasiHttpCtx::new(),
                    vec![WasiHttpOutboundRule::HostPort {
                        host: "127.0.0.1".to_string(),
                        port: addr.port(),
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
                    .uri(format!("http://127.0.0.1:{}/", addr.port()))
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

                let mut response = state
                    .http_hooks
                    .send_request(request, config)
                    .expect("request should be allowed");
                response.ready().await;
                let response = response
                    .unwrap_ready()
                    .expect("request should not trap")
                    .expect("request should succeed");
                let status = response.resp.status();
                let body = response
                    .resp
                    .into_body()
                    .collect()
                    .await
                    .expect("body should be collected")
                    .to_bytes();
                server.await.expect("server task should complete");

                assert_eq!(status, hyper::StatusCode::OK);
                assert_eq!(body.as_ref(), b"ok");
            });
    }
}
