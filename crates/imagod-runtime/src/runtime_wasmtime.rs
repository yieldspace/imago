//! Wasmtime runtime integration used by runner processes.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use imago_protocol::ErrorCode;
use imagod_ipc::RunnerAppType;
use tokio::sync::watch;
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{
    WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
    p2::{add_to_linker_async, bindings::Command},
};
use wasmtime_wasi_http::{
    WasiHttpCtx, WasiHttpView, add_only_http_to_linker_async, bindings::Proxy,
};

use imagod_common::ImagodError;

use crate::runtime::{ComponentRuntime, RuntimeRunFuture, RuntimeRunRequest};

const STAGE_RUNTIME: &str = "runtime.start";

/// Internal WASI host state stored in the Wasmtime store.
struct WasiState {
    table: ResourceTable,
    wasi: WasiCtx,
    http: WasiHttpCtx,
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WasiState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

#[derive(Clone)]
/// Runner-local wrapper around a configured Wasmtime engine.
pub struct WasmRuntime {
    engine: Arc<Engine>,
}

impl WasmRuntime {
    /// Creates a runtime with component model, async support, and epoch interruption enabled.
    pub fn new() -> Result<Self, ImagodError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config)
            .map_err(|e| map_runtime_error(format!("engine init failed: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
        })
    }

    /// Increments the engine epoch to unblock interruption-aware execution.
    pub fn increment_epoch(&self) {
        self.engine.increment_epoch();
    }

    fn validate_component_loadable(&self, component_path: &Path) -> Result<(), ImagodError> {
        Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;
        Ok(())
    }

    fn build_store(
        &self,
        args: &[String],
        envs: &BTreeMap<String, String>,
    ) -> Result<Store<WasiState>, ImagodError> {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdio();
        if !args.is_empty() {
            builder.args(args);
        }
        if !envs.is_empty() {
            let vars = envs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>();
            builder.envs(&vars);
        }

        let state = WasiState {
            table: ResourceTable::new(),
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
        };
        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        Ok(store)
    }

    /// Instantiates and runs a WASI CLI component asynchronously.
    ///
    /// Returns when execution completes or when shutdown is requested.
    async fn run_cli_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        mut shutdown: watch::Receiver<bool>,
        epoch_tick_interval_ms: u64,
    ) -> Result<(), ImagodError> {
        let component = Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI linker: {e}")))?;

        let mut store = self.build_store(args, envs)?;

        let run_future = async {
            let command = Command::instantiate_async(&mut store, &component, &linker)
                .await
                .map_err(|e| map_runtime_error(format!("component instantiate failed: {e}")))?;
            let run_result = command
                .wasi_cli_run()
                .call_run(&mut store)
                .await
                .map_err(|e| map_runtime_error(format!("wasi cli run trap: {e}")))?;

            run_result.map_err(|()| {
                ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_RUNTIME,
                    "wasi cli run returned failure status",
                )
            })
        };

        let tick_runtime = self.clone();
        let (tick_stop_tx, mut tick_stop_rx) = watch::channel(false);
        let tick_interval = Duration::from_millis(epoch_tick_interval_ms.max(1));
        let tick_task = tokio::spawn(async move {
            loop {
                if *tick_stop_rx.borrow() {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(tick_interval) => {
                        tick_runtime.increment_epoch();
                    }
                    changed = tick_stop_rx.changed() => {
                        if changed.is_err() || *tick_stop_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        let result = tokio::select! {
            _ = wait_for_shutdown(&mut shutdown) => Ok(()),
            result = run_future => result,
        };
        let _ = tick_stop_tx.send(true);
        let _ = tick_task.await;
        result
    }

    /// Instantiates a WASI HTTP incoming-handler and waits for shutdown.
    async fn run_http_component_async(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), ImagodError> {
        let component = Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI linker: {e}")))?;
        add_only_http_to_linker_async(&mut linker)
            .map_err(|e| map_runtime_error(format!("failed to add WASI HTTP linker: {e}")))?;

        let mut store = self.build_store(args, envs)?;
        let _proxy = Proxy::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| map_runtime_error(format!("http component instantiate failed: {e}")))?;

        wait_for_shutdown(&mut shutdown).await;
        Ok(())
    }
}

impl ComponentRuntime for WasmRuntime {
    fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError> {
        self.validate_component_loadable(component_path)
    }

    fn run_component<'a>(&'a self, request: RuntimeRunRequest) -> RuntimeRunFuture<'a> {
        Box::pin(async move {
            let RuntimeRunRequest {
                app_type,
                component_path,
                args,
                envs,
                shutdown,
                epoch_tick_interval_ms,
            } = request;

            match app_type {
                RunnerAppType::Cli => {
                    self.run_cli_component_async(
                        &component_path,
                        &args,
                        &envs,
                        shutdown,
                        epoch_tick_interval_ms,
                    )
                    .await
                }
                RunnerAppType::Http => {
                    self.run_http_component_async(&component_path, &args, &envs, shutdown)
                        .await
                }
                RunnerAppType::Socket => Err(ImagodError::new(
                    ErrorCode::Internal,
                    STAGE_RUNTIME,
                    "socket runtime type is not implemented yet",
                )),
            }
        })
    }
}

/// Maps runtime-originated failures to a unified internal error shape.
fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}

/// Waits until shutdown flag is set or sender side is dropped.
async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeRunRequest;
    use imagod_ipc::RunnerAppType;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn socket_type_returns_not_implemented_error() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Socket,
                component_path: PathBuf::from("/tmp/unused.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
            })
            .await
            .expect_err("socket type should be rejected");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("not implemented"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn http_type_uses_http_execution_branch() {
        let runtime = WasmRuntime::new().expect("runtime should initialize");
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);
        let err = runtime
            .run_component(RuntimeRunRequest {
                app_type: RunnerAppType::Http,
                component_path: PathBuf::from("/tmp/non-existent-http-component.wasm"),
                args: Vec::new(),
                envs: BTreeMap::new(),
                shutdown: shutdown_rx,
                epoch_tick_interval_ms: 50,
            })
            .await
            .expect_err("missing component path should fail");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.contains("failed to load component"),
            "unexpected message: {}",
            err.message
        );
    }
}
