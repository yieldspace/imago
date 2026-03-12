//! Shared command-domain contracts used by runtime, spec, and wire adapters.

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, nirvash_macros::Signature)]
/// High-level command category accepted by the manager runtime.
pub enum CommandKind {
    /// Starts an artifact deployment command.
    Deploy,
    /// Starts a runtime invocation command.
    Run,
    /// Starts a stop or termination command.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, nirvash_macros::Signature)]
/// Internal command lifecycle state tracked by the manager runtime.
pub enum CommandLifecycleState {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl CommandLifecycleState {
    /// Returns whether this lifecycle state is terminal.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, nirvash_macros::Signature)]
/// Stable command-domain error classes used by runtime and conformance tests.
pub enum CommandErrorKind {
    /// Rejects unauthenticated command requests.
    Unauthorized,
    /// Rejects malformed command payloads.
    BadRequest,
    /// Rejects invalid manifests during deploy.
    BadManifest,
    /// Rejects commands while the manager is busy.
    Busy,
    /// Rejects requests that reference an unknown command.
    NotFound,
    /// Rejects requests after an unexpected internal failure.
    Internal,
    /// Rejects duplicate idempotency keys with mismatched intent.
    IdempotencyConflict,
    /// Rejects invalid chunk range requests.
    RangeInvalid,
    /// Rejects artifact chunks whose digest does not match.
    ChunkHashMismatch,
    /// Rejects incomplete uploaded artifacts.
    ArtifactIncomplete,
    /// Rejects commands whose preconditions are not met.
    PreconditionFailed,
    /// Rejects commands that exceeded their runtime timeout.
    OperationTimeout,
    /// Rejects commands when rollback could not complete.
    RollbackFailed,
    /// Rejects commands when storage quota is exhausted.
    StorageQuota,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, nirvash_macros::Signature)]
/// Lifecycle phase around the spawn race tracked by `OperationManager`.
pub enum OperationPhase {
    Starting,
    Spawned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, nirvash_macros::Signature)]
/// Stable stage identifiers used for rejected command-protocol actions.
pub enum CommandProtocolStageId {
    CommandStart,
    OperationState,
    StateRequest,
    CommandCancel,
    OperationRemove,
}

impl CommandProtocolStageId {
    /// Returns the wire-facing stage string used in protocol errors.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::CommandStart => "command.start",
            Self::OperationState => "operation.state",
            Self::StateRequest => "state.request",
            Self::CommandCancel => "command.cancel",
            Self::OperationRemove => "operation.remove",
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, nirvash_macros::Signature, nirvash_macros::ActionVocabulary,
)]
/// Shared command action vocabulary applied by `OperationManager`.
pub enum CommandProtocolAction {
    /// Start command
    Start(CommandKind),
    /// Mark running
    SetRunning,
    /// Request cancel
    RequestCancel,
    /// Observe running
    SnapshotRunning,
    /// Mark spawned
    MarkSpawned,
    /// Finish succeeded
    FinishSucceeded,
    /// Finish failed
    FinishFailed(#[sig(domain = finish_failed_error_domain)] CommandErrorKind),
    /// Finish canceled
    FinishCanceled,
    /// Remove command
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Execution context bound to a single logical command request.
pub struct CommandProtocolContext {
    pub request_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Runtime output returned by the command protocol action contract.
pub enum CommandProtocolOutput {
    Ack,
    StateSnapshot {
        state: CommandLifecycleState,
        stage: String,
        updated_at_unix_secs: u64,
    },
    CancelResponse {
        cancellable: bool,
        final_state: CommandLifecycleState,
    },
    SpawnResult {
        spawned: bool,
        canceled: bool,
    },
    Rejected {
        code: CommandErrorKind,
        stage: CommandProtocolStageId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Minimal observed runtime state used by formal conformance tests.
pub struct CommandProtocolObservedState {
    pub tracked: bool,
    pub lifecycle_state: Option<CommandLifecycleState>,
    pub cancel_requested: bool,
    pub phase: Option<OperationPhase>,
}

impl CommandProtocolObservedState {
    /// Returns the missing-operation observation used for absent request ids.
    pub const fn missing() -> Self {
        Self {
            tracked: false,
            lifecycle_state: None,
            cancel_requested: false,
            phase: None,
        }
    }
}

fn finish_failed_error_domain() -> Vec<CommandErrorKind> {
    vec![CommandErrorKind::Internal, CommandErrorKind::Busy]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash::{ActionVocabulary, Signature};

    #[test]
    fn command_signature_domains_match_expected_order() {
        assert_eq!(
            CommandKind::bounded_domain().into_vec(),
            vec![CommandKind::Deploy, CommandKind::Run, CommandKind::Stop]
        );
        assert_eq!(
            CommandLifecycleState::bounded_domain().into_vec(),
            vec![
                CommandLifecycleState::Accepted,
                CommandLifecycleState::Running,
                CommandLifecycleState::Succeeded,
                CommandLifecycleState::Failed,
                CommandLifecycleState::Canceled,
            ]
        );
        assert_eq!(
            CommandErrorKind::bounded_domain().into_vec(),
            vec![
                CommandErrorKind::Unauthorized,
                CommandErrorKind::BadRequest,
                CommandErrorKind::BadManifest,
                CommandErrorKind::Busy,
                CommandErrorKind::NotFound,
                CommandErrorKind::Internal,
                CommandErrorKind::IdempotencyConflict,
                CommandErrorKind::RangeInvalid,
                CommandErrorKind::ChunkHashMismatch,
                CommandErrorKind::ArtifactIncomplete,
                CommandErrorKind::PreconditionFailed,
                CommandErrorKind::OperationTimeout,
                CommandErrorKind::RollbackFailed,
                CommandErrorKind::StorageQuota,
            ]
        );
        assert_eq!(
            OperationPhase::bounded_domain().into_vec(),
            vec![OperationPhase::Starting, OperationPhase::Spawned]
        );
        assert_eq!(
            CommandProtocolStageId::bounded_domain().into_vec(),
            vec![
                CommandProtocolStageId::CommandStart,
                CommandProtocolStageId::OperationState,
                CommandProtocolStageId::StateRequest,
                CommandProtocolStageId::CommandCancel,
                CommandProtocolStageId::OperationRemove,
            ]
        );
    }

    #[test]
    fn command_action_vocabulary_matches_expected_subset() {
        assert_eq!(
            CommandProtocolAction::action_vocabulary(),
            vec![
                CommandProtocolAction::Start(CommandKind::Deploy),
                CommandProtocolAction::Start(CommandKind::Run),
                CommandProtocolAction::Start(CommandKind::Stop),
                CommandProtocolAction::SetRunning,
                CommandProtocolAction::RequestCancel,
                CommandProtocolAction::SnapshotRunning,
                CommandProtocolAction::MarkSpawned,
                CommandProtocolAction::FinishSucceeded,
                CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
                CommandProtocolAction::FinishFailed(CommandErrorKind::Busy),
                CommandProtocolAction::FinishCanceled,
                CommandProtocolAction::Remove,
            ]
        );
        assert_eq!(
            CommandProtocolAction::bounded_domain().into_vec(),
            CommandProtocolAction::action_vocabulary()
        );
    }
}
