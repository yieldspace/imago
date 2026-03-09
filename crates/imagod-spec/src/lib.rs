pub mod artifact_deploy;
pub mod bounds;
pub mod command_protocol;
pub mod manager_shell;
pub mod model;
pub mod plugin_capability;
pub mod plugin_capability_relational;
pub mod runner_bootstrap;
pub mod runner_runtime;
pub mod service_supervision;
pub mod session_transport;
pub mod shutdown_flow;
pub mod system;

#[cfg(test)]
mod toy_model_controls;

pub use system::{ImagodSystemAction, ImagodSystemSpec, ImagodSystemState};
