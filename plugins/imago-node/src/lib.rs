#[cfg(test)]
use imago_protocol::Validate;
use imagod_runtime_wasmtime::{
    NativePluginContext, WasiState,
    native_plugins::{
        HasSelf, NativePlugin, NativePluginLinker, NativePluginResult,
        map_native_plugin_linker_error,
    },
    rpc_bridge::{self, RpcConnection},
};
use url::Url;
use wasmtime::component::Resource;

pub mod imago_node_plugin_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "host",
    });
}

#[derive(Debug, Default)]
pub struct ImagoNodePlugin;

type Connection = imago_node_plugin_bindings::imago::node::rpc::Connection;

const PACKAGE_NAME: &str = "imago:node";
const IMPORT_NAME: &str = "imago:node/rpc@0.1.0";
const SYMBOLS: &[&str] = &[
    "imago:node/rpc@0.1.0.connect",
    "imago:node/rpc@0.1.0.local",
    "imago:node/rpc@0.1.0.[method]connection.invoke",
    "imago:node/rpc@0.1.0.[method]connection.disconnect",
];
const DEFAULT_RPC_PORT: u16 = 4443;

impl NativePlugin for ImagoNodePlugin {
    fn package_name(&self) -> &'static str {
        PACKAGE_NAME
    }

    fn supports_import(&self, import_name: &str) -> bool {
        import_name == IMPORT_NAME
    }

    fn symbols(&self) -> &'static [&'static str] {
        SYMBOLS
    }

    fn supports_symbol(&self, symbol: &str) -> bool {
        symbol.starts_with("imago:node/rpc@0.1.0.")
    }

    fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()> {
        rpc_bridge::register_connection_rep_extractor(extract_connection_rep);
        imago_node_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
            .map_err(|err| map_native_plugin_linker_error(PACKAGE_NAME, err))
    }
}

fn extract_connection_rep(
    mut store: wasmtime::StoreContextMut<'_, WasiState>,
    resource_any: wasmtime::component::ResourceAny,
) -> Result<u32, String> {
    let resource = Resource::<Connection>::try_from_resource_any(resource_any, &mut store)
        .map_err(|err| format!("failed to lift connection resource: {err}"))?;
    Ok(resource.rep())
}

fn parse_remote_rpc_addr(addr: &str) -> Result<String, String> {
    let parsed = Url::parse(addr).map_err(|err| format!("invalid rpc address: {err}"))?;
    if parsed.scheme() != "rpc" {
        return Err("rpc address must use rpc:// scheme".to_string());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("rpc address must not contain userinfo".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("rpc address must not contain query or fragment".to_string());
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err("rpc address must not contain a path".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "rpc address host is required".to_string())?
        .to_ascii_lowercase();
    let port = parsed.port().unwrap_or(DEFAULT_RPC_PORT);
    if port == 0 {
        return Err("rpc address port must be greater than zero".to_string());
    }
    let host = format_host_for_url(&host);
    Ok(format!("rpc://{host}:{port}"))
}

fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn connect_remote_via_manager(
    context: &NativePluginContext,
    authority: &str,
) -> Result<RpcConnection, String> {
    rpc_bridge::connect_remote_via_manager(context, authority)
}

impl imago_node_plugin_bindings::imago::node::rpc::HostConnection for WasiState {
    fn invoke(
        &mut self,
        self_: Resource<Connection>,
        target_service: String,
        interface_id: String,
        function: String,
        args_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        rpc_bridge::invoke_connection(
            self.native_plugin_context(),
            self_.rep(),
            &target_service,
            &interface_id,
            &function,
            args_cbor,
        )
    }

    fn disconnect(&mut self, self_: Resource<Connection>) {
        rpc_bridge::disconnect_connection_best_effort(self.native_plugin_context(), self_.rep());
        let _ = rpc_bridge::unregister_connection(self_.rep());
    }

    fn drop(&mut self, resource: Resource<Connection>) -> wasmtime::Result<()> {
        rpc_bridge::disconnect_connection_best_effort(self.native_plugin_context(), resource.rep());
        let _ = rpc_bridge::unregister_connection(resource.rep());
        Ok(())
    }
}

impl imago_node_plugin_bindings::imago::node::rpc::Host for WasiState {
    fn connect(&mut self, addr: String) -> Result<Resource<Connection>, String> {
        let authority = parse_remote_rpc_addr(&addr)?;
        let connection = connect_remote_via_manager(self.native_plugin_context(), &authority)?;
        Ok(Resource::new_own(rpc_bridge::allocate_connection_rep(
            connection,
        )))
    }

    fn local(&mut self) -> Result<Resource<Connection>, String> {
        Ok(Resource::new_own(rpc_bridge::allocate_connection_rep(
            RpcConnection::LocalUds,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::{
        ControlRequest, ControlResponse, RunnerAppType, RunnerInboundRequest,
        RunnerInboundResponse, random_secret_hex, verify_manager_auth_proof,
    };
    use std::{
        io::{Read, Write},
        net::Shutdown,
        os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream},
        path::PathBuf,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    const TEST_MAX_IPC_FRAME_BYTES: usize = 1024 * 1024 * 8;

    #[test]
    fn parse_remote_rpc_addr_uses_default_port() {
        let parsed = parse_remote_rpc_addr("rpc://example.com").expect("address should parse");
        assert_eq!(parsed, "rpc://example.com:4443");
    }

    #[test]
    fn parse_remote_rpc_addr_accepts_explicit_port() {
        let parsed = parse_remote_rpc_addr("rpc://127.0.0.1:5555").expect("address should parse");
        assert_eq!(parsed, "rpc://127.0.0.1:5555");
    }

    #[test]
    fn parse_remote_rpc_addr_rejects_non_rpc_scheme() {
        let err = parse_remote_rpc_addr("https://example.com").expect_err("scheme should fail");
        assert!(err.contains("rpc://"), "unexpected error: {err}");
    }

    #[test]
    fn parse_remote_rpc_addr_rejects_path_or_query() {
        let path_err =
            parse_remote_rpc_addr("rpc://example.com/path").expect_err("path should be rejected");
        assert!(path_err.contains("path"), "unexpected error: {path_err}");

        let query_err =
            parse_remote_rpc_addr("rpc://example.com?x=1").expect_err("query should be rejected");
        assert!(
            query_err.contains("query or fragment"),
            "unexpected error: {query_err}"
        );
    }

    fn temp_socket_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        PathBuf::from(format!(
            "/tmp/imago-node-{label}-{}-{nanos}.sock",
            std::process::id()
        ))
    }

    fn write_message_sync<T>(stream: &mut StdUnixStream, value: &T) -> Result<(), String>
    where
        T: serde::Serialize + Validate,
    {
        value
            .validate()
            .map_err(|err| format!("ipc request validation failed: {err}"))?;
        let payload = imago_protocol::to_cbor(value)
            .map_err(|err| format!("ipc message encode failed: {err}"))?;
        if payload.len() > TEST_MAX_IPC_FRAME_BYTES {
            return Err(format!("ipc frame too large: {} bytes", payload.len()));
        }
        let len = u32::try_from(payload.len())
            .map_err(|_| format!("ipc frame size overflow: {} bytes", payload.len()))?;
        stream
            .write_all(&len.to_be_bytes())
            .map_err(|err| format!("ipc length write failed: {err}"))?;
        stream
            .write_all(&payload)
            .map_err(|err| format!("ipc payload write failed: {err}"))?;
        Ok(())
    }

    fn read_message_sync<T>(stream: &mut StdUnixStream) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned + Validate,
    {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|err| format!("ipc length read failed: {err}"))?;
        let len = usize::try_from(u32::from_be_bytes(len_buf))
            .map_err(|_| "ipc frame size conversion failed".to_string())?;
        if len > TEST_MAX_IPC_FRAME_BYTES {
            return Err(format!("ipc frame too large: {len} bytes"));
        }
        let mut payload = vec![0u8; len];
        stream
            .read_exact(&mut payload)
            .map_err(|err| format!("ipc payload read failed: {err}"))?;
        let response: T = imago_protocol::from_cbor(&payload)
            .map_err(|err| format!("ipc message decode failed: {err}"))?;
        response
            .validate()
            .map_err(|err| format!("ipc response validation failed: {err}"))?;
        Ok(response)
    }

    #[test]
    fn invoke_local_uds_routes_via_manager_and_runner_sockets() {
        let manager_secret = random_secret_hex();
        let manager_socket = temp_socket_path("manager");
        let runner_socket = temp_socket_path("runner");
        let _ = std::fs::remove_file(&manager_socket);
        let _ = std::fs::remove_file(&runner_socket);

        let manager_listener =
            StdUnixListener::bind(&manager_socket).expect("manager listener should bind");
        let runner_listener =
            StdUnixListener::bind(&runner_socket).expect("runner listener should bind");

        let runner_socket_for_manager = runner_socket.clone();
        let manager_secret_for_thread = manager_secret.clone();
        let manager_thread = thread::spawn(move || {
            let (mut stream, _) = manager_listener
                .accept()
                .expect("manager accept should succeed");
            let request = read_message_sync::<ControlRequest>(&mut stream)
                .expect("control request should decode");
            match request {
                ControlRequest::ResolveInvocationTarget {
                    runner_id,
                    manager_auth_proof,
                    target_service,
                    wit,
                } => {
                    assert_eq!(runner_id, "runner-source");
                    verify_manager_auth_proof(
                        &manager_secret_for_thread,
                        &runner_id,
                        &manager_auth_proof,
                    )
                    .expect("manager auth proof should verify");
                    assert_eq!(target_service, "svc-target");
                    assert_eq!(wit, "yieldspace:svc/invoke");
                }
                other => panic!("unexpected control request: {other:?}"),
            }
            write_message_sync(
                &mut stream,
                &ControlResponse::ResolvedInvocationTarget {
                    endpoint: runner_socket_for_manager,
                    token: "token-1".to_string(),
                },
            )
            .expect("control response should encode");
            stream
                .shutdown(Shutdown::Write)
                .expect("manager write shutdown should succeed");
        });

        let runner_thread = thread::spawn(move || {
            let (mut stream, _) = runner_listener
                .accept()
                .expect("runner accept should succeed");
            let request = read_message_sync::<RunnerInboundRequest>(&mut stream)
                .expect("runner request should decode");
            match request {
                RunnerInboundRequest::Invoke {
                    interface_id,
                    function,
                    payload_cbor,
                    token,
                } => {
                    assert_eq!(interface_id, "yieldspace:svc/invoke");
                    assert_eq!(function, "call");
                    assert_eq!(payload_cbor, vec![0x01, 0x02]);
                    assert_eq!(token, "token-1");
                }
                other => panic!("unexpected runner request: {other:?}"),
            }
            write_message_sync(
                &mut stream,
                &RunnerInboundResponse::InvokeResult {
                    payload_cbor: vec![0xAA, 0xBB],
                },
            )
            .expect("runner response should encode");
            stream
                .shutdown(Shutdown::Write)
                .expect("runner write shutdown should succeed");
        });

        let context = NativePluginContext::new(
            "svc-source".to_string(),
            "release-1".to_string(),
            "runner-source".to_string(),
            RunnerAppType::Rpc,
            manager_socket.clone(),
            manager_secret,
            std::collections::BTreeMap::new(),
        );
        let connection_rep = rpc_bridge::allocate_connection_rep(RpcConnection::LocalUds);
        let actual = rpc_bridge::invoke_connection(
            &context,
            connection_rep,
            "svc-target",
            "yieldspace:svc/invoke",
            "call",
            vec![0x01, 0x02],
        )
        .expect("local invoke should succeed");
        assert_eq!(actual, vec![0xAA, 0xBB]);
        let _ = rpc_bridge::unregister_connection(connection_rep);

        manager_thread
            .join()
            .expect("manager server thread should join");
        runner_thread
            .join()
            .expect("runner server thread should join");
        let _ = std::fs::remove_file(manager_socket);
        let _ = std::fs::remove_file(runner_socket);
    }
}
