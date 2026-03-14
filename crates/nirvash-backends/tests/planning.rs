use nirvash::{
    BoolExpr, ReachableGraphEdge, ReachableGraphSnapshot, Trace, TraceStep, TransitionProgram,
    TrustTier,
};
use nirvash_backends::{
    CoveredTransition, ExplicitSuiteCover, SharedPrefixGroup, SymbolicTraceConstraint,
    build_explicit_suite_cover, build_symbolic_trace_constraint, share_trace_prefixes,
    symbolic::trace_constraints,
};
use nirvash_lower::{FrontendSpec, LoweringCx, ModelInstance, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
    nirvash_transition_program,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoState {
    Idle,
    Busy,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoAction {
    Start,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum PlannerState {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum PlannerAction {
    Start,
    Stop,
}

#[derive(Default)]
struct PlannerSpec;

impl FrontendSpec for PlannerSpec {
    type State = PlannerState;
    type Action = PlannerAction;

    fn frontend_name(&self) -> &'static str {
        "PlanningTraceConstraintSpec"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![PlannerState::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![PlannerAction::Start, PlannerAction::Stop]
    }

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![ModelInstance::new("planner-small")]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, PlannerAction::Start) && matches!(prev, PlannerState::Idle) => {
                set self <= PlannerState::Busy;
            }

            rule stop when matches!(action, PlannerAction::Stop) && matches!(prev, PlannerState::Busy) => {
                set self <= PlannerState::Idle;
            }
        })
    }
}

impl TemporalSpec for PlannerSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

fn lowered_planner_spec() -> nirvash_lower::LoweredSpec<'static, PlannerState, PlannerAction> {
    let spec = Box::leak(Box::new(PlannerSpec));
    let mut cx = LoweringCx;
    spec.lower(&mut cx).expect("planner spec lowers")
}

fn start_trace() -> Trace<DemoState, DemoAction> {
    Trace::new(
        vec![DemoState::Idle, DemoState::Busy],
        vec![TraceStep::Action(DemoAction::Start), TraceStep::Stutter],
        1,
    )
}

fn full_trace() -> Trace<DemoState, DemoAction> {
    Trace::new(
        vec![DemoState::Idle, DemoState::Busy, DemoState::Done],
        vec![
            TraceStep::Action(DemoAction::Start),
            TraceStep::Action(DemoAction::Finish),
            TraceStep::Stutter,
        ],
        2,
    )
}

#[test]
fn explicit_suite_cover_picks_shortest_trace_per_new_edge() {
    let cover = build_explicit_suite_cover(&[full_trace(), start_trace()]);

    assert_eq!(
        cover,
        ExplicitSuiteCover {
            cases: vec![
                nirvash_backends::ExplicitSuiteCase {
                    edge: CoveredTransition {
                        prev: DemoState::Idle,
                        action: DemoAction::Start,
                        next: DemoState::Busy,
                    },
                    trace: start_trace(),
                },
                nirvash_backends::ExplicitSuiteCase {
                    edge: CoveredTransition {
                        prev: DemoState::Busy,
                        action: DemoAction::Finish,
                        next: DemoState::Done,
                    },
                    trace: full_trace(),
                },
            ],
        }
    );
}

#[test]
fn share_trace_prefixes_groups_by_common_leading_steps() {
    let grouped = share_trace_prefixes(&[start_trace(), full_trace()]);

    assert_eq!(
        grouped,
        vec![SharedPrefixGroup {
            prefix: vec![TraceStep::Action(DemoAction::Start)],
            traces: vec![start_trace(), full_trace()],
        }]
    );
}

#[test]
fn symbolic_trace_constraint_clones_trace_shape() {
    let trace = full_trace();

    assert_eq!(
        build_symbolic_trace_constraint(&trace),
        SymbolicTraceConstraint {
            states: vec![DemoState::Idle, DemoState::Busy, DemoState::Done],
            steps: vec![
                TraceStep::Action(DemoAction::Start),
                TraceStep::Action(DemoAction::Finish),
                TraceStep::Stutter,
            ],
            loop_start: 2,
        }
    );
}

#[test]
fn symbolic_trace_constraints_match_candidates_by_action_prefix() {
    let snapshot = ReachableGraphSnapshot {
        states: vec![DemoState::Idle, DemoState::Busy, DemoState::Done],
        edges: vec![
            vec![ReachableGraphEdge {
                action: DemoAction::Start,
                target: 1,
            }],
            vec![ReachableGraphEdge {
                action: DemoAction::Finish,
                target: 2,
            }],
            Vec::new(),
        ],
        initial_indices: vec![0],
        deadlocks: vec![2],
        truncated: false,
        stutter_omitted: false,
        trust_tier: TrustTier::Exact,
    };

    let matched = trace_constraints::matching_candidates(
        &snapshot,
        Some(&DemoState::Idle),
        &[
            TraceStep::Action(DemoAction::Start),
            TraceStep::Action(DemoAction::Finish),
        ],
    );

    assert_eq!(
        matched,
        vec![Trace::new(
            vec![DemoState::Idle, DemoState::Busy, DemoState::Done],
            vec![
                TraceStep::Action(DemoAction::Start),
                TraceStep::Action(DemoAction::Finish),
                TraceStep::Stutter,
            ],
            2,
        )]
    );
}

#[test]
fn symbolic_trace_constraints_solve_observed_steps_against_symbolic_backend() {
    let lowered = lowered_planner_spec();
    let model_case = lowered
        .model_instances()
        .into_iter()
        .next()
        .expect("planner model case");

    let matched = trace_constraints::matching_candidates_for_case(
        &lowered,
        model_case,
        &[Some(&PlannerState::Idle), Some(&PlannerState::Busy)],
        &[TraceStep::Action(PlannerAction::Start)],
    )
    .expect("symbolic trace constraint should solve");

    assert!(
        matched.contains(&Trace::new(
            vec![PlannerState::Idle, PlannerState::Busy],
            vec![TraceStep::Action(PlannerAction::Start), TraceStep::Stutter],
            1,
        )),
        "unexpected matches: {matched:?}"
    );
    assert!(
        matched
            .iter()
            .all(|trace| trace.states() == [PlannerState::Idle, PlannerState::Busy])
    );
    assert!(
        matched
            .iter()
            .all(|trace| matches!(trace.steps()[0], TraceStep::Action(PlannerAction::Start)))
    );
}

#[test]
fn symbolic_trace_constraints_fail_close_on_shape_mismatch() {
    let lowered = lowered_planner_spec();
    let model_case = lowered
        .model_instances()
        .into_iter()
        .next()
        .expect("planner model case");

    let error = trace_constraints::matching_candidates_for_case(
        &lowered,
        model_case,
        &[Some(&PlannerState::Idle)],
        &[TraceStep::Action(PlannerAction::Start)],
    )
    .expect_err("shape mismatch should fail close");

    assert!(
        format!("{error:?}").contains("one more state hint than observed steps"),
        "unexpected error: {error:?}"
    );
}
