use nirvash::{
    BoundedDomain, CounterexampleKind, ModelBackend, ModelCheckConfig, ModelCheckError, Signature,
    SymbolicStateSpec, TemporalSpec, TransitionSystem,
};
use nirvash_check::ModelChecker;
use nirvash_macros::{Signature as FormalSignature, nirvash_transition_program};

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
        panic!("symbolic reachable-graph should not call PanicDomainState::bounded_domain()");
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
        vec![nirvash::pred!(phase_is_known(_state) => true)]
    }
}

impl nirvash::ModelCaseSource for PanicDomainSpec {}

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
        &Spec,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .check_all()
    .unwrap_err();

    assert!(
        matches!(err, ModelCheckError::UnsupportedConfiguration(message) if message.contains("transition_program"))
    );
}

#[test]
fn symbolic_state_spec_derive_and_manual_macro_rebuild_same_indices() {
    let derived = <DerivedSchemaState as SymbolicStateSpec>::symbolic_state_schema();
    let manual = <ManualSchemaState as SymbolicStateSpec>::symbolic_state_schema();

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
}
