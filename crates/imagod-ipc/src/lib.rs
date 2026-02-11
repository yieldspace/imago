//! IPC primitives and transport adapters used by manager/runner processes.

/// IPC message and helper definitions.
pub mod ipc;

/// Re-exports the public IPC API at crate root.
pub use ipc::*;
