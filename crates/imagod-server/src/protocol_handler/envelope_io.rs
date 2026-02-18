use imago_protocol::{
    CommandEvent, CommandEventType, CommandType, MessageType, StructuredError, from_cbor, to_cbor,
};
use imagod_common::ImagodError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;
use web_transport_quinn::SendStream;

use super::{Envelope, clock::ServerClock, codec::FrameCodec};

pub(crate) fn bad_request(stage: &str, message: impl Into<String>) -> ImagodError {
    ImagodError::new(imago_protocol::ErrorCode::BadRequest, stage, message)
}

pub(crate) fn response_envelope<T: Serialize>(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    payload: &T,
) -> Result<Envelope, ImagodError> {
    let payload = serde_json::to_value(payload)
        .map_err(|e| bad_request("protocol", format!("payload encode failed: {e}")))?;
    Ok(Envelope {
        message_type,
        request_id,
        correlation_id,
        payload,
        error: None,
    })
}

pub(crate) fn error_envelope(
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
    error: StructuredError,
) -> Envelope {
    Envelope {
        message_type,
        request_id,
        correlation_id,
        payload: Value::Null,
        error: Some(error),
    }
}

pub(crate) fn response_message_type_for_request(request_type: MessageType) -> MessageType {
    match request_type {
        MessageType::StateRequest => MessageType::StateResponse,
        _ => request_type,
    }
}

pub(crate) fn payload_as<T: DeserializeOwned>(request: &Envelope) -> Result<T, ImagodError> {
    serde_json::from_value(request.payload.clone())
        .map_err(|e| bad_request("protocol", format!("request payload decode failed: {e}")))
}

/// Decodes one stream payload into protocol envelopes.
pub(crate) fn parse_stream_envelopes(
    buf: &[u8],
    codec: &impl FrameCodec,
) -> Result<Vec<Envelope>, ImagodError> {
    let frames = codec.decode_frames(buf)?;
    frames
        .iter()
        .map(|frame| {
            let envelope = from_cbor::<Envelope>(frame)
                .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?;
            ensure_non_nil_envelope_ids(&envelope)?;
            Ok(envelope)
        })
        .collect()
}

pub(crate) fn ensure_non_nil_envelope_ids(envelope: &Envelope) -> Result<(), ImagodError> {
    if envelope.request_id.is_nil() {
        return Err(bad_request("protocol", "request_id must not be nil UUID"));
    }
    if envelope.correlation_id.is_nil() {
        return Err(bad_request(
            "protocol",
            "correlation_id must not be nil UUID",
        ));
    }
    Ok(())
}

/// Ensures the stream carries at most one request envelope.
pub(crate) fn ensure_single_request_envelope(envelopes: &[Envelope]) -> Result<(), ImagodError> {
    if envelopes.len() > 1 {
        return Err(bad_request(
            "session.protocol",
            "multiple request envelopes on a single stream are not allowed",
        ));
    }
    Ok(())
}

pub(crate) async fn write_envelope(
    send: &mut SendStream,
    envelope: &Envelope,
    codec: &impl FrameCodec,
) -> Result<(), ImagodError> {
    let data = to_cbor(envelope)
        .map_err(|e| bad_request("protocol", format!("cbor encode failed: {e}")))?;
    let framed = codec.encode_frame(&data);
    send.write_all(&framed).await.map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to send frame: {e}"),
        )
    })?;
    Ok(())
}

pub(crate) fn finish_stream(send: &mut SendStream) -> Result<(), ImagodError> {
    send.finish().map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "session.write",
            format!("failed to finish stream: {e}"),
        )
    })
}

pub(crate) fn event_envelope(
    clock: &impl ServerClock,
    request_id: Uuid,
    correlation_id: Uuid,
    event_type: CommandEventType,
    command_type: CommandType,
    stage: Option<String>,
    error: Option<StructuredError>,
) -> Result<Envelope, ImagodError> {
    let payload = CommandEvent {
        event_type,
        request_id,
        command_type,
        timestamp: clock.now_unix_secs(),
        stage,
        error,
    };
    response_envelope(
        MessageType::CommandEvent,
        request_id,
        correlation_id,
        &payload,
    )
}
