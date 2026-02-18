//! DBus-like peer-to-peer transport backed by Unix domain sockets and CBOR frames.

use std::path::Path;

use imago_protocol::{ErrorCode, Validate};
use imagod_common::ImagodError;
use serde::{Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

use crate::ipc::{
    BoxFutureResult, ControlPlaneTransport, ControlRequest, ControlResponse, InvocationTransport,
    RunnerInboundRequest, RunnerInboundResponse, map_ipc_error,
};

const STAGE: &str = "ipc.dbus_p2p";
const MAX_FRAME_BYTES: usize = 1024 * 1024 * 8;

#[derive(Debug, Clone, Default)]
/// P2P transport implementation used by manager/runner IPC.
pub struct DbusP2pTransport;

impl DbusP2pTransport {
    /// Sends one control-plane request and waits for its response.
    pub async fn call_control(
        endpoint: &Path,
        request: &ControlRequest,
    ) -> Result<ControlResponse, ImagodError> {
        validate_message(request)?;
        call(endpoint, request).await
    }

    /// Sends one runner inbound request and waits for its response.
    pub async fn call_runner(
        endpoint: &Path,
        request: &RunnerInboundRequest,
    ) -> Result<RunnerInboundResponse, ImagodError> {
        validate_message(request)?;
        call(endpoint, request).await
    }

    /// Reads one length-prefixed CBOR message from a stream.
    pub async fn read_message<T>(stream: &mut UnixStream) -> Result<T, ImagodError>
    where
        T: DeserializeOwned,
    {
        let bytes = read_frame(stream).await?;
        let message = imago_protocol::from_cbor::<T>(&bytes).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE,
                format!("ipc message decode failed: {e}"),
            )
        })?;
        validate_received_message::<T>(&bytes)?;
        Ok(message)
    }

    /// Encodes and writes one length-prefixed CBOR message to a stream.
    pub async fn write_message<T>(stream: &mut UnixStream, value: &T) -> Result<(), ImagodError>
    where
        T: Serialize,
    {
        let bytes = imago_protocol::to_cbor(value)
            .map_err(|e| map_ipc_error("encode", format!("ipc message encode failed: {e}")))?;
        write_frame(stream, &bytes).await
    }
}

impl ControlPlaneTransport for DbusP2pTransport {
    fn call_control<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a ControlRequest,
    ) -> BoxFutureResult<'a, ControlResponse> {
        Box::pin(async move { Self::call_control(endpoint, request).await })
    }
}

impl InvocationTransport for DbusP2pTransport {
    fn call_runner<'a>(
        &'a self,
        endpoint: &'a Path,
        request: &'a RunnerInboundRequest,
    ) -> BoxFutureResult<'a, RunnerInboundResponse> {
        Box::pin(async move { Self::call_runner(endpoint, request).await })
    }
}

async fn call<Req, Resp>(endpoint: &Path, request: &Req) -> Result<Resp, ImagodError>
where
    Req: Serialize,
    Resp: DeserializeOwned,
{
    let mut stream = UnixStream::connect(endpoint).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE,
            format!("failed to connect to {}: {e}", endpoint.display()),
        )
    })?;
    DbusP2pTransport::write_message(&mut stream, request).await?;
    stream.shutdown().await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE,
            format!("failed to flush request stream: {e}"),
        )
    })?;
    DbusP2pTransport::read_message::<Resp>(&mut stream).await
}

fn validate_message<T>(message: &T) -> Result<(), ImagodError>
where
    T: Validate,
{
    message.validate().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc message validation failed: {e}"),
        )
    })
}

fn validate_received_message<T>(bytes: &[u8]) -> Result<(), ImagodError>
where
    T: DeserializeOwned,
{
    let message_type = std::any::type_name::<T>();

    if message_type == std::any::type_name::<ControlRequest>() {
        let message = decode_for_validation::<ControlRequest>(bytes)?;
        return validate_message(&message);
    }

    if message_type == std::any::type_name::<ControlResponse>() {
        let message = decode_for_validation::<ControlResponse>(bytes)?;
        return validate_message(&message);
    }

    if message_type == std::any::type_name::<RunnerInboundRequest>() {
        let message = decode_for_validation::<RunnerInboundRequest>(bytes)?;
        return validate_message(&message);
    }

    if message_type == std::any::type_name::<RunnerInboundResponse>() {
        let message = decode_for_validation::<RunnerInboundResponse>(bytes)?;
        return validate_message(&message);
    }

    Ok(())
}

fn decode_for_validation<T>(bytes: &[u8]) -> Result<T, ImagodError>
where
    T: DeserializeOwned,
{
    imago_protocol::from_cbor(bytes).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc message decode failed: {e}"),
        )
    })
}

/// Writes one big-endian length-prefixed frame.
async fn write_frame(stream: &mut UnixStream, payload: &[u8]) -> Result<(), ImagodError> {
    let len = payload.len();
    if len > MAX_FRAME_BYTES {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc frame too large: {} bytes", len),
        ));
    }

    let len_u32 = u32::try_from(len).map_err(|_| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc frame size overflow: {}", len),
        )
    })?;

    stream
        .write_all(&len_u32.to_be_bytes())
        .await
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE,
                format!("ipc length write failed: {e}"),
            )
        })?;
    stream.write_all(payload).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE,
            format!("ipc payload write failed: {e}"),
        )
    })?;

    Ok(())
}

/// Reads one big-endian length-prefixed frame.
async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, ImagodError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc length read failed: {e}"),
        )
    })?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc frame too large: {len} bytes"),
        ));
    }

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE,
            format!("ipc payload read failed: {e}"),
        )
    })?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use tokio::net::UnixListener;

    use super::*;
    use crate::ipc::{ControlRequest, ControlResponse};

    #[tokio::test]
    async fn call_control_round_trip() {
        let socket_path = std::path::PathBuf::from(format!(
            "/tmp/imago-{}.sock",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).expect("listener bind should succeed");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept should succeed");
            let req = DbusP2pTransport::read_message::<ControlRequest>(&mut stream)
                .await
                .expect("request should decode");
            match req {
                ControlRequest::Heartbeat { .. } => {
                    DbusP2pTransport::write_message(&mut stream, &ControlResponse::Ack)
                        .await
                        .expect("response write should succeed");
                }
                _ => panic!("unexpected request"),
            }
        });

        let response = DbusP2pTransport::call_control(
            &socket_path,
            &ControlRequest::Heartbeat {
                runner_id: "r1".to_string(),
                manager_auth_proof: "proof".to_string(),
            },
        )
        .await
        .expect("call should succeed");
        assert!(matches!(response, ControlResponse::Ack));

        server.await.expect("server task should finish");
        let _ = std::fs::remove_file(socket_path);
    }
}
