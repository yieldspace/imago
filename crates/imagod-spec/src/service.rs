//! Canonical service lifecycle fragments.

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
pub enum ServiceLifecyclePhase {
    Absent,
    Prepared,
    Committed,
    Promoted,
    Running,
    Stopping,
    Reaped,
}

impl ServiceLifecyclePhase {
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    pub const fn is_present(self) -> bool {
        !matches!(self, Self::Absent)
    }
}
