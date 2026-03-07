//! Shared source-of-truth model types for `imagod` internal coordination.

pub mod command;

pub use command::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction,
    CommandProtocolContext, CommandProtocolObservedState, CommandProtocolOutput,
    CommandProtocolStageId, OperationPhase,
};
