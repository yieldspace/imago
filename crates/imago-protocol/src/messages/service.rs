use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::validate::{Validate, ValidationError, ensure_required_strings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
}

impl Validate for ServiceListRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        let Some(names) = self.names.as_ref() else {
            return Ok(());
        };

        let mut seen = BTreeSet::new();
        for name in names {
            if name.trim().is_empty() {
                return Err(ValidationError::empty("names"));
            }
            if !seen.insert(name.as_str()) {
                return Err(ValidationError::invalid(
                    "names",
                    "must not contain duplicates",
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "stopping")]
    Stopping,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatusEntry {
    pub name: String,
    pub release_hash: String,
    pub started_at: String,
    pub state: ServiceState,
}

impl Validate for ServiceStatusEntry {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[
            (&self.name, "services.name"),
            (&self.release_hash, "services.release_hash"),
            (&self.started_at, "services.started_at"),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceListResponse {
    pub services: Vec<ServiceStatusEntry>,
}

impl Validate for ServiceListResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        for service in &self.services {
            service.validate()?;
        }

        Ok(())
    }
}
