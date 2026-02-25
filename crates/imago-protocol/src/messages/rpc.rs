//! RPC invocation payloads exchanged over the deploy protocol envelope.

use serde::{Deserialize, Serialize};

use crate::{
    ErrorCode,
    validate::{Validate, ValidationError, ensure_non_empty, ensure_required_strings},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Target service selector for one RPC call.
pub struct RpcInvokeTargetService {
    pub name: String,
}

impl Validate for RpcInvokeTargetService {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "target_service.name")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// RPC invocation request payload.
pub struct RpcInvokeRequest {
    pub interface_id: String,
    pub function: String,
    #[serde(default)]
    pub args_cbor: Vec<u8>,
    pub target_service: RpcInvokeTargetService,
}

impl Validate for RpcInvokeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.interface_id, "interface_id")?;
        ensure_non_empty(&self.function, "function")?;
        self.target_service.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Structured RPC invocation failure payload.
pub struct RpcInvokeError {
    pub code: ErrorCode,
    pub stage: String,
    pub message: String,
}

impl Validate for RpcInvokeError {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[
            (&self.stage, "error.stage"),
            (&self.message, "error.message"),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// RPC invocation response payload.
///
/// Exactly one of `result_cbor` or `error` MUST be present.
pub struct RpcInvokeResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcInvokeError>,
}

impl RpcInvokeResponse {
    pub fn from_result(result_cbor: Vec<u8>) -> Self {
        Self {
            result_cbor: Some(result_cbor),
            error: None,
        }
    }

    pub fn from_error(
        code: ErrorCode,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            result_cbor: None,
            error: Some(RpcInvokeError {
                code,
                stage: stage.into(),
                message: message.into(),
            }),
        }
    }
}

impl Validate for RpcInvokeResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        match (&self.result_cbor, &self.error) {
            (Some(_), None) => Ok(()),
            (None, Some(error)) => error.validate(),
            (Some(_), Some(_)) => Err(ValidationError::invalid(
                "response",
                "result_cbor and error are mutually exclusive",
            )),
            (None, None) => Err(ValidationError::missing("result_cbor or error")),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::{from_cbor, to_cbor};

    #[derive(Debug, Serialize)]
    struct RpcInvokeMissingTargetService<'a> {
        interface_id: &'a str,
        function: &'a str,
        args_cbor: Vec<u8>,
    }

    #[test]
    fn request_round_trip_and_validate() {
        let request = RpcInvokeRequest {
            interface_id: "yieldspace:service/invoke".to_string(),
            function: "call".to_string(),
            args_cbor: vec![0x01, 0x02],
            target_service: RpcInvokeTargetService {
                name: "svc-target".to_string(),
            },
        };

        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded = from_cbor::<RpcInvokeRequest>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);
    }

    #[test]
    fn request_rejects_missing_target_service() {
        let encoded = to_cbor(&RpcInvokeMissingTargetService {
            interface_id: "yieldspace:service/invoke",
            function: "call",
            args_cbor: vec![],
        })
        .expect("encoding should succeed");
        let decoded = from_cbor::<RpcInvokeRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn response_accepts_result_or_error() {
        let success = RpcInvokeResponse::from_result(vec![0xAA]);
        assert!(success.validate().is_ok());

        let failure =
            RpcInvokeResponse::from_error(ErrorCode::NotFound, "rpc.invoke", "target not found");
        assert!(failure.validate().is_ok());
    }

    #[test]
    fn response_rejects_conflicting_fields() {
        let response = RpcInvokeResponse {
            result_cbor: Some(vec![0x01]),
            error: Some(RpcInvokeError {
                code: ErrorCode::Internal,
                stage: "rpc.invoke".to_string(),
                message: "conflict".to_string(),
            }),
        };
        assert!(response.validate().is_err());
    }
}
