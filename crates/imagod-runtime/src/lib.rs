//! Runner-side execution runtime and process bootstrap entrypoints.

/// Runner bootstrap path that starts from stdin-delivered metadata.
pub mod runner_process;
/// Runtime abstraction shared by runner and concrete runtime implementations.
pub mod runtime;
/// Wasmtime-based component execution runtime.
#[cfg(feature = "runtime-wasmtime")]
pub mod runtime_wasmtime {
    pub use imagod_runtime_wasmtime::*;
}

/// Runner bootstrap helpers extracted from runner process orchestration.
pub use imagod_runtime_bootstrap::{
    MAX_RUNNER_BOOTSTRAP_BYTES, STAGE_RUNNER, STAGE_RUNNER_BOOTSTRAP, SocketCleanupGuard,
    decode_runner_bootstrap, prepare_socket_path, read_runner_bootstrap,
    validate_runner_bootstrap_size,
};
/// Runner control-plane helpers and manager-client abstraction.
pub use imagod_runtime_control::{
    DbusRunnerManagerClient, RunnerManagerClient, mark_ready, register, run_inbound_server,
    send_heartbeats,
};
/// Runner HTTP ingress server helpers.
pub use imagod_runtime_ingress::{
    DEFAULT_HTTP_MAX_BODY_BYTES, MAX_HTTP_MAX_BODY_BYTES, STAGE_HTTP_INGRESS,
    required_http_max_body_bytes, required_http_port, spawn_http_ingress_server,
};
/// Runtime abstraction types.
pub use imagod_runtime_internal::{
    ComponentRuntime, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeInvokeRequest,
    RuntimeInvoker, RuntimeRunRequest,
};
/// Runner runtime wrapper around a shared Wasmtime engine.
#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime_wasmtime::WasmRuntime;
#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime_wasmtime::{
    NativePlugin, NativePluginRegistry, NativePluginRegistryBuilder, WasmEngineTuning,
};
/// Runs `imagod` in runner mode using bootstrap data read from stdin.
pub use runner_process::run_runner_from_stdin;
/// Runs `imagod` in runner mode with a custom native plugin registry.
pub use runner_process::run_runner_from_stdin_with_registry;

#[cfg(not(feature = "runtime-wasmtime"))]
mod native_plugin_stub {
    use imagod_common::ImagodError;
    use imagod_spec::ErrorCode;

    #[derive(Clone, Default)]
    pub struct NativePluginRegistry;

    #[derive(Default)]
    pub struct NativePluginRegistryBuilder;

    impl NativePluginRegistryBuilder {
        pub fn new() -> Self {
            Self
        }

        pub fn register_plugin<T>(&mut self, _plugin: T) -> Result<&mut Self, ImagodError> {
            Err(ImagodError::new(
                ErrorCode::Internal,
                "runner.process",
                "native plugin registry requires feature 'runtime-wasmtime'",
            ))
        }

        pub fn build(self) -> NativePluginRegistry {
            NativePluginRegistry
        }
    }
}

#[cfg(not(feature = "runtime-wasmtime"))]
pub use native_plugin_stub::{NativePluginRegistry, NativePluginRegistryBuilder};

#[cfg(all(test, feature = "runtime-wasmtime"))]
mod tests {
    use super::WasmRuntime;

    #[test]
    fn wasm_runtime_reexport_is_available() {
        let ctor: fn() -> Result<WasmRuntime, imagod_common::ImagodError> = WasmRuntime::new;
        let _ = ctor;
    }
}
