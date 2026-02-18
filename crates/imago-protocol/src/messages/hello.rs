use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_non_empty, ensure_required_strings};

use super::StringMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloNegotiateRequest {
    pub compatibility_date: String,
    pub client_version: String,
    pub required_features: Vec<String>,
}

impl Validate for HelloNegotiateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[
            (&self.compatibility_date, "compatibility_date"),
            (&self.client_version, "client_version"),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloNegotiateResponse {
    pub accepted: bool,
    pub server_version: String,
    pub features: Vec<String>,
    pub limits: StringMap,
}

impl Validate for HelloNegotiateResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.server_version, "server_version")
    }
}
