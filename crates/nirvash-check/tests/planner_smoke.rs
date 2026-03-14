use nirvash::{BoolExpr, TraceStep, TransitionProgram};
use nirvash_check::{
    ExplicitObligationPlanner, ObligationPlanner, PlannedObligationKind, PlannerSeedProfile,
    PlanningCoverageGoal, PropertyPrefixPlanner, TraceConstraintPlanner,
};
use nirvash_lower::{FrontendSpec, LoweringCx, ModelInstance, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
    nirvash_transition_program,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum Action {
    Start,
    Stop,
}

#[derive(Default)]
struct PlannerSpec;

impl FrontendSpec for PlannerSpec {
    type State = State;
    type Action = Action;

    fn frontend_name(&self) -> &'static str {
        "PlannerSpec"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Stop]
    }

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::new("planner-small")]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && matches!(prev, State::Idle) => {
                set self <= State::Busy;
            }

            rule stop when matches!(action, Action::Stop) && matches!(prev, State::Busy) => {
                set self <= State::Idle;
            }
        })
    }
}

impl TemporalSpec for PlannerSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

fn lowered_spec() -> nirvash_lower::LoweredSpec<'static, State, Action> {
    let spec = Box::leak(Box::new(PlannerSpec));
    let mut cx = LoweringCx;
    spec.lower(&mut cx).expect("planner spec lowers")
}

#[test]
fn explicit_obligation_planner_returns_transition_cover() {
    let lowered = lowered_spec();
    let model = lowered.model_instances().remove(0);
    let obligations = ExplicitObligationPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::Transitions],
            &PlannerSeedProfile::default(),
        )
        .expect("explicit plan succeeds");

    assert_eq!(obligations.len(), 2);
    assert!(matches!(
        &obligations[0].kind,
        PlannedObligationKind::ExplicitTraceCover { .. }
    ));
}

#[test]
fn property_prefix_planner_groups_common_prefixes() {
    let lowered = lowered_spec();
    let model = lowered.model_instances().remove(0);
    let obligations = PropertyPrefixPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::PropertyPrefixes],
            &PlannerSeedProfile::default(),
        )
        .expect("prefix plan succeeds");

    assert!(!obligations.is_empty());
    match &obligations[0].kind {
        PlannedObligationKind::PropertyPrefix { prefix, traces } => {
            assert!(!prefix.is_empty());
            assert!(matches!(prefix[0], TraceStep::Action(Action::Start)));
            assert!(!traces.is_empty());
        }
        other => panic!("unexpected obligation: {other:?}"),
    }
}

#[test]
fn trace_constraint_planner_builds_symbolic_constraints() {
    let lowered = lowered_spec();
    let model = lowered.model_instances().remove(0);
    let obligations = TraceConstraintPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::TraceConstraints],
            &PlannerSeedProfile::default(),
        )
        .expect("symbolic trace plan succeeds");

    assert!(!obligations.is_empty());
    match &obligations[0].kind {
        PlannedObligationKind::SymbolicTraceConstraint { constraint } => {
            assert!(!constraint.states.is_empty());
            assert!(!constraint.steps.is_empty());
        }
        other => panic!("unexpected obligation: {other:?}"),
    }
}

#[test]
fn obligations_filter_by_coverage_model_and_seed_labels() {
    let lowered = lowered_spec();
    let model = lowered.model_instances().remove(0);
    let explicit = ExplicitObligationPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::PropertyPrefixes],
            &PlannerSeedProfile::default(),
        )
        .expect("explicit plan succeeds");
    let prefixes = PropertyPrefixPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::PropertyPrefixes],
            &PlannerSeedProfile::default(),
        )
        .expect("prefix plan succeeds");

    assert!(explicit.is_empty());
    assert!(!prefixes.is_empty());

    let mismatched_model = ModelInstance::new("missing-model");
    let none = TraceConstraintPlanner::new()
        .obligations(
            &lowered,
            &mismatched_model,
            &[PlanningCoverageGoal::TraceConstraints],
            &PlannerSeedProfile::default(),
        )
        .expect("mismatched model should short-circuit");
    assert!(none.is_empty());

    let seeded_none = TraceConstraintPlanner::new()
        .obligations(
            &lowered,
            &model,
            &[PlanningCoverageGoal::TraceConstraints],
            &PlannerSeedProfile {
                labels: vec!["explicit_suite".to_owned()],
                ..PlannerSeedProfile::default()
            },
        )
        .expect("seed filter should short-circuit");
    assert!(seeded_none.is_empty());
}
