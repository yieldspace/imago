//! Runtime abstraction for runner-side component execution.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

use imagod_common::ImagodError;
use imagod_ipc::RunnerAppType;
use tokio::sync::watch;

/// Owned run request passed from runner process to runtime implementation.
#[derive(Debug)]
pub struct RuntimeRunRequest {
    /// Component execution model derived from manifest `type`.
    pub app_type: RunnerAppType,
    /// Absolute component path.
    pub component_path: PathBuf,
    /// Runtime arguments.
    pub args: Vec<String>,
    /// Runtime environment variables.
    pub envs: BTreeMap<String, String>,
    /// Shutdown signal observed by runtime implementation.
    pub shutdown: watch::Receiver<bool>,
    /// Epoch tick interval used for interruption-aware runtimes.
    pub epoch_tick_interval_ms: u64,
}

/// Boxed async result for runtime execution methods.
pub type RuntimeRunFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ImagodError>> + Send + 'a>>;

/// Runtime abstraction so runner can swap out concrete wasm engines.
pub trait ComponentRuntime: Send + Sync {
    /// Validates that the component can be loaded by this runtime.
    fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError>;

    /// Executes one component until completion or shutdown.
    fn run_component<'a>(&'a self, request: RuntimeRunRequest) -> RuntimeRunFuture<'a>;
}
