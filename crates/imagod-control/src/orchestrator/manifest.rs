use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::{Component, Path, PathBuf},
};

use imagod_common::ImagodError;
use imagod_spec::{CapabilityPolicy, RunnerAppType, RunnerSocketConfig, ServiceBinding};
use sha2::{Digest, Sha256};

use super::{
    DEFAULT_HTTP_MAX_BODY_BYTES, HashTarget, MAX_HTTP_MAX_BODY_BYTES, Manifest, ManifestBinding,
};

pub(super) trait ManifestValidator: Send + Sync {
    fn parse_manifest(&self, manifest_bytes: &[u8]) -> Result<Manifest, ImagodError>;

    fn validate_manifest_metadata(
        &self,
        manifest: &Manifest,
        manifest_bytes: &[u8],
        expected_manifest_digest: Option<&str>,
    ) -> Result<(), ImagodError>;

    fn validate_release_service_name(
        &self,
        manifest: &Manifest,
        expected_service_name: &str,
    ) -> Result<(), ImagodError>;

    fn validate_http(&self, manifest: &Manifest)
    -> Result<(Option<u16>, Option<u64>), ImagodError>;

    fn validate_socket(
        &self,
        manifest: &Manifest,
    ) -> Result<Option<RunnerSocketConfig>, ImagodError>;

    fn validate_bindings(
        &self,
        bindings: &[ManifestBinding],
    ) -> Result<Vec<ServiceBinding>, ImagodError>;

    fn normalize_archive_entry_path(&self, path: &Path) -> Result<PathBuf, ImagodError>;

    fn normalize_main_path(&self, main: &str) -> Result<PathBuf, ImagodError>;

    fn normalize_relative_path(&self, raw: &str, field_name: &str) -> Result<PathBuf, ImagodError>;

    fn validate_service_name(&self, name: &str) -> Result<(), ImagodError>;

    fn normalize_capability_policy(&self, policy: &CapabilityPolicy) -> CapabilityPolicy;

    fn normalize_string_set(&self, values: &[String]) -> Vec<String>;

    fn validate_plugin_package_name(&self, name: &str) -> Result<(), ImagodError>;

    fn validate_sha256_hex(&self, value: &str, field_name: &str) -> Result<(), ImagodError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DefaultManifestValidator;

impl ManifestValidator for DefaultManifestValidator {
    fn parse_manifest(&self, manifest_bytes: &[u8]) -> Result<Manifest, ImagodError> {
        serde_json::from_slice(manifest_bytes)
            .map_err(|e| super::map_bad_manifest(format!("manifest parse failed: {e}")))
    }

    fn validate_manifest_metadata(
        &self,
        manifest: &Manifest,
        manifest_bytes: &[u8],
        expected_manifest_digest: Option<&str>,
    ) -> Result<(), ImagodError> {
        if manifest.hash.algorithm != "sha256" || !manifest.hash.validate_targets() {
            return Err(super::map_bad_manifest(
                "manifest hash metadata is invalid".to_string(),
            ));
        }
        self.validate_service_name(&manifest.name)?;

        if let Some(expected_manifest_digest) = expected_manifest_digest {
            let manifest_digest = hex::encode(Sha256::digest(manifest_bytes));
            if manifest_digest != expected_manifest_digest {
                return Err(super::map_bad_manifest(
                    "manifest digest does not match artifact metadata".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn validate_release_service_name(
        &self,
        manifest: &Manifest,
        expected_service_name: &str,
    ) -> Result<(), ImagodError> {
        if manifest.name != expected_service_name {
            return Err(super::map_bad_manifest(format!(
                "release manifest name mismatch: expected {}, got {}",
                expected_service_name, manifest.name
            )));
        }
        Ok(())
    }

    fn validate_http(
        &self,
        manifest: &Manifest,
    ) -> Result<(Option<u16>, Option<u64>), ImagodError> {
        match manifest.app_type {
            RunnerAppType::Http => {
                let http = manifest.http.as_ref().ok_or_else(|| {
                    super::map_bad_manifest(
                        "manifest.http is required when type=\"http\"".to_string(),
                    )
                })?;
                if http.port == 0 {
                    return Err(super::map_bad_manifest(
                        "manifest.http.port must be in range 1..=65535".to_string(),
                    ));
                }
                if http.max_body_bytes == 0 || http.max_body_bytes > MAX_HTTP_MAX_BODY_BYTES {
                    return Err(super::map_bad_manifest(format!(
                        "manifest.http.max_body_bytes must be in range 1..={} (got {})",
                        MAX_HTTP_MAX_BODY_BYTES, http.max_body_bytes
                    )));
                }
                Ok((Some(http.port), Some(http.max_body_bytes)))
            }
            RunnerAppType::Cli | RunnerAppType::Rpc | RunnerAppType::Socket => {
                if manifest.http.is_some() {
                    return Err(super::map_bad_manifest(
                        "manifest.http is only allowed when type=\"http\"".to_string(),
                    ));
                }
                Ok((None, None))
            }
        }
    }

    fn validate_socket(
        &self,
        manifest: &Manifest,
    ) -> Result<Option<RunnerSocketConfig>, ImagodError> {
        match manifest.app_type {
            RunnerAppType::Socket => {
                let socket = manifest.socket.clone().ok_or_else(|| {
                    super::map_bad_manifest(
                        "manifest.socket is required when type=\"socket\"".to_string(),
                    )
                })?;
                if socket.listen_port == 0 {
                    return Err(super::map_bad_manifest(
                        "manifest.socket.listen_port must be in range 1..=65535".to_string(),
                    ));
                }
                socket.listen_addr.parse::<IpAddr>().map_err(|err| {
                    super::map_bad_manifest(format!(
                        "manifest.socket.listen_addr must be a valid IP address (got '{}'): {err}",
                        socket.listen_addr
                    ))
                })?;
                Ok(Some(socket))
            }
            RunnerAppType::Cli | RunnerAppType::Rpc | RunnerAppType::Http => {
                if manifest.socket.is_some() {
                    return Err(super::map_bad_manifest(
                        "manifest.socket is only allowed when type=\"socket\"".to_string(),
                    ));
                }
                Ok(None)
            }
        }
    }

    fn validate_bindings(
        &self,
        bindings: &[ManifestBinding],
    ) -> Result<Vec<ServiceBinding>, ImagodError> {
        let mut normalized = Vec::with_capacity(bindings.len());
        for (idx, binding) in bindings.iter().enumerate() {
            if binding.name.is_empty() {
                return Err(super::map_bad_manifest(format!(
                    "manifest.bindings[{idx}].name must not be empty"
                )));
            }
            if binding.wit.is_empty() {
                return Err(super::map_bad_manifest(format!(
                    "manifest.bindings[{idx}].wit must not be empty"
                )));
            }
            if let Err(err) = self.validate_service_name(&binding.name) {
                return Err(super::map_bad_manifest(format!(
                    "manifest.bindings[{idx}].name is invalid '{}': {}",
                    binding.name, err.message
                )));
            }
            normalized.push(ServiceBinding {
                name: binding.name.clone(),
                wit: binding.wit.clone(),
            });
        }
        Ok(normalized)
    }

    fn normalize_archive_entry_path(&self, path: &Path) -> Result<PathBuf, ImagodError> {
        if path.as_os_str().is_empty() {
            return Err(super::map_bad_manifest(
                "artifact contains empty entry path".to_string(),
            ));
        }
        if path.is_absolute() {
            return Err(super::map_bad_manifest(format!(
                "artifact contains absolute entry path: {}",
                path.display()
            )));
        }

        let raw = path.as_os_str().to_string_lossy();
        if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
            return Err(super::map_bad_manifest(format!(
                "artifact contains windows-prefixed entry path: {raw}"
            )));
        }

        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(segment) => normalized.push(segment),
                Component::ParentDir | Component::RootDir => {
                    return Err(super::map_bad_manifest(format!(
                        "artifact contains invalid entry path: {}",
                        path.display()
                    )));
                }
                _ => {
                    return Err(super::map_bad_manifest(format!(
                        "artifact contains invalid entry path: {}",
                        path.display()
                    )));
                }
            }
        }

        if normalized.as_os_str().is_empty() {
            return Err(super::map_bad_manifest(format!(
                "artifact contains invalid entry path: {}",
                path.display()
            )));
        }

        Ok(normalized)
    }

    fn normalize_main_path(&self, main: &str) -> Result<PathBuf, ImagodError> {
        self.normalize_relative_path(main, "manifest.main")
    }

    fn normalize_relative_path(&self, raw: &str, field_name: &str) -> Result<PathBuf, ImagodError> {
        let path = Path::new(raw);
        if raw.is_empty() || path.as_os_str().is_empty() {
            return Err(super::map_bad_manifest(format!(
                "{field_name} must not be empty"
            )));
        }
        if path.is_absolute() {
            return Err(super::map_bad_manifest(format!(
                "{field_name} must be a relative path: {raw}"
            )));
        }

        let raw_os = path.as_os_str().to_string_lossy();
        if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
            return Err(super::map_bad_manifest(format!(
                "{field_name} must not be windows-prefixed: {raw}"
            )));
        }

        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(segment) => normalized.push(segment),
                Component::ParentDir | Component::RootDir => {
                    return Err(super::map_bad_manifest(format!(
                        "{field_name} contains invalid path traversal: {raw}"
                    )));
                }
                _ => {
                    return Err(super::map_bad_manifest(format!(
                        "{field_name} contains invalid path component: {raw}"
                    )));
                }
            }
        }

        if normalized.as_os_str().is_empty() {
            return Err(super::map_bad_manifest(format!(
                "{field_name} is invalid: {raw}"
            )));
        }

        Ok(normalized)
    }

    fn validate_service_name(&self, name: &str) -> Result<(), ImagodError> {
        if name.is_empty() {
            return Err(super::map_bad_manifest(
                "manifest.name must not be empty".to_string(),
            ));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(super::map_bad_manifest(format!(
                "manifest.name contains invalid path characters: {name}"
            )));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(super::map_bad_manifest(format!(
                "manifest.name contains unsupported characters: {name}"
            )));
        }
        Ok(())
    }

    fn normalize_capability_policy(&self, policy: &CapabilityPolicy) -> CapabilityPolicy {
        CapabilityPolicy {
            privileged: policy.privileged,
            deps: normalize_capability_rule_map(policy, &policy.deps, self),
            wasi: normalize_capability_rule_map(policy, &policy.wasi, self),
        }
    }

    fn normalize_string_set(&self, values: &[String]) -> Vec<String> {
        let mut set = BTreeSet::new();
        for value in values {
            let value = value.trim();
            if !value.is_empty() {
                set.insert(value.to_string());
            }
        }
        set.into_iter().collect()
    }

    fn validate_plugin_package_name(&self, name: &str) -> Result<(), ImagodError> {
        if name.is_empty() {
            return Err(super::map_bad_manifest(
                "manifest.dependencies[].name must not be empty".to_string(),
            ));
        }
        if name.contains('\\') || name.contains("..") {
            return Err(super::map_bad_manifest(format!(
                "manifest dependency name contains invalid path characters: {name}"
            )));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
        {
            return Err(super::map_bad_manifest(format!(
                "manifest dependency name contains unsupported characters: {name}"
            )));
        }
        Ok(())
    }

    fn validate_sha256_hex(&self, value: &str, field_name: &str) -> Result<(), ImagodError> {
        if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(super::map_bad_manifest(format!(
                "{field_name} must be a 64-character hex string"
            )));
        }
        Ok(())
    }
}

fn normalize_capability_rule_map(
    _policy: &CapabilityPolicy,
    map: &BTreeMap<String, Vec<String>>,
    validator: &impl ManifestValidator,
) -> BTreeMap<String, Vec<String>> {
    let mut normalized = BTreeMap::new();
    for (key, values) in map {
        if key.trim().is_empty() {
            continue;
        }
        let normalized_values = validator.normalize_string_set(values);
        if !normalized_values.is_empty() {
            normalized.insert(key.clone(), normalized_values);
        }
    }
    normalized
}

pub(super) fn default_http_max_body_bytes() -> u64 {
    DEFAULT_HTTP_MAX_BODY_BYTES
}

pub(super) fn required_hash_targets_valid(targets: &[HashTarget]) -> bool {
    let required = [HashTarget::Wasm, HashTarget::Manifest, HashTarget::Assets]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let actual = targets.iter().copied().collect::<BTreeSet<_>>();
    required == actual && targets.len() == required.len()
}
