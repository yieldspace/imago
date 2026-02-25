//! Handshake payloads for protocol capability and limit negotiation.

use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_non_empty, ensure_required_strings};

use super::StringMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Negotiation request sent before command operations.
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
