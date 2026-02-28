//! Handshake payloads for protocol capability and limit negotiation.

use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_non_empty, ensure_required_strings};

use super::StringMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Negotiation request sent before command operations.
///
/// # Examples
/// ```rust
/// use imago_protocol::{messages::HelloNegotiateRequest, Validate};
///
/// let request = HelloNegotiateRequest {
///     client_version: "0.2.0".to_string(),
///     required_features: vec!["rpc.invoke".to_string()],
/// };
/// request.validate().expect("valid hello request");
/// ```
pub struct HelloNegotiateRequest {
    /// Client protocol version (semver).
    pub client_version: String,
    pub required_features: Vec<String>,
}

impl Validate for HelloNegotiateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[(&self.client_version, "client_version")])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Negotiation response with accepted features and server limits.
///
/// # Examples
/// ```rust
/// use std::collections::BTreeMap;
/// use imago_protocol::{messages::HelloNegotiateResponse, Validate};
///
/// let response = HelloNegotiateResponse {
///     accepted: true,
///     server_version: "imagod/test".to_string(),
///     server_protocol_version: "0.2.0".to_string(),
///     supported_protocol_version_range: "^0.2.0".to_string(),
///     compatibility_announcement: None,
///     features: vec!["rpc.invoke".to_string()],
///     limits: BTreeMap::new(),
/// };
/// response.validate().expect("valid hello response");
/// ```
pub struct HelloNegotiateResponse {
    pub accepted: bool,
    pub server_version: String,
    /// Server protocol version (semver).
    pub server_protocol_version: String,
    /// Semver range expression of supported client protocol versions.
    pub supported_protocol_version_range: String,
    /// Human-readable compatibility guidance on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility_announcement: Option<String>,
    pub features: Vec<String>,
    pub limits: StringMap,
}

impl Validate for HelloNegotiateResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.server_version, "server_version")?;
        ensure_non_empty(&self.server_protocol_version, "server_protocol_version")?;
        ensure_non_empty(
            &self.supported_protocol_version_range,
            "supported_protocol_version_range",
        )?;
        if let Some(announcement) = self.compatibility_announcement.as_deref() {
            ensure_non_empty(announcement, "compatibility_announcement")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::collections::BTreeMap;

    use super::{HelloNegotiateRequest, HelloNegotiateResponse};
    use crate::{Validate, from_cbor, to_cbor};

    #[test]
    fn given_hello_request_cases__when_validate__then_client_version_is_required() {
        let valid = HelloNegotiateRequest {
            client_version: "0.2.0".to_string(),
            required_features: vec!["rpc.invoke".to_string()],
        };
        valid.validate().expect("valid request should pass");

        let invalid = HelloNegotiateRequest {
            client_version: "".to_string(),
            required_features: vec![],
        };
        let err = invalid
            .validate()
            .expect_err("empty client_version should fail");
        assert!(err.to_string().contains("client_version"));
    }

    #[test]
    fn given_response_cases__when_validate__then_required_fields_and_optional_announcement_follow_contract()
     {
        let mut response = HelloNegotiateResponse {
            accepted: true,
            server_version: "imagod/test".to_string(),
            server_protocol_version: "0.2.0".to_string(),
            supported_protocol_version_range: "^0.2.0".to_string(),
            compatibility_announcement: None,
            features: vec!["rpc.invoke".to_string()],
            limits: BTreeMap::new(),
        };
        response.validate().expect("valid response should pass");

        response.compatibility_announcement = Some("".to_string());
        let err = response
            .validate()
            .expect_err("empty compatibility announcement should fail");
        assert!(err.to_string().contains("compatibility_announcement"));
    }

    #[test]
    fn given_wire_payloads__when_round_trip_and_unknown_field_decode__then_deny_unknown_fields_is_enforced()
     {
        let request = HelloNegotiateRequest {
            client_version: "0.2.0".to_string(),
            required_features: vec![],
        };
        let encoded = to_cbor(&request).expect("encode should succeed");
        let decoded = from_cbor::<HelloNegotiateRequest>(&encoded).expect("decode should succeed");
        assert_eq!(decoded, request);

        #[derive(serde::Serialize)]
        struct LegacyHello<'a> {
            client_version: &'a str,
            required_features: Vec<&'a str>,
            compatibility_date: &'a str,
        }
        let encoded = to_cbor(&LegacyHello {
            client_version: "0.2.0",
            required_features: vec![],
            compatibility_date: "2026-02-10",
        })
        .expect("encode should succeed");
        let decoded = from_cbor::<HelloNegotiateRequest>(&encoded);
        assert!(decoded.is_err(), "unknown field should be rejected");
    }
}
