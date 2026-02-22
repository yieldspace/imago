use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::native_plugins::{
    HasSelf, NativePlugin, NativePluginLinker, NativePluginResult, map_native_plugin_linker_error,
};
use wasmtime_wasi::WasiView;

mod capture;
mod common;
mod constants;
mod device_status;
mod hid_control;
mod io_control;
mod runtime_control;
mod session;
mod stream_config;
mod types;

#[cfg(test)]
mod tests;

pub mod imago_nanokvm_plugin_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "host",
        with: {
            "wasi": wasmtime_wasi::p2::bindings::sync,
        },
        require_store_data_send: true,
    });
}

#[derive(Debug, Default)]
#[imago_native_plugin(
    wit = "wit",
    world = "host",
    descriptor_only = true,
    multi_imports = true,
    allow_non_resource_types = true,
    generate_bindings = false
)]
pub struct ImagoNanoKvmPlugin;

impl NativePlugin for ImagoNanoKvmPlugin {
    fn package_name(&self) -> &'static str {
        Self::PACKAGE_NAME
    }

    fn supports_import(&self, import_name: &str) -> bool {
        Self::IMPORTS.contains(&import_name)
    }

    fn symbols(&self) -> &'static [&'static str] {
        Self::SYMBOLS
    }

    fn supports_symbol(&self, symbol: &str) -> bool {
        Self::IMPORTS.iter().any(|import_name| {
            symbol
                .strip_prefix(import_name)
                .is_some_and(|tail| tail.starts_with('.'))
        })
    }

    fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()> {
        imago_nanokvm_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(linker, |state| {
            state.ctx().table
        })
        .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
    }
}
