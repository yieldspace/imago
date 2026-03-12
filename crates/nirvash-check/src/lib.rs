pub use nirvash::{
    Counterexample, CounterexampleKind, ExplorationMode, ModelBackend, ModelCase, ModelCaseSource,
    ModelCheckConfig, ModelCheckError, ModelCheckResult, ReachableGraphSnapshot, TemporalSpec,
    Trace,
};

pub struct ModelChecker<'a, T: TemporalSpec + ModelCaseSource>(
    nirvash_backends::BackendModelChecker<'a, T>,
);

impl<'a, T> ModelChecker<'a, T>
where
    T: TemporalSpec + ModelCaseSource + Sync,
    T::State: PartialEq + nirvash::Signature + Send + Sync,
    T::Action: PartialEq + Send + Sync,
{
    pub fn new(spec: &'a T) -> Self {
        Self(nirvash_backends::BackendModelChecker::new(spec))
    }

    pub fn for_case(spec: &'a T, model_case: ModelCase<T::State, T::Action>) -> Self {
        Self(nirvash_backends::BackendModelChecker::for_case(
            spec, model_case,
        ))
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        Self(nirvash_backends::BackendModelChecker::with_config(
            spec, config,
        ))
    }

    pub fn reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.0.reachable_graph_snapshot()
    }

    pub fn full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.0.full_reachable_graph_snapshot()
    }

    pub fn check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_invariants()
    }

    pub fn check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_deadlocks()
    }

    pub fn check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_properties()
    }

    pub fn check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_all()
    }

    pub fn simulate(&self) -> Result<Vec<Trace<T::State, T::Action>>, ModelCheckError> {
        self.0.simulate()
    }

    pub fn backend(&self) -> ModelBackend {
        self.0.backend()
    }

    pub fn doc_backend(&self) -> ModelBackend {
        self.0.doc_backend()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::ModelChecker;
    use nirvash::{
        BoolExpr, CounterexampleMinimization, ExplicitCheckpointOptions,
        ExplicitDistributedOptions, ExplicitParallelOptions, ExplicitReachabilityStrategy,
        ExplicitSimulationOptions, ExplicitStateStorage, ExplorationMode, ExprDomain, GuardExpr,
        Ltl, ModelBackend, ModelCase, ModelCheckConfig, Signature, StepExpr, SymbolicSort,
        SymbolicSortSpec, SymbolicStateSchema, SymbolicStateSpec, TemporalSpec, TraceStep,
        TransitionProgram, TransitionRule, TransitionSystem, UpdateOp, UpdateProgram,
        UpdateValueExprAst, inventory, registry::RegisteredSymbolicStateSchema,
        symbolic_leaf_field,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Slot {
        Zero,
        One,
        Two,
    }

    impl Signature for Slot {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![Self::Zero, Self::One, Self::Two])
        }
    }

    impl SymbolicSortSpec for Slot {
        fn symbolic_sort() -> SymbolicSort {
            SymbolicSort::finite::<Self>()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum QuantAction {
        Advance,
        Reset,
    }

    impl Signature for QuantAction {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![Self::Advance, Self::Reset])
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct QuantState {
        ready: bool,
        slot: Slot,
    }

    impl Signature for QuantState {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            let mut values = Vec::new();
            for ready in [false, true] {
                for slot in Slot::bounded_domain().into_vec() {
                    values.push(Self { ready, slot });
                }
            }
            nirvash::BoundedDomain::new(values)
        }
    }

    impl SymbolicStateSpec for QuantState {
        fn symbolic_state_schema() -> SymbolicStateSchema<Self> {
            SymbolicStateSchema::new(
                vec![
                    symbolic_leaf_field(
                        "ready",
                        |state: &Self| &state.ready,
                        |state: &mut Self, value: bool| state.ready = value,
                    ),
                    symbolic_leaf_field(
                        "slot",
                        |state: &Self| &state.slot,
                        |state: &mut Self, value: Slot| state.slot = value,
                    ),
                ],
                || QuantState {
                    ready: false,
                    slot: Slot::Zero,
                },
            )
        }
    }

    fn quantified_state_type_id() -> std::any::TypeId {
        std::any::TypeId::of::<QuantState>()
    }

    fn quantified_state_schema() -> Box<dyn std::any::Any> {
        Box::new(<QuantState as SymbolicStateSpec>::symbolic_state_schema())
    }

    inventory::submit! {
        RegisteredSymbolicStateSchema {
            state_type_id: quantified_state_type_id,
            build: quantified_state_schema,
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct StructuralQuantifierSpec;

    impl StructuralQuantifierSpec {
        fn next_slot(slot: Slot) -> Slot {
            match slot {
                Slot::Zero => Slot::One,
                Slot::One | Slot::Two => Slot::Two,
            }
        }
    }

    impl TransitionSystem for StructuralQuantifierSpec {
        type State = QuantState;
        type Action = QuantAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![QuantState {
                ready: true,
                slot: Slot::Zero,
            }]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![QuantAction::Advance, QuantAction::Reset]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "structural_quantifiers",
                vec![
                    TransitionRule::ast(
                        "advance",
                        GuardExpr::exists_in(
                            "advance_ready",
                            ExprDomain::new("flags", [false, true]),
                            "flag && prev.ready && action == advance",
                            &["prev.ready"],
                            |prev: &QuantState, action: &QuantAction, flag: &bool| {
                                *flag && prev.ready && matches!(action, QuantAction::Advance)
                            },
                        ),
                        UpdateProgram::ast(
                            "advance",
                            vec![UpdateOp::assign_ast(
                                "slot",
                                UpdateValueExprAst::builtin_pure_call_with_paths(
                                    "next_slot",
                                    &["prev.slot"],
                                ),
                                |prev: &QuantState,
                                 state: &mut QuantState,
                                 action: &QuantAction| {
                                    if matches!(action, QuantAction::Advance) {
                                        state.slot = Self::next_slot(prev.slot);
                                    }
                                },
                            )],
                        ),
                    ),
                    TransitionRule::ast(
                        "reset",
                        GuardExpr::builtin_pure_call(
                            "is_reset",
                            |_prev: &QuantState, action: &QuantAction| {
                                matches!(action, QuantAction::Reset)
                            },
                        ),
                        UpdateProgram::ast(
                            "reset",
                            vec![UpdateOp::assign_ast(
                                "slot",
                                UpdateValueExprAst::literal("Slot::Zero"),
                                |_prev: &QuantState,
                                 state: &mut QuantState,
                                 _action: &QuantAction| {
                                    state.slot = Slot::Zero;
                                },
                            )],
                        ),
                    ),
                ],
            ))
        }
    }

    impl TemporalSpec for StructuralQuantifierSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![BoolExpr::forall_in(
                "slot_tautology",
                ExprDomain::of_signature("slots"),
                "state.slot == candidate || state.slot != candidate",
                &["state.slot"],
                |state: &QuantState, candidate: &Slot| {
                    state.slot == *candidate || state.slot != *candidate
                },
            )]
        }
    }

    impl nirvash::ModelCaseSource for StructuralQuantifierSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            vec![
                ModelCase::new("structural_quantifiers").with_action_constraint(
                    StepExpr::exists_in(
                        "known_next_slot",
                        ExprDomain::of_signature("slots"),
                        "candidate == next.slot",
                        &["next.slot"],
                        |_prev: &QuantState,
                         _action: &QuantAction,
                         next: &QuantState,
                         candidate: &Slot| *candidate == next.slot,
                    ),
                ),
            ]
        }

        fn default_model_backend(&self) -> Option<ModelBackend> {
            Some(ModelBackend::Symbolic)
        }
    }

    #[test]
    fn symbolic_backend_accepts_structural_quantifiers() {
        let spec = StructuralQuantifierSpec;
        let checker = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                ..ModelCheckConfig::default()
            },
        );

        let snapshot = checker
            .full_reachable_graph_snapshot()
            .expect("symbolic reachable graph should encode structural quantifiers");
        let invariants = checker
            .check_invariants()
            .expect("symbolic invariant check should succeed");

        assert_eq!(checker.backend(), ModelBackend::Symbolic);
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.states.len(), 3);
        assert!(invariants.is_ok());
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SimulationState {
        Left,
        Right,
        Done,
    }

    impl Signature for SimulationState {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![Self::Left, Self::Right, Self::Done])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SimulationAction {
        Finish,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CounterexampleAction {
        TakeLong,
        TakeShort,
        Advance,
        Finish,
    }

    impl Signature for CounterexampleAction {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![
                Self::TakeLong,
                Self::TakeShort,
                Self::Advance,
                Self::Finish,
            ])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CounterexampleState {
        Start,
        Long1,
        Long2,
        Done,
    }

    impl Signature for CounterexampleState {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![Self::Start, Self::Long1, Self::Long2, Self::Done])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct CounterexampleSpec;

    impl TransitionSystem for CounterexampleSpec {
        type State = CounterexampleState;
        type Action = CounterexampleAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![CounterexampleState::Start]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![
                CounterexampleAction::TakeLong,
                CounterexampleAction::TakeShort,
                CounterexampleAction::Advance,
                CounterexampleAction::Finish,
            ]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (CounterexampleState::Start, CounterexampleAction::TakeLong) => {
                    Some(CounterexampleState::Long1)
                }
                (CounterexampleState::Start, CounterexampleAction::TakeShort) => {
                    Some(CounterexampleState::Done)
                }
                (CounterexampleState::Long1, CounterexampleAction::Advance) => {
                    Some(CounterexampleState::Long2)
                }
                (CounterexampleState::Long2, CounterexampleAction::Finish) => {
                    Some(CounterexampleState::Done)
                }
                _ => None,
            }
        }
    }

    impl TemporalSpec for CounterexampleSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::Always(Box::new(Ltl::Pred(BoolExpr::pure_call(
                "not_done",
                |state: &CounterexampleState| !matches!(state, CounterexampleState::Done),
            ))))]
        }
    }

    impl nirvash::ModelCaseSource for CounterexampleSpec {}

    impl Signature for SimulationAction {
        fn bounded_domain() -> nirvash::BoundedDomain<Self> {
            nirvash::BoundedDomain::new(vec![Self::Finish])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct SimulationSpec;

    impl TransitionSystem for SimulationSpec {
        type State = SimulationState;
        type Action = SimulationAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![SimulationState::Left, SimulationState::Right]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![SimulationAction::Finish]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (SimulationState::Left, SimulationAction::Finish)
                | (SimulationState::Right, SimulationAction::Finish) => Some(SimulationState::Done),
                (SimulationState::Done, SimulationAction::Finish) => None,
            }
        }

        fn allow_stutter(&self) -> bool {
            false
        }
    }

    impl TemporalSpec for SimulationSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl nirvash::ModelCaseSource for SimulationSpec {}

    fn checkpoint_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nirvash-check-{label}-{}-{stamp}.json",
            std::process::id()
        ))
    }

    #[test]
    fn explicit_simulation_is_seed_reproducible() {
        let spec = SimulationSpec;
        let explicit = nirvash::ExplicitModelCheckOptions::current()
            .with_simulation(ExplicitSimulationOptions::new(2, 4, 1));
        let config = ModelCheckConfig::reachable_graph().with_explicit_options(explicit);

        let left_run = ModelChecker::with_config(&spec, config.clone())
            .simulate()
            .expect("explicit simulation should run");
        let left_run_again = ModelChecker::with_config(&spec, config)
            .simulate()
            .expect("explicit simulation should be reproducible");
        let right_run = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_simulation(ExplicitSimulationOptions::new(2, 4, 2)),
            ),
        )
        .simulate()
        .expect("different seed should still run");

        assert_eq!(left_run, left_run_again);
        assert_eq!(left_run.len(), 2);
        assert_eq!(left_run[0].states()[0], SimulationState::Left);
        assert_eq!(right_run[0].states()[0], SimulationState::Right);
        assert!(matches!(
            left_run[0].steps().last(),
            Some(TraceStep::Stutter)
        ));
    }

    #[test]
    fn explicit_reachable_graph_matches_fingerprinted_storage() {
        let spec = SimulationSpec;
        let exact = ModelChecker::with_config(&spec, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let fingerprinted = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_state_storage(ExplicitStateStorage::InMemoryFingerprinted),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("fingerprinted storage snapshot");

        assert_eq!(fingerprinted, exact);
    }

    #[test]
    fn explicit_reachable_graph_roundtrips_checkpoint_file() {
        let spec = SimulationSpec;
        let path = checkpoint_path("reachable-graph");
        let explicit = nirvash::ExplicitModelCheckOptions::current().with_checkpoint(
            ExplicitCheckpointOptions::at_path(path.display().to_string()),
        );
        let config = ModelCheckConfig::reachable_graph().with_explicit_options(explicit);

        let first = ModelChecker::with_config(&spec, config.clone())
            .full_reachable_graph_snapshot()
            .expect("checkpointed snapshot");
        let second = ModelChecker::with_config(&spec, config)
            .full_reachable_graph_snapshot()
            .expect("resumed checkpointed snapshot");

        assert_eq!(second, first);
        assert!(path.exists());
        fs::remove_file(path).expect("cleanup checkpoint file");
    }

    #[test]
    fn explicit_reachable_graph_matches_parallel_frontier_strategy() {
        let spec = SimulationSpec;
        let exact = ModelChecker::with_config(&spec, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let parallel = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_reachability(ExplicitReachabilityStrategy::ParallelFrontier)
                    .with_parallel(ExplicitParallelOptions::current().with_workers(2)),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("parallel frontier snapshot");

        assert_eq!(parallel, exact);
    }

    #[test]
    fn explicit_reachable_graph_matches_distributed_frontier_strategy() {
        let spec = SimulationSpec;
        let exact = ModelChecker::with_config(&spec, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let distributed = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_reachability(ExplicitReachabilityStrategy::DistributedFrontier)
                    .with_distributed(ExplicitDistributedOptions::current().with_shards(3)),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("distributed frontier snapshot");

        assert_eq!(distributed, exact);
    }

    #[test]
    fn parallel_frontier_rejects_model_case_constraints() {
        let spec = SimulationSpec;
        let model_case = ModelCase::new("parallel_constraints")
            .with_checker_config(
                ModelCheckConfig::reachable_graph().with_explicit_options(
                    nirvash::ExplicitModelCheckOptions::current()
                        .with_reachability(ExplicitReachabilityStrategy::ParallelFrontier)
                        .with_parallel(ExplicitParallelOptions::current().with_workers(2)),
                ),
            )
            .with_state_constraint(BoolExpr::pure_call(
                "always_true_constraint",
                |_state: &SimulationState| true,
            ));

        let err = ModelChecker::for_case(&spec, model_case)
            .full_reachable_graph_snapshot()
            .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("parallel frontier exploration")
        ));
    }

    #[test]
    fn counterexample_minimization_prefers_shorter_property_trace() {
        let spec = CounterexampleSpec;
        let without = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph()
                .with_counterexample_minimization(CounterexampleMinimization::None),
        )
        .check_properties()
        .expect("property check should run");
        let with = ModelChecker::with_config(
            &spec,
            ModelCheckConfig::reachable_graph()
                .with_counterexample_minimization(CounterexampleMinimization::ShortestTrace),
        )
        .check_properties()
        .expect("property check should run");

        let without_trace = &without.violations()[0].trace;
        let with_trace = &with.violations()[0].trace;

        assert!(without_trace.len() > with_trace.len());
        assert_eq!(
            without_trace.steps()[0],
            TraceStep::Action(CounterexampleAction::TakeLong)
        );
        assert_eq!(
            with_trace.steps()[0],
            TraceStep::Action(CounterexampleAction::TakeShort)
        );
    }

    #[test]
    fn symbolic_backend_rejects_simulation_mode() {
        let spec = StructuralQuantifierSpec;
        let err = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .simulate()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("simulation")
        ));
    }
}
