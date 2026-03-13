//! Lowering boundary and checker-facing public API surface for `nirvash`.

use std::fmt::{self, Debug, Display};

pub use nirvash::{
    ActionVocabulary, BoolExpr, BoolExprAst, Counterexample, CounterexampleKind,
    CounterexampleMinimization, DocGraphPolicy, ExplicitBoundedLassoStrategy,
    ExplicitCheckpointOptions, ExplicitDistributedOptions, ExplicitModelCheckOptions,
    ExplicitParallelOptions, ExplicitReachabilityStrategy, ExplicitSimulationOptions,
    ExplicitStateCompression, ExplicitStateStorage, ExplorationMode, Fairness, Ltl, ModelBackend,
    ModelCheckConfig, ModelCheckError, ModelCheckResult, ReachableGraphEdge,
    ReachableGraphSnapshot, RelationalBridgeOptions, RelationalBridgeStrategy, SpecVizBundle,
    SpecVizRegistrationSet, StateExpr, StepExpr, StepExprAst, SymbolicKInductionOptions,
    SymbolicModelCheckOptions, SymbolicPdrOptions, SymbolicSafetyEngine, SymbolicTemporalEngine,
    Trace, TraceStep, TransitionProgram, TransitionProgramError, TransitionRule,
    TransitionSuccessor, UpdateAst, UpdateOp, UpdateProgram, UpdateValueExprAst, VizPolicy,
    collect_relational_state_schema, collect_relational_state_summary,
    registry::RegisteredSymbolicStateSchema,
};
pub use nirvash_foundation::{
    BoundedDomain, ExprDomain, FiniteModelDomain, IntoBoundedDomain, OpaqueModelValue,
    SymbolicEncoding, SymbolicSort, SymbolicSortField, SymbolicStateField, SymbolicStateSchema,
    bounded_vec_domain, into_bounded_domain, lookup_symbolic_state_schema,
    normalize_symbolic_state_path, symbolic_leaf_field, symbolic_leaf_index, symbolic_leaf_value,
    symbolic_seed_value, symbolic_state_fields,
};
pub use nirvash_ir::{
    ActionExpr, BuiltinPredicateOp as IrBuiltinPredicateOp, ComparisonOp as IrComparisonOp,
    FairnessDecl, QuantifierKind as IrQuantifierKind, SpecCore, StateExpr as IrStateExpr,
    TemporalExpr, UpdateExpr as IrUpdateExpr, UpdateOpDecl as IrUpdateOpDecl,
    ValueExpr as IrValueExpr, ViewExpr,
};

#[derive(Debug)]
pub struct StateAbstraction<S> {
    name: &'static str,
    project: fn(&S) -> String,
}

impl<S> StateAbstraction<S> {
    pub const fn new(name: &'static str, project: fn(&S) -> String) -> Self {
        Self { name, project }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn project(&self, state: &S) -> String {
        (self.project)(state)
    }
}

impl<S> Clone for StateAbstraction<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for StateAbstraction<S> {}

#[derive(Debug)]
pub struct PorHints<S, A> {
    name: &'static str,
    allow_action: fn(&S, &A) -> bool,
}

impl<S, A> PorHints<S, A> {
    pub const fn new(name: &'static str, allow_action: fn(&S, &A) -> bool) -> Self {
        Self { name, allow_action }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn allow_action(&self, state: &S, action: &A) -> bool {
        (self.allow_action)(state, action)
    }
}

impl<S, A> Clone for PorHints<S, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, A> Copy for PorHints<S, A> {}

#[derive(Debug)]
pub struct SymmetrySpec<S> {
    name: &'static str,
    canonicalize: fn(&S) -> S,
}

impl<S> Clone for SymmetrySpec<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for SymmetrySpec<S> {}

impl<S> SymmetrySpec<S> {
    pub const fn new(name: &'static str, canonicalize: fn(&S) -> S) -> Self {
        Self { name, canonicalize }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn canonicalize(&self, state: &S) -> S {
        (self.canonicalize)(state)
    }
}

#[derive(Debug, Clone)]
pub struct ModelPresentationConfig<S> {
    pub doc_checker: Option<ModelCheckConfig>,
    pub doc_graph: DocGraphPolicy<S>,
    pub viz: Option<VizPolicy>,
}

impl<S> Default for ModelPresentationConfig<S> {
    fn default() -> Self {
        Self {
            doc_checker: None,
            doc_graph: DocGraphPolicy::default(),
            viz: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelInstance<S, A> {
    label: &'static str,
    state_constraints: Vec<BoolExpr<S>>,
    action_constraints: Vec<StepExpr<S, A>>,
    symmetry: Option<SymmetrySpec<S>>,
    state_abstraction: Option<StateAbstraction<S>>,
    por_hints: Option<PorHints<S, A>>,
    checker_config: ModelCheckConfig,
    check_deadlocks: bool,
    presentation: ModelPresentationConfig<S>,
}

impl<S, A> ModelInstance<S, A> {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            state_constraints: Vec::new(),
            action_constraints: Vec::new(),
            symmetry: None,
            state_abstraction: None,
            por_hints: None,
            checker_config: ModelCheckConfig::default(),
            check_deadlocks: true,
            presentation: ModelPresentationConfig::default(),
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

    pub fn with_symmetry(mut self, symmetry: SymmetrySpec<S>) -> Self {
        self.symmetry = Some(symmetry);
        self
    }

    pub fn with_state_abstraction(mut self, state_abstraction: StateAbstraction<S>) -> Self {
        self.state_abstraction = Some(state_abstraction);
        self
    }

    pub fn with_por(mut self, por: PorHints<S, A>) -> Self {
        self.por_hints = Some(por);
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
        self.presentation.doc_checker = Some(config);
        self
    }

    pub fn with_doc_graph_policy(mut self, doc_graph_policy: DocGraphPolicy<S>) -> Self {
        self.presentation.doc_graph = doc_graph_policy;
        self
    }

    pub fn with_viz_policy(mut self, viz_policy: VizPolicy) -> Self {
        self.presentation.viz = Some(viz_policy);
        self
    }

    pub fn with_resolved_backend(mut self, default_backend: ModelBackend) -> Self {
        self.checker_config.backend = self.checker_config.backend.or(Some(default_backend));
        if let Some(mut doc_checker) = self.presentation.doc_checker.take() {
            doc_checker.backend = doc_checker.backend.or(self.checker_config.backend);
            self.presentation.doc_checker = Some(doc_checker);
        }
        self
    }

    pub fn state_constraints(&self) -> &[BoolExpr<S>] {
        &self.state_constraints
    }

    pub fn action_constraints(&self) -> &[StepExpr<S, A>] {
        &self.action_constraints
    }

    pub fn symmetry(&self) -> Option<SymmetrySpec<S>> {
        self.symmetry
    }

    pub fn state_abstraction(&self) -> Option<StateAbstraction<S>> {
        self.state_abstraction
    }

    pub fn por(&self) -> Option<PorHints<S, A>> {
        self.por_hints
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
        self.presentation.doc_checker.clone()
    }

    pub fn doc_graph_policy(&self) -> &DocGraphPolicy<S> {
        &self.presentation.doc_graph
    }

    pub fn viz_policy(&self) -> VizPolicy {
        self.presentation.viz.clone().unwrap_or_default()
    }
}

impl<S, A> Default for ModelInstance<S, A> {
    fn default() -> Self {
        Self::new("default")
    }
}

#[derive(Debug, Clone)]
pub struct SystemComposition<S, A> {
    name: &'static str,
    subsystems: Vec<nirvash::RegisteredSubsystemSpec>,
    invariants: Vec<BoolExpr<S>>,
    properties: Vec<Ltl<S, A>>,
    fairness: Vec<Fairness<S, A>>,
    model_instances: Vec<ModelInstance<S, A>>,
}

impl<S, A> SystemComposition<S, A> {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            subsystems: Vec::new(),
            invariants: Vec::new(),
            properties: Vec::new(),
            fairness: Vec::new(),
            model_instances: Vec::new(),
        }
    }

    pub fn with_subsystem(mut self, subsystem: nirvash::RegisteredSubsystemSpec) -> Self {
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

    pub fn with_model_instance(mut self, model_instance: ModelInstance<S, A>) -> Self {
        self.model_instances.push(model_instance);
        self
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn subsystems(&self) -> &[nirvash::RegisteredSubsystemSpec] {
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

    pub fn model_instances(&self) -> &[ModelInstance<S, A>] {
        &self.model_instances
    }
}

type TransitionRelationFn<'a, S, A> = dyn Fn(&S, &A) -> Vec<S> + 'a;
type SuccessorsFn<'a, S, A> = dyn Fn(&S) -> Vec<(A, S)> + 'a;

pub struct ExecutableSemantics<'a, S, A> {
    initial_states: Vec<S>,
    actions: Vec<A>,
    transition_program: Option<TransitionProgram<S, A>>,
    transition_relation: Box<TransitionRelationFn<'a, S, A>>,
    successors: Box<SuccessorsFn<'a, S, A>>,
    invariants: Vec<BoolExpr<S>>,
    properties: Vec<Ltl<S, A>>,
    fairness: Vec<Fairness<S, A>>,
    default_model_backend: Option<ModelBackend>,
}

impl<'a, S, A> fmt::Debug for ExecutableSemantics<'a, S, A>
where
    S: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutableSemantics")
            .field("initial_states", &self.initial_states)
            .field("actions", &self.actions)
            .field("transition_program", &self.transition_program)
            .field("invariants", &self.invariants)
            .field("properties", &self.properties)
            .field("fairness", &self.fairness)
            .field("default_model_backend", &self.default_model_backend)
            .finish_non_exhaustive()
    }
}

impl<'a, S, A> ExecutableSemantics<'a, S, A> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        initial_states: Vec<S>,
        actions: Vec<A>,
        transition_program: Option<TransitionProgram<S, A>>,
        transition_relation: impl Fn(&S, &A) -> Vec<S> + 'a,
        successors: impl Fn(&S) -> Vec<(A, S)> + 'a,
        invariants: Vec<BoolExpr<S>>,
        properties: Vec<Ltl<S, A>>,
        fairness: Vec<Fairness<S, A>>,
        default_model_backend: Option<ModelBackend>,
    ) -> Self {
        Self {
            initial_states,
            actions,
            transition_program,
            transition_relation: Box::new(transition_relation),
            successors: Box::new(successors),
            invariants,
            properties,
            fairness,
            default_model_backend,
        }
    }
}

pub struct LoweredSpec<'a, S, A> {
    name: &'static str,
    pub core: SpecCore,
    model_instances: Vec<ModelInstance<S, A>>,
    state_ty: &'static str,
    action_ty: &'static str,
    symbolic_artifacts: SymbolicArtifacts<S, A>,
    executable: ExecutableSemantics<'a, S, A>,
}

impl<'a, S, A> fmt::Debug for LoweredSpec<'a, S, A>
where
    S: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoweredSpec")
            .field("name", &self.name)
            .field("core", &self.core)
            .field("model_instances", &self.model_instances)
            .field("state_ty", &self.state_ty)
            .field("action_ty", &self.action_ty)
            .field("symbolic_artifacts", &self.symbolic_artifacts)
            .field("executable", &self.executable)
            .finish()
    }
}

impl<'a, S, A> LoweredSpec<'a, S, A> {
    pub fn new(
        name: &'static str,
        core: SpecCore,
        model_instances: Vec<ModelInstance<S, A>>,
        state_ty: &'static str,
        action_ty: &'static str,
        symbolic_artifacts: SymbolicArtifacts<S, A>,
        executable: ExecutableSemantics<'a, S, A>,
    ) -> Self {
        Self {
            name,
            core,
            model_instances,
            state_ty,
            action_ty,
            symbolic_artifacts,
            executable,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn state_ty(&self) -> &'static str {
        self.state_ty
    }

    pub const fn action_ty(&self) -> &'static str {
        self.action_ty
    }

    pub fn symbolic_artifacts(&self) -> &SymbolicArtifacts<S, A> {
        &self.symbolic_artifacts
    }

    pub fn executable(&self) -> &ExecutableSemantics<'a, S, A> {
        &self.executable
    }

    pub fn model_instances(&self) -> Vec<ModelInstance<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.model_instances.clone()
    }

    pub fn initial_states(&self) -> Vec<S>
    where
        S: Clone,
    {
        self.executable.initial_states.clone()
    }

    pub fn actions(&self) -> Vec<A>
    where
        A: Clone,
    {
        self.executable.actions.clone()
    }

    pub fn transition_program(&self) -> Option<TransitionProgram<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.executable.transition_program.clone()
    }

    pub fn transition_relation(&self, state: &S, action: &A) -> Vec<S> {
        (self.executable.transition_relation)(state, action)
    }

    pub fn successors(&self, state: &S) -> Vec<(A, S)> {
        (self.executable.successors)(state)
    }

    pub fn contains_initial(&self, state: &S) -> bool
    where
        S: PartialEq + Clone,
    {
        self.initial_states()
            .into_iter()
            .any(|candidate| candidate == *state)
    }

    pub fn contains_transition(&self, prev: &S, action: &A, next: &S) -> bool
    where
        S: PartialEq,
    {
        self.transition_relation(prev, action)
            .into_iter()
            .any(|candidate_next| candidate_next == *next)
    }

    pub fn invariants(&self) -> Vec<BoolExpr<S>>
    where
        S: Clone,
    {
        self.executable.invariants.clone()
    }

    pub fn properties(&self) -> Vec<Ltl<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.executable.properties.clone()
    }

    pub fn fairness(&self) -> Vec<Fairness<S, A>>
    where
        S: Clone,
        A: Clone,
    {
        self.executable.fairness.clone()
    }

    pub const fn default_model_backend(&self) -> Option<ModelBackend> {
        self.executable.default_model_backend
    }

    pub const fn frontend_name(&self) -> &'static str {
        self.name
    }
}

#[derive(Debug, Default)]
pub struct LoweringCx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringError {
    spec_name: String,
    node_kind: &'static str,
    label: String,
    backend_fragment: Option<&'static str>,
    detail: String,
}

impl LoweringError {
    pub fn unsupported(
        spec_name: impl Into<String>,
        node_kind: &'static str,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            spec_name: spec_name.into(),
            node_kind,
            label: label.into(),
            backend_fragment: None,
            detail: detail.into(),
        }
    }

    pub fn unsupported_fragment(
        spec_name: impl Into<String>,
        node_kind: &'static str,
        label: impl Into<String>,
        backend_fragment: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            spec_name: spec_name.into(),
            node_kind,
            label: label.into(),
            backend_fragment: Some(backend_fragment),
            detail: detail.into(),
        }
    }

    pub fn spec_name(&self) -> &str {
        &self.spec_name
    }

    pub const fn node_kind(&self) -> &'static str {
        self.node_kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn backend_fragment(&self) -> Option<&'static str> {
        self.backend_fragment
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl Display for LoweringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(fragment) = self.backend_fragment {
            write!(
                f,
                "{} lowering failed for {} `{}` in `{}`: {}",
                self.spec_name, self.node_kind, self.label, fragment, self.detail
            )
        } else {
            write!(
                f,
                "{} lowering failed for {} `{}`: {}",
                self.spec_name, self.node_kind, self.label, self.detail
            )
        }
    }
}

impl std::error::Error for LoweringError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicSupportIssue {
    spec_name: String,
    node_kind: &'static str,
    label: String,
    backend_fragment: &'static str,
    detail: String,
}

impl SymbolicSupportIssue {
    pub fn new(
        spec_name: impl Into<String>,
        node_kind: &'static str,
        label: impl Into<String>,
        backend_fragment: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            spec_name: spec_name.into(),
            node_kind,
            label: label.into(),
            backend_fragment,
            detail: detail.into(),
        }
    }

    pub fn spec_name(&self) -> &str {
        &self.spec_name
    }

    pub const fn node_kind(&self) -> &'static str {
        self.node_kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn backend_fragment(&self) -> &'static str {
        self.backend_fragment
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl Display for SymbolicSupportIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "symbolic backend cannot lower {} `{}` in spec `{}` to `{}`: {}",
            self.node_kind, self.label, self.spec_name, self.backend_fragment, self.detail
        )
    }
}

#[derive(Debug, Clone)]
pub struct SymbolicArtifacts<S, A> {
    state_schema: Option<SymbolicStateSchema<S>>,
    transition_program: Option<TransitionProgram<S, A>>,
    invariants: Vec<BoolExpr<S>>,
    properties: Vec<Ltl<S, A>>,
    fairness: Vec<Fairness<S, A>>,
    issues: Vec<SymbolicSupportIssue>,
}

impl<S, A> SymbolicArtifacts<S, A> {
    pub fn new(
        state_schema: Option<SymbolicStateSchema<S>>,
        transition_program: Option<TransitionProgram<S, A>>,
        invariants: Vec<BoolExpr<S>>,
        properties: Vec<Ltl<S, A>>,
        fairness: Vec<Fairness<S, A>>,
        issues: Vec<SymbolicSupportIssue>,
    ) -> Self {
        Self {
            state_schema,
            transition_program,
            invariants,
            properties,
            fairness,
            issues,
        }
    }

    pub fn state_schema(&self) -> Option<&SymbolicStateSchema<S>> {
        self.state_schema.as_ref()
    }

    pub fn transition_program(&self) -> Option<&TransitionProgram<S, A>> {
        self.transition_program.as_ref()
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

    pub fn issues(&self) -> &[SymbolicSupportIssue] {
        &self.issues
    }

    pub fn first_issue_for_fragment(
        &self,
        backend_fragment: &'static str,
    ) -> Option<&SymbolicSupportIssue> {
        self.issues
            .iter()
            .find(|issue| issue.backend_fragment == backend_fragment)
    }
}

fn named_ref(name: impl Into<String>) -> IrStateExpr {
    IrStateExpr::Ref(name.into())
}

fn definition(name: impl Into<String>, body: impl Into<String>) -> nirvash_ir::Definition {
    nirvash_ir::Definition {
        name: name.into(),
        body: body.into(),
    }
}

pub trait FrontendSpec {
    type State: Clone + Debug + Eq + 'static;
    type Action: Clone + Debug + Eq + 'static;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State>;

    fn actions(&self) -> Vec<Self::Action>;

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match self.transition_program() {
            Some(program) => match program.evaluate(state, action) {
                Ok(next) => next,
                Err(error) => panic!(
                    "transition program `{}` is ambiguous: {:?}",
                    program.name(),
                    error
                ),
            },
            None => None,
        }
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        None
    }

    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State> {
        match self.transition_program() {
            Some(program) => program
                .successors(state, action)
                .into_iter()
                .map(|successor| successor.into_next())
                .collect(),
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

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::default()]
    }

    fn default_model_backend(&self) -> Option<ModelBackend> {
        None
    }

    fn lower<'a>(
        &'a self,
        _cx: &mut LoweringCx,
    ) -> Result<LoweredSpec<'a, Self::State, Self::Action>, LoweringError>
    where
        Self: TemporalSpec,
        Self::State: PartialEq + FiniteModelDomain,
        Self::Action: PartialEq,
    {
        let initial_states = self.initial_states();
        let actions = self.actions();
        let invariants = self.invariants();
        let properties = self.properties();
        let fairness = self.fairness();
        let default_model_backend = self.default_model_backend();
        let model_instances = self.model_instances();
        let transition_program = self.transition_program();
        let symbolic_artifacts = SymbolicArtifacts::new(
            lookup_symbolic_state_schema::<Self::State>(),
            transition_program.clone(),
            invariants.clone(),
            properties.clone(),
            fairness.clone(),
            collect_symbolic_support_issues(
                self.frontend_name(),
                transition_program.as_ref(),
                &invariants,
                &properties,
                &fairness,
                lookup_symbolic_state_schema::<Self::State>().is_some(),
            ),
        );
        let core = lower_spec_core(
            self.frontend_name(),
            &initial_states,
            &actions,
            transition_program.as_ref(),
            &invariants,
            &properties,
            &fairness,
            default_model_backend,
        )?;
        let executable = ExecutableSemantics::new(
            initial_states,
            actions,
            transition_program,
            move |state, action| self.transition_relation(state, action),
            move |state| self.successors(state),
            invariants,
            properties,
            fairness,
            default_model_backend,
        );
        Ok(LoweredSpec::new(
            self.frontend_name(),
            core,
            model_instances,
            std::any::type_name::<Self::State>(),
            std::any::type_name::<Self::Action>(),
            symbolic_artifacts,
            executable,
        ))
    }
}

pub trait TemporalSpec: FrontendSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>>;

    fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
        Vec::new()
    }

    fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
        Vec::new()
    }
}

pub trait CheckerSpec {
    type State: Clone + Debug + Eq + 'static;
    type Action: Clone + Debug + Eq + 'static;

    fn frontend_name(&self) -> &'static str;
    fn initial_states(&self) -> Vec<Self::State>;
    fn actions(&self) -> Vec<Self::Action>;
    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>>;
    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State>;
    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)>;
    fn contains_initial(&self, state: &Self::State) -> bool;
    fn contains_transition(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: &Self::State,
    ) -> bool;
    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>>;
    fn default_model_backend(&self) -> Option<ModelBackend>;
    fn invariants(&self) -> Vec<BoolExpr<Self::State>>;
    fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>>;
    fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>>;
    fn symbolic_artifacts(&self) -> &SymbolicArtifacts<Self::State, Self::Action>;
}

impl<'a, S, A> CheckerSpec for LoweredSpec<'a, S, A>
where
    S: Clone + Debug + Eq + 'static,
    A: Clone + Debug + Eq + 'static,
{
    type State = S;
    type Action = A;

    fn frontend_name(&self) -> &'static str {
        LoweredSpec::frontend_name(self)
    }

    fn initial_states(&self) -> Vec<Self::State> {
        LoweredSpec::initial_states(self)
    }

    fn actions(&self) -> Vec<Self::Action> {
        LoweredSpec::actions(self)
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        LoweredSpec::transition_program(self)
    }

    fn transition_relation(&self, state: &Self::State, action: &Self::Action) -> Vec<Self::State> {
        LoweredSpec::transition_relation(self, state, action)
    }

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        LoweredSpec::successors(self, state)
    }

    fn contains_initial(&self, state: &Self::State) -> bool {
        LoweredSpec::contains_initial(self, state)
    }

    fn contains_transition(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: &Self::State,
    ) -> bool {
        LoweredSpec::contains_transition(self, prev, action, next)
    }

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        LoweredSpec::model_instances(self)
    }

    fn default_model_backend(&self) -> Option<ModelBackend> {
        LoweredSpec::default_model_backend(self)
    }

    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        LoweredSpec::invariants(self)
    }

    fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
        LoweredSpec::properties(self)
    }

    fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
        LoweredSpec::fairness(self)
    }

    fn symbolic_artifacts(&self) -> &SymbolicArtifacts<Self::State, Self::Action> {
        LoweredSpec::symbolic_artifacts(self)
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_spec_core<S, A>(
    spec_name: &'static str,
    initial_states: &[S],
    actions: &[A],
    transition_program: Option<&TransitionProgram<S, A>>,
    invariants: &[BoolExpr<S>],
    properties: &[Ltl<S, A>],
    fairness: &[Fairness<S, A>],
    default_model_backend: Option<ModelBackend>,
) -> Result<SpecCore, LoweringError>
where
    S: Debug + 'static,
    A: Debug + 'static,
{
    let mut defs = vec![
        definition("frontend", spec_name),
        definition("state_ty", std::any::type_name::<S>()),
        definition("action_ty", std::any::type_name::<A>()),
        definition(
            "init_states",
            initial_states
                .iter()
                .map(|state| format!("{state:?}"))
                .collect::<Vec<_>>()
                .join(" | "),
        ),
        definition(
            "action_domain",
            actions
                .iter()
                .map(|action| format!("{action:?}"))
                .collect::<Vec<_>>()
                .join(" | "),
        ),
    ];
    if let Some(default_backend) = default_model_backend {
        defs.push(definition(
            "default_backend",
            format!("{default_backend:?}"),
        ));
    }
    if let Some(program) = transition_program {
        defs.push(definition("transition_program", program.name()));
        defs.extend(
            program
                .rules()
                .iter()
                .map(|rule| definition(format!("rule::{}", rule.name()), rule.name())),
        );
    } else {
        defs.push(definition(
            "transition_relation",
            format!("{spec_name}::transition_relation"),
        ));
    }
    for predicate in invariants {
        defs.push(definition(
            format!("invariant::{}", predicate.name()),
            predicate.name(),
        ));
    }
    for property in properties {
        defs.push(definition(
            format!("property::{}", property.describe()),
            property.describe(),
        ));
    }
    for fairness_decl in fairness {
        defs.push(definition(
            format!("fairness::{}", fairness_decl.name()),
            fairness_decl.name(),
        ));
    }

    let init = lower_init(spec_name, initial_states)?;
    let next = lower_next(spec_name, actions, transition_program);
    let fairness = fairness
        .iter()
        .map(|decl| lower_fairness(spec_name, decl))
        .collect::<Result<Vec<_>, _>>()?;
    let invariants = invariants
        .iter()
        .map(|predicate| lower_bool_expr(spec_name, "invariant", predicate))
        .collect::<Result<Vec<_>, _>>()?;
    let temporal_props = properties
        .iter()
        .map(|property| lower_ltl(spec_name, property))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SpecCore {
        vars: vec![nirvash_ir::VarDecl {
            name: "vars".to_owned(),
        }],
        defs,
        init,
        next,
        fairness,
        invariants,
        temporal_props,
    })
}

fn collect_symbolic_support_issues<S, A>(
    spec_name: &'static str,
    transition_program: Option<&TransitionProgram<S, A>>,
    invariants: &[BoolExpr<S>],
    properties: &[Ltl<S, A>],
    fairness: &[Fairness<S, A>],
    has_state_schema: bool,
) -> Vec<SymbolicSupportIssue>
where
    S: 'static,
    A: 'static,
{
    let mut issues = Vec::new();
    if !has_state_schema {
        issues.push(SymbolicSupportIssue::new(
            spec_name,
            "state schema",
            std::any::type_name::<S>(),
            "direct_smt.state_schema",
            "state does not provide SymbolicEncoding",
        ));
    }
    match transition_program {
        Some(program) => {
            if !program.is_ast_native() {
                issues.push(SymbolicSupportIssue::new(
                    spec_name,
                    "transition program",
                    program.name(),
                    "direct_smt.transition",
                    "transition program is not AST-native",
                ));
            } else if let Some(node) = program.first_unencodable_symbolic_node() {
                issues.push(SymbolicSupportIssue::new(
                    spec_name,
                    "transition program",
                    program.name(),
                    "direct_smt.transition",
                    format!("helper or effect `{node}` is not registered for symbolic use"),
                ));
            }
        }
        None => issues.push(SymbolicSupportIssue::new(
            spec_name,
            "transition program",
            spec_name,
            "direct_smt.transition",
            "direct SMT requires an AST-native transition program",
        )),
    }
    for invariant in invariants {
        collect_symbolic_predicate_issue(
            &mut issues,
            spec_name,
            "invariant",
            invariant.name(),
            invariant.is_ast_native(),
            invariant.first_unencodable_symbolic_node(),
            "direct_smt.invariant",
        );
    }
    for property in properties {
        collect_symbolic_predicate_issue(
            &mut issues,
            spec_name,
            "property",
            property.describe(),
            property.is_ast_native(),
            property.first_unencodable_symbolic_node(),
            "direct_smt.temporal",
        );
    }
    for fairness_decl in fairness {
        let predicate = fairness_decl.predicate();
        collect_symbolic_predicate_issue(
            &mut issues,
            spec_name,
            "fairness",
            fairness_decl.name(),
            predicate.is_ast_native(),
            predicate.first_unencodable_symbolic_node(),
            "direct_smt.fairness",
        );
    }
    issues
}

fn collect_symbolic_predicate_issue(
    issues: &mut Vec<SymbolicSupportIssue>,
    spec_name: &'static str,
    node_kind: &'static str,
    label: impl Into<String>,
    is_ast_native: bool,
    first_unencodable_symbolic_node: Option<&'static str>,
    backend_fragment: &'static str,
) {
    let label = label.into();
    if !is_ast_native {
        issues.push(SymbolicSupportIssue::new(
            spec_name,
            node_kind,
            label.clone(),
            backend_fragment,
            "node is not AST-native",
        ));
    } else if let Some(node) = first_unencodable_symbolic_node {
        issues.push(SymbolicSupportIssue::new(
            spec_name,
            node_kind,
            label,
            backend_fragment,
            format!("helper `{node}` is not registered for symbolic use"),
        ));
    }
}

fn lower_symbolic_key(symbolic: nirvash::SymbolicRegistration) -> Option<String> {
    symbolic.symbolic_key().map(str::to_owned)
}

const fn lower_quantifier_kind(kind: nirvash::QuantifierKind) -> IrQuantifierKind {
    match kind {
        nirvash::QuantifierKind::ForAll => IrQuantifierKind::ForAll,
        nirvash::QuantifierKind::Exists => IrQuantifierKind::Exists,
    }
}

const fn lower_comparison_op(op: nirvash::ComparisonOp) -> IrComparisonOp {
    match op {
        nirvash::ComparisonOp::Eq => IrComparisonOp::Eq,
        nirvash::ComparisonOp::Ne => IrComparisonOp::Ne,
        nirvash::ComparisonOp::Lt => IrComparisonOp::Lt,
        nirvash::ComparisonOp::Le => IrComparisonOp::Le,
        nirvash::ComparisonOp::Gt => IrComparisonOp::Gt,
        nirvash::ComparisonOp::Ge => IrComparisonOp::Ge,
    }
}

const fn lower_builtin_predicate_op(op: nirvash::BuiltinPredicateOp) -> IrBuiltinPredicateOp {
    match op {
        nirvash::BuiltinPredicateOp::Contains => IrBuiltinPredicateOp::Contains,
        nirvash::BuiltinPredicateOp::SubsetOf => IrBuiltinPredicateOp::SubsetOf,
    }
}

fn lower_state_value_ast<S: 'static>(ast: &nirvash::ErasedStateExprAst<S>) -> IrValueExpr {
    match ast {
        nirvash::ErasedStateExprAst::Opaque { repr } => IrValueExpr::Opaque((*repr).to_owned()),
        nirvash::ErasedStateExprAst::Literal { repr } => IrValueExpr::Literal((*repr).to_owned()),
        nirvash::ErasedStateExprAst::FieldRead { path } => IrValueExpr::Field((*path).to_owned()),
        nirvash::ErasedStateExprAst::PureCall {
            name,
            symbolic,
            read_paths,
        } => IrValueExpr::PureCall {
            name: (*name).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
            symbolic_key: lower_symbolic_key(*symbolic),
        },
        nirvash::ErasedStateExprAst::Add { lhs, rhs } => IrValueExpr::Add(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Sub { lhs, rhs } => IrValueExpr::Sub(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Mul { lhs, rhs } => IrValueExpr::Mul(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Neg { expr } => {
            IrValueExpr::Neg(Box::new(lower_state_value_ast(expr)))
        }
        nirvash::ErasedStateExprAst::Union { lhs, rhs } => IrValueExpr::Union(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Intersection { lhs, rhs } => IrValueExpr::Intersection(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Difference { lhs, rhs } => IrValueExpr::Difference(
            Box::new(lower_state_value_ast(lhs)),
            Box::new(lower_state_value_ast(rhs)),
        ),
        nirvash::ErasedStateExprAst::Comprehension {
            domain,
            body,
            read_paths,
        } => IrValueExpr::Comprehension {
            domain: (*domain).to_owned(),
            body: (*body).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
        },
        nirvash::ErasedStateExprAst::IfElse {
            condition,
            then_branch,
            else_branch,
        } => IrValueExpr::Conditional {
            condition: condition.name().to_owned(),
            then_branch: Box::new(lower_state_value_ast(then_branch)),
            else_branch: Box::new(lower_state_value_ast(else_branch)),
        },
    }
}

fn lower_step_value_ast<S: 'static, A: 'static>(
    ast: &nirvash::ErasedStepValueExprAst<S, A>,
) -> IrValueExpr {
    match ast {
        nirvash::ErasedStepValueExprAst::Opaque { repr } => IrValueExpr::Opaque((*repr).to_owned()),
        nirvash::ErasedStepValueExprAst::Literal { repr } => {
            IrValueExpr::Literal((*repr).to_owned())
        }
        nirvash::ErasedStepValueExprAst::FieldRead { path } => {
            IrValueExpr::Field((*path).to_owned())
        }
        nirvash::ErasedStepValueExprAst::PureCall {
            name,
            symbolic,
            read_paths,
        } => IrValueExpr::PureCall {
            name: (*name).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
            symbolic_key: lower_symbolic_key(*symbolic),
        },
        nirvash::ErasedStepValueExprAst::Add { lhs, rhs } => IrValueExpr::Add(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Sub { lhs, rhs } => IrValueExpr::Sub(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Mul { lhs, rhs } => IrValueExpr::Mul(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Neg { expr } => {
            IrValueExpr::Neg(Box::new(lower_step_value_ast(expr)))
        }
        nirvash::ErasedStepValueExprAst::Union { lhs, rhs } => IrValueExpr::Union(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Intersection { lhs, rhs } => IrValueExpr::Intersection(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Difference { lhs, rhs } => IrValueExpr::Difference(
            Box::new(lower_step_value_ast(lhs)),
            Box::new(lower_step_value_ast(rhs)),
        ),
        nirvash::ErasedStepValueExprAst::Comprehension {
            domain,
            body,
            read_paths,
        } => IrValueExpr::Comprehension {
            domain: (*domain).to_owned(),
            body: (*body).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
        },
        nirvash::ErasedStepValueExprAst::IfElse {
            condition,
            then_branch,
            else_branch,
        } => IrValueExpr::Conditional {
            condition: condition.name().to_owned(),
            then_branch: Box::new(lower_step_value_ast(then_branch)),
            else_branch: Box::new(lower_step_value_ast(else_branch)),
        },
    }
}

fn lower_guard_value_ast<S: 'static, A: 'static>(
    ast: &nirvash::ErasedGuardValueExprAst<S, A>,
) -> IrValueExpr {
    match ast {
        nirvash::ErasedGuardValueExprAst::Opaque { repr } => {
            IrValueExpr::Opaque((*repr).to_owned())
        }
        nirvash::ErasedGuardValueExprAst::Literal { repr } => {
            IrValueExpr::Literal((*repr).to_owned())
        }
        nirvash::ErasedGuardValueExprAst::FieldRead { path } => {
            IrValueExpr::Field((*path).to_owned())
        }
        nirvash::ErasedGuardValueExprAst::PureCall {
            name,
            symbolic,
            read_paths,
        } => IrValueExpr::PureCall {
            name: (*name).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
            symbolic_key: lower_symbolic_key(*symbolic),
        },
        nirvash::ErasedGuardValueExprAst::Add { lhs, rhs } => IrValueExpr::Add(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Sub { lhs, rhs } => IrValueExpr::Sub(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Mul { lhs, rhs } => IrValueExpr::Mul(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Neg { expr } => {
            IrValueExpr::Neg(Box::new(lower_guard_value_ast(expr)))
        }
        nirvash::ErasedGuardValueExprAst::Union { lhs, rhs } => IrValueExpr::Union(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Intersection { lhs, rhs } => IrValueExpr::Intersection(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Difference { lhs, rhs } => IrValueExpr::Difference(
            Box::new(lower_guard_value_ast(lhs)),
            Box::new(lower_guard_value_ast(rhs)),
        ),
        nirvash::ErasedGuardValueExprAst::Comprehension {
            domain,
            body,
            read_paths,
        } => IrValueExpr::Comprehension {
            domain: (*domain).to_owned(),
            body: (*body).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
        },
        nirvash::ErasedGuardValueExprAst::IfElse {
            condition,
            then_branch,
            else_branch,
        } => IrValueExpr::Conditional {
            condition: condition.name().to_owned(),
            then_branch: Box::new(lower_guard_value_ast(then_branch)),
            else_branch: Box::new(lower_guard_value_ast(else_branch)),
        },
    }
}

fn lower_update_value_ast<S: 'static, A: 'static>(ast: &UpdateValueExprAst<S, A>) -> IrValueExpr {
    match ast {
        UpdateValueExprAst::Opaque { repr } => IrValueExpr::Opaque((*repr).to_owned()),
        UpdateValueExprAst::Literal { repr } => IrValueExpr::Literal((*repr).to_owned()),
        UpdateValueExprAst::FieldRead { path } => IrValueExpr::Field((*path).to_owned()),
        UpdateValueExprAst::PureCall {
            name,
            symbolic,
            read_paths,
        } => IrValueExpr::PureCall {
            name: (*name).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
            symbolic_key: lower_symbolic_key(*symbolic),
        },
        UpdateValueExprAst::Add { lhs, rhs } => IrValueExpr::Add(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::Sub { lhs, rhs } => IrValueExpr::Sub(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::Mul { lhs, rhs } => IrValueExpr::Mul(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::Neg { expr } => {
            IrValueExpr::Neg(Box::new(lower_update_value_ast(expr)))
        }
        UpdateValueExprAst::Union { lhs, rhs } => IrValueExpr::Union(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::Intersection { lhs, rhs } => IrValueExpr::Intersection(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::Difference { lhs, rhs } => IrValueExpr::Difference(
            Box::new(lower_update_value_ast(lhs)),
            Box::new(lower_update_value_ast(rhs)),
        ),
        UpdateValueExprAst::SequenceUpdate { base, index, value } => IrValueExpr::SequenceUpdate {
            base: Box::new(lower_update_value_ast(base)),
            index: Box::new(lower_update_value_ast(index)),
            value: Box::new(lower_update_value_ast(value)),
        },
        UpdateValueExprAst::FunctionUpdate { base, key, value } => IrValueExpr::FunctionUpdate {
            base: Box::new(lower_update_value_ast(base)),
            key: Box::new(lower_update_value_ast(key)),
            value: Box::new(lower_update_value_ast(value)),
        },
        UpdateValueExprAst::RecordUpdate { base, field, value } => IrValueExpr::RecordUpdate {
            base: Box::new(lower_update_value_ast(base)),
            field: (*field).to_owned(),
            value: Box::new(lower_update_value_ast(value)),
        },
        UpdateValueExprAst::Comprehension {
            domain,
            body,
            read_paths,
        } => IrValueExpr::Comprehension {
            domain: (*domain).to_owned(),
            body: (*body).to_owned(),
            read_paths: read_paths.iter().map(|path| (*path).to_owned()).collect(),
        },
        UpdateValueExprAst::IfElse {
            condition,
            then_branch,
            else_branch,
        } => IrValueExpr::Conditional {
            condition: condition.name().to_owned(),
            then_branch: Box::new(lower_update_value_ast(then_branch)),
            else_branch: Box::new(lower_update_value_ast(else_branch)),
        },
        UpdateValueExprAst::_Phantom(_) => IrValueExpr::Unit,
    }
}

fn lower_guard_ast<S, A>(
    spec_name: &'static str,
    label: &'static str,
    ast: &nirvash::GuardAst<S, A>,
) -> Result<ActionExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    Ok(match ast {
        nirvash::GuardAst::Literal(true) => ActionExpr::True,
        nirvash::GuardAst::Literal(false) => ActionExpr::False,
        nirvash::GuardAst::FieldRead(field) | nirvash::GuardAst::PureCall(field) => {
            ActionExpr::Pred(named_ref(field.label()))
        }
        nirvash::GuardAst::Eq(compare)
        | nirvash::GuardAst::Ne(compare)
        | nirvash::GuardAst::Lt(compare)
        | nirvash::GuardAst::Le(compare)
        | nirvash::GuardAst::Gt(compare)
        | nirvash::GuardAst::Ge(compare) => ActionExpr::Compare {
            op: lower_comparison_op(compare.op()),
            lhs: lower_guard_value_ast(compare.lhs_ast()),
            rhs: lower_guard_value_ast(compare.rhs_ast()),
        },
        nirvash::GuardAst::Contains(predicate) | nirvash::GuardAst::SubsetOf(predicate) => {
            ActionExpr::Builtin {
                op: lower_builtin_predicate_op(predicate.op()),
                lhs: lower_guard_value_ast(predicate.lhs_ast()),
                rhs: lower_guard_value_ast(predicate.rhs_ast()),
            }
        }
        nirvash::GuardAst::Match(matcher) => ActionExpr::Match {
            value: matcher.value().to_owned(),
            pattern: matcher.pattern().to_owned(),
        },
        nirvash::GuardAst::ForAll(quantifier) | nirvash::GuardAst::Exists(quantifier) => {
            ActionExpr::Quantified {
                kind: lower_quantifier_kind(quantifier.kind()),
                domain: quantifier.domain().to_owned(),
                body: quantifier.body().to_owned(),
                read_paths: quantifier
                    .symbolic_state_paths()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                symbolic_supported: quantifier.is_symbolic_supported(),
            }
        }
        nirvash::GuardAst::Not(inner) => ActionExpr::Implies(
            Box::new(lower_guard_expr(spec_name, label, inner)?),
            Box::new(ActionExpr::False),
        ),
        nirvash::GuardAst::And(parts) => ActionExpr::And(
            parts
                .iter()
                .map(|part| lower_guard_expr(spec_name, part.name(), part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        nirvash::GuardAst::Or(parts) => ActionExpr::Or(
            parts
                .iter()
                .map(|part| lower_guard_expr(spec_name, part.name(), part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

fn lower_guard_expr<S, A>(
    spec_name: &'static str,
    label: &'static str,
    guard: &nirvash::GuardExpr<S, A>,
) -> Result<ActionExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    let Some(ast) = guard.ast() else {
        return Err(LoweringError::unsupported_fragment(
            spec_name,
            "guard",
            label,
            "direct_smt.transition",
            "non-AST guard cannot be lowered into direct SMT artifacts",
        ));
    };
    lower_guard_ast(spec_name, label, ast)
}

fn lower_update_ast<S, A>(ast: &UpdateAst<S, A>) -> IrUpdateExpr
where
    S: 'static,
    A: 'static,
{
    match ast {
        UpdateAst::Sequence(ops) => IrUpdateExpr::Sequence(
            ops.iter()
                .map(|op| match op {
                    UpdateOp::Assign {
                        target, value_ast, ..
                    } => IrUpdateOpDecl::Assign {
                        target: (*target).to_owned(),
                        value: lower_update_value_ast(value_ast),
                    },
                    UpdateOp::SetInsert {
                        target, item_ast, ..
                    } => IrUpdateOpDecl::SetInsert {
                        target: (*target).to_owned(),
                        item: lower_update_value_ast(item_ast),
                    },
                    UpdateOp::SetRemove {
                        target, item_ast, ..
                    } => IrUpdateOpDecl::SetRemove {
                        target: (*target).to_owned(),
                        item: lower_update_value_ast(item_ast),
                    },
                    UpdateOp::Effect { name, symbolic, .. } => IrUpdateOpDecl::Effect {
                        name: (*name).to_owned(),
                        symbolic_key: lower_symbolic_key(*symbolic),
                    },
                })
                .collect(),
        ),
        UpdateAst::Choice(choice) => IrUpdateExpr::Choice {
            domain: choice.domain().to_owned(),
            body: choice.body().to_owned(),
            read_paths: choice
                .symbolic_state_paths()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            write_paths: choice
                .write_paths()
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        },
    }
}

fn lower_transition_rule<S, A>(
    spec_name: &'static str,
    rule: &TransitionRule<S, A>,
) -> Result<ActionExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    let Some(guard_ast) = rule.guard_ast() else {
        return Err(LoweringError::unsupported_fragment(
            spec_name,
            "transition rule",
            rule.name(),
            "direct_smt.transition",
            "rule guard is not AST-native",
        ));
    };
    let Some(update_ast) = rule.update_ast() else {
        return Err(LoweringError::unsupported_fragment(
            spec_name,
            "transition rule",
            rule.name(),
            "direct_smt.transition",
            "rule update is not AST-native",
        ));
    };
    Ok(ActionExpr::Rule {
        name: rule.name().to_owned(),
        guard: Box::new(lower_guard_ast(spec_name, rule.name(), guard_ast)?),
        update: lower_update_ast(update_ast),
    })
}

fn lower_init<S>(
    spec_name: &'static str,
    initial_states: &[S],
) -> Result<IrStateExpr, LoweringError>
where
    S: Debug,
{
    if initial_states.is_empty() {
        return Err(LoweringError::unsupported(
            spec_name,
            "init",
            spec_name,
            "FrontendSpec::initial_states() returned an empty set",
        ));
    }
    let states = initial_states
        .iter()
        .map(|state| IrStateExpr::Const(format!("{state:?}")))
        .collect::<Vec<_>>();
    Ok(match states.as_slice() {
        [single] => single.clone(),
        many => IrStateExpr::Or(many.to_vec()),
    })
}

fn lower_next<S, A>(
    spec_name: &'static str,
    actions: &[A],
    transition_program: Option<&TransitionProgram<S, A>>,
) -> ActionExpr
where
    S: 'static,
    A: Debug + 'static,
{
    let body = if let Some(program) = transition_program {
        let rules = program
            .rules()
            .iter()
            .map(|rule| {
                lower_transition_rule(spec_name, rule)
                    .unwrap_or_else(|_| ActionExpr::Ref(format!("rule::{}", rule.name())))
            })
            .collect::<Vec<_>>();
        match rules.as_slice() {
            [] => ActionExpr::Ref(format!("{spec_name}::next")),
            [single] => single.clone(),
            many => ActionExpr::Or(many.to_vec()),
        }
    } else {
        let lowered = actions
            .iter()
            .map(|action| ActionExpr::Pred(IrStateExpr::Const(format!("{action:?}"))))
            .collect::<Vec<_>>();
        match lowered.as_slice() {
            [] => ActionExpr::Ref(format!("{spec_name}::transition_relation")),
            [single] => single.clone(),
            many => ActionExpr::Or(many.to_vec()),
        }
    };

    ActionExpr::BoxAction {
        action: Box::new(body),
        view: ViewExpr::Vars,
    }
}

fn lower_fairness<S, A>(
    spec_name: &'static str,
    fairness: &Fairness<S, A>,
) -> Result<FairnessDecl, LoweringError>
where
    S: 'static,
    A: 'static,
{
    let action = lower_step_expr(spec_name, "fairness", fairness.name(), fairness.predicate())?;
    Ok(match fairness {
        Fairness::Weak(_) => FairnessDecl::WF {
            view: ViewExpr::Vars,
            action,
        },
        Fairness::Strong(_) => FairnessDecl::SF {
            view: ViewExpr::Vars,
            action,
        },
    })
}

fn lower_ltl<S, A>(
    spec_name: &'static str,
    formula: &Ltl<S, A>,
) -> Result<TemporalExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    Ok(match formula {
        Ltl::True => TemporalExpr::State(IrStateExpr::True),
        Ltl::False => TemporalExpr::State(IrStateExpr::False),
        Ltl::Pred(predicate) => {
            TemporalExpr::State(lower_bool_expr(spec_name, "property", predicate)?)
        }
        Ltl::StepPred(predicate) => TemporalExpr::Action(lower_step_expr(
            spec_name,
            "property",
            predicate.name(),
            predicate,
        )?),
        Ltl::Not(inner) => TemporalExpr::Not(Box::new(lower_ltl(spec_name, inner)?)),
        Ltl::And(lhs, rhs) => {
            TemporalExpr::And(vec![lower_ltl(spec_name, lhs)?, lower_ltl(spec_name, rhs)?])
        }
        Ltl::Or(lhs, rhs) => {
            TemporalExpr::Or(vec![lower_ltl(spec_name, lhs)?, lower_ltl(spec_name, rhs)?])
        }
        Ltl::Implies(lhs, rhs) => TemporalExpr::Implies(
            Box::new(lower_ltl(spec_name, lhs)?),
            Box::new(lower_ltl(spec_name, rhs)?),
        ),
        Ltl::Next(inner) => TemporalExpr::Next(Box::new(lower_ltl(spec_name, inner)?)),
        Ltl::Always(inner) => TemporalExpr::Always(Box::new(lower_ltl(spec_name, inner)?)),
        Ltl::Eventually(inner) => TemporalExpr::Eventually(Box::new(lower_ltl(spec_name, inner)?)),
        Ltl::Until(lhs, rhs) => TemporalExpr::Until(
            Box::new(lower_ltl(spec_name, lhs)?),
            Box::new(lower_ltl(spec_name, rhs)?),
        ),
        Ltl::Enabled(predicate) => TemporalExpr::Enabled(lower_step_expr(
            spec_name,
            "property",
            predicate.name(),
            predicate,
        )?),
    })
}

fn lower_bool_expr<S>(
    spec_name: &'static str,
    node_kind: &'static str,
    predicate: &BoolExpr<S>,
) -> Result<IrStateExpr, LoweringError>
where
    S: 'static,
{
    let Some(ast) = predicate.ast() else {
        return Err(LoweringError::unsupported(
            spec_name,
            node_kind,
            predicate.name(),
            "non-AST predicate cannot be lowered into SpecCore",
        ));
    };
    lower_bool_ast(spec_name, node_kind, predicate.name(), ast)
}

fn lower_bool_ast<S>(
    spec_name: &'static str,
    node_kind: &'static str,
    _label: &'static str,
    ast: &BoolExprAst<S>,
) -> Result<IrStateExpr, LoweringError>
where
    S: 'static,
{
    Ok(match ast {
        BoolExprAst::Literal(true) => IrStateExpr::True,
        BoolExprAst::Literal(false) => IrStateExpr::False,
        BoolExprAst::FieldRead(field) | BoolExprAst::PureCall(field) => named_ref(field.label()),
        BoolExprAst::Eq(compare)
        | BoolExprAst::Ne(compare)
        | BoolExprAst::Lt(compare)
        | BoolExprAst::Le(compare)
        | BoolExprAst::Gt(compare)
        | BoolExprAst::Ge(compare) => IrStateExpr::Compare {
            op: lower_comparison_op(compare.op()),
            lhs: lower_state_value_ast(compare.lhs_ast()),
            rhs: lower_state_value_ast(compare.rhs_ast()),
        },
        BoolExprAst::Contains(predicate) | BoolExprAst::SubsetOf(predicate) => {
            IrStateExpr::Builtin {
                op: lower_builtin_predicate_op(predicate.op()),
                lhs: lower_state_value_ast(predicate.lhs_ast()),
                rhs: lower_state_value_ast(predicate.rhs_ast()),
            }
        }
        BoolExprAst::Match(matcher) => IrStateExpr::Match {
            value: matcher.value().to_owned(),
            pattern: matcher.pattern().to_owned(),
        },
        BoolExprAst::ForAll(quantifier) | BoolExprAst::Exists(quantifier) => {
            IrStateExpr::Quantified {
                kind: lower_quantifier_kind(quantifier.kind()),
                domain: quantifier.domain().to_owned(),
                body: quantifier.body().to_owned(),
                read_paths: Vec::new(),
                symbolic_supported: quantifier.is_symbolic_supported(),
            }
        }
        BoolExprAst::Not(inner) => {
            IrStateExpr::Not(Box::new(lower_bool_expr(spec_name, node_kind, inner)?))
        }
        BoolExprAst::And(parts) => IrStateExpr::And(
            parts
                .iter()
                .map(|part| lower_bool_expr(spec_name, node_kind, part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        BoolExprAst::Or(parts) => IrStateExpr::Or(
            parts
                .iter()
                .map(|part| lower_bool_expr(spec_name, node_kind, part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

fn lower_step_expr<S, A>(
    spec_name: &'static str,
    node_kind: &'static str,
    label: &'static str,
    predicate: &StepExpr<S, A>,
) -> Result<ActionExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    let Some(ast) = predicate.ast() else {
        return Err(LoweringError::unsupported(
            spec_name,
            node_kind,
            label,
            "non-AST step predicate cannot be lowered into SpecCore",
        ));
    };
    lower_step_ast(spec_name, node_kind, label, ast)
}

fn lower_step_ast<S, A>(
    spec_name: &'static str,
    node_kind: &'static str,
    label: &'static str,
    ast: &StepExprAst<S, A>,
) -> Result<ActionExpr, LoweringError>
where
    S: 'static,
    A: 'static,
{
    Ok(match ast {
        StepExprAst::Literal(true) => ActionExpr::True,
        StepExprAst::Literal(false) => ActionExpr::False,
        StepExprAst::FieldRead(field) | StepExprAst::PureCall(field) => {
            ActionExpr::Pred(named_ref(field.label()))
        }
        StepExprAst::Eq(compare)
        | StepExprAst::Ne(compare)
        | StepExprAst::Lt(compare)
        | StepExprAst::Le(compare)
        | StepExprAst::Gt(compare)
        | StepExprAst::Ge(compare) => ActionExpr::Compare {
            op: lower_comparison_op(compare.op()),
            lhs: lower_step_value_ast(compare.lhs_ast()),
            rhs: lower_step_value_ast(compare.rhs_ast()),
        },
        StepExprAst::Contains(predicate) | StepExprAst::SubsetOf(predicate) => {
            ActionExpr::Builtin {
                op: lower_builtin_predicate_op(predicate.op()),
                lhs: lower_step_value_ast(predicate.lhs_ast()),
                rhs: lower_step_value_ast(predicate.rhs_ast()),
            }
        }
        StepExprAst::Match(matcher) => ActionExpr::Match {
            value: matcher.value().to_owned(),
            pattern: matcher.pattern().to_owned(),
        },
        StepExprAst::ForAll(quantifier) | StepExprAst::Exists(quantifier) => {
            ActionExpr::Quantified {
                kind: lower_quantifier_kind(quantifier.kind()),
                domain: quantifier.domain().to_owned(),
                body: quantifier.body().to_owned(),
                read_paths: Vec::new(),
                symbolic_supported: quantifier.is_symbolic_supported(),
            }
        }
        StepExprAst::Not(inner) => ActionExpr::Implies(
            Box::new(lower_step_expr(spec_name, node_kind, label, inner)?),
            Box::new(ActionExpr::False),
        ),
        StepExprAst::And(parts) => ActionExpr::And(
            parts
                .iter()
                .map(|part| lower_step_expr(spec_name, node_kind, part.name(), part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        StepExprAst::Or(parts) => ActionExpr::Or(
            parts
                .iter()
                .map(|part| lower_step_expr(spec_name, node_kind, part.name(), part))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

pub mod registry {
    use std::{
        any::{Any, TypeId, type_name},
        collections::BTreeSet,
    };

    use super::{FrontendSpec, ModelInstance, SymmetrySpec};
    use nirvash::{
        BoolExpr, Fairness, Ltl, SpecVizRegistrationSet, StepExpr,
        registry::{
            RegisteredActionConstraint, RegisteredFairness, RegisteredInvariant,
            RegisteredProperty, RegisteredStateConstraint, RegisteredSymmetry,
        },
    };

    type ErasedBuilder = fn() -> Box<dyn Any>;
    type NamedBuilder = (&'static str, ErasedBuilder);

    #[derive(Debug, Clone)]
    pub struct ScopedStateConstraint<S> {
        name: &'static str,
        case_labels: Option<&'static [&'static str]>,
        constraint: BoolExpr<S>,
    }

    impl<S> ScopedStateConstraint<S> {
        pub const fn name(&self) -> &'static str {
            self.name
        }

        pub const fn case_labels(&self) -> Option<&'static [&'static str]> {
            self.case_labels
        }

        pub fn constraint(&self) -> &BoolExpr<S> {
            &self.constraint
        }

        pub fn applies_to(&self, case_label: &str) -> bool {
            self.case_labels
                .is_none_or(|labels| labels.contains(&case_label))
        }
    }

    #[derive(Debug, Clone)]
    pub struct ScopedActionConstraint<S, A> {
        name: &'static str,
        case_labels: Option<&'static [&'static str]>,
        constraint: StepExpr<S, A>,
    }

    impl<S, A> ScopedActionConstraint<S, A> {
        pub const fn name(&self) -> &'static str {
            self.name
        }

        pub const fn case_labels(&self) -> Option<&'static [&'static str]> {
            self.case_labels
        }

        pub fn constraint(&self) -> &StepExpr<S, A> {
            &self.constraint
        }

        pub fn applies_to(&self, case_label: &str) -> bool {
            self.case_labels
                .is_none_or(|labels| labels.contains(&case_label))
        }
    }

    trait RegistryEntry {
        fn spec_type_id(&self) -> TypeId;
        fn name(&self) -> &'static str;
    }

    macro_rules! impl_registry_entry {
        ($ty:ty) => {
            impl RegistryEntry for $ty {
                fn spec_type_id(&self) -> TypeId {
                    (self.spec_type_id)()
                }

                fn name(&self) -> &'static str {
                    self.name
                }
            }
        };
    }

    impl_registry_entry!(RegisteredInvariant);
    impl_registry_entry!(RegisteredProperty);
    impl_registry_entry!(RegisteredFairness);
    impl_registry_entry!(RegisteredStateConstraint);
    impl_registry_entry!(RegisteredActionConstraint);
    impl_registry_entry!(RegisteredSymmetry);

    fn sorted_builders<'a, Spec, I>(entries: I, kind: &'static str) -> Vec<NamedBuilder>
    where
        Spec: 'static,
        I: IntoIterator<Item = (&'a dyn RegistryEntry, ErasedBuilder)>,
    {
        let spec_type_id = TypeId::of::<Spec>();
        let spec_name = type_name::<Spec>();
        let mut matched = entries
            .into_iter()
            .filter(|(entry, _)| entry.spec_type_id() == spec_type_id)
            .map(|(entry, build)| (entry.name(), build))
            .collect::<Vec<_>>();
        matched.sort_by_key(|(name, _)| *name);

        let mut seen = BTreeSet::new();
        for (name, _) in &matched {
            if !seen.insert(*name) {
                panic!("duplicate {kind} registration `{name}` for spec `{spec_name}`");
            }
        }

        matched
    }

    fn downcast_registered<T>(
        value: Box<dyn Any>,
        spec_name: &'static str,
        kind: &str,
        name: &str,
    ) -> T
    where
        T: 'static,
    {
        *value.downcast::<T>().unwrap_or_else(|_| {
            panic!("registered {kind} `{name}` for spec `{spec_name}` has an unexpected type")
        })
    }

    pub fn collect_invariants_for<Spec, State>() -> Vec<BoolExpr<State>>
    where
        Spec: 'static,
        State: 'static,
    {
        let spec_name = type_name::<Spec>();
        sorted_builders::<Spec, _>(
            nirvash::inventory::iter::<RegisteredInvariant>
                .into_iter()
                .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
            "invariant",
        )
        .into_iter()
        .map(|(name, build)| {
            downcast_registered::<BoolExpr<State>>(build(), spec_name, "invariant", name)
        })
        .collect()
    }

    pub fn collect_properties_for<Spec, State, Action>() -> Vec<Ltl<State, Action>>
    where
        Spec: 'static,
        State: 'static,
        Action: 'static,
    {
        let spec_name = type_name::<Spec>();
        sorted_builders::<Spec, _>(
            nirvash::inventory::iter::<RegisteredProperty>
                .into_iter()
                .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
            "property",
        )
        .into_iter()
        .map(|(name, build)| {
            downcast_registered::<Ltl<State, Action>>(build(), spec_name, "property", name)
        })
        .collect()
    }

    pub fn collect_fairness_for<Spec, State, Action>() -> Vec<Fairness<State, Action>>
    where
        Spec: 'static,
        State: 'static,
        Action: 'static,
    {
        let spec_name = type_name::<Spec>();
        sorted_builders::<Spec, _>(
            nirvash::inventory::iter::<RegisteredFairness>
                .into_iter()
                .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
            "fairness",
        )
        .into_iter()
        .map(|(name, build)| {
            downcast_registered::<Fairness<State, Action>>(build(), spec_name, "fairness", name)
        })
        .collect()
    }

    pub fn collect_scoped_state_constraints_for<Spec, State>() -> Vec<ScopedStateConstraint<State>>
    where
        Spec: 'static,
        State: 'static,
    {
        let spec_name = type_name::<Spec>();
        let spec_type_id = TypeId::of::<Spec>();
        let mut matched = nirvash::inventory::iter::<RegisteredStateConstraint>
            .into_iter()
            .filter(|entry| (entry.spec_type_id)() == spec_type_id)
            .collect::<Vec<_>>();
        matched.sort_by_key(|entry| entry.name);

        let mut seen = BTreeSet::new();
        for entry in &matched {
            if !seen.insert(entry.name) {
                panic!(
                    "duplicate state constraint registration `{}` for spec `{spec_name}`",
                    entry.name
                );
            }
        }

        matched
            .into_iter()
            .map(|entry| ScopedStateConstraint {
                name: entry.name,
                case_labels: entry.case_labels,
                constraint: downcast_registered::<BoolExpr<State>>(
                    (entry.build)(),
                    spec_name,
                    "state constraint",
                    entry.name,
                ),
            })
            .collect()
    }

    pub fn collect_scoped_action_constraints_for<Spec, State, Action>()
    -> Vec<ScopedActionConstraint<State, Action>>
    where
        Spec: 'static,
        State: 'static,
        Action: 'static,
    {
        let spec_name = type_name::<Spec>();
        let spec_type_id = TypeId::of::<Spec>();
        let mut matched = nirvash::inventory::iter::<RegisteredActionConstraint>
            .into_iter()
            .filter(|entry| (entry.spec_type_id)() == spec_type_id)
            .collect::<Vec<_>>();
        matched.sort_by_key(|entry| entry.name);

        let mut seen = BTreeSet::new();
        for entry in &matched {
            if !seen.insert(entry.name) {
                panic!(
                    "duplicate action constraint registration `{}` for spec `{spec_name}`",
                    entry.name
                );
            }
        }

        matched
            .into_iter()
            .map(|entry| ScopedActionConstraint {
                name: entry.name,
                case_labels: entry.case_labels,
                constraint: downcast_registered::<StepExpr<State, Action>>(
                    (entry.build)(),
                    spec_name,
                    "action constraint",
                    entry.name,
                ),
            })
            .collect()
    }

    fn validate_case_labels(
        spec_name: &'static str,
        kind: &'static str,
        name: &'static str,
        case_labels: Option<&'static [&'static str]>,
        available_labels: &BTreeSet<&'static str>,
    ) {
        if let Some(labels) = case_labels {
            for label in labels {
                assert!(
                    available_labels.contains(label),
                    "registered {kind} `{name}` references unknown model case `{label}` for spec `{spec_name}`"
                );
            }
        }
    }

    pub fn collect_symmetry_for<Spec, State>() -> Option<SymmetrySpec<State>>
    where
        Spec: 'static,
        State: 'static,
    {
        let spec_name = type_name::<Spec>();
        let matched = sorted_builders::<Spec, _>(
            nirvash::inventory::iter::<RegisteredSymmetry>
                .into_iter()
                .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
            "symmetry",
        );
        assert!(
            matched.len() <= 1,
            "multiple symmetry registrations for spec `{spec_name}` are not supported"
        );
        matched.into_iter().next().map(|(name, build)| {
            downcast_registered::<SymmetrySpec<State>>(build(), spec_name, "symmetry", name)
        })
    }

    pub fn collect_symmetry_name_for<Spec>() -> Option<String>
    where
        Spec: 'static,
    {
        let matched = sorted_builders::<Spec, _>(
            nirvash::inventory::iter::<RegisteredSymmetry>
                .into_iter()
                .map(|entry| (entry as &dyn RegistryEntry, entry.build)),
            "symmetry",
        );
        matched.into_iter().next().map(|(name, _)| name.to_owned())
    }

    pub fn apply_registered_model_case_metadata_for<Spec, State, Action>(
        model_instances: &mut Vec<ModelInstance<State, Action>>,
    ) where
        Spec: 'static,
        State: Clone + 'static,
        Action: Clone + 'static,
    {
        let state_constraints = collect_scoped_state_constraints_for::<Spec, State>();
        let action_constraints = collect_scoped_action_constraints_for::<Spec, State, Action>();
        let symmetry = collect_symmetry_for::<Spec, State>();
        let spec_name = type_name::<Spec>();
        let available_labels = model_instances
            .iter()
            .map(|model_instance| model_instance.label())
            .collect::<BTreeSet<_>>();

        for entry in &state_constraints {
            validate_case_labels(
                spec_name,
                "state constraint",
                entry.name(),
                entry.case_labels(),
                &available_labels,
            );
        }
        for entry in &action_constraints {
            validate_case_labels(
                spec_name,
                "action constraint",
                entry.name(),
                entry.case_labels(),
                &available_labels,
            );
        }

        for model_instance in model_instances {
            let mut next_model_instance = std::mem::take(model_instance);
            for constraint in &state_constraints {
                if constraint.applies_to(next_model_instance.label()) {
                    next_model_instance =
                        next_model_instance.with_state_constraint(constraint.constraint().clone());
                }
            }
            for constraint in &action_constraints {
                if constraint.applies_to(next_model_instance.label()) {
                    next_model_instance =
                        next_model_instance.with_action_constraint(constraint.constraint().clone());
                }
            }
            if next_model_instance.symmetry().is_none()
                && let Some(symmetry) = symmetry
            {
                next_model_instance = next_model_instance.with_symmetry(symmetry);
            }
            *model_instance = next_model_instance;
        }
    }

    pub fn collect_spec_viz_registrations_for<Spec, State, Action>() -> SpecVizRegistrationSet
    where
        Spec: 'static,
        State: 'static,
        Action: 'static,
    {
        SpecVizRegistrationSet {
            invariants: collect_invariants_for::<Spec, State>()
                .into_iter()
                .map(|predicate| predicate.name().to_owned())
                .collect(),
            properties: collect_properties_for::<Spec, State, Action>()
                .into_iter()
                .map(|property| property.describe().to_owned())
                .collect(),
            fairness: collect_fairness_for::<Spec, State, Action>()
                .into_iter()
                .map(|fairness| fairness.name().to_owned())
                .collect(),
            state_constraints: collect_scoped_state_constraints_for::<Spec, State>()
                .into_iter()
                .map(|constraint| constraint.name().to_owned())
                .collect(),
            action_constraints: collect_scoped_action_constraints_for::<Spec, State, Action>()
                .into_iter()
                .map(|constraint| constraint.name().to_owned())
                .collect(),
            symmetries: collect_symmetry_name_for::<Spec>().into_iter().collect(),
        }
    }

    pub fn collect_invariants<T>() -> Vec<BoolExpr<T::State>>
    where
        T: FrontendSpec + 'static,
        T::State: 'static,
    {
        collect_invariants_for::<T, T::State>()
    }

    pub fn collect_properties<T>() -> Vec<Ltl<T::State, T::Action>>
    where
        T: FrontendSpec + 'static,
        T::State: 'static,
        T::Action: 'static,
    {
        collect_properties_for::<T, T::State, T::Action>()
    }

    pub fn collect_fairness<T>() -> Vec<Fairness<T::State, T::Action>>
    where
        T: FrontendSpec + 'static,
        T::State: 'static,
        T::Action: 'static,
    {
        collect_fairness_for::<T, T::State, T::Action>()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionExpr, BoolExpr, Fairness, FairnessDecl, FiniteModelDomain, FrontendSpec, IrStateExpr,
        LoweringCx, Ltl, ModelInstance, StateAbstraction, StepExpr, TemporalExpr, TemporalSpec,
        TransitionProgram, TransitionRule, UpdateOp, UpdateProgram, UpdateValueExprAst, ViewExpr,
    };
    use nirvash::GuardExpr;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DemoState {
        Idle,
        Busy,
    }

    impl FiniteModelDomain for DemoState {
        fn finite_domain() -> super::BoundedDomain<Self> {
            super::BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DemoAction {
        Start,
    }

    impl FiniteModelDomain for DemoAction {
        fn finite_domain() -> super::BoundedDomain<Self> {
            super::BoundedDomain::new(vec![Self::Start])
        }
    }

    struct DemoSpec;

    impl FrontendSpec for DemoSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn frontend_name(&self) -> &'static str {
            "DemoSpec"
        }

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DemoState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Start]
        }
    }

    impl TemporalSpec for DemoSpec {
        fn invariants(&self) -> Vec<super::BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    struct StructuredSpec;

    impl FrontendSpec for StructuredSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn frontend_name(&self) -> &'static str {
            "StructuredSpec"
        }

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DemoState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Start]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "structured_demo",
                vec![TransitionRule::ast(
                    "start",
                    GuardExpr::builtin_pure_call_with_paths(
                        "can_start",
                        &["prev.state", "action"],
                        |prev: &DemoState, action: &DemoAction| {
                            matches!(prev, DemoState::Idle) && matches!(action, DemoAction::Start)
                        },
                    ),
                    UpdateProgram::ast(
                        "start",
                        vec![UpdateOp::assign_ast(
                            "state",
                            UpdateValueExprAst::literal("Busy"),
                            |_prev: &DemoState, state: &mut DemoState, _action: &DemoAction| {
                                *state = DemoState::Busy;
                            },
                        )],
                    ),
                )],
            ))
        }
    }

    impl TemporalSpec for StructuredSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![BoolExpr::builtin_pure_call_with_paths(
                "state_is_known",
                &["state"],
                |_state: &DemoState| true,
            )]
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![
                Ltl::always(Ltl::pred(BoolExpr::builtin_pure_call_with_paths(
                    "is_busy",
                    &["state"],
                    |state: &DemoState| matches!(state, DemoState::Busy),
                ))),
                Ltl::enabled(StepExpr::builtin_pure_call_with_paths(
                    "busy_step",
                    &["next.state"],
                    |_prev: &DemoState, _action: &DemoAction, next: &DemoState| {
                        matches!(next, DemoState::Busy)
                    },
                )),
            ]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![
                Fairness::weak(StepExpr::builtin_pure_call_with_paths(
                    "weak_busy",
                    &["next.state"],
                    |_prev: &DemoState, _action: &DemoAction, next: &DemoState| {
                        matches!(next, DemoState::Busy)
                    },
                )),
                Fairness::strong(StepExpr::builtin_pure_call_with_paths(
                    "strong_busy",
                    &["next.state"],
                    |_prev: &DemoState, _action: &DemoAction, next: &DemoState| {
                        matches!(next, DemoState::Busy)
                    },
                )),
            ]
        }
    }

    struct EmptyInitSpec;

    impl FrontendSpec for EmptyInitSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn frontend_name(&self) -> &'static str {
            "EmptyInitSpec"
        }

        fn initial_states(&self) -> Vec<Self::State> {
            Vec::new()
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Start]
        }
    }

    impl TemporalSpec for EmptyInitSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    #[test]
    fn lowering_records_frontend_name() {
        let lowered = DemoSpec.lower(&mut LoweringCx).expect("lowered");
        assert_eq!(lowered.name(), "DemoSpec");
    }

    #[test]
    fn model_instance_keeps_owned_reduction_metadata() {
        let model: ModelInstance<DemoState, DemoAction> = ModelInstance::new("demo")
            .with_state_abstraction(StateAbstraction::new("phase", |state: &DemoState| {
                format!("{state:?}")
            }));
        assert_eq!(model.label(), "demo");
        assert_eq!(
            model
                .state_abstraction()
                .map(|abstraction| abstraction.name()),
            Some("phase")
        );
    }

    #[test]
    fn lowering_wraps_next_in_box_action_over_vars() {
        let lowered = DemoSpec.lower(&mut LoweringCx).expect("lowered");

        assert_eq!(
            lowered.core.next,
            ActionExpr::BoxAction {
                action: Box::new(ActionExpr::Pred(IrStateExpr::Const("Start".to_owned()))),
                view: ViewExpr::Vars,
            }
        );
    }

    #[test]
    fn lowering_builds_structured_spec_core() {
        let lowered = StructuredSpec.lower(&mut LoweringCx).expect("lowered");

        assert_eq!(lowered.core.init, IrStateExpr::Const("Idle".to_owned()));
        assert_eq!(
            lowered.core.next,
            ActionExpr::BoxAction {
                action: Box::new(ActionExpr::Rule {
                    name: "start".to_owned(),
                    guard: Box::new(ActionExpr::Pred(IrStateExpr::Ref("can_start".to_owned()))),
                    update: nirvash_ir::UpdateExpr::Sequence(vec![
                        nirvash_ir::UpdateOpDecl::Assign {
                            target: "state".to_owned(),
                            value: nirvash_ir::ValueExpr::Literal("Busy".to_owned()),
                        },
                    ]),
                }),
                view: ViewExpr::Vars,
            }
        );
        assert_eq!(
            lowered.core.invariants,
            vec![IrStateExpr::Ref("state_is_known".to_owned())]
        );
        assert_eq!(
            lowered.core.temporal_props,
            vec![
                TemporalExpr::Always(Box::new(TemporalExpr::State(IrStateExpr::Ref(
                    "is_busy".to_owned(),
                )))),
                TemporalExpr::Enabled(ActionExpr::Pred(IrStateExpr::Ref("busy_step".to_owned(),))),
            ]
        );
        assert_eq!(
            lowered.core.fairness,
            vec![
                FairnessDecl::WF {
                    view: ViewExpr::Vars,
                    action: ActionExpr::Pred(IrStateExpr::Ref("weak_busy".to_owned())),
                },
                FairnessDecl::SF {
                    view: ViewExpr::Vars,
                    action: ActionExpr::Pred(IrStateExpr::Ref("strong_busy".to_owned())),
                },
            ]
        );
    }

    #[test]
    fn lowering_reports_fail_closed_error_for_empty_initial_states() {
        let err = EmptyInitSpec
            .lower(&mut LoweringCx)
            .expect_err("empty initial states should fail-closed");

        assert_eq!(err.spec_name(), "EmptyInitSpec");
        assert_eq!(err.node_kind(), "init");
        assert_eq!(err.label(), "EmptyInitSpec");
        assert!(
            err.detail().contains("empty set"),
            "detail should describe the invalid init set: {}",
            err.detail()
        );
    }
}
