//! Public library entrypoints for embedding `imagod` manager/runner dispatch.

use std::{path::PathBuf, sync::Arc};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind};
use imago_plugin_imago_admin::ImagoAdminPlugin;
use imago_plugin_imago_experimental_gpio::ImagoExperimentalGpioPlugin;
use imago_plugin_imago_experimental_i2c::ImagoExperimentalI2cPlugin;
use imago_plugin_imago_node::ImagoNodePlugin;
use imago_plugin_imago_usb::ImagoUsbPlugin;
use imago_protocol::PROTOCOL_VERSION;
use imagod_common::BUILTIN_NATIVE_PLUGIN_DESCRIPTORS;
use imagod_config::DEFAULT_CONTROL_SOCKET_PATH;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod manager_runtime;
mod runner_runtime;
mod shutdown;

const STDIO_MESSAGE_TERMINATOR: [u8; 4] = 0u32.to_be_bytes();
const PROXY_STDIO_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Manager,
    Runner,
    ProxyStdio,
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    /// Bridge stdin/stdout to the local control socket for SSH transport.
    ProxyStdio(ProxyStdioArgs),
}

#[derive(Debug, Clone, Parser)]
struct ProxyStdioArgs {
    /// Override the local control socket path.
    #[arg(long = "socket", value_name = "PATH")]
    socket_path: Option<PathBuf>,
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
    #[command(subcommand)]
    command: Option<CliCommand>,
}

impl CliArgs {
    fn mode(&self) -> RunMode {
        if matches!(self.command, Some(CliCommand::ProxyStdio(_))) {
            RunMode::ProxyStdio
        } else if self.runner {
            RunMode::Runner
        } else {
            RunMode::Manager
        }
    }

    fn proxy_socket_path(&self) -> Option<PathBuf> {
        match &self.command {
            Some(CliCommand::ProxyStdio(args)) => args.socket_path.clone(),
            None => None,
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
        RunMode::ProxyStdio => run_proxy_stdio(cli.proxy_socket_path()).await,
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
        RunMode::ProxyStdio => run_proxy_stdio(cli.proxy_socket_path()).await,
    }
}

/// Registers built-in native plugins into a caller-provided registry builder.
pub fn register_builtin_native_plugins(
    builder: &mut NativePluginRegistryBuilder,
) -> Result<(), anyhow::Error> {
    for descriptor in BUILTIN_NATIVE_PLUGIN_DESCRIPTORS {
        register_builtin_native_plugin(builder, descriptor.package_name)?;
    }
    Ok(())
}

/// Builds a native plugin registry containing all built-in plugins.
pub fn builtin_native_plugin_registry() -> Result<NativePluginRegistry, anyhow::Error> {
    let mut builder = NativePluginRegistryBuilder::new();
    register_builtin_native_plugins(&mut builder)?;
    Ok(builder.build())
}

fn register_builtin_native_plugin(
    builder: &mut NativePluginRegistryBuilder,
    package_name: &str,
) -> Result<(), anyhow::Error> {
    match package_name {
        "imago:admin" => builder
            .register_plugin(Arc::new(ImagoAdminPlugin))
            .map_err(anyhow::Error::new)?,
        "imago:node" => builder
            .register_plugin(Arc::new(ImagoNodePlugin))
            .map_err(anyhow::Error::new)?,
        "imago:experimental-gpio" => builder
            .register_plugin(Arc::new(ImagoExperimentalGpioPlugin))
            .map_err(anyhow::Error::new)?,
        "imago:experimental-i2c" => builder
            .register_plugin(Arc::new(ImagoExperimentalI2cPlugin))
            .map_err(anyhow::Error::new)?,
        "imago:usb" => builder
            .register_plugin(Arc::new(ImagoUsbPlugin))
            .map_err(anyhow::Error::new)?,
        other => {
            return Err(anyhow::anyhow!(
                "unsupported built-in native plugin package '{}'",
                other
            ));
        }
    };

    Ok(())
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
    let cli = command
        .try_get_matches_from(std::iter::once("imagod".to_string()).chain(args))
        .and_then(|matches| CliArgs::from_arg_matches(&matches))?;
    validate_cli_args(cli)
}

fn cli_about_text() -> String {
    format!("imago daemon (protocol {PROTOCOL_VERSION})")
}

fn validate_cli_args(cli: CliArgs) -> Result<CliArgs, clap::Error> {
    if cli.command.is_some() && cli.runner {
        return Err(clap::Error::raw(
            ErrorKind::ArgumentConflict,
            "--runner cannot be used with proxy-stdio",
        ));
    }
    if cli.command.is_some() && cli.config_path.is_some() {
        return Err(clap::Error::raw(
            ErrorKind::ArgumentConflict,
            "--config cannot be used with proxy-stdio",
        ));
    }
    Ok(cli)
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

#[cfg(unix)]
async fn run_proxy_stdio(socket_path: Option<PathBuf>) -> Result<(), anyhow::Error> {
    let socket_path = socket_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_SOCKET_PATH));
    proxy_stdio_streams(socket_path, io::stdin(), io::stdout()).await
}

#[cfg(not(unix))]
async fn run_proxy_stdio(_socket_path: Option<PathBuf>) -> Result<(), anyhow::Error> {
    Err(anyhow::anyhow!(
        "proxy-stdio is only supported on unix platforms"
    ))
}

#[cfg(unix)]
async fn proxy_stdio_streams<R, W>(
    socket_path: PathBuf,
    mut input: R,
    mut output: W,
) -> Result<(), anyhow::Error>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    while let Some(message) = read_stdio_message(&mut input).await? {
        let mut stream = tokio::net::UnixStream::connect(&socket_path).await?;
        stream.write_all(&message).await?;
        stream.write_all(&STDIO_MESSAGE_TERMINATOR).await?;
        tokio::io::copy(&mut stream, &mut output).await?;
        output.write_all(&STDIO_MESSAGE_TERMINATOR).await?;
        output.flush().await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn read_stdio_message<R>(input: &mut R) -> Result<Option<Vec<u8>>, anyhow::Error>
where
    R: AsyncRead + Unpin + Send,
{
    let mut message = Vec::new();
    loop {
        let mut header = [0u8; 4];
        match input.read_exact(&mut header).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof && message.is_empty() => {
                return Ok(None);
            }
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(anyhow::anyhow!(
                    "proxy-stdio received a truncated frame header"
                ));
            }
            Err(err) => return Err(err.into()),
        }

        let len = u32::from_be_bytes(header) as usize;
        if len == 0 {
            if message.is_empty() {
                return Err(anyhow::anyhow!("proxy-stdio received an empty request"));
            }
            return Ok(Some(message));
        }

        ensure_stdio_message_growth(message.len(), len)?;
        let mut payload = vec![0u8; len];
        input.read_exact(&mut payload).await?;
        message.extend_from_slice(&header);
        message.extend_from_slice(&payload);
    }
}

#[cfg(unix)]
fn ensure_stdio_message_growth(
    current_len: usize,
    payload_len: usize,
) -> Result<(), anyhow::Error> {
    let next_frame_len = 4usize.checked_add(payload_len).ok_or_else(|| {
        anyhow::anyhow!(
            "proxy-stdio request exceeds max size {PROXY_STDIO_MAX_REQUEST_BYTES} bytes"
        )
    })?;
    let projected_len = current_len.checked_add(next_frame_len).ok_or_else(|| {
        anyhow::anyhow!(
            "proxy-stdio request exceeds max size {PROXY_STDIO_MAX_REQUEST_BYTES} bytes"
        )
    })?;
    if projected_len > PROXY_STDIO_MAX_REQUEST_BYTES {
        return Err(anyhow::anyhow!(
            "proxy-stdio request exceeds max size {PROXY_STDIO_MAX_REQUEST_BYTES} bytes"
        ));
    }
    Ok(())
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
    fn parse_cli_accepts_proxy_stdio_subcommand() {
        let cli =
            parse_cli_args(vec!["proxy-stdio".to_string()]).expect("proxy-stdio parse should work");
        assert_eq!(cli.mode(), RunMode::ProxyStdio);
        assert_eq!(cli.proxy_socket_path(), None);
    }

    #[test]
    fn parse_cli_accepts_proxy_stdio_socket_override() {
        let cli = parse_cli_args(vec![
            "proxy-stdio".to_string(),
            "--socket".to_string(),
            "/tmp/imagod.sock".to_string(),
        ])
        .expect("proxy-stdio socket parse should work");
        assert_eq!(cli.mode(), RunMode::ProxyStdio);
        assert_eq!(
            cli.proxy_socket_path(),
            Some(PathBuf::from("/tmp/imagod.sock"))
        );
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
    fn parse_cli_rejects_runner_with_proxy_stdio() {
        let err = parse_cli_args(vec!["--runner".to_string(), "proxy-stdio".to_string()])
            .expect_err("runner and proxy-stdio must conflict");
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
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

    #[cfg(unix)]
    mod proxy_stdio_tests {
        use super::*;
        use std::{
            io,
            path::PathBuf,
            pin::Pin,
            sync::{Arc, Mutex},
            task::{Context, Poll},
            time::{SystemTime, UNIX_EPOCH},
        };
        use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

        #[derive(Clone, Default)]
        struct CapturedOutput {
            bytes: Arc<Mutex<Vec<u8>>>,
        }

        impl CapturedOutput {
            fn bytes(&self) -> Vec<u8> {
                self.bytes
                    .lock()
                    .expect("output lock should succeed")
                    .clone()
            }
        }

        impl AsyncWrite for CapturedOutput {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.bytes
                    .lock()
                    .expect("output lock should succeed")
                    .extend_from_slice(buf);
                Poll::Ready(Ok(buf.len()))
            }

            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }

            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        fn frame(payload: &[u8]) -> Vec<u8> {
            let mut framed = Vec::with_capacity(4 + payload.len());
            framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            framed.extend_from_slice(payload);
            framed
        }

        fn temp_socket_path(test_name: &str) -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos();
            PathBuf::from("/tmp").join(format!(
                "ig-{}-{}-{}.sock",
                test_name,
                std::process::id(),
                nanos
            ))
        }

        async fn read_socket_request(stream: &mut tokio::net::UnixStream) -> Vec<u8> {
            let mut request = Vec::new();
            loop {
                let mut header = [0u8; 4];
                stream
                    .read_exact(&mut header)
                    .await
                    .expect("server should read frame header");
                let len = u32::from_be_bytes(header) as usize;
                if len == 0 {
                    break;
                }
                let mut payload = vec![0u8; len];
                stream
                    .read_exact(&mut payload)
                    .await
                    .expect("server should read frame payload");
                request.extend_from_slice(&header);
                request.extend_from_slice(&payload);
            }
            request
        }

        #[tokio::test]
        async fn proxy_stdio_streams_bridges_framed_messages_and_terminator() {
            let socket_path = temp_socket_path("bridge");
            let listener =
                tokio::net::UnixListener::bind(&socket_path).expect("unix listener should bind");

            let server_task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept should succeed");
                let request = read_socket_request(&mut stream).await;
                assert_eq!(request, frame(b"hello imagod"));
                stream
                    .write_all(&frame(b"hello cli"))
                    .await
                    .expect("server should write response");
                stream
                    .shutdown()
                    .await
                    .expect("server shutdown should succeed");
            });

            let (mut input_writer, input_reader) = tokio::io::duplex(64);
            input_writer
                .write_all(&frame(b"hello imagod"))
                .await
                .expect("input write should succeed");
            input_writer
                .write_all(&STDIO_MESSAGE_TERMINATOR)
                .await
                .expect("input write should succeed");
            input_writer
                .shutdown()
                .await
                .expect("input shutdown should succeed");
            drop(input_writer);

            let output = CapturedOutput::default();
            proxy_stdio_streams(socket_path.clone(), input_reader, output.clone())
                .await
                .expect("proxy-stdio should succeed");
            server_task.await.expect("server task should join");
            let mut expected = frame(b"hello cli");
            expected.extend_from_slice(&STDIO_MESSAGE_TERMINATOR);
            assert_eq!(output.bytes(), expected);
            std::fs::remove_file(&socket_path).expect("socket file should be removed");
        }

        #[tokio::test]
        async fn proxy_stdio_streams_reconnects_for_each_request() {
            let socket_path = temp_socket_path("reconnect");
            let listener =
                tokio::net::UnixListener::bind(&socket_path).expect("unix listener should bind");

            let server_task = tokio::spawn(async move {
                for (expected_request, response) in [
                    (b"first".as_slice(), b"alpha".as_slice()),
                    (b"second".as_slice(), b"beta".as_slice()),
                ] {
                    let (mut stream, _) = listener.accept().await.expect("accept should succeed");
                    let request = read_socket_request(&mut stream).await;
                    assert_eq!(request, frame(expected_request));
                    stream
                        .write_all(&frame(response))
                        .await
                        .expect("server should write response");
                    stream
                        .shutdown()
                        .await
                        .expect("server shutdown should succeed");
                }
            });

            let (mut input_writer, input_reader) = tokio::io::duplex(128);
            for payload in [b"first".as_slice(), b"second".as_slice()] {
                input_writer
                    .write_all(&frame(payload))
                    .await
                    .expect("input write should succeed");
                input_writer
                    .write_all(&STDIO_MESSAGE_TERMINATOR)
                    .await
                    .expect("terminator write should succeed");
            }
            input_writer
                .shutdown()
                .await
                .expect("input shutdown should succeed");
            drop(input_writer);

            let output = CapturedOutput::default();
            proxy_stdio_streams(socket_path.clone(), input_reader, output.clone())
                .await
                .expect("proxy-stdio should succeed");
            server_task.await.expect("server task should join");

            let mut expected = frame(b"alpha");
            expected.extend_from_slice(&STDIO_MESSAGE_TERMINATOR);
            expected.extend_from_slice(&frame(b"beta"));
            expected.extend_from_slice(&STDIO_MESSAGE_TERMINATOR);
            assert_eq!(output.bytes(), expected);
            std::fs::remove_file(&socket_path).expect("socket file should be removed");
        }

        #[tokio::test]
        async fn read_stdio_message_rejects_oversized_request_before_payload_allocation() {
            let oversized_len = PROXY_STDIO_MAX_REQUEST_BYTES - 3;
            let mut encoded = Vec::new();
            encoded.extend_from_slice(&(oversized_len as u32).to_be_bytes());
            let (mut writer, mut reader) = tokio::io::duplex(16);
            writer
                .write_all(&encoded)
                .await
                .expect("oversized header write should succeed");

            let err = read_stdio_message(&mut reader)
                .await
                .expect_err("oversized request must be rejected");

            assert!(err.to_string().contains("exceeds max size"));
        }
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
            for descriptor in BUILTIN_NATIVE_PLUGIN_DESCRIPTORS {
                assert!(
                    registry.has_plugin(descriptor.package_name),
                    "missing builtin plugin {}",
                    descriptor.package_name
                );
            }
        }

        #[test]
        fn builtin_registry_can_be_filtered_to_default_plugins() {
            let registry = builtin_native_plugin_registry().expect("registry should build");
            let filtered = registry
                .filtered(|package_name| {
                    BUILTIN_NATIVE_PLUGIN_DESCRIPTORS.iter().any(|descriptor| {
                        descriptor.default_enabled && descriptor.package_name == package_name
                    })
                })
                .expect("filtered registry should build");

            assert!(filtered.has_plugin("imago:admin"));
            assert!(filtered.has_plugin("imago:node"));
            assert!(!filtered.has_plugin("imago:experimental-gpio"));
            assert!(!filtered.has_plugin("imago:experimental-i2c"));
            assert!(!filtered.has_plugin("imago:usb"));
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
            for descriptor in BUILTIN_NATIVE_PLUGIN_DESCRIPTORS {
                assert!(
                    registry.has_plugin(descriptor.package_name),
                    "missing builtin plugin {}",
                    descriptor.package_name
                );
            }
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
