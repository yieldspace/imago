use std::{future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use imago_protocol::{ErrorCode, MessageType};
use imagod_common::ImagodError;
use uuid::Uuid;
use web_transport_quinn::{RecvStream, SendStream, Session};

use super::{
    MAX_STREAM_BYTES, ProtocolHandler, STREAM_READ_TIMEOUT_SECS,
    envelope_io::{
        ensure_single_request_envelope, error_envelope, finish_stream, parse_stream_envelopes,
        response_message_type_for_request, write_envelope,
    },
};

#[async_trait]
pub(crate) trait ProtocolSession: Send + Sync {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)>;

    fn max_datagram_size(&self) -> usize;

    fn send_datagram(&self, payload: Vec<u8>) -> Result<(), ImagodError>;

    async fn closed(&self);
}

#[async_trait]
impl ProtocolSession for Session {
    async fn accept_bi(&self) -> Option<(SendStream, RecvStream)> {
        Session::accept_bi(self).await.ok()
    }

    fn max_datagram_size(&self) -> usize {
        Session::max_datagram_size(self)
    }

    fn send_datagram(&self, payload: Vec<u8>) -> Result<(), ImagodError> {
        Session::send_datagram(self, payload.into()).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                "logs.datagram",
                format!("failed to send datagram: {e}"),
            )
        })
    }

    async fn closed(&self) {
        let _ = Session::closed(self).await;
    }
}

pub(crate) async fn run_session_loop<S>(
    handler: &ProtocolHandler,
    session: Arc<S>,
) -> Result<(), ImagodError>
where
    S: ProtocolSession + 'static,
{
    loop {
        let Some((mut send, mut recv)) = session.accept_bi().await else {
            break;
        };

        let buf = match read_stream_with_timeout(
            recv.read_to_end(MAX_STREAM_BYTES),
            Duration::from_secs(STREAM_READ_TIMEOUT_SECS),
        )
        .await
        {
            Ok(buf) => buf,
            Err(err) => {
                let envelope = error_envelope(
                    MessageType::CommandEvent,
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    err.to_structured(),
                );
                write_envelope(&mut send, &envelope, handler.frame_codec.as_ref()).await?;
                finish_stream(&mut send)?;
                continue;
            }
        };

        let envelopes = match parse_stream_envelopes(&buf, handler.frame_codec.as_ref()) {
            Ok(v) => v,
            Err(err) => {
                let envelope = error_envelope(
                    MessageType::CommandEvent,
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    err.to_structured(),
                );
                write_envelope(&mut send, &envelope, handler.frame_codec.as_ref()).await?;
                finish_stream(&mut send)?;
                continue;
            }
        };

        if envelopes.is_empty() {
            finish_stream(&mut send)?;
            continue;
        }

        if let Err(err) = ensure_single_request_envelope(&envelopes) {
            let first = &envelopes[0];
            let response = error_envelope(
                response_message_type_for_request(first.message_type),
                first.request_id,
                first.correlation_id,
                err.to_structured(),
            );
            write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
            finish_stream(&mut send)?;
            continue;
        }

        let request = envelopes[0].clone();
        if request.message_type == MessageType::CommandStart {
            handler.handle_command_start(request, &mut send).await?;
            finish_stream(&mut send)?;
            continue;
        }
        if request.message_type == MessageType::LogsRequest {
            if let Err(err) = handler
                .handle_logs_request(session.clone(), request.clone(), &mut send)
                .await
            {
                let response = error_envelope(
                    MessageType::LogsRequest,
                    request.request_id,
                    request.correlation_id,
                    err.to_structured(),
                );
                write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
            }
            finish_stream(&mut send)?;
            continue;
        }

        let response = match handler.handle_single(request.clone()).await {
            Ok(resp) => resp,
            Err(err) => error_envelope(
                response_message_type_for_request(request.message_type),
                request.request_id,
                request.correlation_id,
                err.to_structured(),
            ),
        };
        write_envelope(&mut send, &response, handler.frame_codec.as_ref()).await?;
        finish_stream(&mut send)?;
    }

    Ok(())
}

pub(crate) async fn read_stream_with_timeout<F, E>(
    read_future: F,
    timeout_duration: Duration,
) -> Result<Vec<u8>, ImagodError>
where
    F: Future<Output = Result<Vec<u8>, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout_duration, read_future).await {
        Ok(result) => result.map_err(|e| {
            ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "session.read",
                format!("failed to read stream: {e}"),
            )
        }),
        Err(_) => Err(stream_read_timeout_error()),
    }
}

pub(crate) fn stream_read_timeout_error() -> ImagodError {
    ImagodError::new(
        imago_protocol::ErrorCode::OperationTimeout,
        "session.read",
        format!(
            "stream read timed out after {} seconds",
            STREAM_READ_TIMEOUT_SECS
        ),
    )
}
