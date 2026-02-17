use std::{collections::BTreeMap, sync::Arc};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use wasmtime::component::Linker;

use super::WasiState;

const STAGE_NATIVE_PLUGIN_REGISTRY: &str = "runtime.native_plugin_registry";
const STAGE_NATIVE_PLUGIN: &str = "runtime.native_plugin";

pub type NativePluginResult<T> = Result<T, ImagodError>;
pub type NativePluginLinker = Linker<WasiState>;

pub trait NativePlugin: Send + Sync + 'static {
    fn package_name(&self) -> &'static str;
    fn supports_import(&self, import_name: &str) -> bool;
    fn symbols(&self) -> &'static [&'static str];
    fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()>;

    fn supports_symbol(&self, symbol: &str) -> bool {
        self.symbols().contains(&symbol)
    }
}

#[derive(Clone, Default)]
pub struct NativePluginRegistry {
    plugins: Arc<BTreeMap<String, Arc<dyn NativePlugin>>>,
}

impl NativePluginRegistry {
    pub fn has_plugin(&self, package_name: &str) -> bool {
        self.plugins.contains_key(package_name)
    }

    pub fn plugin(&self, package_name: &str) -> Option<Arc<dyn NativePlugin>> {
        self.plugins.get(package_name).cloned()
    }

    pub fn has_symbol(&self, package_name: &str, symbol: &str) -> bool {
        self.plugin(package_name)
            .map(|plugin| plugin.supports_symbol(symbol))
            .unwrap_or(false)
    }
}

#[derive(Default)]
pub struct NativePluginRegistryBuilder {
    plugins: BTreeMap<String, Arc<dyn NativePlugin>>,
}

impl NativePluginRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_plugin(
        &mut self,
        plugin: Arc<dyn NativePlugin>,
    ) -> Result<&mut Self, ImagodError> {
        let package_name = plugin.package_name().trim();
        if package_name.is_empty() {
            return Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_NATIVE_PLUGIN_REGISTRY,
                "native plugin package name must not be empty",
            ));
        }

        if self.plugins.contains_key(package_name) {
            return Err(ImagodError::new(
                ErrorCode::Internal,
                STAGE_NATIVE_PLUGIN_REGISTRY,
                format!("native plugin '{}' is already registered", package_name),
            ));
        }

        self.plugins.insert(package_name.to_string(), plugin);
        Ok(self)
    }

    pub fn build(self) -> NativePluginRegistry {
        NativePluginRegistry {
            plugins: Arc::new(self.plugins),
        }
    }
}

pub fn map_native_plugin_linker_error(
    package_name: &str,
    err: impl std::fmt::Display,
) -> ImagodError {
    ImagodError::new(
        ErrorCode::Internal,
        STAGE_NATIVE_PLUGIN,
        format!(
            "failed to add native plugin linker '{}' to wasmtime linker: {err}",
            package_name
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin;

    impl NativePlugin for TestPlugin {
        fn package_name(&self) -> &'static str {
            "test:plugin"
        }

        fn supports_import(&self, import_name: &str) -> bool {
            import_name == "test:plugin/runtime@0.1.0"
        }

        fn symbols(&self) -> &'static [&'static str] {
            &["test:plugin/runtime@0.1.0.ping"]
        }

        fn add_to_linker(&self, _linker: &mut NativePluginLinker) -> NativePluginResult<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_builder_rejects_duplicate_package() {
        let mut builder = NativePluginRegistryBuilder::new();
        builder
            .register_plugin(Arc::new(TestPlugin))
            .expect("first register should succeed");

        let err = match builder.register_plugin(Arc::new(TestPlugin)) {
            Ok(_) => panic!("duplicate package should fail"),
            Err(err) => err,
        };
        assert!(
            err.message.contains("already registered"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn registry_has_symbol_checks_plugin_descriptor() {
        let mut builder = NativePluginRegistryBuilder::new();
        builder
            .register_plugin(Arc::new(TestPlugin))
            .expect("register should succeed");
        let registry = builder.build();

        assert!(
            registry.has_symbol("test:plugin", "test:plugin/runtime@0.1.0.ping"),
            "expected known symbol to be found"
        );
        assert!(
            !registry.has_symbol("test:plugin", "test:plugin/runtime@0.1.0.unknown"),
            "unexpected symbol should be rejected"
        );
    }
}
