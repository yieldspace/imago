use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, anyhow};
use futures_util::TryStreamExt as _;
use sha2::{Digest, Sha256};
use wasm_pkg_client::Client;
use wasm_pkg_common::{
    config::{Config, RegistryMapping},
    package::{PackageRef, Version},
    registry::Registry,
};
use wit_component::DecodedWasm;
use wit_parser::UnresolvedPackageGroup;

pub(crate) const DEFAULT_WARG_REGISTRY: &str = "wa.dev";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializedWitComponent {
    pub source: String,
    pub registry: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MaterializedWitSource {
    pub derived_component: Option<MaterializedWitComponent>,
    pub transitive_packages: Vec<MaterializedTransitiveWitPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializedTransitiveWitPackage {
    pub name: String,
    pub registry: Option<String>,
    pub requirement: String,
    pub version: Option<String>,
    pub digest: String,
    pub source: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedWitSource {
    File {
        path: PathBuf,
        source: String,
    },
    Warg {
        package: String,
        version: String,
        registry: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedComponentSource {
    File(PathBuf),
    Warg {
        package: String,
        version: String,
        registry: String,
    },
}

pub(crate) fn sanitize_wit_deps_name(name: &str) -> String {
    // Keep dependency path naming compatible with wkg.
    name.replace([':', '@'], "-")
}

pub(crate) fn warg_local_package_key(package: &str) -> String {
    format!("pkg-{}", hex::encode(package.as_bytes()))
}

pub(crate) fn path_to_manifest_string(path: &Path) -> String {
    path.iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn normalize_registry_for_source(
    source: &str,
    registry: Option<&str>,
    field_name: &str,
) -> anyhow::Result<Option<String>> {
    if source.starts_with("file://") {
        if registry.is_some() {
            return Err(anyhow!(
                "{field_name}.registry is only allowed when source is warg://"
            ));
        }
        return Ok(None);
    }

    if source.starts_with("warg://") {
        let normalized = normalize_registry_name(
            registry
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_WARG_REGISTRY),
        )?;
        return Ok(Some(normalized));
    }

    Err(anyhow!(
        "{field_name}.source must start with one of: file://, warg://"
    ))
}

pub(crate) fn validate_wit_source(source: &str, field_name: &str) -> anyhow::Result<()> {
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

pub(crate) fn validate_component_source(source: &str, field_name: &str) -> anyhow::Result<()> {
    if source.starts_with("file://") || source.starts_with("warg://") {
        return Ok(());
    }
    Err(anyhow!(
        "{field_name} must start with one of: file://, warg://"
    ))
}

pub(crate) fn expected_component_identity_from_wit_source(
    source: &str,
    registry: Option<&str>,
) -> anyhow::Result<(String, Option<String>)> {
    if source.starts_with("file://") {
        return Ok((source.to_string(), None));
    }
    if let Some(spec) = source.strip_prefix("warg://") {
        let (package, version) = parse_warg_spec(spec)?;
        let registry = normalize_registry_name(registry.unwrap_or(DEFAULT_WARG_REGISTRY))?;
        return Ok((canonical_warg_source(package, version), Some(registry)));
    }
    if source.starts_with("https://wa.dev/") {
        return Err(anyhow!(
            "wit source no longer accepts https://wa.dev shorthand; use warg://<package>@<version>"
        ));
    }
    Err(anyhow!(
        "wit source must start with one of: file://, warg://"
    ))
}

pub(crate) fn validate_sha256_hex(value: &str, field_name: &str) -> anyhow::Result<()> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("{field_name} must be a 64-character hex string"));
    }
    Ok(())
}

pub(crate) async fn materialize_wit_source(
    project_root: &Path,
    source: &str,
    registry: Option<&str>,
    _dependency_version: &str,
    destination_dir: &Path,
) -> anyhow::Result<MaterializedWitSource> {
    let parsed = parse_wit_source(project_root, source, registry)?;
    match parsed {
        ParsedWitSource::File { path, source } => {
            let metadata = fs::metadata(&path)
                .with_context(|| format!("failed to inspect wit source: {}", path.display()))?;
            if metadata.is_dir() {
                copy_wit_tree(&path, destination_dir)?;
                return Ok(MaterializedWitSource::default());
            }

            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read wit source file: {}", path.display()))?;
            let mut reader = std::io::Cursor::new(bytes.as_slice());
            match wit_component::decode_reader(&mut reader) {
                Ok(DecodedWasm::WitPackage(resolve, top_package)) => {
                    let transitive_packages = materialize_wit_package_resolve(
                        destination_dir,
                        &resolve,
                        top_package,
                        None,
                        None,
                        None,
                        &format!("file source '{}'", path.display()),
                    )?;
                    Ok(MaterializedWitSource {
                        derived_component: None,
                        transitive_packages,
                    })
                }
                Ok(DecodedWasm::Component(resolve, world)) => {
                    let top_package = component_world_package_id(
                        &resolve,
                        world,
                        &format!("file source '{}'", path.display()),
                    )?;
                    let transitive_packages = materialize_wit_package_resolve(
                        destination_dir,
                        &resolve,
                        top_package,
                        None,
                        None,
                        None,
                        &format!("file source '{}'", path.display()),
                    )?;
                    Ok(MaterializedWitSource {
                        derived_component: Some(MaterializedWitComponent {
                            source,
                            registry: None,
                            sha256: hex::encode(Sha256::digest(&bytes)),
                        }),
                        transitive_packages,
                    })
                }
                Err(_) => {
                    copy_wit_tree(&path, destination_dir)?;
                    Ok(MaterializedWitSource::default())
                }
            }
        }
        ParsedWitSource::Warg {
            package,
            version,
            registry,
        } => {
            let bytes = fetch_warg_wit_bytes_with_local_fallback(
                project_root,
                &package,
                &version,
                &registry,
            )
            .await?;
            materialize_warg_wit_bytes(destination_dir, &bytes, &package, &version, &registry)
        }
    }
}

pub(crate) async fn resolve_component_sha256(
    project_root: &Path,
    source: &str,
    registry: Option<&str>,
    expected_sha256: Option<&str>,
) -> anyhow::Result<String> {
    let parsed = parse_component_source(project_root, source, registry)?;
    let digest = match parsed {
        ParsedComponentSource::File(path) => compute_sha256_hex(&path)?,
        ParsedComponentSource::Warg {
            package,
            version,
            registry,
        } => {
            if let Some(local_path) =
                find_local_warg_component_candidate(project_root, &package, &version)
            {
                compute_sha256_hex(&local_path)?
            } else {
                let bytes = fetch_warg_release_bytes(&package, &version, &registry).await?;
                hex::encode(Sha256::digest(&bytes))
            }
        }
    };

    if let Some(expected) = expected_sha256 {
        validate_sha256_hex(expected, "dependencies[].component.sha256")?;
        if !digest.eq_ignore_ascii_case(expected) {
            return Err(anyhow!(
                "component sha256 mismatch: expected {}, actual {}",
                expected,
                digest
            ));
        }
    }

    Ok(digest)
}

pub(crate) async fn materialize_component_file(
    project_root: &Path,
    source: &str,
    registry: Option<&str>,
    sha256: &str,
    destination_path: &Path,
    destination_label: &str,
) -> anyhow::Result<()> {
    validate_sha256_hex(sha256, "component_sha256")?;

    let parsed = parse_component_source(project_root, source, registry)?;
    match parsed {
        ParsedComponentSource::File(path) => {
            copy_component_source_to_destination(
                &path,
                destination_path,
                sha256,
                destination_label,
            )?;
        }
        ParsedComponentSource::Warg {
            package,
            version,
            registry,
        } => {
            if let Some(local_path) =
                find_local_warg_component_candidate(project_root, &package, &version)
            {
                copy_component_source_to_destination(
                    &local_path,
                    destination_path,
                    sha256,
                    destination_label,
                )?;
            } else {
                let bytes = fetch_warg_release_bytes(&package, &version, &registry).await?;
                let digest = hex::encode(Sha256::digest(&bytes));
                if digest != sha256 {
                    return Err(anyhow!(
                        "component sha256 mismatch: expected {}, actual {}",
                        sha256,
                        digest
                    ));
                }
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent).with_context(|| {
                        format!(
                            "failed to create {} destination dir: {}",
                            destination_label,
                            parent.display()
                        )
                    })?;
                }
                fs::write(destination_path, bytes).with_context(|| {
                    format!(
                        "failed to write {} file {}",
                        destination_label,
                        destination_path.display()
                    )
                })?;
            }
        }
    }

    Ok(())
}

fn copy_component_source_to_destination(
    source_path: &Path,
    destination_path: &Path,
    expected_sha256: &str,
    destination_label: &str,
) -> anyhow::Result<()> {
    let digest = compute_sha256_hex(source_path)?;
    if !digest.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "component sha256 mismatch: expected {}, actual {}",
            expected_sha256,
            digest
        ));
    }

    if destination_path.exists() {
        let existing_digest = compute_sha256_hex(destination_path)?;
        if existing_digest.eq_ignore_ascii_case(expected_sha256) {
            return Ok(());
        }
        return Err(anyhow!(
            "{} hash mismatch for {} (expected {}, actual {})",
            destination_label,
            destination_path.display(),
            expected_sha256,
            existing_digest
        ));
    }

    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create {} destination dir: {}",
                destination_label,
                parent.display()
            )
        })?;
    }

    fs::copy(source_path, destination_path).with_context(|| {
        format!(
            "failed to copy component source {} into {} {}",
            source_path.display(),
            destination_label,
            destination_path.display()
        )
    })?;
    Ok(())
}

fn parse_wit_source(
    project_root: &Path,
    source: &str,
    registry: Option<&str>,
) -> anyhow::Result<ParsedWitSource> {
    if let Some(raw_path) = source.strip_prefix("file://") {
        let path = resolve_file_source_path(project_root, raw_path)?;
        return Ok(ParsedWitSource::File {
            path,
            source: source.to_string(),
        });
    }

    if let Some(spec) = source.strip_prefix("warg://") {
        let (package, version) = parse_warg_spec(spec)?;
        let registry = normalize_registry_name(registry.unwrap_or(DEFAULT_WARG_REGISTRY))?;
        return Ok(ParsedWitSource::Warg {
            package: package.to_string(),
            version: version.to_string(),
            registry,
        });
    }

    Err(anyhow!(
        "wit source must start with one of: file://, warg://"
    ))
}

fn parse_component_source(
    project_root: &Path,
    source: &str,
    registry: Option<&str>,
) -> anyhow::Result<ParsedComponentSource> {
    if let Some(raw_path) = source.strip_prefix("file://") {
        let path = resolve_file_source_path(project_root, raw_path)?;
        let metadata = fs::metadata(&path)
            .with_context(|| format!("failed to inspect component source: {}", path.display()))?;
        if !metadata.is_file() {
            return Err(anyhow!(
                "file:// component source must point to a file: {}",
                path.display()
            ));
        }
        return Ok(ParsedComponentSource::File(path));
    }

    if let Some(spec) = source.strip_prefix("warg://") {
        let (package, version) = parse_warg_spec(spec)?;
        let registry = normalize_registry_name(registry.unwrap_or(DEFAULT_WARG_REGISTRY))?;
        return Ok(ParsedComponentSource::Warg {
            package: package.to_string(),
            version: version.to_string(),
            registry,
        });
    }

    Err(anyhow!(
        "component source must start with one of: file://, warg://"
    ))
}

fn resolve_file_source_path(project_root: &Path, raw_path: &str) -> anyhow::Result<PathBuf> {
    if raw_path.trim().is_empty() {
        return Err(anyhow!("file:// source path must not be empty"));
    }
    let path = PathBuf::from(raw_path);
    let resolved = if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    };
    let metadata = fs::metadata(&resolved).with_context(|| {
        format!(
            "resolved file:// source does not exist: {}",
            resolved.display()
        )
    })?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(anyhow!(
            "resolved file:// source is not a file or directory: {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn parse_warg_spec(spec: &str) -> anyhow::Result<(&str, &str)> {
    let (package, version) = spec
        .rsplit_once('@')
        .ok_or_else(|| anyhow!("warg source must be in form warg://<package>@<version>"))?;
    let package = package.trim();
    let version = version.trim();
    if package.is_empty() || version.is_empty() {
        return Err(anyhow!(
            "warg source must include both package and version (warg://<package>@<version>)"
        ));
    }
    validate_warg_package_for_local_path(package)?;
    validate_warg_version_for_local_path(version)?;
    Ok((package, version))
}

fn validate_warg_package_for_local_path(package: &str) -> anyhow::Result<()> {
    if package.contains('\\')
        || package
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(anyhow!(
            "warg source package contains invalid path components: {package}"
        ));
    }
    for component in Path::new(package).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(anyhow!(
                "warg source package contains invalid path components: {package}"
            ));
        }
    }
    Ok(())
}

fn validate_warg_version_for_local_path(version: &str) -> anyhow::Result<()> {
    if version.contains('/') || version.contains('\\') {
        return Err(anyhow!(
            "warg source version contains invalid path components: {version}"
        ));
    }
    let mut components = Path::new(version).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(anyhow!(
            "warg source version contains invalid path components: {version}"
        ));
    }
    Ok(())
}

fn canonical_warg_source(package: &str, version: &str) -> String {
    format!("warg://{package}@{version}")
}

fn normalize_registry_name(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let no_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    if no_scheme.is_empty() {
        return Err(anyhow!("registry must not be empty"));
    }
    Ok(no_scheme.to_string())
}

fn materialize_warg_wit_bytes(
    destination_dir: &Path,
    bytes: &[u8],
    package: &str,
    version: &str,
    registry: &str,
) -> anyhow::Result<MaterializedWitSource> {
    let source_desc = format!("warg://{package}@{version} (registry={registry})");
    let mut reader = std::io::Cursor::new(bytes);
    match wit_component::decode_reader(&mut reader) {
        Ok(DecodedWasm::WitPackage(resolve, top_package)) => {
            let transitive_packages = materialize_wit_package_resolve(
                destination_dir,
                &resolve,
                top_package,
                Some(package),
                Some(version),
                Some(registry),
                &source_desc,
            )?;
            Ok(MaterializedWitSource {
                derived_component: None,
                transitive_packages,
            })
        }
        Ok(DecodedWasm::Component(resolve, world)) => {
            let top_package =
                select_top_package_for_component(&resolve, world, package, &source_desc)?;
            let transitive_packages = materialize_wit_package_resolve(
                destination_dir,
                &resolve,
                top_package,
                Some(package),
                Some(version),
                Some(registry),
                &source_desc,
            )?;
            Ok(MaterializedWitSource {
                derived_component: Some(MaterializedWitComponent {
                    source: canonical_warg_source(package, version),
                    registry: Some(registry.to_string()),
                    sha256: hex::encode(Sha256::digest(bytes)),
                }),
                transitive_packages,
            })
        }
        Err(_) => {
            materialize_plain_wit_text(destination_dir, bytes, package, version, registry)?;
            Ok(MaterializedWitSource::default())
        }
    }
}

fn materialize_plain_wit_text(
    destination_dir: &Path,
    bytes: &[u8],
    package: &str,
    version: &str,
    registry: &str,
) -> anyhow::Result<()> {
    let text = String::from_utf8(bytes.to_vec()).with_context(|| {
        format!(
            "failed to decode wit source for package '{}@{}' from registry '{}': payload is not UTF-8",
            package, version, registry
        )
    })?;
    let unresolved = UnresolvedPackageGroup::parse("dependency.wit", &text).with_context(|| {
        format!(
            "failed to parse plain WIT source for package '{}@{}' from registry '{}'",
            package, version, registry
        )
    })?;
    let has_foreign_deps = !unresolved.main.foreign_deps.is_empty()
        || unresolved
            .nested
            .iter()
            .any(|nested| !nested.foreign_deps.is_empty());
    if has_foreign_deps {
        return Err(anyhow!(
            "warg source '{}@{}' contains foreign imports in plain .wit form; publish/use a WIT package so `imago update` can resolve transitive dependencies",
            package,
            version
        ));
    }
    fs::write(destination_dir.join("dependency.wit"), text).with_context(|| {
        format!(
            "failed to write resolved wit into {}",
            destination_dir.display()
        )
    })?;
    Ok(())
}

fn materialize_wit_package_resolve(
    destination_dir: &Path,
    resolve: &wit_parser::Resolve,
    top_package: wit_parser::PackageId,
    expected_package: Option<&str>,
    expected_version: Option<&str>,
    registry: Option<&str>,
    source_desc: &str,
) -> anyhow::Result<Vec<MaterializedTransitiveWitPackage>> {
    let deps_root = destination_dir.parent().ok_or_else(|| {
        anyhow!(
            "failed to resolve deps root for destination {}",
            destination_dir.display()
        )
    })?;
    validate_top_package_version(
        resolve,
        top_package,
        expected_package,
        expected_version,
        source_desc,
    )?;
    let top_text = render_wit_package(resolve, top_package)?;
    let top_path = destination_dir.join("package.wit");
    write_or_verify_identical_wit_file(&top_path, &top_text).with_context(|| {
        format!(
            "failed to write top-level WIT package into {}",
            destination_dir.display()
        )
    })?;
    let mut materialized = Vec::new();

    let mut transitive = resolve
        .packages
        .iter()
        .filter(|(pkg_id, _)| *pkg_id != top_package)
        .map(|(pkg_id, pkg)| {
            let package_name = format!("{}:{}", pkg.name.namespace, pkg.name.name);
            (sanitize_wit_package_name(&pkg.name), package_name, pkg_id)
        })
        .collect::<Vec<_>>();
    transitive.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut sanitized_to_package: BTreeMap<String, String> = BTreeMap::new();
    for (sanitized, package_id, pkg_id) in transitive {
        if let Some(existing_package) = sanitized_to_package.get(&sanitized)
            && existing_package != &package_id
        {
            return Err(anyhow!(
                "conflicting transitive package path '{}' for '{}' and '{}'",
                sanitized,
                existing_package,
                package_id
            ));
        }
        sanitized_to_package.insert(sanitized.clone(), package_id);

        let package_name = &resolve.packages[pkg_id].name;
        let pkg_text = render_wit_package(resolve, pkg_id)?;
        let global_path = deps_root.join(&sanitized).join("package.wit");
        write_or_verify_identical_wit_file(&global_path, &pkg_text).with_context(|| {
            format!(
                "failed to materialize transitive package into {}",
                global_path.display()
            )
        })?;

        let version = package_name.version.as_ref().map(ToString::to_string);
        let requirement = match version.as_deref() {
            Some(value) => format!("={value}"),
            None => "*".to_string(),
        };
        let source = match (registry, version.as_deref()) {
            (Some(_), Some(version)) => Some(canonical_warg_source(
                &format!("{}:{}", package_name.namespace, package_name.name),
                version,
            )),
            _ => None,
        };
        let digest = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(pkg_text.as_bytes()))
        );
        let path = path_to_manifest_string(
            &PathBuf::from("wit")
                .join("deps")
                .join(sanitize_wit_package_name(package_name)),
        );

        materialized.push(MaterializedTransitiveWitPackage {
            name: format!("{}:{}", package_name.namespace, package_name.name),
            registry: registry.map(str::to_string),
            requirement,
            version,
            digest,
            source,
            path,
        });
    }

    Ok(materialized)
}

fn validate_top_package_version(
    resolve: &wit_parser::Resolve,
    top_package: wit_parser::PackageId,
    expected_package: Option<&str>,
    expected_version: Option<&str>,
    source_desc: &str,
) -> anyhow::Result<()> {
    let top = &resolve.packages[top_package].name;
    let actual_package = format!("{}:{}", top.namespace, top.name);
    if let Some(expected_package) = expected_package
        && actual_package != expected_package
    {
        return Err(anyhow!(
            "top-level WIT package mismatch for {source_desc}: expected package '{}', actual '{}'",
            expected_package,
            actual_package
        ));
    }

    if let (Some(expected_version), Some(actual_version)) = (expected_version, top.version.as_ref())
    {
        let actual_version = actual_version.to_string();
        if actual_version != expected_version {
            return Err(anyhow!(
                "top-level WIT package '{}:{}' version mismatch for {source_desc}: expected '{}', actual '{}'",
                top.namespace,
                top.name,
                expected_version,
                actual_version
            ));
        }
    }
    Ok(())
}

fn component_world_package_id(
    resolve: &wit_parser::Resolve,
    world: wit_parser::WorldId,
    source_desc: &str,
) -> anyhow::Result<wit_parser::PackageId> {
    resolve.worlds[world].package.ok_or_else(|| {
        anyhow!("failed to resolve package metadata for component world from {source_desc}")
    })
}

fn select_top_package_for_component(
    resolve: &wit_parser::Resolve,
    world: wit_parser::WorldId,
    expected_package: &str,
    source_desc: &str,
) -> anyhow::Result<wit_parser::PackageId> {
    let world_package = component_world_package_id(resolve, world, source_desc)?;
    let Some((expected_namespace, expected_name)) = expected_package.split_once(':') else {
        return Ok(world_package);
    };
    for (pkg_id, pkg) in resolve.packages.iter() {
        if pkg.name.namespace == expected_namespace && pkg.name.name == expected_name {
            return Ok(pkg_id);
        }
    }
    Ok(world_package)
}

fn render_wit_package(
    resolve: &wit_parser::Resolve,
    package: wit_parser::PackageId,
) -> anyhow::Result<String> {
    let mut printer = wit_component::WitPrinter::default();
    printer
        .print(resolve, package, &[])
        .context("failed to print WIT package")?;
    Ok(printer.output.to_string())
}

fn sanitize_wit_package_name(name: &wit_parser::PackageName) -> String {
    sanitize_wit_deps_name(&format!("{}:{}", name.namespace, name.name))
}

fn write_or_verify_identical_wit_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create wit output dir {}", parent.display()))?;
    }
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read existing wit file {}", path.display()))?;
        if existing != contents {
            return Err(anyhow!(
                "conflicting transitive WIT package detected at {}",
                path.display()
            ));
        }
        return Ok(());
    }
    fs::write(path, contents)
        .with_context(|| format!("failed to write transitive wit file {}", path.display()))?;
    Ok(())
}

async fn fetch_warg_release_bytes(
    package: &str,
    version: &str,
    registry: &str,
) -> anyhow::Result<Vec<u8>> {
    let package_ref: PackageRef = package
        .parse()
        .with_context(|| format!("invalid package name in warg source: {package}"))?;
    let version: Version = version
        .parse()
        .with_context(|| format!("invalid package version in warg source: {version}"))?;
    let registry: Registry = registry
        .parse()
        .with_context(|| format!("invalid registry value: {registry}"))?;

    let mut config = Config::empty();
    config.set_default_registry(Some(registry.clone()));
    config.set_package_registry_override(package_ref.clone(), RegistryMapping::Registry(registry));

    let client = Client::new(config);
    let release = client
        .get_release(&package_ref, &version)
        .await
        .map_err(|err| anyhow!("failed to get release for {package_ref}@{version}: {err:#}"))?;
    let mut stream = client
        .stream_content(&package_ref, &release)
        .await
        .map_err(|err| anyhow!("failed to stream release content: {err:#}"))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.try_next().await? {
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn fetch_warg_wit_bytes_with_local_fallback(
    project_root: &Path,
    package: &str,
    version: &str,
    registry: &str,
) -> anyhow::Result<Vec<u8>> {
    if let Some(local) = find_local_warg_wit_candidate(project_root, package, version) {
        return fs::read(&local)
            .with_context(|| format!("failed to read local warg wit source {}", local.display()));
    }
    fetch_warg_release_bytes(package, version, registry).await
}

fn find_local_warg_wit_candidate(
    project_root: &Path,
    package: &str,
    version: &str,
) -> Option<PathBuf> {
    let package_dir = warg_local_package_key(package);
    let base = project_root
        .join(".imago")
        .join("warg")
        .join(package_dir)
        .join(version);
    [
        base.join("wit.wasm"),
        base.join("wit"),
        base.join("wit.wit"),
        project_root.join(".imago").join("warg").join(format!(
            "{}@{}.wit",
            warg_local_package_key(package),
            version
        )),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn find_local_warg_component_candidate(
    project_root: &Path,
    package: &str,
    version: &str,
) -> Option<PathBuf> {
    let package_dir = warg_local_package_key(package);
    let base = project_root
        .join(".imago")
        .join("warg")
        .join(package_dir)
        .join(version);
    [
        base.join("wit.wasm"),
        base.join("wit"),
        base.join("component.wasm"),
        base.join("component").join("component.wasm"),
        project_root.join(".imago").join("warg").join(format!(
            "{}@{}.wasm",
            warg_local_package_key(package),
            version
        )),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open file for sha256: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read file for sha256: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn copy_wit_tree(source: &Path, destination_dir: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect wit source: {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "wit source must not contain symlinks: {}",
            source.display()
        ));
    }
    if metadata.is_file() {
        let file_name = source
            .file_name()
            .ok_or_else(|| anyhow!("wit source file name is missing: {}", source.display()))?;
        fs::copy(source, destination_dir.join(file_name)).with_context(|| {
            format!(
                "failed to copy wit file {} -> {}",
                source.display(),
                destination_dir.display()
            )
        })?;
        return Ok(());
    }

    copy_dir_contents(source, destination_dir)
}

fn copy_dir_contents(source_dir: &Path, destination_dir: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("failed to read directory: {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to read directory entry in {}", source_dir.display())
        })?;
        let source_path = entry.path();
        let relative = source_path.strip_prefix(source_dir).with_context(|| {
            format!(
                "failed to compute relative path for {}",
                source_path.display()
            )
        })?;
        let destination_path = destination_dir.join(relative);
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect source path: {}", source_path.display()))?;
        if file_type.is_symlink() {
            return Err(anyhow!(
                "wit source must not contain symlinks: {}",
                source_path.display()
            ));
        }
        if file_type.is_dir() {
            fs::create_dir_all(&destination_path).with_context(|| {
                format!(
                    "failed to create destination directory: {}",
                    destination_path.display()
                )
            })?;
            copy_dir_contents(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create destination parent: {}", parent.display())
                })?;
            }
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "failed to copy wit file {} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{copy_wit_tree, parse_warg_spec, sanitize_wit_deps_name, warg_local_package_key};
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-cli-plugin-sources-tests-{}-{}-{}",
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
    fn parse_warg_spec_accepts_nested_package_name() {
        let (package, version) =
            parse_warg_spec("yieldspace:plugin/example@1.0.0").expect("must parse");
        assert_eq!(package, "yieldspace:plugin/example");
        assert_eq!(version, "1.0.0");
    }

    #[test]
    fn parse_warg_spec_rejects_traversal_package_name() {
        let err = parse_warg_spec("../../tmp/evil@1.0.0").expect_err("must reject traversal");
        assert!(
            err.to_string()
                .contains("warg source package contains invalid path components")
        );
    }

    #[test]
    fn parse_warg_spec_rejects_empty_package_segment() {
        let err = parse_warg_spec("yieldspace:plugin//example@1.0.0")
            .expect_err("must reject empty segment");
        assert!(
            err.to_string()
                .contains("warg source package contains invalid path components")
        );
    }

    #[test]
    fn parse_warg_spec_rejects_invalid_version_path() {
        let err = parse_warg_spec("yieldspace:plugin/example@../1.0.0")
            .expect_err("must reject invalid version path");
        assert!(
            err.to_string()
                .contains("warg source version contains invalid path components")
        );
    }

    #[test]
    fn warg_local_package_key_avoids_sanitized_name_collisions() {
        let left = "foo:bar-baz";
        let right = "foo-bar:baz";
        assert_eq!(
            sanitize_wit_deps_name(left),
            sanitize_wit_deps_name(right),
            "precondition: sanitized wit/deps names should collide"
        );
        assert_ne!(
            warg_local_package_key(left),
            warg_local_package_key(right),
            "local warg cache key must stay collision-free"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_wit_tree_rejects_symlinked_directory_entries() {
        let root = new_temp_dir("copy-wit-tree-symlink-entry");
        let source = root.join("source");
        let destination = root.join("destination");
        let outside = root.join("outside");

        write(&source.join("package.wit"), b"package test:source;\n");
        write(&outside.join("package.wit"), b"package test:outside;\n");
        symlink(&outside, source.join("linked")).expect("symlink should be created");

        let err = copy_wit_tree(&source, &destination).expect_err("symlinked entry must fail");
        assert!(
            err.to_string()
                .contains("wit source must not contain symlinks"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn copy_wit_tree_rejects_symlink_root_source() {
        let root = new_temp_dir("copy-wit-tree-symlink-root");
        let source_real = root.join("source-real");
        let source_link = root.join("source-link");
        let destination = root.join("destination");

        write(&source_real.join("package.wit"), b"package test:source;\n");
        symlink(&source_real, &source_link).expect("symlink should be created");

        let err = copy_wit_tree(&source_link, &destination).expect_err("symlink root must fail");
        assert!(
            err.to_string()
                .contains("wit source must not contain symlinks"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
