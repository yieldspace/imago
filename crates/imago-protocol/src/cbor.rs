use std::fmt;
use std::io::Cursor;

use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborError {
    Encode(String),
    Decode(String),
}

impl fmt::Display for CborError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(err) => write!(f, "failed to encode CBOR: {err}"),
            Self::Decode(err) => write!(f, "failed to decode CBOR: {err}"),
        }
    }
}

impl std::error::Error for CborError {}

pub fn to_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, CborError> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes)
        .map_err(|err| CborError::Encode(err.to_string()))?;
    Ok(bytes)
}

pub fn from_cbor<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CborError> {
    let cursor = Cursor::new(bytes);
    ciborium::de::from_reader(cursor).map_err(|err| CborError::Decode(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_u64() {
        let encoded = to_cbor(&42_u64).expect("encoding should succeed");
        let decoded: u64 = from_cbor(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, 42);
    }

    #[test]
    fn decode_invalid_bytes_returns_error() {
        let decoded = from_cbor::<u64>(&[0xff, 0x00]);
        assert!(decoded.is_err());
    }
}
