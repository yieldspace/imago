//! Common protocol envelope and identifier invariants.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StructuredError;
use crate::messages::MessageType;
use crate::validate::{Validate, ValidationError, ensure_uuid_not_nil};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Generic message envelope used for all request/response payloads.
pub struct ProtocolEnvelope<TPayload> {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    pub request_id: Uuid,
    pub correlation_id: Uuid,
    pub payload: TPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredError>,
}

impl<TPayload> ProtocolEnvelope<TPayload> {
    /// Builds a successful envelope without an embedded structured error.
    pub fn new(
        message_type: MessageType,
        request_id: Uuid,
        correlation_id: Uuid,
        payload: TPayload,
    ) -> Self {
        Self {
            message_type,
            request_id,
            correlation_id,
            payload,
            error: None,
        }
    }
}

impl<TPayload> Validate for ProtocolEnvelope<TPayload>
where
    TPayload: Validate,
{
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        ensure_uuid_not_nil(&self.correlation_id, "correlation_id")?;
        self.payload.validate()?;

        if let Some(error) = &self.error {
            error.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::HelloNegotiateRequest;

    fn sample_uuid() -> Uuid {
        Uuid::from_u128(0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB)
    }

    #[test]
    fn envelope_requires_non_nil_identifiers() {
        let envelope = ProtocolEnvelope::new(
            MessageType::HelloNegotiate,
            Uuid::nil(),
            sample_uuid(),
            HelloNegotiateRequest {
                client_version: "0.1.0".to_string(),
                required_features: vec![],
            },
        );

        assert!(envelope.validate().is_err());
    }
}
