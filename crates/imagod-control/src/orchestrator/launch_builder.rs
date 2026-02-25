use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use imagod_common::ImagodError;
use imagod_ipc::{RunnerWasiMount, WasiHttpOutboundRule};

use super::{
    Manifest, ServiceLaunch, manifest::ManifestValidator, plugin_cache::FilesystemPluginCache,
};

const DEFAULT_WASI_HTTP_OUTBOUND: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

pub(super) async fn build_launch_from_release(
    release_hash: &str,
    release_dir: &Path,
    manifest: &Manifest,
    manifest_validator: &impl ManifestValidator,
    plugin_cache: &FilesystemPluginCache,
) -> Result<ServiceLaunch, ImagodError> {
    let normalized_main = manifest_validator.normalize_main_path(&manifest.main)?;
    let component_path = release_dir.join(&normalized_main);
    if !component_path.starts_with(release_dir) {
        return Err(super::map_bad_manifest(format!(
            "manifest main path resolved outside release dir: {}",
            manifest.main
        )));
    }
    if let Err(err) = tokio::fs::metadata(&component_path).await {
        return Err(super::map_bad_manifest(format!(
            "component path is not accessible: {} ({err})",
            component_path.display(),
        )));
    }

    let ResolvedWasiConfig {
        args,
        env,
        mounts,
        http_outbound,
    } = resolve_wasi_config(release_dir, manifest, manifest_validator).await?;

    let bindings = manifest_validator.validate_bindings(&manifest.bindings)?;
    let (http_port, http_max_body_bytes) = manifest_validator.validate_http(manifest)?;
    let socket = manifest_validator.validate_socket(manifest)?;
    let plugin_dependencies = plugin_cache
        .prepare_plugin_dependencies(release_dir, &manifest.dependencies, manifest_validator)
        .await?;

    Ok(ServiceLaunch {
        name: manifest.name.clone(),
        release_hash: release_hash.to_string(),
        app_type: manifest.app_type,
        http_port,
        http_max_body_bytes,
        socket,
        component_path,
        args,
        envs: env,
        wasi_mounts: mounts,
        wasi_http_outbound: http_outbound,
        bindings,
        plugin_dependencies,
        capabilities: manifest_validator.normalize_capability_policy(&manifest.capabilities),
    })
}

#[derive(Default)]
struct ResolvedWasiConfig {
    args: Vec<String>,
    env: BTreeMap<String, String>,
    mounts: Vec<RunnerWasiMount>,
    http_outbound: Vec<WasiHttpOutboundRule>,
}

async fn resolve_wasi_config(
    release_dir: &Path,
    manifest: &Manifest,
    manifest_validator: &impl ManifestValidator,
) -> Result<ResolvedWasiConfig, ImagodError> {
    let Some(wasi) = manifest.wasi.as_ref() else {
        return Ok(ResolvedWasiConfig {
            args: Vec::new(),
            env: BTreeMap::new(),
            mounts: Vec::new(),
            http_outbound: resolve_wasi_http_outbound_rules(&[], "manifest.wasi.http_outbound")?,
        });
    };

    let mut args = Vec::with_capacity(wasi.args.len());
    for (index, arg) in wasi.args.iter().enumerate() {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            return Err(super::map_bad_manifest(format!(
                "manifest.wasi.args[{index}] must not be empty"
            )));
        }
        args.push(trimmed.to_string());
    }

    let mut env = BTreeMap::new();
    for (key, value) in &wasi.env {
        if key.trim().is_empty() {
            return Err(super::map_bad_manifest(
                "manifest.wasi.env contains empty key".to_string(),
            ));
        }
        env.insert(key.clone(), value.clone());
    }

    let allowed_asset_dirs = collect_allowed_asset_dirs(manifest, manifest_validator)?;
    let mounts =
        resolve_wasi_mounts(release_dir, wasi, &allowed_asset_dirs, manifest_validator).await?;
    let http_outbound =
        resolve_wasi_http_outbound_rules(&wasi.http_outbound, "manifest.wasi.http_outbound")?;

    Ok(ResolvedWasiConfig {
        args,
        env,
        mounts,
        http_outbound,
    })
}

fn resolve_wasi_http_outbound_rules(
    values: &[String],
    field_name: &str,
) -> Result<Vec<WasiHttpOutboundRule>, ImagodError> {
    let mut rules = Vec::new();

    for default_value in DEFAULT_WASI_HTTP_OUTBOUND {
        let rule = WasiHttpOutboundRule::parse(default_value).map_err(|err| {
            super::map_bad_manifest(format!(
                "failed to build default {field_name} rule '{default_value}': {err}"
            ))
        })?;
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    for (index, raw) in values.iter().enumerate() {
        let rule = WasiHttpOutboundRule::parse(raw).map_err(|err| {
            super::map_bad_manifest(format!("{field_name}[{index}] is invalid: {err}"))
        })?;
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    Ok(rules)
}

fn collect_allowed_asset_dirs(
    manifest: &Manifest,
    manifest_validator: &impl ManifestValidator,
) -> Result<BTreeSet<PathBuf>, ImagodError> {
    let mut dirs = BTreeSet::new();
    for (index, asset) in manifest.assets.iter().enumerate() {
        let normalized = manifest_validator
            .normalize_relative_path(&asset.path, &format!("manifest.assets[{index}].path"))?;
        if let Some(parent) = normalized.parent()
            && !parent.as_os_str().is_empty()
        {
            dirs.insert(parent.to_path_buf());
        }
    }
    Ok(dirs)
}

async fn resolve_wasi_mounts(
    release_dir: &Path,
    wasi: &super::ManifestWasiConfig,
    allowed_asset_dirs: &BTreeSet<PathBuf>,
    manifest_validator: &impl ManifestValidator,
) -> Result<Vec<RunnerWasiMount>, ImagodError> {
    let mut mounts = Vec::with_capacity(wasi.mounts.len() + wasi.read_only_mounts.len());
    let mut seen_guest_paths = BTreeSet::new();
    let mut seen_asset_dirs = BTreeSet::new();

    resolve_wasi_mount_entries(
        release_dir,
        "manifest.wasi.mounts",
        &wasi.mounts,
        false,
        allowed_asset_dirs,
        manifest_validator,
        &mut seen_guest_paths,
        &mut seen_asset_dirs,
        &mut mounts,
    )
    .await?;
    resolve_wasi_mount_entries(
        release_dir,
        "manifest.wasi.read_only_mounts",
        &wasi.read_only_mounts,
        true,
        allowed_asset_dirs,
        manifest_validator,
        &mut seen_guest_paths,
        &mut seen_asset_dirs,
        &mut mounts,
    )
    .await?;

    Ok(mounts)
}

#[allow(clippy::too_many_arguments)]
async fn resolve_wasi_mount_entries(
    release_dir: &Path,
    field_name: &str,
    entries: &[super::ManifestWasiMount],
    read_only: bool,
    allowed_asset_dirs: &BTreeSet<PathBuf>,
    manifest_validator: &impl ManifestValidator,
    seen_guest_paths: &mut BTreeSet<String>,
    seen_asset_dirs: &mut BTreeSet<PathBuf>,
    resolved: &mut Vec<RunnerWasiMount>,
) -> Result<(), ImagodError> {
    for (index, entry) in entries.iter().enumerate() {
        let asset_dir = manifest_validator.normalize_relative_path(
            &entry.asset_dir,
            &format!("{field_name}[{index}].asset_dir"),
        )?;
        if !allowed_asset_dirs.contains(&asset_dir) {
            return Err(super::map_bad_manifest(format!(
                "{field_name}[{index}].asset_dir must match a directory derived from assets[].path"
            )));
        }
        if !seen_asset_dirs.insert(asset_dir.clone()) {
            return Err(super::map_bad_manifest(format!(
                "{field_name}[{index}].asset_dir duplicates another wasi mount entry: {}",
                asset_dir.display()
            )));
        }

        let guest_path = normalize_wasi_guest_path(
            &entry.guest_path,
            &format!("{field_name}[{index}].guest_path"),
        )?;
        if !seen_guest_paths.insert(guest_path.clone()) {
            return Err(super::map_bad_manifest(format!(
                "{field_name}[{index}].guest_path duplicates another wasi mount entry: {guest_path}"
            )));
        }

        let host_path = release_dir.join(&asset_dir);
        if !host_path.starts_with(release_dir) {
            return Err(super::map_bad_manifest(format!(
                "{field_name}[{index}].asset_dir resolved outside release dir: {}",
                entry.asset_dir
            )));
        }
        let metadata = tokio::fs::metadata(&host_path).await.map_err(|err| {
            super::map_bad_manifest(format!(
                "{field_name}[{index}].asset_dir is not accessible: {} ({err})",
                host_path.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(super::map_bad_manifest(format!(
                "{field_name}[{index}].asset_dir must resolve to a directory: {}",
                host_path.display()
            )));
        }

        resolved.push(RunnerWasiMount {
            host_path,
            guest_path,
            read_only,
        });
    }
    Ok(())
}

fn normalize_wasi_guest_path(raw: &str, field_name: &str) -> Result<String, ImagodError> {
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty() {
        return Err(super::map_bad_manifest(format!(
            "{field_name} must not be empty"
        )));
    }
    if raw.contains('\\') {
        return Err(super::map_bad_manifest(format!(
            "{field_name} must not contain backslashes: {}",
            raw.trim()
        )));
    }
    if !path.is_absolute() {
        return Err(super::map_bad_manifest(format!(
            "{field_name} must be an absolute path: {}",
            raw.trim()
        )));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(super::map_bad_manifest(format!(
            "{field_name} must not be windows-prefixed: {}",
            raw.trim()
        )));
    }

    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => {
                segments.push(segment.to_string_lossy().to_string());
            }
            Component::ParentDir | Component::CurDir => {
                return Err(super::map_bad_manifest(format!(
                    "{field_name} must not contain path traversal: {}",
                    raw.trim()
                )));
            }
            _ => {
                return Err(super::map_bad_manifest(format!(
                    "{field_name} is invalid: {}",
                    raw.trim()
                )));
            }
        }
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}
