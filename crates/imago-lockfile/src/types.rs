//! Serializable lockfile schema and in-memory expectation/resolve types.

use serde::{Deserialize, Serialize};

pub const IMAGO_LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Root lockfile model persisted as `imago.lock`.
pub struct ImagoLock {
    #[serde(default = "default_lock_version")]
    pub version: u32,
    #[serde(default)]
    pub dependencies: Vec<ImagoLockDependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding_wits: Vec<ImagoLockBindingWit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wit_packages: Vec<ImagoLockWitPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Direct dependency resolution record.
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Resolved binding WIT source record.
pub struct ImagoLockBindingWit {
    pub name: String,
    pub wit_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_registry: Option<String>,
    pub wit_version: String,
    pub wit_digest: String,
    pub wit_path: String,
    pub interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Aggregated transitive WIT package entry.
pub struct ImagoLockWitPackage {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default)]
    pub versions: Vec<ImagoLockWitPackageVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// One resolved version requirement for a transitive WIT package.
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
/// Expected component metadata supplied by config parsing.
pub struct ComponentExpectation {
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Expected direct dependency record supplied by config parsing.
pub struct DependencyExpectation {
    pub name: String,
    pub version: String,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub component: Option<ComponentExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Expected binding WIT record supplied by config parsing.
pub struct BindingWitExpectation {
    pub name: String,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub wit_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lock-resolved direct dependency materialized for build/deploy.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lock-resolved binding WIT materialized for build operations.
pub struct ResolvedBindingWit {
    pub name: String,
    pub wit_source: String,
    pub wit_registry: Option<String>,
    pub wit_version: String,
    pub wit_digest: String,
    pub wit_path: String,
    pub interfaces: Vec<String>,
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
        }
    }
}

impl From<&ImagoLockBindingWit> for ResolvedBindingWit {
    fn from(value: &ImagoLockBindingWit) -> Self {
        Self {
            name: value.name.clone(),
            wit_source: value.wit_source.clone(),
            wit_registry: value.wit_registry.clone(),
            wit_version: value.wit_version.clone(),
            wit_digest: value.wit_digest.clone(),
            wit_path: value.wit_path.clone(),
            interfaces: value.interfaces.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Flattened transitive package record used when rebuilding lock state.
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
