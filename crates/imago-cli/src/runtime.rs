use std::{
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Context;
use tokio::task_local;

use crate::commands::deploy::network::TargetConnector as _;
use crate::commands::{
    build,
    deploy::{self, network},
    ui::{UiMode, UiState, detect_mode},
};

#[doc(hidden)]
pub use crate::commands::deploy::network::{LocalProxyTargetConnector, SshTargetConnector};

#[doc(hidden)]
pub trait OutputSink: Send + Sync {
    fn write_stdout(&self, bytes: &[u8]) -> anyhow::Result<()>;
    fn write_stderr(&self, bytes: &[u8]) -> anyhow::Result<()>;
}

#[doc(hidden)]
#[derive(Debug, Default)]
pub struct StdioOutputSink;

impl OutputSink for StdioOutputSink {
    fn write_stdout(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(bytes)
            .context("failed to write buffered stdout")
    }

    fn write_stderr(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut stderr = io::stderr().lock();
        stderr
            .write_all(bytes)
            .context("failed to write buffered stderr")
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferedOutput {
    pub stdout: String,
    pub stderr: String,
}

impl BufferedOutput {
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

#[derive(Debug, Default)]
struct BufferedOutputBytes {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[doc(hidden)]
#[derive(Debug, Default)]
pub struct BufferedOutputSink {
    inner: Mutex<BufferedOutputBytes>,
}

impl BufferedOutputSink {
    pub fn snapshot(&self) -> BufferedOutput {
        self.inner
            .lock()
            .map(|guard| BufferedOutput {
                stdout: String::from_utf8_lossy(&guard.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&guard.stderr).into_owned(),
            })
            .unwrap_or_default()
    }
}

impl OutputSink for BufferedOutputSink {
    fn write_stdout(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock buffered stdout"))?;
        guard.stdout.extend_from_slice(bytes);
        Ok(())
    }

    fn write_stderr(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("failed to lock buffered stderr"))?;
        guard.stderr.extend_from_slice(bytes);
        Ok(())
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct CliRuntime {
    project_root: PathBuf,
    target_connector: Arc<dyn network::TargetConnector>,
    output_sink: Arc<dyn OutputSink>,
    ui_state: Arc<Mutex<UiState>>,
}

impl std::fmt::Debug for CliRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliRuntime")
            .field("project_root", &self.project_root)
            .field("ui_mode", &self.ui_mode())
            .finish()
    }
}

impl CliRuntime {
    pub fn new(
        project_root: impl Into<PathBuf>,
        ui_mode: UiMode,
        target_connector: Arc<dyn network::TargetConnector>,
        output_sink: Arc<dyn OutputSink>,
    ) -> Self {
        Self {
            project_root: project_root.into(),
            target_connector,
            output_sink,
            ui_state: Arc::new(Mutex::new(UiState::new(ui_mode))),
        }
    }

    pub fn production(project_root: impl AsRef<Path>) -> Self {
        Self::new(
            project_root.as_ref(),
            detect_mode(),
            Arc::new(network::SshTargetConnector),
            Arc::new(StdioOutputSink),
        )
    }

    pub fn plain(
        project_root: impl AsRef<Path>,
        target_connector: Arc<dyn network::TargetConnector>,
        output_sink: Arc<dyn OutputSink>,
    ) -> Self {
        Self::new(
            project_root.as_ref(),
            UiMode::Plain,
            target_connector,
            output_sink,
        )
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn ui_mode(&self) -> UiMode {
        self.ui_state
            .lock()
            .map(|guard| guard.mode())
            .unwrap_or_else(|_| detect_mode())
    }
}

task_local! {
    static CURRENT_RUNTIME: Arc<CliRuntime>;
}

pub async fn scope<R, F>(runtime: Arc<CliRuntime>, future: F) -> R
where
    F: Future<Output = R>,
{
    CURRENT_RUNTIME.scope(runtime, future).await
}

pub(crate) fn current() -> Option<Arc<CliRuntime>> {
    CURRENT_RUNTIME.try_with(Arc::clone).ok()
}

pub(crate) fn with_ui_state<R>(f: impl FnOnce(&mut UiState) -> R) -> Option<R> {
    let runtime = current()?;
    let mut guard = runtime.ui_state.lock().ok()?;
    Some(f(&mut guard))
}

pub(crate) async fn connect_target(
    target: &build::DeployTargetConfig,
) -> anyhow::Result<deploy::ConnectedTargetSession> {
    match current() {
        Some(runtime) => runtime.target_connector.connect(target).await,
        None => network::SshTargetConnector.connect(target).await,
    }
}

pub(crate) fn target_connector() -> Arc<dyn network::TargetConnector> {
    current()
        .map(|runtime| runtime.target_connector.clone())
        .unwrap_or_else(|| Arc::new(network::SshTargetConnector))
}

pub(crate) fn write_stdout(bytes: &[u8]) -> anyhow::Result<()> {
    match current() {
        Some(runtime) => runtime.output_sink.write_stdout(bytes),
        None => StdioOutputSink.write_stdout(bytes),
    }
}

pub(crate) fn write_stdout_line(line: &str) -> anyhow::Result<()> {
    write_stdout(format!("{line}\n").as_bytes())
}

pub(crate) fn write_stderr(bytes: &[u8]) -> anyhow::Result<()> {
    match current() {
        Some(runtime) => runtime.output_sink.write_stderr(bytes),
        None => StdioOutputSink.write_stderr(bytes),
    }
}

pub(crate) fn write_stderr_line(line: &str) -> anyhow::Result<()> {
    write_stderr(format!("{line}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::deploy::network;

    fn plain_runtime(output_sink: Arc<dyn OutputSink>) -> Arc<CliRuntime> {
        Arc::new(CliRuntime::plain(
            Path::new("."),
            Arc::new(network::SshTargetConnector),
            output_sink,
        ))
    }

    fn run_in_runtime(runtime: Arc<CliRuntime>, action: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(scope(runtime, action));
    }

    #[test]
    fn buffered_output_combined_concatenates_stdout_and_stderr() {
        let output = BufferedOutput {
            stdout: "out".to_string(),
            stderr: "err".to_string(),
        };

        assert_eq!(output.combined(), "outerr");
    }

    #[test]
    fn plain_runtime_uses_plain_ui_mode() {
        let runtime = plain_runtime(Arc::new(BufferedOutputSink::default()));
        assert_eq!(runtime.ui_mode(), UiMode::Plain);
    }

    #[test]
    fn runtime_scope_routes_output_through_buffered_sink() {
        let output_sink = Arc::new(BufferedOutputSink::default());
        let runtime = plain_runtime(output_sink.clone());

        run_in_runtime(runtime, async {
            write_stdout_line("stdout line").expect("stdout should write");
            write_stderr_line("stderr line").expect("stderr should write");
        });

        let output = output_sink.snapshot();
        assert_eq!(output.stdout, "stdout line\n");
        assert_eq!(output.stderr, "stderr line\n");
    }

    #[test]
    fn buffered_output_sink_decodes_split_utf8_only_on_snapshot() {
        let output_sink = BufferedOutputSink::default();
        output_sink
            .write_stdout(&[0xe3, 0x81])
            .expect("stdout should accept partial utf-8");
        output_sink
            .write_stdout(&[0x82])
            .expect("stdout should accept trailing utf-8");

        let output = output_sink.snapshot();
        assert_eq!(output.stdout, "あ");
        assert_eq!(output.stderr, "");
    }
}
