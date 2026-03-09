//! Shared command-domain contracts used by runtime, spec, and wire adapters.

use nirvash_core::{ActionVocabulary, BoundedDomain, Signature};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// High-level command category accepted by the manager runtime.
pub enum CommandKind {
    /// Starts an artifact deployment command.
    Deploy,
    /// Starts a runtime invocation command.
    Run,
    /// Starts a stop or termination command.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Lifecycle phase around the spawn race tracked by `OperationManager`.
pub enum OperationPhase {
    Starting,
    Spawned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    FinishFailed(CommandErrorKind),
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

impl Signature for CommandKind {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Deploy, Self::Run, Self::Stop])
    }
}

impl Signature for CommandLifecycleState {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            Self::Accepted,
            Self::Running,
            Self::Succeeded,
            Self::Failed,
            Self::Canceled,
        ])
    }
}

impl Signature for CommandErrorKind {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            Self::Unauthorized,
            Self::BadRequest,
            Self::BadManifest,
            Self::Busy,
            Self::NotFound,
            Self::Internal,
            Self::IdempotencyConflict,
            Self::RangeInvalid,
            Self::ChunkHashMismatch,
            Self::ArtifactIncomplete,
            Self::PreconditionFailed,
            Self::OperationTimeout,
            Self::RollbackFailed,
            Self::StorageQuota,
        ])
    }
}

impl Signature for OperationPhase {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Starting, Self::Spawned])
    }
}

impl Signature for CommandProtocolStageId {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            Self::CommandStart,
            Self::OperationState,
            Self::StateRequest,
            Self::CommandCancel,
            Self::OperationRemove,
        ])
    }
}

impl ActionVocabulary for CommandProtocolAction {
    fn action_vocabulary() -> Vec<Self> {
        vec![
            Self::Start(CommandKind::Deploy),
            Self::Start(CommandKind::Run),
            Self::Start(CommandKind::Stop),
            Self::SetRunning,
            Self::RequestCancel,
            Self::SnapshotRunning,
            Self::MarkSpawned,
            Self::FinishSucceeded,
            Self::FinishFailed(CommandErrorKind::Internal),
            Self::FinishFailed(CommandErrorKind::Busy),
            Self::FinishCanceled,
            Self::Remove,
        ]
    }
}

impl Signature for CommandProtocolAction {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(Self::action_vocabulary())
    }
}
