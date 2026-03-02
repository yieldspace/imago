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
        ImagoLockRequested, ImagoLockRequestedBinding, ImagoLockRequestedDependency,
        ImagoLockResolvedBinding, ImagoLockResolvedDependency, ImagoLockResolvedPackage,
        ImagoLockResolvedPackageEdge, LockCapabilityPolicy, LockDependencyKind, LockEdgeFromKind,
        LockPackageEdgeReason, LockSourceKind, ResolvedBindingWit, ResolvedDependency,
        TransitivePackageRecord,
    },
    validation::{
        PathVerifier, StrictPathVerifier, parse_prefixed_sha256, validate_sha256_hex,
        validate_wit_source,
    },
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

pub fn compute_dependency_request_id(expectation: &DependencyExpectation) -> String {
    let mut lines = vec![
        "kind=".to_string() + dependency_kind_label(expectation.kind),
        "version=".to_string() + expectation.version.as_str(),
        "source_kind=".to_string() + source_kind_label(expectation.source_kind),
        "source=".to_string() + expectation.source.as_str(),
        "registry=".to_string() + expectation.registry.as_deref().unwrap_or(""),
        "sha256=".to_string() + expectation.sha256.as_deref().unwrap_or(""),
    ];
    if let Some(component) = expectation.component.as_ref() {
        lines.push("component_kind=".to_string() + source_kind_label(component.source_kind));
        lines.push("component_source=".to_string() + component.source.as_str());
        lines.push("component_registry=".to_string() + component.registry.as_deref().unwrap_or(""));
        lines.push("component_sha256=".to_string() + component.sha256.as_deref().unwrap_or(""));
    }
    for requires in normalize_string_set(expectation.requires.iter().cloned()) {
        lines.push("declared_requires=".to_string() + requires.as_str());
    }
    let capabilities = normalize_capability_policy(&expectation.capabilities);
    lines.push(format!("cap.privileged={}", capabilities.privileged));
    for (key, values) in capabilities.deps {
        lines.push(format!("cap.deps:{key}={}", values.join(",")));
    }
    for (key, values) in capabilities.wasi {
        lines.push(format!("cap.wasi:{key}={}", values.join(",")));
    }

    let joined = lines.join("\n");
    format!("dep:{}", sha256_hex(joined.as_bytes()))
}

pub fn compute_binding_request_id(expectation: &BindingWitExpectation) -> String {
    let lines = [
        "name=".to_string() + expectation.name.as_str(),
        "version=".to_string() + expectation.version.as_str(),
        "source_kind=".to_string() + source_kind_label(expectation.source_kind),
        "source=".to_string() + expectation.source.as_str(),
        "registry=".to_string() + expectation.registry.as_deref().unwrap_or(""),
        "sha256=".to_string() + expectation.sha256.as_deref().unwrap_or(""),
    ];
    let joined = lines.join("\n");
    format!("bind:{}", sha256_hex(joined.as_bytes()))
}

pub fn build_requested_snapshot(
    dependency_expectations: &[DependencyExpectation],
    binding_expectations: &[BindingWitExpectation],
    namespace_registries: Option<&BTreeMap<String, String>>,
) -> anyhow::Result<ImagoLockRequested> {
    let mut dependencies = dependency_expectations
        .iter()
        .map(|expectation| ImagoLockRequestedDependency {
            id: compute_dependency_request_id(expectation),
            kind: expectation.kind,
            version: expectation.version.clone(),
            source_kind: expectation.source_kind,
            source: expectation.source.clone(),
            registry: expectation.registry.clone(),
            sha256: expectation.sha256.clone(),
            declared_requires: normalize_string_set(expectation.requires.iter().cloned()),
            component_source_kind: expectation
                .component
                .as_ref()
                .map(|component| component.source_kind),
            component_source: expectation
                .component
                .as_ref()
                .map(|component| component.source.clone()),
            component_registry: expectation
                .component
                .as_ref()
                .and_then(|component| component.registry.clone()),
            component_sha256: expectation
                .component
                .as_ref()
                .and_then(|component| component.sha256.clone()),
            capabilities: normalize_capability_policy(&expectation.capabilities),
        })
        .collect::<Vec<_>>();
    let mut seen_dependency_ids = BTreeSet::new();
    for dependency in &dependencies {
        if !seen_dependency_ids.insert(dependency.id.clone()) {
            return Err(anyhow!(
                "duplicate dependency request id '{}' in requested snapshot; remove duplicated dependency requests",
                dependency.id
            ));
        }
    }
    dependencies.sort_by(|a, b| a.id.cmp(&b.id));

    let mut bindings = binding_expectations
        .iter()
        .map(|expectation| ImagoLockRequestedBinding {
            id: compute_binding_request_id(expectation),
            name: expectation.name.clone(),
            version: expectation.version.clone(),
            source_kind: expectation.source_kind,
            source: expectation.source.clone(),
            registry: expectation.registry.clone(),
            sha256: expectation.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let mut seen_binding_ids = BTreeSet::new();
    for binding in &bindings {
        if !seen_binding_ids.insert(binding.id.clone()) {
            return Err(anyhow!(
                "duplicate binding request id '{}' in requested snapshot; remove duplicated binding requests",
                binding.id
            ));
        }
    }
    bindings.sort_by(|a, b| a.id.cmp(&b.id));

    let fingerprint = compute_requested_fingerprint(&dependencies, &bindings, namespace_registries);
    Ok(ImagoLockRequested {
        fingerprint,
        dependencies,
        bindings,
    })
}

pub fn compute_requested_fingerprint(
    dependencies: &[ImagoLockRequestedDependency],
    bindings: &[ImagoLockRequestedBinding],
    namespace_registries: Option<&BTreeMap<String, String>>,
) -> String {
    let mut lines = vec!["imago-lock-requested:v1".to_string()];

    if let Some(namespace_registries) = namespace_registries {
        for (namespace, registry) in namespace_registries {
            lines.push(format!("ns:{namespace}={registry}"));
        }
    }

    for dependency in dependencies {
        lines.push(format!("dep.id={}", dependency.id));
        lines.push(format!(
            "dep.kind={}",
            dependency_kind_label(dependency.kind)
        ));
        lines.push(format!("dep.version={}", dependency.version));
        lines.push(format!(
            "dep.source_kind={}",
            source_kind_label(dependency.source_kind)
        ));
        lines.push(format!("dep.source={}", dependency.source));
        lines.push(format!(
            "dep.registry={}",
            dependency.registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.sha256={}",
            dependency.sha256.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_kind={}",
            dependency
                .component_source_kind
                .map(source_kind_label)
                .unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_source={}",
            dependency.component_source.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_registry={}",
            dependency.component_registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_sha256={}",
            dependency.component_sha256.as_deref().unwrap_or("")
        ));

        for requires in &dependency.declared_requires {
            lines.push(format!("dep.requires={requires}"));
        }

        lines.push(format!(
            "dep.cap.privileged={}",
            dependency.capabilities.privileged
        ));
        for (key, values) in &dependency.capabilities.deps {
            lines.push(format!("dep.cap.deps:{key}={}", values.join(",")));
        }
        for (key, values) in &dependency.capabilities.wasi {
            lines.push(format!("dep.cap.wasi:{key}={}", values.join(",")));
        }
    }

    for binding in bindings {
        lines.push(format!("bind.id={}", binding.id));
        lines.push(format!("bind.name={}", binding.name));
        lines.push(format!("bind.version={}", binding.version));
        lines.push(format!(
            "bind.source_kind={}",
            source_kind_label(binding.source_kind)
        ));
        lines.push(format!("bind.source={}", binding.source));
        lines.push(format!(
            "bind.registry={}",
            binding.registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "bind.sha256={}",
            binding.sha256.as_deref().unwrap_or("")
        ));
    }

    sha256_hex(lines.join("\n").as_bytes())
}

pub fn resolve_dependencies(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[DependencyExpectation],
) -> anyhow::Result<BTreeMap<String, ResolvedDependency>> {
    let digest_provider = Sha256DigestProvider;
    let path_verifier = StrictPathVerifier;
    resolve_dependencies_with(
        project_root,
        lock,
        expectations,
        &digest_provider,
        &path_verifier,
    )
}

fn resolve_dependencies_with(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[DependencyExpectation],
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<BTreeMap<String, ResolvedDependency>> {
    ensure_supported_lock_version(lock.version)?;

    let expected_requested = build_requested_snapshot(expectations, &[], None)?;
    let mut requested_by_id = BTreeMap::new();
    for requested in &lock.requested.dependencies {
        if requested_by_id
            .insert(requested.id.clone(), requested)
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.requested.dependencies contains duplicate id '{}'; run `imago deps sync`",
                requested.id
            ));
        }
    }

    let expected_by_id = expected_requested
        .dependencies
        .iter()
        .map(|expected| (expected.id.clone(), expected))
        .collect::<BTreeMap<_, _>>();

    if requested_by_id.len() != expected_by_id.len() {
        return Err(anyhow!(
            "dependency request set does not match imago.lock.requested; run `imago deps sync`"
        ));
    }
    for (request_id, expected) in &expected_by_id {
        let actual = requested_by_id.get(request_id).ok_or_else(|| {
            anyhow!(
                "dependency request '{}' is missing in imago.lock.requested; run `imago deps sync`",
                request_id
            )
        })?;
        if actual != expected {
            return Err(anyhow!(
                "dependency request '{}' differs from imago.lock.requested; run `imago deps sync`",
                request_id
            ));
        }
    }

    let mut resolved_by_request_id = BTreeMap::new();
    for resolved in &lock.resolved.dependencies {
        if resolved_by_request_id
            .insert(
                resolved.request_id.clone(),
                ResolvedDependency::from(resolved),
            )
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.resolved.dependencies contains duplicate request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        let requested = requested_by_id.get(&resolved.request_id).ok_or_else(|| {
            anyhow!(
                "imago.lock.resolved.dependencies contains unknown request_id '{}'; run `imago deps sync`",
                resolved.request_id
            )
        })?;

        for requires in &resolved.requires_request_ids {
            if !requested_by_id.contains_key(requires) {
                return Err(anyhow!(
                    "imago.lock.resolved.dependencies['{}'].requires_request_ids contains unknown request_id '{}'; run `imago deps sync`",
                    resolved.request_id,
                    requires
                ));
            }
        }

        validate_resolved_wit_tree(
            project_root,
            &resolved.wit_path,
            &resolved.wit_tree_digest,
            &format!(
                "imago.lock.resolved.dependencies['{}'].wit_path",
                resolved.request_id
            ),
            digest_provider,
            path_verifier,
        )?;
        validate_resolved_component_against_requested(requested, resolved)?;
    }

    verify_resolved_packages_and_edges(project_root, lock, digest_provider, path_verifier)?;

    let mut resolved = BTreeMap::new();
    for expectation in expectations {
        let request_id = compute_dependency_request_id(expectation);
        let entry = resolved_by_request_id.get(&request_id).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                expectation.name
            )
        })?;
        resolved.insert(expectation.name.clone(), entry.clone());
    }

    Ok(resolved)
}

pub fn resolve_binding_wits(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[BindingWitExpectation],
) -> anyhow::Result<Vec<ResolvedBindingWit>> {
    let digest_provider = Sha256DigestProvider;
    let path_verifier = StrictPathVerifier;
    resolve_binding_wits_with(
        project_root,
        lock,
        expectations,
        &digest_provider,
        &path_verifier,
    )
}

fn resolve_binding_wits_with(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[BindingWitExpectation],
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<Vec<ResolvedBindingWit>> {
    ensure_supported_lock_version(lock.version)?;

    let expected_requested = build_requested_snapshot(&[], expectations, None)?;

    let mut requested_by_id = BTreeMap::new();
    for requested in &lock.requested.bindings {
        if requested_by_id
            .insert(requested.id.clone(), requested)
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.requested.bindings contains duplicate id '{}'; run `imago deps sync`",
                requested.id
            ));
        }
    }

    let expected_by_id = expected_requested
        .bindings
        .iter()
        .map(|expected| (expected.id.clone(), expected))
        .collect::<BTreeMap<_, _>>();

    if requested_by_id.len() != expected_by_id.len() {
        return Err(anyhow!(
            "binding request set does not match imago.lock.requested; run `imago deps sync`"
        ));
    }
    for (request_id, expected) in &expected_by_id {
        let actual = requested_by_id.get(request_id).ok_or_else(|| {
            anyhow!(
                "binding request '{}' is missing in imago.lock.requested; run `imago deps sync`",
                request_id
            )
        })?;
        if actual != expected {
            return Err(anyhow!(
                "binding request '{}' differs from imago.lock.requested; run `imago deps sync`",
                request_id
            ));
        }
    }

    let mut resolved_by_request_id = BTreeMap::new();
    for resolved in &lock.resolved.bindings {
        if resolved_by_request_id
            .insert(
                resolved.request_id.clone(),
                ResolvedBindingWit::from(resolved),
            )
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.resolved.bindings contains duplicate request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        if !requested_by_id.contains_key(&resolved.request_id) {
            return Err(anyhow!(
                "imago.lock.resolved.bindings contains unknown request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        validate_resolved_wit_tree(
            project_root,
            &resolved.wit_path,
            &resolved.wit_tree_digest,
            &format!(
                "imago.lock.resolved.bindings['{}'].wit_path",
                resolved.request_id
            ),
            digest_provider,
            path_verifier,
        )?;

        validate_binding_interfaces(resolved)?;
    }

    verify_resolved_packages_and_edges(project_root, lock, digest_provider, path_verifier)?;

    let mut resolved = Vec::with_capacity(expectations.len());
    for expectation in expectations {
        let request_id = compute_binding_request_id(expectation);
        let entry = resolved_by_request_id.get(&request_id).ok_or_else(|| {
            anyhow!(
                "binding '{}' is not resolved in imago.lock; run `imago deps sync`",
                expectation.name
            )
        })?;
        resolved.push(entry.clone());
    }
    Ok(resolved)
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

pub fn collect_resolved_packages_and_edges(
    records: impl IntoIterator<Item = TransitivePackageRecord>,
) -> anyhow::Result<(
    Vec<ImagoLockResolvedPackage>,
    Vec<ImagoLockResolvedPackageEdge>,
)> {
    let mut packages_by_ref = BTreeMap::<String, ImagoLockResolvedPackage>::new();
    let mut edges = BTreeSet::<(u8, String, String, LockPackageEdgeReason)>::new();

    for record in records {
        let package_ref = resolved_package_ref(
            &record.name,
            record.version.as_deref(),
            record.registry.as_deref(),
        );

        let package = ImagoLockResolvedPackage {
            package_ref: package_ref.clone(),
            name: record.name,
            version: record.version,
            registry: record.registry,
            requirement: record.requirement,
            source: record.source,
            path: record.path,
            digest: record.digest,
        };

        if let Some(existing) = packages_by_ref.get(&package_ref) {
            if existing != &package {
                return Err(anyhow!(
                    "transitive package '{}' has conflicting lock records; run `imago deps sync`",
                    package_ref
                ));
            }
        } else {
            packages_by_ref.insert(package_ref.clone(), package);
        }

        if let (Some(from_kind), Some(from_ref), Some(reason)) =
            (record.from_kind, record.from_ref, record.reason)
            && !from_ref.trim().is_empty()
        {
            edges.insert((edge_kind_sort_key(from_kind), from_ref, package_ref, reason));
        }
    }

    let packages = packages_by_ref.into_values().collect::<Vec<_>>();
    let package_edges = edges
        .into_iter()
        .map(
            |(kind, from_ref, to_package_ref, reason)| ImagoLockResolvedPackageEdge {
                from_kind: edge_sort_key_to_kind(kind),
                from_ref,
                to_package_ref,
                reason,
            },
        )
        .collect::<Vec<_>>();

    Ok((packages, package_edges))
}

pub fn resolved_package_ref(name: &str, version: Option<&str>, registry: Option<&str>) -> String {
    let version = version.unwrap_or("*");
    let registry = registry.unwrap_or("");
    format!("{name}@{version}#{registry}")
}

fn validate_binding_interfaces(entry: &ImagoLockResolvedBinding) -> anyhow::Result<()> {
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

fn validate_resolved_component_against_requested(
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

fn validate_binding_interface_format(interface: &str, field_name: &str) -> anyhow::Result<()> {
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

fn validate_resolved_wit_tree(
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

fn verify_resolved_packages_and_edges(
    project_root: &Path,
    lock: &ImagoLock,
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<()> {
    let dependency_request_ids = lock
        .requested
        .dependencies
        .iter()
        .map(|dependency| dependency.id.clone())
        .collect::<BTreeSet<_>>();
    let binding_request_ids = lock
        .requested
        .bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<BTreeSet<_>>();

    let mut package_refs = BTreeSet::new();
    for package in &lock.resolved.packages {
        if package.package_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages[].package_ref must not be empty; run `imago deps sync`"
            ));
        }
        if !package_refs.insert(package.package_ref.clone()) {
            return Err(anyhow!(
                "imago.lock.resolved.packages contains duplicate package_ref '{}'; run `imago deps sync`",
                package.package_ref
            ));
        }
        let expected_package_ref = resolved_package_ref(
            &package.name,
            package.version.as_deref(),
            package.registry.as_deref(),
        );
        if package.package_ref != expected_package_ref {
            return Err(anyhow!(
                "imago.lock.resolved.packages has non-canonical package_ref '{}' (expected '{}'); run `imago deps sync`",
                package.package_ref,
                expected_package_ref
            ));
        }
        if package.requirement.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages['{}'].requirement must not be empty; run `imago deps sync`",
                package.package_ref
            ));
        }
        if package.path.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages['{}'].path must not be empty; run `imago deps sync`",
                package.package_ref
            ));
        }

        if let Some(source) = package.source.as_deref() {
            validate_wit_source(
                source,
                &format!(
                    "imago.lock.resolved.packages['{}'].source",
                    package.package_ref
                ),
            )?;
            if source.contains('@') {
                return Err(anyhow!(
                    "imago.lock.resolved.packages['{}'].source must not include '@version'; run `imago deps sync`",
                    package.package_ref
                ));
            }
        }

        let expected_digest = parse_prefixed_sha256(
            &package.digest,
            &format!(
                "imago.lock.resolved.packages['{}'].digest",
                package.package_ref
            ),
        )?;
        let relative_path = path_verifier.validate_safe_wit_path(
            &package.path,
            &format!(
                "imago.lock.resolved.packages['{}'].path",
                package.package_ref
            ),
        )?;
        path_verifier.ensure_no_symlink_in_relative_path(
            project_root,
            &relative_path,
            &format!(
                "imago.lock.resolved.packages['{}'].path",
                package.package_ref
            ),
        )?;

        let package_wit_file = project_root.join(relative_path).join("package.wit");
        if !package_wit_file.is_file() {
            return Err(anyhow!(
                "transitive wit package '{}' is missing package.wit at '{}'; run `imago deps sync`",
                package.package_ref,
                package_wit_file.display()
            ));
        }
        let actual_digest = digest_provider
            .compute_sha256_hex(&package_wit_file)
            .with_context(|| {
                format!(
                    "failed to hash transitive wit package '{}' at '{}'",
                    package.package_ref,
                    package_wit_file.display()
                )
            })?;
        if !actual_digest.eq_ignore_ascii_case(expected_digest) {
            return Err(anyhow!(
                "lock digest mismatch for transitive wit package '{}'; run `imago deps sync`",
                package.package_ref
            ));
        }
    }

    let mut seen_edges = BTreeSet::<(u8, String, String, LockPackageEdgeReason)>::new();
    for edge in &lock.resolved.package_edges {
        if edge.from_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges[].from_ref must not be empty; run `imago deps sync`"
            ));
        }
        if edge.to_package_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges[].to_package_ref must not be empty; run `imago deps sync`"
            ));
        }
        if !package_refs.contains(&edge.to_package_ref) {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges points to unknown package_ref '{}'; run `imago deps sync`",
                edge.to_package_ref
            ));
        }

        let from_ok = match edge.from_kind {
            LockEdgeFromKind::Dependency => dependency_request_ids.contains(&edge.from_ref),
            LockEdgeFromKind::Binding => binding_request_ids.contains(&edge.from_ref),
            LockEdgeFromKind::Package => package_refs.contains(&edge.from_ref),
        };
        if !from_ok {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges contains unknown from_ref '{}' for kind '{}'; run `imago deps sync`",
                edge.from_ref,
                edge_from_kind_label(edge.from_kind)
            ));
        }

        let key = (
            edge_kind_sort_key(edge.from_kind),
            edge.from_ref.clone(),
            edge.to_package_ref.clone(),
            edge.reason,
        );
        if !seen_edges.insert(key) {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges contains duplicate edge from '{}' to '{}'; run `imago deps sync`",
                edge.from_ref,
                edge.to_package_ref
            ));
        }
    }

    Ok(())
}

fn ensure_supported_lock_version(version: u32) -> anyhow::Result<()> {
    if version != IMAGO_LOCK_VERSION {
        return Err(anyhow!(
            "imago.lock version '{}' is not supported; run `imago deps sync`",
            version
        ));
    }
    Ok(())
}

fn normalize_capability_policy(policy: &LockCapabilityPolicy) -> LockCapabilityPolicy {
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

fn normalize_string_set(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            set.insert(value.to_string());
        }
    }
    set.into_iter().collect()
}

fn source_kind_label(kind: LockSourceKind) -> &'static str {
    match kind {
        LockSourceKind::Wit => "wit",
        LockSourceKind::Oci => "oci",
        LockSourceKind::Path => "path",
    }
}

fn dependency_kind_label(kind: LockDependencyKind) -> &'static str {
    match kind {
        LockDependencyKind::Native => "native",
        LockDependencyKind::Wasm => "wasm",
    }
}

fn edge_from_kind_label(kind: LockEdgeFromKind) -> &'static str {
    match kind {
        LockEdgeFromKind::Dependency => "dependency",
        LockEdgeFromKind::Binding => "binding",
        LockEdgeFromKind::Package => "package",
    }
}

fn edge_kind_sort_key(kind: LockEdgeFromKind) -> u8 {
    match kind {
        LockEdgeFromKind::Dependency => 0,
        LockEdgeFromKind::Binding => 1,
        LockEdgeFromKind::Package => 2,
    }
}

fn edge_sort_key_to_kind(key: u8) -> LockEdgeFromKind {
    match key {
        0 => LockEdgeFromKind::Dependency,
        1 => LockEdgeFromKind::Binding,
        _ => LockEdgeFromKind::Package,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
