use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use imago_protocol::ErrorCode;
use tokio::sync::watch;
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{
    WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
    p2::{add_to_linker_async, bindings::Command},
};

use imagod_common::ImagodError;

const STAGE_RUNTIME: &str = "runtime.start";

struct WasiState {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl WasiView for WasiState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

#[derive(Clone)]
pub struct WasmRuntime {
    engine: Arc<Engine>,
}

impl WasmRuntime {
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

    pub fn increment_epoch(&self) {
        self.engine.increment_epoch();
    }

    pub fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError> {
        Component::from_file(&self.engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;
        Ok(())
    }

    pub async fn run_cli_component_async(
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
        };
        let mut store = Store::new(&self.engine, state);
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);

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
}

fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}

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
