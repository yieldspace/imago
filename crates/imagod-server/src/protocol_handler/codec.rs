use imagod_common::ImagodError;

pub(crate) trait FrameCodec: Send + Sync {
    fn encode_frame(&self, payload: &[u8]) -> Vec<u8>;
    fn decode_frames(&self, value: &[u8]) -> Result<Vec<Vec<u8>>, ImagodError>;
}

pub(crate) struct LengthPrefixedFrameCodec;

impl FrameCodec for LengthPrefixedFrameCodec {
    fn encode_frame(&self, payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    fn decode_frames(&self, value: &[u8]) -> Result<Vec<Vec<u8>>, ImagodError> {
        let mut out = Vec::new();
        let mut offset = 0usize;

        while offset < value.len() {
            if value.len() - offset < 4 {
                return Err(ImagodError::new(
                    imago_protocol::ErrorCode::BadRequest,
                    "protocol",
                    "truncated frame header",
                ));
            }

            let len = u32::from_be_bytes(value[offset..offset + 4].try_into().map_err(|_| {
                ImagodError::new(
                    imago_protocol::ErrorCode::BadRequest,
                    "protocol",
                    "invalid frame header",
                )
            })?) as usize;
            offset += 4;

            if value.len() - offset < len {
                return Err(ImagodError::new(
                    imago_protocol::ErrorCode::BadRequest,
                    "protocol",
                    "truncated frame payload",
                ));
            }

            out.push(value[offset..offset + len].to_vec());
            offset += len;
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use super::{FrameCodec, LengthPrefixedFrameCodec};

    #[test]
    fn given_single_payload__when_encode_and_decode__then_round_trip_succeeds() {
        let codec = LengthPrefixedFrameCodec;
        let payload = b"hello-imagod";

        let frame = codec.encode_frame(payload);
        let decoded = codec.decode_frames(&frame).expect("decode should succeed");

        assert_eq!(decoded, vec![payload.to_vec()]);
    }

    #[test]
    fn given_multiple_frames__when_decode_frames__then_each_frame_is_recovered() {
        let codec = LengthPrefixedFrameCodec;
        let mut bytes = Vec::new();
        bytes.extend(codec.encode_frame(b"first"));
        bytes.extend(codec.encode_frame(b"second"));
        bytes.extend(codec.encode_frame(&[0x01, 0x02, 0x03]));

        let decoded = codec
            .decode_frames(&bytes)
            .expect("multi frame decode should succeed");

        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0], b"first");
        assert_eq!(decoded[1], b"second");
        assert_eq!(decoded[2], vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn given_truncated_header__when_decode_frames__then_bad_request_is_returned() {
        let codec = LengthPrefixedFrameCodec;
        let err = codec
            .decode_frames(&[0x00, 0x00, 0x00])
            .expect_err("truncated header must fail");

        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "protocol");
        assert_eq!(err.message, "truncated frame header");
    }

    #[test]
    fn given_truncated_payload__when_decode_frames__then_bad_request_is_returned() {
        let codec = LengthPrefixedFrameCodec;
        let mut frame = vec![0, 0, 0, 5];
        frame.extend_from_slice(b"abc");

        let err = codec
            .decode_frames(&frame)
            .expect_err("truncated payload must fail");

        assert_eq!(err.code, imago_protocol::ErrorCode::BadRequest);
        assert_eq!(err.stage, "protocol");
        assert_eq!(err.message, "truncated frame payload");
    }

    #[test]
    fn given_empty_stream__when_decode_frames__then_empty_list_is_returned() {
        let codec = LengthPrefixedFrameCodec;
        let decoded = codec
            .decode_frames(&[])
            .expect("empty stream should decode as empty list");

        assert!(decoded.is_empty());
    }
}
