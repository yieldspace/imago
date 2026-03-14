use nirvash::{
    CounterexampleKind, ExprDomain, GuardExpr, ModelBackend, ModelCheckConfig, ModelCheckError,
    SymbolicSort, TransitionProgram, TransitionRule, UpdateProgram,
};
use nirvash_check as checks;
use nirvash_lower::{
    FairnessDecl, FiniteModelDomain, FrontendSpec, LoweringCx, ModelInstance, SymbolicEncoding,
    SymbolicStateSchema, TemporalSpec, lower_core_fairness,
};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program,
};

macro_rules! frontend_name {
    () => {
        fn frontend_name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }
    };
}

fn lower_spec<T>(spec: &T) -> nirvash_lower::LoweredSpec<'_, T::State, T::Action>
where
    T: TemporalSpec,
    T::State: PartialEq + FiniteModelDomain,
    T::Action: PartialEq,
{
    let mut lowering_cx = LoweringCx;
    spec.lower(&mut lowering_cx).expect("spec should lower")
}

fn lower_symbolic_spec<T>(spec: &T) -> nirvash_lower::LoweredSpec<'_, T::State, T::Action>
where
    T: TemporalSpec,
    T::State: PartialEq,
    T::Action: PartialEq,
{
    let mut lowering_cx = LoweringCx;
    spec.lower(&mut lowering_cx)
        .expect("symbolic-only spec should lower")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum State {
    Idle,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum Action {
    Start,
    Stop,
}

#[derive(Debug, Default, Clone, Copy)]
struct Spec;

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct DeadlockSpec;

impl FrontendSpec for DeadlockSpec {
    type State = State;
    type Action = Action;

    frontend_name!();

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum Phase {
    Idle,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum ToggleAction {
    Flip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum ReadyFlag {
    No,
    Yes,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
struct DerivedSchemaState {
    phase: Phase,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain)]
struct ManualSchemaState {
    phase: Phase,
}

impl SymbolicEncoding for ManualSchemaState {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::composite::<Self>(vec![nirvash::SymbolicSortField::new(
            "phase",
            <Phase as SymbolicEncoding>::symbolic_sort(),
        )])
    }

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>> {
        Some(SymbolicStateSchema::new(
            vec![nirvash::symbolic_leaf_field(
                "phase",
                |state: &Self| &state.phase,
                |state: &mut Self, value: Phase| {
                    state.phase = value;
                },
            )],
            || ManualSchemaState {
                phase: nirvash::symbolic_seed_value::<Phase>(),
            },
        ))
    }
}

fn manual_schema_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<ManualSchemaState>()
}

fn build_manual_schema_state_schema() -> Box<dyn std::any::Any> {
    Box::new(
        <ManualSchemaState as SymbolicEncoding>::symbolic_state_schema()
            .expect("manual schema should be registered"),
    )
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: manual_schema_state_type_id,
        build: build_manual_schema_state_schema,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain)]
struct PartialSchemaState {
    phase: Phase,
    ready: ReadyFlag,
}

impl SymbolicEncoding for PartialSchemaState {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::composite::<Self>(vec![nirvash::SymbolicSortField::new(
            "phase",
            <Phase as SymbolicEncoding>::symbolic_sort(),
        )])
    }

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>> {
        Some(SymbolicStateSchema::new(
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
        ))
    }
}

fn partial_schema_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<PartialSchemaState>()
}

fn build_partial_schema_state_schema() -> Box<dyn std::any::Any> {
    Box::new(
        <PartialSchemaState as SymbolicEncoding>::symbolic_state_schema()
            .expect("partial schema should be registered"),
    )
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: partial_schema_state_type_id,
        build: build_partial_schema_state_schema,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain)]
struct MissingSchemaState {
    phase: Phase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanicDomainState {
    phase: Phase,
}

fn panic_domain_state_representatives() -> nirvash::BoundedDomain<PanicDomainState> {
    panic!("symbolic backend should not call PanicDomainState::bounded_domain()");
}

impl FiniteModelDomain for PanicDomainState {
    fn finite_domain() -> nirvash::BoundedDomain<Self> {
        panic_domain_state_representatives()
    }
}

impl SymbolicEncoding for PanicDomainState {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::Composite {
            type_name: std::any::type_name::<Self>(),
            domain_size: 0,
            fields: vec![nirvash::SymbolicSortField::new(
                "phase",
                <Phase as SymbolicEncoding>::symbolic_sort(),
            )],
        }
    }

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>> {
        Some(SymbolicStateSchema::new(
            vec![nirvash::symbolic_leaf_field(
                "phase",
                |state: &Self| &state.phase,
                |state: &mut Self, value: Phase| {
                    state.phase = value;
                },
            )],
            || PanicDomainState {
                phase: nirvash::symbolic_seed_value::<Phase>(),
            },
        ))
    }
}

fn panic_domain_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<PanicDomainState>()
}

fn build_panic_domain_state_schema() -> Box<dyn std::any::Any> {
    Box::new(
        <PanicDomainState as SymbolicEncoding>::symbolic_state_schema()
            .expect("panic-domain schema should be registered"),
    )
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: panic_domain_state_type_id,
        build: build_panic_domain_state_schema,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolicOnlyState {
    phase: Phase,
}

impl SymbolicEncoding for SymbolicOnlyState {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::Composite {
            type_name: std::any::type_name::<Self>(),
            domain_size: 0,
            fields: vec![nirvash::SymbolicSortField::new(
                "phase",
                <Phase as SymbolicEncoding>::symbolic_sort(),
            )],
        }
    }

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>> {
        Some(SymbolicStateSchema::new(
            vec![nirvash::symbolic_leaf_field(
                "phase",
                |state: &Self| &state.phase,
                |state: &mut Self, value: Phase| {
                    state.phase = value;
                },
            )],
            || SymbolicOnlyState {
                phase: nirvash::symbolic_seed_value::<Phase>(),
            },
        ))
    }
}

fn symbolic_only_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<SymbolicOnlyState>()
}

fn build_symbolic_only_state_schema() -> Box<dyn std::any::Any> {
    Box::new(
        <SymbolicOnlyState as SymbolicEncoding>::symbolic_state_schema()
            .expect("symbolic-only schema should be registered"),
    )
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: symbolic_only_state_type_id,
        build: build_symbolic_only_state_schema,
    }
}

fn toggled_phase(phase: Phase) -> Phase {
    match phase {
        Phase::Idle => Phase::Busy,
        Phase::Busy => Phase::Idle,
    }
}

fn toggled_symbolic_only_state(prev: &SymbolicOnlyState) -> SymbolicOnlyState {
    SymbolicOnlyState {
        phase: toggled_phase(prev.phase),
    }
}

nirvash::register_symbolic_pure_helpers!("toggled_symbolic_only_state");

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

impl FrontendSpec for ManualSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

impl FrontendSpec for ChoiceSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct ChoicePropertySpec;

impl FrontendSpec for ChoicePropertySpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct ChoiceFairnessSpec;

impl FrontendSpec for ChoiceFairnessSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

    fn core_fairness(&self) -> Vec<FairnessDecl> {
        self.executable_fairness()
            .iter()
            .map(|fairness| {
                lower_core_fairness(self.frontend_name(), fairness).expect("fairness lowers")
            })
            .collect()
    }

    fn executable_fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        vec![nirvash::Fairness::weak(choice_busy_step())]
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct NoProgramSchemaSpec;

impl FrontendSpec for NoProgramSchemaSpec {
    type State = ManualSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct SymbolicOnlySpec;

impl FrontendSpec for SymbolicOnlySpec {
    type State = SymbolicOnlyState;
    type Action = ToggleAction;

    frontend_name!();

    fn initial_states(&self) -> Vec<Self::State> {
        vec![SymbolicOnlyState { phase: Phase::Idle }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ToggleAction::Flip]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule toggle when matches!(action, ToggleAction::Flip) => {
                set self <= toggled_symbolic_only_state(prev);
            }
        })
    }
}

impl TemporalSpec for SymbolicOnlySpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingReadPathSpec;

impl FrontendSpec for MissingReadPathSpec {
    type State = PartialSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

    fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
        vec![
            ModelInstance::default().with_state_constraint(nirvash::pred!(
                ready_is_visible(_state) => _state.ready == ReadyFlag::Yes
            )),
        ]
    }
}

impl TemporalSpec for MissingReadPathSpec {
    fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
        Vec::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingSchemaSpec;

impl FrontendSpec for MissingSchemaSpec {
    type State = MissingSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct PanicDomainSpec;

impl FrontendSpec for PanicDomainSpec {
    type State = PanicDomainState;
    type Action = ToggleAction;

    frontend_name!();

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

    fn core_fairness(&self) -> Vec<FairnessDecl> {
        Vec::new()
    }

    fn executable_fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        Vec::new()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RegisteredEffectSpec;

impl FrontendSpec for RegisteredEffectSpec {
    type State = DerivedSchemaState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain)]
struct MissingReadPathState {
    phase: Phase,
    ready: bool,
}

impl SymbolicEncoding for MissingReadPathState {
    fn symbolic_sort() -> SymbolicSort {
        SymbolicSort::composite::<Self>(vec![nirvash::SymbolicSortField::new(
            "phase",
            <Phase as SymbolicEncoding>::symbolic_sort(),
        )])
    }

    fn symbolic_state_schema() -> Option<SymbolicStateSchema<Self>> {
        Some(SymbolicStateSchema::new(
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
        ))
    }
}

fn missing_read_path_state_type_id() -> std::any::TypeId {
    std::any::TypeId::of::<MissingReadPathState>()
}

fn build_missing_read_path_state_schema() -> Box<dyn std::any::Any> {
    Box::new(
        <MissingReadPathState as SymbolicEncoding>::symbolic_state_schema()
            .expect("missing-read-path schema should be registered"),
    )
}

nirvash::inventory::submit! {
    nirvash::registry::RegisteredSymbolicStateSchema {
        state_type_id: missing_read_path_state_type_id,
        build: build_missing_read_path_state_schema,
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingProgramReadPathSpec;

impl FrontendSpec for MissingProgramReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    frontend_name!();

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

#[derive(Debug, Default, Clone, Copy)]
struct MissingInvariantReadPathSpec;

impl FrontendSpec for MissingInvariantReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    frontend_name!();

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
        vec![nirvash::BoolExpr::builtin_pure_call_with_paths(
            "ready_is_visible",
            &["ready"],
            |state: &MissingReadPathState| state.ready,
        )]
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingPropertyReadPathSpec;

impl FrontendSpec for MissingPropertyReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    frontend_name!();

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
            nirvash::BoolExpr::builtin_pure_call_with_paths(
                "ready_is_visible",
                &["ready"],
                |state: &MissingReadPathState| state.ready,
            ),
        ))]
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct MissingFairnessReadPathSpec;

impl FrontendSpec for MissingFairnessReadPathSpec {
    type State = MissingReadPathState;
    type Action = ToggleAction;

    frontend_name!();

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

    fn core_fairness(&self) -> Vec<FairnessDecl> {
        self.executable_fairness()
            .iter()
            .map(|fairness| {
                lower_core_fairness(self.frontend_name(), fairness).expect("fairness lowers")
            })
            .collect()
    }

    fn executable_fairness(&self) -> Vec<nirvash::Fairness<Self::State, Self::Action>> {
        vec![nirvash::Fairness::weak(
            nirvash::StepExpr::builtin_pure_call_with_paths(
                "ready_progress",
                &["prev.ready"],
                |prev: &MissingReadPathState,
                 action: &ToggleAction,
                 _next: &MissingReadPathState| {
                    matches!(action, ToggleAction::Flip) && prev.ready
                },
            ),
        )]
    }
}

#[test]
fn explicit_snapshot_exposes_states_and_edges() {
    let lowered = lower_spec(&Spec);
    let snapshot = checks::ExplicitModelChecker::new(&lowered)
        .reachable_graph_snapshot()
        .expect("reachable graph should build");

    assert_eq!(snapshot.states, vec![State::Idle, State::Busy]);
    assert_eq!(snapshot.initial_indices, vec![0]);
    assert_eq!(snapshot.edges.len(), 2);
    assert_eq!(snapshot.edges[0].len(), 1);
    assert!(snapshot.deadlocks.is_empty());
}

#[test]
fn deadlocks_are_reported_by_explicit_checker() {
    let lowered = lower_spec(&DeadlockSpec);
    let result = checks::ExplicitModelChecker::new(&lowered)
        .check_deadlocks()
        .expect("deadlock check should run");

    assert!(!result.is_ok());
    assert_eq!(result.violations()[0].kind, CounterexampleKind::Deadlock);
}

#[test]
fn explicit_checker_builds_expected_snapshot_and_properties() {
    let lowered = lower_spec(&ChoicePropertySpec);
    let expected_snapshot = checks::ExplicitModelChecker::new(&lowered)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");
    let actual_snapshot = checks::ExplicitModelChecker::new(&lowered)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");

    assert_eq!(actual_snapshot, expected_snapshot);

    let expected_properties =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit property check should run");
    let actual_properties =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit property check should run");

    assert_eq!(actual_properties, expected_properties);
}

#[test]
fn symbolic_backend_rejects_specs_without_transition_program() {
    let lowered = lower_spec(&NoProgramSchemaSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
fn symbolic_encoding_derive_and_manual_impl_rebuild_same_indices() {
    let derived = <DerivedSchemaState as SymbolicEncoding>::symbolic_state_schema()
        .expect("derived schema should exist");
    let manual = <ManualSchemaState as SymbolicEncoding>::symbolic_state_schema()
        .expect("manual schema should exist");
    let derived_sort = <DerivedSchemaState as SymbolicEncoding>::symbolic_sort();
    let manual_sort = <ManualSchemaState as SymbolicEncoding>::symbolic_sort();

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
    let lowered = lower_spec(&ManualSchemaSpec);
    let explicit = checks::ExplicitModelChecker::new(&lowered)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");
    let symbolic = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&ChoiceSchemaSpec);
    let explicit = checks::ExplicitModelChecker::new(&lowered)
        .full_reachable_graph_snapshot()
        .expect("explicit snapshot should build");
    let symbolic = checks::SymbolicModelChecker::with_config(
        &lowered,
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
fn symbolic_checker_matches_explicit_snapshot_and_properties() {
    let lowered = lower_spec(&ChoicePropertySpec);
    let config = ModelCheckConfig {
        backend: Some(ModelBackend::Symbolic),
        ..ModelCheckConfig::bounded_lasso(3)
    };

    let expected_snapshot =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("explicit snapshot should build");
    let actual_snapshot = checks::SymbolicModelChecker::with_config(
        &lowered,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .expect("symbolic snapshot should build");

    assert_eq!(actual_snapshot, expected_snapshot);

    let expected_properties =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit property check should run");
    let actual_properties = checks::SymbolicModelChecker::with_config(&lowered, config)
        .check_properties()
        .expect("symbolic property check should run");

    assert_eq!(actual_properties, expected_properties);
}

#[test]
fn symbolic_bounded_lasso_matches_explicit_for_choice_property_violation() {
    let lowered = lower_spec(&ChoicePropertySpec);
    let explicit =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit bounded lasso should run");
    let symbolic = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&ChoiceFairnessSpec);
    let explicit =
        checks::ExplicitModelChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(3))
            .check_properties()
            .expect("explicit bounded lasso should run");
    let symbolic = checks::SymbolicModelChecker::with_config(
        &lowered,
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
fn symbolic_backend_rejects_states_without_symbolic_encoding() {
    let lowered = lower_spec(&MissingSchemaSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
            if message.contains("SymbolicEncoding")
    ));
}

#[test]
fn symbolic_backend_rejects_missing_read_paths_in_state_constraints() {
    let lowered = lower_spec(&MissingReadPathSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&PanicDomainSpec);
    let snapshot = checks::SymbolicModelChecker::with_config(
        &lowered,
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
        checks::SymbolicModelChecker::with_config(
            &lowered,
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
        checks::SymbolicModelChecker::with_config(
            &lowered,
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
fn symbolic_typed_checker_works_without_state_finite_model_domain() {
    let lowered = lower_symbolic_spec(&SymbolicOnlySpec);
    let snapshot = checks::SymbolicModelChecker::with_config(
        &lowered,
        ModelCheckConfig {
            backend: Some(ModelBackend::Symbolic),
            ..ModelCheckConfig::reachable_graph()
        },
    )
    .full_reachable_graph_snapshot()
    .expect("symbolic typed checker should build without finite state domain");

    assert_eq!(
        snapshot.states,
        vec![
            SymbolicOnlyState { phase: Phase::Idle },
            SymbolicOnlyState { phase: Phase::Busy },
        ]
    );
    assert!(
        checks::SymbolicModelChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::bounded_lasso(3)
            },
        )
        .check_all()
        .expect("symbolic typed checker should run without finite state domain")
        .is_ok()
    );
}

#[test]
fn symbolic_bounded_lasso_does_not_call_state_bounded_domain() {
    let lowered = lower_spec(&PanicDomainSpec);
    assert!(
        checks::SymbolicModelChecker::with_config(
            &lowered,
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
    let lowered = lower_spec(&RegisteredEffectSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&MissingProgramReadPathSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&MissingInvariantReadPathSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&MissingPropertyReadPathSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
    let lowered = lower_spec(&MissingFairnessReadPathSpec);
    let err = checks::SymbolicModelChecker::with_config(
        &lowered,
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
