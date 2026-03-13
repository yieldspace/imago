use nirvash::{
    BoundedDomain, CounterexampleKind, ExprDomain, GuardExpr, ModelBackend, ModelCase,
    ModelCheckConfig, ModelCheckError, Signature, SymbolicSort, SymbolicSortSpec,
    SymbolicStateSpec, TemporalSpec, TransitionProgram, TransitionRule, TransitionSystem,
    UpdateProgram,
};
use nirvash_check::ModelChecker;
use nirvash_macros::{
    Signature as FormalSignature, nirvash_expr, nirvash_step_expr, nirvash_transition_program,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Busy,
}

impl Signature for State {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Idle, Self::Busy])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
}

impl Signature for Action {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![Self::Start, Self::Stop])
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Spec;

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Stop]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::Start) => Some(State::Busy),
            (State::Busy, Action::Stop) => Some(State::Idle),
            _ => None,
        }
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for Spec {}

#[derive(Debug, Default, Clone, Copy)]
struct DeadlockSpec;

impl TransitionSystem for DeadlockSpec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start]
    }

    fn transition(&self, _state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
        None
    }
}

impl TemporalSpec for DeadlockSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for DeadlockSpec {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum Phase {
    Idle,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum ToggleAction {
    Flip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
enum ReadyFlag {
    No,
    Yes,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
struct DerivedSchemaState {
    phase: Phase,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
struct ManualSchemaState {
    phase: Phase,
}

nirvash::signature_spec!(
    ManualSchemaStateSignatureSpec for ManualSchemaState,
    representatives = vec![
        ManualSchemaState { phase: Phase::Idle },
        ManualSchemaState { phase: Phase::Busy },
    ]
);

nirvash::symbolic_state_spec!(for ManualSchemaState {
    phase: Phase,
});

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
struct PartialSchemaState {
    phase: Phase,
    ready: ReadyFlag,
}

nirvash::signature_spec!(
    PartialSchemaStateSignatureSpec for PartialSchemaState,
    representatives = vec![
        PartialSchemaState {
            phase: Phase::Idle,
            ready: ReadyFlag::No,
        },
        PartialSchemaState {
            phase: Phase::Idle,
            ready: ReadyFlag::Yes,
        },
        PartialSchemaState {
            phase: Phase::Busy,
            ready: ReadyFlag::No,
        },
        PartialSchemaState {
            phase: Phase::Busy,
            ready: ReadyFlag::Yes,
        },
    ]
);

impl SymbolicStateSpec for PartialSchemaState {
    fn symbolic_state_schema() -> nirvash::SymbolicStateSchema<Self> {
        nirvash::SymbolicStateSchema::new(
            vec![nirvash::symbolic_leaf_field(
                "phase",
                |state: &Self| &state.phase,
                |state: &mut Self, value: Phase| {
                    state.phase = value;
                },
            )],
            || PartialSchemaState {
                phase: nirvash::symbolic_seed_value::<Phase>(),
                ready: ReadyFlag::No,
            },
        )
    }
}

fn partial_schema_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<PartialSchemaState>()
}

fn build_partial_schema_state_schema() -> Box<dyn std::any::Any> {
    Box::new(<PartialSchemaState as SymbolicStateSpec>::symbolic_state_schema())
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: partial_schema_state_type_id,
        build: build_partial_schema_state_schema,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MissingSchemaState {
    phase: Phase,
}

impl Signature for MissingSchemaState {
    fn bounded_domain() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            Self { phase: Phase::Idle },
            Self { phase: Phase::Busy },
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanicDomainState {
    phase: Phase,
}

impl Signature for PanicDomainState {
    fn bounded_domain() -> BoundedDomain<Self> {
        panic!("symbolic backend should not call PanicDomainState::bounded_domain()");
    }
}

nirvash::symbolic_state_spec!(for PanicDomainState {
    phase: Phase,
});

fn toggled_phase(phase: Phase) -> Phase {
    match phase {
        Phase::Idle => Phase::Busy,
        Phase::Busy => Phase::Idle,
    }
}

fn toggled_manual_state(prev: &ManualSchemaState) -> ManualSchemaState {
    ManualSchemaState {
        phase: toggled_phase(prev.phase),
    }
}

nirvash::register_symbolic_pure_helpers!("toggled_manual_state");

fn flip_phase_effect(
    _prev: &DerivedSchemaState,
    state: &mut DerivedSchemaState,
    _action: &ToggleAction,
) {
    state.phase = toggled_phase(state.phase);
}

nirvash::register_symbolic_effects!("flip_phase_effect");

#[derive(Debug, Default, Clone, Copy)]
struct ManualSchemaSpec;

impl TransitionSystem for ManualSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ManualSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule toggle when matches!(action, ToggleAction::Flip) => {
                set self <= toggled_manual_state(prev);
            }
        })
    }
}

impl TemporalSpec for ManualSchemaSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for ManualSchemaSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct ChoiceSchemaSpec;

fn choice_transition_program() -> TransitionProgram<ManualSchemaState, ToggleAction> {
    TransitionProgram::named(
        "choice_schema",
        vec![TransitionRule::ast(
            "choose_phase",
            GuardExpr::matches_variant(
                "flip",
                "action",
                "ToggleAction::Flip",
                |_prev: &ManualSchemaState, action: &ToggleAction| {
                    matches!(action, ToggleAction::Flip)
                },
            ),
            UpdateProgram::choose_in(
                "choose_phase",
                ExprDomain::new("phase_domain", [Phase::Idle, Phase::Busy]),
                "phase <- choice",
                &[],
                &["phase"],
                |_prev: &ManualSchemaState, _action: &ToggleAction, phase: &Phase| {
                    ManualSchemaState { phase: *phase }
                },
            ),
        )],
    )
}

fn choice_reaches_busy() -> nirvash::BoolExpr<ManualSchemaState> {
    nirvash_expr!(choice_reaches_busy(state) => matches!(state.phase, Phase::Busy))
}

fn choice_busy_step() -> nirvash::StepExpr<ManualSchemaState, ToggleAction> {
    nirvash_step_expr!(choice_busy_step(prev, action, next) =>
        matches!(action, ToggleAction::Flip) && matches!(next.phase, Phase::Busy) && (matches!(prev.phase, Phase::Idle) || matches!(prev.phase, Phase::Busy))
    )
}

impl TransitionSystem for ChoiceSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ManualSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(choice_transition_program())
    }
}

impl TemporalSpec for ChoiceSchemaSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for ChoiceSchemaSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct ChoicePropertySpec;

impl TransitionSystem for ChoicePropertySpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ManualSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(choice_transition_program())
    }
}

impl TemporalSpec for ChoicePropertySpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }

    fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
        vec![nirvash::Ltl::eventually(nirvash::Ltl::pred(
            choice_reaches_busy(),
        ))]
    }
}

impl nirvash::ModelCaseSource for ChoicePropertySpec {}

#[derive(Debug, Default, Clone, Copy)]
struct ChoiceFairnessSpec;

impl TransitionSystem for ChoiceFairnessSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ManualSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(choice_transition_program())
    }
}

impl TemporalSpec for ChoiceFairnessSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }

    fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
        vec![
            nirvash::Ltl::always(nirvash::Ltl::enabled(choice_busy_step())),
            nirvash::Ltl::eventually(nirvash::Ltl::pred(choice_reaches_busy())),
        ]
    }

    fn fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        vec![nirvash::Fairness::weak(choice_busy_step())]
    }
}

impl nirvash::ModelCaseSource for ChoiceFairnessSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct NoProgramSchemaSpec;

impl TransitionSystem for NoProgramSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ManualSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }
}

impl TemporalSpec for NoProgramSchemaSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for NoProgramSchemaSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct MissingReadPathSpec;

impl TransitionSystem for MissingReadPathSpec {
    type State = PartialSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![PartialSchemaState {
            phase: Phase::Idle,
            ready: ReadyFlag::No,
        }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for MissingReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for MissingReadPathSpec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![ModelCase::default().with_state_constraint(nirvash::pred!(
            ready_is_visible(_state) => _state.ready == ReadyFlag::Yes
        ))]
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingSchemaSpec;

impl TransitionSystem for MissingSchemaSpec {
    type State = MissingSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![MissingSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for MissingSchemaSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for MissingSchemaSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct PanicDomainSpec;

impl TransitionSystem for PanicDomainSpec {
    type State = PanicDomainState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![PanicDomainState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for PanicDomainSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        vec![nirvash::BoolExpr::literal("phase_is_known", true)]
    }

    fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
        vec![nirvash::Ltl::truth()]
    }

    fn fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for PanicDomainSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct RegisteredEffectSpec;

impl TransitionSystem for RegisteredEffectSpec {
    type State = DerivedSchemaState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![DerivedSchemaState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash::TransitionProgram::named(
            "registered_effect",
            vec![nirvash::TransitionRule::ast(
                "flip",
                nirvash::GuardExpr::matches_variant(
                    "flip",
                    "action",
                    "ToggleAction::Flip",
                    |_prev: &DerivedSchemaState, action: &ToggleAction| {
                        matches!(action, ToggleAction::Flip)
                    },
                ),
                nirvash::UpdateProgram::ast(
                    "flip",
                    vec![nirvash::UpdateOp::registered_effect(
                        "flip_phase_effect()",
                        "flip_phase_effect",
                        flip_phase_effect,
                    )],
                ),
            )],
        ))
    }
}

impl TemporalSpec for RegisteredEffectSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for RegisteredEffectSpec {}

#[derive(Debug, Clone, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
struct MissingReadPathState {
    phase: Phase,
    ready: bool,
}

nirvash::signature_spec!(
    MissingReadPathStateSignatureSpec for MissingReadPathState,
    representatives = vec![
        MissingReadPathState {
            phase: Phase::Idle,
            ready: false,
        },
        MissingReadPathState {
            phase: Phase::Idle,
            ready: true,
        },
        MissingReadPathState {
            phase: Phase::Busy,
            ready: false,
        },
        MissingReadPathState {
            phase: Phase::Busy,
            ready: true,
        },
    ]
);

impl SymbolicStateSpec for MissingReadPathState {
    fn symbolic_state_schema() -> nirvash::SymbolicStateSchema<Self> {
        nirvash::SymbolicStateSchema::new(
            vec![nirvash::symbolic_leaf_field(
                "phase",
                |state: &Self| &state.phase,
                |state: &mut Self, value: Phase| {
                    state.phase = value;
                },
            )],
            || MissingReadPathState {
                phase: nirvash::symbolic_seed_value::<Phase>(),
                ready: false,
            },
        )
    }
}

fn missing_read_path_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<MissingReadPathState>()
}

fn build_missing_read_path_state_schema() -> Box<dyn std::any::Any> {
    Box::new(<MissingReadPathState as SymbolicStateSpec>::symbolic_state_schema())
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: missing_read_path_state_type_id,
        build: build_missing_read_path_state_schema,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingProgramReadPathSpec;

impl TransitionSystem for MissingProgramReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![MissingReadPathState {
            phase: Phase::Idle,
            ready: true,
        }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_when_ready when matches!(action, ToggleAction::Flip) && prev.ready == true => {
                set phase <= Phase::Busy;
            }
        })
    }
}

impl TemporalSpec for MissingProgramReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl nirvash::ModelCaseSource for MissingProgramReadPathSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct MissingInvariantReadPathSpec;

impl TransitionSystem for MissingInvariantReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![MissingReadPathState {
            phase: Phase::Idle,
            ready: true,
        }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for MissingInvariantReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        vec![nirvash::pred!(ready_is_visible(state) => state.ready)]
    }
}

impl nirvash::ModelCaseSource for MissingInvariantReadPathSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct MissingPropertyReadPathSpec;

impl TransitionSystem for MissingPropertyReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![MissingReadPathState {
            phase: Phase::Idle,
            ready: true,
        }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for MissingPropertyReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }

    fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
        vec![nirvash::Ltl::always(nirvash::Ltl::pred(
            nirvash::pred!(ready_is_visible(state) => state.ready),
        ))]
    }
}

impl nirvash::ModelCaseSource for MissingPropertyReadPathSpec {}

#[derive(Debug, Default, Clone, Copy)]
struct MissingFairnessReadPathSpec;

impl TransitionSystem for MissingFairnessReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![MissingReadPathState {
            phase: Phase::Idle,
            ready: true,
        }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule flip_idle when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Idle) => {
                set phase <= Phase::Busy;
            }

            rule flip_busy when matches!(action, ToggleAction::Flip) && matches!(prev.phase, Phase::Busy) => {
                set phase <= Phase::Idle;
            }
        })
    }
}

impl TemporalSpec for MissingFairnessReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }

    fn properties(&self) -> Vec<nirvash::Ltl<Self::State, Self::Action>> {
        vec![nirvash::Ltl::truth()]
    }

    fn fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        vec![nirvash::Fairness::weak(nirvash::step!(
            ready_progress(prev, action, _next) =>
                matches!(action, ToggleAction::Flip) && prev.ready == true
        ))]
    }
}

impl nirvash::ModelCaseSource for MissingFairnessReadPathSpec {}

#[test]
fn explicit_snapshot_exposes_states_and_edges() {
    let snapshot = ModelChecker::new(&Spec)
        .reachable_graph_snapshot()
        .expect("reachable graph should build");

    assert_eq!(snapshot.states, vec![State::Idle, State::Busy]);
    assert_eq!(snapshot.initial_indices, vec![0]);
    assert_eq!(snapshot.edges.len(), 2);
    assert_eq!(snapshot.edges[0].len(), 1);
    assert!(snapshot.deadlocks.is_empty());
}

#[test]
fn deadlocks_are_reported_by_frontdoor_checker() {
    let result = ModelChecker::new(&DeadlockSpec)
        .check_deadlocks()
        .expect("deadlock check should run");

    assert!(!result.is_ok());
    assert_eq!(result.violations()[0].kind, CounterexampleKind::Deadlock);
}

#[test]
fn symbolic_backend_rejects_specs_without_transition_program() {
    let err = ModelChecker::with_config(
        &NoProgramSchemaSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .check_all()
    .unwrap_err();

    assert!(matches!(err, ModelCheckError::UnsupportedConfiguration(_)));
}

#[test]
fn symbolic_state_spec_derive_and_manual_macro_rebuild_same_indices() {
    let derived = <DerivedSchemaState as SymbolicStateSpec>::symbolic_state_schema();
    let manual = <ManualSchemaState as SymbolicStateSpec>::symbolic_state_schema();
    let derived_sort = <DerivedSchemaState as SymbolicSortSpec>::symbolic_sort();
    let manual_sort = <ManualSchemaState as SymbolicSortSpec>::symbolic_sort();

    assert_eq!(
        derived
            .fields()
            .iter()
            .map(|field| field.path())
            .collect::<Vec<_>>(),
        vec!["phase"]
    );
    assert_eq!(
        manual
            .fields()
            .iter()
            .map(|field| field.path())
            .collect::<Vec<_>>(),
        vec!["phase"]
    );

    let derived_busy = DerivedSchemaState { phase: Phase::Busy };
    let manual_busy = ManualSchemaState { phase: Phase::Busy };
    assert_eq!(derived.read_indices(&derived_busy), vec![1]);
    assert_eq!(manual.read_indices(&manual_busy), vec![1]);
    assert_eq!(derived.rebuild_from_indices(&[1]), derived_busy);
    assert_eq!(manual.rebuild_from_indices(&[1]), manual_busy);
    assert!(matches!(
        derived.fields()[0].sort(),
        SymbolicSort::Finite { type_name, domain_size }
            if *type_name == std::any::type_name::<Phase>() && *domain_size == 2
    ));
    assert!(matches!(
        derived_sort,
        SymbolicSort::Composite { ref fields, .. }
            if fields.len() == 1
                && fields[0].name() == "phase"
                && matches!(
                    fields[0].sort(),
                    SymbolicSort::Finite { type_name, domain_size }
                        if *type_name == std::any::type_name::<Phase>() && *domain_size == 2
                )
    ));
    assert!(matches!(
        manual_sort,
        SymbolicSort::Composite { ref fields, .. }
            if fields.len() == 1
                && fields[0].name() == "phase"
                && matches!(
                    fields[0].sort(),
                    SymbolicSort::Finite { type_name, domain_size }
                        if *type_name == std::any::type_name::<Phase>() && *domain_size == 2
                )
    ));
}

#[test]
fn step_pure_call_symbolic_state_paths_include_receiver_paths() {
    let predicate = nirvash::StepExpr::builtin_pure_call_with_paths(
        "prev.ready.clone",
        &["prev.ready"],
        |prev: &MissingReadPathState, _action: &ToggleAction, _next: &MissingReadPathState| {
            prev.ready
        },
    );

    assert_eq!(predicate.symbolic_state_paths(), vec!["ready"]);
}

#[test]
fn symbolic_backend_matches_explicit_snapshot_for_manual_whole_state_updates() {
    let explicit = ModelChecker::new(&ManualSchemaSpec)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");
    let symbolic = ModelChecker::with_config(
        &ManualSchemaSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .expect("symbolic snapshot should build");

    assert_eq!(symbolic, explicit);
}

#[test]
fn symbolic_backend_matches_explicit_snapshot_for_choice_updates() {
    let explicit = ModelChecker::new(&ChoiceSchemaSpec)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");
    let symbolic = ModelChecker::with_config(
        &ChoiceSchemaSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::default()
        },
    )
    .full_reachable_graph_snapshot()
    .expect("symbolic snapshot should build");

    assert_eq!(symbolic, explicit);
}

#[test]
fn symbolic_bounded_lasso_matches_explicit_for_choice_property_violation() {
    let explicit =
        ModelChecker::with_config(&ChoicePropertySpec, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit bounded lasso should run");
    let symbolic = ModelChecker::with_config(
        &ChoicePropertySpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::bounded_lasso(3)
        },
    )
    .check_properties()
    .expect("symbolic bounded lasso should run");

    assert_eq!(symbolic, explicit);
    assert!(!symbolic.is_ok());
    assert_eq!(symbolic.violations()[0].kind, CounterexampleKind::Property);
}

#[test]
fn symbolic_bounded_lasso_matches_explicit_for_choice_fairness_and_enabled() {
    let explicit =
        ModelChecker::with_config(&ChoiceFairnessSpec, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit bounded lasso should run");
    let symbolic = ModelChecker::with_config(
        &ChoiceFairnessSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::bounded_lasso(3)
        },
    )
    .check_properties()
    .expect("symbolic bounded lasso should run");

    assert_eq!(symbolic, explicit);
    assert!(symbolic.is_ok());
}

#[test]
fn symbolic_backend_rejects_states_without_symbolic_state_spec() {
    let err = ModelChecker::with_config(
        &MissingSchemaSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("SymbolicStateSpec")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_read_paths_in_state_constraints() {
    let err = ModelChecker::with_config(
        &MissingReadPathSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("state constraint")
                && message.contains("ready")
    ));
}

#[test]
fn symbolic_reachable_graph_does_not_call_state_bounded_domain() {
    let snapshot = ModelChecker::with_config(
        &PanicDomainSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .expect("symbolic reachable graph should build without enumerating whole states");

    assert_eq!(
        snapshot.states,
        vec![
            PanicDomainState { phase: Phase::Idle },
            PanicDomainState { phase: Phase::Busy },
        ]
    );
    assert!(
        ModelChecker::with_config(
            &PanicDomainSpec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_invariants()
        .expect("symbolic invariant check should run")
        .is_ok()
    );
    assert!(
        ModelChecker::with_config(
            &PanicDomainSpec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .check_properties()
        .expect("symbolic reachable-graph property check should run")
        .is_ok()
    );
}

#[test]
fn symbolic_bounded_lasso_does_not_call_state_bounded_domain() {
    assert!(
        ModelChecker::with_config(
            &PanicDomainSpec,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::bounded_lasso(3)
            },
        )
        .check_all()
        .expect("symbolic bounded lasso should run without enumerating whole states")
        .is_ok()
    );
}

#[test]
fn symbolic_reachable_graph_rejects_registered_effect_updates() {
    let err = ModelChecker::with_config(
        &RegisteredEffectSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("update effect") && message.contains("flip_phase_effect()")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_program_read_path_in_state_schema() {
    let err = ModelChecker::with_config(
        &MissingProgramReadPathSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("ready")
                && message.contains("transition program")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_invariant_read_path_in_state_schema() {
    let err = ModelChecker::with_config(
        &MissingInvariantReadPathSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .check_invariants()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("ready")
                && message.contains("invariant")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_property_read_path_in_state_schema() {
    let err = ModelChecker::with_config(
        &MissingPropertyReadPathSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .check_properties()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("ready")
                && message.contains("property")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_fairness_read_path_in_state_schema() {
    let err = ModelChecker::with_config(
        &MissingFairnessReadPathSpec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .check_properties()
    .unwrap_err();

    assert!(matches!(
        err,
        ModelCheckError::UnsupportedConfiguration(message)
            if message.contains("ready")
                && message.contains("fairness")
    ));
}
