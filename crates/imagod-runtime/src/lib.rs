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
    ComponentRuntime, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeRunRequest,
};
/// Runner runtime wrapper around a shared Wasmtime engine.
#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime_wasmtime::WasmRuntime;
#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime_wasmtime::{
    NativePlugin, NativePluginRegistry, NativePluginRegistryBuilder,
};
/// Runs `imagod` in runner mode using bootstrap data read from stdin.
pub use runner_process::run_runner_from_stdin;
/// Runs `imagod` in runner mode with a custom native plugin registry.
pub use runner_process::run_runner_from_stdin_with_registry;

#[cfg(not(feature = "runtime-wasmtime"))]
mod native_plugin_stub {
    use imago_protocol::ErrorCode;
    use imagod_common::ImagodError;

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
