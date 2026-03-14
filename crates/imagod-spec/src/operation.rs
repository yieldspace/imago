//! Canonical command and manager-auth fragments.

pub use crate::command_contract::{CommandErrorKind, CommandKind, CommandLifecycleState};
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
pub enum ManagerAuthState {
    Missing,
    Verified,
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
pub enum CommandTerminalState {
    Succeeded,
    Failed,
    Canceled,
}

impl CommandTerminalState {
    pub const fn into_lifecycle(self) -> CommandLifecycleState {
        match self {
            Self::Succeeded => CommandLifecycleState::Succeeded,
            Self::Failed => CommandLifecycleState::Failed,
            Self::Canceled => CommandLifecycleState::Canceled,
        }
    }
}
