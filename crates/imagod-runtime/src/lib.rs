//! Runner-side execution runtime and process bootstrap entrypoints.

/// Runner bootstrap path that starts from stdin-delivered metadata.
pub mod runner_process;
/// Wasmtime-based component execution runtime.
pub mod runtime_wasmtime;

/// Runs `imagod` in runner mode using bootstrap data read from stdin.
pub use runner_process::run_runner_from_stdin;
/// Runner runtime wrapper around a shared Wasmtime engine.
pub use runtime_wasmtime::WasmRuntime;
