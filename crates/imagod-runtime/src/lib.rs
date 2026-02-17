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

/// Runtime abstraction types.
pub use imagod_runtime_internal::{
    ComponentRuntime, NativePluginRegistry, NativePluginRegistryBuilder, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeRunRequest,
};
/// Runner runtime wrapper around a shared Wasmtime engine.
#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime_wasmtime::WasmRuntime;
/// Runs `imagod` in runner mode using bootstrap data read from stdin.
pub use runner_process::run_runner_from_stdin;
/// Runs `imagod` in runner mode with a custom native plugin registry.
pub use runner_process::run_runner_from_stdin_with_registry;

#[cfg(all(test, feature = "runtime-wasmtime"))]
mod tests {
    use super::WasmRuntime;

    #[test]
    fn wasm_runtime_reexport_is_available() {
        let ctor: fn() -> Result<WasmRuntime, imagod_common::ImagodError> = WasmRuntime::new;
        let _ = ctor;
    }
}
