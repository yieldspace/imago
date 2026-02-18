use serde::{Deserialize, Serialize};

pub const IMAGO_LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagoLock {
    #[serde(default = "default_lock_version")]
    pub version: u32,
    #[serde(default)]
    pub dependencies: Vec<ImagoLockDependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wit_packages: Vec<ImagoLockWitPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagoLockDependency {
    pub name: String,
    pub version: String,
    pub wit_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_registry: Option<String>,
    pub wit_digest: String,
    pub wit_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_sha256: Option<String>,
    pub resolved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagoLockWitPackage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default)]
    pub versions: Vec<ImagoLockWitPackageVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImagoLockWitPackageVersion {
    pub requirement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub path: String,
    #[serde(default)]
    pub via: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentExpectation {
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyExpectation {
    pub name: String,
    pub version: String,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub component: Option<ComponentExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDependency {
    pub name: String,
    pub version: String,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub wit_digest: String,
    pub wit_path: String,
    pub component_source: Option<String>,
    pub component_registry: Option<String>,
    pub component_sha256: Option<String>,
    pub resolved_at: String,
}

impl From<&ImagoLockDependency> for ResolvedDependency {
    fn from(value: &ImagoLockDependency) -> Self {
        Self {
            name: value.name.clone(),
            version: value.version.clone(),
            wit_source: value.wit_source.clone(),
            wit_registry: value.wit_registry.clone(),
            wit_digest: value.wit_digest.clone(),
            wit_path: value.wit_path.clone(),
            component_source: value.component_source.clone(),
            component_registry: value.component_registry.clone(),
            component_sha256: value.component_sha256.clone(),
            resolved_at: value.resolved_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitivePackageRecord {
    pub name: String,
    pub registry: Option<String>,
    pub requirement: String,
    pub version: Option<String>,
    pub digest: String,
    pub source: Option<String>,
    pub path: String,
    pub via: String,
}

pub fn default_lock_version() -> u32 {
    IMAGO_LOCK_VERSION
}
