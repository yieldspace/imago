mod checker;
pub mod concurrent;
pub mod conformance;
mod doc_graph;
mod domain;
mod dsl_macros;
mod fairness;
mod ltl;
mod predicate;
pub mod registry;
mod relation;
mod symmetry;
mod system;
mod trace;

pub use checker::{
    Counterexample, CounterexampleKind, ExplorationMode, ModelCheckConfig, ModelCheckError,
    ModelCheckResult, ModelChecker,
};
pub use concurrent::{ConcurrentAction, ConcurrentTransitionSystem};
pub use conformance::{
    DynamicTestCase, NegativeWitness, PositiveWitness, ProtocolConformanceSpec,
    ProtocolInputWitnessBinding, ProtocolInputWitnessCodec, ProtocolRuntimeBinding,
    RegisteredCodeWitnessTestProvider, WitnessFamily, WitnessKind,
    run_registered_code_witness_tests,
};
pub use doc_graph::{
    DocGraphCase, DocGraphEdge, DocGraphPolicy, DocGraphProvider, DocGraphReductionMode,
    DocGraphSnapshot, DocGraphSpec, DocGraphState, ReachableGraphEdge, ReachableGraphSnapshot,
    ReducedDocGraph, ReducedDocGraphEdge, ReducedDocGraphNode, RegisteredDocGraphProvider,
    collect_doc_graph_specs, format_doc_graph_action, reduce_doc_graph, summarize_doc_graph_state,
    summarize_doc_graph_text,
};
pub use domain::{
    BoundedDomain, IntoBoundedDomain, OpaqueModelValue, Signature, bounded_vec_domain,
    into_bounded_domain,
};
pub use fairness::Fairness;
pub use inventory;
pub use ltl::Ltl;
pub use predicate::{ActionConstraint, StateConstraint, StatePredicate, StepPredicate};
pub use registry::RegisteredActionDocLabel;
pub use relation::{
    RegisteredRelationalState, RelAtom, RelSet, Relation2, RelationError, RelationField,
    RelationFieldKind, RelationFieldSchema, RelationFieldSummary, RelationalState,
    collect_relational_state_schema, collect_relational_state_summary,
};
pub use symmetry::SymmetryReducer;
pub use system::{
    ActionApplier, ActionVocabulary, ModelCase, ModelCaseSource, StateObserver, SystemComposition,
    TemporalSpec, TransitionSystem,
};
pub use trace::{Trace, TraceStep};

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::pin,
        sync::Mutex,
        task::{Context, Poll, Waker},
    };

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestState {
        Idle,
        Busy,
    }

    impl Signature for TestState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Start,
        Stop,
    }

    impl Signature for TestAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start, Self::Stop])
        }
    }

    fn is_idle(state: &TestState) -> bool {
        matches!(state, TestState::Idle)
    }

    fn starts_work(prev: &TestState, action: &TestAction, next: &TestState) -> bool {
        matches!(
            (prev, action, next),
            (TestState::Idle, TestAction::Start, TestState::Busy)
        )
    }

    #[test]
    fn bounded_vec_domain_enumerates_lengths_with_cartesian_product() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Tiny {
            Zero,
            One,
        }

        impl Signature for Tiny {
            fn bounded_domain() -> BoundedDomain<Self> {
                BoundedDomain::new(vec![Self::Zero, Self::One])
            }
        }

        let values = bounded_vec_domain::<Tiny>(0, 2).into_vec();
        assert_eq!(values.len(), 1 + 2 + 4);
        assert!(values.contains(&vec![]));
        assert!(values.contains(&vec![Tiny::Zero]));
        assert!(values.contains(&vec![Tiny::One, Tiny::Zero]));
    }

    #[test]
    fn into_bounded_domain_accepts_vec_and_array() {
        let from_vec = into_bounded_domain(vec![1_u8, 2, 3]).into_vec();
        let from_array = into_bounded_domain([4_u8, 5]).into_vec();

        assert_eq!(from_vec, vec![1, 2, 3]);
        assert_eq!(from_array, vec![4, 5]);
    }

    #[test]
    fn signature_spec_macro_filters_manual_domains() {
        #[derive(Debug, Clone, PartialEq, Eq)]
        struct ManualState {
            ready: bool,
        }

        trait ManualStateSignatureSpec: Sized {
            fn representatives() -> BoundedDomain<Self>;

            fn signature_invariant(&self) -> bool {
                true
            }
        }

        crate::signature_spec!(
            ManualStateSignatureSpec for ManualState,
            representatives = vec![ManualState { ready: false }, ManualState { ready: true }],
            filter(state) => state.ready,
            invariant(state) => state.ready,
        );

        let values = <ManualState as ManualStateSignatureSpec>::representatives().into_vec();
        assert_eq!(values, vec![ManualState { ready: true }]);
        assert!(
            <ManualState as ManualStateSignatureSpec>::signature_invariant(&ManualState {
                ready: true
            })
        );
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TestSpec;

    impl TransitionSystem for TestSpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start, TestAction::Stop]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (TestState::Idle, TestAction::Start) => Some(TestState::Busy),
                (TestState::Busy, TestAction::Stop) => Some(TestState::Idle),
                _ => None,
            }
        }
    }

    impl TemporalSpec for TestSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            vec![StatePredicate::new("known_state", |_| true)]
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![
                Ltl::leads_to(
                    Ltl::pred(StatePredicate::new("busy", |state| {
                        matches!(state, TestState::Busy)
                    })),
                    Ltl::pred(StatePredicate::new("idle", |state| {
                        matches!(state, TestState::Idle)
                    })),
                ),
                Ltl::always(Ltl::implies(
                    Ltl::enabled(StepPredicate::new("can_stop", |prev, action, next| {
                        matches!(
                            (prev, action, next),
                            (TestState::Busy, TestAction::Stop, TestState::Idle)
                        )
                    })),
                    Ltl::eventually(Ltl::pred(StatePredicate::new("idle_again", |state| {
                        matches!(state, TestState::Idle)
                    }))),
                )),
            ]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::strong(StepPredicate::new(
                "stop",
                |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (TestState::Busy, TestAction::Stop, TestState::Idle)
                    )
                },
            ))]
        }
    }

    impl ModelCaseSource for TestSpec {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockState {
        Idle,
        Busy,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockAction {
        Start,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DeadlockSpec;

    impl TransitionSystem for DeadlockSpec {
        type State = DeadlockState;
        type Action = DeadlockAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DeadlockState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DeadlockAction::Start]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (DeadlockState::Idle, DeadlockAction::Start) => Some(DeadlockState::Busy),
                _ => None,
            }
        }
    }

    impl TemporalSpec for DeadlockSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for DeadlockSpec {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterState {
        Cold,
        Warm,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterAction {
        Tick,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct StutterSpec;

    impl TransitionSystem for StutterSpec {
        type State = StutterState;
        type Action = StutterAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![StutterState::Cold]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![StutterAction::Tick]
        }

        fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
            None
        }

        fn stutter_state(&self, state: &Self::State) -> Self::State {
            match state {
                StutterState::Cold => StutterState::Warm,
                StutterState::Warm => StutterState::Warm,
            }
        }
    }

    impl TemporalSpec for StutterSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(StatePredicate::new(
                "warm",
                |state| matches!(state, StutterState::Warm),
            )))]
        }
    }

    impl ModelCaseSource for StutterSpec {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ControlState {
        Idle,
        Busy,
        Blocked,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ControlAction {
        Start,
        Stop,
        Block,
    }

    #[derive(Debug, Clone, Copy)]
    struct ConstraintSpec {
        constrained: bool,
    }

    impl TransitionSystem for ConstraintSpec {
        type State = ControlState;
        type Action = ControlAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![ControlState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![
                ControlAction::Start,
                ControlAction::Stop,
                ControlAction::Block,
            ]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (ControlState::Idle, ControlAction::Start) => Some(ControlState::Busy),
                (ControlState::Busy, ControlAction::Stop) => Some(ControlState::Idle),
                (ControlState::Busy, ControlAction::Block) => Some(ControlState::Blocked),
                (ControlState::Blocked, ControlAction::Stop) => Some(ControlState::Idle),
                _ => None,
            }
        }
    }

    impl TemporalSpec for ConstraintSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::leads_to(
                Ltl::pred(StatePredicate::new("busy", |state| {
                    matches!(state, ControlState::Busy)
                })),
                Ltl::pred(StatePredicate::new("idle", |state| {
                    matches!(state, ControlState::Idle)
                })),
            )]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::weak(StepPredicate::new(
                "stop_progress",
                |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (ControlState::Busy, ControlAction::Stop, ControlState::Idle)
                    )
                },
            ))]
        }
    }

    impl ModelCaseSource for ConstraintSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            if self.constrained {
                vec![
                    ModelCase::default()
                        .with_state_constraint(StateConstraint::new("exclude_blocked", |state| {
                            !matches!(state, ControlState::Blocked)
                        }))
                        .with_action_constraint(ActionConstraint::new(
                            "disallow_block",
                            |_, action, _| !matches!(action, ControlAction::Block),
                        ))
                        .with_check_deadlocks(false),
                ]
            } else {
                vec![ModelCase::default().with_check_deadlocks(false)]
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FilteredGraphState {
        Idle,
        Busy,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FilteredGraphAction {
        Start,
        Block,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct FilteredGraphSpec;

    impl TransitionSystem for FilteredGraphSpec {
        type State = FilteredGraphState;
        type Action = FilteredGraphAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![FilteredGraphState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![FilteredGraphAction::Start, FilteredGraphAction::Block]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (FilteredGraphState::Idle, FilteredGraphAction::Start) => {
                    Some(FilteredGraphState::Busy)
                }
                (FilteredGraphState::Idle, FilteredGraphAction::Block) => {
                    Some(FilteredGraphState::Idle)
                }
                _ => None,
            }
        }

        fn successors(&self, _: &Self::State) -> Vec<(Self::Action, Self::State)> {
            panic!("reachable graph should use successors_constrained for constrained exploration")
        }

        fn successors_constrained(
            &self,
            _state: &Self::State,
            action_allowed: &dyn Fn(&Self::Action, &Self::State) -> bool,
        ) -> Vec<(Self::Action, Self::State)> {
            [
                (FilteredGraphAction::Start, FilteredGraphState::Busy),
                (FilteredGraphAction::Block, FilteredGraphState::Idle),
            ]
            .into_iter()
            .filter(|(action, next)| action_allowed(action, next))
            .collect()
        }
    }

    impl TemporalSpec for FilteredGraphSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for FilteredGraphSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            vec![
                ModelCase::default().with_action_constraint(ActionConstraint::new(
                    "disallow_block",
                    |_, action, _| !matches!(action, FilteredGraphAction::Block),
                )),
            ]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SymmetryState {
        Left,
        Right,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SymmetryAction {
        Swap,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct SymmetrySpec;

    impl TransitionSystem for SymmetrySpec {
        type State = SymmetryState;
        type Action = SymmetryAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![SymmetryState::Left, SymmetryState::Right]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![SymmetryAction::Swap]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (SymmetryState::Left, SymmetryAction::Swap) => Some(SymmetryState::Right),
                (SymmetryState::Right, SymmetryAction::Swap) => Some(SymmetryState::Left),
            }
        }
    }

    impl TemporalSpec for SymmetrySpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(StatePredicate::new(
                "left",
                |state| matches!(state, SymmetryState::Left),
            )))]
        }
    }

    impl ModelCaseSource for SymmetrySpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            vec![
                ModelCase::default()
                    .with_symmetry(SymmetryReducer::new("collapse_lr", |_| SymmetryState::Left))
                    .with_check_deadlocks(false),
            ]
        }
    }

    #[test]
    fn ltl_ast_builders_preserve_formula_shape() {
        let idle = StatePredicate::new("idle", is_idle);
        let start = StepPredicate::new("start", starts_work);

        let formula = Ltl::always(Ltl::until(
            Ltl::pred(idle),
            Ltl::and(Ltl::step(start), Ltl::enabled(start)),
        ));
        match formula {
            Ltl::Always(inner) => match *inner {
                Ltl::Until(lhs, rhs) => {
                    assert!(matches!(*lhs, Ltl::Pred(_)));
                    assert!(matches!(*rhs, Ltl::And(_, _)));
                }
                other => panic!("unexpected inner formula: {other:?}"),
            },
            other => panic!("unexpected formula: {other:?}"),
        }
    }

    #[test]
    fn predicates_evaluate_against_state_and_transition() {
        let idle = StatePredicate::new("idle", is_idle);
        let start = StepPredicate::new("start", starts_work);

        assert!(idle.eval(&TestState::Idle));
        assert!(!idle.eval(&TestState::Busy));
        assert!(start.eval(&TestState::Idle, &TestAction::Start, &TestState::Busy));
        assert!(!start.eval(&TestState::Busy, &TestAction::Stop, &TestState::Idle));
    }

    #[test]
    fn system_composition_collects_typed_spec_fragments_and_model_cases() {
        let model_case = ModelCase::<TestState, TestAction>::default()
            .with_checker_config(ModelCheckConfig::bounded_lasso(5));
        let composition = SystemComposition::new("test-system")
            .with_subsystem("manager")
            .with_subsystem("runtime")
            .with_invariant(StatePredicate::new("idle", is_idle))
            .with_property(Ltl::eventually(Ltl::pred(StatePredicate::new(
                "idle", is_idle,
            ))))
            .with_fairness(Fairness::weak(StepPredicate::new("start", starts_work)))
            .with_model_case(model_case.clone());

        assert_eq!(composition.name(), "test-system");
        assert_eq!(composition.subsystems(), ["manager", "runtime"]);
        assert_eq!(composition.invariants().len(), 1);
        assert_eq!(composition.properties().len(), 1);
        assert_eq!(composition.fairness().len(), 1);
        assert_eq!(composition.model_cases().len(), 1);
        assert_eq!(
            composition.model_cases()[0]
                .effective_checker_config()
                .bounded_depth,
            Some(5)
        );
    }

    #[test]
    fn quantified_builders_expand_over_signature_domains() {
        let formula =
            Ltl::<TestState, TestAction>::forall::<TestAction, _>(|action| match action {
                TestAction::Start | TestAction::Stop => {
                    Ltl::pred(StatePredicate::new("idle", is_idle))
                }
            });

        assert!(formula.describe().contains("/\\"));
    }

    #[test]
    fn model_checker_accepts_simple_lasso_spec() {
        let spec = TestSpec;
        let checker = ModelChecker::with_config(&spec, ModelCheckConfig::bounded_lasso(3));
        assert!(checker.check_invariants().unwrap().is_ok());
        assert!(checker.check_deadlocks().unwrap().is_ok());
        assert!(checker.check_properties().unwrap().is_ok());
    }

    #[test]
    fn model_checker_detects_deadlocks_and_respects_toggle() {
        let spec = DeadlockSpec;
        let deadlocks = ModelChecker::new(&spec).check_deadlocks().unwrap();
        assert!(!deadlocks.is_ok());
        assert!(matches!(
            deadlocks.violations()[0].kind,
            CounterexampleKind::Deadlock
        ));

        let checker = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        );
        assert!(checker.check_deadlocks().unwrap().is_ok());
    }

    #[test]
    fn stutter_state_can_drive_temporal_progress() {
        let spec = StutterSpec;
        let checker = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        );
        assert!(checker.check_properties().unwrap().is_ok());
    }

    #[test]
    fn reachable_graph_snapshot_exposes_states_edges_and_initials() {
        let snapshot = ModelChecker::new(&TestSpec)
            .reachable_graph_snapshot()
            .expect("snapshot should build");

        assert_eq!(snapshot.states, vec![TestState::Idle, TestState::Busy]);
        assert_eq!(snapshot.initial_indices, vec![0]);
        assert!(snapshot.deadlocks.is_empty());
        assert!(!snapshot.truncated);
        assert!(snapshot.stutter_omitted);
        assert_eq!(
            snapshot.edges[0],
            vec![ReachableGraphEdge {
                action: TestAction::Start,
                target: 1,
            }]
        );
        assert_eq!(
            snapshot.edges[1],
            vec![ReachableGraphEdge {
                action: TestAction::Stop,
                target: 0,
            }]
        );
    }

    #[test]
    fn reachable_graph_snapshot_omits_stutter_edges_but_marks_deadlocks() {
        let snapshot = ModelChecker::new(&StutterSpec)
            .reachable_graph_snapshot()
            .expect("snapshot should build");

        assert_eq!(
            snapshot.states,
            vec![StutterState::Cold, StutterState::Warm]
        );
        assert!(snapshot.edges.iter().all(Vec::is_empty));
        assert_eq!(snapshot.initial_indices, vec![0]);
        assert_eq!(snapshot.deadlocks, vec![0, 1]);
        assert!(snapshot.stutter_omitted);
    }

    #[test]
    fn full_reachable_graph_snapshot_ignores_doc_only_limits() {
        let model_case = ModelCase::default().with_doc_checker_config(ModelCheckConfig {
            exploration: ExplorationMode::ReachableGraph,
            bounded_depth: None,
            max_states: Some(1),
            max_transitions: Some(1),
            check_deadlocks: true,
            stop_on_first_violation: false,
        });
        let checker = ModelChecker::for_case(&TestSpec, model_case);

        let doc_snapshot = checker
            .reachable_graph_snapshot()
            .expect("doc snapshot should build");
        let full_snapshot = checker
            .full_reachable_graph_snapshot()
            .expect("full snapshot should build");

        assert!(doc_snapshot.truncated);
        assert_eq!(doc_snapshot.states.len(), 1);
        assert!(!full_snapshot.truncated);
        assert_eq!(full_snapshot.states, vec![TestState::Idle, TestState::Busy]);
    }

    #[test]
    fn constraints_prune_problematic_edges_from_the_graph() {
        let unconstrained = ConstraintSpec { constrained: false };
        let constrained = ConstraintSpec { constrained: true };
        let config = ModelCheckConfig {
            check_deadlocks: false,
            ..ModelCheckConfig::default()
        };

        assert!(
            !ModelChecker::with_config(&unconstrained, config)
                .check_properties()
                .unwrap()
                .is_ok()
        );
        assert!(
            ModelChecker::with_config(&constrained, config)
                .check_properties()
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn reachable_graph_snapshot_respects_constraints_and_symmetry() {
        let constrained = ConstraintSpec { constrained: true };
        let constrained_snapshot = ModelChecker::with_config(
            &constrained,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        )
        .reachable_graph_snapshot()
        .expect("snapshot should build");
        assert_eq!(
            constrained_snapshot.states,
            vec![ControlState::Idle, ControlState::Busy]
        );
        assert_eq!(
            constrained_snapshot.edges[0],
            vec![ReachableGraphEdge {
                action: ControlAction::Start,
                target: 1,
            }]
        );
        assert_eq!(
            constrained_snapshot.edges[1],
            vec![ReachableGraphEdge {
                action: ControlAction::Stop,
                target: 0,
            }]
        );

        let symmetry_snapshot = ModelChecker::with_config(
            &SymmetrySpec,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        )
        .reachable_graph_snapshot()
        .expect("snapshot should build");
        assert_eq!(symmetry_snapshot.states, vec![SymmetryState::Left]);
        assert_eq!(symmetry_snapshot.initial_indices, vec![0]);
        assert_eq!(
            symmetry_snapshot.edges[0],
            vec![ReachableGraphEdge {
                action: SymmetryAction::Swap,
                target: 0,
            }]
        );
    }

    #[test]
    fn reachable_graph_uses_successors_constrained_for_filtered_exploration() {
        let snapshot = ModelChecker::new(&FilteredGraphSpec)
            .reachable_graph_snapshot()
            .expect("snapshot should build via constrained successors");
        assert_eq!(
            snapshot.states,
            vec![FilteredGraphState::Idle, FilteredGraphState::Busy]
        );
        assert_eq!(
            snapshot.edges[0],
            vec![ReachableGraphEdge {
                action: FilteredGraphAction::Start,
                target: 1,
            }]
        );
    }

    #[test]
    fn symmetry_with_temporal_properties_fails_closed() {
        let spec = SymmetrySpec;
        let err = ModelChecker::new(&spec).check_properties().unwrap_err();
        assert!(matches!(err, ModelCheckError::UnsupportedConfiguration(_)));
    }

    #[test]
    fn opaque_model_values_are_bounded() {
        struct OpaqueTag;

        let domain = OpaqueModelValue::<OpaqueTag, 3>::bounded_domain().into_vec();
        assert_eq!(domain.len(), 3);
        assert_eq!(domain[0].index(), 0);
        assert_eq!(domain[2].index(), 2);
    }

    #[test]
    fn pred_and_step_macros_preserve_names_and_behavior() {
        let idle = crate::pred!(idle(state) => matches!(state, TestState::Idle));
        let start = crate::step!(start(prev, action, next) => matches!(
            (prev, action, next),
            (TestState::Idle, TestAction::Start, TestState::Busy)
        ));

        assert_eq!(idle.name(), "idle");
        assert!(idle.eval(&TestState::Idle));
        assert!(!idle.eval(&TestState::Busy));

        assert_eq!(start.name(), "start");
        assert!(start.eval(&TestState::Idle, &TestAction::Start, &TestState::Busy));
        assert!(!start.eval(&TestState::Busy, &TestAction::Start, &TestState::Busy));
    }

    #[test]
    fn ltl_macro_builds_expected_formula_shape() {
        let formula: Ltl<TestState, TestAction> = crate::ltl!(always(until(
            (pred!(idle(state) => matches!(state, TestState::Idle))),
            ((step!(start(prev, action, next) => matches!(
                (prev, action, next),
                (TestState::Idle, TestAction::Start, TestState::Busy)
            ))) && (enabled(step!(stop(prev, action, next) => matches!(
                (prev, action, next),
                (TestState::Busy, TestAction::Stop, TestState::Idle)
            )))))
        )));

        let expected: Ltl<TestState, TestAction> = Ltl::always(Ltl::until(
            Ltl::pred(StatePredicate::new("idle", |state| {
                matches!(state, TestState::Idle)
            })),
            Ltl::and(
                Ltl::step(StepPredicate::new("start", |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (TestState::Idle, TestAction::Start, TestState::Busy)
                    )
                })),
                Ltl::enabled(StepPredicate::new("stop", |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (TestState::Busy, TestAction::Stop, TestState::Idle)
                    )
                })),
            ),
        ));

        assert_eq!(formula.describe(), expected.describe());
    }

    #[test]
    fn ltl_macro_supports_boolean_and_temporal_operators() {
        let formula: Ltl<TestState, TestAction> = crate::ltl!(always(
            (((! pred!(idle(state) => matches!(state, TestState::Idle))))
                => (eventually(pred!(busy(state) => matches!(state, TestState::Busy)))))
        ));

        let expected: Ltl<TestState, TestAction> = Ltl::always(Ltl::implies(
            Ltl::negate(Ltl::pred(StatePredicate::new("idle", |state| {
                matches!(state, TestState::Idle)
            }))),
            Ltl::eventually(Ltl::pred(StatePredicate::new("busy", |state| {
                matches!(state, TestState::Busy)
            }))),
        ));

        assert_eq!(formula.describe(), expected.describe());
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestOutput {
        Ack,
        Rejected,
    }

    struct TestRuntime {
        state: Mutex<TestState>,
    }

    impl ActionApplier for TestRuntime {
        type Action = TestAction;
        type Output = TestOutput;
        type Context = ();

        async fn execute_action(
            &self,
            _context: &Self::Context,
            action: &Self::Action,
        ) -> Self::Output {
            let mut state = self.state.lock().expect("lock test runtime state");
            match (*state, action) {
                (TestState::Idle, TestAction::Start) => {
                    *state = TestState::Busy;
                    TestOutput::Ack
                }
                (TestState::Busy, TestAction::Stop) => {
                    *state = TestState::Idle;
                    TestOutput::Ack
                }
                _ => TestOutput::Rejected,
            }
        }
    }

    impl StateObserver for TestRuntime {
        type SummaryState = TestState;
        type Context = ();

        async fn observe_state(&self, _context: &Self::Context) -> Self::SummaryState {
            *self.state.lock().expect("lock test runtime state")
        }
    }

    fn block_on_ready<F>(future: F) -> F::Output
    where
        F: Future,
    {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("test future unexpectedly returned Pending"),
        }
    }

    #[test]
    fn conformance_traits_support_runtime_replay() {
        let runtime = TestRuntime {
            state: Mutex::new(TestState::Idle),
        };

        let initial = block_on_ready(<TestRuntime as StateObserver>::observe_state(&runtime, &()));
        assert_eq!(initial, TestState::Idle);

        let start_output = block_on_ready(<TestRuntime as ActionApplier>::execute_action(
            &runtime,
            &(),
            &TestAction::Start,
        ));
        assert_eq!(start_output, TestOutput::Ack);
        let busy = block_on_ready(<TestRuntime as StateObserver>::observe_state(&runtime, &()));
        assert_eq!(busy, TestState::Busy);

        let stop_output = block_on_ready(<TestRuntime as ActionApplier>::execute_action(
            &runtime,
            &(),
            &TestAction::Stop,
        ));
        assert_eq!(stop_output, TestOutput::Ack);
        let idle = block_on_ready(<TestRuntime as StateObserver>::observe_state(&runtime, &()));
        assert_eq!(idle, TestState::Idle);
    }
}
