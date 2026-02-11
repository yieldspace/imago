//! `imagod` binary entrypoint.
//!
//! This binary runs in manager mode by default and switches to runner mode
//! when `--runner` is passed by the manager supervisor.

use std::{path::PathBuf, sync::Arc};

use imagod_config::{ImagodConfig, resolve_config_path};
use imagod_control::{ArtifactStore, OperationManager, Orchestrator, ServiceSupervisor};
use imagod_runtime::run_runner_from_stdin;
use imagod_server::{ProtocolHandler, build_server};
use web_transport_quinn::http::StatusCode;

const MAINTENANCE_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

#[tokio::main]
async fn main() {
    if let Err(err) = dispatch().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn dispatch() -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let cli = parse_cli_args()?;
    match cli.mode {
        RunMode::Runner => run_runner_from_stdin().await.map_err(anyhow::Error::new),
        RunMode::Manager => run_manager(cli.config_path).await,
    }
}

async fn run_manager(config_path: Option<PathBuf>) -> Result<(), anyhow::Error> {
    let config_path = resolve_config_path(config_path);
    let config = Arc::new(ImagodConfig::load(&config_path).map_err(anyhow::Error::new)?);

    let artifact_root = config.storage_root.join("artifacts");
    let artifacts = ArtifactStore::new(
        &artifact_root,
        config.runtime.upload_session_ttl_secs,
        config.runtime.chunk_size,
        config.runtime.max_inflight_chunks,
        config.runtime.max_artifact_size_bytes,
    )
    .await
    .map_err(anyhow::Error::new)?;
    let operations = OperationManager::new();
    let supervisor = ServiceSupervisor::new(
        &config.storage_root,
        config.runtime.stop_grace_timeout_secs,
        config.runtime.runner_ready_timeout_secs,
        config.runtime.runner_log_buffer_bytes,
        config.runtime.epoch_tick_interval_ms,
    )
    .map_err(anyhow::Error::new)?;
    let orchestrator = Orchestrator::new(&config.storage_root, artifacts.clone(), supervisor);

    let handler = ProtocolHandler::new(config.clone(), artifacts, operations, orchestrator);

    let maintenance_handler = handler.clone();
    let active_tick_interval =
        std::time::Duration::from_millis(config.runtime.epoch_tick_interval_ms.max(1));
    let idle_tick_interval = std::time::Duration::from_secs(1);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let maintenance_task = tokio::spawn(async move {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let mut reap_future = std::pin::pin!(maintenance_handler.reap_finished_services());
            tokio::select! {
                _ = &mut reap_future => {}
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
            }

            let mut has_live_services = std::pin::pin!(maintenance_handler.has_live_services());
            let has_live_services = tokio::select! {
                live = &mut has_live_services => live,
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
            };

            let sleep_duration = if has_live_services {
                active_tick_interval
            } else {
                idle_tick_interval
            };
            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    });

    let mut server = build_server(&config).map_err(anyhow::Error::new)?;
    eprintln!("imagod listening on {}", config.listen_addr);
    let mut shutdown_signal = std::pin::pin!(tokio::signal::ctrl_c());

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                eprintln!("shutdown signal received");
                break;
            }
            request = server.accept() => {
                let Some(request): Option<web_transport_quinn::Request> = request else {
                    break;
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let Ok(session) = request.respond(StatusCode::OK).await else {
                        return;
                    };
                    if let Err(err) = handler.handle_session(session).await {
                        eprintln!("session error: {err}");
                    }
                });
            }
        }
    }

    let stop_errors = handler.stop_all_services(false).await;
    for (service_name, err) in stop_errors {
        eprintln!(
            "service shutdown failed name={} code={:?} stage={} message={}",
            service_name, err.code, err.stage, err.message
        );
    }

    let _ = shutdown_tx.send(true);
    wait_for_maintenance_shutdown(maintenance_task).await?;

    Ok(())
}

async fn wait_for_maintenance_shutdown(
    maintenance_task: tokio::task::JoinHandle<()>,
) -> Result<(), anyhow::Error> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(MAINTENANCE_SHUTDOWN_TIMEOUT_SECS),
        maintenance_task,
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(anyhow::anyhow!("maintenance task join failed: {err}")),
        Err(_) => Err(anyhow::anyhow!(
            "maintenance task did not shut down within {} seconds",
            MAINTENANCE_SHUTDOWN_TIMEOUT_SECS
        )),
    }
}

fn install_rustls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return;
    }

    let provider = web_transport_quinn::crypto::default_provider();
    if let Some(provider) = std::sync::Arc::into_inner(provider) {
        let _ = provider.install_default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manager,
    Runner,
}

#[derive(Debug, Clone)]
struct CliArgs {
    config_path: Option<PathBuf>,
    mode: RunMode,
}

fn parse_cli_args() -> Result<CliArgs, anyhow::Error> {
    let mut args = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;
    let mut mode = RunMode::Manager;

    while let Some(arg) = args.next() {
        if arg == "--runner" {
            mode = RunMode::Runner;
            continue;
        }
        if arg == "--config" {
            let path = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--config requires a file path argument"))?;
            config = Some(PathBuf::from(path));
            continue;
        }

        if let Some(path) = arg.strip_prefix("--config=") {
            config = Some(PathBuf::from(path));
            continue;
        }
    }

    Ok(CliArgs {
        config_path: config,
        mode,
    })
}
