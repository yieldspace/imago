use std::{collections::BTreeMap, path::Path};

use imago_protocol::ErrorCode;
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{
    WasiCtx, WasiCtxBuilder, WasiView, add_to_linker_sync, bindings::sync::Command,
};

use crate::error::ImagodError;

const STAGE_RUNTIME: &str = "runtime.start";

struct WasiState {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl WasiView for WasiState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

#[derive(Clone, Default)]
pub struct WasmRuntime;

impl WasmRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn run_cli_component(
        &self,
        component_path: &Path,
        args: &[String],
        envs: &BTreeMap<String, String>,
    ) -> Result<(), ImagodError> {
        let mut config = Config::new();
        config.wasm_component_model(true);

        let engine = Engine::new(&config)
            .map_err(|e| map_runtime_error(format!("engine init failed: {e}")))?;
        let component = Component::from_file(&engine, component_path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load component {}: {e}",
                component_path.display()
            ))
        })?;

        let mut linker = Linker::new(&engine);
        add_to_linker_sync(&mut linker)
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
        let mut store = Store::new(&engine, state);

        let command = Command::instantiate(&mut store, &component, &linker)
            .map_err(|e| map_runtime_error(format!("component instantiate failed: {e}")))?;
        let run_result = command
            .wasi_cli_run()
            .call_run(&mut store)
            .map_err(|e| map_runtime_error(format!("wasi cli run trap: {e}")))?;

        run_result.map_err(|()| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNTIME,
                "wasi cli run returned failure status",
            )
        })
    }
}

fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}
