//! Public library entrypoints for embedding `imagod` manager/runner dispatch.

use std::{path::PathBuf, sync::Arc};

use imago_plugin_imago_admin::ImagoAdminPlugin;
use imago_plugin_imago_experimental_gpio::ImagoExperimentalGpioPlugin;
use imago_plugin_imago_experimental_i2c::ImagoExperimentalI2cPlugin;
use imago_plugin_imago_node::ImagoNodePlugin;
use imago_plugin_imago_usb::ImagoUsbPlugin;

mod manager_runtime;
mod runner_runtime;
mod shutdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manager,
    Runner,
}

#[derive(Debug, Clone)]
struct CliArgs {
    config_path: Option<PathBuf>,
    mode: RunMode,
}

#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime::NativePlugin;
pub use imagod_runtime::{NativePluginRegistry, NativePluginRegistryBuilder};

/// Dispatches `imagod` from process arguments with built-in native plugins.
pub async fn dispatch_from_env() -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let cli = parse_cli_args(std::env::args().skip(1))?;
    match cli.mode {
        RunMode::Runner => {
            runner_runtime::run_runner_with_registry(builtin_native_plugin_registry()?).await
        }
        RunMode::Manager => manager_runtime::run_manager(cli.config_path).await,
    }
}

/// Dispatches `imagod` from process arguments with a caller-provided native plugin registry.
pub async fn dispatch_from_env_with_registry(
    native_plugin_registry: NativePluginRegistry,
) -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let cli = parse_cli_args(std::env::args().skip(1))?;
    match cli.mode {
        RunMode::Runner => runner_runtime::run_runner_with_registry(native_plugin_registry).await,
        RunMode::Manager => manager_runtime::run_manager(cli.config_path).await,
    }
}

/// Registers built-in native plugins into a caller-provided registry builder.
pub fn register_builtin_native_plugins(
    builder: &mut NativePluginRegistryBuilder,
) -> Result<(), anyhow::Error> {
    builder
        .register_plugin(Arc::new(ImagoAdminPlugin))
        .map_err(anyhow::Error::new)?;
    builder
        .register_plugin(Arc::new(ImagoNodePlugin))
        .map_err(anyhow::Error::new)?;
    builder
        .register_plugin(Arc::new(ImagoExperimentalGpioPlugin))
        .map_err(anyhow::Error::new)?;
    builder
        .register_plugin(Arc::new(ImagoExperimentalI2cPlugin))
        .map_err(anyhow::Error::new)?;
    builder
        .register_plugin(Arc::new(ImagoUsbPlugin))
        .map_err(anyhow::Error::new)?;
    Ok(())
}

/// Builds a native plugin registry containing all built-in plugins.
pub fn builtin_native_plugin_registry() -> Result<NativePluginRegistry, anyhow::Error> {
    let mut builder = NativePluginRegistryBuilder::new();
    register_builtin_native_plugins(&mut builder)?;
    Ok(builder.build())
}

fn install_rustls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return;
    }

    let provider = web_transport_quinn::crypto::default_provider();
    if let Some(provider) = std::sync::Arc::into_inner(provider) {
        let _ = provider.install_default();
    }
}

fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, anyhow::Error> {
    let mut args = args.into_iter();
    let mut config: Option<PathBuf> = None;
    let mut mode = RunMode::Manager;

    while let Some(arg) = args.next() {
        if arg == "--runner" {
            mode = RunMode::Runner;
            continue;
        }
        if arg == "--config" {
            let path = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--config requires a file path argument"))?;
            config = Some(PathBuf::from(path));
            continue;
        }

        if let Some(path) = arg.strip_prefix("--config=") {
            config = Some(PathBuf::from(path));
            continue;
        }
    }

    Ok(CliArgs {
        config_path: config,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_defaults_to_manager_mode() {
        let cli = parse_cli_args(Vec::<String>::new()).expect("cli parse should succeed");
        assert_eq!(cli.mode, RunMode::Manager);
        assert_eq!(cli.config_path, None);
    }

    #[test]
    fn parse_cli_accepts_runner_flag() {
        let cli = parse_cli_args(vec!["--runner".to_string()]).expect("runner parse should work");
        assert_eq!(cli.mode, RunMode::Runner);
    }

    #[test]
    fn parse_cli_accepts_config_separate_argument() {
        let cli = parse_cli_args(vec!["--config".to_string(), "imagod.toml".to_string()])
            .expect("config parse should work");
        assert_eq!(cli.mode, RunMode::Manager);
        assert_eq!(cli.config_path, Some(PathBuf::from("imagod.toml")));
    }

    #[test]
    fn parse_cli_accepts_config_equals_argument() {
        let cli = parse_cli_args(vec!["--config=/tmp/imagod.toml".to_string()])
            .expect("config parse should work");
        assert_eq!(cli.mode, RunMode::Manager);
        assert_eq!(cli.config_path, Some(PathBuf::from("/tmp/imagod.toml")));
    }

    #[test]
    fn parse_cli_requires_config_value() {
        let err = parse_cli_args(vec!["--config".to_string()]).expect_err("must fail");
        assert!(
            err.to_string()
                .contains("--config requires a file path argument"),
            "unexpected error: {err}"
        );
    }

    #[cfg(feature = "runtime-wasmtime")]
    mod native_plugin_registry_tests {
        use super::*;
        use imagod_runtime::runtime_wasmtime::native_plugins::{
            NativePluginLinker, NativePluginResult,
        };

        #[derive(Debug)]
        struct TestPlugin;

        impl NativePlugin for TestPlugin {
            fn package_name(&self) -> &'static str {
                "test:custom"
            }

            fn supports_import(&self, import_name: &str) -> bool {
                import_name == "test:custom/runtime@0.1.0"
            }

            fn symbols(&self) -> &'static [&'static str] {
                &["test:custom/runtime@0.1.0.ping"]
            }

            fn add_to_linker(&self, _linker: &mut NativePluginLinker) -> NativePluginResult<()> {
                Ok(())
            }
        }

        #[test]
        fn builtin_registry_contains_default_plugins() {
            let registry = builtin_native_plugin_registry().expect("registry should build");
            assert!(registry.has_plugin("imago:admin"));
            assert!(registry.has_plugin("imago:node"));
            assert!(registry.has_plugin("imago:experimental-gpio"));
            assert!(registry.has_plugin("imago:experimental-i2c"));
            assert!(registry.has_plugin("imago:usb"));
        }

        #[test]
        fn custom_plugin_coexists_with_builtin_plugins() {
            let mut builder = NativePluginRegistryBuilder::new();
            register_builtin_native_plugins(&mut builder)
                .expect("builtin registration should work");
            builder
                .register_plugin(Arc::new(TestPlugin))
                .expect("custom plugin registration should work");
            let registry = builder.build();
            assert!(registry.has_plugin("imago:admin"));
            assert!(registry.has_plugin("imago:node"));
            assert!(registry.has_plugin("imago:experimental-gpio"));
            assert!(registry.has_plugin("imago:experimental-i2c"));
            assert!(registry.has_plugin("imago:usb"));
            assert!(registry.has_plugin("test:custom"));
        }

        #[test]
        fn duplicate_builtin_registration_fails() {
            let mut builder = NativePluginRegistryBuilder::new();
            register_builtin_native_plugins(&mut builder).expect("first registration should work");
            let err = register_builtin_native_plugins(&mut builder)
                .expect_err("duplicate registration should fail");
            assert!(
                err.to_string().contains("already registered"),
                "unexpected error: {err}"
            );
        }
    }
}
