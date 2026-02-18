use std::{collections::BTreeMap, path::Path};

use imagod_common::ImagodError;

use super::{
    Manifest, ServiceLaunch,
    manifest::ManifestValidator,
    plugin_cache::FilesystemPluginCache,
};

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

    let mut envs: BTreeMap<String, String> = manifest.vars.clone();
    for (k, v) in &manifest.secrets {
        envs.insert(k.clone(), v.clone());
    }

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
        args: Vec::new(),
        envs,
        bindings,
        plugin_dependencies,
        capabilities: manifest_validator.normalize_capability_policy(&manifest.capabilities),
    })
}
