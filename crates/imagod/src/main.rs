mod artifact_store;
mod config;
mod error;
mod operation_state;
mod orchestrator;
mod protocol_handler;
mod runtime_wasmtime;
mod service_supervisor;
mod transport;

use std::{path::PathBuf, sync::Arc};

use artifact_store::ArtifactStore;
use config::{ImagodConfig, resolve_config_path};
use operation_state::OperationManager;
use orchestrator::Orchestrator;
use protocol_handler::ProtocolHandler;
use runtime_wasmtime::WasmRuntime;
use service_supervisor::ServiceSupervisor;
use transport::build_server;
use web_transport_quinn::http::StatusCode;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let config_path = resolve_config_path(parse_config_arg()?);
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
    let runtime = WasmRuntime::new().map_err(anyhow::Error::new)?;
    let supervisor =
        ServiceSupervisor::new(runtime.clone(), config.runtime.stop_grace_timeout_secs);
    let orchestrator = Orchestrator::new(&config.storage_root, artifacts.clone(), supervisor);

    let handler = ProtocolHandler::new(config.clone(), artifacts, operations, orchestrator);

    let maintenance_handler = handler.clone();
    let maintenance_runtime = runtime.clone();
    let active_tick_interval =
        std::time::Duration::from_millis(config.runtime.epoch_tick_interval_ms.max(1));
    let idle_tick_interval = std::time::Duration::from_secs(1);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let maintenance_task = tokio::spawn(async move {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            maintenance_handler.reap_finished_services().await;
            let sleep_duration = if maintenance_handler.has_live_services().await {
                maintenance_runtime.increment_epoch();
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
    let _ = shutdown_tx.send(true);
    maintenance_task
        .await
        .map_err(|err| anyhow::anyhow!("maintenance task join failed: {err}"))?;

    Ok(())
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

fn parse_config_arg() -> Result<Option<PathBuf>, anyhow::Error> {
    let mut args = std::env::args().skip(1);
    let mut config: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
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

    Ok(config)
}
