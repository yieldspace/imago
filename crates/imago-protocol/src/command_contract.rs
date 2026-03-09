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
    Start(#[sig(domain = command_protocol_command_kind_vocabulary)] CommandKind),
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
    FinishFailed(#[sig(domain = command_protocol_error_kind_vocabulary)] CommandErrorKind),
    /// Finish canceled
    FinishCanceled,
    /// Remove command
    Remove,
}

fn command_protocol_command_kind_vocabulary() -> Vec<CommandKind> {
    vec![CommandKind::Deploy, CommandKind::Run, CommandKind::Stop]
}

fn command_protocol_error_kind_vocabulary() -> Vec<CommandErrorKind> {
    vec![CommandErrorKind::Internal, CommandErrorKind::Busy]
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
