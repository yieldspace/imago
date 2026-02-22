use std::sync::Arc;

use imago_plugin_nanokvm_plugin::ImagoNanoKvmPlugin;
use imagod::{
    NativePluginRegistry, NativePluginRegistryBuilder, dispatch_from_env_with_registry,
    register_builtin_native_plugins,
};

fn build_native_plugin_registry() -> Result<NativePluginRegistry, anyhow::Error> {
    let mut builder = NativePluginRegistryBuilder::new();
    register_builtin_native_plugins(&mut builder)?;
    builder
        .register_plugin(Arc::new(ImagoNanoKvmPlugin))
        .map_err(anyhow::Error::new)?;
    Ok(builder.build())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    dispatch_from_env_with_registry(build_native_plugin_registry()?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_builtin_and_nanokvm_plugins() {
        let registry = build_native_plugin_registry().expect("registry should build");
        assert!(registry.has_plugin("imago:admin"));
        assert!(registry.has_plugin("imago:node"));
        assert!(registry.has_plugin("imago:nanokvm"));
    }

    #[test]
    fn duplicate_nanokvm_plugin_registration_fails() {
        let mut builder = NativePluginRegistryBuilder::new();
        register_builtin_native_plugins(&mut builder)
            .expect("builtin plugin registration should work");
        builder
            .register_plugin(Arc::new(ImagoNanoKvmPlugin))
            .expect("first nanokvm plugin registration should work");

        let err = match builder.register_plugin(Arc::new(ImagoNanoKvmPlugin)) {
            Ok(_) => panic!("duplicate package should fail"),
            Err(err) => err,
        };
        assert!(
            err.message.contains("already registered"),
            "unexpected message: {}",
            err.message
        );
    }
}
