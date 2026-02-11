use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
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
const STARTUP_PROBE_TIMEOUT_SECS: u64 = 3;
const STARTUP_PROBE_POLL_INTERVAL_MS: u64 = 25;

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
    startup_probe_timeout: Duration,
    inner: Arc<RwLock<BTreeMap<String, RunningService>>>,
    stopping_count: Arc<AtomicUsize>,
}

impl ServiceSupervisor {
    pub fn new(runtime: WasmRuntime, stop_grace_timeout_secs: u64) -> Self {
        Self {
            runtime,
            stop_grace_timeout: Duration::from_secs(stop_grace_timeout_secs.max(1)),
            startup_probe_timeout: Duration::from_secs(STARTUP_PROBE_TIMEOUT_SECS),
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            stopping_count: Arc::new(AtomicUsize::new(0)),
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
        let service_name = launch.name.clone();

        let join_handle = tokio::spawn(async move {
            runtime
                .run_cli_component_async(&component_path, &args, &envs, shutdown_rx)
                .await
        });
        let abort_handle = join_handle.abort_handle();

        inner.insert(
            service_name.clone(),
            RunningService {
                release_hash: launch.release_hash,
                started_at: now_unix_secs(),
                status: RunningStatus::Running,
                shutdown_tx,
                abort_handle,
                join_handle,
            },
        );

        drop(inner);
        self.wait_for_startup_probe(&service_name).await?;

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
        let _stopping_guard = StoppingCounterGuard::new(self.stopping_count.clone());
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
        if self.stopping_count.load(Ordering::SeqCst) > 0 {
            return true;
        }

        let inner = self.inner.read().await;
        inner
            .values()
            .any(|service| !service.join_handle.is_finished())
    }

    async fn wait_for_startup_probe(&self, service_name: &str) -> Result<(), ImagodError> {
        let deadline = time::Instant::now() + self.startup_probe_timeout;

        loop {
            let running_state = {
                let inner = self.inner.read().await;
                inner
                    .get(service_name)
                    .map(|service| service.join_handle.is_finished())
            };

            match running_state {
                Some(true) => {
                    let service = {
                        let mut inner = self.inner.write().await;
                        inner.remove(service_name)
                    };
                    if let Some(service) = service {
                        let result = service.join_handle.await;
                        let startup_error = map_startup_probe_failure(service_name, &result);
                        log_join_outcome(
                            service_name,
                            &service.release_hash,
                            &service.started_at,
                            service.status,
                            result,
                        );
                        return Err(startup_error);
                    }

                    return Err(ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_START,
                        format!("service '{service_name}' disappeared during startup probe"),
                    ));
                }
                Some(false) => {}
                None => {
                    return Err(ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_START,
                        format!("service '{service_name}' disappeared during startup probe"),
                    ));
                }
            }

            let now = time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let sleep_for = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(STARTUP_PROBE_POLL_INTERVAL_MS));
            time::sleep(sleep_for).await;
        }
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

struct StoppingCounterGuard {
    counter: Arc<AtomicUsize>,
}

impl StoppingCounterGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for StoppingCounterGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

fn map_startup_probe_failure(
    service_name: &str,
    result: &Result<Result<(), ImagodError>, tokio::task::JoinError>,
) -> ImagodError {
    match result {
        Ok(Ok(())) => ImagodError::new(
            ErrorCode::Internal,
            STAGE_START,
            format!("service '{service_name}' exited during startup probe"),
        ),
        Ok(Err(err)) => ImagodError::new(
            ErrorCode::Internal,
            STAGE_START,
            format!("service '{service_name}' failed during startup: {err}"),
        ),
        Err(err) => ImagodError::new(
            ErrorCode::Internal,
            STAGE_START,
            format!("service '{service_name}' task join failed during startup: {err}"),
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_wasmtime::WasmRuntime;
    use tokio::sync::watch;

    #[tokio::test]
    async fn has_live_services_is_true_while_stop_waits_for_join() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let supervisor = ServiceSupervisor::new(runtime, 1);
        let service_name = "svc-stop-live".to_string();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let join_handle = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            time::sleep(Duration::from_millis(200)).await;
            Ok::<(), ImagodError>(())
        });
        let abort_handle = join_handle.abort_handle();

        {
            let mut inner = supervisor.inner.write().await;
            inner.insert(
                service_name.clone(),
                RunningService {
                    release_hash: "release-a".to_string(),
                    started_at: now_unix_secs(),
                    status: RunningStatus::Running,
                    shutdown_tx,
                    abort_handle,
                    join_handle,
                },
            );
        }

        let stop_supervisor = supervisor.clone();
        let service_name_for_stop = service_name.clone();
        let stop_task =
            tokio::spawn(async move { stop_supervisor.stop(&service_name_for_stop, false).await });

        time::sleep(Duration::from_millis(50)).await;
        assert!(supervisor.has_live_services().await);

        stop_task
            .await
            .expect("stop task should not panic")
            .expect("stop should succeed");
        assert!(!supervisor.has_live_services().await);
    }

    #[tokio::test]
    async fn start_returns_error_when_task_exits_during_startup_probe() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let supervisor = ServiceSupervisor::new(runtime, 1);

        let launch = ServiceLaunch {
            name: "svc-start-fail".to_string(),
            release_hash: "release-b".to_string(),
            component_path: std::env::temp_dir()
                .join(format!("missing-component-{}.wasm", now_unix_secs())),
            args: Vec::new(),
            envs: BTreeMap::new(),
        };

        let err = supervisor
            .start(launch)
            .await
            .expect_err("start should fail when component path is missing");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(!supervisor.has_live_services().await);
    }

    #[tokio::test]
    async fn stop_not_found_keeps_live_state_false() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let supervisor = ServiceSupervisor::new(runtime, 1);

        let err = supervisor
            .stop("missing-service", false)
            .await
            .expect_err("missing service should return not found");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(
            !supervisor.has_live_services().await,
            "stopping_count guard should be released on early NotFound"
        );
    }
}
