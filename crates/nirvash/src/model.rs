use crate::Trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplorationMode {
    ReachableGraph,
    BoundedLasso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelBackend {
    Explicit,
    Symbolic,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum TrustTier {
    Exact,
    CertifiedReduction,
    ClaimedReduction,
    Heuristic,
}

impl TrustTier {
    pub fn join(self, other: Self) -> Self {
        if self >= other { self } else { other }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CounterexampleMinimization {
    None,
    ShortestTrace,
}

/// Current explicit-backend state storage strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitStateStorage {
    /// Exact in-memory state storage keyed by full state equality.
    InMemoryExact,
    /// In-memory storage indexed by a stable fingerprint with equality fallback on collisions.
    InMemoryFingerprinted,
}

/// Current explicit-backend state compression strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ExplicitStateCompression {
    /// Keep explicit states in memory as full values.
    #[default]
    None,
    /// Store explicit states as stable indices into `T::State::bounded_domain()`.
    DomainIndex,
}

/// Current explicit-backend reachable-graph exploration strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitReachabilityStrategy {
    /// Single-process breadth-first graph exploration.
    BreadthFirst,
    /// Multi-threaded breadth-first exploration that expands each frontier in parallel.
    ParallelFrontier,
    /// Shard-assigned breadth-first exploration that merges frontiers by deterministic shard owner.
    DistributedFrontier,
}

/// Current explicit-backend bounded-lasso search strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitBoundedLassoStrategy {
    /// Enumerate bounded prefixes and close lassos by exact state revisit.
    EnumeratedPaths,
}

/// Parallel frontier exploration settings for the explicit reachable-graph backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitParallelOptions {
    pub workers: usize,
}

impl ExplicitParallelOptions {
    pub const fn current() -> Self {
        Self { workers: 1 }
    }

    pub const fn with_workers(mut self, workers: usize) -> Self {
        self.workers = if workers == 0 { 1 } else { workers };
        self
    }
}

impl Default for ExplicitParallelOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Distributed frontier exploration settings for the explicit reachable-graph backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitDistributedOptions {
    pub shards: usize,
}

impl ExplicitDistributedOptions {
    pub const fn current() -> Self {
        Self { shards: 1 }
    }

    pub const fn with_shards(mut self, shards: usize) -> Self {
        self.shards = if shards == 0 { 1 } else { shards };
        self
    }
}

impl Default for ExplicitDistributedOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Checkpoint configuration for explicit reachable-graph exploration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitCheckpointOptions {
    pub path: Option<String>,
    pub save_every_frontiers: usize,
    pub resume: bool,
}

impl ExplicitCheckpointOptions {
    pub const fn disabled() -> Self {
        Self {
            path: None,
            save_every_frontiers: 1,
            resume: false,
        }
    }

    pub fn at_path(path: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
            save_every_frontiers: 1,
            resume: true,
        }
    }

    pub fn with_save_every_frontiers(mut self, save_every_frontiers: usize) -> Self {
        self.save_every_frontiers = save_every_frontiers.max(1);
        self
    }

    pub const fn with_resume(mut self, resume: bool) -> Self {
        self.resume = resume;
        self
    }
}

impl Default for ExplicitCheckpointOptions {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Configuration for explicit simulation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitSimulationOptions {
    pub runs: usize,
    pub max_depth: usize,
    pub seed: u64,
}

impl ExplicitSimulationOptions {
    pub const fn current() -> Self {
        Self {
            runs: 1,
            max_depth: 32,
            seed: 0,
        }
    }

    pub const fn new(runs: usize, max_depth: usize, seed: u64) -> Self {
        Self {
            runs,
            max_depth,
            seed,
        }
    }
}

impl Default for ExplicitSimulationOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Backend-specific knobs for the explicit model checker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitModelCheckOptions {
    pub state_storage: ExplicitStateStorage,
    pub compression: ExplicitStateCompression,
    pub reachability: ExplicitReachabilityStrategy,
    pub bounded_lasso: ExplicitBoundedLassoStrategy,
    pub checkpoint: ExplicitCheckpointOptions,
    pub parallel: ExplicitParallelOptions,
    pub distributed: ExplicitDistributedOptions,
    pub simulation: ExplicitSimulationOptions,
}

impl ExplicitModelCheckOptions {
    pub const fn current() -> Self {
        Self {
            state_storage: ExplicitStateStorage::InMemoryExact,
            compression: ExplicitStateCompression::None,
            reachability: ExplicitReachabilityStrategy::BreadthFirst,
            bounded_lasso: ExplicitBoundedLassoStrategy::EnumeratedPaths,
            checkpoint: ExplicitCheckpointOptions::disabled(),
            parallel: ExplicitParallelOptions::current(),
            distributed: ExplicitDistributedOptions::current(),
            simulation: ExplicitSimulationOptions::current(),
        }
    }

    pub const fn with_state_storage(mut self, state_storage: ExplicitStateStorage) -> Self {
        self.state_storage = state_storage;
        self
    }

    pub const fn with_compression(mut self, compression: ExplicitStateCompression) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_checkpoint(mut self, checkpoint: ExplicitCheckpointOptions) -> Self {
        self.checkpoint = checkpoint;
        self
    }

    pub const fn with_reachability(mut self, reachability: ExplicitReachabilityStrategy) -> Self {
        self.reachability = reachability;
        self
    }

    pub const fn with_parallel(mut self, parallel: ExplicitParallelOptions) -> Self {
        self.parallel = parallel;
        self
    }

    pub const fn with_distributed(mut self, distributed: ExplicitDistributedOptions) -> Self {
        self.distributed = distributed;
        self
    }

    pub const fn with_simulation(mut self, simulation: ExplicitSimulationOptions) -> Self {
        self.simulation = simulation;
        self
    }
}

impl Default for ExplicitModelCheckOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Current relational symbolic bridge solving strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RelationalBridgeStrategy {
    /// Enumerate successors by repeatedly solving the transition relation with blocking clauses.
    SolverEnumeration,
}

/// Settings for the relational symbolic bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RelationalBridgeOptions {
    pub strategy: RelationalBridgeStrategy,
}

impl RelationalBridgeOptions {
    pub const fn current() -> Self {
        Self {
            strategy: RelationalBridgeStrategy::SolverEnumeration,
        }
    }

    pub const fn with_strategy(mut self, strategy: RelationalBridgeStrategy) -> Self {
        self.strategy = strategy;
        self
    }
}

impl Default for RelationalBridgeOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Current symbolic-backend temporal engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolicTemporalEngine {
    /// Search temporal counterexamples by direct SMT bounded-lasso encoding.
    BoundedLasso,
    /// Search temporal counterexamples via a liveness-to-safety reduction.
    LivenessToSafety,
}

/// Current symbolic-backend safety engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolicSafetyEngine {
    /// Search safety counterexamples by direct SMT bounded unrolling.
    Bmc,
    /// Prove invariants with bounded base cases plus inductive step checks.
    KInduction,
    /// Prove invariants with property-directed blocking over symbolic predecessor queries.
    PdrIc3,
}

/// Settings for symbolic k-induction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolicKInductionOptions {
    /// Maximum induction depth. `0` means auto from the symbolic state-space bound.
    pub max_depth: usize,
}

impl SymbolicKInductionOptions {
    pub const fn current() -> Self {
        Self { max_depth: 0 }
    }

    pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }
}

impl Default for SymbolicKInductionOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Settings for symbolic PDR/IC3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolicPdrOptions {
    /// Maximum number of frames to build. `0` means auto from the symbolic state-space bound.
    pub max_frames: usize,
}

impl SymbolicPdrOptions {
    pub const fn current() -> Self {
        Self { max_frames: 0 }
    }

    pub const fn with_max_frames(mut self, max_frames: usize) -> Self {
        self.max_frames = max_frames;
        self
    }
}

impl Default for SymbolicPdrOptions {
    fn default() -> Self {
        Self::current()
    }
}

/// Backend-specific knobs for the symbolic model checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolicModelCheckOptions {
    pub bridge: RelationalBridgeOptions,
    pub temporal: SymbolicTemporalEngine,
    pub safety: SymbolicSafetyEngine,
    pub k_induction: SymbolicKInductionOptions,
    pub pdr: SymbolicPdrOptions,
}

impl SymbolicModelCheckOptions {
    pub const fn current() -> Self {
        Self {
            bridge: RelationalBridgeOptions::current(),
            temporal: SymbolicTemporalEngine::BoundedLasso,
            safety: SymbolicSafetyEngine::Bmc,
            k_induction: SymbolicKInductionOptions::current(),
            pdr: SymbolicPdrOptions::current(),
        }
    }

    pub const fn with_bridge(mut self, bridge: RelationalBridgeOptions) -> Self {
        self.bridge = bridge;
        self
    }

    pub const fn with_temporal(mut self, temporal: SymbolicTemporalEngine) -> Self {
        self.temporal = temporal;
        self
    }

    pub const fn with_safety(mut self, safety: SymbolicSafetyEngine) -> Self {
        self.safety = safety;
        self
    }

    pub const fn with_k_induction(mut self, k_induction: SymbolicKInductionOptions) -> Self {
        self.k_induction = k_induction;
        self
    }

    pub const fn with_pdr(mut self, pdr: SymbolicPdrOptions) -> Self {
        self.pdr = pdr;
        self
    }
}

impl Default for SymbolicModelCheckOptions {
    fn default() -> Self {
        Self::current()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelCheckConfig {
    pub backend: Option<ModelBackend>,
    pub exploration: ExplorationMode,
    pub bounded_depth: Option<usize>,
    pub max_states: Option<usize>,
    pub max_transitions: Option<usize>,
    pub check_deadlocks: bool,
    pub stop_on_first_violation: bool,
    pub counterexample_minimization: CounterexampleMinimization,
    pub explicit: ExplicitModelCheckOptions,
    pub symbolic: SymbolicModelCheckOptions,
}

impl ModelCheckConfig {
    pub const fn reachable_graph() -> Self {
        Self {
            backend: None,
            exploration: ExplorationMode::ReachableGraph,
            bounded_depth: None,
            max_states: None,
            max_transitions: None,
            check_deadlocks: true,
            stop_on_first_violation: true,
            counterexample_minimization: CounterexampleMinimization::ShortestTrace,
            explicit: ExplicitModelCheckOptions::current(),
            symbolic: SymbolicModelCheckOptions::current(),
        }
    }

    pub const fn bounded_lasso(depth: usize) -> Self {
        Self {
            backend: None,
            exploration: ExplorationMode::BoundedLasso,
            bounded_depth: Some(depth),
            max_states: None,
            max_transitions: None,
            check_deadlocks: true,
            stop_on_first_violation: true,
            counterexample_minimization: CounterexampleMinimization::ShortestTrace,
            explicit: ExplicitModelCheckOptions::current(),
            symbolic: SymbolicModelCheckOptions::current(),
        }
    }

    pub fn with_explicit_options(mut self, explicit: ExplicitModelCheckOptions) -> Self {
        self.explicit = explicit;
        self
    }

    pub const fn with_symbolic_options(mut self, symbolic: SymbolicModelCheckOptions) -> Self {
        self.symbolic = symbolic;
        self
    }

    pub const fn with_counterexample_minimization(
        mut self,
        counterexample_minimization: CounterexampleMinimization,
    ) -> Self {
        self.counterexample_minimization = counterexample_minimization;
        self
    }
}

impl Default for ModelCheckConfig {
    fn default() -> Self {
        Self::reachable_graph()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCheckError {
    UnsupportedConfiguration(&'static str),
    ExplorationLimitReached { states: usize, transitions: usize },
    NoInitialStates,
    CheckpointIo(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterexampleKind {
    Invariant,
    Deadlock,
    StateConstraint,
    ActionConstraint,
    Property,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counterexample<S, A> {
    pub kind: CounterexampleKind,
    pub name: String,
    pub trace: Trace<S, A>,
    pub trust_tier: TrustTier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCheckResult<S, A> {
    violations: Vec<Counterexample<S, A>>,
    trust_tier: TrustTier,
}

impl<S, A> ModelCheckResult<S, A> {
    pub fn ok() -> Self {
        Self {
            violations: Vec::new(),
            trust_tier: TrustTier::Exact,
        }
    }

    pub fn with_tier(trust_tier: TrustTier) -> Self {
        Self {
            violations: Vec::new(),
            trust_tier,
        }
    }

    pub fn with_violation(violation: Counterexample<S, A>) -> Self {
        Self {
            trust_tier: violation.trust_tier,
            violations: vec![violation],
        }
    }

    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn violations(&self) -> &[Counterexample<S, A>] {
        &self.violations
    }

    pub const fn trust_tier(&self) -> TrustTier {
        self.trust_tier
    }

    pub fn push(&mut self, violation: Counterexample<S, A>) {
        self.trust_tier = self.trust_tier.join(violation.trust_tier);
        self.violations.push(violation);
    }

    pub fn extend(&mut self, other: Self) {
        self.trust_tier = self.trust_tier.join(other.trust_tier);
        self.violations.extend(other.violations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachable_graph_defaults_match_current_backend_strategies() {
        let config = ModelCheckConfig::reachable_graph();

        assert_eq!(config.explicit, ExplicitModelCheckOptions::current());
        assert_eq!(config.symbolic, SymbolicModelCheckOptions::current());
        assert_eq!(config.exploration, ExplorationMode::ReachableGraph);
        assert_eq!(config.bounded_depth, None);
        assert_eq!(
            config.counterexample_minimization,
            CounterexampleMinimization::ShortestTrace
        );
    }

    #[test]
    fn bounded_lasso_preserves_backend_specific_defaults() {
        let config = ModelCheckConfig::bounded_lasso(7);

        assert_eq!(config.bounded_depth, Some(7));
        assert_eq!(config.explicit, ExplicitModelCheckOptions::current());
        assert_eq!(config.symbolic, SymbolicModelCheckOptions::current());
    }

    #[test]
    fn backend_specific_options_can_be_overridden_independently() {
        let explicit = ExplicitModelCheckOptions {
            state_storage: ExplicitStateStorage::InMemoryFingerprinted,
            compression: ExplicitStateCompression::DomainIndex,
            reachability: ExplicitReachabilityStrategy::BreadthFirst,
            bounded_lasso: ExplicitBoundedLassoStrategy::EnumeratedPaths,
            checkpoint: ExplicitCheckpointOptions::at_path("tmp/nirvash-checkpoint.json")
                .with_save_every_frontiers(2),
            parallel: ExplicitParallelOptions::current().with_workers(4),
            distributed: ExplicitDistributedOptions::current().with_shards(3),
            simulation: ExplicitSimulationOptions::new(4, 12, 7),
        };
        let symbolic = SymbolicModelCheckOptions {
            bridge: RelationalBridgeOptions::current(),
            temporal: SymbolicTemporalEngine::BoundedLasso,
            safety: SymbolicSafetyEngine::PdrIc3,
            k_induction: SymbolicKInductionOptions::current().with_max_depth(6),
            pdr: SymbolicPdrOptions::current().with_max_frames(9),
        };

        let config = ModelCheckConfig::reachable_graph()
            .with_explicit_options(explicit.clone())
            .with_symbolic_options(symbolic);

        assert_eq!(config.explicit, explicit);
        assert_eq!(config.symbolic, symbolic);
    }

    #[test]
    fn explicit_checkpoint_options_default_to_disabled() {
        assert_eq!(
            ExplicitCheckpointOptions::default(),
            ExplicitCheckpointOptions {
                path: None,
                save_every_frontiers: 1,
                resume: false,
            }
        );
    }

    #[test]
    fn explicit_parallel_and_distributed_options_clamp_to_positive_sizes() {
        assert_eq!(
            ExplicitParallelOptions::current().with_workers(0),
            ExplicitParallelOptions { workers: 1 }
        );
        assert_eq!(
            ExplicitDistributedOptions::current().with_shards(0),
            ExplicitDistributedOptions { shards: 1 }
        );
    }

    #[test]
    fn explicit_state_compression_defaults_to_none_and_can_be_overridden() {
        assert_eq!(
            ExplicitModelCheckOptions::current().compression,
            ExplicitStateCompression::None
        );
        assert_eq!(
            ExplicitModelCheckOptions::current()
                .with_compression(ExplicitStateCompression::DomainIndex)
                .compression,
            ExplicitStateCompression::DomainIndex
        );
    }

    #[test]
    fn counterexample_minimization_can_be_disabled() {
        let config = ModelCheckConfig::reachable_graph()
            .with_counterexample_minimization(CounterexampleMinimization::None);

        assert_eq!(
            config.counterexample_minimization,
            CounterexampleMinimization::None
        );
    }
}
