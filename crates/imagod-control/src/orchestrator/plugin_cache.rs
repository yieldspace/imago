use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use imagod_common::ImagodError;
use imagod_ipc::{PluginDependency, PluginKind};
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncReadExt};
use wasmparser::{ComponentExternalKind, ComponentTypeRef, Parser, Payload};

use super::{Manifest, manifest::ManifestValidator};

#[derive(Debug, Clone)]
pub(super) struct FilesystemPluginCache {
    storage_root: PathBuf,
}

impl FilesystemPluginCache {
    pub(super) fn new(storage_root: PathBuf) -> Self {
        Self { storage_root }
    }

    pub(super) async fn prepare_plugin_dependencies(
        &self,
        release_dir: &Path,
        dependencies: &[PluginDependency],
        manifest_validator: &impl ManifestValidator,
    ) -> Result<Vec<PluginDependency>, ImagodError> {
        prepare_plugin_dependencies_for_root(
            &self.storage_root,
            release_dir,
            dependencies,
            manifest_validator,
        )
        .await
    }

    pub(super) async fn gc_unused_plugin_components_on_boot(
        &self,
        manifest_validator: &impl ManifestValidator,
    ) -> Result<(), ImagodError> {
        gc_unused_plugin_components_on_boot_for_root(&self.storage_root, manifest_validator).await
    }
}

pub(super) async fn prepare_plugin_dependencies_for_root(
    storage_root: &Path,
    release_dir: &Path,
    dependencies: &[PluginDependency],
    manifest_validator: &impl ManifestValidator,
) -> Result<Vec<PluginDependency>, ImagodError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let mut known_names = BTreeSet::new();
    for dep in dependencies {
        manifest_validator.validate_plugin_package_name(&dep.name)?;
        if dep.version.trim().is_empty() {
            return Err(super::map_bad_manifest(format!(
                "manifest.dependencies[{}].version must not be empty",
                dep.name
            )));
        }
        if dep.wit.trim().is_empty() {
            return Err(super::map_bad_manifest(format!(
                "manifest.dependencies[{}].wit must not be empty",
                dep.name
            )));
        }
        if !known_names.insert(dep.name.clone()) {
            return Err(super::map_bad_manifest(format!(
                "manifest.dependencies contains duplicate dependency '{}'",
                dep.name
            )));
        }
    }

    let mut normalized = Vec::with_capacity(dependencies.len());
    let components_root = plugin_component_cache_root(storage_root);
    fs::create_dir_all(&components_root).await.map_err(|e| {
        super::map_internal(format!(
            "failed to create plugin component cache dir {}: {e}",
            components_root.display()
        ))
    })?;

    for dep in dependencies {
        for required in &dep.requires {
            manifest_validator.validate_plugin_package_name(required)?;
            if !known_names.contains(required) {
                return Err(super::map_bad_manifest(format!(
                    "manifest.dependencies[{}].requires references unknown dependency '{}'",
                    dep.name, required
                )));
            }
        }

        let mut dep = dep.clone();
        dep.capabilities = manifest_validator.normalize_capability_policy(&dep.capabilities);
        dep.requires = manifest_validator.normalize_string_set(&dep.requires);

        match dep.kind {
            PluginKind::Native => {
                if dep.component.is_some() {
                    return Err(super::map_bad_manifest(format!(
                        "manifest.dependencies[{}].component is only allowed when kind=\"wasm\"",
                        dep.name
                    )));
                }
            }
            PluginKind::Wasm => {
                let component = dep.component.clone().ok_or_else(|| {
                    super::map_bad_manifest(format!(
                        "manifest.dependencies[{}].component is required when kind=\"wasm\"",
                        dep.name
                    ))
                })?;
                manifest_validator.validate_sha256_hex(
                    &component.sha256,
                    &format!("manifest.dependencies[{}].component.sha256", dep.name),
                )?;

                let component_path_str = component.path.to_str().ok_or_else(|| {
                    super::map_bad_manifest(format!(
                        "manifest.dependencies[{}].component.path must be valid UTF-8",
                        dep.name
                    ))
                })?;
                let normalized_component_path = manifest_validator.normalize_relative_path(
                    component_path_str,
                    &format!("manifest.dependencies[{}].component.path", dep.name),
                )?;
                let release_component_path = release_dir.join(&normalized_component_path);
                let metadata = fs::metadata(&release_component_path).await.map_err(|e| {
                    super::map_bad_manifest(format!(
                        "plugin component is not accessible: {} ({e})",
                        release_component_path.display()
                    ))
                })?;
                if !metadata.is_file() {
                    return Err(super::map_bad_manifest(format!(
                        "plugin component path is not a file: {}",
                        release_component_path.display()
                    )));
                }

                let digest = compute_sha256_hex_async(&release_component_path).await?;
                if !digest.eq_ignore_ascii_case(&component.sha256) {
                    return Err(super::map_bad_manifest(format!(
                        "plugin component sha256 mismatch for '{}': expected {}, actual {}",
                        dep.name, component.sha256, digest
                    )));
                }
                let component_bytes = fs::read(&release_component_path).await.map_err(|e| {
                    super::map_bad_manifest(format!(
                        "failed to read plugin component bytes {}: {e}",
                        release_component_path.display()
                    ))
                })?;
                let (imports, exports) = collect_component_instance_interface_metadata(
                    &component_bytes,
                )
                .map_err(|err| {
                    super::map_bad_manifest(format!(
                        "failed to extract plugin component metadata for '{}': {}",
                        dep.name, err.message
                    ))
                })?;

                let cache_path = components_root.join(format!("{}.wasm", component.sha256));
                let cache_digest_matches = match fs::metadata(&cache_path).await {
                    Ok(existing_meta) => {
                        if !existing_meta.is_file() {
                            return Err(super::map_internal(format!(
                                "plugin component cache path is not a file: {}",
                                cache_path.display()
                            )));
                        }
                        let existing = compute_sha256_hex_async(&cache_path).await?;
                        existing.eq_ignore_ascii_case(&component.sha256)
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                    Err(err) => {
                        return Err(super::map_internal(format!(
                            "failed to inspect plugin component cache {}: {err}",
                            cache_path.display()
                        )));
                    }
                };
                if !cache_digest_matches {
                    fs::copy(&release_component_path, &cache_path)
                        .await
                        .map_err(|e| {
                            super::map_internal(format!(
                                "failed to copy plugin component to cache {}: {e}",
                                cache_path.display()
                            ))
                        })?;
                }

                dep.component = Some(imagod_ipc::PluginComponent {
                    path: cache_path,
                    sha256: component.sha256,
                    imports: Some(imports),
                    exports: Some(exports),
                });
            }
        }

        normalized.push(dep);
    }

    Ok(normalized)
}

fn collect_component_instance_interface_metadata(
    component_bytes: &[u8],
) -> Result<(Vec<String>, Vec<String>), ImagodError> {
    let mut imports = BTreeSet::new();
    let mut exports = BTreeSet::new();
    let mut nested_depth = 0usize;

    for payload in Parser::new(0).parse_all(component_bytes) {
        let payload = payload.map_err(|err| {
            super::map_bad_manifest(format!("component metadata parse failed: {err}"))
        })?;
        match payload {
            Payload::ComponentImportSection(section) if nested_depth == 0 => {
                for item in section {
                    let import = item.map_err(|err| {
                        super::map_bad_manifest(format!(
                            "component import metadata decode failed: {err}"
                        ))
                    })?;
                    if matches!(import.ty, ComponentTypeRef::Instance(_)) {
                        imports.insert(import.name.0.to_string());
                    }
                }
            }
            Payload::ComponentExportSection(section) if nested_depth == 0 => {
                for item in section {
                    let export = item.map_err(|err| {
                        super::map_bad_manifest(format!(
                            "component export metadata decode failed: {err}"
                        ))
                    })?;
                    if export.kind == ComponentExternalKind::Instance {
                        exports.insert(export.name.0.to_string());
                    }
                }
            }
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => {
                nested_depth = nested_depth.saturating_add(1);
            }
            Payload::End(_) => {
                if nested_depth == 0 {
                    break;
                }
                nested_depth = nested_depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    Ok((imports.into_iter().collect(), exports.into_iter().collect()))
}

pub(super) fn plugin_component_cache_root(storage_root: &Path) -> PathBuf {
    storage_root.join("plugins").join("components")
}

pub(super) async fn compute_sha256_hex_async(path: &Path) -> Result<String, ImagodError> {
    let mut file = fs::File::open(path).await.map_err(|e| {
        super::map_internal(format!(
            "failed to open file for sha256 {}: {e}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).await.map_err(|e| {
            super::map_internal(format!(
                "failed to read file for sha256 {}: {e}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(super) async fn gc_unused_plugin_components_on_boot_for_root(
    storage_root: &Path,
    manifest_validator: &impl ManifestValidator,
) -> Result<(), ImagodError> {
    let components_root = plugin_component_cache_root(storage_root);
    let referenced =
        collect_referenced_plugin_component_hashes_for_root(storage_root, manifest_validator)
            .await?;

    let mut entries = match fs::read_dir(&components_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(super::map_internal(format!(
                "failed to read plugin components dir {}: {err}",
                components_root.display()
            )));
        }
    };

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| super::map_internal(format!("failed to iterate plugin components dir: {e}")))?
    {
        let path = entry.path();
        let file_type = match entry.file_type().await {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped unreadable entry {}: {}",
                    path.display(),
                    err
                );
                continue;
            }
        };
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if referenced.contains(stem) {
            continue;
        }

        if let Err(err) = fs::remove_file(&path).await {
            eprintln!(
                "plugin component gc failed to remove {}: {}",
                path.display(),
                err
            );
        }
    }

    Ok(())
}

pub(super) async fn collect_referenced_plugin_component_hashes_for_root(
    storage_root: &Path,
    manifest_validator: &impl ManifestValidator,
) -> Result<BTreeSet<String>, ImagodError> {
    let services_root = storage_root.join("services");
    let mut entries = match fs::read_dir(&services_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(err) => {
            return Err(super::map_internal(format!(
                "failed to read services root for plugin gc {}: {err}",
                services_root.display()
            )));
        }
    };

    let mut referenced = BTreeSet::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| super::map_internal(format!("failed to iterate services root: {e}")))?
    {
        let service_root = entry.path();
        let file_type = match entry.file_type().await {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped unreadable service entry {}: {}",
                    service_root.display(),
                    err
                );
                continue;
            }
        };
        if !file_type.is_dir() {
            continue;
        }

        let active = match super::read_active_release(&service_root.join("active_release")).await {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped service {} due to active_release error: {}",
                    service_root.display(),
                    err.message
                );
                continue;
            }
        };
        if active.is_empty() {
            continue;
        }

        let manifest_path = service_root.join(active).join("manifest.json");
        let manifest_bytes = match fs::read(&manifest_path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped missing manifest {}: {}",
                    manifest_path.display(),
                    err
                );
                continue;
            }
        };
        let manifest: Manifest = match manifest_validator.parse_manifest(&manifest_bytes) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped unparsable manifest {}: {}",
                    manifest_path.display(),
                    err.message
                );
                continue;
            }
        };
        for dependency in manifest.dependencies {
            if dependency.kind == PluginKind::Wasm
                && let Some(component) = dependency.component
            {
                referenced.insert(component.sha256);
            }
        }
    }

    Ok(referenced)
}
