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

/// Current explicit-backend state storage strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitStateStorage {
    /// Exact in-memory state storage keyed by full state equality.
    InMemoryExact,
    /// In-memory storage indexed by a stable fingerprint with equality fallback on collisions.
    InMemoryFingerprinted,
}

/// Current explicit-backend reachable-graph exploration strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitReachabilityStrategy {
    /// Single-process breadth-first graph exploration.
    BreadthFirst,
}

/// Current explicit-backend bounded-lasso search strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExplicitBoundedLassoStrategy {
    /// Enumerate bounded prefixes and close lassos by exact state revisit.
    EnumeratedPaths,
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
    pub reachability: ExplicitReachabilityStrategy,
    pub bounded_lasso: ExplicitBoundedLassoStrategy,
    pub checkpoint: ExplicitCheckpointOptions,
    pub simulation: ExplicitSimulationOptions,
}

impl ExplicitModelCheckOptions {
    pub const fn current() -> Self {
        Self {
            state_storage: ExplicitStateStorage::InMemoryExact,
            reachability: ExplicitReachabilityStrategy::BreadthFirst,
            bounded_lasso: ExplicitBoundedLassoStrategy::EnumeratedPaths,
            checkpoint: ExplicitCheckpointOptions::disabled(),
            simulation: ExplicitSimulationOptions::current(),
        }
    }

    pub const fn with_state_storage(mut self, state_storage: ExplicitStateStorage) -> Self {
        self.state_storage = state_storage;
        self
    }

    pub fn with_checkpoint(mut self, checkpoint: ExplicitCheckpointOptions) -> Self {
        self.checkpoint = checkpoint;
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

/// Current symbolic-backend successor solving strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolicSuccessorStrategy {
    /// Enumerate successors by repeatedly solving the transition relation with blocking clauses.
    SolverEnumeration,
}

/// Current symbolic-backend bounded-lasso encoding strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolicBoundedLassoEncoding {
    /// Unroll bounded lasso states directly into SMT with a loop selector.
    DirectSmt,
}

/// Backend-specific knobs for the symbolic model checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolicModelCheckOptions {
    pub successors: SymbolicSuccessorStrategy,
    pub bounded_lasso: SymbolicBoundedLassoEncoding,
}

impl SymbolicModelCheckOptions {
    pub const fn current() -> Self {
        Self {
            successors: SymbolicSuccessorStrategy::SolverEnumeration,
            bounded_lasso: SymbolicBoundedLassoEncoding::DirectSmt,
        }
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCheckResult<S, A> {
    violations: Vec<Counterexample<S, A>>,
}

impl<S, A> ModelCheckResult<S, A> {
    pub fn ok() -> Self {
        Self {
            violations: Vec::new(),
        }
    }

    pub fn with_violation(violation: Counterexample<S, A>) -> Self {
        Self {
            violations: vec![violation],
        }
    }

    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn violations(&self) -> &[Counterexample<S, A>] {
        &self.violations
    }

    pub fn push(&mut self, violation: Counterexample<S, A>) {
        self.violations.push(violation);
    }

    pub fn extend(&mut self, other: Self) {
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
            reachability: ExplicitReachabilityStrategy::BreadthFirst,
            bounded_lasso: ExplicitBoundedLassoStrategy::EnumeratedPaths,
            checkpoint: ExplicitCheckpointOptions::at_path("tmp/nirvash-checkpoint.json")
                .with_save_every_frontiers(2),
            simulation: ExplicitSimulationOptions::new(4, 12, 7),
        };
        let symbolic = SymbolicModelCheckOptions {
            successors: SymbolicSuccessorStrategy::SolverEnumeration,
            bounded_lasso: SymbolicBoundedLassoEncoding::DirectSmt,
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
}
