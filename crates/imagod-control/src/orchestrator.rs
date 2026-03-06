//! High-level orchestration for deploy/run/stop commands.
//!
//! Contract highlights:
//! - verifies artifact/manifest integrity before release promotion
//! - persists active release and restart policy state
//! - builds launch metadata consumed by `ServiceSupervisor`
//! - coordinates best-effort boot restore for eligible services

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};

use imago_protocol::{
    DeployCommandPayload, ErrorCode, RunCommandPayload, ServiceState, ServiceStatusEntry,
    StopCommandPayload,
};
use imagod_common::ImagodError;
use imagod_ipc::{
    CapabilityPolicy, PluginDependency, ResourceMap, RunnerAppType, RunnerSocketConfig,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use uuid::Uuid;

use crate::{
    artifact_store::{ArtifactStore, CommittedArtifact},
    service_supervisor::{
        RunningStatus, RuntimeServiceState, ServiceLaunch, ServiceLogSubscription,
        ServiceSupervisor,
    },
};

use self::{
    manifest::{DefaultManifestValidator, ManifestValidator},
    plugin_cache::FilesystemPluginCache,
};

mod launch_builder;
mod manifest;
mod plugin_cache;

const STAGE_ORCHESTRATE: &str = "orchestration";
const EXPECTED_CURRENT_RELEASE_ANY: &str = "any";
const RESTART_POLICY_NEVER: &str = "never";
const RESTART_POLICY_ON_FAILURE: &str = "on-failure";
const RESTART_POLICY_ALWAYS: &str = "always";
const RESTART_POLICY_UNLESS_STOPPED: &str = "unless-stopped";
const RESTART_POLICY_FILE_NAME: &str = "restart_policy";
const DEFAULT_HTTP_MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HTTP_MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;
const STOPPED_SERVICE_STARTED_AT: &str = "";

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
    resources: Option<ManifestResourcesConfig>,
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
    name: String,
    wit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest HTTP execution settings.
struct ManifestHttp {
    port: u16,
    #[serde(default = "default_http_max_body_bytes")]
    max_body_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
/// Manifest resource policy settings.
struct ManifestResourcesConfig {
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    http_outbound: Vec<String>,
    #[serde(default)]
    mounts: Vec<ManifestWasiMount>,
    #[serde(default)]
    read_only_mounts: Vec<ManifestWasiMount>,
    #[serde(flatten, default)]
    extra: ResourceMap,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// One WASI mount declaration from manifest.
struct ManifestWasiMount {
    asset_dir: String,
    guest_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
/// Manifest hash metadata describing required verification targets.
struct ManifestHash {
    algorithm: String,
    targets: Vec<HashTarget>,
}

impl ManifestHash {
    fn validate_targets(&self) -> bool {
        manifest::required_hash_targets_valid(&self.targets)
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
    manifest_validator: DefaultManifestValidator,
    plugin_cache: FilesystemPluginCache,
    command_gate: ServiceCommandGate,
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

/// Staged deploy context after artifact extraction and manifest verification.
struct StagedRelease {
    service_name: String,
    staging_dir: PathBuf,
    manifest: Manifest,
    artifact_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RollbackOutcome {
    SkippedNoPreviousRelease,
    RestoredPreviousRelease,
    RecoveredByForceStop,
}

#[derive(Clone, Default)]
struct ServiceCommandGate {
    inflight_services: Arc<StdMutex<BTreeSet<String>>>,
}

impl ServiceCommandGate {
    fn acquire(&self, service_name: &str) -> Result<ServiceCommandGuard, ImagodError> {
        let mut inflight_services = match self.inflight_services.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if inflight_services.contains(service_name) {
            return Err(service_command_busy_error(service_name));
        }
        inflight_services.insert(service_name.to_string());
        Ok(ServiceCommandGuard {
            service_name: service_name.to_string(),
            inflight_services: self.inflight_services.clone(),
        })
    }
}

#[derive(Debug)]
struct ServiceCommandGuard {
    service_name: String,
    inflight_services: Arc<StdMutex<BTreeSet<String>>>,
}

impl Drop for ServiceCommandGuard {
    fn drop(&mut self) {
        let mut inflight_services = match self.inflight_services.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        inflight_services.remove(&self.service_name);
    }
}

impl Orchestrator {
    /// Creates an orchestrator with shared storage and supervisor handles.
    pub fn new(
        storage_root: impl AsRef<Path>,
        artifact_store: ArtifactStore,
        supervisor: ServiceSupervisor,
    ) -> Self {
        let storage_root = storage_root.as_ref().to_path_buf();
        Self {
            storage_root: storage_root.clone(),
            artifact_store,
            supervisor,
            manifest_validator: DefaultManifestValidator,
            plugin_cache: FilesystemPluginCache::new(storage_root),
            command_gate: ServiceCommandGate::default(),
        }
    }

    /// Handles deploy orchestration and service replacement.
    pub async fn deploy(
        &self,
        payload: &DeployCommandPayload,
    ) -> Result<DeploySummary, ImagodError> {
        let (mut service_name, mut service_command_guard, staged) = {
            let _deploy_pin_guard = self.artifact_store.pin_deploy_session(&payload.deploy_id);
            let service_name = self
                .artifact_store
                .service_name_for_deploy(&payload.deploy_id)
                .await?;
            let service_command_guard = Some(self.command_gate.acquire(&service_name)?);
            let staged = self.prepare_release_staging(payload).await?;
            (service_name, service_command_guard, staged)
        };
        rebind_deploy_service_command_guard(
            &self.command_gate,
            &mut service_name,
            &mut service_command_guard,
            &staged.service_name,
        )?;
        let prepared = self.prepare_release_from_staged(payload, staged).await?;

        let launch = prepared.launch.clone();
        if let Err(start_error) = self.supervisor.replace(launch).await {
            if payload.auto_rollback {
                match self.rollback_previous_release(&prepared).await {
                    Ok(rollback_outcome) => {
                        return Err(append_rollback_success_message(
                            start_error,
                            rollback_outcome,
                        ));
                    }
                    Err(rollback_error) => {
                        return Err(compose_rollback_failure_error(
                            &start_error,
                            &rollback_error,
                        ));
                    }
                }
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
        let _service_command_guard = self.command_gate.acquire(&payload.name)?;

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
        self.plugin_cache
            .gc_unused_plugin_components_on_boot(&self.manifest_validator)
            .await
    }

    /// Stops a running service.
    pub async fn stop(&self, payload: &StopCommandPayload) -> Result<StopSummary, ImagodError> {
        let _service_command_guard = self.command_gate.acquire(&payload.name)?;

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

    /// Returns deployed service states merged with runtime snapshots.
    pub async fn list_service_states(
        &self,
        names_filter: Option<&[String]>,
    ) -> Result<Vec<ServiceStatusEntry>, ImagodError> {
        let names_filter = names_filter.map(|names| names.iter().cloned().collect::<BTreeSet<_>>());
        let deployed_releases =
            collect_deployed_service_releases(&self.storage_root, names_filter.as_ref()).await?;
        let runtime_states = self
            .supervisor
            .runtime_service_states()
            .await
            .into_iter()
            .filter(|state| {
                if let Some(names_filter) = names_filter.as_ref() {
                    names_filter.contains(&state.name)
                } else {
                    true
                }
            })
            .collect();
        Ok(merge_deployed_and_runtime_service_states(
            deployed_releases,
            runtime_states,
        ))
    }

    /// Returns names of services that have log snapshots (running + retained).
    pub async fn loggable_service_names(&self) -> Vec<String> {
        self.supervisor.loggable_service_names().await
    }

    /// Opens one service logs snapshot and optional follow stream.
    pub async fn open_logs(
        &self,
        service_name: &str,
        tail_lines: u32,
        follow: bool,
        with_timestamp: bool,
    ) -> Result<ServiceLogSubscription, ImagodError> {
        self.supervisor
            .open_logs(service_name, tail_lines, follow, with_timestamp)
            .await
    }

    /// Invokes one function on a running service runner.
    pub async fn invoke(
        &self,
        target_service_name: &str,
        interface_id: &str,
        function: &str,
        args_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, ImagodError> {
        self.supervisor
            .invoke(target_service_name, interface_id, function, args_cbor)
            .await
    }

    /// Prepares a validated release and launch spec from committed artifact data.
    async fn prepare_release_staging(
        &self,
        payload: &DeployCommandPayload,
    ) -> Result<StagedRelease, ImagodError> {
        let committed = self
            .artifact_store
            .committed_artifact(&payload.deploy_id)
            .await?;
        stage_committed_release(&self.storage_root, &self.manifest_validator, &committed).await
    }

    /// Finalizes release promotion under an acquired command gate.
    async fn prepare_release_from_staged(
        &self,
        payload: &DeployCommandPayload,
        staged: StagedRelease,
    ) -> Result<PreparedRelease, ImagodError> {
        prepare_release_from_staged_inner(
            &self.storage_root,
            payload,
            staged,
            &self.manifest_validator,
            &self.plugin_cache,
        )
        .await
    }

    /// Attempts to roll back active release marker when replacement start fails.
    async fn rollback_previous_release(
        &self,
        prepared: &PreparedRelease,
    ) -> Result<RollbackOutcome, ImagodError> {
        let Some(previous_release) = prepared.previous_release.as_deref() else {
            return Ok(RollbackOutcome::SkippedNoPreviousRelease);
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
        let rollback_outcome = start_previous_release_with_busy_recovery(
            || self.supervisor.start(previous_launch.clone()),
            || self.supervisor.stop(&prepared.service_name, true),
        )
        .await?;

        Ok(rollback_outcome)
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
        let manifest = self.manifest_validator.parse_manifest(&manifest_bytes)?;
        self.manifest_validator
            .validate_release_service_name(&manifest, service_name)?;

        launch_builder::build_launch_from_release(
            release_hash,
            &release_dir,
            &manifest,
            &self.manifest_validator,
            &self.plugin_cache,
        )
        .await
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

async fn stage_committed_release(
    storage_root: &Path,
    manifest_validator: &DefaultManifestValidator,
    committed: &CommittedArtifact,
) -> Result<StagedRelease, ImagodError> {
    let staging_dir = storage_root.join("staging").join(&committed.deploy_id);

    clean_dir(&staging_dir).await?;
    fs::create_dir_all(&staging_dir)
        .await
        .map_err(|e| map_internal(format!("failed to create staging dir: {e}")))?;

    extract_tar(&committed.path, &staging_dir).await?;

    let manifest_path = staging_dir.join("manifest.json");
    let manifest_bytes = fs::read(&manifest_path)
        .await
        .map_err(|e| map_bad_manifest(format!("manifest read failed: {e}")))?;
    let manifest = manifest_validator.parse_manifest(&manifest_bytes)?;
    manifest_validator.validate_manifest_metadata(
        &manifest,
        &manifest_bytes,
        Some(&committed.manifest_digest),
    )?;

    Ok(StagedRelease {
        service_name: manifest.name.clone(),
        staging_dir,
        manifest,
        artifact_digest: committed.artifact_digest.clone(),
    })
}

async fn prepare_release_from_staged_inner(
    storage_root: &Path,
    payload: &DeployCommandPayload,
    staged: StagedRelease,
    manifest_validator: &DefaultManifestValidator,
    plugin_cache: &FilesystemPluginCache,
) -> Result<PreparedRelease, ImagodError> {
    let StagedRelease {
        service_name,
        staging_dir,
        manifest,
        artifact_digest,
    } = staged;

    let release_hash = release_id_from_artifact_digest(&artifact_digest);
    let service_root = storage_root.join("services").join(&service_name);
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

    let launch = launch_builder::build_launch_from_release(
        &release_hash,
        &release_dir,
        &manifest,
        manifest_validator,
        plugin_cache,
    )
    .await?;

    Ok(PreparedRelease {
        service_name,
        service_root,
        release_hash,
        active_file,
        restart_policy_file,
        previous_release,
        previous_restart_policy,
        launch,
    })
}

/// Builds launch metadata for supervisor from a promoted release directory.
#[cfg(test)]
async fn build_launch_from_release(
    storage_root: &Path,
    release_hash: &str,
    release_dir: &Path,
    manifest: &Manifest,
) -> Result<ServiceLaunch, ImagodError> {
    let manifest_validator = DefaultManifestValidator;
    let plugin_cache = FilesystemPluginCache::new(storage_root.to_path_buf());
    launch_builder::build_launch_from_release(
        release_hash,
        release_dir,
        manifest,
        &manifest_validator,
        &plugin_cache,
    )
    .await
}

fn default_http_max_body_bytes() -> u64 {
    manifest::default_http_max_body_bytes()
}

#[cfg(test)]
async fn gc_unused_plugin_components_on_boot(storage_root: &Path) -> Result<(), ImagodError> {
    plugin_cache::gc_unused_plugin_components_on_boot_for_root(
        storage_root,
        &DefaultManifestValidator,
    )
    .await
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
    DefaultManifestValidator.normalize_archive_entry_path(path)
}

#[cfg(test)]
fn normalize_manifest_main_path(main: &str) -> Result<PathBuf, ImagodError> {
    DefaultManifestValidator.normalize_main_path(main)
}

#[cfg(test)]
fn validate_service_name(name: &str) -> Result<(), ImagodError> {
    DefaultManifestValidator.validate_service_name(name)
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

async fn collect_deployed_service_releases(
    storage_root: &Path,
    names_filter: Option<&BTreeSet<String>>,
) -> Result<BTreeMap<String, String>, ImagodError> {
    let services_root = storage_root.join("services");
    if let Some(names_filter) = names_filter {
        let mut deployed = BTreeMap::new();
        for service_name in names_filter {
            let active_file = services_root.join(service_name).join("active_release");
            let Some(active_release) = read_active_release(&active_file).await? else {
                continue;
            };
            if active_release.is_empty() {
                continue;
            }
            deployed.insert(service_name.clone(), active_release);
        }
        return Ok(deployed);
    }

    let mut entries = match fs::read_dir(&services_root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => {
            return Err(map_internal(format!(
                "failed to read services root {}: {err}",
                services_root.display()
            )));
        }
    };

    let mut deployed = BTreeMap::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|err| map_internal(format!("failed to iterate services root: {err}")))?
    {
        let file_type = entry.file_type().await.map_err(|err| {
            map_internal(format!(
                "failed to read service entry type {}: {err}",
                entry.path().display()
            ))
        })?;
        if !file_type.is_dir() {
            continue;
        }

        let service_name = entry.file_name().to_string_lossy().to_string();
        let active_file = entry.path().join("active_release");
        let Some(active_release) = read_active_release(&active_file).await? else {
            continue;
        };
        if active_release.is_empty() {
            continue;
        }

        deployed.insert(service_name, active_release);
    }

    Ok(deployed)
}

fn merge_deployed_and_runtime_service_states(
    deployed_releases: BTreeMap<String, String>,
    runtime_states: Vec<RuntimeServiceState>,
) -> Vec<ServiceStatusEntry> {
    let mut merged = deployed_releases
        .into_iter()
        .map(|(name, release_hash)| {
            let service = ServiceStatusEntry {
                name: name.clone(),
                release_hash,
                started_at: STOPPED_SERVICE_STARTED_AT.to_string(),
                state: ServiceState::Stopped,
            };
            (name, service)
        })
        .collect::<BTreeMap<_, _>>();

    for runtime_state in runtime_states {
        let RuntimeServiceState {
            name,
            release_hash,
            started_at,
            status,
        } = runtime_state;
        let service = merged
            .entry(name.clone())
            .or_insert_with(|| ServiceStatusEntry {
                name,
                release_hash: String::new(),
                started_at: STOPPED_SERVICE_STARTED_AT.to_string(),
                state: ServiceState::Stopped,
            });
        service.release_hash = release_hash;
        service.started_at = started_at;
        service.state = runtime_status_to_service_state(status);
    }

    merged.into_values().collect()
}

fn runtime_status_to_service_state(status: RunningStatus) -> ServiceState {
    match status {
        RunningStatus::Running => ServiceState::Running,
        RunningStatus::Stopping => ServiceState::Stopping,
    }
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

fn service_command_busy_error(service_name: &str) -> ImagodError {
    ImagodError::new(
        ErrorCode::Busy,
        STAGE_ORCHESTRATE,
        format!("service '{service_name}' command is already in progress"),
    )
}

fn rebind_deploy_service_command_guard(
    command_gate: &ServiceCommandGate,
    current_service_name: &mut String,
    current_guard: &mut Option<ServiceCommandGuard>,
    manifest_service_name: &str,
) -> Result<(), ImagodError> {
    if current_service_name == manifest_service_name {
        return Ok(());
    }

    let manifest_guard = command_gate.acquire(manifest_service_name)?;
    *current_guard = Some(manifest_guard);
    *current_service_name = manifest_service_name.to_string();
    Ok(())
}

fn map_rollback_error(err: ImagodError) -> ImagodError {
    let mut mapped = ImagodError::new(ErrorCode::RollbackFailed, STAGE_ORCHESTRATE, err.message)
        .with_retryable(err.retryable);
    for (key, value) in err.details {
        mapped = mapped.with_detail(key, value);
    }
    mapped
}

fn append_rollback_success_message(
    mut start_error: ImagodError,
    rollback_outcome: RollbackOutcome,
) -> ImagodError {
    let rollback_note = match rollback_outcome {
        RollbackOutcome::SkippedNoPreviousRelease => return start_error,
        RollbackOutcome::RestoredPreviousRelease => "rollback: restored previous release",
        RollbackOutcome::RecoveredByForceStop => {
            "rollback: recovered by force-stop and restored previous release"
        }
    };
    start_error.message = format!("{}; {}", start_error.message, rollback_note);
    start_error
}

fn compose_rollback_failure_error(
    start_error: &ImagodError,
    rollback_error: &ImagodError,
) -> ImagodError {
    let mut composed = ImagodError::new(
        ErrorCode::RollbackFailed,
        STAGE_ORCHESTRATE,
        format!(
            "{}; rollback failed: {}",
            start_error.message, rollback_error.message
        ),
    )
    .with_retryable(rollback_error.retryable);
    for (key, value) in &start_error.details {
        composed = composed.with_detail(key.clone(), value.clone());
    }
    for (key, value) in &rollback_error.details {
        if start_error.details.contains_key(key) {
            composed = composed.with_detail(format!("rollback.{key}"), value.clone());
        } else {
            composed = composed.with_detail(key.clone(), value.clone());
        }
    }
    composed
}

async fn start_previous_release_with_busy_recovery<StartFn, StartFut, StopFn, StopFut>(
    mut start_previous_release: StartFn,
    mut stop_current_service_force: StopFn,
) -> Result<RollbackOutcome, ImagodError>
where
    StartFn: FnMut() -> StartFut,
    StartFut: Future<Output = Result<(), ImagodError>>,
    StopFn: FnMut() -> StopFut,
    StopFut: Future<Output = Result<(), ImagodError>>,
{
    match start_previous_release().await {
        Ok(()) => Ok(RollbackOutcome::RestoredPreviousRelease),
        Err(start_error) if start_error.code == ErrorCode::Busy => {
            match stop_current_service_force().await {
                Ok(()) => {}
                Err(stop_error) if stop_error.code == ErrorCode::NotFound => {}
                Err(stop_error) => return Err(map_rollback_error(stop_error)),
            }
            start_previous_release().await.map_err(map_rollback_error)?;
            Ok(RollbackOutcome::RecoveredByForceStop)
        }
        Err(start_error) => Err(map_rollback_error(start_error)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_HTTP_MAX_BODY_BYTES, HashTarget, MAX_HTTP_MAX_BODY_BYTES, Manifest, ManifestAsset,
        ManifestBinding, ManifestHash, ManifestHttp, ManifestResourcesConfig, ManifestWasiMount,
        RESTART_POLICY_ALWAYS, RESTART_POLICY_FILE_NAME, RollbackOutcome, RunnerAppType,
        RunningStatus, RuntimeServiceState, STOPPED_SERVICE_STARTED_AT, ServiceCommandGate,
        append_rollback_success_message, build_launch_from_release, classify_boot_restore_entry,
        collect_boot_restore_candidates, collect_deployed_service_releases,
        compose_rollback_failure_error, extract_tar, gc_unused_plugin_components_on_boot,
        merge_deployed_and_runtime_service_states, normalize_archive_entry_path,
        normalize_manifest_main_path, prepare_release_from_staged_inner, promote_staging_release,
        rebind_deploy_service_command_guard, release_id_from_artifact_digest,
        stage_committed_release, start_previous_release_with_busy_recovery,
        validate_deploy_preconditions, validate_service_name,
    };
    use crate::artifact_store::CommittedArtifact;
    use imago_protocol::{DeployCommandPayload, ErrorCode, ServiceState, ServiceStatusEntry};
    use imagod_common::ImagodError;
    use imagod_ipc::{
        CapabilityPolicy, PluginComponent, PluginDependency, PluginKind, RunnerSocketConfig,
        RunnerSocketDirection, RunnerSocketProtocol, RunnerWasiMount, WasiHttpOutboundRule,
    };
    use sha2::{Digest, Sha256};
    use std::{
        collections::{BTreeMap, BTreeSet, VecDeque},
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
            resources: None,
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

    fn append_tar_file(builder: &mut Builder<fs::File>, name: &str, bytes: &[u8]) {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, bytes)
            .expect("tar entry should be appended");
    }

    fn minimal_component_bytes() -> Vec<u8> {
        wat::parse_str("(component)").expect("minimal component should compile")
    }

    fn committed_artifact_for_manifest(
        root: &Path,
        deploy_id: &str,
        manifest: &Manifest,
        component_bytes: &[u8],
    ) -> CommittedArtifact {
        let artifact_path = root.join(format!("{deploy_id}.artifact"));
        let artifact_file = fs::File::create(&artifact_path).expect("artifact file should exist");
        let mut builder = Builder::new(artifact_file);

        let manifest_bytes =
            serde_json::to_vec(manifest).expect("manifest should serialize to JSON");
        append_tar_file(&mut builder, "manifest.json", &manifest_bytes);
        append_tar_file(&mut builder, &manifest.main, component_bytes);
        builder.finish().expect("artifact tar should finish");

        CommittedArtifact {
            deploy_id: deploy_id.to_string(),
            path: artifact_path,
            manifest_digest: hex::encode(Sha256::digest(&manifest_bytes)),
            artifact_digest: hex::encode(Sha256::digest(component_bytes)),
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

    #[test]
    fn command_gate_rejects_duplicate_service_with_busy() {
        let gate = ServiceCommandGate::default();
        let _guard = gate
            .acquire("svc-a")
            .expect("first service command should be accepted");

        let err = gate
            .acquire("svc-a")
            .expect_err("same service command should be rejected while inflight");
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(err.stage, "orchestration");
        assert_eq!(
            err.message,
            "service 'svc-a' command is already in progress"
        );
    }

    #[test]
    fn command_gate_allows_reacquire_after_guard_drop() {
        let gate = ServiceCommandGate::default();
        {
            let _guard = gate
                .acquire("svc-a")
                .expect("first service command should be accepted");
        }

        let _guard = gate
            .acquire("svc-a")
            .expect("service should be available again after previous command is dropped");
    }

    #[test]
    fn command_gate_allows_different_services_in_parallel() {
        let gate = ServiceCommandGate::default();
        let _first = gate
            .acquire("svc-a")
            .expect("first service command should be accepted");
        let _second = gate
            .acquire("svc-b")
            .expect("different service command should be accepted");
    }

    #[test]
    fn rebind_deploy_gate_is_noop_when_service_name_matches() {
        let gate = ServiceCommandGate::default();
        let mut service_name = "svc-a".to_string();
        let mut guard = Some(
            gate.acquire("svc-a")
                .expect("initial service command should be accepted"),
        );

        rebind_deploy_service_command_guard(&gate, &mut service_name, &mut guard, "svc-a")
            .expect("same service name should not require rebind");
        assert_eq!(service_name, "svc-a");

        let err = gate
            .acquire("svc-a")
            .expect_err("current service should remain locked");
        assert_eq!(err.code, ErrorCode::Busy);
    }

    #[test]
    fn rebind_deploy_gate_switches_to_manifest_service_name() {
        let gate = ServiceCommandGate::default();
        let mut service_name = "svc-prepare".to_string();
        let mut guard = Some(
            gate.acquire("svc-prepare")
                .expect("prepare service should be locked initially"),
        );

        rebind_deploy_service_command_guard(&gate, &mut service_name, &mut guard, "svc-manifest")
            .expect("manifest service should be rebound when available");
        assert_eq!(service_name, "svc-manifest");

        let _prepare_guard = gate
            .acquire("svc-prepare")
            .expect("prepare service lock should be released after rebind");
        let err = gate
            .acquire("svc-manifest")
            .expect_err("manifest service should stay locked after rebind");
        assert_eq!(err.code, ErrorCode::Busy);
    }

    #[test]
    fn rebind_deploy_gate_returns_busy_when_manifest_service_is_inflight() {
        let gate = ServiceCommandGate::default();
        let _manifest_inflight = gate
            .acquire("svc-manifest")
            .expect("manifest service should be held by another command");
        let mut service_name = "svc-prepare".to_string();
        let mut guard = Some(
            gate.acquire("svc-prepare")
                .expect("prepare service should be locked initially"),
        );

        let err = rebind_deploy_service_command_guard(
            &gate,
            &mut service_name,
            &mut guard,
            "svc-manifest",
        )
        .expect_err("rebind should fail when manifest service is already in-flight");
        assert_eq!(err.code, ErrorCode::Busy);
        assert_eq!(err.stage, "orchestration");
        assert_eq!(
            err.message,
            "service 'svc-manifest' command is already in progress"
        );

        let prepare_err = gate
            .acquire("svc-prepare")
            .expect_err("prepare lock should remain held when rebind fails");
        assert_eq!(prepare_err.code, ErrorCode::Busy);
    }

    #[tokio::test]
    async fn stage_committed_release_does_not_write_services_tree() {
        let root = temp_dir_path("orchestrator-stage-committed");
        fs::create_dir_all(&root).expect("root should exist");

        let mut manifest = valid_manifest();
        manifest.name = "svc-staged".to_string();
        let committed =
            committed_artifact_for_manifest(&root, "deploy-staged", &manifest, b"wasm-binary");

        let staged = stage_committed_release(&root, &super::DefaultManifestValidator, &committed)
            .await
            .expect("staging should succeed");

        assert_eq!(staged.service_name, "svc-staged");
        assert!(
            !root.join("services").exists(),
            "staging phase must not touch services tree"
        );
        assert!(
            staged.staging_dir.join("manifest.json").exists(),
            "manifest should be extracted into staging"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn prepare_release_from_staged_writes_services_tree_in_second_phase() {
        let root = temp_dir_path("orchestrator-finalize-from-staged");
        fs::create_dir_all(&root).expect("root should exist");

        let mut manifest = valid_manifest();
        manifest.name = "svc-finalize".to_string();
        let committed = committed_artifact_for_manifest(
            &root,
            "deploy-finalize",
            &manifest,
            b"wasm-binary-finalize",
        );
        let staged = stage_committed_release(&root, &super::DefaultManifestValidator, &committed)
            .await
            .expect("staging should succeed");
        let service_root = root.join("services").join("svc-finalize");
        assert!(
            !service_root.exists(),
            "services tree should still be untouched before finalize phase"
        );

        let payload = DeployCommandPayload {
            deploy_id: "deploy-finalize".to_string(),
            expected_current_release: "any".to_string(),
            restart_policy: "never".to_string(),
            auto_rollback: true,
        };
        let plugin_cache = super::FilesystemPluginCache::new(root.clone());
        let prepared = prepare_release_from_staged_inner(
            &root,
            &payload,
            staged,
            &super::DefaultManifestValidator,
            &plugin_cache,
        )
        .await
        .expect("finalize should promote release into services tree");

        assert_eq!(prepared.service_name, "svc-finalize");
        assert!(
            service_root.join(&prepared.release_hash).exists(),
            "finalize phase should create promoted release directory"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn rollback_busy_uses_force_stop_and_retries_start_once() {
        let mut start_results = VecDeque::from(vec![
            Err(ImagodError::new(
                ErrorCode::Busy,
                "service.start",
                "service 'svc-a' is already running",
            )),
            Ok(()),
        ]);
        let mut stop_results = VecDeque::from(vec![Ok(())]);
        let mut start_calls = 0usize;
        let mut stop_calls = 0usize;

        let outcome = start_previous_release_with_busy_recovery(
            || {
                start_calls = start_calls.saturating_add(1);
                let result = start_results
                    .pop_front()
                    .expect("start result should exist for each call");
                async move { result }
            },
            || {
                stop_calls = stop_calls.saturating_add(1);
                let result = stop_results
                    .pop_front()
                    .expect("stop result should exist for each call");
                async move { result }
            },
        )
        .await
        .expect("busy rollback should recover via force stop");

        assert_eq!(outcome, RollbackOutcome::RecoveredByForceStop);
        assert_eq!(
            start_calls, 2,
            "start should be retried once after force stop"
        );
        assert_eq!(stop_calls, 1, "force stop should run exactly once");
    }

    #[tokio::test]
    async fn rollback_busy_continues_when_force_stop_reports_not_found() {
        let mut start_results = VecDeque::from(vec![
            Err(ImagodError::new(
                ErrorCode::Busy,
                "service.start",
                "service 'svc-a' is already running",
            )),
            Ok(()),
        ]);
        let mut stop_results = VecDeque::from(vec![Err(ImagodError::new(
            ErrorCode::NotFound,
            "service.stop",
            "service 'svc-a' is not running",
        ))]);
        let mut start_calls = 0usize;
        let mut stop_calls = 0usize;

        let outcome = start_previous_release_with_busy_recovery(
            || {
                start_calls = start_calls.saturating_add(1);
                let result = start_results
                    .pop_front()
                    .expect("start result should exist for each call");
                async move { result }
            },
            || {
                stop_calls = stop_calls.saturating_add(1);
                let result = stop_results
                    .pop_front()
                    .expect("stop result should exist for each call");
                async move { result }
            },
        )
        .await
        .expect("NotFound from force stop should be treated as already stopped");

        assert_eq!(outcome, RollbackOutcome::RecoveredByForceStop);
        assert_eq!(start_calls, 2, "start should still retry after NotFound");
        assert_eq!(stop_calls, 1, "force stop should run exactly once");
    }

    #[tokio::test]
    async fn rollback_busy_returns_rollback_failed_when_retry_start_also_fails() {
        let mut start_results = VecDeque::from(vec![
            Err(ImagodError::new(
                ErrorCode::Busy,
                "service.start",
                "service 'svc-a' is already running",
            )),
            Err(ImagodError::new(
                ErrorCode::Internal,
                "service.start",
                "runner crashed while starting previous release",
            )),
        ]);
        let mut stop_results = VecDeque::from(vec![Ok(())]);
        let mut start_calls = 0usize;
        let mut stop_calls = 0usize;

        let err = start_previous_release_with_busy_recovery(
            || {
                start_calls = start_calls.saturating_add(1);
                let result = start_results
                    .pop_front()
                    .expect("start result should exist for each call");
                async move { result }
            },
            || {
                stop_calls = stop_calls.saturating_add(1);
                let result = stop_results
                    .pop_front()
                    .expect("stop result should exist for each call");
                async move { result }
            },
        )
        .await
        .expect_err("retry start failure should surface rollback failure");

        assert_eq!(err.code, ErrorCode::RollbackFailed);
        assert_eq!(err.stage, "orchestration");
        assert_eq!(
            err.message,
            "runner crashed while starting previous release"
        );
        assert_eq!(start_calls, 2, "start should be attempted exactly twice");
        assert_eq!(stop_calls, 1, "force stop should run exactly once");
    }

    #[test]
    fn rollback_success_message_appends_expected_suffix() {
        let start_error = ImagodError::new(
            ErrorCode::Internal,
            "runtime.start",
            "wasmtime instantiation failed",
        )
        .with_retryable(true)
        .with_detail("component", "svc-a.wasm");
        let expected_code = start_error.code;
        let expected_stage = start_error.stage.clone();
        let expected_retryable = start_error.retryable;
        let expected_details = start_error.details.clone();

        let appended =
            append_rollback_success_message(start_error, RollbackOutcome::RestoredPreviousRelease);
        assert_eq!(
            appended.message,
            "wasmtime instantiation failed; rollback: restored previous release"
        );
        assert_eq!(appended.code, expected_code);
        assert_eq!(appended.stage, expected_stage);
        assert_eq!(appended.retryable, expected_retryable);
        assert_eq!(appended.details, expected_details);

        let busy_start_error = ImagodError::new(
            ErrorCode::Internal,
            "runtime.start",
            "wasmtime instantiation failed",
        )
        .with_retryable(true)
        .with_detail("component", "svc-a.wasm");
        let busy_recovered = append_rollback_success_message(
            busy_start_error,
            RollbackOutcome::RecoveredByForceStop,
        );
        assert_eq!(
            busy_recovered.message,
            "wasmtime instantiation failed; rollback: recovered by force-stop and restored previous release"
        );
    }

    #[test]
    fn rollback_failure_message_combines_start_and_rollback_errors() {
        let start_error = ImagodError::new(
            ErrorCode::Internal,
            "runtime.start",
            "wasmtime instantiation failed",
        )
        .with_detail("component", "svc-a.wasm")
        .with_detail("phase", "replace");
        let rollback_error = ImagodError::new(
            ErrorCode::RollbackFailed,
            "orchestration",
            "service 'svc-a' is already running",
        )
        .with_detail("phase", "rollback")
        .with_detail("action", "force-stop");

        let composed = compose_rollback_failure_error(&start_error, &rollback_error);
        assert_eq!(composed.code, ErrorCode::RollbackFailed);
        assert_eq!(composed.stage, "orchestration");
        assert_eq!(
            composed.message,
            "wasmtime instantiation failed; rollback failed: service 'svc-a' is already running"
        );
        assert_eq!(
            composed.details.get("component").map(String::as_str),
            Some("svc-a.wasm")
        );
        assert_eq!(
            composed.details.get("phase").map(String::as_str),
            Some("replace")
        );
        assert_eq!(
            composed.details.get("rollback.phase").map(String::as_str),
            Some("rollback")
        );
        assert_eq!(
            composed.details.get("action").map(String::as_str),
            Some("force-stop")
        );
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
    async fn build_launch_rejects_binding_with_empty_name() {
        let root = temp_dir_path("orchestrator-binding-empty-name");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.bindings = vec![ManifestBinding {
            name: String::new(),
            wit: "yieldspace:service/invoke".to_string(),
        }];

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("empty binding name should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].name"));

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
    async fn build_launch_applies_wasi_settings_and_mount_permissions() {
        let root = temp_dir_path("orchestrator-wasi-settings");
        fs::create_dir_all(root.join("assets/rw")).expect("rw assets dir should exist");
        fs::create_dir_all(root.join("assets/ro")).expect("ro assets dir should exist");
        fs::write(root.join("assets/rw/input.txt"), b"rw").expect("rw asset should exist");
        fs::write(root.join("assets/ro/input.txt"), b"ro").expect("ro asset should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.assets = vec![
            ManifestAsset {
                path: "assets/rw/input.txt".to_string(),
            },
            ManifestAsset {
                path: "assets/ro/input.txt".to_string(),
            },
        ];
        manifest.resources = Some(ManifestResourcesConfig {
            args: vec!["--serve".to_string()],
            env: BTreeMap::from([
                ("VAR_A".to_string(), "1".to_string()),
                ("SECRET_B".to_string(), "2".to_string()),
                ("WASI_ONLY".to_string(), "1".to_string()),
            ]),
            http_outbound: vec!["api.example.com:443".to_string()],
            mounts: vec![ManifestWasiMount {
                asset_dir: "assets/rw".to_string(),
                guest_path: "/guest/rw".to_string(),
            }],
            read_only_mounts: vec![ManifestWasiMount {
                asset_dir: "assets/ro".to_string(),
                guest_path: "/guest/ro".to_string(),
            }],
            extra: BTreeMap::from([(
                "custom".to_string(),
                serde_json::json!({ "allow": ["i2c"] }),
            )]),
        });

        let launch = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        assert_eq!(launch.args, vec!["--serve".to_string()]);
        assert_eq!(launch.envs.get("VAR_A"), Some(&"1".to_string()));
        assert_eq!(launch.envs.get("SECRET_B"), Some(&"2".to_string()));
        assert_eq!(launch.envs.get("WASI_ONLY"), Some(&"1".to_string()));
        assert_eq!(launch.wasi_http_outbound.len(), 4);
        assert_eq!(
            launch.wasi_mounts,
            vec![
                RunnerWasiMount {
                    host_path: root.join("assets/rw"),
                    guest_path: "/guest/rw".to_string(),
                    read_only: false,
                },
                RunnerWasiMount {
                    host_path: root.join("assets/ro"),
                    guest_path: "/guest/ro".to_string(),
                    read_only: true,
                }
            ]
        );
        assert_eq!(
            launch.resources.get("args"),
            Some(&serde_json::json!(["--serve"]))
        );
        assert_eq!(
            launch.resources.get("env"),
            Some(&serde_json::json!({
                "SECRET_B": "2",
                "VAR_A": "1",
                "WASI_ONLY": "1"
            }))
        );
        assert_eq!(
            launch.resources.get("custom"),
            Some(&serde_json::json!({ "allow": ["i2c"] }))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_injects_default_localhost_http_outbound_when_resources_is_missing() {
        let root = temp_dir_path("orchestrator-wasi-default-http-outbound");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let manifest = valid_manifest();
        let launch = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        assert_eq!(
            launch.wasi_http_outbound,
            vec![
                WasiHttpOutboundRule::Host {
                    host: "localhost".to_string()
                },
                WasiHttpOutboundRule::Host {
                    host: "127.0.0.1".to_string()
                },
                WasiHttpOutboundRule::Host {
                    host: "::1".to_string()
                }
            ]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_invalid_wasi_http_outbound_rule() {
        let root = temp_dir_path("orchestrator-wasi-invalid-http-outbound");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.resources = Some(ManifestResourcesConfig {
            args: Vec::new(),
            env: BTreeMap::new(),
            http_outbound: vec!["*.example.com".to_string()],
            mounts: Vec::new(),
            read_only_mounts: Vec::new(),
            extra: BTreeMap::new(),
        });

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("invalid rule should be rejected as bad manifest");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("manifest.resources.http_outbound[0]"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_rejects_duplicate_wasi_guest_path() {
        let root = temp_dir_path("orchestrator-wasi-duplicate-guest");
        fs::create_dir_all(root.join("assets/rw")).expect("rw assets dir should exist");
        fs::create_dir_all(root.join("assets/ro")).expect("ro assets dir should exist");
        fs::write(root.join("assets/rw/input.txt"), b"rw").expect("rw asset should exist");
        fs::write(root.join("assets/ro/input.txt"), b"ro").expect("ro asset should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.assets = vec![
            ManifestAsset {
                path: "assets/rw/input.txt".to_string(),
            },
            ManifestAsset {
                path: "assets/ro/input.txt".to_string(),
            },
        ];
        manifest.resources = Some(ManifestResourcesConfig {
            args: Vec::new(),
            env: BTreeMap::new(),
            http_outbound: Vec::new(),
            mounts: vec![ManifestWasiMount {
                asset_dir: "assets/rw".to_string(),
                guest_path: "/guest/shared".to_string(),
            }],
            read_only_mounts: vec![ManifestWasiMount {
                asset_dir: "assets/ro".to_string(),
                guest_path: "/guest/shared".to_string(),
            }],
            extra: BTreeMap::new(),
        });

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("duplicate guest path must be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("guest_path"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_caches_wasm_plugin_component_by_sha256() {
        let root = temp_dir_path("orchestrator-plugin-cache");
        fs::create_dir_all(root.join("plugins-src")).expect("plugins source dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("main component should exist");
        let plugin_bytes = minimal_component_bytes();
        fs::write(root.join("plugins-src/ffmpeg.wasm"), &plugin_bytes)
            .expect("plugin component should exist");
        let plugin_sha = hex::encode(Sha256::digest(&plugin_bytes));

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
                imports: None,
                exports: None,
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
        let component = launch.plugin_dependencies[0]
            .component
            .as_ref()
            .expect("plugin component should exist");
        assert!(
            component
                .imports
                .as_ref()
                .is_some_and(|imports| imports.is_empty()),
            "manager should embed component import metadata"
        );
        assert!(
            component
                .exports
                .as_ref()
                .is_some_and(|exports| exports.is_empty()),
            "manager should embed component export metadata"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn build_launch_accepts_uppercase_plugin_component_sha256() {
        let root = temp_dir_path("orchestrator-plugin-cache-uppercase-sha");
        fs::create_dir_all(root.join("plugins-src")).expect("plugins source dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("main component should exist");
        let plugin_bytes = minimal_component_bytes();
        fs::write(root.join("plugins-src/ffmpeg.wasm"), &plugin_bytes)
            .expect("plugin component should exist");
        let plugin_sha_upper = hex::encode(Sha256::digest(&plugin_bytes)).to_uppercase();

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
                imports: None,
                exports: None,
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
                imports: None,
                exports: None,
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
            Some(4 * 1024 * 1024)
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
            name: "svc-b".to_string(),
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
    async fn build_launch_rejects_binding_with_invalid_name() {
        let root = temp_dir_path("orchestrator-binding-invalid-name");
        fs::create_dir_all(&root).expect("release dir should exist");
        fs::write(root.join("component.wasm"), b"wasm").expect("component should exist");

        let mut manifest = valid_manifest();
        manifest.bindings = vec![ManifestBinding {
            name: "svc/invalid".to_string(),
            wit: "yieldspace:service/invoke".to_string(),
        }];

        let err = build_launch_from_release(&root, "release-a", &root, &manifest)
            .await
            .expect_err("invalid binding name should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].name is invalid"));

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

    #[test]
    fn merge_deployed_and_runtime_service_states_prefers_runtime_metadata() {
        let deployed = BTreeMap::from([
            ("svc-b".to_string(), "release-b".to_string()),
            ("svc-a".to_string(), "release-a".to_string()),
        ]);
        let runtime = vec![
            RuntimeServiceState {
                name: "svc-b".to_string(),
                release_hash: "release-b-runtime".to_string(),
                started_at: "100".to_string(),
                status: RunningStatus::Stopping,
            },
            RuntimeServiceState {
                name: "svc-undeployed".to_string(),
                release_hash: "release-x".to_string(),
                started_at: "200".to_string(),
                status: RunningStatus::Running,
            },
        ];

        let merged = merge_deployed_and_runtime_service_states(deployed, runtime);
        assert_eq!(
            merged,
            vec![
                ServiceStatusEntry {
                    name: "svc-a".to_string(),
                    release_hash: "release-a".to_string(),
                    started_at: STOPPED_SERVICE_STARTED_AT.to_string(),
                    state: ServiceState::Stopped,
                },
                ServiceStatusEntry {
                    name: "svc-b".to_string(),
                    release_hash: "release-b-runtime".to_string(),
                    started_at: "100".to_string(),
                    state: ServiceState::Stopping,
                },
                ServiceStatusEntry {
                    name: "svc-undeployed".to_string(),
                    release_hash: "release-x".to_string(),
                    started_at: "200".to_string(),
                    state: ServiceState::Running,
                },
            ]
        );
    }

    #[tokio::test]
    async fn collect_deployed_service_releases_scans_directories_when_filter_is_none() {
        let root = temp_dir_path("orchestrator-service-list");
        let services_root = root.join("services");
        fs::create_dir_all(&services_root).expect("services root should exist");

        let svc_a = services_root.join("svc-a");
        fs::create_dir_all(&svc_a).expect("svc-a dir should exist");
        fs::write(svc_a.join("active_release"), b"release-a\n")
            .expect("svc-a active_release should exist");

        let svc_b = services_root.join("svc-b");
        fs::create_dir_all(&svc_b).expect("svc-b dir should exist");
        fs::write(svc_b.join("active_release"), b"\n").expect("svc-b active_release should exist");

        fs::write(services_root.join("not-a-service"), b"skip")
            .expect("non-directory entry should exist");

        let releases = collect_deployed_service_releases(&root, None)
            .await
            .expect("collection should succeed");
        assert_eq!(
            releases,
            BTreeMap::from([("svc-a".to_string(), "release-a".to_string())])
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collect_deployed_service_releases_reads_only_filtered_names_and_ignores_unknown() {
        let root = temp_dir_path("orchestrator-service-list-filter");
        let services_root = root.join("services");
        fs::create_dir_all(&services_root).expect("services root should exist");

        let svc_a = services_root.join("svc-a");
        fs::create_dir_all(&svc_a).expect("svc-a dir should exist");
        fs::write(svc_a.join("active_release"), b"release-a\n")
            .expect("svc-a active_release should exist");

        let svc_b = services_root.join("svc-b");
        fs::create_dir_all(&svc_b).expect("svc-b dir should exist");
        fs::write(svc_b.join("active_release"), b"release-b\n")
            .expect("svc-b active_release should exist");

        let filter = BTreeSet::from(["svc-a".to_string(), "svc-unknown".to_string()]);
        let releases = collect_deployed_service_releases(&root, Some(&filter))
            .await
            .expect("filtered collection should succeed");
        assert_eq!(
            releases,
            BTreeMap::from([("svc-a".to_string(), "release-a".to_string())])
        );

        let unknown_only = BTreeSet::from(["svc-missing".to_string()]);
        let empty = collect_deployed_service_releases(&root, Some(&unknown_only))
            .await
            .expect("collection with unknown filter should succeed");
        assert!(empty.is_empty());

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
