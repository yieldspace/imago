use std::sync::Arc;

use imago_plugin_imago_admin::ImagoAdminPlugin;
use imago_plugin_imago_node::ImagoNodePlugin;
use imagod_runtime::{
    NativePluginRegistry, NativePluginRegistryBuilder, run_runner_from_stdin_with_registry,
};

pub(crate) async fn run_runner() -> Result<(), anyhow::Error> {
    run_runner_from_stdin_with_registry(builtin_native_plugin_registry()?)
        .await
        .map_err(anyhow::Error::new)
}

fn builtin_native_plugin_registry() -> Result<NativePluginRegistry, anyhow::Error> {
    let mut builder = NativePluginRegistryBuilder::new();
    builder
        .register_plugin(Arc::new(ImagoAdminPlugin))
        .map_err(anyhow::Error::new)?;
    builder
        .register_plugin(Arc::new(ImagoNodePlugin))
        .map_err(anyhow::Error::new)?;
    Ok(builder.build())
}
