use std::{
    collections::BTreeMap,
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use imago_protocol::{DeployCommandPayload, ErrorCode, RunCommandPayload, StopCommandPayload};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::{
    artifact_store::ArtifactStore,
    error::ImagodError,
    service_supervisor::{ServiceLaunch, ServiceSupervisor},
};

const STAGE_ORCHESTRATE: &str = "orchestration";

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Manifest {
    name: String,
    main: String,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    hash: ManifestHash,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ManifestAsset {
    path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
enum HashTarget {
    #[serde(rename = "wasm")]
    Wasm,
    #[serde(rename = "manifest")]
    Manifest,
    #[serde(rename = "assets")]
    Assets,
}

#[derive(Debug, Clone)]
pub struct DeploySummary {
    pub service_name: String,
    pub release_hash: String,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub service_name: String,
    pub release_hash: String,
}

#[derive(Debug, Clone)]
pub struct StopSummary {
    pub service_name: String,
}

#[derive(Clone)]
pub struct Orchestrator {
    storage_root: PathBuf,
    artifact_store: ArtifactStore,
    supervisor: ServiceSupervisor,
}

struct PreparedRelease {
    service_name: String,
    service_root: PathBuf,
    release_hash: String,
    active_file: PathBuf,
    previous_release: Option<String>,
    launch: ServiceLaunch,
}

impl Orchestrator {
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

    pub async fn stop(&self, payload: &StopCommandPayload) -> Result<StopSummary, ImagodError> {
        self.supervisor.stop(&payload.name, payload.force).await?;
        Ok(StopSummary {
            service_name: payload.name.clone(),
        })
    }

    pub async fn reap_finished_services(&self) {
        self.supervisor.reap_finished().await;
    }

    pub async fn has_live_services(&self) -> bool {
        self.supervisor.has_live_services().await
    }

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

        let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
        if manifest_digest != committed.manifest_digest {
            return Err(map_bad_manifest(
                "manifest digest does not match artifact metadata".to_string(),
            ));
        }

        let release_hash = short_hash(&committed.artifact_digest);
        let service_root = self.storage_root.join("services").join(&manifest.name);
        let release_dir = service_root.join(&release_hash);
        let active_file = service_root.join("active_release");
        let previous_release = read_active_release(&active_file).await?;

        fs::create_dir_all(&service_root)
            .await
            .map_err(|e| map_internal(format!("service root creation failed: {e}")))?;
        clean_dir(&release_dir).await?;
        fs::rename(&staging_dir, &release_dir)
            .await
            .map_err(|e| map_internal(format!("release move failed: {e}")))?;

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

async fn build_launch_from_release(
    release_hash: &str,
    release_dir: &Path,
    manifest: &Manifest,
) -> Result<ServiceLaunch, ImagodError> {
    let component_path = release_dir.join(&manifest.main);
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

    Ok(ServiceLaunch {
        name: manifest.name.clone(),
        release_hash: release_hash.to_string(),
        component_path,
        args: Vec::new(),
        envs,
    })
}

async fn extract_tar(bundle: &Path, dest: &Path) -> Result<(), ImagodError> {
    let bundle = bundle.to_path_buf();
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), ImagodError> {
        let file = std::fs::File::open(&bundle)
            .map_err(|e| map_bad_manifest(format!("artifact open failed: {e}")))?;
        let mut archive = tar::Archive::new(file);
        archive
            .unpack(&dest)
            .map_err(|e| map_bad_manifest(format!("artifact unpack failed: {e}")))?;
        Ok(())
    })
    .await
    .map_err(|e| map_internal(format!("artifact unpack task join failed: {e}")))?
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

fn short_hash(full: &str) -> String {
    full.chars().take(16).collect()
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
