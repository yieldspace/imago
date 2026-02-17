use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::native_plugins::WasiState;

#[derive(Debug, Default)]
#[imago_native_plugin(wit = "wit", world = "host")]
pub struct ImagoAdminPlugin;

impl imago_admin_plugin_bindings::imago::admin::runtime::Host for WasiState {
    fn service_name(&mut self) -> String {
        self.native_plugin_context().service_name().to_string()
    }

    fn release_hash(&mut self) -> String {
        self.native_plugin_context().release_hash().to_string()
    }

    fn runner_id(&mut self) -> String {
        self.native_plugin_context().runner_id().to_string()
    }

    fn app_type(&mut self) -> String {
        self.native_plugin_context().app_type().to_string()
    }
}
