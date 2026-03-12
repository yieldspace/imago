use crate::{
    BoolExpr, DocGraphPolicy, Fairness, Ltl, ModelBackend, ModelCheckConfig, StepExpr,
    SymmetryReducer, TransitionProgram, VizPolicy,
};

pub trait ActionVocabulary: Sized {
    fn action_vocabulary() -> Vec<Self>;
}

pub trait TransitionSystem {
    type State: Clone + std::fmt::Debug + Eq + 'static;
    type Action: Clone + std::fmt::Debug + Eq + 'static;

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State>;

    fn actions(&self) -> Vec<Self::Action>;

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match self.transition_program() {
            Some(program) => {
                assert!(
                    program.is_ast_native(),
                    "transition program `{}` for spec `{}` must be AST-native; use nirvash_transition_program! instead of TransitionRule::new/UpdateProgram::new",
                    program.name(),
                    self.name()
                );
                match program.evaluate(state, action) {
                    Ok(next) => next,
                    Err(error) => panic!(
                        "transition program `{}` is ambiguous: {:?}",
                        program.name(),
                        error
                    ),
                }
            }
            None => None,
        }
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        None
    }

    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State> {
        match self.transition_program() {
            Some(program) => {
                assert!(
                    program.is_ast_native(),
                    "transition program `{}` for spec `{}` must be AST-native; use nirvash_transition_program! instead of TransitionRule::new/UpdateProgram::new",
                    program.name(),
                    self.name()
                );
                program
                    .successors(state, action)
                    .into_iter()
                    .map(|successor| successor.into_next())
                    .collect()
            }
            None => self.transition(state, action).into_iter().collect(),
        }
    }

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        self.actions()
            .into_iter()
            .flat_map(|action| {
                self.transition_relation(state, &action)
                    .into_iter()
                    .map(move |next| (action.clone(), next))
            })
            .collect()
    }

    fn successors_constrained(
        &self,
        state: &Self::State,
        action_allowed: &dyn Fn(&Self::Action, &Self::State) -> bool,
    ) -> Vec<(Self::Action, Self::State)> {
        self.successors(state)
            .into_iter()
            .filter(|(action, next)| action_allowed(action, next))
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
        self.transition_relation(prev, action)
            .into_iter()
            .any(|candidate_next| candidate_next == *next)
    }

    fn allow_stutter(&self) -> bool {
        true
    }

    fn stutter_state(&self, state: &Self::State) -> Self::State {
        state.clone()
    }
}

pub trait TemporalSpec: TransitionSystem {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>>;

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

    fn default_model_backend(&self) -> Option<ModelBackend> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct ModelCase<S, A> {
    label: &'static str,
    state_constraints: Vec<BoolExpr<S>>,
    action_constraints: Vec<StepExpr<S, A>>,
    symmetry: Option<SymmetryReducer<S>>,
    checker_config: ModelCheckConfig,
    check_deadlocks: bool,
    doc_checker_config: Option<ModelCheckConfig>,
    doc_graph_policy: DocGraphPolicy<S>,
    viz_policy: Option<VizPolicy>,
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
            viz_policy: None,
        }
    }

    pub const fn label(&self) -> &'static str {
        self.label
    }

    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    pub fn with_state_constraint(mut self, constraint: BoolExpr<S>) -> Self {
        self.state_constraints.push(constraint);
        self
    }

    pub fn with_action_constraint(mut self, constraint: StepExpr<S, A>) -> Self {
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

    pub fn with_viz_policy(mut self, viz_policy: VizPolicy) -> Self {
        self.viz_policy = Some(viz_policy);
        self
    }

    pub fn with_resolved_backend(mut self, default_backend: ModelBackend) -> Self {
        self.checker_config.backend = self.checker_config.backend.or(Some(default_backend));
        if let Some(mut doc_checker_config) = self.doc_checker_config {
            doc_checker_config.backend = doc_checker_config.backend.or(self.checker_config.backend);
            self.doc_checker_config = Some(doc_checker_config);
        }
        self
    }

    pub fn state_constraints(&self) -> &[BoolExpr<S>] {
        &self.state_constraints
    }

    pub fn action_constraints(&self) -> &[StepExpr<S, A>] {
        &self.action_constraints
    }

    pub fn symmetry(&self) -> Option<SymmetryReducer<S>> {
        self.symmetry
    }

    pub fn checker_config(&self) -> ModelCheckConfig {
        self.checker_config.clone()
    }

    pub const fn check_deadlocks(&self) -> bool {
        self.check_deadlocks
    }

    pub fn effective_checker_config(&self) -> ModelCheckConfig {
        let mut config = self.checker_config.clone();
        config.check_deadlocks = self.check_deadlocks;
        config
    }

    pub fn doc_checker_config(&self) -> Option<ModelCheckConfig> {
        self.doc_checker_config.clone()
    }

    pub fn doc_graph_policy(&self) -> &DocGraphPolicy<S> {
        &self.doc_graph_policy
    }

    pub fn viz_policy(&self) -> VizPolicy {
        self.viz_policy.clone().unwrap_or_default()
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
    type SummaryState;
    type Context;

    async fn observe_state(&self, context: &Self::Context) -> Self::SummaryState;
}

#[derive(Debug, Clone)]
pub struct SystemComposition<S, A> {
    name: &'static str,
    subsystems: Vec<&'static str>,
    invariants: Vec<BoolExpr<S>>,
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

    pub fn with_invariant(mut self, invariant: BoolExpr<S>) -> Self {
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

    pub fn invariants(&self) -> &[BoolExpr<S>] {
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
