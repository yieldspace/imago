#[allow(dead_code)]
mod concurrent;
pub mod conformance;
mod doc_graph;
mod domain;
mod dsl_macros;
mod fairness;
mod ltl;
mod model;
mod predicate;
pub mod registry;
mod relation;
mod symbolic_state;
mod symmetry;
mod system;
mod trace;

pub use conformance::{
    DynamicTestCase, NegativeWitness, PositiveWitness, ProtocolConformanceSpec,
    ProtocolInputWitnessBinding, ProtocolInputWitnessCodec, ProtocolRuntimeBinding,
    RegisteredCodeWitnessTestProvider, WitnessFamily, WitnessKind,
    run_registered_code_witness_tests,
};
pub use doc_graph::{
    DocGraphActionPresentation, DocGraphCase, DocGraphEdge, DocGraphInteractionStep,
    DocGraphPolicy, DocGraphProcessKind, DocGraphProcessStep, DocGraphProvider,
    DocGraphReductionMode, DocGraphSnapshot, DocGraphSpec, DocGraphState, ReachableGraphEdge,
    ReachableGraphSnapshot, ReducedDocGraph, ReducedDocGraphEdge, ReducedDocGraphNode,
    RegisteredDocGraphProvider, RegisteredSpecVizProvider, SpecVizActionDescriptor, SpecVizBundle,
    SpecVizCase, SpecVizCaseStats, SpecVizKind, SpecVizMetadata, SpecVizProvider,
    SpecVizRegistrationSet, VizPolicy, VizScenario, VizScenarioKind, VizScenarioStep,
    collect_doc_graph_specs, collect_spec_viz_bundles, describe_doc_graph_action,
    format_doc_graph_action, reduce_doc_graph, summarize_doc_graph_state, summarize_doc_graph_text,
};
pub use domain::{
    BoundedDomain, IntoBoundedDomain, OpaqueModelValue, Signature, bounded_vec_domain,
    into_bounded_domain,
};
pub use fairness::Fairness;
pub use inventory;
pub use ltl::Ltl;
pub use model::{
    Counterexample, CounterexampleKind, ExplorationMode, ModelBackend, ModelCheckConfig,
    ModelCheckError, ModelCheckResult,
};
pub use predicate::{
    BoolExpr, BoolExprAst, GuardAst, GuardExpr, GuardValueExpr, QuantifierKind, StateExpr,
    StateExprAst, StepExpr, StepExprAst, StepValueExpr, TransitionProgram, TransitionProgramError,
    TransitionRule, TransitionSuccessor, UpdateAst, UpdateOp, UpdateProgram, UpdateValueExprAst,
};
pub use registry::{RegisteredActionDocLabel, RegisteredActionDocPresentation};
pub use relation::{
    RegisteredRelationalState, RelAtom, RelSet, Relation2, RelationError, RelationField,
    RelationFieldKind, RelationFieldSchema, RelationFieldSummary, RelationalState,
    collect_relational_state_schema, collect_relational_state_summary,
};
pub use symbolic_state::{
    SymbolicStateField, SymbolicStateSchema, SymbolicStateSpec, normalize_symbolic_state_path,
    symbolic_leaf_field, symbolic_leaf_index, symbolic_leaf_value, symbolic_seed_value,
    symbolic_state_fields,
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

    crate::register_symbolic_pure_helpers!(
        "registered_transition_target",
        "start",
        "stop",
        "start_a",
        "start_b"
    );

    macro_rules! register_leaf_symbolic_state_spec {
        ($module:ident, $ty:ty) => {
            mod $module {
                use super::*;

                impl SymbolicStateSpec for $ty {
                    fn symbolic_state_schema() -> SymbolicStateSchema<Self> {
                        SymbolicStateSchema::new(
                            vec![symbolic_leaf_field(
                                "self",
                                |state: &Self| state,
                                |state: &mut Self, value: Self| *state = value,
                            )],
                            || symbolic_seed_value::<Self>(),
                        )
                    }
                }

                fn symbolic_state_type_id() -> std::any::TypeId {
                    std::any::TypeId::of::<$ty>()
                }

                fn build_symbolic_state_schema() -> Box<dyn std::any::Any> {
                    Box::new(<$ty as SymbolicStateSpec>::symbolic_state_schema())
                }

                crate::inventory::submit! {
                    crate::registry::RegisteredSymbolicStateSchema {
                        state_type_id: symbolic_state_type_id,
                        build: build_symbolic_state_schema,
                    }
                }
            }
        };
    }

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

    register_leaf_symbolic_state_spec!(test_state_symbolic_schema, TestState);

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

    fn idle_predicate() -> BoolExpr<TestState> {
        crate::pred!(idle(state) => matches!(state, TestState::Idle))
    }

    fn busy_predicate() -> BoolExpr<TestState> {
        crate::pred!(busy(state) => matches!(state, TestState::Busy))
    }

    fn idle_again_predicate() -> BoolExpr<TestState> {
        crate::pred!(idle_again(state) => matches!(state, TestState::Idle))
    }

    fn start_step_predicate() -> StepExpr<TestState, TestAction> {
        crate::step!(start(prev, action, next) => matches!(
            (prev, action, next),
            (TestState::Idle, TestAction::Start, TestState::Busy)
        ))
    }

    fn stop_step_predicate() -> StepExpr<TestState, TestAction> {
        crate::step!(stop(prev, action, next) => matches!(
            (prev, action, next),
            (TestState::Busy, TestAction::Stop, TestState::Idle)
        ))
    }

    fn can_stop_step_predicate() -> StepExpr<TestState, TestAction> {
        crate::step!(can_stop(prev, action, next) => matches!(
            (prev, action, next),
            (TestState::Busy, TestAction::Stop, TestState::Idle)
        ))
    }

    fn registered_transition_target(state: &TestState, action: &TestAction) -> Option<TestState> {
        match (state, action) {
            (TestState::Idle, TestAction::Start) => Some(TestState::Busy),
            (TestState::Busy, TestAction::Stop) => Some(TestState::Idle),
            _ => None,
        }
    }

    fn missing_transition_target(state: &TestState, action: &TestAction) -> Option<TestState> {
        registered_transition_target(state, action)
    }

    fn test_transition_program() -> TransitionProgram<TestState, TestAction> {
        TransitionProgram::named(
            "test_spec",
            vec![
                TransitionRule::ast(
                    "start",
                    GuardExpr::registered_pure_call("start", "start", |state, action| {
                        matches!((state, action), (TestState::Idle, TestAction::Start))
                    }),
                    UpdateProgram::ast(
                        "start",
                        vec![UpdateOp::assign_ast(
                            "self",
                            UpdateValueExprAst::literal("TestState::Busy"),
                            |_prev: &TestState, state: &mut TestState, _action: &TestAction| {
                                *state = TestState::Busy;
                            },
                        )],
                    ),
                ),
                TransitionRule::ast(
                    "stop",
                    GuardExpr::registered_pure_call("stop", "stop", |state, action| {
                        matches!((state, action), (TestState::Busy, TestAction::Stop))
                    }),
                    UpdateProgram::ast(
                        "stop",
                        vec![UpdateOp::assign_ast(
                            "self",
                            UpdateValueExprAst::literal("TestState::Idle"),
                            |_prev: &TestState, state: &mut TestState, _action: &TestAction| {
                                *state = TestState::Idle;
                            },
                        )],
                    ),
                ),
            ],
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

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(test_transition_program())
        }
    }

    impl TemporalSpec for TestSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![crate::pred!(known_state(_state) => true)]
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![
                Ltl::leads_to(Ltl::pred(busy_predicate()), Ltl::pred(idle_predicate())),
                Ltl::always(Ltl::implies(
                    Ltl::enabled(can_stop_step_predicate()),
                    Ltl::eventually(Ltl::pred(idle_again_predicate())),
                )),
            ]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::strong(stop_step_predicate())]
        }
    }

    impl ModelCaseSource for TestSpec {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockState {
        Idle,
        Busy,
    }

    impl Signature for DeadlockState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    register_leaf_symbolic_state_spec!(deadlock_state_symbolic_schema, DeadlockState);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockAction {
        Start,
    }

    impl Signature for DeadlockAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start])
        }
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
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for DeadlockSpec {}

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterState {
        Cold,
        Warm,
    }

    impl Signature for StutterState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Cold, Self::Warm])
        }
    }

    register_leaf_symbolic_state_spec!(stutter_state_symbolic_schema, StutterState);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterAction {
        Tick,
    }

    impl Signature for StutterAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Tick])
        }
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

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named("stutter", Vec::new()))
        }

        fn stutter_state(&self, state: &Self::State) -> Self::State {
            match state {
                StutterState::Cold => StutterState::Warm,
                StutterState::Warm => StutterState::Warm,
            }
        }
    }

    impl TemporalSpec for StutterSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(crate::pred!(
                warm(state) => matches!(state, StutterState::Warm)
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

    impl Signature for ControlState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy, Self::Blocked])
        }
    }

    register_leaf_symbolic_state_spec!(control_state_symbolic_schema, ControlState);

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
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::leads_to(
                Ltl::pred(crate::pred!(
                    busy(state) => matches!(state, ControlState::Busy)
                )),
                Ltl::pred(crate::pred!(
                    idle(state) => matches!(state, ControlState::Idle)
                )),
            )]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::weak(crate::step!(
                stop_progress(prev, action, next) => matches!(
                    (prev, action, next),
                    (ControlState::Busy, ControlAction::Stop, ControlState::Idle)
                )
            ))]
        }
    }

    impl ModelCaseSource for ConstraintSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            if self.constrained {
                vec![
                    ModelCase::default()
                        .with_state_constraint(crate::pred!(
                            exclude_blocked(state) => !matches!(state, ControlState::Blocked)
                        ))
                        .with_action_constraint(crate::step!(
                            disallow_block(_prev, action, _next) =>
                                !matches!(action, ControlAction::Block)
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

    impl Signature for FilteredGraphState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    register_leaf_symbolic_state_spec!(filtered_graph_state_symbolic_schema, FilteredGraphState);

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
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for FilteredGraphSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            vec![ModelCase::default().with_action_constraint(crate::step!(
                disallow_block(_prev, action, _next) =>
                    !matches!(action, FilteredGraphAction::Block)
            ))]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SymmetryState {
        Left,
        Right,
    }

    impl Signature for SymmetryState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Left, Self::Right])
        }
    }

    register_leaf_symbolic_state_spec!(symmetry_state_symbolic_schema, SymmetryState);

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
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(crate::pred!(
                left(state) => matches!(state, SymmetryState::Left)
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

    #[derive(Debug, Clone, Copy, Default)]
    struct LegacyConstraintSpec;

    impl TransitionSystem for LegacyConstraintSpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start, TestAction::Stop]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(test_transition_program())
        }
    }

    impl TemporalSpec for LegacyConstraintSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for LegacyConstraintSpec {
        fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
            vec![
                ModelCase::default().with_action_constraint(crate::predicate::legacy_step_expr(
                    "legacy_action_constraint",
                    |_, action, _| !matches!(action, TestAction::Stop),
                )),
            ]
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct LegacyPropertySpec;

    impl TransitionSystem for LegacyPropertySpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start, TestAction::Stop]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(test_transition_program())
        }
    }

    impl TemporalSpec for LegacyPropertySpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(
                crate::predicate::legacy_bool_expr("legacy_busy", |state| {
                    matches!(state, TestState::Busy)
                }),
            ))]
        }
    }

    impl ModelCaseSource for LegacyPropertySpec {}

    #[derive(Debug, Clone, Copy, Default)]
    struct MissingRegisteredHelperSpec;

    impl TransitionSystem for MissingRegisteredHelperSpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start, TestAction::Stop]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "missing_helper_spec",
                vec![TransitionRule::ast(
                    "helper_start",
                    GuardExpr::registered_pure_call(
                        "missing_transition_target(prev, action).is_some()",
                        "missing_transition_target",
                        |state, action| missing_transition_target(state, action).is_some(),
                    ),
                    UpdateProgram::ast(
                        "helper_start",
                        vec![UpdateOp::assign_ast(
                            "self",
                            UpdateValueExprAst::registered_pure_call(
                                "missing_transition_target(prev, action).expect(\"helper_start guard matched\")",
                                "missing_transition_target",
                            ),
                            |prev: &TestState, state: &mut TestState, action: &TestAction| {
                                *state = missing_transition_target(prev, action)
                                    .expect("helper_start guard matched");
                            },
                        )],
                    ),
                )],
            ))
        }
    }

    impl TemporalSpec for MissingRegisteredHelperSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for MissingRegisteredHelperSpec {}

    #[derive(Debug, Clone, Copy, Default)]
    struct RegisteredHelperSpec;

    impl TransitionSystem for RegisteredHelperSpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start, TestAction::Stop]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "registered_helper_spec",
                vec![TransitionRule::ast(
                    "helper_start",
                    GuardExpr::registered_pure_call(
                        "registered_transition_target(prev, action).is_some()",
                        "registered_transition_target",
                        |state, action| registered_transition_target(state, action).is_some(),
                    ),
                    UpdateProgram::ast(
                        "helper_start",
                        vec![UpdateOp::assign_ast(
                            "self",
                            UpdateValueExprAst::registered_pure_call(
                                "registered_transition_target(prev, action).expect(\"helper_start guard matched\")",
                                "registered_transition_target",
                            ),
                            |prev: &TestState, state: &mut TestState, action: &TestAction| {
                                *state = registered_transition_target(prev, action)
                                    .expect("helper_start guard matched");
                            },
                        )],
                    ),
                )],
            ))
        }
    }

    impl TemporalSpec for RegisteredHelperSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for RegisteredHelperSpec {}

    #[derive(Debug, Clone, Copy, Default)]
    struct AmbiguousTransitionSpec;

    impl TransitionSystem for AmbiguousTransitionSpec {
        type State = TestState;
        type Action = TestAction;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![TestState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![TestAction::Start]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "ambiguous_symbolic",
                vec![
                    TransitionRule::ast(
                        "start_a",
                        GuardExpr::registered_pure_call("start_a", "start_a", |state, action| {
                            matches!((state, action), (TestState::Idle, TestAction::Start))
                        }),
                        UpdateProgram::ast(
                            "start_a",
                            vec![UpdateOp::assign_ast(
                                "self",
                                UpdateValueExprAst::literal("TestState::Busy"),
                                |_prev: &TestState, state: &mut TestState, _action: &TestAction| {
                                    *state = TestState::Busy;
                                },
                            )],
                        ),
                    ),
                    TransitionRule::ast(
                        "start_b",
                        GuardExpr::registered_pure_call("start_b", "start_b", |state, action| {
                            matches!((state, action), (TestState::Idle, TestAction::Start))
                        }),
                        UpdateProgram::ast(
                            "start_b",
                            vec![UpdateOp::assign_ast(
                                "self",
                                UpdateValueExprAst::literal("TestState::Busy"),
                                |_prev: &TestState, state: &mut TestState, _action: &TestAction| {
                                    *state = TestState::Busy;
                                },
                            )],
                        ),
                    ),
                ],
            ))
        }
    }

    impl TemporalSpec for AmbiguousTransitionSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl ModelCaseSource for AmbiguousTransitionSpec {}

    #[test]
    fn ltl_ast_builders_preserve_formula_shape() {
        let idle = idle_predicate();
        let start = start_step_predicate();

        let formula = Ltl::always(Ltl::until(
            Ltl::pred(idle),
            Ltl::and(Ltl::step(start.clone()), Ltl::enabled(start)),
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
        let idle = idle_predicate();
        let start = start_step_predicate();

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
            .with_invariant(idle_predicate())
            .with_property(Ltl::eventually(Ltl::pred(idle_predicate())))
            .with_fairness(Fairness::weak(start_step_predicate()))
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
                TestAction::Start | TestAction::Stop => Ltl::pred(idle_predicate()),
            });

        assert!(formula.describe().contains("/\\"));
    }

    #[cfg(feature = "checker-tests")]
    mod checker_frontdoor_tests {
        use super::*;
        use nirvash_check::ModelChecker;

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
                backend: None,
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
            Ltl::pred(idle_predicate()),
            Ltl::and(
                Ltl::step(start_step_predicate()),
                Ltl::enabled(stop_step_predicate()),
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
            Ltl::negate(Ltl::pred(idle_predicate())),
            Ltl::eventually(Ltl::pred(busy_predicate())),
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

    #[cfg(feature = "checker-tests")]
    mod symbolic_checker_tests {
        use super::*;
        use nirvash_check::ModelChecker;

        #[test]
        fn symbolic_checker_matches_explicit_snapshot_and_results() {
            let explicit_snapshot = ModelChecker::new(&TestSpec)
                .full_reachable_graph_snapshot()
                .expect("explicit snapshot should build");
            let symbolic_snapshot = ModelChecker::with_config(
                &TestSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .expect("symbolic snapshot should build");
            assert_eq!(symbolic_snapshot, explicit_snapshot);

            let explicit_result = ModelChecker::new(&TestSpec)
                .check_all()
                .expect("explicit checker should run");
            let symbolic_result = ModelChecker::with_config(
                &TestSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .check_all()
            .expect("symbolic checker should run");
            assert_eq!(symbolic_result, explicit_result);
        }

        #[test]
        fn symbolic_checker_rejects_non_identity_stutter_states() {
            let err = ModelChecker::with_config(
                &StutterSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .unwrap_err();
            match err {
                ModelCheckError::UnsupportedConfiguration(message) => {
                    assert!(message.contains("stutter_state"));
                }
                other => panic!("unexpected error: {other:?}"),
            }
        }

        #[test]
        fn symbolic_checker_rejects_specs_without_transition_program() {
            let err = ModelChecker::with_config(
                &FilteredGraphSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .unwrap_err();
            assert!(matches!(err, ModelCheckError::UnsupportedConfiguration(_)));
        }

        #[test]
        fn symbolic_checker_rejects_unregistered_transition_helpers() {
            let err = ModelChecker::with_config(
                &MissingRegisteredHelperSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .unwrap_err();
            match err {
                ModelCheckError::UnsupportedConfiguration(message) => {
                    assert!(message.contains("missing_transition_target"));
                }
                other => panic!("unexpected error: {other:?}"),
            }
        }

        #[test]
        fn symbolic_checker_accepts_registered_transition_helpers() {
            let snapshot = ModelChecker::with_config(
                &RegisteredHelperSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .expect("registered symbolic helper snapshot should build");
            assert_eq!(snapshot.states, vec![TestState::Idle, TestState::Busy]);
        }

        #[test]
        fn symbolic_checker_rejects_legacy_constraints_and_properties() {
            let legacy_constraint_err = ModelChecker::with_config(
                &LegacyConstraintSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .unwrap_err();
            assert!(matches!(
                legacy_constraint_err,
                ModelCheckError::UnsupportedConfiguration(_)
            ));

            let legacy_property_err = ModelChecker::with_config(
                &LegacyPropertySpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .check_properties()
            .unwrap_err();
            assert!(matches!(
                legacy_property_err,
                ModelCheckError::UnsupportedConfiguration(_)
            ));
        }

        #[test]
        fn symbolic_checker_accepts_relation_ast_transition_programs() {
            let snapshot = ModelChecker::with_config(
                &AmbiguousTransitionSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::reachable_graph()
                },
            )
            .full_reachable_graph_snapshot()
            .expect("symbolic relation snapshot should build");
            assert_eq!(snapshot.states, vec![TestState::Idle, TestState::Busy]);
        }

        #[test]
        fn symbolic_checker_matches_explicit_bounded_lasso_results() {
            let explicit_result = ModelChecker::with_config(
                &TestSpec,
                ModelCheckConfig {
                    ..ModelCheckConfig::bounded_lasso(3)
                },
            )
            .check_all()
            .expect("explicit bounded lasso should run");
            let symbolic_result = ModelChecker::with_config(
                &TestSpec,
                ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    ..ModelCheckConfig::bounded_lasso(3)
                },
            )
            .check_all()
            .expect("symbolic bounded lasso should run");
            assert_eq!(symbolic_result, explicit_result);
        }
    }
}
