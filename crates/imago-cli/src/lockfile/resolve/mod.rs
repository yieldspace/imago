//! Lockfile read/write and resolution logic.
//!
//! Resolution verifies that project materialization (`wit/deps`, component cache)
//! matches lock metadata and returns deterministic records for build/deploy paths.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, anyhow};
use sha2::{Digest, Sha256};

use super::{
    hash::{DigestProvider, Sha256DigestProvider},
    types::{
        BindingWitExpectation, DependencyExpectation, IMAGO_LOCK_VERSION, ImagoLock,
        ImagoLockRequestedDependency, ImagoLockResolvedBinding, ImagoLockResolvedDependency,
        LockCapabilityPolicy, LockDependencyKind, LockSourceKind,
    },
    validation::{PathVerifier, StrictPathVerifier, validate_sha256_hex},
};

mod binding;
mod dependency;
mod packages;
mod requested;

pub use binding::resolve_binding_wits;
pub use dependency::resolve_dependencies;
pub use packages::{collect_resolved_packages_and_edges, resolved_package_ref};
pub use requested::{
    build_requested_snapshot, compute_binding_request_id, compute_dependency_request_id,
    compute_requested_fingerprint,
};

pub fn load_from_project_root(project_root: &Path) -> anyhow::Result<ImagoLock> {
    let lock_path = project_root.join("imago.lock");
    let lock_raw = fs::read_to_string(&lock_path)
        .with_context(|| "imago.lock is missing; run `imago deps sync`".to_string())?;
    let lock: ImagoLock = toml::from_str(&lock_raw).context("failed to parse imago.lock")?;
    ensure_supported_lock_version(lock.version)?;
    Ok(lock)
}

pub fn save_to_project_root(project_root: &Path, lock: &ImagoLock) -> anyhow::Result<()> {
    ensure_supported_lock_version(lock.version)?;
    let lock_bytes = toml::to_string_pretty(lock).context("failed to serialize imago.lock")?;
    let lock_path = project_root.join("imago.lock");
    fs::write(&lock_path, lock_bytes)
        .with_context(|| format!("failed to write {}", lock_path.display()))?;
    Ok(())
}

pub fn ensure_requested_fingerprint(
    lock: &ImagoLock,
    dependency_expectations: &[DependencyExpectation],
    binding_expectations: &[BindingWitExpectation],
    namespace_registries: Option<&BTreeMap<String, String>>,
) -> anyhow::Result<()> {
    let expected = build_requested_snapshot(
        dependency_expectations,
        binding_expectations,
        namespace_registries,
    )?;
    if lock.requested.fingerprint != expected.fingerprint {
        return Err(anyhow!(
            "imago.lock requested fingerprint mismatch; run `imago deps sync`"
        ));
    }
    Ok(())
}

pub(super) fn validate_binding_interfaces(entry: &ImagoLockResolvedBinding) -> anyhow::Result<()> {
    if entry.interfaces.is_empty() {
        return Err(anyhow!(
            "imago.lock.resolved.bindings['{}'].interfaces must not be empty; run `imago deps sync`",
            entry.request_id
        ));
    }
    for interface in &entry.interfaces {
        validate_binding_interface_format(
            interface,
            &format!(
                "imago.lock.resolved.bindings['{}'].interfaces[]",
                entry.request_id
            ),
        )?;
    }
    Ok(())
}

pub(super) fn validate_resolved_component_against_requested(
    requested: &ImagoLockRequestedDependency,
    resolved: &ImagoLockResolvedDependency,
) -> anyhow::Result<()> {
    if let Some(requested_sha256) = requested.component_sha256.as_deref() {
        validate_sha256_hex(
            requested_sha256,
            &format!(
                "imago.lock.requested.dependencies['{}'].component_sha256",
                requested.id
            ),
        )?;
    }
    if let Some(resolved_sha256) = resolved.component_sha256.as_deref() {
        validate_sha256_hex(
            resolved_sha256,
            &format!(
                "imago.lock.resolved.dependencies['{}'].component_sha256",
                resolved.request_id
            ),
        )?;
    }

    let requested_has_component = requested.component_source_kind.is_some()
        || requested.component_source.is_some()
        || requested.component_registry.is_some()
        || requested.component_sha256.is_some();
    let resolved_has_component = resolved.component_source.is_some()
        || resolved.component_registry.is_some()
        || resolved.component_sha256.is_some();

    if requested.kind == LockDependencyKind::Native
        && !requested_has_component
        && resolved_has_component
    {
        return Err(anyhow!(
            "imago.lock.resolved.dependencies['{}'] must not define component metadata for native dependency request '{}'; run `imago deps sync`",
            resolved.request_id,
            requested.id
        ));
    }

    if requested.component_source != resolved.component_source {
        return Err(anyhow!(
            "imago.lock.resolved.dependencies['{}'] component source mismatch with requested dependency '{}'; run `imago deps sync`",
            resolved.request_id,
            requested.id
        ));
    }
    if requested.component_registry != resolved.component_registry {
        return Err(anyhow!(
            "imago.lock.resolved.dependencies['{}'] component registry mismatch with requested dependency '{}'; run `imago deps sync`",
            resolved.request_id,
            requested.id
        ));
    }

    if requested.kind == LockDependencyKind::Wasm && resolved.component_sha256.is_none() {
        return Err(anyhow!(
            "imago.lock.resolved.dependencies['{}'].component_sha256 is missing for wasm dependency request '{}'; run `imago deps sync`",
            resolved.request_id,
            requested.id
        ));
    }

    if let Some(expected_sha256) = requested.component_sha256.as_deref() {
        let actual_sha256 = resolved.component_sha256.as_deref().ok_or_else(|| {
            anyhow!(
                "imago.lock.resolved.dependencies['{}'].component_sha256 is missing for dependency request '{}'; run `imago deps sync`",
                resolved.request_id,
                requested.id
            )
        })?;
        if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
            return Err(anyhow!(
                "imago.lock.resolved.dependencies['{}'] component sha256 mismatch with requested dependency '{}'; run `imago deps sync`",
                resolved.request_id,
                requested.id
            ));
        }
    }

    Ok(())
}

pub(super) fn validate_binding_interface_format(
    interface: &str,
    field_name: &str,
) -> anyhow::Result<()> {
    let Some((package, interface_name)) = interface.split_once('/') else {
        return Err(anyhow!(
            "{field_name} must be in '<package>/<interface>' format; run `imago deps sync`"
        ));
    };
    if package.trim().is_empty() || interface_name.trim().is_empty() || interface_name.contains('/')
    {
        return Err(anyhow!(
            "{field_name} must be in '<package>/<interface>' format; run `imago deps sync`"
        ));
    }
    Ok(())
}

pub(super) fn validate_resolved_wit_tree(
    project_root: &Path,
    wit_path: &str,
    expected_digest: &str,
    field_name: &str,
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<()> {
    let relative_wit_path = path_verifier.validate_safe_wit_path(wit_path, field_name)?;
    path_verifier.ensure_no_symlink_in_relative_path(
        project_root,
        &relative_wit_path,
        field_name,
    )?;
    let resolved_wit_path = project_root.join(relative_wit_path);
    let digest = digest_provider
        .compute_path_digest_hex(&resolved_wit_path)
        .with_context(|| {
            format!(
                "failed to compute digest for '{}' from imago.lock",
                resolved_wit_path.display()
            )
        })?;
    if digest != expected_digest {
        return Err(anyhow!(
            "{field_name} digest mismatch; run `imago deps sync`"
        ));
    }
    Ok(())
}

pub(super) fn ensure_supported_lock_version(version: u32) -> anyhow::Result<()> {
    if version != IMAGO_LOCK_VERSION {
        return Err(anyhow!(
            "imago.lock version '{}' is not supported; run `imago deps sync`",
            version
        ));
    }
    Ok(())
}

pub(super) fn normalize_capability_policy(policy: &LockCapabilityPolicy) -> LockCapabilityPolicy {
    let mut normalized = LockCapabilityPolicy {
        privileged: policy.privileged,
        deps: BTreeMap::new(),
        wasi: BTreeMap::new(),
    };

    for (key, values) in &policy.deps {
        normalized
            .deps
            .insert(key.clone(), normalize_string_set(values.iter().cloned()));
    }
    for (key, values) in &policy.wasi {
        normalized
            .wasi
            .insert(key.clone(), normalize_string_set(values.iter().cloned()));
    }
    normalized
}

pub(super) fn normalize_string_set(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            set.insert(value.to_string());
        }
    }
    set.into_iter().collect()
}

pub(super) fn source_kind_label(kind: LockSourceKind) -> &'static str {
    match kind {
        LockSourceKind::Wit => "wit",
        LockSourceKind::Oci => "oci",
        LockSourceKind::Path => "path",
    }
}

pub(super) fn dependency_kind_label(kind: LockDependencyKind) -> &'static str {
    match kind {
        LockDependencyKind::Native => "native",
        LockDependencyKind::Wasm => "wasm",
    }
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub(super) fn new_digest_provider() -> Sha256DigestProvider {
    Sha256DigestProvider
}

pub(super) fn new_path_verifier() -> StrictPathVerifier {
    StrictPathVerifier
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::lockfile::{IMAGO_LOCK_VERSION, ImagoLock, ImagoLockRequested, ImagoLockResolved};

    use super::{
        ensure_requested_fingerprint, ensure_supported_lock_version, normalize_string_set,
    };

    #[test]
    fn ensure_supported_lock_version_rejects_unknown_version() {
        let err = ensure_supported_lock_version(IMAGO_LOCK_VERSION + 1)
            .expect_err("unknown lock version should fail");
        assert!(err.to_string().contains("is not supported"));
    }

    #[test]
    fn normalize_string_set_trims_and_deduplicates_values() {
        let normalized = normalize_string_set(vec![
            "b".to_string(),
            " a ".to_string(),
            "a".to_string(),
            "".to_string(),
        ]);
        assert_eq!(normalized, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn ensure_requested_fingerprint_detects_mismatch() {
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested: ImagoLockRequested {
                fingerprint: "not-matching".to_string(),
                dependencies: vec![],
                bindings: vec![],
            },
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };
        let err = ensure_requested_fingerprint(&lock, &[], &[], Some(&BTreeMap::new()))
            .expect_err("fingerprint mismatch should fail");
        assert!(err.to_string().contains("requested fingerprint mismatch"));
    }
}
