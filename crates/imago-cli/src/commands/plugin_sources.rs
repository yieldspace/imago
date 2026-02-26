use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, anyhow};
use futures_util::TryStreamExt as _;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use wasm_pkg_client::Client;
use wasm_pkg_common::{
    config::{Config, CustomConfig, RegistryMapping},
    label::Label,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, UnresolvedPackageGroup};

pub(crate) const DEFAULT_WARG_REGISTRY: &str = "wa.dev";
pub(crate) const DEFAULT_WASI_WARG_REGISTRY: &str = "wasi.dev";

pub(crate) type NamespaceRegistries = BTreeMap<String, String>;

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
    Remote {
        protocol: RemoteSourceProtocol,
        package: String,
        version: String,
        registry: String,
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedComponentSource {
    File(PathBuf),
    Remote {
        protocol: RemoteSourceProtocol,
        package: String,
        version: String,
        registry: String,
        source: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteSourceProtocol {
    Warg,
    Oci,
}

impl RemoteSourceProtocol {
    fn scheme(self) -> &'static str {
        match self {
            Self::Warg => "warg",
            Self::Oci => "oci",
        }
    }

    fn default_backend(self) -> &'static str {
        match self {
            Self::Warg => "warg",
            Self::Oci => "oci",
        }
    }
}

pub(crate) fn sanitize_wit_deps_name(name: &str) -> String {
    // Keep dependency path naming compatible with wkg.
    name.replace([':', '@'], "-")
}

pub(crate) fn warg_local_package_key(package: &str) -> String {
    format!("pkg-{}", hex::encode(package.as_bytes()))
}

fn oci_local_package_key(registry: &str, package: &str) -> String {
    format!(
        "pkg-{}",
        hex::encode(format!("{registry}/{package}").as_bytes())
    )
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
    namespace_registries: Option<&NamespaceRegistries>,
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

    if let Some(spec) = source.strip_prefix("warg://") {
        let (package, _) = parse_warg_spec(spec)?;
        let normalized =
            resolve_warg_registry_for_package(package, registry, namespace_registries)?;
        return Ok(Some(normalized));
    }

    if source.starts_with("oci://") {
        if registry.is_some() {
            return Err(anyhow!(
                "{field_name}.registry is not allowed when source is oci://"
            ));
        }
        return Ok(None);
    }

    Err(anyhow!(
        "{field_name}.source must start with one of: file://, warg://, oci://"
    ))
}

pub(crate) fn validate_wit_source(source: &str, field_name: &str) -> anyhow::Result<()> {
    if source.starts_with("file://")
        || source.starts_with("warg://")
        || source.starts_with("oci://")
    {
        return Ok(());
    }
    if source.starts_with("https://wa.dev/") {
        return Err(anyhow!(
            "{field_name} no longer accepts https://wa.dev shorthand; use warg://<package>@<version>"
        ));
    }
    Err(anyhow!(
        "{field_name} must start with one of: file://, warg://, oci://"
    ))
}

pub(crate) fn validate_component_source(source: &str, field_name: &str) -> anyhow::Result<()> {
    if source.starts_with("file://")
        || source.starts_with("warg://")
        || source.starts_with("oci://")
    {
        return Ok(());
    }
    Err(anyhow!(
        "{field_name} must start with one of: file://, warg://, oci://"
    ))
}

pub(crate) fn resolve_warg_registry_for_package(
    package: &str,
    explicit_registry: Option<&str>,
    namespace_registries: Option<&NamespaceRegistries>,
) -> anyhow::Result<String> {
    resolve_warg_registry_for_package_with_fallback(
        package,
        explicit_registry,
        namespace_registries,
        None,
    )
}

pub(crate) fn resolve_warg_registry_for_package_with_fallback(
    package: &str,
    explicit_registry: Option<&str>,
    namespace_registries: Option<&NamespaceRegistries>,
    fallback_default_registry: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(value) = explicit_registry
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return normalize_registry_name(value);
    }
    if let Some(namespace) = extract_package_namespace(package)
        && let Some(registry) = namespace_registries.and_then(|map| map.get(namespace))
    {
        return normalize_registry_name(registry);
    }
    if extract_package_namespace(package) == Some("wasi") {
        return Ok(DEFAULT_WASI_WARG_REGISTRY.to_string());
    }
    if let Some(fallback_registry) = fallback_default_registry
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return normalize_registry_name(fallback_registry);
    }
    Ok(DEFAULT_WARG_REGISTRY.to_string())
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
        let registry = resolve_warg_registry_for_package(package, registry, None)?;
        return Ok((canonical_warg_source(package, version), Some(registry)));
    }
    if let Some(spec) = source.strip_prefix("oci://") {
        if registry.is_some() {
            return Err(anyhow!(
                "wit registry is not allowed when wit source is oci://"
            ));
        }
        let parsed = parse_oci_spec(spec)?;
        return Ok((parsed.source, None));
    }
    if source.starts_with("https://wa.dev/") {
        return Err(anyhow!(
            "wit source no longer accepts https://wa.dev shorthand; use warg://<package>@<version>"
        ));
    }
    Err(anyhow!(
        "wit source must start with one of: file://, warg://, oci://"
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
    namespace_registries: Option<&NamespaceRegistries>,
    expected_package: Option<&str>,
    destination_dir: &Path,
) -> anyhow::Result<MaterializedWitSource> {
    let parsed = parse_wit_source(project_root, source, registry, namespace_registries)?;
    match parsed {
        ParsedWitSource::File { path, source } => {
            ensure_wit_source_has_no_symlinks(&path)?;
            let metadata = fs::metadata(&path)
                .with_context(|| format!("failed to inspect wit source: {}", path.display()))?;
            if metadata.is_dir() {
                if let Ok((resolve, top_package)) = parse_local_wit_package_dir(&path) {
                    let source_desc = format!("file source '{}'", path.display());
                    let transitive_packages = materialize_wit_package_resolve(
                        destination_dir,
                        &resolve,
                        top_package,
                        WitPackageResolveOptions {
                            expected_package,
                            expected_version: None,
                            source_detail: None,
                            namespace_registries,
                            source_desc: &source_desc,
                        },
                    )?;
                    return Ok(MaterializedWitSource {
                        derived_component: None,
                        transitive_packages,
                    });
                }
                copy_wit_tree(&path, destination_dir)?;
                return Ok(MaterializedWitSource::default());
            }

            let bytes = fs::read(&path)
                .with_context(|| format!("failed to read wit source file: {}", path.display()))?;
            let mut reader = std::io::Cursor::new(bytes.as_slice());
            match wit_component::decode_reader(&mut reader) {
                Ok(DecodedWasm::WitPackage(resolve, top_package)) => {
                    let source_desc = format!("file source '{}'", path.display());
                    let transitive_packages = materialize_wit_package_resolve(
                        destination_dir,
                        &resolve,
                        top_package,
                        WitPackageResolveOptions {
                            expected_package: None,
                            expected_version: None,
                            source_detail: None,
                            namespace_registries,
                            source_desc: &source_desc,
                        },
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
                    let source_desc = format!("file source '{}'", path.display());
                    let transitive_packages = materialize_wit_package_resolve(
                        destination_dir,
                        &resolve,
                        top_package,
                        WitPackageResolveOptions {
                            expected_package: None,
                            expected_version: None,
                            source_detail: None,
                            namespace_registries,
                            source_desc: &source_desc,
                        },
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
        ParsedWitSource::Remote {
            protocol,
            package,
            version,
            registry,
            source,
        } => {
            let bytes = fetch_wit_bytes_with_local_fallback(
                project_root,
                protocol,
                &package,
                &version,
                &registry,
                namespace_registries,
            )
            .await?;
            materialize_remote_wit_bytes(
                destination_dir,
                &bytes,
                MaterializeRemoteWitBytesRequest {
                    protocol,
                    canonical_source: &source,
                    package: &package,
                    version: &version,
                    registry: &registry,
                    namespace_registries,
                    expected_package,
                },
            )
        }
    }
}

fn ensure_wit_source_has_no_symlinks(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect wit source: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "wit source must not contain symlinks: {}",
            path.display()
        ));
    }
    if metadata.is_dir() {
        ensure_wit_dir_has_no_symlink_entries(path)?;
    }
    Ok(())
}

fn ensure_wit_dir_has_no_symlink_entries(path: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(path)
        .with_context(|| format!("failed to read directory: {}", path.display()))?
    {
        let entry = entry
            .with_context(|| format!("failed to read directory entry in {}", path.display()))?;
        let source_path = entry.path();
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
            ensure_wit_dir_has_no_symlink_entries(&source_path)?;
        }
    }
    Ok(())
}

fn parse_local_wit_package_dir(path: &Path) -> anyhow::Result<(Resolve, wit_parser::PackageId)> {
    let mut resolve = Resolve::default();
    let (top_package, _) = resolve.push_path(path).with_context(|| {
        format!(
            "failed to parse local WIT package directory {}",
            path.display()
        )
    })?;
    Ok((resolve, top_package))
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
        ParsedComponentSource::Remote {
            protocol,
            package,
            version,
            registry,
            ..
        } => {
            if let Some(local_path) = find_local_component_candidate(
                project_root,
                protocol,
                &package,
                &version,
                &registry,
            ) {
                compute_sha256_hex(&local_path)?
            } else {
                let bytes =
                    fetch_release_bytes(protocol, &package, &version, &registry, None).await?;
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
        ParsedComponentSource::Remote {
            protocol,
            package,
            version,
            registry,
            ..
        } => {
            if let Some(local_path) = find_local_component_candidate(
                project_root,
                protocol,
                &package,
                &version,
                &registry,
            ) {
                copy_component_source_to_destination(
                    &local_path,
                    destination_path,
                    sha256,
                    destination_label,
                )?;
            } else {
                let bytes =
                    fetch_release_bytes(protocol, &package, &version, &registry, None).await?;
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
    namespace_registries: Option<&NamespaceRegistries>,
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
        let registry = resolve_warg_registry_for_package(package, registry, namespace_registries)?;
        let source = canonical_warg_source(package, version);
        return Ok(ParsedWitSource::Remote {
            protocol: RemoteSourceProtocol::Warg,
            package: package.to_string(),
            version: version.to_string(),
            registry,
            source,
        });
    }

    if let Some(spec) = source.strip_prefix("oci://") {
        if registry.is_some() {
            return Err(anyhow!(
                "wit registry is not allowed when wit source is oci://"
            ));
        }
        let parsed = parse_oci_spec(spec)?;
        return Ok(ParsedWitSource::Remote {
            protocol: RemoteSourceProtocol::Oci,
            package: parsed.package,
            version: parsed.version,
            registry: parsed.registry,
            source: parsed.source,
        });
    }

    Err(anyhow!(
        "wit source must start with one of: file://, warg://, oci://"
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
        let registry = resolve_warg_registry_for_package(package, registry, None)?;
        let source = canonical_warg_source(package, version);
        return Ok(ParsedComponentSource::Remote {
            protocol: RemoteSourceProtocol::Warg,
            package: package.to_string(),
            version: version.to_string(),
            registry,
            source,
        });
    }

    if let Some(spec) = source.strip_prefix("oci://") {
        if registry.is_some() {
            return Err(anyhow!(
                "component registry is not allowed when component source is oci://"
            ));
        }
        let parsed = parse_oci_spec(spec)?;
        return Ok(ParsedComponentSource::Remote {
            protocol: RemoteSourceProtocol::Oci,
            package: parsed.package,
            version: parsed.version,
            registry: parsed.registry,
            source: parsed.source,
        });
    }

    Err(anyhow!(
        "component source must start with one of: file://, warg://, oci://"
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

#[derive(Debug, Clone)]
struct ParsedOciSpec {
    package: String,
    version: String,
    registry: String,
    source: String,
}

fn parse_oci_spec(spec: &str) -> anyhow::Result<ParsedOciSpec> {
    let (registry_and_package, version) = spec.rsplit_once('@').ok_or_else(|| {
        anyhow!("oci source must be in form oci://<registry>/<namespace>/<name...>@<version>")
    })?;
    let registry_and_package = registry_and_package.trim();
    let version = version.trim();
    if registry_and_package.is_empty() || version.is_empty() {
        return Err(anyhow!(
            "oci source must include registry, package, and version (oci://<registry>/<namespace>/<name...>@<version>)"
        ));
    }
    validate_warg_version_for_local_path(version)?;
    let (registry_raw, package_path) = registry_and_package.split_once('/').ok_or_else(|| {
        anyhow!(
            "oci source must include package path (oci://<registry>/<namespace>/<name...>@<version>)"
        )
    })?;
    let registry = normalize_registry_name(registry_raw)?;
    if package_path.contains('\\') {
        return Err(anyhow!(
            "oci source package contains invalid path components: {package_path}"
        ));
    }
    let mut segments = package_path.split('/').map(str::trim);
    let namespace = segments.next().unwrap_or_default();
    let name_segments = segments.collect::<Vec<_>>();
    if namespace.is_empty()
        || name_segments.is_empty()
        || name_segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        || namespace == "."
        || namespace == ".."
    {
        return Err(anyhow!(
            "oci source package must be '<namespace>/<name...>': {package_path}"
        ));
    }
    let package = format!("{namespace}:{}", name_segments.join("/"));
    let _ = resolve_oci_package_for_client(&package)?;
    let _: Version = version
        .parse()
        .with_context(|| format!("invalid package version in oci source: {version}"))?;
    let source = canonical_oci_source(&registry, &package, version)?;
    Ok(ParsedOciSpec {
        package,
        version: version.to_string(),
        registry,
        source,
    })
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

fn canonical_oci_source(registry: &str, package: &str, version: &str) -> anyhow::Result<String> {
    let (namespace, name) = package
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid package name for oci source: {package}"))?;
    Ok(format!("oci://{registry}/{namespace}/{name}@{version}"))
}

fn extract_package_namespace(package: &str) -> Option<&str> {
    package.split_once(':').map(|(namespace, _)| namespace)
}

pub(crate) fn normalize_registry_name(raw: &str) -> anyhow::Result<String> {
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

#[derive(Debug, Clone, Copy)]
struct MaterializeSourceDetail<'a> {
    protocol: RemoteSourceProtocol,
    registry: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct MaterializeRemoteWitBytesRequest<'a> {
    protocol: RemoteSourceProtocol,
    canonical_source: &'a str,
    package: &'a str,
    version: &'a str,
    registry: &'a str,
    namespace_registries: Option<&'a NamespaceRegistries>,
    expected_package: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct MaterializePlainWitTextRequest<'a> {
    protocol: RemoteSourceProtocol,
    package: &'a str,
    version: &'a str,
    registry: &'a str,
    expected_package: Option<&'a str>,
    expected_version: Option<&'a str>,
    source_desc: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct WitPackageResolveOptions<'a> {
    expected_package: Option<&'a str>,
    expected_version: Option<&'a str>,
    source_detail: Option<MaterializeSourceDetail<'a>>,
    namespace_registries: Option<&'a NamespaceRegistries>,
    source_desc: &'a str,
}

fn materialize_remote_wit_bytes(
    destination_dir: &Path,
    bytes: &[u8],
    request: MaterializeRemoteWitBytesRequest<'_>,
) -> anyhow::Result<MaterializedWitSource> {
    let source_desc = format!(
        "{} (registry={})",
        request.canonical_source, request.registry
    );
    let source_detail = MaterializeSourceDetail {
        protocol: request.protocol,
        registry: request.registry,
    };
    let mut reader = std::io::Cursor::new(bytes);
    match wit_component::decode_reader(&mut reader) {
        Ok(DecodedWasm::WitPackage(resolve, top_package)) => {
            let transitive_packages = materialize_wit_package_resolve(
                destination_dir,
                &resolve,
                top_package,
                WitPackageResolveOptions {
                    expected_package: request.expected_package,
                    expected_version: Some(request.version),
                    source_detail: Some(source_detail),
                    namespace_registries: request.namespace_registries,
                    source_desc: &source_desc,
                },
            )?;
            Ok(MaterializedWitSource {
                derived_component: None,
                transitive_packages,
            })
        }
        Ok(DecodedWasm::Component(resolve, world)) => {
            let top_package = select_top_package_for_component(&resolve, world, &source_desc)?;
            let transitive_packages = materialize_wit_package_resolve(
                destination_dir,
                &resolve,
                top_package,
                WitPackageResolveOptions {
                    expected_package: request.expected_package,
                    expected_version: Some(request.version),
                    source_detail: Some(source_detail),
                    namespace_registries: request.namespace_registries,
                    source_desc: &source_desc,
                },
            )?;
            Ok(MaterializedWitSource {
                derived_component: Some(MaterializedWitComponent {
                    source: request.canonical_source.to_string(),
                    registry: (request.protocol == RemoteSourceProtocol::Warg)
                        .then(|| request.registry.to_string()),
                    sha256: hex::encode(Sha256::digest(bytes)),
                }),
                transitive_packages,
            })
        }
        Err(_) => {
            materialize_plain_wit_text(
                destination_dir,
                bytes,
                MaterializePlainWitTextRequest {
                    protocol: request.protocol,
                    package: request.package,
                    version: request.version,
                    registry: request.registry,
                    expected_package: request.expected_package,
                    expected_version: Some(request.version),
                    source_desc: &source_desc,
                },
            )?;
            Ok(MaterializedWitSource::default())
        }
    }
}

fn materialize_plain_wit_text(
    destination_dir: &Path,
    bytes: &[u8],
    request: MaterializePlainWitTextRequest<'_>,
) -> anyhow::Result<()> {
    let scheme = match request.protocol {
        RemoteSourceProtocol::Warg => "warg",
        RemoteSourceProtocol::Oci => "oci",
    };
    let text = String::from_utf8(bytes.to_vec()).with_context(|| {
        format!(
            "failed to decode wit source for package '{}@{}' from registry '{}': payload is not UTF-8",
            request.package, request.version, request.registry
        )
    })?;
    let unresolved = UnresolvedPackageGroup::parse("dependency.wit", &text).with_context(|| {
        format!(
            "failed to parse plain WIT source for package '{}@{}' from registry '{}'",
            request.package, request.version, request.registry
        )
    })?;
    let actual_package = format!(
        "{}:{}",
        unresolved.main.name.namespace, unresolved.main.name.name
    );
    if let Some(expected_package) = request.expected_package
        && actual_package != expected_package
    {
        return Err(anyhow!(
            "top-level WIT package mismatch for {}: expected package '{}', actual '{}'",
            request.source_desc,
            expected_package,
            actual_package
        ));
    }
    if let (Some(expected_version), Some(actual_version)) = (
        request.expected_version,
        unresolved.main.name.version.as_ref(),
    ) {
        let actual_version = actual_version.to_string();
        if actual_version != expected_version {
            return Err(anyhow!(
                "top-level WIT package '{}:{}' version mismatch for {}: expected '{}', actual '{}'",
                unresolved.main.name.namespace,
                unresolved.main.name.name,
                request.source_desc,
                expected_version,
                actual_version
            ));
        }
    }
    let has_foreign_deps = !unresolved.main.foreign_deps.is_empty()
        || unresolved
            .nested
            .iter()
            .any(|nested| !nested.foreign_deps.is_empty());
    if has_foreign_deps {
        return Err(anyhow!(
            "{} source '{}@{}' contains foreign imports in plain .wit form; publish/use a WIT package so `imago update` can resolve transitive dependencies",
            scheme,
            request.package,
            request.version
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
    options: WitPackageResolveOptions<'_>,
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
        options.expected_package,
        options.expected_version,
        options.source_desc,
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
        let package_ref = format!("{}:{}", package_name.namespace, package_name.name);
        let (resolved_registry, source) = match (options.source_detail, version.as_deref()) {
            (Some(detail), Some(version)) => match detail.protocol {
                RemoteSourceProtocol::Warg => {
                    let transitive_registry = resolve_warg_registry_for_package_with_fallback(
                        &package_ref,
                        None,
                        options.namespace_registries,
                        Some(detail.registry),
                    )?;
                    (
                        Some(transitive_registry),
                        Some(canonical_warg_source(&package_ref, version)),
                    )
                }
                RemoteSourceProtocol::Oci => (
                    None,
                    Some(canonical_oci_source(
                        detail.registry,
                        &package_ref,
                        version,
                    )?),
                ),
            },
            _ => (None, None),
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
            name: package_ref,
            registry: resolved_registry,
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
    source_desc: &str,
) -> anyhow::Result<wit_parser::PackageId> {
    component_world_package_id(resolve, world, source_desc)
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

#[derive(Debug, Clone)]
struct ResolvedOciPackageForClient {
    package_ref: PackageRef,
    namespace_prefix: Option<String>,
}

fn resolve_oci_package_for_client(package: &str) -> anyhow::Result<ResolvedOciPackageForClient> {
    let (namespace, name_path) = package
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid package name in oci source: {package}"))?;
    let mut repository_segments = Vec::with_capacity(2);
    repository_segments.push(namespace.trim());
    repository_segments.extend(name_path.split('/').map(str::trim));
    if repository_segments.len() < 2
        || repository_segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err(anyhow!("invalid package name in oci source: {package}"));
    }
    for segment in &repository_segments {
        let _: Label = segment
            .parse()
            .with_context(|| format!("invalid package name in oci source: {package}"))?;
    }

    let package_ref: PackageRef = format!(
        "{}:{}",
        repository_segments[repository_segments.len() - 2],
        repository_segments[repository_segments.len() - 1]
    )
    .parse()
    .with_context(|| format!("invalid package name in oci source: {package}"))?;

    let namespace_prefix = if repository_segments.len() > 2 {
        Some(format!(
            "{}/",
            repository_segments[..repository_segments.len() - 2].join("/")
        ))
    } else {
        None
    };

    Ok(ResolvedOciPackageForClient {
        package_ref,
        namespace_prefix,
    })
}

fn oci_registry_mapping(
    registry: &Registry,
    namespace_prefix: Option<&str>,
) -> anyhow::Result<RegistryMapping> {
    let Some(namespace_prefix) = namespace_prefix else {
        return Ok(RegistryMapping::Registry(registry.clone()));
    };
    let metadata: RegistryMetadata = serde_json::from_value(json!({
        "preferredProtocol": "oci",
        "oci": {
            "namespacePrefix": namespace_prefix,
        },
    }))
    .context("failed to build OCI registry metadata override")?;
    Ok(RegistryMapping::Custom(CustomConfig {
        registry: registry.clone(),
        metadata,
    }))
}

async fn fetch_release_bytes(
    protocol: RemoteSourceProtocol,
    package: &str,
    version: &str,
    registry: &str,
    namespace_registries: Option<&NamespaceRegistries>,
) -> anyhow::Result<Vec<u8>> {
    let source_scheme = protocol.scheme();
    let (package_ref, oci_namespace_prefix) = match protocol {
        RemoteSourceProtocol::Warg => {
            let package_ref: PackageRef = package.parse().with_context(|| {
                format!("invalid package name in {source_scheme} source: {package}")
            })?;
            (package_ref, None)
        }
        RemoteSourceProtocol::Oci => {
            let resolved = resolve_oci_package_for_client(package)?;
            (resolved.package_ref, resolved.namespace_prefix)
        }
    };
    let version: Version = version
        .parse()
        .with_context(|| format!("invalid package version in {source_scheme} source: {version}"))?;
    let registry: Registry = registry
        .parse()
        .with_context(|| format!("invalid registry value: {registry}"))?;

    let mut config = Config::empty();
    config.set_default_registry(Some(registry.clone()));
    let registry_mapping = if protocol == RemoteSourceProtocol::Oci {
        oci_registry_mapping(&registry, oci_namespace_prefix.as_deref())?
    } else {
        RegistryMapping::Registry(registry.clone())
    };
    config.set_package_registry_override(package_ref.clone(), registry_mapping);
    if protocol == RemoteSourceProtocol::Warg {
        configure_warg_namespace_registry_mappings(&mut config, namespace_registries)?;
    }
    configure_backend_for_registry(&mut config, protocol, &registry)?;

    let client = Client::new(config);
    let release = client
        .get_release(&package_ref, &version)
        .await
        .map_err(|err| {
            anyhow!("failed to get {source_scheme} release for {package_ref}@{version}: {err:#}")
        })?;
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

fn configure_warg_namespace_registry_mappings(
    config: &mut Config,
    namespace_registries: Option<&NamespaceRegistries>,
) -> anyhow::Result<()> {
    let Some(namespace_registries) = namespace_registries else {
        return Ok(());
    };

    for (namespace, registry) in namespace_registries {
        let label = namespace
            .parse()
            .with_context(|| format!("invalid namespace registry key: {namespace}"))?;
        let normalized_registry = normalize_registry_name(registry)
            .with_context(|| format!("invalid namespace registry for '{namespace}'"))?;
        let resolved_registry: Registry = normalized_registry
            .parse()
            .with_context(|| format!("invalid namespace registry value: {normalized_registry}"))?;
        config.set_namespace_registry(label, RegistryMapping::Registry(resolved_registry));
    }
    Ok(())
}

fn configure_backend_for_registry(
    config: &mut Config,
    protocol: RemoteSourceProtocol,
    registry: &Registry,
) -> anyhow::Result<()> {
    let registry_config = config.get_or_insert_registry_config_mut(registry);
    registry_config.set_default_backend(Some(protocol.default_backend().to_string()));
    if protocol == RemoteSourceProtocol::Oci
        && let Some(auth_config) = oci_backend_auth_config_from_env()
    {
        registry_config
            .set_backend_config("oci", &auth_config)
            .map_err(|err| anyhow!("failed to configure oci auth from environment: {err:#}"))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct OciBackendAuthConfig {
    auth: OciBasicAuth,
}

#[derive(Debug, Clone, Serialize)]
struct OciBasicAuth {
    username: String,
    password: String,
}

fn oci_backend_auth_config_from_env() -> Option<OciBackendAuthConfig> {
    oci_backend_auth_config(
        std::env::var("IMAGO_OCI_USERNAME").ok(),
        std::env::var("IMAGO_OCI_PASSWORD").ok(),
    )
}

fn oci_backend_auth_config(
    username: Option<String>,
    password: Option<String>,
) -> Option<OciBackendAuthConfig> {
    if username.is_none() && password.is_none() {
        return None;
    }
    Some(OciBackendAuthConfig {
        auth: OciBasicAuth {
            username: username.unwrap_or_default(),
            password: password.unwrap_or_default(),
        },
    })
}

async fn fetch_wit_bytes_with_local_fallback(
    project_root: &Path,
    protocol: RemoteSourceProtocol,
    package: &str,
    version: &str,
    registry: &str,
    namespace_registries: Option<&NamespaceRegistries>,
) -> anyhow::Result<Vec<u8>> {
    if let Some(local) =
        find_local_wit_candidate(project_root, protocol, package, version, registry)
    {
        return fs::read(&local).with_context(|| {
            format!(
                "failed to read local {} wit source {}",
                protocol.scheme(),
                local.display()
            )
        });
    }
    fetch_release_bytes(protocol, package, version, registry, namespace_registries).await
}

fn find_local_wit_candidate(
    project_root: &Path,
    protocol: RemoteSourceProtocol,
    package: &str,
    version: &str,
    registry: &str,
) -> Option<PathBuf> {
    match protocol {
        RemoteSourceProtocol::Warg => find_local_warg_wit_candidate(project_root, package, version),
        RemoteSourceProtocol::Oci => {
            find_local_oci_wit_candidate(project_root, package, version, registry)
        }
    }
}

fn find_local_component_candidate(
    project_root: &Path,
    protocol: RemoteSourceProtocol,
    package: &str,
    version: &str,
    registry: &str,
) -> Option<PathBuf> {
    match protocol {
        RemoteSourceProtocol::Warg => {
            find_local_warg_component_candidate(project_root, package, version)
        }
        RemoteSourceProtocol::Oci => {
            find_local_oci_component_candidate(project_root, package, version, registry)
        }
    }
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

fn find_local_oci_wit_candidate(
    project_root: &Path,
    package: &str,
    version: &str,
    registry: &str,
) -> Option<PathBuf> {
    let package_dir = oci_local_package_key(registry, package);
    let base = project_root
        .join(".imago")
        .join("oci")
        .join(package_dir.clone())
        .join(version);
    [
        base.join("wit.wasm"),
        base.join("wit"),
        base.join("wit.wit"),
        project_root
            .join(".imago")
            .join("oci")
            .join(format!("{package_dir}@{version}.wit")),
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

fn find_local_oci_component_candidate(
    project_root: &Path,
    package: &str,
    version: &str,
    registry: &str,
) -> Option<PathBuf> {
    let package_dir = oci_local_package_key(registry, package);
    let base = project_root
        .join(".imago")
        .join("oci")
        .join(package_dir.clone())
        .join(version);
    [
        base.join("wit.wasm"),
        base.join("wit"),
        base.join("component.wasm"),
        base.join("component").join("component.wasm"),
        project_root
            .join(".imago")
            .join("oci")
            .join(format!("{package_dir}@{version}.wasm")),
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
    use super::{
        DEFAULT_WARG_REGISTRY, DEFAULT_WASI_WARG_REGISTRY, copy_wit_tree, materialize_wit_source,
        normalize_registry_for_source, oci_backend_auth_config, parse_oci_spec, parse_warg_spec,
        resolve_oci_package_for_client, resolve_warg_registry_for_package,
        resolve_warg_registry_for_package_with_fallback, sanitize_wit_deps_name,
        warg_local_package_key,
    };
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::{
        collections::BTreeMap,
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
    fn parse_oci_spec_accepts_registry_namespace_name_and_version() {
        let parsed =
            parse_oci_spec("ghcr.io/chikoski/advent-of-spin@0.2.0").expect("must parse oci spec");
        assert_eq!(parsed.registry, "ghcr.io");
        assert_eq!(parsed.package, "chikoski:advent-of-spin");
        assert_eq!(parsed.version, "0.2.0");
        assert_eq!(parsed.source, "oci://ghcr.io/chikoski/advent-of-spin@0.2.0");
    }

    #[test]
    fn parse_oci_spec_accepts_nested_name_path() {
        let parsed =
            parse_oci_spec("ghcr.io/yieldspace/imago/nanokvm@1.2.3").expect("must parse oci spec");
        assert_eq!(parsed.registry, "ghcr.io");
        assert_eq!(parsed.package, "yieldspace:imago/nanokvm");
        assert_eq!(parsed.version, "1.2.3");
        assert_eq!(
            parsed.source,
            "oci://ghcr.io/yieldspace/imago/nanokvm@1.2.3"
        );
    }

    #[test]
    fn parse_oci_spec_rejects_invalid_nested_path_component() {
        let err = parse_oci_spec("ghcr.io/yieldspace/imago/../nanokvm@1.2.3")
            .expect_err("must reject traversal");
        assert!(
            err.to_string()
                .contains("oci source package must be '<namespace>/<name...>'")
        );
    }

    #[test]
    fn parse_oci_spec_rejects_empty_nested_path_component() {
        let err = parse_oci_spec("ghcr.io/yieldspace/imago//nanokvm@1.2.3")
            .expect_err("must reject empty segment");
        assert!(
            err.to_string()
                .contains("oci source package must be '<namespace>/<name...>'")
        );
    }

    #[test]
    fn parse_oci_spec_rejects_invalid_prefix_identifier_segment() {
        let err = parse_oci_spec("ghcr.io/Yieldspace/imago/nanokvm@1.2.3")
            .expect_err("must reject invalid identifier segment");
        assert!(
            err.to_string()
                .contains("invalid package name in oci source")
        );
    }

    #[test]
    fn resolve_oci_package_for_client_maps_nested_path_to_namespace_prefix() {
        let resolved = resolve_oci_package_for_client("yieldspace:imago/nanokvm")
            .expect("nested package should resolve for oci client");
        assert_eq!(resolved.package_ref.to_string(), "imago:nanokvm");
        assert_eq!(resolved.namespace_prefix.as_deref(), Some("yieldspace/"));
    }

    #[test]
    fn resolve_oci_package_for_client_maps_deep_nested_path_to_namespace_prefix() {
        let resolved = resolve_oci_package_for_client("yieldspace:imago/plugins/nanokvm")
            .expect("deep nested package should resolve for oci client");
        assert_eq!(resolved.package_ref.to_string(), "plugins:nanokvm");
        assert_eq!(
            resolved.namespace_prefix.as_deref(),
            Some("yieldspace/imago/")
        );
    }

    #[test]
    fn resolve_oci_package_for_client_rejects_invalid_prefix_identifier_segment() {
        let err = resolve_oci_package_for_client("Yieldspace:imago/nanokvm")
            .expect_err("invalid namespace segment must be rejected");
        assert!(
            err.to_string()
                .contains("invalid package name in oci source")
        );
    }

    #[test]
    fn resolve_warg_registry_defaults_non_wasi_namespace_to_wa_dev() {
        let registry = resolve_warg_registry_for_package("yieldspace:plugin/example", None, None)
            .expect("default registry should resolve");
        assert_eq!(registry, DEFAULT_WARG_REGISTRY);
    }

    #[test]
    fn resolve_warg_registry_defaults_wasi_namespace_to_wasi_dev() {
        let registry = resolve_warg_registry_for_package("wasi:cli", None, None)
            .expect("wasi default registry should resolve");
        assert_eq!(registry, DEFAULT_WASI_WARG_REGISTRY);
    }

    #[test]
    fn resolve_warg_registry_prefers_namespace_override_over_wasi_default() {
        let namespace_registries =
            BTreeMap::from([("wasi".to_string(), "example.registry".to_string())]);
        let registry =
            resolve_warg_registry_for_package("wasi:io", None, Some(&namespace_registries))
                .expect("namespace override should resolve");
        assert_eq!(registry, "example.registry");
    }

    #[test]
    fn resolve_warg_registry_for_transitive_falls_back_to_parent_registry_for_non_wasi() {
        let registry = resolve_warg_registry_for_package_with_fallback(
            "chikoski:name",
            None,
            None,
            Some("custom-root.example"),
        )
        .expect("transitive fallback registry should resolve");
        assert_eq!(registry, "custom-root.example");
    }

    #[test]
    fn resolve_warg_registry_for_transitive_prefers_namespace_override_over_parent_registry() {
        let namespace_registries =
            BTreeMap::from([("chikoski".to_string(), "override.example".to_string())]);
        let registry = resolve_warg_registry_for_package_with_fallback(
            "chikoski:name",
            None,
            Some(&namespace_registries),
            Some("custom-root.example"),
        )
        .expect("namespace override should resolve");
        assert_eq!(registry, "override.example");
    }

    #[test]
    fn normalize_registry_for_source_rejects_registry_override_for_oci() {
        let err = normalize_registry_for_source(
            "oci://ghcr.io/chikoski/advent-of-spin@0.2.0",
            Some("wa.dev"),
            None,
            "dependencies[0].wit",
        )
        .expect_err("oci source must reject explicit registry");
        assert!(
            err.to_string()
                .contains(".registry is not allowed when source is oci://"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn oci_backend_auth_config_is_none_without_injected_credentials() {
        assert!(oci_backend_auth_config(None, None).is_none());
    }

    #[test]
    fn oci_backend_auth_config_uses_injected_credentials() {
        let auth = oci_backend_auth_config(Some("user-a".to_string()), Some("pass-b".to_string()))
            .expect("auth config should be created");
        assert_eq!(auth.auth.username, "user-a");
        assert_eq!(auth.auth.password, "pass-b");
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

    #[tokio::test]
    async fn materialize_file_directory_includes_transitive_wit_packages() {
        let root = new_temp_dir("materialize-file-dir-transitive");
        let source = root.join("source");
        let destination = root.join("dest/wit/deps/imago-experimental-gpio");
        write(
            &source.join("package.wit"),
            br#"
package imago:experimental-gpio@0.1.0;

interface digital {
    use wasi:io/poll@0.2.6.{pollable};

    resource digital-out-pin {
        watch-for-ready: func() -> pollable;
    }
}

world host {
    import digital;
}
"#,
        );
        write(
            &source.join("deps/wasi-io-0.2.6/package.wit"),
            br#"
package wasi:io@0.2.6;

interface poll {
    resource pollable {
        block: func();
    }
}
"#,
        );

        let source_ref = format!("file://{}", source.display());
        let materialized = materialize_wit_source(
            &root,
            &source_ref,
            None,
            None,
            Some("imago:experimental-gpio"),
            &destination,
        )
        .await
        .expect("materialize should succeed");

        assert!(
            destination.join("package.wit").is_file(),
            "top-level package.wit must be written"
        );
        assert!(
            root.join("dest/wit/deps/wasi-io/package.wit").is_file(),
            "transitive package must be materialized at wit/deps root"
        );
        assert_eq!(materialized.transitive_packages.len(), 1);
        assert_eq!(materialized.transitive_packages[0].name, "wasi:io");
        assert_eq!(materialized.transitive_packages[0].path, "wit/deps/wasi-io");

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn materialize_file_directory_rejects_symlink_root_source() {
        let root = new_temp_dir("materialize-file-dir-symlink-root");
        let source_real = root.join("source-real");
        let source_link = root.join("source-link");
        let destination = root.join("dest/wit/deps/imago-experimental-gpio");

        write(
            &source_real.join("package.wit"),
            br#"
package imago:experimental-gpio@0.1.0;

interface digital {
    resource digital-out-pin {
        watch-for-ready: func();
    }
}

world host {
    import digital;
}
"#,
        );
        symlink(&source_real, &source_link).expect("symlink should be created");

        let source_ref = format!("file://{}", source_link.display());
        let err = materialize_wit_source(
            &root,
            &source_ref,
            None,
            None,
            Some("imago:experimental-gpio"),
            &destination,
        )
        .await
        .expect_err("symlink root source must fail");

        assert!(
            err.to_string()
                .contains("wit source must not contain symlinks"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
