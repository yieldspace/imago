//! Runtime abstraction for runner-side component execution.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use imagod_common::ImagodError;
use imagod_ipc::{CapabilityPolicy, PluginDependency, RunnerAppType, RunnerSocketConfig};
use tokio::sync::{oneshot, watch};

#[derive(Debug, Clone, Default)]
/// Registry of native plugin symbols that can be injected by custom imagod builds.
pub struct NativePluginRegistry {
    plugins: Arc<BTreeMap<String, BTreeSet<String>>>,
}

impl NativePluginRegistry {
    /// Returns true when a plugin name is registered.
    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    /// Returns true when a registered plugin declares a callable symbol.
    pub fn has_symbol(&self, plugin: &str, symbol: &str) -> bool {
        self.plugins
            .get(plugin)
            .map(|symbols| symbols.contains(symbol))
            .unwrap_or(false)
    }
}

#[derive(Debug, Default)]
/// Builder API used by custom imagod binaries to register native plugins.
pub struct NativePluginRegistryBuilder {
    plugins: BTreeMap<String, BTreeSet<String>>,
}

impl NativePluginRegistryBuilder {
    /// Creates an empty native plugin builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one plugin and its exported symbol names.
    pub fn register_plugin<I, S>(&mut self, plugin_name: impl Into<String>, symbols: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let name = plugin_name.into();
        let entry = self.plugins.entry(name).or_default();
        for symbol in symbols {
            let symbol = symbol.into();
            if !symbol.is_empty() {
                entry.insert(symbol);
            }
        }
        self
    }

    /// Finalizes the registry.
    pub fn build(self) -> NativePluginRegistry {
        NativePluginRegistry {
            plugins: Arc::new(self.plugins),
        }
    }
}

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
    /// Socket runtime settings when `app_type=socket`.
    pub socket: Option<RunnerSocketConfig>,
    /// Plugin dependencies resolved from manifest and prepared by manager.
    pub plugin_dependencies: Vec<PluginDependency>,
    /// App-level capability policy used by runtime bridge authorization.
    pub capabilities: CapabilityPolicy,
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
