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

use crate::{
    hash::{DigestProvider, Sha256DigestProvider},
    types::{
        BindingWitExpectation, DependencyExpectation, IMAGO_LOCK_VERSION, ImagoLock,
        ImagoLockWitPackage, ImagoLockWitPackageVersion, ResolvedBindingWit, ResolvedDependency,
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
    verify_wit_packages_lock(
        project_root,
        expectations,
        &lock.wit_packages,
        digest_provider,
        path_verifier,
    )?;

    let mut by_name = BTreeMap::new();
    for entry in &lock.dependencies {
        if by_name
            .insert(entry.name.clone(), ResolvedDependency::from(entry))
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock contains duplicate dependency '{}'; run `imago deps sync`",
                entry.name
            ));
        }
    }

    let mut seen_expectations = BTreeSet::new();
    let mut resolved = BTreeMap::new();
    for expected in expectations {
        if !seen_expectations.insert(expected.name.clone()) {
            return Err(anyhow!(
                "duplicate dependency expectation '{}'",
                expected.name
            ));
        }
        let entry = by_name.get(&expected.name).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                expected.name
            )
        })?;
        if entry.version != expected.version {
            return Err(anyhow!(
                "dependency '{}@{}' does not match lock version '{}'; run `imago deps sync`",
                expected.name,
                expected.version,
                entry.version
            ));
        }
        if entry.wit_source != expected.wit_source {
            return Err(anyhow!(
                "dependency '{}' wit source mismatch (lock='{}', config='{}'); run `imago deps sync`",
                expected.name,
                entry.wit_source,
                expected.wit_source
            ));
        }
        if entry.wit_registry != expected.wit_registry {
            return Err(anyhow!(
                "dependency '{}' wit registry mismatch (lock='{}', config='{}'); run `imago deps sync`",
                expected.name,
                entry.wit_registry.as_deref().unwrap_or(""),
                expected.wit_registry.as_deref().unwrap_or("")
            ));
        }

        let relative_wit_path = path_verifier.validate_safe_wit_path(
            &entry.wit_path,
            &format!("imago.lock.dependencies['{}'].wit_path", expected.name),
        )?;
        path_verifier.ensure_no_symlink_in_relative_path(
            project_root,
            &relative_wit_path,
            &format!("imago.lock.dependencies['{}'].wit_path", expected.name),
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
        if digest != entry.wit_digest {
            return Err(anyhow!(
                "dependency '{}' lock digest mismatch; run `imago deps sync`",
                expected.name
            ));
        }

        match expected.component.as_ref() {
            None => {}
            Some(component_expected) => {
                let lock_component_source = entry.component_source.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' component source is missing in imago.lock; run `imago deps sync`",
                        expected.name
                    )
                })?;
                if lock_component_source != &component_expected.source {
                    return Err(anyhow!(
                        "dependency '{}' component source mismatch (lock='{}', config='{}'); run `imago deps sync`",
                        expected.name,
                        lock_component_source,
                        component_expected.source
                    ));
                }
                if entry.component_registry != component_expected.registry {
                    return Err(anyhow!(
                        "dependency '{}' component registry mismatch (lock='{}', config='{}'); run `imago deps sync`",
                        expected.name,
                        entry.component_registry.as_deref().unwrap_or(""),
                        component_expected.registry.as_deref().unwrap_or("")
                    ));
                }
                let lock_component_sha = entry.component_sha256.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' component sha256 is missing in imago.lock; run `imago deps sync`",
                        expected.name
                    )
                })?;
                validate_sha256_hex(
                    lock_component_sha,
                    &format!(
                        "imago.lock.dependencies[{}].component_sha256",
                        expected.name
                    ),
                )?;
                if let Some(expected_sha) = component_expected.sha256.as_ref()
                    && !lock_component_sha.eq_ignore_ascii_case(expected_sha)
                {
                    return Err(anyhow!(
                        "dependency '{}' component sha256 mismatch (lock='{}', config='{}'); run `imago deps sync`",
                        expected.name,
                        lock_component_sha,
                        expected_sha
                    ));
                }
            }
        }

        resolved.insert(expected.name.clone(), entry.clone());
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

    let mut by_key = BTreeMap::new();
    for entry in &lock.binding_wits {
        let lock_key = binding_wit_key(
            &entry.name,
            &entry.wit_source,
            entry.wit_registry.as_deref(),
        );
        if by_key
            .insert(lock_key.clone(), ResolvedBindingWit::from(entry))
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock contains duplicate binding_wits entry (name='{}', source='{}', registry='{}'); run `imago deps sync`",
                lock_key.0,
                lock_key.1,
                lock_key.2.as_deref().unwrap_or("")
            ));
        }
    }

    let mut seen_expectations = BTreeSet::new();
    let mut resolved = Vec::with_capacity(expectations.len());
    for expected in expectations {
        let expected_key = binding_wit_key(
            &expected.name,
            &expected.wit_source,
            expected.wit_registry.as_deref(),
        );
        if !seen_expectations.insert(expected_key.clone()) {
            return Err(anyhow!(
                "duplicate binding wit expectation (name='{}', source='{}', registry='{}')",
                expected_key.0,
                expected_key.1,
                expected_key.2.as_deref().unwrap_or("")
            ));
        }

        let entry = by_key.get(&expected_key).ok_or_else(|| {
            anyhow!(
                "binding wit (name='{}', source='{}', registry='{}') is not resolved in imago.lock; run `imago deps sync`",
                expected_key.0,
                expected_key.1,
                expected_key.2.as_deref().unwrap_or("")
            )
        })?;

        validate_wit_source(
            &entry.wit_source,
            &format!("imago.lock.binding_wits['{}'].wit_source", entry.name),
        )?;
        let relative_wit_path = path_verifier.validate_safe_wit_path(
            &entry.wit_path,
            &format!("imago.lock.binding_wits['{}'].wit_path", entry.name),
        )?;
        path_verifier.ensure_no_symlink_in_relative_path(
            project_root,
            &relative_wit_path,
            &format!("imago.lock.binding_wits['{}'].wit_path", entry.name),
        )?;
        let resolved_wit_path = project_root.join(relative_wit_path);
        let digest = digest_provider
            .compute_path_digest_hex(&resolved_wit_path)
            .with_context(|| {
                format!(
                    "failed to compute digest for binding wit '{}' from imago.lock at '{}'",
                    entry.name,
                    resolved_wit_path.display()
                )
            })?;
        if digest != entry.wit_digest {
            return Err(anyhow!(
                "binding wit '{}' lock digest mismatch; run `imago deps sync`",
                entry.name
            ));
        }

        validate_binding_interfaces(entry)?;
        resolved.push(entry.clone());
    }

    Ok(resolved)
}

pub fn collect_wit_packages(
    records: impl IntoIterator<Item = TransitivePackageRecord>,
) -> Vec<ImagoLockWitPackage> {
    let mut grouped = BTreeMap::<
        (String, Option<String>),
        BTreeMap<(String, Option<String>, String, Option<String>, String), BTreeSet<String>>,
    >::new();

    for record in records {
        let package_key = (record.name, record.registry);
        let version_key = (
            record.requirement,
            record.version,
            record.digest,
            record.source,
            record.path,
        );
        grouped
            .entry(package_key)
            .or_default()
            .entry(version_key)
            .or_default()
            .insert(record.via);
    }

    let mut packages = Vec::with_capacity(grouped.len());
    for ((name, registry), versions) in grouped {
        let mut version_entries = Vec::with_capacity(versions.len());
        for ((requirement, version, digest, source, path), via_set) in versions {
            version_entries.push(ImagoLockWitPackageVersion {
                requirement,
                version,
                digest,
                source,
                path,
                via: via_set.into_iter().collect(),
            });
        }
        packages.push(ImagoLockWitPackage {
            name,
            registry,
            versions: version_entries,
        });
    }
    packages
}

fn binding_wit_key(
    name: &str,
    wit_source: &str,
    wit_registry: Option<&str>,
) -> (String, String, Option<String>) {
    (
        name.to_string(),
        wit_source.to_string(),
        wit_registry.map(ToString::to_string),
    )
}

fn validate_binding_interfaces(entry: &ResolvedBindingWit) -> anyhow::Result<()> {
    if entry.interfaces.is_empty() {
        return Err(anyhow!(
            "imago.lock.binding_wits['{}'].interfaces must not be empty; run `imago deps sync`",
            entry.name
        ));
    }
    for interface in &entry.interfaces {
        validate_binding_interface_format(
            interface,
            &format!("imago.lock.binding_wits['{}'].interfaces[]", entry.name),
        )?;
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

fn ensure_supported_lock_version(version: u32) -> anyhow::Result<()> {
    if version != IMAGO_LOCK_VERSION {
        return Err(anyhow!(
            "imago.lock version '{}' is not supported; run `imago deps sync`",
            version
        ));
    }
    Ok(())
}

fn verify_wit_packages_lock(
    project_root: &Path,
    direct_dependencies: &[DependencyExpectation],
    wit_packages: &[ImagoLockWitPackage],
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<()> {
    if wit_packages.is_empty() {
        return Ok(());
    }

    let direct_dependency_names = direct_dependencies
        .iter()
        .map(|dep| dep.name.clone())
        .collect::<BTreeSet<_>>();

    for wit_package in wit_packages {
        if wit_package.name.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.wit_packages[].name must not be empty; run `imago deps sync`"
            ));
        }
        if wit_package.versions.is_empty() {
            return Err(anyhow!(
                "imago.lock.wit_packages['{}'].versions must not be empty; run `imago deps sync`",
                wit_package.name
            ));
        }

        for version_entry in &wit_package.versions {
            if version_entry.requirement.trim().is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].requirement must not be empty; run `imago deps sync`",
                    wit_package.name
                ));
            }
            if version_entry.path.trim().is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].path must not be empty; run `imago deps sync`",
                    wit_package.name
                ));
            }
            if version_entry.via.is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].via must not be empty; run `imago deps sync`",
                    wit_package.name
                ));
            }

            for via in &version_entry.via {
                if !direct_dependency_names.contains(via) {
                    return Err(anyhow!(
                        "imago.lock.wit_packages['{}'].versions[].via contains unknown dependency '{}'; run `imago deps sync`",
                        wit_package.name,
                        via
                    ));
                }
            }

            if let Some(source) = version_entry.source.as_deref() {
                validate_wit_source(
                    source,
                    &format!(
                        "imago.lock.wit_packages['{}'].versions[].source",
                        wit_package.name
                    ),
                )?;
                if source.starts_with("warg://") {
                    let lock_version = version_entry.version.as_deref().ok_or_else(|| {
                        anyhow!(
                            "imago.lock.wit_packages['{}'].versions[].version is required for warg source; run `imago deps sync`",
                            wit_package.name
                        )
                    })?;
                    let expected_source = format!("warg://{}@{lock_version}", wit_package.name);
                    if source != expected_source {
                        return Err(anyhow!(
                            "imago.lock.wit_packages['{}'].versions[].source mismatch (lock='{}', expected='{}'); run `imago deps sync`",
                            wit_package.name,
                            source,
                            expected_source
                        ));
                    }
                }
            }

            let expected_digest_hex = parse_prefixed_sha256(
                &version_entry.digest,
                &format!(
                    "imago.lock.wit_packages['{}'].versions[].digest",
                    wit_package.name
                ),
            )?;
            let relative_package_path = path_verifier.validate_safe_wit_path(
                &version_entry.path,
                &format!(
                    "imago.lock.wit_packages['{}'].versions[].path",
                    wit_package.name
                ),
            )?;
            path_verifier.ensure_no_symlink_in_relative_path(
                project_root,
                &relative_package_path,
                &format!(
                    "imago.lock.wit_packages['{}'].versions[].path",
                    wit_package.name
                ),
            )?;
            let package_wit_file = project_root.join(relative_package_path).join("package.wit");
            if !package_wit_file.is_file() {
                return Err(anyhow!(
                    "transitive wit package '{}' is missing package.wit at '{}'; run `imago deps sync`",
                    wit_package.name,
                    package_wit_file.display()
                ));
            }

            let actual_digest = digest_provider
                .compute_sha256_hex(&package_wit_file)
                .with_context(|| {
                    format!(
                        "failed to hash transitive wit package '{}' at '{}'",
                        wit_package.name,
                        package_wit_file.display()
                    )
                })?;
            if !actual_digest.eq_ignore_ascii_case(expected_digest_hex) {
                return Err(anyhow!(
                    "lock digest mismatch for transitive wit package '{}'; run `imago deps sync`",
                    wit_package.name
                ));
            }
        }
    }

    Ok(())
}
