//! Canonical manager lifecycle fragments.

use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum ManagerPhase {
    Booting,
    ConfigReady,
    Restoring,
    Listening,
    ShutdownRequested,
    Stopped,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum ManagerShutdownPhase {
    Idle,
    SignalReceived,
    DrainingSessions,
    StoppingServices,
    StoppingMaintenance,
    Completed,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum MaintenancePhase {
    Running,
    Stopping,
    Stopped,
}
