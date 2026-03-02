//! Public library entrypoints for embedding `imagod` manager/runner dispatch.

use std::{path::PathBuf, sync::Arc};

use clap::{CommandFactory, FromArgMatches, Parser, error::ErrorKind};
use imago_plugin_imago_admin::ImagoAdminPlugin;
use imago_plugin_imago_experimental_gpio::ImagoExperimentalGpioPlugin;
use imago_plugin_imago_experimental_i2c::ImagoExperimentalI2cPlugin;
use imago_plugin_imago_node::ImagoNodePlugin;
use imago_plugin_imago_usb::ImagoUsbPlugin;
use imago_protocol::PROTOCOL_VERSION;

mod manager_runtime;
mod runner_runtime;
mod shutdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manager,
    Runner,
}

#[derive(Debug, Clone, Parser)]
#[command(name = "imagod", about = "imago daemon")]
struct CliArgs {
    /// Path to imagod.toml used by manager mode.
    #[arg(long = "config", value_name = "PATH")]
    config_path: Option<PathBuf>,
    /// Start as an internal runner process.
    #[arg(long)]
    runner: bool,
}

impl CliArgs {
    fn mode(&self) -> RunMode {
        if self.runner {
            RunMode::Runner
        } else {
            RunMode::Manager
        }
    }
}

#[cfg(feature = "runtime-wasmtime")]
pub use imagod_runtime::NativePlugin;
pub use imagod_runtime::{NativePluginRegistry, NativePluginRegistryBuilder};

/// Dispatches `imagod` from process arguments with built-in native plugins.
pub async fn dispatch_from_env() -> Result<(), anyhow::Error> {
    install_rustls_provider();
    let Some(cli) = parse_cli_args_or_emit(std::env::args().skip(1))? else {
        return Ok(());
    };

    match cli.mode() {
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
    let Some(cli) = parse_cli_args_or_emit(std::env::args().skip(1))? else {
        return Ok(());
    };

    match cli.mode() {
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

fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<CliArgs, clap::Error> {
    let mut command = CliArgs::command();
    command = command.about(cli_about_text());
    command = command.version(env!("CARGO_PKG_VERSION"));
    command
        .try_get_matches_from(std::iter::once("imagod".to_string()).chain(args))
        .and_then(|matches| CliArgs::from_arg_matches(&matches))
}

fn cli_about_text() -> String {
    format!("imago daemon (protocol {PROTOCOL_VERSION})")
}

fn parse_cli_args_or_emit(
    args: impl IntoIterator<Item = String>,
) -> Result<Option<CliArgs>, anyhow::Error> {
    match parse_cli_args(args) {
        Ok(cli) => Ok(Some(cli)),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            err.print()?;
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_defaults_to_manager_mode() {
        let cli = parse_cli_args(Vec::<String>::new()).expect("cli parse should succeed");
        assert_eq!(cli.mode(), RunMode::Manager);
        assert!(!cli.runner);
        assert_eq!(cli.config_path, None);
    }

    #[test]
    fn parse_cli_accepts_runner_flag() {
        let cli = parse_cli_args(vec!["--runner".to_string()]).expect("runner parse should work");
        assert_eq!(cli.mode(), RunMode::Runner);
        assert!(cli.runner);
    }

    #[test]
    fn parse_cli_accepts_config_separate_argument() {
        let cli = parse_cli_args(vec!["--config".to_string(), "imagod.toml".to_string()])
            .expect("config parse should work");
        assert_eq!(cli.mode(), RunMode::Manager);
        assert_eq!(cli.config_path, Some(PathBuf::from("imagod.toml")));
    }

    #[test]
    fn parse_cli_accepts_config_equals_argument() {
        let cli = parse_cli_args(vec!["--config=/tmp/imagod.toml".to_string()])
            .expect("config parse should work");
        assert_eq!(cli.mode(), RunMode::Manager);
        assert_eq!(cli.config_path, Some(PathBuf::from("/tmp/imagod.toml")));
    }

    #[test]
    fn parse_cli_requires_config_value() {
        let err = parse_cli_args(vec!["--config".to_string()]).expect_err("must fail");
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
        assert!(err.to_string().contains("--config <PATH>"));
    }

    #[test]
    fn parse_cli_reports_version_information() {
        let err = parse_cli_args(vec!["--version".to_string()]).expect_err("must print version");
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn parse_cli_help_contains_protocol_version_in_about() {
        let err = parse_cli_args(vec!["--help".to_string()]).expect_err("must print help");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let expected = format!("protocol {PROTOCOL_VERSION}");
        assert!(err.to_string().contains(&expected));
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
