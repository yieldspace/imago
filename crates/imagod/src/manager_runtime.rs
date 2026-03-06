use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use imagod_common::ImagodError;
use imagod_config::{load_or_create_default, resolve_config_path};
use imagod_control::{ArtifactStore, OperationManager, Orchestrator, ServiceSupervisor};
use imagod_server::{ProtocolHandler, build_server};
#[cfg(unix)]
use tokio::net::UnixListener;
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
        config.runtime.committed_session_ttl_secs,
        config.runtime.max_committed_sessions,
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
        config.runtime.retained_logs_capacity_bytes,
        config.runtime.epoch_tick_interval_ms,
        &config_path,
    )
    .map_err(anyhow::Error::new)?
    .with_wasm_engine_tuning(
        config.runtime.wasm_memory_reservation_bytes,
        config.runtime.wasm_memory_reservation_for_growth_bytes,
        config.runtime.wasm_memory_guard_size_bytes,
        config.runtime.wasm_guard_before_linear_memory,
        config.runtime.wasm_parallel_compilation,
    )
    .with_http_queue_memory_budget_bytes(config.runtime.http_queue_memory_budget_bytes);
    let orchestrator = Orchestrator::new(&config.storage_root, artifacts.clone(), supervisor);
    let mut server = build_server(&config).map_err(anyhow::Error::new)?;
    #[cfg(unix)]
    let control_listener =
        bind_control_listener(config.control_socket_path.as_path()).map_err(anyhow::Error::new)?;

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

    eprintln!(
        "imagod listening on {} and {}",
        config.listen_addr,
        config.control_socket_path.display()
    );
    let mut shutdown_signal = std::pin::pin!(tokio::signal::ctrl_c());
    let mut session_tasks = tokio::task::JoinSet::new();
    let session_concurrency = Arc::new(tokio::sync::Semaphore::new(
        config.runtime.max_concurrent_sessions as usize,
    ));
    let mut shutdown_started = false;

    #[cfg(unix)]
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
            accepted = control_listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        eprintln!("control socket accept error: {err}");
                        continue;
                    }
                };
                let permit = match session_concurrency.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        drop(stream);
                        continue;
                    }
                };
                let handler = handler.clone();
                session_tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(err) = handler.handle_local_stream(stream).await {
                        eprintln!("control socket error: {err}");
                    }
                });
            }
        }
    }

    #[cfg(not(unix))]
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

#[cfg(unix)]
fn bind_control_listener(path: &Path) -> Result<UnixListener, ImagodError> {
    use std::{
        fs,
        os::unix::fs::{FileTypeExt, PermissionsExt},
    };

    let parent = path.parent().ok_or_else(|| {
        ImagodError::new(
            imago_protocol::ErrorCode::BadRequest,
            "transport.control_socket",
            format!("control socket path must have a parent: {}", path.display()),
        )
    })?;
    let parent_existed = parent.exists();
    fs::create_dir_all(parent).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "transport.control_socket",
            format!(
                "failed to create control socket dir {}: {e}",
                parent.display()
            ),
        )
    })?;
    if !parent_existed {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|e| {
            ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "transport.control_socket",
                format!(
                    "failed to set control socket dir permissions {}: {e}",
                    parent.display()
                ),
            )
        })?;
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|e| {
                ImagodError::new(
                    imago_protocol::ErrorCode::Internal,
                    "transport.control_socket",
                    format!(
                        "failed to remove stale control socket {}: {e}",
                        path.display()
                    ),
                )
            })?;
        }
        Ok(_) => {
            return Err(ImagodError::new(
                imago_protocol::ErrorCode::BadRequest,
                "transport.control_socket",
                format!(
                    "control socket path already exists and is not a socket: {}",
                    path.display()
                ),
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(ImagodError::new(
                imago_protocol::ErrorCode::Internal,
                "transport.control_socket",
                format!(
                    "failed to inspect control socket path {}: {err}",
                    path.display()
                ),
            ));
        }
    }

    let listener = UnixListener::bind(path).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "transport.control_socket",
            format!("failed to bind control socket {}: {e}", path.display()),
        )
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| {
        ImagodError::new(
            imago_protocol::ErrorCode::Internal,
            "transport.control_socket",
            format!(
                "failed to set control socket permissions {}: {e}",
                path.display()
            ),
        )
    })?;
    Ok(listener)
}

#[cfg(all(test, unix))]
mod tests {
    use super::bind_control_listener;
    use std::{
        fs,
        os::unix::fs::{FileTypeExt, PermissionsExt},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    fn unique_socket_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        PathBuf::from("/tmp").join(format!("imagod-{name}-{unique}.sock"))
    }

    #[tokio::test]
    async fn bind_control_listener_replaces_stale_socket_file() {
        let socket_path = unique_socket_path("stale-socket");
        let stale_listener = std::os::unix::net::UnixListener::bind(&socket_path)
            .expect("initial unix listener should bind");
        drop(stale_listener);
        let metadata = fs::symlink_metadata(&socket_path).expect("stale socket should remain");
        assert!(
            metadata.file_type().is_socket(),
            "expected stale socket file"
        );

        let rebound = bind_control_listener(&socket_path).expect("stale socket should be rebound");
        let metadata = fs::symlink_metadata(&socket_path).expect("rebound socket should exist");
        assert!(
            metadata.file_type().is_socket(),
            "expected rebound socket file"
        );
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        drop(rebound);
        fs::remove_file(&socket_path).expect("socket file should be removed");
    }

    #[tokio::test]
    async fn bind_control_listener_rejects_non_socket_path() {
        let socket_path = unique_socket_path("regular-file");
        fs::write(&socket_path, b"not-a-socket").expect("regular file should be created");

        let err = bind_control_listener(&socket_path).expect_err("regular file must be rejected");
        assert!(err.message.contains("is not a socket"));

        fs::remove_file(&socket_path).expect("regular file should be removed");
    }

    #[tokio::test]
    async fn bind_control_listener_sets_private_permissions_for_new_parent_dir() {
        let root = unique_socket_path("private-parent-root");
        fs::create_dir_all(&root).expect("root temp dir should be created");
        let parent = root.join("control");
        let socket_path = parent.join("imagod.sock");

        let listener = bind_control_listener(&socket_path).expect("listener should bind");
        let parent_metadata = fs::metadata(&parent).expect("parent dir should exist");
        let socket_metadata = fs::symlink_metadata(&socket_path).expect("socket should exist");
        assert_eq!(parent_metadata.permissions().mode() & 0o777, 0o700);
        assert_eq!(socket_metadata.permissions().mode() & 0o777, 0o600);

        drop(listener);
        fs::remove_dir_all(&root).expect("temp dir should be removed");
    }
}
