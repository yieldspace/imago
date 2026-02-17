mod registry;

pub use registry::{
    NativePlugin, NativePluginLinker, NativePluginRegistry, NativePluginRegistryBuilder,
    NativePluginResult, map_native_plugin_linker_error,
};

pub use super::{NativePluginContext, WasiState};
pub use wasmtime::component::HasSelf;
