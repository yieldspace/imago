use imagod_runtime::{NativePluginRegistry, run_runner_from_stdin_with_registry};

pub(crate) async fn run_runner_with_registry(
    native_plugin_registry: NativePluginRegistry,
) -> Result<(), anyhow::Error> {
    run_runner_from_stdin_with_registry(native_plugin_registry)
        .await
        .map_err(anyhow::Error::new)
}
