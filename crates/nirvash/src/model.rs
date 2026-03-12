use crate::Trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorationMode {
    ReachableGraph,
    BoundedLasso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelBackend {
    Explicit,
    Symbolic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCheckConfig {
    pub backend: Option<ModelBackend>,
    pub exploration: ExplorationMode,
    pub bounded_depth: Option<usize>,
    pub max_states: Option<usize>,
    pub max_transitions: Option<usize>,
    pub check_deadlocks: bool,
    pub stop_on_first_violation: bool,
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
        }
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
