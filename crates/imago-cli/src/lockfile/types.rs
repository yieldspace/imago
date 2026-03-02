//! Serializable lockfile schema and in-memory expectation/resolve types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const IMAGO_LOCK_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Root lockfile model persisted as `imago.lock`.
pub struct ImagoLock {
    #[serde(default = "default_lock_version")]
    pub version: u32,
    pub requested: ImagoLockRequested,
    pub resolved: ImagoLockResolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Requested dependency/binding snapshot used for lock compatibility checks.
pub struct ImagoLockRequested {
    pub fingerprint: String,
    #[serde(default)]
    pub dependencies: Vec<ImagoLockRequestedDependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ImagoLockRequestedBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Resolved state consumed by build/deploy paths.
pub struct ImagoLockResolved {
    #[serde(default)]
    pub dependencies: Vec<ImagoLockResolvedDependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ImagoLockResolvedBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<ImagoLockResolvedPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_edges: Vec<ImagoLockResolvedPackageEdge>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockSourceKind {
    Wit,
    Oci,
    Path,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockDependencyKind {
    Native,
    Wasm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockEdgeFromKind {
    Dependency,
    Binding,
    Package,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum LockPackageEdgeReason {
    DeclaredRequires,
    WitImport,
    ComponentWorld,
    AutoWasi,
    WitDirClosure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct LockCapabilityPolicy {
    #[serde(default)]
    pub privileged: bool,
    #[serde(default)]
    pub deps: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub wasi: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockRequestedDependency {
    pub id: String,
    pub kind: LockDependencyKind,
    pub version: String,
    pub source_kind: LockSourceKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub declared_requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source_kind: Option<LockSourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "LockCapabilityPolicy::is_empty")]
    pub capabilities: LockCapabilityPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockRequestedBinding {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_kind: LockSourceKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockResolvedDependency {
    pub request_id: String,
    pub resolved_name: String,
    pub resolved_version: String,
    pub wit_path: String,
    pub wit_tree_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_registry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_sha256: Option<String>,
    #[serde(default)]
    pub requires_request_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockResolvedBinding {
    pub request_id: String,
    pub name: String,
    pub resolved_package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    pub wit_path: String,
    pub wit_tree_digest: String,
    pub interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockResolvedPackage {
    pub package_ref: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    pub requirement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImagoLockResolvedPackageEdge {
    pub from_kind: LockEdgeFromKind,
    pub from_ref: String,
    pub to_package_ref: String,
    pub reason: LockPackageEdgeReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Expected component metadata supplied by config parsing.
pub struct ComponentExpectation {
    pub source_kind: LockSourceKind,
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Expected direct dependency record supplied by config parsing.
pub struct DependencyExpectation {
    pub name: String,
    pub kind: LockDependencyKind,
    pub version: String,
    pub source_kind: LockSourceKind,
    pub source: String,
    pub registry: Option<String>,
    pub sha256: Option<String>,
    pub requires: Vec<String>,
    pub capabilities: LockCapabilityPolicy,
    pub component: Option<ComponentExpectation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Expected binding WIT record supplied by config parsing.
pub struct BindingWitExpectation {
    pub name: String,
    pub source_kind: LockSourceKind,
    pub source: String,
    pub registry: Option<String>,
    pub version: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lock-resolved direct dependency materialized for build/deploy.
pub struct ResolvedDependency {
    pub request_id: String,
    pub resolved_name: String,
    pub resolved_version: String,
    pub wit_path: String,
    pub wit_tree_digest: String,
    pub component_source: Option<String>,
    pub component_registry: Option<String>,
    pub component_sha256: Option<String>,
    pub requires_request_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Lock-resolved binding WIT materialized for build operations.
pub struct ResolvedBindingWit {
    pub request_id: String,
    pub name: String,
    pub resolved_package: String,
    pub resolved_version: Option<String>,
    pub wit_path: String,
    pub wit_tree_digest: String,
    pub interfaces: Vec<String>,
}

impl From<&ImagoLockResolvedDependency> for ResolvedDependency {
    fn from(value: &ImagoLockResolvedDependency) -> Self {
        Self {
            request_id: value.request_id.clone(),
            resolved_name: value.resolved_name.clone(),
            resolved_version: value.resolved_version.clone(),
            wit_path: value.wit_path.clone(),
            wit_tree_digest: value.wit_tree_digest.clone(),
            component_source: value.component_source.clone(),
            component_registry: value.component_registry.clone(),
            component_sha256: value.component_sha256.clone(),
            requires_request_ids: value.requires_request_ids.clone(),
        }
    }
}

impl From<&ImagoLockResolvedBinding> for ResolvedBindingWit {
    fn from(value: &ImagoLockResolvedBinding) -> Self {
        Self {
            request_id: value.request_id.clone(),
            name: value.name.clone(),
            resolved_package: value.resolved_package.clone(),
            resolved_version: value.resolved_version.clone(),
            wit_path: value.wit_path.clone(),
            wit_tree_digest: value.wit_tree_digest.clone(),
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
    pub from_kind: Option<LockEdgeFromKind>,
    pub from_ref: Option<String>,
    pub reason: Option<LockPackageEdgeReason>,
}

pub fn default_lock_version() -> u32 {
    IMAGO_LOCK_VERSION
}

impl LockCapabilityPolicy {
    pub fn is_empty(&self) -> bool {
        !self.privileged && self.deps.is_empty() && self.wasi.is_empty()
    }
}
