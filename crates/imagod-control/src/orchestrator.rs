//! High-level orchestration for deploy/run/stop commands.

use std::{
    collections::BTreeMap,
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use imago_protocol::{DeployCommandPayload, ErrorCode, RunCommandPayload, StopCommandPayload};
use imagod_common::ImagodError;
use imagod_ipc::{RunnerAppType, ServiceBinding};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::fs;
use uuid::Uuid;

use crate::{
    artifact_store::ArtifactStore,
    service_supervisor::{ServiceLaunch, ServiceLogSubscription, ServiceSupervisor},
};

const STAGE_ORCHESTRATE: &str = "orchestration";
const EXPECTED_CURRENT_RELEASE_ANY: &str = "any";
const RESTART_POLICY_NEVER: &str = "never";

#[derive(Debug, Clone, Deserialize, PartialEq)]
/// Release manifest loaded from extracted artifact.
struct Manifest {
    name: String,
    main: String,
    #[serde(rename = "type")]
    app_type: RunnerAppType,
    #[serde(default)]
    http: Option<ManifestHttp>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    #[serde(default)]
    bindings: Vec<ManifestBinding>,
    hash: ManifestHash,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
/// Manifest-declared asset path.
struct ManifestAsset {
    path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
/// Manifest binding authorization entry.
struct ManifestBinding {
    target: String,
    wit: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
/// Manifest HTTP execution settings.
struct ManifestHttp {
    port: u16,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
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
    previous_release: Option<String>,
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
        let previous_release = read_active_release(&active_file).await?;
        validate_deploy_preconditions(payload, previous_release.as_deref())?;

        fs::create_dir_all(&service_root)
            .await
            .map_err(|e| map_internal(format!("service root creation failed: {e}")))?;
        promote_staging_release(&staging_dir, &release_dir).await?;

        cleanup_old_releases(&service_root, &release_hash, previous_release.as_deref()).await?;

        let launch = build_launch_from_release(&release_hash, &release_dir, &manifest).await?;

        Ok(PreparedRelease {
            service_name: manifest.name,
            service_root,
            release_hash,
            active_file,
            previous_release,
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

        build_launch_from_release(release_hash, &release_dir, &manifest).await
    }
}

/// Builds launch metadata for supervisor from a promoted release directory.
async fn build_launch_from_release(
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
    let http_port = validate_manifest_http(manifest)?;

    Ok(ServiceLaunch {
        name: manifest.name.clone(),
        release_hash: release_hash.to_string(),
        app_type: manifest.app_type,
        http_port,
        component_path,
        args: Vec::new(),
        envs,
        bindings,
    })
}

fn validate_manifest_http(manifest: &Manifest) -> Result<Option<u16>, ImagodError> {
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
            Ok(Some(http.port))
        }
        RunnerAppType::Cli | RunnerAppType::Socket => {
            if manifest.http.is_some() {
                return Err(map_bad_manifest(
                    "manifest.http is only allowed when type=\"http\"".to_string(),
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
    let path = Path::new(main);
    if main.is_empty() || path.as_os_str().is_empty() {
        return Err(map_bad_manifest(
            "manifest.main must not be empty".to_string(),
        ));
    }
    if path.is_absolute() {
        return Err(map_bad_manifest(format!(
            "manifest.main must be a relative path: {}",
            main
        )));
    }

    let raw = path.as_os_str().to_string_lossy();
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return Err(map_bad_manifest(format!(
            "manifest.main must not be windows-prefixed: {main}"
        )));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir | Component::RootDir => {
                return Err(map_bad_manifest(format!(
                    "manifest.main contains invalid path traversal: {main}"
                )));
            }
            _ => {
                return Err(map_bad_manifest(format!(
                    "manifest.main contains invalid path component: {main}"
                )));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(map_bad_manifest(format!(
            "manifest.main is invalid: {main}"
        )));
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
    if payload.restart_policy != RESTART_POLICY_NEVER {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_ORCHESTRATE,
            format!(
                "unsupported restart_policy '{}': only '{}' is supported",
                payload.restart_policy, RESTART_POLICY_NEVER
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
        HashTarget, Manifest, ManifestAsset, ManifestBinding, ManifestHash, ManifestHttp,
        RunnerAppType, build_launch_from_release, extract_tar, normalize_archive_entry_path,
        normalize_manifest_main_path, promote_staging_release, release_id_from_artifact_digest,
        validate_deploy_preconditions, validate_service_name,
    };
    use imago_protocol::{DeployCommandPayload, ErrorCode};
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
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            assets: Vec::<ManifestAsset>::new(),
            bindings: Vec::new(),
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
        let payload_any = DeployCommandPayload {
            deploy_id: "deploy-1".to_string(),
            expected_current_release: "any".to_string(),
            restart_policy: "never".to_string(),
            auto_rollback: true,
        };
        assert!(validate_deploy_preconditions(&payload_any, None).is_ok());

        let payload_match = DeployCommandPayload {
            deploy_id: "deploy-1".to_string(),
            expected_current_release: "release-abc".to_string(),
            restart_policy: "never".to_string(),
            auto_rollback: true,
        };
        assert!(validate_deploy_preconditions(&payload_match, Some("release-abc")).is_ok());
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
            restart_policy: "always".to_string(),
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

        let err = build_launch_from_release("release-a", &root, &manifest)
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
        manifest.http = Some(ManifestHttp { port: 18080 });

        let launch = build_launch_from_release("release-a", &root, &manifest)
            .await
            .expect("launch should be built");
        assert_eq!(launch.app_type, RunnerAppType::Http);
        assert_eq!(launch.http_port, Some(18080));

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

        let err = build_launch_from_release("release-a", &root, &manifest)
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
        manifest.http = Some(ManifestHttp { port: 18080 });

        let err = build_launch_from_release("release-a", &root, &manifest)
            .await
            .expect_err("type=cli with manifest.http must fail");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("only allowed when type=\"http\""));

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

        let err = build_launch_from_release("release-a", &root, &manifest)
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

        let err = build_launch_from_release("release-a", &root, &manifest)
            .await
            .expect_err("invalid binding target should be rejected");
        assert_eq!(err.code, ErrorCode::BadManifest);
        assert!(err.message.contains("bindings[0].target is invalid"));

        let _ = fs::remove_dir_all(root);
    }
}
