use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_required_strings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingsCertUploadRequest {
    pub public_key_hex: String,
    pub authority: String,
}

impl Validate for BindingsCertUploadRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[
            (&self.public_key_hex, "public_key_hex"),
            (&self.authority, "authority"),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingsCertUploadResponse {
    pub updated: bool,
    pub detail: String,
}

impl Validate for BindingsCertUploadResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[(&self.detail, "detail")])
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::{from_cbor, to_cbor};

    #[derive(Debug, Serialize)]
    struct BindingsCertUploadMissingAuthority<'a> {
        public_key_hex: &'a str,
    }

    #[test]
    fn request_round_trip_and_validate() {
        let request = BindingsCertUploadRequest {
            public_key_hex: "1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
            authority: "rpc://node-a:4443".to_string(),
        };

        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded =
            from_cbor::<BindingsCertUploadRequest>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);
    }

    #[test]
    fn request_rejects_missing_authority() {
        let encoded = to_cbor(&BindingsCertUploadMissingAuthority {
            public_key_hex: "1111111111111111111111111111111111111111111111111111111111111111",
        })
        .expect("encoding should succeed");
        let decoded = from_cbor::<BindingsCertUploadRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn response_rejects_empty_detail() {
        let response = BindingsCertUploadResponse {
            updated: true,
            detail: String::new(),
        };
        assert!(response.validate().is_err());
    }
}
