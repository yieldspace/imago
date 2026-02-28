use imago_protocol::{
    ArtifactPushRequest, CommandEvent, CommandEventType, CommandType, MessageType,
    ProtocolEnvelope, StructuredError, from_cbor, to_cbor,
};
use imagod_common::ImagodError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;
use web_transport_quinn::SendStream;

use super::{Envelope, clock::ServerClock, codec::FrameCodec};

#[derive(Debug)]
pub(crate) struct ParsedSingleRequestEnvelope {
    pub request: Envelope,
    pub typed_push: Option<ArtifactPushRequest>,
}

#[derive(Debug, serde::Deserialize)]
struct EnvelopeHeader {
    #[serde(rename = "type")]
    message_type: MessageType,
    request_id: Uuid,
    correlation_id: Uuid,
}

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

#[allow(dead_code)]
pub(crate) fn payload_as<T: DeserializeOwned>(request: &Envelope) -> Result<T, ImagodError> {
    serde_json::from_value(request.payload.clone())
        .map_err(|e| bad_request("protocol", format!("request payload decode failed: {e}")))
}

pub(crate) fn payload_take<T: DeserializeOwned>(request: &mut Envelope) -> Result<T, ImagodError> {
    let payload = std::mem::take(&mut request.payload);
    serde_json::from_value(payload)
        .map_err(|e| bad_request("protocol", format!("request payload decode failed: {e}")))
}

/// Decodes one stream payload into protocol envelopes.
#[allow(dead_code)]
pub(crate) fn parse_stream_envelopes(
    buf: &[u8],
    codec: &impl FrameCodec,
) -> Result<Vec<Envelope>, ImagodError> {
    let frames = codec.decode_frame_slices(buf)?;
    let mut envelopes = Vec::with_capacity(frames.len());
    for frame in frames {
        let envelope = {
            from_cbor::<Envelope>(frame)
                .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?
        };
        ensure_non_nil_envelope_ids(&envelope)?;
        envelopes.push(envelope);
    }
    Ok(envelopes)
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
#[allow(dead_code)]
pub(crate) fn ensure_single_request_envelope(envelopes: &[Envelope]) -> Result<(), ImagodError> {
    if envelopes.len() > 1 {
        return Err(bad_request(
            "session.protocol",
            "multiple request envelopes on a single stream are not allowed",
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn take_single_request_envelope(
    mut envelopes: Vec<Envelope>,
) -> Result<Option<Envelope>, ImagodError> {
    ensure_single_request_envelope(&envelopes)?;
    Ok(envelopes.pop())
}

pub(crate) fn parse_single_request_envelope(
    buf: &[u8],
    codec: &impl FrameCodec,
) -> Result<Option<ParsedSingleRequestEnvelope>, ImagodError> {
    let frames = codec.decode_frame_slices(buf)?;
    if frames.is_empty() {
        return Ok(None);
    }
    if frames.len() > 1 {
        return Err(bad_request(
            "session.protocol",
            "multiple request envelopes on a single stream are not allowed",
        ));
    }
    let frame = frames[0];
    let header = from_cbor::<EnvelopeHeader>(frame)
        .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?;
    if header.request_id.is_nil() {
        return Err(bad_request("protocol", "request_id must not be nil UUID"));
    }
    if header.correlation_id.is_nil() {
        return Err(bad_request(
            "protocol",
            "correlation_id must not be nil UUID",
        ));
    }

    if header.message_type == MessageType::ArtifactPush {
        let typed = from_cbor::<ProtocolEnvelope<ArtifactPushRequest>>(frame)
            .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?;
        let request = Envelope {
            message_type: typed.message_type,
            request_id: typed.request_id,
            correlation_id: typed.correlation_id,
            payload: Value::Null,
            error: typed.error,
        };
        return Ok(Some(ParsedSingleRequestEnvelope {
            request,
            typed_push: Some(typed.payload),
        }));
    }

    let request = from_cbor::<Envelope>(frame)
        .map_err(|e| bad_request("protocol", format!("invalid frame payload: {e}")))?;
    ensure_non_nil_envelope_ids(&request)?;
    Ok(Some(ParsedSingleRequestEnvelope {
        request,
        typed_push: None,
    }))
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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use imago_protocol::{ArtifactPushChunkHeader, ArtifactPushRequest, ProtocolEnvelope};
    use serde::Deserialize;
    use serde::Serialize;

    use super::*;
    use crate::protocol_handler::{
        Envelope, clock::ServerClock, codec::FrameCodec, codec::LengthPrefixedFrameCodec,
    };

    struct FixedClock;

    impl ServerClock for FixedClock {
        fn now_unix_secs(&self) -> String {
            "1700000000".to_string()
        }
    }

    fn sample_envelope(message_type: MessageType) -> Envelope {
        Envelope {
            message_type,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"ok": true}),
            error: None,
        }
    }

    #[test]
    fn given_payload_value__when_response_envelope__then_payload_is_embedded() {
        #[derive(Serialize)]
        struct Payload {
            status: &'static str,
            count: u32,
        }

        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let envelope = response_envelope(
            MessageType::ServicesList,
            request_id,
            correlation_id,
            &Payload {
                status: "ok",
                count: 2,
            },
        )
        .expect("response envelope should be created");

        assert_eq!(envelope.message_type, MessageType::ServicesList);
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.correlation_id, correlation_id);
        assert_eq!(envelope.payload["status"], "ok");
        assert_eq!(envelope.payload["count"], 2);
        assert!(envelope.error.is_none());
    }

    #[test]
    fn given_structured_error__when_error_envelope__then_null_payload_and_error_are_set() {
        let error = StructuredError {
            code: imago_protocol::ErrorCode::BadRequest,
            stage: "session.protocol".to_string(),
            message: "invalid".to_string(),
            retryable: false,
            details: std::collections::BTreeMap::new(),
        };

        let envelope = error_envelope(
            MessageType::CommandEvent,
            Uuid::new_v4(),
            Uuid::new_v4(),
            error.clone(),
        );

        assert!(envelope.payload.is_null());
        assert_eq!(envelope.error, Some(error));
    }

    #[test]
    fn given_request_type__when_response_message_type_for_request__then_state_request_maps_to_response()
     {
        let cases = [
            (MessageType::StateRequest, MessageType::StateResponse),
            (MessageType::DeployPrepare, MessageType::DeployPrepare),
            (MessageType::RpcInvoke, MessageType::RpcInvoke),
        ];

        for (request, expected) in cases {
            let got = response_message_type_for_request(request);
            assert_eq!(got, expected, "request={request:?}");
        }
    }

    #[test]
    fn given_envelope_payload__when_payload_as__then_decodes_struct() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct RequestPayload {
            value: String,
            n: u32,
        }

        let envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"value":"x","n":3}),
            error: None,
        };
        let payload: RequestPayload = payload_as(&envelope).expect("payload decode should succeed");
        assert_eq!(
            payload,
            RequestPayload {
                value: "x".to_string(),
                n: 3
            }
        );
    }

    #[test]
    fn given_bad_payload_shape__when_payload_as__then_bad_request_is_returned() {
        #[derive(Debug, Deserialize)]
        struct RequestPayload {
            value: String,
        }

        let envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"other":"x"}),
            error: None,
        };
        let err = payload_as::<RequestPayload>(&envelope).expect_err("decode must fail");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "protocol");
        assert!(err.message.contains("request payload decode failed"));
    }

    #[test]
    fn given_envelope_payload__when_payload_take__then_decodes_and_consumes_payload() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct RequestPayload {
            value: String,
            n: u32,
        }

        let mut envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"value":"x","n":3}),
            error: None,
        };
        let payload: RequestPayload =
            payload_take(&mut envelope).expect("payload decode should succeed");
        assert_eq!(
            payload,
            RequestPayload {
                value: "x".to_string(),
                n: 3
            }
        );
        assert!(envelope.payload.is_null(), "payload should be consumed");
    }

    #[test]
    fn given_bad_payload_shape__when_payload_take__then_bad_request_is_returned() {
        #[derive(Debug, Deserialize)]
        struct RequestPayload {
            value: String,
        }

        let mut envelope = Envelope {
            message_type: MessageType::RpcInvoke,
            request_id: Uuid::new_v4(),
            correlation_id: Uuid::new_v4(),
            payload: serde_json::json!({"other":"x"}),
            error: None,
        };
        let err = payload_take::<RequestPayload>(&mut envelope).expect_err("decode must fail");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "protocol");
        assert!(err.message.contains("request payload decode failed"));
        assert!(envelope.payload.is_null(), "payload should be consumed");
    }

    #[test]
    fn given_frame_stream__when_parse_stream_envelopes__then_all_frames_are_decoded() {
        let codec = LengthPrefixedFrameCodec;
        let first = sample_envelope(MessageType::HelloNegotiate);
        let second = sample_envelope(MessageType::RpcInvoke);
        let mut bytes = Vec::new();
        bytes.extend(codec.encode_frame(&to_cbor(&first).expect("encode first")));
        bytes.extend(codec.encode_frame(&to_cbor(&second).expect("encode second")));

        let decoded = parse_stream_envelopes(&bytes, &codec).expect("parse should succeed");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].message_type, MessageType::HelloNegotiate);
        assert_eq!(decoded[1].message_type, MessageType::RpcInvoke);
    }

    #[test]
    fn given_nil_ids_or_multi_request__when_validation__then_bad_request_is_returned() {
        let good = sample_envelope(MessageType::HelloNegotiate);
        let bad_request_id = Envelope {
            request_id: Uuid::nil(),
            ..good.clone()
        };
        let err = ensure_non_nil_envelope_ids(&bad_request_id)
            .expect_err("nil request_id must be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);

        let bad_correlation_id = Envelope {
            correlation_id: Uuid::nil(),
            ..good.clone()
        };
        let err = ensure_non_nil_envelope_ids(&bad_correlation_id)
            .expect_err("nil correlation_id must be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);

        let err = ensure_single_request_envelope(&[good.clone(), good])
            .expect_err("multi request stream must fail");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "session.protocol");
    }

    #[test]
    fn given_empty_stream__when_take_single_request_envelope__then_none_is_returned() {
        let request = take_single_request_envelope(Vec::new())
            .expect("empty stream should be accepted as no request");
        assert!(request.is_none(), "empty stream should return None");
    }

    #[test]
    fn given_one_request__when_take_single_request_envelope__then_request_is_moved_out() {
        let envelope = sample_envelope(MessageType::DeployPrepare);
        let request = take_single_request_envelope(vec![envelope.clone()])
            .expect("single request stream should pass")
            .expect("one request should be present");
        assert_eq!(request, envelope);
    }

    #[test]
    fn given_multiple_requests__when_take_single_request_envelope__then_bad_request_is_returned() {
        let first = sample_envelope(MessageType::HelloNegotiate);
        let second = sample_envelope(MessageType::RpcInvoke);
        let err = take_single_request_envelope(vec![first, second])
            .expect_err("multiple request envelopes must be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "session.protocol");
        assert_eq!(
            err.message,
            "multiple request envelopes on a single stream are not allowed"
        );
    }

    #[test]
    fn given_artifact_push_frame__when_parse_single_request_envelope__then_typed_push_is_decoded() {
        let codec = LengthPrefixedFrameCodec;
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let typed = ProtocolEnvelope::new(
            MessageType::ArtifactPush,
            request_id,
            correlation_id,
            ArtifactPushRequest {
                header: ArtifactPushChunkHeader {
                    deploy_id: "deploy-1".to_string(),
                    offset: 0,
                    length: 4,
                    chunk_sha256: "abcd".to_string(),
                    upload_token: "token-1".to_string(),
                },
                chunk: vec![1, 2, 3, 4],
            },
        );
        let bytes = codec.encode_frame(&to_cbor(&typed).expect("encode typed push"));

        let parsed = parse_single_request_envelope(&bytes, &codec)
            .expect("single frame parse should succeed")
            .expect("single request should be present");
        assert_eq!(parsed.request.message_type, MessageType::ArtifactPush);
        assert!(
            parsed.request.payload.is_null(),
            "typed path should avoid json payload"
        );
        let payload = parsed
            .typed_push
            .expect("artifact.push should decode into typed payload");
        assert_eq!(payload.header.length, 4);
        assert_eq!(payload.chunk, vec![1, 2, 3, 4]);
    }

    #[test]
    fn given_multiple_frames__when_parse_single_request_envelope__then_bad_request_is_returned() {
        let codec = LengthPrefixedFrameCodec;
        let first = sample_envelope(MessageType::HelloNegotiate);
        let second = sample_envelope(MessageType::RpcInvoke);
        let mut bytes = Vec::new();
        bytes.extend(codec.encode_frame(&to_cbor(&first).expect("encode first")));
        bytes.extend(codec.encode_frame(&to_cbor(&second).expect("encode second")));

        let err = parse_single_request_envelope(&bytes, &codec)
            .expect_err("multiple request envelopes must be rejected");
        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "session.protocol");
        assert_eq!(
            err.message,
            "multiple request envelopes on a single stream are not allowed"
        );
    }

    #[test]
    fn given_clock_and_event_fields__when_event_envelope__then_command_event_payload_is_built() {
        let request_id = Uuid::new_v4();
        let correlation_id = Uuid::new_v4();
        let envelope = event_envelope(
            &FixedClock,
            request_id,
            correlation_id,
            CommandEventType::Progress,
            CommandType::Deploy,
            Some("stage-a".to_string()),
            None,
        )
        .expect("event envelope should be created");
        assert_eq!(envelope.message_type, MessageType::CommandEvent);
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.correlation_id, correlation_id);

        let payload = envelope.payload;
        assert_eq!(payload["event_type"], "progress");
        assert_eq!(payload["command_type"], "deploy");
        assert_eq!(payload["timestamp"], "1700000000");
        assert_eq!(payload["stage"], "stage-a");
    }
}
