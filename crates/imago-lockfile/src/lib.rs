use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

pub fn load_from_project_root(project_root: &Path) -> anyhow::Result<ImagoLock> {
    let lock_path = project_root.join("imago.lock");
    let lock_raw = fs::read_to_string(&lock_path)
        .with_context(|| "imago.lock is missing; run `imago update`".to_string())?;
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
    ensure_supported_lock_version(lock.version)?;
    verify_wit_packages_lock(project_root, expectations, &lock.wit_packages)?;

    let mut by_name = BTreeMap::new();
    for entry in &lock.dependencies {
        if by_name
            .insert(entry.name.clone(), ResolvedDependency::from(entry))
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock contains duplicate dependency '{}'; run `imago update`",
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
                "dependency '{}' is not resolved in imago.lock; run `imago update`",
                expected.name
            )
        })?;
        if entry.version != expected.version {
            return Err(anyhow!(
                "dependency '{}@{}' does not match lock version '{}'; run `imago update`",
                expected.name,
                expected.version,
                entry.version
            ));
        }
        if entry.wit_source != expected.wit_source {
            return Err(anyhow!(
                "dependency '{}' wit source mismatch (lock='{}', config='{}'); run `imago update`",
                expected.name,
                entry.wit_source,
                expected.wit_source
            ));
        }
        if entry.wit_registry != expected.wit_registry {
            return Err(anyhow!(
                "dependency '{}' wit registry mismatch (lock='{}', config='{}'); run `imago update`",
                expected.name,
                entry.wit_registry.as_deref().unwrap_or(""),
                expected.wit_registry.as_deref().unwrap_or("")
            ));
        }

        let resolved_wit_path = project_root.join(&entry.wit_path);
        let digest = compute_path_digest_hex(&resolved_wit_path).with_context(|| {
            format!(
                "failed to compute digest for '{}' from imago.lock",
                resolved_wit_path.display()
            )
        })?;
        if digest != entry.wit_digest {
            return Err(anyhow!(
                "dependency '{}' lock digest mismatch; run `imago update`",
                expected.name
            ));
        }

        match expected.component.as_ref() {
            None => {}
            Some(component_expected) => {
                let lock_component_source = entry.component_source.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' component source is missing in imago.lock; run `imago update`",
                        expected.name
                    )
                })?;
                if lock_component_source != &component_expected.source {
                    return Err(anyhow!(
                        "dependency '{}' component source mismatch (lock='{}', config='{}'); run `imago update`",
                        expected.name,
                        lock_component_source,
                        component_expected.source
                    ));
                }
                if entry.component_registry != component_expected.registry {
                    return Err(anyhow!(
                        "dependency '{}' component registry mismatch (lock='{}', config='{}'); run `imago update`",
                        expected.name,
                        entry.component_registry.as_deref().unwrap_or(""),
                        component_expected.registry.as_deref().unwrap_or("")
                    ));
                }
                let lock_component_sha = entry.component_sha256.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' component sha256 is missing in imago.lock; run `imago update`",
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
                        "dependency '{}' component sha256 mismatch (lock='{}', config='{}'); run `imago update`",
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

fn ensure_supported_lock_version(version: u32) -> anyhow::Result<()> {
    if version != IMAGO_LOCK_VERSION {
        return Err(anyhow!(
            "imago.lock version '{}' is not supported; run `imago update`",
            version
        ));
    }
    Ok(())
}

fn verify_wit_packages_lock(
    project_root: &Path,
    direct_dependencies: &[DependencyExpectation],
    wit_packages: &[ImagoLockWitPackage],
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
                "imago.lock.wit_packages[].name must not be empty; run `imago update`"
            ));
        }
        if wit_package.versions.is_empty() {
            return Err(anyhow!(
                "imago.lock.wit_packages['{}'].versions must not be empty; run `imago update`",
                wit_package.name
            ));
        }

        for version_entry in &wit_package.versions {
            if version_entry.requirement.trim().is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].requirement must not be empty; run `imago update`",
                    wit_package.name
                ));
            }
            if version_entry.path.trim().is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].path must not be empty; run `imago update`",
                    wit_package.name
                ));
            }
            if version_entry.via.is_empty() {
                return Err(anyhow!(
                    "imago.lock.wit_packages['{}'].versions[].via must not be empty; run `imago update`",
                    wit_package.name
                ));
            }

            for via in &version_entry.via {
                if !direct_dependency_names.contains(via) {
                    return Err(anyhow!(
                        "imago.lock.wit_packages['{}'].versions[].via contains unknown dependency '{}'; run `imago update`",
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
                            "imago.lock.wit_packages['{}'].versions[].version is required for warg source; run `imago update`",
                            wit_package.name
                        )
                    })?;
                    let expected_source = format!("warg://{}@{lock_version}", wit_package.name);
                    if source != expected_source {
                        return Err(anyhow!(
                            "imago.lock.wit_packages['{}'].versions[].source mismatch (lock='{}', expected='{}'); run `imago update`",
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
            let package_wit_file = project_root.join(&version_entry.path).join("package.wit");
            if !package_wit_file.is_file() {
                return Err(anyhow!(
                    "transitive wit package '{}' is missing package.wit at '{}'; run `imago update`",
                    wit_package.name,
                    package_wit_file.display()
                ));
            }

            let actual_digest = compute_sha256_hex(&package_wit_file).with_context(|| {
                format!(
                    "failed to hash transitive wit package '{}' at '{}'",
                    wit_package.name,
                    package_wit_file.display()
                )
            })?;
            if !actual_digest.eq_ignore_ascii_case(expected_digest_hex) {
                return Err(anyhow!(
                    "lock digest mismatch for transitive wit package '{}'; run `imago update`",
                    wit_package.name
                ));
            }
        }
    }

    Ok(())
}

fn validate_wit_source(source: &str, field_name: &str) -> anyhow::Result<()> {
    if source.starts_with("file://") || source.starts_with("warg://") {
        return Ok(());
    }
    if source.starts_with("https://wa.dev/") {
        return Err(anyhow!(
            "{field_name} no longer accepts https://wa.dev shorthand; use warg://<package>@<version>"
        ));
    }
    Err(anyhow!(
        "{field_name} must start with one of: file://, warg://"
    ))
}

fn validate_sha256_hex(value: &str, field_name: &str) -> anyhow::Result<()> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("{field_name} must be a 64-character hex string"));
    }
    Ok(())
}

fn parse_prefixed_sha256<'a>(value: &'a str, field_name: &str) -> anyhow::Result<&'a str> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(anyhow!("{field_name} must start with 'sha256:'"));
    };
    validate_sha256_hex(hex, field_name)?;
    Ok(hex)
}

fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, path, "file for sha256")?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read path for digest: {}", path.display()))?;
    if metadata.is_file() {
        return compute_sha256_hex(path);
    }
    if !metadata.is_dir() {
        return Err(anyhow!("path is not file or directory: {}", path.display()));
    }

    let mut stack = vec![path.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory for digest: {}", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read directory entry while hashing {}",
                    dir.display()
                )
            })?;
            let entry_path = entry.path();
            let entry_metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;
            if entry_metadata.is_dir() {
                stack.push(entry_path);
            } else if entry_metadata.is_file() {
                files.push(entry_path);
            }
        }
    }
    files.sort();

    let mut hasher = Sha256::new();
    for file in files {
        let rel = file
            .strip_prefix(path)
            .with_context(|| format!("failed to relativize digest path: {}", file.display()))?;
        hasher.update(normalized_path_to_string(rel).as_bytes());
        hasher.update([0]);
        hash_file_into(&mut hasher, &file, "directory digest file")?;
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn normalized_path_to_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_file_into(hasher: &mut Sha256, path: &Path, context_label: &str) -> anyhow::Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-lockfile-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    #[test]
    fn resolve_dependencies_rejects_unsupported_lock_version() {
        let root = new_temp_dir("unsupported-version");
        let lock = ImagoLock {
            version: 2,
            dependencies: vec![],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(&root, &lock, &[]).expect_err("must fail");
        assert!(err.to_string().contains("version '2' is not supported"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_dependency_mismatch() {
        let root = new_temp_dir("dep-mismatch");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
                resolved_at: "0".to_string(),
            }],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.2.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("does not match lock version"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_transitive_digest_mismatch() {
        let root = new_temp_dir("transitive-digest-mismatch");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
                resolved_at: "0".to_string(),
            }],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest:
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["demo:test".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(
            err.to_string()
                .contains("lock digest mismatch for transitive wit package")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_unknown_via_dependency() {
        let root = new_temp_dir("unknown-via");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/transitive/package.wit")).expect("digest")
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
                resolved_at: "0".to_string(),
            }],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest: transitive_digest,
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["demo:other".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("via contains unknown dependency"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn collect_wit_packages_groups_and_sorts_deterministically() {
        let packages = collect_wit_packages(vec![
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wa.dev".to_string()),
                requirement: "=0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.0".to_string()),
                path: "wit/deps/wasi_io".to_string(),
                via: "a:dep".to_string(),
            },
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wa.dev".to_string()),
                requirement: "=0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.0".to_string()),
                path: "wit/deps/wasi_io".to_string(),
                via: "b:dep".to_string(),
            },
        ]);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "wasi:io");
        assert_eq!(packages[0].versions.len(), 1);
        assert_eq!(
            packages[0].versions[0].via,
            vec!["a:dep".to_string(), "b:dep".to_string()]
        );
    }
}
