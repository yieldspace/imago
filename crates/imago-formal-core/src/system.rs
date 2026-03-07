use crate::{
    ActionConstraint, Fairness, Ltl, ModelCheckConfig, Signature, StateConstraint, StatePredicate,
    StepPredicate, SymmetryReducer,
};

pub trait TransitionSystem {
    type State: Signature;
    type Action: Signature;

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn init(&self, state: &Self::State) -> bool;

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool;

    fn initial_states(&self) -> Vec<Self::State> {
        Self::State::bounded_domain()
            .filter(|state| self.init(state))
            .into_vec()
    }

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        let mut values = Vec::new();
        for action in Self::Action::bounded_domain().into_vec() {
            for next in Self::State::bounded_domain().into_vec() {
                if self.next(state, &action, &next) {
                    values.push((action.clone(), next));
                }
            }
        }
        values
    }

    fn enabled(
        &self,
        state: &Self::State,
        predicate: StepPredicate<Self::State, Self::Action>,
    ) -> bool {
        self.successors(state)
            .into_iter()
            .any(|(action, next)| predicate.eval(state, &action, &next))
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

    fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>>;

    fn state_constraints(&self) -> Vec<StateConstraint<Self::State>> {
        Vec::new()
    }

    fn action_constraints(&self) -> Vec<ActionConstraint<Self::State, Self::Action>> {
        Vec::new()
    }

    fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
        self.progress_properties()
    }

    fn progress_properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
        self.properties()
    }

    fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
        Vec::new()
    }

    fn symmetry(&self) -> Option<SymmetryReducer<Self::State>> {
        None
    }

    fn checker_config(&self) -> ModelCheckConfig {
        ModelCheckConfig::default()
    }
}

#[derive(Debug, Clone)]
pub struct SystemComposition<S, A> {
    name: &'static str,
    subsystems: Vec<&'static str>,
    invariants: Vec<StatePredicate<S>>,
    illegal_transitions: Vec<StepPredicate<S, A>>,
    state_constraints: Vec<StateConstraint<S>>,
    action_constraints: Vec<ActionConstraint<S, A>>,
    properties: Vec<Ltl<S, A>>,
    fairness: Vec<Fairness<S, A>>,
    symmetry: Option<SymmetryReducer<S>>,
    checker_config: ModelCheckConfig,
}

impl<S, A> SystemComposition<S, A> {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            subsystems: Vec::new(),
            invariants: Vec::new(),
            illegal_transitions: Vec::new(),
            state_constraints: Vec::new(),
            action_constraints: Vec::new(),
            properties: Vec::new(),
            fairness: Vec::new(),
            symmetry: None,
            checker_config: ModelCheckConfig::default(),
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

    pub fn with_illegal_transition(mut self, transition: StepPredicate<S, A>) -> Self {
        self.illegal_transitions.push(transition);
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

    pub fn with_property(mut self, property: Ltl<S, A>) -> Self {
        self.properties.push(property);
        self
    }

    pub fn with_progress_property(self, property: Ltl<S, A>) -> Self {
        self.with_property(property)
    }

    pub fn with_fairness(mut self, fairness: Fairness<S, A>) -> Self {
        self.fairness.push(fairness);
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

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn subsystems(&self) -> &[&'static str] {
        &self.subsystems
    }

    pub fn invariants(&self) -> &[StatePredicate<S>] {
        &self.invariants
    }

    pub fn illegal_transitions(&self) -> &[StepPredicate<S, A>] {
        &self.illegal_transitions
    }

    pub fn state_constraints(&self) -> &[StateConstraint<S>] {
        &self.state_constraints
    }

    pub fn action_constraints(&self) -> &[ActionConstraint<S, A>] {
        &self.action_constraints
    }

    pub fn properties(&self) -> &[Ltl<S, A>] {
        &self.properties
    }

    pub fn progress_properties(&self) -> &[Ltl<S, A>] {
        self.properties()
    }

    pub fn fairness(&self) -> &[Fairness<S, A>] {
        &self.fairness
    }

    pub fn symmetry(&self) -> Option<SymmetryReducer<S>> {
        self.symmetry
    }

    pub const fn checker_config(&self) -> ModelCheckConfig {
        self.checker_config
    }
}
