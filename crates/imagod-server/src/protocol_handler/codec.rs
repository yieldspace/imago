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
