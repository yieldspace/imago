//! Runtime abstraction for runner-side component execution.

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

use imagod_common::ImagodError;
use imagod_ipc::RunnerAppType;
use tokio::sync::{oneshot, watch};

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
    /// Optional signal sent when HTTP runtime initialization has completed.
    pub http_ready_tx: Option<oneshot::Sender<()>>,
}

/// Boxed async result for runtime execution methods.
pub type RuntimeRunFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ImagodError>> + Send + 'a>>;

/// Runtime-agnostic HTTP request model passed from ingress to wasm runtime.
#[derive(Debug, Clone)]
pub struct RuntimeHttpRequest {
    /// Upper-case HTTP method (e.g. `GET`).
    pub method: String,
    /// Request URI as received by ingress (path or absolute URI).
    pub uri: String,
    /// HTTP headers represented as raw bytes.
    pub headers: Vec<(String, Vec<u8>)>,
    /// Entire request body.
    pub body: Vec<u8>,
}

/// Runtime-agnostic HTTP response model returned from wasm runtime.
#[derive(Debug, Clone)]
pub struct RuntimeHttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// HTTP headers represented as raw bytes.
    pub headers: Vec<(String, Vec<u8>)>,
    /// Entire response body.
    pub body: Vec<u8>,
}

/// Boxed async result for runtime HTTP handling.
pub type RuntimeHttpFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RuntimeHttpResponse, ImagodError>> + Send + 'a>>;

/// Runtime abstraction so runner can swap out concrete wasm engines.
pub trait ComponentRuntime: Send + Sync {
    /// Validates that the component can be loaded by this runtime.
    fn validate_component(&self, component_path: &Path) -> Result<(), ImagodError>;

    /// Executes one component until completion or shutdown.
    fn run_component<'a>(&'a self, request: RuntimeRunRequest) -> RuntimeRunFuture<'a>;

    /// Handles one HTTP request using the already-running HTTP component.
    fn handle_http_request<'a>(&'a self, request: RuntimeHttpRequest) -> RuntimeHttpFuture<'a>;
}
