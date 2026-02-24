use std::{path::PathBuf, sync::Arc};

use imagod_config::{load_or_create_default, resolve_config_path};
use imagod_control::{ArtifactStore, OperationManager, Orchestrator, ServiceSupervisor};
use imagod_server::{ProtocolHandler, build_server};
use web_transport_quinn::http::StatusCode;

use crate::shutdown::{
    drain_session_tasks, log_session_task_join_result, wait_for_maintenance_shutdown,
};

const SESSION_TASK_DRAIN_TIMEOUT_SECS: u64 = 15;

pub(crate) async fn run_manager(config_path: Option<PathBuf>) -> Result<(), anyhow::Error> {
    let config_path = resolve_config_path(config_path);
    let load_result = load_or_create_default(&config_path).map_err(anyhow::Error::new)?;
    if load_result.created_default {
        eprintln!(
            "imagod created default config at {}; review tls.server_key and tls.client_public_keys",
            config_path.display()
        );
    }
    let config = Arc::new(load_result.config);

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
    let supervisor = ServiceSupervisor::new_with_config_path(
        &config.storage_root,
        config.runtime.stop_grace_timeout_secs,
        config.runtime.runner_ready_timeout_secs,
        config.runtime.manager_control_read_timeout_ms,
        config.runtime.http_worker_count,
        config.runtime.http_worker_queue_capacity,
        config.runtime.runner_log_buffer_bytes,
        config.runtime.epoch_tick_interval_ms,
        &config_path,
    )
    .map_err(anyhow::Error::new)?;
    let orchestrator = Orchestrator::new(&config.storage_root, artifacts.clone(), supervisor);
    let mut server = build_server(&config).map_err(anyhow::Error::new)?;

    if config.runtime.boot_plugin_gc_enabled {
        match orchestrator.gc_unused_plugin_components_on_boot().await {
            Ok(()) => {
                eprintln!("plugin component cache gc completed");
            }
            Err(err) => {
                eprintln!(
                    "plugin component cache gc failed code={:?} stage={} message={}",
                    err.code, err.stage, err.message
                );
            }
        }
    } else {
        eprintln!("plugin component cache gc skipped by runtime.boot_plugin_gc_enabled=false");
    }

    if config.runtime.boot_restore_enabled {
        match orchestrator.restore_active_services_on_boot().await {
            Ok(summary) => {
                for started in &summary.started {
                    eprintln!(
                        "boot restore started name={} release={}",
                        started.service_name, started.release_hash
                    );
                }
                for failed in &summary.failed {
                    eprintln!(
                        "boot restore failed name={} code={:?} stage={} message={}",
                        failed.service_name,
                        failed.error.code,
                        failed.error.stage,
                        failed.error.message
                    );
                }
                eprintln!(
                    "boot restore summary started={} failed={}",
                    summary.started.len(),
                    summary.failed.len()
                );
            }
            Err(err) => {
                eprintln!(
                    "boot restore scan failed code={:?} stage={} message={}",
                    err.code, err.stage, err.message
                );
            }
        }
    } else {
        eprintln!("boot restore skipped by runtime.boot_restore_enabled=false");
    }

    let handler = ProtocolHandler::new(
        config.clone(),
        config_path.clone(),
        artifacts,
        operations,
        orchestrator,
    );

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

    eprintln!("imagod listening on {}", config.listen_addr);
    let mut shutdown_signal = std::pin::pin!(tokio::signal::ctrl_c());
    let mut session_tasks = tokio::task::JoinSet::new();
    let session_concurrency = Arc::new(tokio::sync::Semaphore::new(
        config.runtime.max_concurrent_sessions as usize,
    ));
    let mut shutdown_started = false;

    loop {
        tokio::select! {
            _ = &mut shutdown_signal => {
                eprintln!("shutdown signal received");
                handler.begin_shutdown();
                shutdown_started = true;
                break;
            }
            joined = session_tasks.join_next(), if !session_tasks.is_empty() => {
                if let Some(joined) = joined {
                    log_session_task_join_result(joined);
                }
            }
            request = server.accept() => {
                let Some(request): Option<web_transport_quinn::Request> = request else {
                    break;
                };
                let permit = match session_concurrency.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        let _ = request.respond(StatusCode::TOO_MANY_REQUESTS).await;
                        continue;
                    }
                };
                let handler = handler.clone();
                session_tasks.spawn(async move {
                    let _permit = permit;
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

    if !shutdown_started {
        handler.begin_shutdown();
    }
    drain_session_tasks(
        &mut session_tasks,
        std::time::Duration::from_secs(SESSION_TASK_DRAIN_TIMEOUT_SECS),
    )
    .await;

    let stop_errors = handler.stop_all_services(false).await;
    for (service_name, err) in stop_errors {
        eprintln!(
            "service shutdown failed name={} code={:?} stage={} message={}",
            service_name, err.code, err.stage, err.message
        );
    }
    if handler.has_live_services().await {
        let force_stop_errors = handler.stop_all_services(true).await;
        for (service_name, err) in force_stop_errors {
            eprintln!(
                "service force-shutdown failed name={} code={:?} stage={} message={}",
                service_name, err.code, err.stage, err.message
            );
        }
    }

    let _ = shutdown_tx.send(true);
    wait_for_maintenance_shutdown(maintenance_task).await?;

    Ok(())
}
