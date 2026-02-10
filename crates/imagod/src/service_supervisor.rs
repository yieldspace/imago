use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use imago_protocol::ErrorCode;
use tokio::{
    sync::{RwLock, watch},
    task::{AbortHandle, JoinHandle},
    time,
};

use crate::{error::ImagodError, runtime_wasmtime::WasmRuntime};

const STAGE_START: &str = "service.start";
const STAGE_STOP: &str = "service.stop";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLaunch {
    pub name: String,
    pub release_hash: String,
    pub component_path: PathBuf,
    pub args: Vec<String>,
    pub envs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningStatus {
    Running,
    Stopping,
}

#[derive(Debug)]
pub struct RunningService {
    pub release_hash: String,
    pub started_at: String,
    pub status: RunningStatus,
    shutdown_tx: watch::Sender<bool>,
    abort_handle: AbortHandle,
    join_handle: JoinHandle<Result<(), ImagodError>>,
}

#[derive(Clone)]
pub struct ServiceSupervisor {
    runtime: WasmRuntime,
    stop_grace_timeout: Duration,
    inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
}

impl ServiceSupervisor {
    pub fn new(runtime: WasmRuntime, stop_grace_timeout_secs: u64) -> Self {
        Self {
            runtime,
            stop_grace_timeout: Duration::from_secs(stop_grace_timeout_secs.max(1)),
            inner: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub async fn start(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        self.reap_finished_service(&launch.name).await;

        let mut inner = self.inner.write().await;
        if inner.contains_key(&launch.name) {
            return Err(ImagodError::new(
                ErrorCode::Busy,
                STAGE_START,
                format!("service '{}' is already running", launch.name),
            ));
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = self.runtime.clone();
        let component_path = launch.component_path.clone();
        let args = launch.args.clone();
        let envs = launch.envs.clone();

        let join_handle = tokio::spawn(async move {
            runtime
                .run_cli_component_async(&component_path, &args, &envs, shutdown_rx)
                .await
        });
        let abort_handle = join_handle.abort_handle();

        inner.insert(
            launch.name,
            RunningService {
                release_hash: launch.release_hash,
                started_at: now_unix_secs(),
                status: RunningStatus::Running,
                shutdown_tx,
                abort_handle,
                join_handle,
            },
        );

        Ok(())
    }

    pub async fn replace(&self, launch: ServiceLaunch) -> Result<(), ImagodError> {
        match self.stop(&launch.name, false).await {
            Ok(()) => {}
            Err(err) if err.code == ErrorCode::NotFound => {}
            Err(err) => return Err(err),
        }
        self.start(launch).await
    }

    pub async fn stop(&self, service_name: &str, force: bool) -> Result<(), ImagodError> {
        let mut service = self.take_running(service_name).await?;

        if service.join_handle.is_finished() {
            let result = service.join_handle.await;
            log_join_outcome(
                service_name,
                &service.release_hash,
                &service.started_at,
                service.status,
                result,
            );
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            ));
        }

        service.status = RunningStatus::Stopping;

        if force {
            service.abort_handle.abort();
            let result = service.join_handle.await;
            log_join_outcome(
                service_name,
                &service.release_hash,
                &service.started_at,
                service.status,
                result,
            );
            return Ok(());
        }

        let _ = service.shutdown_tx.send(true);
        match time::timeout(self.stop_grace_timeout, &mut service.join_handle).await {
            Ok(result) => {
                log_join_outcome(
                    service_name,
                    &service.release_hash,
                    &service.started_at,
                    service.status,
                    result,
                );
            }
            Err(_) => {
                service.abort_handle.abort();
                let result = service.join_handle.await;
                log_join_outcome(
                    service_name,
                    &service.release_hash,
                    &service.started_at,
                    service.status,
                    result,
                );
            }
        }

        Ok(())
    }

    pub async fn reap_finished(&self) {
        let finished_names = {
            let inner = self.inner.read().await;
            inner
                .iter()
                .filter_map(|(name, service)| {
                    if service.join_handle.is_finished() {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };

        for name in finished_names {
            let service = {
                let mut inner = self.inner.write().await;
                inner.remove(&name)
            };
            if let Some(service) = service {
                let result = service.join_handle.await;
                log_join_outcome(
                    &name,
                    &service.release_hash,
                    &service.started_at,
                    service.status,
                    result,
                );
            }
        }
    }

    pub async fn has_live_services(&self) -> bool {
        let inner = self.inner.read().await;
        inner
            .values()
            .any(|service| !service.join_handle.is_finished())
    }

    async fn reap_finished_service(&self, service_name: &str) {
        let should_reap = {
            let inner = self.inner.read().await;
            inner
                .get(service_name)
                .map(|service| service.join_handle.is_finished())
                .unwrap_or(false)
        };
        if !should_reap {
            return;
        }

        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        if let Some(service) = service {
            let result = service.join_handle.await;
            log_join_outcome(
                service_name,
                &service.release_hash,
                &service.started_at,
                service.status,
                result,
            );
        }
    }

    async fn take_running(&self, service_name: &str) -> Result<RunningService, ImagodError> {
        let service = {
            let mut inner = self.inner.write().await;
            inner.remove(service_name)
        };
        service.ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                STAGE_STOP,
                format!("service '{service_name}' is not running"),
            )
        })
    }
}

fn log_join_outcome(
    service_name: &str,
    release_hash: &str,
    started_at: &str,
    status: RunningStatus,
    result: Result<Result<(), ImagodError>, tokio::task::JoinError>,
) {
    match result {
        Ok(Ok(())) => eprintln!(
            "service stopped name={} release={} started_at={} state={:?} result=ok",
            service_name, release_hash, started_at, status
        ),
        Ok(Err(err)) => eprintln!(
            "service failed name={} release={} started_at={} state={:?} error={}",
            service_name, release_hash, started_at, status, err
        ),
        Err(err) => eprintln!(
            "service task join error name={} release={} started_at={} state={:?} error={}",
            service_name, release_hash, started_at, status, err
        ),
    }
}

fn now_unix_secs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}
