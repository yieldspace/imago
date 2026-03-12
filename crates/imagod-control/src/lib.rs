//! Manager-side control plane state and orchestration primitives.

pub use nirvash::conformance::{ActionApplier, StateObserver};

/// Artifact upload session and committed artifact storage.
pub mod artifact_store;
/// In-memory lifecycle state tracking for in-flight commands.
pub mod operation_state;
/// Deploy/run/stop orchestration over stored releases and supervisor control.
pub mod orchestrator;
/// Service process supervisor and manager-runner control-plane handlers.
pub mod service_supervisor;

#[cfg(feature = "bench-internals")]
/// Benchmark-only internals exposed for Criterion benches.
pub mod bench_internals {
    pub use crate::service_supervisor::bench_internals::*;
}

/// Re-export of the artifact storage entry point.
pub use artifact_store::ArtifactStore;
/// Re-export for command operation state management.
pub use operation_state::OperationManager;
/// Re-export of deployment/run orchestration entry point.
pub use orchestrator::{Orchestrator, RestoreActiveServicesSummary, RestoreFailure};
/// Re-exports for service supervision primitives.
pub use service_supervisor::{
    RunningStatus, ServiceLaunch, ServiceLogEvent, ServiceLogSnapshot, ServiceLogStream,
    ServiceLogSubscription, ServiceSupervisor,
};
