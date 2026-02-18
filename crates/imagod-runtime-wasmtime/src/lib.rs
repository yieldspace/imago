//! Wasmtime runtime integration used by runner processes.

pub mod native_plugins;

mod capability_checker;
mod http_supervisor;
mod plugin_resolver;
mod runtime_entry;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::RunnerAppType;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

pub use native_plugins::{NativePlugin, NativePluginRegistry, NativePluginRegistryBuilder};
pub use runtime_entry::WasmRuntime;

pub(crate) const STAGE_RUNTIME: &str = "runtime.start";
pub(crate) const HTTP_REQUEST_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePluginContext {
    service_name: String,
    release_hash: String,
    runner_id: String,
    app_type: String,
}

impl NativePluginContext {
    pub fn new(
        service_name: String,
        release_hash: String,
        runner_id: String,
        app_type: RunnerAppType,
    ) -> Self {
        Self {
            service_name,
            release_hash,
            runner_id,
            app_type: app_type_text(app_type).to_string(),
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn release_hash(&self) -> &str {
        &self.release_hash
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    pub fn app_type(&self) -> &str {
        &self.app_type
    }
}

pub fn app_type_text(app_type: RunnerAppType) -> &'static str {
    match app_type {
        RunnerAppType::Cli => "cli",
        RunnerAppType::Http => "http",
        RunnerAppType::Socket => "socket",
    }
}

/// Internal WASI host state stored in the Wasmtime store.
pub struct WasiState {
    pub(crate) table: ResourceTable,
    pub(crate) wasi: WasiCtx,
    pub(crate) http: WasiHttpCtx,
    pub(crate) native_plugin_context: NativePluginContext,
}

impl WasiState {
    pub fn native_plugin_context(&self) -> &NativePluginContext {
        &self.native_plugin_context
    }
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

pub(crate) fn map_runtime_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_RUNTIME, message)
}

pub(crate) fn map_runtime_unauthorized_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Unauthorized, STAGE_RUNTIME, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_ipc::RunnerAppType;

    #[test]
    fn native_plugin_app_type_text_is_stable() {
        assert_eq!(app_type_text(RunnerAppType::Cli), "cli");
        assert_eq!(app_type_text(RunnerAppType::Http), "http");
        assert_eq!(app_type_text(RunnerAppType::Socket), "socket");
    }

    #[test]
    fn native_plugin_context_stores_runner_metadata() {
        let context = NativePluginContext::new(
            "svc-test".to_string(),
            "release-test".to_string(),
            "runner-test".to_string(),
            RunnerAppType::Http,
        );
        assert_eq!(context.service_name(), "svc-test");
        assert_eq!(context.release_hash(), "release-test");
        assert_eq!(context.runner_id(), "runner-test");
        assert_eq!(context.app_type(), "http");
    }
}
