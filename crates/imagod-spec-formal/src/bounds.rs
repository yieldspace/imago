use nirvash_macros::FiniteModelDomain as FormalFiniteModelDomain;

use crate::{CommandErrorKind, CommandKind, CommandLifecycleState, PluginKind, RunnerAppType};

pub const MAX_SERVICES: u8 = 2;
pub const MAX_SESSIONS: u8 = 2;
pub const MAX_RUNNERS: u8 = 2;
pub const MAX_ARTIFACT_CHUNKS: u8 = 2;
pub const MAX_PLUGIN_DEPENDENCIES: u8 = 3;
pub const MAX_HTTP_QUEUE_DEPTH: u8 = 2;
pub const MAX_EPOCH_TICKS: u8 = 3;
pub const MAX_TIME_STEPS: u8 = 4;

pub const SPEC_COMMAND_TYPES: [CommandKind; 3] =
    [CommandKind::Deploy, CommandKind::Run, CommandKind::Stop];
pub const SPEC_COMMAND_STATES: [CommandLifecycleState; 5] = [
    CommandLifecycleState::Accepted,
    CommandLifecycleState::Running,
    CommandLifecycleState::Succeeded,
    CommandLifecycleState::Failed,
    CommandLifecycleState::Canceled,
];
pub const SPEC_ERROR_CODES: [CommandErrorKind; 14] = [
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
];
pub const SPEC_RUNNER_APP_TYPES: [RunnerAppType; 4] = [
    RunnerAppType::Cli,
    RunnerAppType::Rpc,
    RunnerAppType::Http,
    RunnerAppType::Socket,
];
pub const SPEC_PLUGIN_KINDS: [PluginKind; 2] = [PluginKind::Native, PluginKind::Wasm];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, FormalFiniteModelDomain)]
#[finite_model_domain(range = "0..=MAX")]
pub struct BoundedU8<const MAX: u8>(u8);

impl<const MAX: u8> BoundedU8<MAX> {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_max(self) -> bool {
        self.0 == MAX
    }

    pub fn saturating_inc(self) -> Self {
        Self(self.0.saturating_add(1).min(MAX))
    }

    pub fn saturating_dec(self) -> Self {
        Self(self.0.saturating_sub(1))
    }
}

impl<const MAX: u8> nirvash_lower::SymbolicEncoding for BoundedU8<MAX> {
    fn symbolic_sort() -> nirvash::SymbolicSort {
        nirvash::SymbolicSort::finite::<Self>()
    }
}

pub type ServiceSlots = BoundedU8<MAX_SERVICES>;
pub type SessionSlots = BoundedU8<MAX_SESSIONS>;
pub type RunnerSlots = BoundedU8<MAX_RUNNERS>;
pub type ArtifactChunks = BoundedU8<MAX_ARTIFACT_CHUNKS>;
pub type PluginDependencySlots = BoundedU8<MAX_PLUGIN_DEPENDENCIES>;
pub type HttpQueueDepth = BoundedU8<MAX_HTTP_QUEUE_DEPTH>;
pub type EpochTicks = BoundedU8<MAX_EPOCH_TICKS>;
pub type TimeSteps = BoundedU8<MAX_TIME_STEPS>;

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_lower::FiniteModelDomain;

    #[test]
    fn bounded_u8_domain_matches_declared_max() {
        let values = HttpQueueDepth::bounded_domain().into_vec();
        assert_eq!(values.len(), usize::from(MAX_HTTP_QUEUE_DEPTH) + 1);
        assert_eq!(
            values.last().map(|value| value.get()),
            Some(MAX_HTTP_QUEUE_DEPTH)
        );
    }

    #[test]
    fn public_case_tables_cover_current_public_variants() {
        assert_eq!(SPEC_COMMAND_TYPES.len(), 3);
        assert_eq!(SPEC_COMMAND_STATES.len(), 5);
        assert_eq!(SPEC_RUNNER_APP_TYPES.len(), 4);
        assert_eq!(SPEC_ERROR_CODES.len(), 14);
        assert_eq!(SPEC_PLUGIN_KINDS.len(), 2);
    }
}
