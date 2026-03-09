use std::{
    collections::BTreeMap,
    sync::{
        LazyLock, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};

use imagod_ipc::compute_manager_auth_proof;
use imagod_spec::{ControlRequest, ControlResponse, RunnerInboundRequest, RunnerInboundResponse};

#[cfg(unix)]
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream as StdUnixStream,
    path::Path,
};

#[cfg(unix)]
use imago_protocol::{from_cbor, to_cbor};
#[cfg(unix)]
use imagod_spec::Validate;
#[cfg(unix)]
use serde::{Serialize, de::DeserializeOwned};
use wasmtime::{
    StoreContextMut,
    component::{ResourceAny, Val},
};

use crate::{NativePluginContext, WasiState, map_runtime_error};

const MAX_IPC_FRAME_BYTES: usize = 1024 * 1024 * 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcConnection {
    LocalUds,
    RemoteManager {
        authority: String,
        connection_id: String,
    },
}

static NEXT_CONNECTION_REP: AtomicU32 = AtomicU32::new(1);
static CONNECTIONS: LazyLock<Mutex<BTreeMap<u32, RpcConnection>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
type ConnectionRepExtractor =
    for<'a> fn(StoreContextMut<'a, WasiState>, ResourceAny) -> Result<u32, String>;
static CONNECTION_REP_EXTRACTOR: OnceLock<ConnectionRepExtractor> = OnceLock::new();

pub fn register_connection_rep_extractor(extractor: ConnectionRepExtractor) {
    let _ = CONNECTION_REP_EXTRACTOR.set(extractor);
}

pub fn extract_connection_rep(
    store: StoreContextMut<'_, WasiState>,
    connection_value: &Val,
) -> Result<u32, imagod_common::ImagodError> {
    let Val::Resource(resource_any) = connection_value else {
        return Err(map_runtime_error(
            "binding shim first runtime argument must be a connection resource".to_string(),
        ));
    };
    let extractor = CONNECTION_REP_EXTRACTOR.get().ok_or_else(|| {
        map_runtime_error("connection resource extractor is not registered".to_string())
    })?;
    extractor(store, *resource_any).map_err(|message| {
        map_runtime_error(format!("failed to decode connection handle: {message}"))
    })
}

pub fn allocate_connection_rep(connection: RpcConnection) -> u32 {
    let rep = NEXT_CONNECTION_REP.fetch_add(1, Ordering::Relaxed);
    let mut guard = CONNECTIONS
        .lock()
        .expect("connection registry mutex poisoned");
    guard.insert(rep, connection);
    rep
}

pub fn lookup_connection(rep: u32) -> Option<RpcConnection> {
    let guard = CONNECTIONS
        .lock()
        .expect("connection registry mutex poisoned");
    guard.get(&rep).cloned()
}

pub fn unregister_connection(rep: u32) -> Option<RpcConnection> {
    let mut guard = CONNECTIONS
        .lock()
        .expect("connection registry mutex poisoned");
    guard.remove(&rep)
}

pub fn invoke_connection(
    context: &NativePluginContext,
    connection_rep: u32,
    target_service: &str,
    interface_id: &str,
    function: &str,
    args_cbor: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let connection = lookup_connection(connection_rep)
        .ok_or_else(|| format!("rpc connection {connection_rep} is not available"))?;
    match connection {
        RpcConnection::LocalUds => {
            invoke_local_uds(context, target_service, interface_id, function, args_cbor)
        }
        RpcConnection::RemoteManager {
            authority: _,
            connection_id,
        } => invoke_remote_via_manager(
            context,
            &connection_id,
            target_service,
            interface_id,
            function,
            args_cbor,
        ),
    }
}

pub fn disconnect_connection_best_effort(context: &NativePluginContext, connection_rep: u32) {
    let Some(RpcConnection::RemoteManager { connection_id, .. }) =
        lookup_connection(connection_rep)
    else {
        return;
    };
    disconnect_remote_via_manager(context, &connection_id);
}

pub fn connect_remote_via_manager(
    context: &NativePluginContext,
    authority: &str,
) -> Result<RpcConnection, String> {
    let manager_auth_proof =
        compute_manager_auth_proof(context.manager_auth_secret(), context.runner_id())
            .map_err(|err| format!("failed to compute manager auth proof: {err}"))?;
    let response = call_ipc_sync::<ControlRequest, ControlResponse>(
        context.manager_control_endpoint(),
        &ControlRequest::RpcConnectRemote {
            runner_id: context.runner_id().to_string(),
            manager_auth_proof,
            authority: authority.to_string(),
        },
    )?;
    match response {
        ControlResponse::RpcRemoteConnected { connection_id } => Ok(RpcConnection::RemoteManager {
            authority: authority.to_string(),
            connection_id,
        }),
        ControlResponse::Error(err) => Err(format!(
            "remote connect failed: {:?} {} {}",
            err.code, err.stage, err.message
        )),
        other => Err(format!("unexpected remote connect response: {other:?}")),
    }
}

#[cfg(unix)]
fn write_message_sync<T>(stream: &mut StdUnixStream, value: &T) -> Result<(), String>
where
    T: Serialize + Validate,
{
    value
        .validate()
        .map_err(|err| format!("ipc request validation failed: {err}"))?;
    let payload = to_cbor(value).map_err(|err| format!("ipc message encode failed: {err}"))?;
    if payload.len() > MAX_IPC_FRAME_BYTES {
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

#[cfg(unix)]
fn read_message_sync<T>(stream: &mut StdUnixStream) -> Result<T, String>
where
    T: DeserializeOwned + Validate,
{
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|err| format!("ipc length read failed: {err}"))?;
    let len = usize::try_from(u32::from_be_bytes(len_buf))
        .map_err(|_| "ipc frame size conversion failed".to_string())?;
    if len > MAX_IPC_FRAME_BYTES {
        return Err(format!("ipc frame too large: {len} bytes"));
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|err| format!("ipc payload read failed: {err}"))?;
    let response: T =
        from_cbor(&payload).map_err(|err| format!("ipc message decode failed: {err}"))?;
    response
        .validate()
        .map_err(|err| format!("ipc response validation failed: {err}"))?;
    Ok(response)
}

#[cfg(unix)]
fn call_ipc_sync<Req, Resp>(endpoint: &Path, request: &Req) -> Result<Resp, String>
where
    Req: Serialize + Validate,
    Resp: DeserializeOwned + Validate,
{
    let mut stream = StdUnixStream::connect(endpoint)
        .map_err(|err| format!("failed to connect to {}: {err}", endpoint.display()))?;
    write_message_sync(&mut stream, request)?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|err| format!("failed to flush request stream: {err}"))?;
    read_message_sync::<Resp>(&mut stream)
}

#[cfg(not(unix))]
fn call_ipc_sync<Req, Resp>(_endpoint: &std::path::Path, _request: &Req) -> Result<Resp, String>
where
    Req: serde::Serialize + imagod_spec::Validate,
    Resp: serde::de::DeserializeOwned + imagod_spec::Validate,
{
    Err("rpc bridge is only supported on unix targets".to_string())
}

fn invoke_local_uds(
    context: &NativePluginContext,
    target_service: &str,
    interface_id: &str,
    function: &str,
    args_cbor: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let manager_auth_proof =
        compute_manager_auth_proof(context.manager_auth_secret(), context.runner_id())
            .map_err(|err| format!("failed to compute manager auth proof: {err}"))?;
    let resolved = call_ipc_sync::<ControlRequest, ControlResponse>(
        context.manager_control_endpoint(),
        &ControlRequest::ResolveInvocationTarget {
            runner_id: context.runner_id().to_string(),
            manager_auth_proof,
            target_service: target_service.to_string(),
            wit: interface_id.to_string(),
        },
    )?;
    let (endpoint, token) = match resolved {
        ControlResponse::ResolvedInvocationTarget { endpoint, token } => (endpoint, token),
        ControlResponse::Error(err) => {
            return Err(format!(
                "resolve invocation target failed: {:?} {} {}",
                err.code, err.stage, err.message
            ));
        }
        other => {
            return Err(format!(
                "unexpected resolve invocation target response: {other:?}"
            ));
        }
    };
    let invoke_response = call_ipc_sync::<RunnerInboundRequest, RunnerInboundResponse>(
        &endpoint,
        &RunnerInboundRequest::Invoke {
            interface_id: interface_id.to_string(),
            function: function.to_string(),
            payload_cbor: args_cbor,
            token,
        },
    )?;
    match invoke_response {
        RunnerInboundResponse::InvokeResult { payload_cbor } => Ok(payload_cbor),
        RunnerInboundResponse::Error(err) => Err(format!(
            "runner invoke failed: {:?} {} {}",
            err.code, err.stage, err.message
        )),
        other => Err(format!("unexpected runner invoke response: {other:?}")),
    }
}

fn invoke_remote_via_manager(
    context: &NativePluginContext,
    connection_id: &str,
    target_service: &str,
    interface_id: &str,
    function: &str,
    args_cbor: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let manager_auth_proof =
        compute_manager_auth_proof(context.manager_auth_secret(), context.runner_id())
            .map_err(|err| format!("failed to compute manager auth proof: {err}"))?;
    let response = call_ipc_sync::<ControlRequest, ControlResponse>(
        context.manager_control_endpoint(),
        &ControlRequest::RpcInvokeRemote {
            runner_id: context.runner_id().to_string(),
            manager_auth_proof,
            connection_id: connection_id.to_string(),
            target_service: target_service.to_string(),
            interface_id: interface_id.to_string(),
            function: function.to_string(),
            args_cbor,
        },
    )?;
    match response {
        ControlResponse::RpcRemoteInvokeResult { result_cbor } => Ok(result_cbor),
        ControlResponse::Error(err) => Err(format!(
            "remote invoke failed: {:?} {} {}",
            err.code, err.stage, err.message
        )),
        other => Err(format!("unexpected remote invoke response: {other:?}")),
    }
}

fn disconnect_remote_via_manager(context: &NativePluginContext, connection_id: &str) {
    let Ok(manager_auth_proof) =
        compute_manager_auth_proof(context.manager_auth_secret(), context.runner_id())
    else {
        return;
    };
    let _ = call_ipc_sync::<ControlRequest, ControlResponse>(
        context.manager_control_endpoint(),
        &ControlRequest::RpcDisconnectRemote {
            runner_id: context.runner_id().to_string(),
            manager_auth_proof,
            connection_id: connection_id.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::{random_secret_hex, verify_manager_auth_proof};
    use imagod_spec::RunnerAppType;
    use std::{
        os::unix::net::UnixListener as StdUnixListener,
        path::PathBuf,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

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

    #[test]
    fn invoke_connection_local_routes_via_manager_and_runner_sockets() {
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
        let connection_rep = allocate_connection_rep(RpcConnection::LocalUds);
        let actual = invoke_connection(
            &context,
            connection_rep,
            "svc-target",
            "yieldspace:svc/invoke",
            "call",
            vec![0x01, 0x02],
        )
        .expect("local invoke should succeed");
        assert_eq!(actual, vec![0xAA, 0xBB]);

        unregister_connection(connection_rep);

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
