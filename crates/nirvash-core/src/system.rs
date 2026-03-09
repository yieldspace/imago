use crate::{
    ActionConstraint, DocGraphPolicy, Fairness, Ltl, ModelCheckConfig, StateConstraint,
    StatePredicate, SymmetryReducer,
};

pub trait TransitionSystem {
    type State: Clone + std::fmt::Debug + Eq + 'static;
    type Action: Clone + std::fmt::Debug + Eq + 'static;

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State>;

    fn actions(&self) -> Vec<Self::Action>;

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State>;

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        self.actions()
            .into_iter()
            .filter_map(|action| self.transition(state, &action).map(|next| (action, next)))
            .collect()
    }

    fn contains_initial(&self, state: &Self::State) -> bool {
        self.initial_states()
            .iter()
            .any(|candidate| candidate == state)
    }

    fn contains_transition(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: &Self::State,
    ) -> bool {
        self.transition(prev, action)
            .is_some_and(|candidate_next| candidate_next == *next)
    }

    fn allow_stutter(&self) -> bool {
        true
    }

    fn stutter_state(&self, state: &Self::State) -> Self::State {
        state.clone()
    }
}

pub trait TemporalSpec: TransitionSystem {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>>;

    fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
        Vec::new()
    }

    fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
        Vec::new()
    }
}

pub trait ModelCaseSource: TransitionSystem {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default()]
    }
}

#[derive(Debug, Clone)]
pub struct ModelCase<S, A> {
    label: &'static str,
    state_constraints: Vec<StateConstraint<S>>,
    action_constraints: Vec<ActionConstraint<S, A>>,
    symmetry: Option<SymmetryReducer<S>>,
    checker_config: ModelCheckConfig,
    check_deadlocks: bool,
    doc_checker_config: Option<ModelCheckConfig>,
    doc_graph_policy: DocGraphPolicy<S>,
}

impl<S, A> ModelCase<S, A> {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            state_constraints: Vec::new(),
            action_constraints: Vec::new(),
            symmetry: None,
            checker_config: ModelCheckConfig::default(),
            check_deadlocks: true,
            doc_checker_config: None,
            doc_graph_policy: DocGraphPolicy::default(),
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    pub fn with_state_constraint(mut self, constraint: StateConstraint<S>) -> Self {
        self.state_constraints.push(constraint);
        self
    }

    pub fn with_action_constraint(mut self, constraint: ActionConstraint<S, A>) -> Self {
        self.action_constraints.push(constraint);
        self
    }

    pub fn with_symmetry(mut self, symmetry: SymmetryReducer<S>) -> Self {
        self.symmetry = Some(symmetry);
        self
    }

    pub fn with_checker_config(mut self, config: ModelCheckConfig) -> Self {
        self.checker_config = config;
        self
    }

    pub fn with_check_deadlocks(mut self, check_deadlocks: bool) -> Self {
        self.check_deadlocks = check_deadlocks;
        self
    }

    pub fn with_doc_checker_config(mut self, config: ModelCheckConfig) -> Self {
        self.doc_checker_config = Some(config);
        self
    }

    pub fn with_doc_graph_policy(mut self, doc_graph_policy: DocGraphPolicy<S>) -> Self {
        self.doc_graph_policy = doc_graph_policy;
        self
    }

    pub fn state_constraints(&self) -> &[StateConstraint<S>] {
        &self.state_constraints
    }

    pub fn action_constraints(&self) -> &[ActionConstraint<S, A>] {
        &self.action_constraints
    }

    pub fn symmetry(&self) -> Option<SymmetryReducer<S>> {
        self.symmetry
    }

    pub const fn checker_config(&self) -> ModelCheckConfig {
        self.checker_config
    }

    pub const fn check_deadlocks(&self) -> bool {
        self.check_deadlocks
    }

    pub fn effective_checker_config(&self) -> ModelCheckConfig {
        let mut config = self.checker_config;
        config.check_deadlocks = self.check_deadlocks;
        config
    }

    pub const fn doc_checker_config(&self) -> Option<ModelCheckConfig> {
        self.doc_checker_config
    }

    pub fn doc_graph_policy(&self) -> &DocGraphPolicy<S> {
        &self.doc_graph_policy
    }
}

impl<S, A> Default for ModelCase<S, A> {
    fn default() -> Self {
        Self::new("default")
    }
}

#[allow(async_fn_in_trait)]
pub trait ActionApplier {
    type Action;
    type Output;
    type Context;

    async fn execute_action(&self, context: &Self::Context, action: &Self::Action) -> Self::Output;
}

#[allow(async_fn_in_trait)]
pub trait StateObserver {
    type ObservedState;
    type Context;

    async fn observe_state(&self, context: &Self::Context) -> Self::ObservedState;
}

#[derive(Debug, Clone)]
pub struct SystemComposition<S, A> {
    name: &'static str,
    subsystems: Vec<&'static str>,
    invariants: Vec<StatePredicate<S>>,
    properties: Vec<Ltl<S, A>>,
    fairness: Vec<Fairness<S, A>>,
    model_cases: Vec<ModelCase<S, A>>,
}

impl<S, A> SystemComposition<S, A> {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            subsystems: Vec::new(),
            invariants: Vec::new(),
            properties: Vec::new(),
            fairness: Vec::new(),
            model_cases: Vec::new(),
        }
    }

    pub fn with_subsystem(mut self, subsystem: &'static str) -> Self {
        self.subsystems.push(subsystem);
        self
    }

    pub fn with_invariant(mut self, invariant: StatePredicate<S>) -> Self {
        self.invariants.push(invariant);
        self
    }

    pub fn with_property(mut self, property: Ltl<S, A>) -> Self {
        self.properties.push(property);
        self
    }

    pub fn with_fairness(mut self, fairness: Fairness<S, A>) -> Self {
        self.fairness.push(fairness);
        self
    }

    pub fn with_model_case(mut self, model_case: ModelCase<S, A>) -> Self {
        self.model_cases.push(model_case);
        self
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn subsystems(&self) -> &[&'static str] {
        &self.subsystems
    }

    pub fn invariants(&self) -> &[StatePredicate<S>] {
        &self.invariants
    }

    pub fn properties(&self) -> &[Ltl<S, A>] {
        &self.properties
    }

    pub fn fairness(&self) -> &[Fairness<S, A>] {
        &self.fairness
    }

    pub fn model_cases(&self) -> &[ModelCase<S, A>] {
        &self.model_cases
    }
}
