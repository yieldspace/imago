//! High-level orchestration for deploy/run/stop commands.

use std::{
    collections::BTreeMap,
    collections::BTreeSet,
    net::IpAddr,
    path::{Component, Path, PathBuf},
};

use imago_protocol::{DeployCommandPayload, ErrorCode, RunCommandPayload, StopCommandPayload};
use imagod_common::ImagodError;
use imagod_ipc::{
    CapabilityPolicy, PluginDependency, PluginKind, RunnerAppType, RunnerSocketConfig,
    ServiceBinding,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::{
    artifact_store::ArtifactStore,
    service_supervisor::{ServiceLaunch, ServiceLogSubscription, ServiceSupervisor},
};

const STAGE_ORCHESTRATE: &str = "orchestration";
const EXPECTED_CURRENT_RELEASE_ANY: &str = "any";
const RESTART_POLICY_NEVER: &str = "never";
const RESTART_POLICY_ON_FAILURE: &str = "on-failure";
const RESTART_POLICY_ALWAYS: &str = "always";
const RESTART_POLICY_UNLESS_STOPPED: &str = "unless-stopped";
const RESTART_POLICY_FILE_NAME: &str = "restart_policy";
const DEFAULT_HTTP_MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HTTP_MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
/// Release manifest loaded from extracted artifact.
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: RunnerAppType,
    #[serde(default)]
    http: Option<ManifestHttp>,
    #[serde(default)]
    socket: Option<RunnerSocketConfig>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    bindings: Vec<ManifestBinding>,
    #[serde(default)]
    dependencies: Vec<PluginDependency>,
    #[serde(default)]
    capabilities: CapabilityPolicy,
    hash: ManifestHash,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest-declared asset path.
struct ManifestAsset {
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest binding authorization entry.
struct ManifestBinding {
    target: String,
    wit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest HTTP execution settings.
struct ManifestHttp {
    port: u16,
    #[serde(default = "default_http_max_body_bytes")]
    max_body_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest hash metadata describing required verification targets.
struct ManifestHash {
    algorithm: String,
    targets: Vec<HashTarget>,
}

impl ManifestHash {
    fn validate_targets(&self) -> bool {
        let required = [HashTarget::Wasm, HashTarget::Manifest, HashTarget::Assets]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let actual = self.targets.iter().copied().collect::<BTreeSet<_>>();
        required == actual && self.targets.len() == required.len()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
/// Hash verification targets required by manifest metadata.
enum HashTarget {
    #[serde(rename = "wasm")]
    Wasm,
    #[serde(rename = "manifest")]
    Manifest,
    #[serde(rename = "assets")]
    Assets,
}

#[derive(Debug, Clone)]
/// Result summary returned after successful deploy.
pub struct DeploySummary {
    /// Service name that was deployed.
    pub service_name: String,
    /// Release hash activated for the service.
    pub release_hash: String,
}

#[derive(Debug, Clone)]
/// Result summary returned after successful run command.
pub struct RunSummary {
    /// Service name that was started.
    pub service_name: String,
    /// Active release hash used for start.
    pub release_hash: String,
}

#[derive(Debug)]
/// One failed service restore result at manager boot.
pub struct RestoreFailure {
    /// Service name that failed to restore.
    pub service_name: String,
    /// Failure detail returned by launch/load logic.
    pub error: ImagodError,
}

#[derive(Debug)]
/// Summary of service restore attempts executed at manager boot.
pub struct RestoreActiveServicesSummary {
    /// Services restored successfully.
    pub started: Vec<RunSummary>,
    /// Services that failed to restore.
    pub failed: Vec<RestoreFailure>,
}

#[derive(Debug, Clone)]
/// Result summary returned after successful stop command.
pub struct StopSummary {
    /// Service name that was stopped.
    pub service_name: String,
}

#[derive(Clone)]
/// Coordinates artifact validation, release promotion, and process supervision.
pub struct Orchestrator {
    storage_root: PathBuf,
    artifact_store: ArtifactStore,
    supervisor: ServiceSupervisor,
}

/// Prepared release context passed from deploy preparation into final activation.
struct PreparedRelease {
    service_name: String,
    service_root: PathBuf,
    release_hash: String,
    active_file: PathBuf,
    restart_policy_file: PathBuf,
    previous_release: Option<String>,
    previous_restart_policy: Option<String>,
    launch: ServiceLaunch,
}

impl Orchestrator {
    /// Creates an orchestrator with shared storage and supervisor handles.
    pub fn new(
        storage_root: impl AsRef<Path>,
        artifact_store: ArtifactStore,
        supervisor: ServiceSupervisor,
    ) -> Self {
        Self {
            storage_root: storage_root.as_ref().to_path_buf(),
            artifact_store,
            supervisor,
        }
    }

    /// Handles deploy orchestration and service replacement.
    pub async fn deploy(
        &self,
        payload: &DeployCommandPayload,
    ) -> Result<DeploySummary, ImagodError> {
        let prepared = self.prepare_release(payload).await?;

        let launch = prepared.launch.clone();
        if let Err(start_error) = self.supervisor.replace(launch).await {
            if payload.auto_rollback {
                self.rollback_previous_release(&prepared).await?;
            }
            return Err(start_error);
        }

        fs::write(&prepared.active_file, prepared.release_hash.as_bytes())
            .await
            .map_err(|e| map_internal(format!("active release update failed: {e}")))?;
        write_restart_policy(&prepared.restart_policy_file, &payload.restart_policy).await?;

        Ok(DeploySummary {
            service_name: prepared.service_name,
            release_hash: prepared.release_hash,
        })
    }

    /// Starts a service from its currently active release.
    pub async fn run(&self, payload: &RunCommandPayload) -> Result<RunSummary, ImagodError> {
        let service_root = self.storage_root.join("services").join(&payload.name);
        let active_file = service_root.join("active_release");
        let active_release = read_active_release(&active_file).await?.ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                STAGE_ORCHESTRATE,
                format!("service '{}' has no active release", payload.name),
            )
        })?;

        let launch = self
            .load_launch_from_release(&payload.name, &service_root, &active_release)
            .await?;
        self.supervisor.start(launch).await?;

        Ok(RunSummary {
            service_name: payload.name.clone(),
            release_hash: active_release,
        })
    }

    /// Restores services marked with `restart_policy=always` at manager boot.
    pub async fn restore_active_services_on_boot(
        &self,
    ) -> Result<RestoreActiveServicesSummary, ImagodError> {
        let (candidates, mut failed) = collect_boot_restore_candidates(&self.storage_root).await?;
        let mut started = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            match self.restore_service_from_release(&candidate).await {
                Ok(summary) => started.push(summary),
                Err(error) => failed.push(RestoreFailure {
                    service_name: candidate.service_name,
                    error,
                }),
            }
        }

        Ok(RestoreActiveServicesSummary { started, failed })
    }

    /// Removes cached plugin components not referenced by any active release at boot.
    pub async fn gc_unused_plugin_components_on_boot(&self) -> Result<(), ImagodError> {
        gc_unused_plugin_components_on_boot(&self.storage_root).await
    }

    /// Stops a running service.
    pub async fn stop(&self, payload: &StopCommandPayload) -> Result<StopSummary, ImagodError> {
        self.supervisor.stop(&payload.name, payload.force).await?;
        Ok(StopSummary {
            service_name: payload.name.clone(),
        })
    }

    /// Reaps finished runner processes from supervisor state.
    pub async fn reap_finished_services(&self) {
        self.supervisor.reap_finished().await;
    }

    /// Returns whether any services are currently running or stopping.
    pub async fn has_live_services(&self) -> bool {
        self.supervisor.has_live_services().await
    }

    /// Stops all currently running services.
    pub async fn stop_all_services(&self, force: bool) -> Vec<(String, ImagodError)> {
        self.supervisor.stop_all(force).await
    }

    /// Returns names of currently running services.
    pub async fn running_service_names(&self) -> Vec<String> {
        self.supervisor.running_service_names().await
    }

    /// Opens one service logs snapshot and optional follow stream.
    pub async fn open_logs(
        &self,
        service_name: &str,
        tail_lines: u32,
        follow: bool,
    ) -> Result<ServiceLogSubscription, ImagodError> {
        self.supervisor
            .open_logs(service_name, tail_lines, follow)
            .await
    }

    /// Prepares a validated release and launch spec from committed artifact data.
    async fn prepare_release(
        &self,
        payload: &DeployCommandPayload,
    ) -> Result<PreparedRelease, ImagodError> {
        let committed = self
            .artifact_store
            .committed_artifact(&payload.deploy_id)
            .await?;
        let staging_dir = self.storage_root.join("staging").join(&committed.deploy_id);

        clean_dir(&staging_dir).await?;
        fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| map_internal(format!("failed to create staging dir: {e}")))?;

        extract_tar(&committed.path, &staging_dir).await?;

        let manifest_path = staging_dir.join("manifest.json");
        let manifest_bytes = fs::read(&manifest_path)
            .await
            .map_err(|e| map_bad_manifest(format!("manifest read failed: {e}")))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| map_bad_manifest(format!("manifest parse failed: {e}")))?;

        if manifest.hash.algorithm != "sha256" || !manifest.hash.validate_targets() {
            return Err(map_bad_manifest(
                "manifest hash metadata is invalid".to_string(),
            ));
        }
        validate_service_name(&manifest.name)?;

        let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
        if manifest_digest != committed.manifest_digest {
            return Err(map_bad_manifest(
                "manifest digest does not match artifact metadata".to_string(),
            ));
        }

        let release_hash = release_id_from_artifact_digest(&committed.artifact_digest);
        let service_root = self.storage_root.join("services").join(&manifest.name);
        let release_dir = service_root.join(&release_hash);
        let active_file = service_root.join("active_release");
        let restart_policy_file = service_root.join(RESTART_POLICY_FILE_NAME);
        let previous_release = read_active_release(&active_file).await?;
        let previous_restart_policy = read_restart_policy(&restart_policy_file).await?;
        validate_deploy_preconditions(payload, previous_release.as_deref())?;

        fs::create_dir_all(&service_root)
            .await
            .map_err(|e| map_internal(format!("service root creation failed: {e}")))?;
        promote_staging_release(&staging_dir, &release_dir).await?;

        cleanup_old_releases(&service_root, &release_hash, previous_release.as_deref()).await?;

        let launch =
            build_launch_from_release(&self.storage_root, &release_hash, &release_dir, &manifest)
                .await?;

        Ok(PreparedRelease {
            service_name: manifest.name,
            service_root,
            release_hash,
            active_file,
            restart_policy_file,
            previous_release,
            previous_restart_policy,
            launch,
        })
    }

    /// Attempts to roll back active release marker when replacement start fails.
    async fn rollback_previous_release(
        &self,
        prepared: &PreparedRelease,
    ) -> Result<(), ImagodError> {
        let Some(previous_release) = prepared.previous_release.as_deref() else {
            return Ok(());
        };

        fs::write(&prepared.active_file, previous_release.as_bytes())
            .await
            .map_err(|e| {
                ImagodError::new(
                    ErrorCode::RollbackFailed,
                    STAGE_ORCHESTRATE,
                    format!("failed to write rollback release: {e}"),
                )
            })?;
        restore_restart_policy(
            &prepared.restart_policy_file,
            prepared.previous_restart_policy.as_deref(),
        )
        .await
        .map_err(map_rollback_error)?;

        let previous_launch = self
            .load_launch_from_release(
                &prepared.service_name,
                &prepared.service_root,
                previous_release,
            )
            .await
            .map_err(map_rollback_error)?;
        self.supervisor
            .start(previous_launch)
            .await
            .map_err(map_rollback_error)?;

        Ok(())
    }

    /// Loads launch information from an existing promoted release.
    async fn load_launch_from_release(
        &self,
        service_name: &str,
        service_root: &Path,
        release_hash: &str,
    ) -> Result<ServiceLaunch, ImagodError> {
        let release_dir = service_root.join(release_hash);
        let manifest_path = release_dir.join("manifest.json");

        let manifest_bytes = fs::read(&manifest_path)
            .await
            .map_err(|e| map_bad_manifest(format!("manifest read failed: {e}")))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| map_bad_manifest(format!("manifest parse failed: {e}")))?;

        if manifest.name != service_name {
            return Err(map_bad_manifest(format!(
                "release manifest name mismatch: expected {}, got {}",
                service_name, manifest.name
            )));
        }

        build_launch_from_release(&self.storage_root, release_hash, &release_dir, &manifest).await
    }

    /// Starts one service from a discovered boot-restore candidate.
    async fn restore_service_from_release(
        &self,
        candidate: &BootRestoreCandidate,
    ) -> Result<RunSummary, ImagodError> {
        let launch = self
            .load_launch_from_release(
                &candidate.service_name,
                &candidate.service_root,
                &candidate.release_hash,
            )
            .await?;
        self.supervisor.start(launch).await?;

        Ok(RunSummary {
            service_name: candidate.service_name.clone(),
            release_hash: candidate.release_hash.clone(),
        })
    }
}

/// One boot restore target discovered from `services/<name>/active_release` and `restart_policy=always`.
struct BootRestoreCandidate {
    service_name: String,
    service_root: PathBuf,
    release_hash: String,
}

/// Builds launch metadata for supervisor from a promoted release directory.
async fn build_launch_from_release(
    storage_root: &Path,
    release_hash: &str,
    release_dir: &Path,
    manifest: &Manifest,
) -> Result<ServiceLaunch, ImagodError> {
    let normalized_main = normalize_manifest_main_path(&manifest.main)?;
    let component_path = release_dir.join(&normalized_main);
    if !component_path.starts_with(release_dir) {
        return Err(map_bad_manifest(format!(
            "manifest main path resolved outside release dir: {}",
            manifest.main
        )));
    }
    if let Err(err) = fs::metadata(&component_path).await {
        return Err(map_bad_manifest(format!(
            "component path is not accessible: {} ({err})",
            component_path.display(),
        )));
    }

    let mut envs: BTreeMap<String, String> = manifest.vars.clone();
    for (k, v) in &manifest.secrets {
        envs.insert(k.clone(), v.clone());
    }

    let bindings = validate_manifest_bindings(&manifest.bindings)?;
    let (http_port, http_max_body_bytes) = validate_manifest_http(manifest)?;
    let socket = validate_manifest_socket(manifest)?;
    let plugin_dependencies =
        prepare_plugin_dependencies(storage_root, release_dir, &manifest.dependencies).await?;

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
        capabilities: normalize_capability_policy(&manifest.capabilities),
    })
}

fn default_http_max_body_bytes() -> u64 {
    DEFAULT_HTTP_MAX_BODY_BYTES
}

fn validate_manifest_http(manifest: &Manifest) -> Result<(Option<u16>, Option<u64>), ImagodError> {
    match manifest.app_type {
        RunnerAppType::Http => {
            let http = manifest.http.as_ref().ok_or_else(|| {
                map_bad_manifest("manifest.http is required when type=\"http\"".to_string())
            })?;
            if http.port == 0 {
                return Err(map_bad_manifest(
                    "manifest.http.port must be in range 1..=65535".to_string(),
                ));
            }
            if http.max_body_bytes == 0 || http.max_body_bytes > MAX_HTTP_MAX_BODY_BYTES {
                return Err(map_bad_manifest(format!(
                    "manifest.http.max_body_bytes must be in range 1..={} (got {})",
                    MAX_HTTP_MAX_BODY_BYTES, http.max_body_bytes
                )));
            }
            Ok((Some(http.port), Some(http.max_body_bytes)))
        }
        RunnerAppType::Cli | RunnerAppType::Socket => {
            if manifest.http.is_some() {
                return Err(map_bad_manifest(
                    "manifest.http is only allowed when type=\"http\"".to_string(),
                ));
            }
            Ok((None, None))
        }
    }
}

fn validate_manifest_socket(
    manifest: &Manifest,
) -> Result<Option<RunnerSocketConfig>, ImagodError> {
    match manifest.app_type {
        RunnerAppType::Socket => {
            let socket = manifest.socket.clone().ok_or_else(|| {
                map_bad_manifest("manifest.socket is required when type=\"socket\"".to_string())
            })?;
            if socket.listen_port == 0 {
                return Err(map_bad_manifest(
                    "manifest.socket.listen_port must be in range 1..=65535".to_string(),
                ));
            }
            socket.listen_addr.parse::<IpAddr>().map_err(|err| {
                map_bad_manifest(format!(
                    "manifest.socket.listen_addr must be a valid IP address (got '{}'): {err}",
                    socket.listen_addr
                ))
            })?;
            Ok(Some(socket))
        }
        RunnerAppType::Cli | RunnerAppType::Http => {
            if manifest.socket.is_some() {
                return Err(map_bad_manifest(
                    "manifest.socket is only allowed when type=\"socket\"".to_string(),
                ));
            }
            Ok(None)
        }
    }
}

fn validate_manifest_bindings(
    bindings: &[ManifestBinding],
) -> Result<Vec<ServiceBinding>, ImagodError> {
    let mut normalized = Vec::with_capacity(bindings.len());
    for (idx, binding) in bindings.iter().enumerate() {
        if binding.target.is_empty() {
            return Err(map_bad_manifest(format!(
                "manifest.bindings[{idx}].target must not be empty"
            )));
        }
        if binding.wit.is_empty() {
            return Err(map_bad_manifest(format!(
                "manifest.bindings[{idx}].wit must not be empty"
            )));
        }
        if let Err(err) = validate_service_name(&binding.target) {
            return Err(map_bad_manifest(format!(
                "manifest.bindings[{idx}].target is invalid '{}': {}",
                binding.target, err.message
            )));
        }
        normalized.push(ServiceBinding {
            target: binding.target.clone(),
            wit: binding.wit.clone(),
        });
    }
    Ok(normalized)
}

async fn prepare_plugin_dependencies(
    storage_root: &Path,
    release_dir: &Path,
    dependencies: &[PluginDependency],
) -> Result<Vec<PluginDependency>, ImagodError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }

    let mut known_names = BTreeSet::new();
    for dep in dependencies {
        validate_plugin_package_name(&dep.name)?;
        if dep.version.trim().is_empty() {
            return Err(map_bad_manifest(format!(
                "manifest.dependencies[{}].version must not be empty",
                dep.name
            )));
        }
        if dep.wit.trim().is_empty() {
            return Err(map_bad_manifest(format!(
                "manifest.dependencies[{}].wit must not be empty",
                dep.name
            )));
        }
        if !known_names.insert(dep.name.clone()) {
            return Err(map_bad_manifest(format!(
                "manifest.dependencies contains duplicate dependency '{}'",
                dep.name
            )));
        }
    }

    let mut normalized = Vec::with_capacity(dependencies.len());
    let components_root = plugin_component_cache_root(storage_root);
    fs::create_dir_all(&components_root).await.map_err(|e| {
        map_internal(format!(
            "failed to create plugin component cache dir {}: {e}",
            components_root.display()
        ))
    })?;

    for dep in dependencies {
        for required in &dep.requires {
            validate_plugin_package_name(required)?;
            if !known_names.contains(required) {
                return Err(map_bad_manifest(format!(
                    "manifest.dependencies[{}].requires references unknown dependency '{}'",
                    dep.name, required
                )));
            }
        }

        let mut dep = dep.clone();
        dep.capabilities = normalize_capability_policy(&dep.capabilities);
        dep.requires = normalize_string_set(&dep.requires);

        match dep.kind {
            PluginKind::Native => {
                if dep.component.is_some() {
                    return Err(map_bad_manifest(format!(
                        "manifest.dependencies[{}].component is only allowed when kind=\"wasm\"",
                        dep.name
                    )));
                }
            }
            PluginKind::Wasm => {
                let component = dep.component.clone().ok_or_else(|| {
                    map_bad_manifest(format!(
                        "manifest.dependencies[{}].component is required when kind=\"wasm\"",
                        dep.name
                    ))
                })?;
                validate_sha256_hex(
                    &component.sha256,
                    &format!("manifest.dependencies[{}].component.sha256", dep.name),
                )?;

                let component_path_str = component.path.to_str().ok_or_else(|| {
                    map_bad_manifest(format!(
                        "manifest.dependencies[{}].component.path must be valid UTF-8",
                        dep.name
                    ))
                })?;
                let normalized_component_path = normalize_manifest_relative_path(
                    component_path_str,
                    &format!("manifest.dependencies[{}].component.path", dep.name),
                )?;
                let release_component_path = release_dir.join(&normalized_component_path);
                let metadata = fs::metadata(&release_component_path).await.map_err(|e| {
                    map_bad_manifest(format!(
                        "plugin component is not accessible: {} ({e})",
                        release_component_path.display()
                    ))
                })?;
                if !metadata.is_file() {
                    return Err(map_bad_manifest(format!(
                        "plugin component path is not a file: {}",
                        release_component_path.display()
                    )));
                }

                let digest = compute_sha256_hex_async(&release_component_path).await?;
                if !digest.eq_ignore_ascii_case(&component.sha256) {
                    return Err(map_bad_manifest(format!(
                        "plugin component sha256 mismatch for '{}': expected {}, actual {}",
                        dep.name, component.sha256, digest
                    )));
                }

                let cache_path = components_root.join(format!("{}.wasm", component.sha256));
                let cache_digest_matches = match fs::metadata(&cache_path).await {
                    Ok(existing_meta) => {
                        if !existing_meta.is_file() {
                            return Err(map_internal(format!(
                                "plugin component cache path is not a file: {}",
                                cache_path.display()
                            )));
                        }
                        let existing = compute_sha256_hex_async(&cache_path).await?;
                        existing.eq_ignore_ascii_case(&component.sha256)
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                    Err(err) => {
                        return Err(map_internal(format!(
                            "failed to inspect plugin component cache {}: {err}",
                            cache_path.display()
                        )));
                    }
                };
                if !cache_digest_matches {
                    fs::copy(&release_component_path, &cache_path)
                        .await
                        .map_err(|e| {
                            map_internal(format!(
                                "failed to copy plugin component to cache {}: {e}",
                                cache_path.display()
                            ))
                        })?;
                }

                dep.component = Some(imagod_ipc::PluginComponent {
                    path: cache_path,
                    sha256: component.sha256,
                });
            }
        }

        normalized.push(dep);
    }

    Ok(normalized)
}

fn normalize_capability_policy(policy: &CapabilityPolicy) -> CapabilityPolicy {
    CapabilityPolicy {
        privileged: policy.privileged,
        deps: normalize_capability_rule_map(&policy.deps),
        wasi: normalize_capability_rule_map(&policy.wasi),
    }
}

fn normalize_capability_rule_map(
    map: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    let mut normalized = BTreeMap::new();
    for (key, values) in map {
        if key.trim().is_empty() {
            continue;
        }
        let normalized_values = normalize_string_set(values);
        if !normalized_values.is_empty() {
            normalized.insert(key.clone(), normalized_values);
        }
    }
    normalized
}

fn normalize_string_set(values: &[String]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            set.insert(value.to_string());
        }
    }
    set.into_iter().collect()
}

fn validate_plugin_package_name(name: &str) -> Result<(), ImagodError> {
    if name.is_empty() {
        return Err(map_bad_manifest(
            "manifest.dependencies[].name must not be empty".to_string(),
        ));
    }
    if name.contains('\\') || name.contains("..") {
        return Err(map_bad_manifest(format!(
            "manifest dependency name contains invalid path characters: {name}"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
    {
        return Err(map_bad_manifest(format!(
            "manifest dependency name contains unsupported characters: {name}"
        )));
    }
    Ok(())
}

fn validate_sha256_hex(value: &str, field_name: &str) -> Result<(), ImagodError> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(map_bad_manifest(format!(
            "{field_name} must be a 64-character hex string"
        )));
    }
    Ok(())
}

fn plugin_component_cache_root(storage_root: &Path) -> PathBuf {
    storage_root.join("plugins").join("components")
}

async fn compute_sha256_hex_async(path: &Path) -> Result<String, ImagodError> {
    let mut file = fs::File::open(path).await.map_err(|e| {
        map_internal(format!(
            "failed to open file for sha256 {}: {e}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).await.map_err(|e| {
            map_internal(format!(
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

async fn gc_unused_plugin_components_on_boot(storage_root: &Path) -> Result<(), ImagodError> {
    let components_root = plugin_component_cache_root(storage_root);
    let referenced = collect_referenced_plugin_component_hashes(storage_root).await?;

    let mut entries = match fs::read_dir(&components_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(map_internal(format!(
                "failed to read plugin components dir {}: {err}",
                components_root.display()
            )));
        }
    };

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| map_internal(format!("failed to iterate plugin components dir: {e}")))?
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

async fn collect_referenced_plugin_component_hashes(
    storage_root: &Path,
) -> Result<BTreeSet<String>, ImagodError> {
    let services_root = storage_root.join("services");
    let mut entries = match fs::read_dir(&services_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(err) => {
            return Err(map_internal(format!(
                "failed to read services root for plugin gc {}: {err}",
                services_root.display()
            )));
        }
    };

    let mut referenced = BTreeSet::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| map_internal(format!("failed to iterate services root: {e}")))?
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

        let active = match read_active_release(&service_root.join("active_release")).await {
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
        let manifest: Manifest = match serde_json::from_slice(&manifest_bytes) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!(
                    "plugin component gc skipped unparsable manifest {}: {}",
                    manifest_path.display(),
                    err
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

/// Extracts a tar archive into destination while rejecting unsupported entries.
async fn extract_tar(bundle: &Path, dest: &Path) -> Result<(), ImagodError> {
    let bundle = bundle.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), ImagodError> {
        let file = std::fs::File::open(&bundle)
            .map_err(|e| map_bad_manifest(format!("artifact open failed: {e}")))?;
        let mut archive = tar::Archive::new(file);

        let entries = archive
            .entries()
            .map_err(|e| map_bad_manifest(format!("artifact entries failed: {e}")))?;
        for entry in entries {
            let mut entry =
                entry.map_err(|e| map_bad_manifest(format!("artifact entry read failed: {e}")))?;
            let entry_type = entry.header().entry_type();
            if !(entry_type.is_file() || entry_type.is_dir()) {
                return Err(map_bad_manifest(format!(
                    "artifact contains unsupported entry type: {entry_type:?}"
                )));
            }

            let entry_path = entry
                .path()
                .map_err(|e| map_bad_manifest(format!("artifact entry path failed: {e}")))?;
            let relative = normalize_archive_entry_path(entry_path.as_ref())?;
            let output_path = dest.join(relative);

            if !output_path.starts_with(&dest) {
                return Err(map_bad_manifest(
                    "artifact entry resolved outside destination".to_string(),
                ));
            }

            if entry_type.is_dir() {
                std::fs::create_dir_all(&output_path).map_err(|e| {
                    map_bad_manifest(format!(
                        "artifact directory create failed {}: {e}",
                        output_path.display()
                    ))
                })?;
                continue;
            }

            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    map_bad_manifest(format!(
                        "artifact parent directory create failed {}: {e}",
                        parent.display()
                    ))
                })?;
            }

            entry.unpack(&output_path).map_err(|e| {
                map_bad_manifest(format!(
                    "artifact unpack failed for {}: {e}",
                    output_path.display()
                ))
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|e| map_internal(format!("artifact unpack task join failed: {e}")))?
}

fn normalize_archive_entry_path(path: &Path) -> Result<PathBuf, ImagodError> {
    if path.as_os_str().is_empty() {
        return Err(map_bad_manifest(
            "artifact contains empty entry path".to_string(),
        ));
    }
    if path.is_absolute() {
        return Err(map_bad_manifest(format!(
            "artifact contains absolute entry path: {}",
            path.display()
        )));
    }

    let raw = path.as_os_str().to_string_lossy();
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return Err(map_bad_manifest(format!(
            "artifact contains windows-prefixed entry path: {raw}"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(map_bad_manifest(format!(
                    "artifact contains invalid entry path: {}",
                    path.display()
                )));
            }
            _ => {
                return Err(map_bad_manifest(format!(
                    "artifact contains invalid entry path: {}",
                    path.display()
                )));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(map_bad_manifest(format!(
            "artifact contains invalid entry path: {}",
            path.display()
        )));
    }

    Ok(normalized)
}

fn normalize_manifest_main_path(main: &str) -> Result<PathBuf, ImagodError> {
    normalize_manifest_relative_path(main, "manifest.main")
}

fn normalize_manifest_relative_path(raw: &str, field_name: &str) -> Result<PathBuf, ImagodError> {
    let path = Path::new(raw);
    if raw.is_empty() || path.as_os_str().is_empty() {
        return Err(map_bad_manifest(format!("{field_name} must not be empty")));
    }
    if path.is_absolute() {
        return Err(map_bad_manifest(format!(
            "{field_name} must be a relative path: {raw}"
        )));
    }

    let raw_os = path.as_os_str().to_string_lossy();
    if raw_os.len() >= 2 && raw_os.as_bytes()[1] == b':' {
        return Err(map_bad_manifest(format!(
            "{field_name} must not be windows-prefixed: {raw}"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(map_bad_manifest(format!(
                    "{field_name} contains invalid path traversal: {raw}"
                )));
            }
            _ => {
                return Err(map_bad_manifest(format!(
                    "{field_name} contains invalid path component: {raw}"
                )));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(map_bad_manifest(format!("{field_name} is invalid: {raw}")));
    }

    Ok(normalized)
}

fn validate_service_name(name: &str) -> Result<(), ImagodError> {
    if name.is_empty() {
        return Err(map_bad_manifest(
            "manifest.name must not be empty".to_string(),
        ));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(map_bad_manifest(format!(
            "manifest.name contains invalid path characters: {name}"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(map_bad_manifest(format!(
            "manifest.name contains unsupported characters: {name}"
        )));
    }
    Ok(())
}

/// Validates deploy preconditions against currently active release.
fn validate_deploy_preconditions(
    payload: &DeployCommandPayload,
    active_release: Option<&str>,
) -> Result<(), ImagodError> {
    if !is_supported_restart_policy(&payload.restart_policy) {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_ORCHESTRATE,
            format!(
                "unsupported restart_policy '{}': supported values are '{}', '{}', '{}' and '{}'",
                payload.restart_policy,
                RESTART_POLICY_NEVER,
                RESTART_POLICY_ON_FAILURE,
                RESTART_POLICY_ALWAYS,
                RESTART_POLICY_UNLESS_STOPPED
            ),
        ));
    }

    if payload.expected_current_release == EXPECTED_CURRENT_RELEASE_ANY {
        return Ok(());
    }

    let actual = active_release.unwrap_or("none");
    if payload.expected_current_release == actual {
        return Ok(());
    }

    Err(ImagodError::new(
        ErrorCode::PreconditionFailed,
        STAGE_ORCHESTRATE,
        "expected_current_release does not match active release",
    )
    .with_detail(
        "expected_current_release",
        payload.expected_current_release.clone(),
    )
    .with_detail("actual_current_release", actual.to_string()))
}

fn is_supported_restart_policy(value: &str) -> bool {
    matches!(
        value,
        RESTART_POLICY_NEVER
            | RESTART_POLICY_ON_FAILURE
            | RESTART_POLICY_ALWAYS
            | RESTART_POLICY_UNLESS_STOPPED
    )
}

async fn write_restart_policy(path: &Path, policy: &str) -> Result<(), ImagodError> {
    if !is_supported_restart_policy(policy) {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_ORCHESTRATE,
            format!("invalid restart_policy: {policy}"),
        ));
    }
    fs::write(path, policy.as_bytes())
        .await
        .map_err(|e| map_internal(format!("restart policy update failed: {e}")))
}

async fn restore_restart_policy(path: &Path, policy: Option<&str>) -> Result<(), ImagodError> {
    match policy {
        Some(policy) => write_restart_policy(path, policy).await,
        None => match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(map_internal(format!(
                "failed to remove restart policy file {}: {err}",
                path.display()
            ))),
        },
    }
}

async fn cleanup_old_releases(
    service_root: &Path,
    new_release: &str,
    previous_release: Option<&str>,
) -> Result<(), ImagodError> {
    let mut entries = fs::read_dir(service_root)
        .await
        .map_err(|e| map_internal(format!("failed to read service root: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| map_internal(format!("failed to iterate service root: {e}")))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|e| map_internal(format!("failed to read entry type: {e}")))?;
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == new_release {
            continue;
        }
        if previous_release.is_some_and(|prev| prev == name) {
            continue;
        }
        fs::remove_dir_all(&path).await.map_err(|e| {
            map_internal(format!(
                "failed to cleanup old release {}: {e}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

async fn read_active_release(path: &Path) -> Result<Option<String>, ImagodError> {
    match fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(map_internal(format!("failed to read active release: {e}"))),
    }
}

async fn read_restart_policy(path: &Path) -> Result<Option<String>, ImagodError> {
    match fs::read_to_string(path).await {
        Ok(content) => {
            let policy = content.trim().to_string();
            if !is_supported_restart_policy(&policy) {
                return Err(map_internal(format!(
                    "invalid restart policy persisted in {}: {}",
                    path.display(),
                    policy
                )));
            }
            Ok(Some(policy))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(map_internal(format!(
            "failed to read restart policy: {err}"
        ))),
    }
}

async fn read_restart_policy_or_default(path: &Path) -> Result<String, ImagodError> {
    Ok(read_restart_policy(path)
        .await?
        .unwrap_or_else(|| RESTART_POLICY_NEVER.to_string()))
}

/// Collects restart-policy-eligible (`always`) active-release candidates sorted by service name.
async fn collect_boot_restore_candidates(
    storage_root: &Path,
) -> Result<(Vec<BootRestoreCandidate>, Vec<RestoreFailure>), ImagodError> {
    let services_root = storage_root.join("services");
    let mut entries = match fs::read_dir(&services_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(e) => {
            return Err(map_internal(format!(
                "failed to read services root {}: {e}",
                services_root.display()
            )));
        }
    };

    let mut service_entries = Vec::new();
    let mut failed = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| map_internal(format!("failed to iterate services root: {e}")))?
    {
        let service_name = entry.file_name().to_string_lossy().to_string();
        let service_root = entry.path();
        classify_boot_restore_entry(
            service_name,
            service_root,
            entry.file_type().await,
            &mut service_entries,
            &mut failed,
        );
    }
    service_entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut candidates = Vec::new();
    for (service_name, service_root) in service_entries {
        let restart_policy_file = service_root.join(RESTART_POLICY_FILE_NAME);
        let restart_policy = match read_restart_policy_or_default(&restart_policy_file).await {
            Ok(restart_policy) => restart_policy,
            Err(error) => {
                failed.push(RestoreFailure {
                    service_name: service_name.clone(),
                    error,
                });
                continue;
            }
        };
        if restart_policy != RESTART_POLICY_ALWAYS {
            continue;
        }

        let active_file = service_root.join("active_release");
        let active_release = match read_active_release(&active_file).await {
            Ok(Some(active_release)) => active_release,
            Ok(None) => continue,
            Err(error) => {
                failed.push(RestoreFailure {
                    service_name: service_name.clone(),
                    error,
                });
                continue;
            }
        };

        if active_release.is_empty() {
            failed.push(RestoreFailure {
                service_name,
                error: map_bad_manifest("active_release must not be empty".to_string()),
            });
            continue;
        }

        candidates.push(BootRestoreCandidate {
            service_name,
            service_root,
            release_hash: active_release,
        });
    }

    Ok((candidates, failed))
}

/// Classifies one services-root entry and accumulates per-service failures.
fn classify_boot_restore_entry(
    service_name: String,
    service_root: PathBuf,
    file_type: Result<std::fs::FileType, std::io::Error>,
    service_entries: &mut Vec<(String, PathBuf)>,
    failed: &mut Vec<RestoreFailure>,
) {
    match file_type {
        Ok(file_type) => {
            if file_type.is_dir() {
                service_entries.push((service_name, service_root));
            }
        }
        Err(err) => {
            let error = map_internal(format!(
                "failed to read service entry type for '{}': {err}",
                service_name
            ));
            failed.push(RestoreFailure {
                service_name,
                error,
            });
        }
    }
}

async fn clean_dir(path: &Path) -> Result<(), ImagodError> {
    match fs::remove_dir_all(path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(map_internal(format!(
            "failed to remove dir {}: {e}",
            path.display()
        ))),
    }
}

/// Promotes staging release directory into the service release path atomically.
async fn promote_staging_release(
    staging_dir: &Path,
    release_dir: &Path,
) -> Result<(), ImagodError> {
    match fs::metadata(release_dir).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return fs::rename(staging_dir, release_dir)
                .await
                .map_err(|err| map_internal(format!("release move failed: {err}")));
        }
        Err(e) => {
            return Err(map_internal(format!(
                "failed to inspect release dir {}: {e}",
                release_dir.display()
            )));
        }
    }

    let release_name = release_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("release");
    let backup_dir =
        release_dir.with_file_name(format!("{}.swap-backup-{}", release_name, Uuid::new_v4()));

    fs::rename(release_dir, &backup_dir)
        .await
        .map_err(|e| map_internal(format!("failed to move release to backup: {e}")))?;

    match fs::rename(staging_dir, release_dir).await {
        Ok(_) => {
            fs::remove_dir_all(&backup_dir).await.map_err(|e| {
                map_internal(format!(
                    "failed to cleanup release backup {}: {e}",
                    backup_dir.display()
                ))
            })?;
            Ok(())
        }
        Err(e) => {
            let restore_err = fs::rename(&backup_dir, release_dir).await.err();
            if let Some(restore_err) = restore_err {
                return Err(map_internal(format!(
                    "release move failed: {e}; rollback restore failed: {restore_err}"
                )));
            }
            Err(map_internal(format!(
                "release move failed and backup restored: {e}"
            )))
        }
    }
}

fn release_id_from_artifact_digest(full: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(full.as_bytes());
    hex::encode(hasher.finalize())
}

fn map_internal(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_ORCHESTRATE, message).with_retryable(true)
}

fn map_bad_manifest(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::BadManifest, STAGE_ORCHESTRATE, message)
}

fn map_rollback_error(err: ImagodError) -> ImagodError {
    ImagodError::new(
        ErrorCode::RollbackFailed,
        STAGE_ORCHESTRATE,
        format!("rollback failed: {}", err.message),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_HTTP_MAX_BODY_BYTES, HashTarget, MAX_HTTP_MAX_BODY_BYTES, Manifest, ManifestAsset,
        ManifestBinding, ManifestHash, ManifestHttp, RESTART_POLICY_ALWAYS,
        RESTART_POLICY_FILE_NAME, RunnerAppType, build_launch_from_release,
        classify_boot_restore_entry, collect_boot_restore_candidates, extract_tar,
        gc_unused_plugin_components_on_boot, normalize_archive_entry_path,
        normalize_manifest_main_path, promote_staging_release, release_id_from_artifact_digest,
        validate_deploy_preconditions, validate_service_name,
    };
    use imago_protocol::{DeployCommandPayload, ErrorCode};
    use imagod_ipc::{
        CapabilityPolicy, PluginComponent, PluginDependency, PluginKind, RunnerSocketConfig,
        RunnerSocketDirection, RunnerSocketProtocol,
    };
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };
    use tar::{Builder, Header};
    use uuid::Uuid;

    fn temp_dir_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("imago-{prefix}-{}", Uuid::new_v4()))
    }

    fn valid_manifest() -> Manifest {
        Manifest {
            name: "svc-a".to_string(),
            main: "component.wasm".to_string(),
            app_type: RunnerAppType::Cli,
            http: None,
            socket: None,
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            assets: Vec::<ManifestAsset>::new(),
            bindings: Vec::new(),
            dependencies: Vec::<PluginDependency>::new(),
            capabilities: CapabilityPolicy::default(),
            hash: ManifestHash {
                algorithm: "sha256".to_string(),
                targets: vec![HashTarget::Wasm, HashTarget::Manifest, HashTarget::Assets],
            },
        }
    }

    #[test]
    fn normalize_archive_entry_path_rejects_traversal_and_absolute_paths() {
        assert!(normalize_archive_entry_path(Path::new("../evil")).is_err());
        assert!(normalize_archive_entry_path(Path::new("/etc/passwd")).is_err());
        assert!(normalize_archive_entry_path(Path::new("C:\\windows\\system32")).is_err());
        assert!(normalize_archive_entry_path(Path::new("")).is_err());
    }

    #[test]
    fn release_id_is_stable_64_hex_hash() {
        let release = release_id_from_artifact_digest("sha256:artifact-digest");
        assert_eq!(release.len(), 64);
        assert!(release.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn normalize_manifest_main_rejects_unsafe_paths() {
        assert!(normalize_manifest_main_path("../evil.wasm").is_err());
        assert!(normalize_manifest_main_path("/tmp/evil.wasm").is_err());
        assert!(normalize_manifest_main_path("C:\\evil.wasm").is_err());
        assert!(normalize_manifest_main_path("").is_err());
    }

    #[test]
    fn validate_service_name_rejects_invalid_values() {
        assert!(validate_service_name("svc-good_1.0").is_ok());
        assert!(validate_service_name("../evil").is_err());
        assert!(validate_service_name("svc/evil").is_err());
        assert!(validate_service_name("svc\\evil").is_err());
        assert!(validate_service_name("svc evil").is_err());
        assert!(validate_service_name("").is_err());
    }

    #[test]
    fn deploy_precondition_accepts_any_and_matching_release() {
        for restart_policy in ["never", "on-failure", "always", "unless-stopped"] {
            let payload_any = DeployCommandPayload {
                deploy_id: "deploy-1".to_string(),
                expected_current_release: "any".to_string(),
                restart_policy: restart_policy.to_string(),
                auto_rollback: true,
            };
            assert!(validate_deploy_preconditions(&payload_any, None).is_ok());

            let payload_match = DeployCommandPayload {
                deploy_id: "deploy-1".to_string(),
                expected_current_release: "release-abc".to_string(),
                restart_policy: restart_policy.to_string(),
                auto_rollback: true,
            };
            assert!(validate_deploy_preconditions(&payload_match, Some("release-abc")).is_ok());
        }
    }

    #[test]
    fn deploy_precondition_rejects_mismatch_and_bad_restart_policy() {
        let mismatch_payload = DeployCommandPayload {
            deploy_id: "deploy-1".to_string(),
            expected_current_release: "release-expected".to_string(),
            restart_policy: "never".to_string(),
            auto_rollback: true,
        };
        let mismatch = validate_deploy_preconditions(&mismatch_payload, Some("release-actual"))
            .expect_err("mismatch should be rejected");
        assert_eq!(mismatch.code, imago_protocol::ErrorCode::PreconditionFailed);

        let bad_policy_payload = DeployCommandPayload {
            deploy_id: "deploy-1".to_string(),
            expected_current_release: "any".to_string(),
            restart_policy: "sometimes".to_string(),
            auto_rollback: true,
        };
        let bad_policy = validate_deploy_preconditions(&bad_policy_payload, None)
            .expect_err("unsupported restart policy should be rejected");
        assert_eq!(bad_policy.code, imago_protocol::ErrorCode::BadRequest);
    }

    #[tokio::test]
    async fn extract_tar_rejects_path_traversal_entry() {
        let root = temp_dir_path("orchestrator-tar");
        let dest = root.join("dest");
        fs::create_dir_all(&dest).expect("destination should be created");

        let tar_path = root.join("artifact.tar");
        let tar_file = fs::File::create(&tar_path).expect("tar file should be created");
        let mut builder = Builder::new(tar_file);

        let payload = b"evil";
        let mut header = Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        {
            let bytes = header.as_mut_bytes();
            bytes[..100].fill(0);
            let name = b"../evil.txt";
            bytes[..name.len()].copy_from_slice(name);
        }
        header.set_cksum();
        builder
            .append(&header, &payload[..])
            .expect("malicious entry should be written");
        builder.finish().expect("tar should finish");

        let result = extract_tar(&tar_path, &dest).await;
        assert!(result.is_err());
        assert!(!dest.join("evil.txt").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn promote_staging_release_swaps_and_cleans_backup() {
        let root = temp_dir_path("orchestrator-promote-swap");
        let staging = root.join("staging");
        let release = root.join("release");
        fs::create_dir_all(&staging).expect("staging dir should be created");
        fs::create_dir_all(&release).expect("release dir should be created");
        fs::write(staging.join("new.txt"), b"new").expect("new file should be written");
        fs::write(release.join("old.txt"), b"old").expect("old file should be written");

        promote_staging_release(&staging, &release)
            .await
            .expect("release promotion should succeed");

        assert!(!staging.exists(), "staging should be moved");
        assert!(
            release.join("new.txt").exists(),
            "new release contents should exist"
        );
        assert!(
            !release.join("old.txt").exists(),
            "old release contents should be replaced"
        );

        let backups = fs::read_dir(&root)
            .expect("root should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".swap-backup-"))
            .collect::<Vec<_>>();
        assert!(backups.is_empty(), "backup dir should be cleaned up");

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn promote_staging_release_restores_previous_on_move_failure() {
        let root = temp_dir_path("orchestrator-promote-restore");
        let missing_staging = root.join("missing-staging");
        let release = root.join("release");
        fs::create_dir_all(&release).expect("release dir should be created");
        fs::write(release.join("active.txt"), b"active").expect("active file should be written");

        let err = promote_staging_release(&missing_staging, &release)
            .await
            .expect_err("promotion should fail when staging is missing");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            release.join("active.txt").exists(),
            "existing release should be restored after failure"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_binding_with_empty_target() {
        let root = temp_dir_path("orchestrator-binding-empty-target");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.bindings = vec![ManifestBinding {
            target: String::new(),
            wit: "yieldspace:service/invoke".to_string(),
        }];

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("empty binding target should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].target"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_keeps_manifest_app_type() {
        let root = temp_dir_path("orchestrator-app-type");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Http;
        manifest.http = Some(ManifestHttp {
            port: 18080,
            max_body_bytes: DEFAULT_HTTP_MAX_BODY_BYTES,
        });

        let launch = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        assert_eq!(launch.app_type, RunnerAppType::Http);
        assert_eq!(launch.http_port, Some(18080));
        assert_eq!(
            launch.http_max_body_bytes,
            Some(DEFAULT_HTTP_MAX_BODY_BYTES)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_caches_wasm_plugin_component_by_sha256() {
        let root = temp_dir_path("orchestrator-plugin-cache");
        fs::create_dir_all(root.join("plugins-src")).expect("plugins source dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("main component should exist");
        let plugin_bytes = b"plugin-wasm-bytes";
        fs::write(root.join("plugins-src/ffmpeg.wasm"), plugin_bytes)
            .expect("plugin component should exist");
        let plugin_sha = hex::encode(Sha256::digest(plugin_bytes));

        let mut manifest = valid_manifest();
        manifest.dependencies = vec![PluginDependency {
            name: "yieldspace:plugin/ffmpeg".to_string(),
            version: "1.0.0".to_string(),
            kind: PluginKind::Wasm,
            wit: "warg://yieldspace:plugin/ffmpeg@1.0.0".to_string(),
            requires: vec![],
            component: Some(PluginComponent {
                path: PathBuf::from("plugins-src/ffmpeg.wasm"),
                sha256: plugin_sha.clone(),
            }),
            capabilities: CapabilityPolicy::default(),
        }];

        let launch = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        let cached = launch
            .plugin_dependencies
            .first()
            .and_then(|dep| dep.component.as_ref())
            .map(|component| component.path.clone())
            .expect("cached plugin component path should exist");
        assert_eq!(
            cached,
            root.join("plugins/components")
                .join(format!("{plugin_sha}.wasm"))
        );
        assert!(cached.exists(), "cached component file must exist");

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_accepts_uppercase_plugin_component_sha256() {
        let root = temp_dir_path("orchestrator-plugin-cache-uppercase-sha");
        fs::create_dir_all(root.join("plugins-src")).expect("plugins source dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("main component should exist");
        let plugin_bytes = b"plugin-wasm-bytes-uppercase";
        fs::write(root.join("plugins-src/ffmpeg.wasm"), plugin_bytes)
            .expect("plugin component should exist");
        let plugin_sha_upper = hex::encode(Sha256::digest(plugin_bytes)).to_uppercase();

        let mut manifest = valid_manifest();
        manifest.dependencies = vec![PluginDependency {
            name: "yieldspace:plugin/ffmpeg".to_string(),
            version: "1.0.0".to_string(),
            kind: PluginKind::Wasm,
            wit: "warg://yieldspace:plugin/ffmpeg@1.0.0".to_string(),
            requires: vec![],
            component: Some(PluginComponent {
                path: PathBuf::from("plugins-src/ffmpeg.wasm"),
                sha256: plugin_sha_upper.clone(),
            }),
            capabilities: CapabilityPolicy::default(),
        }];

        let launch = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        let cached = launch
            .plugin_dependencies
            .first()
            .and_then(|dep| dep.component.as_ref())
            .map(|component| component.path.clone())
            .expect("cached plugin component path should exist");
        assert_eq!(
            cached,
            root.join("plugins/components")
                .join(format!("{plugin_sha_upper}.wasm"))
        );
        assert!(cached.exists(), "cached component file must exist");

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn gc_unused_plugin_components_removes_unreferenced_files() {
        let root = temp_dir_path("orchestrator-plugin-gc");
        let components_root = root.join("plugins/components");
        fs::create_dir_all(&components_root).expect("components root should exist");

        let keep_bytes = b"plugin-keep";
        let remove_bytes = b"plugin-remove";
        let keep_sha = hex::encode(Sha256::digest(keep_bytes));
        let remove_sha = hex::encode(Sha256::digest(remove_bytes));
        fs::write(components_root.join(format!("{keep_sha}.wasm")), keep_bytes)
            .expect("keep file should exist");
        fs::write(
            components_root.join(format!("{remove_sha}.wasm")),
            remove_bytes,
        )
        .expect("remove file should exist");

        let service_root = root.join("services").join("svc-a");
        let release_hash = "release-a";
        let release_root = service_root.join(release_hash);
        fs::create_dir_all(&release_root).expect("release root should exist");
        fs::write(service_root.join("active_release"), release_hash).expect("active release");
        fs::write(
            service_root.join(RESTART_POLICY_FILE_NAME),
            RESTART_POLICY_ALWAYS,
        )
        .expect("restart policy should exist");

        let mut manifest = valid_manifest();
        manifest.dependencies = vec![PluginDependency {
            name: "yieldspace:plugin/keep".to_string(),
            version: "1.0.0".to_string(),
            kind: PluginKind::Wasm,
            wit: "warg://yieldspace:plugin/keep@1.0.0".to_string(),
            requires: vec![],
            component: Some(PluginComponent {
                path: PathBuf::from("plugins/components/keep.wasm"),
                sha256: keep_sha.clone(),
            }),
            capabilities: CapabilityPolicy::default(),
        }];
        let manifest_bytes =
            serde_json::to_vec(&manifest).expect("manifest should serialize for gc test");
        fs::write(release_root.join("manifest.json"), manifest_bytes).expect("manifest write");

        gc_unused_plugin_components_on_boot(&root)
            .await
            .expect("gc should succeed");

        assert!(
            components_root.join(format!("{keep_sha}.wasm")).exists(),
            "referenced component must be preserved"
        );
        assert!(
            !components_root.join(format!("{remove_sha}.wasm")).exists(),
            "unreferenced component must be removed"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_http_type_without_http_section() {
        let root = temp_dir_path("orchestrator-http-missing");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Http;
        manifest.http = None;

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("type=http without manifest.http must fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("manifest.http is required"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_http_section_for_non_http_type() {
        let root = temp_dir_path("orchestrator-http-extra");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Cli;
        manifest.http = Some(ManifestHttp {
            port: 18080,
            max_body_bytes: DEFAULT_HTTP_MAX_BODY_BYTES,
        });

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("type=cli with manifest.http must fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("only allowed when type=\"http\""));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_socket_type_without_socket_section() {
        let root = temp_dir_path("orchestrator-socket-missing");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Socket;
        manifest.socket = None;

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("type=socket without manifest.socket must fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("manifest.socket is required"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_socket_section_for_non_socket_type() {
        let root = temp_dir_path("orchestrator-socket-extra");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Cli;
        manifest.socket = Some(RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Udp,
            direction: RunnerSocketDirection::Inbound,
            listen_addr: "0.0.0.0".to_string(),
            listen_port: 514,
        });

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("type=cli with manifest.socket must fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("only allowed when type=\"socket\""));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_invalid_socket_listen_addr() {
        let root = temp_dir_path("orchestrator-socket-addr-invalid");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.app_type = RunnerAppType::Socket;
        manifest.socket = Some(RunnerSocketConfig {
            protocol: RunnerSocketProtocol::Udp,
            direction: RunnerSocketDirection::Inbound,
            listen_addr: "not-an-ip".to_string(),
            listen_port: 514,
        });

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("invalid listen_addr should fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("listen_addr"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_parse_defaults_http_max_body_bytes_when_missing() {
        let manifest_json = r#"{
  "name": "svc-a",
  "main": "component.wasm",
  "type": "http",
  "http": {
    "port": 18080
  },
  "vars": {},
  "secrets": {},
  "assets": [],
  "bindings": [],
  "hash": {
    "algorithm": "sha256",
    "targets": ["wasm", "manifest", "assets"]
  }
}"#;

        let manifest = serde_json::from_str::<Manifest>(manifest_json)
            .expect("http manifest without max_body_bytes should parse");
        assert_eq!(
            manifest.http.map(|http| http.max_body_bytes),
            Some(8 * 1024 * 1024)
        );
    }

    #[tokio::test]
    async fn build_launch_rejects_http_max_body_bytes_out_of_range() {
        let root = temp_dir_path("orchestrator-http-max-body-range");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        for invalid in [0, MAX_HTTP_MAX_BODY_BYTES + 1] {
            let mut manifest = valid_manifest();
            manifest.app_type = RunnerAppType::Http;
            manifest.http = Some(ManifestHttp {
                port: 18080,
                max_body_bytes: invalid,
            });

            let err = build_launch_from_release(&root, "release-a", &root, &manifest)
                .await
                .expect_err("invalid max_body_bytes should fail");
            assert_eq!(err.code, ErrorCode::BadManifest);
            assert!(err.message.contains("max_body_bytes"));
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_parse_rejects_unknown_type_variant() {
        let manifest_json = r#"{
  "name": "svc-a",
  "main": "component.wasm",
  "type": "worker",
  "vars": {},
  "secrets": {},
  "assets": [],
  "bindings": [],
  "hash": {
    "algorithm": "sha256",
    "targets": ["wasm", "manifest", "assets"]
  }
}"#;

        let err = serde_json::from_str::<Manifest>(manifest_json)
            .expect_err("unknown type should be rejected");
        assert!(
            err.to_string().contains("unknown variant"),
            "unexpected parse error: {err}"
        );
    }

    #[tokio::test]
    async fn build_launch_rejects_binding_with_empty_wit() {
        let root = temp_dir_path("orchestrator-binding-empty-wit");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.bindings = vec![ManifestBinding {
            target: "svc-b".to_string(),
            wit: String::new(),
        }];

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("empty binding wit should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].wit"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_binding_with_invalid_target_name() {
        let root = temp_dir_path("orchestrator-binding-invalid-target");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.bindings = vec![ManifestBinding {
            target: "svc/invalid".to_string(),
            wit: "yieldspace:service/invoke".to_string(),
        }];

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("invalid binding target should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].target is invalid"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_boot_restore_candidates_returns_empty_when_services_dir_missing() {
        let root = temp_dir_path("orchestrator-restore-missing-services");
        fs::create_dir_all(&root).expect("storage root should exist");

        let (candidates, failed) = collect_boot_restore_candidates(&root)
            .await
            .expect("missing services dir should not fail");
        assert!(candidates.is_empty());
        assert!(failed.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_boot_restore_candidates_sorts_by_service_name() {
        let root = temp_dir_path("orchestrator-restore-sorted");
        let services = root.join("services");
        fs::create_dir_all(&services).expect("services root should exist");

        let svc_b = services.join("svc-b");
        fs::create_dir_all(&svc_b).expect("svc-b should exist");
        fs::write(svc_b.join("restart_policy"), b"always")
            .expect("svc-b restart policy should exist");
        fs::write(svc_b.join("active_release"), b"release-b\n")
            .expect("svc-b active_release should exist");

        let svc_a = services.join("svc-a");
        fs::create_dir_all(&svc_a).expect("svc-a should exist");
        fs::write(svc_a.join("restart_policy"), b"always")
            .expect("svc-a restart policy should exist");
        fs::write(svc_a.join("active_release"), b"release-a")
            .expect("svc-a active_release should exist");

        let svc_skip = services.join("svc-skip");
        fs::create_dir_all(&svc_skip).expect("svc-skip should exist");

        let (candidates, failed) = collect_boot_restore_candidates(&root)
            .await
            .expect("candidate collection should succeed");

        assert!(
            failed.is_empty(),
            "no failure should be reported for valid candidates"
        );
        let names = candidates
            .iter()
            .map(|candidate| candidate.service_name.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["svc-a".to_string(), "svc-b".to_string()]);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_boot_restore_candidates_reports_empty_active_release_as_failure() {
        let root = temp_dir_path("orchestrator-restore-empty-active");
        let service_dir = root.join("services").join("svc-empty");
        fs::create_dir_all(&service_dir).expect("service dir should exist");
        fs::write(service_dir.join("restart_policy"), b"always")
            .expect("restart policy should exist");
        fs::write(service_dir.join("active_release"), b"\n").expect("active_release should exist");

        let (candidates, failed) = collect_boot_restore_candidates(&root)
            .await
            .expect("candidate collection should succeed");

        assert!(candidates.is_empty());
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].service_name, "svc-empty");
        assert_eq!(failed[0].error.code, ErrorCode::BadManifest);
        assert!(failed[0].error.message.contains("active_release"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_boot_restore_candidates_skips_non_always_restart_policy() {
        let root = temp_dir_path("orchestrator-restore-policy-filter");
        let service_dir = root.join("services").join("svc-never");
        fs::create_dir_all(&service_dir).expect("service dir should exist");
        fs::write(service_dir.join("restart_policy"), b"never")
            .expect("restart policy should exist");
        fs::write(service_dir.join("active_release"), b"release-never")
            .expect("active release should exist");

        let (candidates, failed) = collect_boot_restore_candidates(&root)
            .await
            .expect("candidate collection should succeed");

        assert!(failed.is_empty());
        assert!(candidates.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_boot_restore_candidates_reports_invalid_restart_policy_as_failure() {
        let root = temp_dir_path("orchestrator-restore-policy-invalid");
        let service_dir = root.join("services").join("svc-invalid");
        fs::create_dir_all(&service_dir).expect("service dir should exist");
        fs::write(service_dir.join("restart_policy"), b"sometimes")
            .expect("restart policy should exist");
        fs::write(service_dir.join("active_release"), b"release-invalid")
            .expect("active release should exist");

        let (candidates, failed) = collect_boot_restore_candidates(&root)
            .await
            .expect("candidate collection should succeed");

        assert!(candidates.is_empty());
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].service_name, "svc-invalid");
        assert_eq!(failed[0].error.code, ErrorCode::Internal);
        assert!(failed[0].error.message.contains("invalid restart policy"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classify_boot_restore_entry_keeps_processing_after_file_type_error() {
        let mut service_entries = Vec::new();
        let mut failed = Vec::new();

        classify_boot_restore_entry(
            "svc-bad".to_string(),
            PathBuf::from("/tmp/svc-bad"),
            Err(std::io::Error::other("raced entry")),
            &mut service_entries,
            &mut failed,
        );

        let good_root = temp_dir_path("orchestrator-restore-classify-good");
        fs::create_dir_all(&good_root).expect("good service dir should exist");
        let good_file_type = fs::metadata(&good_root)
            .expect("metadata should be readable")
            .file_type();
        classify_boot_restore_entry(
            "svc-good".to_string(),
            good_root.clone(),
            Ok(good_file_type),
            &mut service_entries,
            &mut failed,
        );

        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].service_name, "svc-bad");
        assert!(failed[0].error.message.contains("svc-bad"));
        assert_eq!(service_entries.len(), 1);
        assert_eq!(service_entries[0].0, "svc-good");

        let _ = fs::remove_dir_all(good_root);
    }
}
