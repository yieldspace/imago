use std::sync::Mutex;

use nirvash_core::{
    ActionConstraint, ModelCase, ModelCaseSource, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::{
        ActionApplier, NegativeWitness, PositiveWitness, ProtocolConformanceSpec,
        ProtocolInputWitnessBinding, ProtocolRuntimeBinding, RegisteredCodeWitnessTestProvider,
        StateObserver,
    },
};
use nirvash_macros::Signature as FormalSignature;
use nirvash_macros::code_witness_tests;

#[derive(Clone, Copy, Debug, Default)]
struct Spec;

#[derive(Clone, Copy, Debug, Default)]
struct Binding;

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Starting,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    StartFast,
    StartSlow,
    FinishSlow,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Ack,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Context;

#[derive(Debug, Default)]
struct Session {
    context: Context,
    history: Vec<Action>,
}

struct Driver {
    state: Mutex<State>,
}

impl TransitionSystem for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![
            Action::StartFast,
            Action::StartSlow,
            Action::FinishSlow,
            Action::Stop,
        ]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (State::Idle, Action::StartFast) => Some(State::Busy),
            (State::Idle, Action::StartSlow) => Some(State::Starting),
            (State::Starting, Action::FinishSlow) => Some(State::Busy),
            (State::Busy, Action::Stop) => Some(State::Idle),
            _ => None,
        }
    }
}

impl ModelCaseSource for Spec {
    fn model_cases(&self) -> Vec<ModelCase<Self::State, Self::Action>> {
        vec![
            ModelCase::new("fast_path")
                .with_action_constraint(ActionConstraint::new("fast_path_only", |_, action, _| {
                    !matches!(action, Action::StartSlow | Action::FinishSlow)
                })),
            ModelCase::new("slow_path")
                .with_action_constraint(ActionConstraint::new("slow_path_only", |_, action, _| {
                    !matches!(action, Action::StartFast)
                })),
        ]
    }
}

impl TemporalSpec for Spec {
    fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
        Vec::new()
    }
}

impl ProtocolConformanceSpec for Spec {
    type ExpectedOutput = Output;
    type ProbeState = State;
    type ProbeOutput = Output;
    type SummaryState = State;
    type SummaryOutput = Output;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if next.is_some() {
            Output::Ack
        } else {
            Output::Rejected
        }
    }

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
        *probe
    }

    fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
        probe.clone()
    }

    fn abstract_state(&self, observed: &Self::SummaryState) -> Self::State {
        *observed
    }

    fn abstract_output(&self, observed: &Self::SummaryOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

#[code_witness_tests(spec = Spec, binding = Binding)]
const _: () = ();

impl ProtocolRuntimeBinding<Spec> for Binding {
    type Runtime = Driver;
    type Context = Context;

    async fn fresh_runtime(_spec: &Spec) -> Self::Runtime {
        Driver {
            state: Mutex::new(State::Idle),
        }
    }

    fn context(_spec: &Spec) -> Self::Context {
        Context
    }
}

impl ProtocolInputWitnessBinding<Spec> for Binding {
    type Input = Action;
    type Session = Session;

    async fn fresh_session(_spec: &Spec) -> Self::Session {
        Session {
            context: Context,
            history: Vec::new(),
        }
    }

    fn positive_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        prev: &State,
        action: &Action,
        _next: &State,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>> {
        let witness_name = match (session.history.as_slice(), prev, action) {
            ([], State::Idle, Action::StartFast) => "cold_start",
            ([], State::Idle, Action::StartSlow) => "slow_boot",
            ([Action::StartSlow], State::Starting, Action::FinishSlow) => "slow_finish",
            ([Action::StartFast], State::Busy, Action::Stop) => "fast_stop",
            ([Action::StartSlow, Action::FinishSlow], State::Busy, Action::Stop) => "slow_stop",
            _ => "principal",
        };
        vec![
            PositiveWitness::new(witness_name, session.context, action.clone())
                .with_canonical(true),
        ]
    }

    fn negative_witnesses(
        _spec: &Spec,
        session: &Self::Session,
        _prev: &State,
        action: &Action,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>> {
        let witness_name = match (session.history.as_slice(), action) {
            ([], Action::FinishSlow) => "reject_finish_without_slow_start",
            ([Action::StartFast], Action::StartFast) => "reject_double_fast_start",
            _ => "reject",
        };
        vec![NegativeWitness::new(
            witness_name,
            session.context,
            action.clone(),
        )]
    }

    async fn execute_input(
        runtime: &Self::Runtime,
        session: &mut Self::Session,
        context: &Self::Context,
        input: &Self::Input,
    ) -> Output {
        let output = runtime.execute_action(context, input).await;
        session.history.push(input.clone());
        output
    }

    fn probe_context(session: &Self::Session) -> Self::Context {
        session.context
    }
}

impl ActionApplier for Driver {
    type Action = Action;
    type Output = Output;
    type Context = Context;

    async fn execute_action(&self, _context: &Context, action: &Action) -> Output {
        let mut state = self.state.lock().expect("lock state");
        match (*state, action) {
            (State::Idle, Action::StartFast) => {
                *state = State::Busy;
                Output::Ack
            }
            (State::Idle, Action::StartSlow) => {
                *state = State::Starting;
                Output::Ack
            }
            (State::Starting, Action::FinishSlow) => {
                *state = State::Busy;
                Output::Ack
            }
            (State::Busy, Action::Stop) => {
                *state = State::Idle;
                Output::Ack
            }
            _ => Output::Rejected,
        }
    }
}

impl StateObserver for Driver {
    type SummaryState = State;
    type Context = Context;

    async fn observe_state(&self, _context: &Context) -> State {
        *self.state.lock().expect("lock state")
    }
}

fn generated_test_names() -> Vec<String> {
    let mut names = nirvash_core::inventory::iter::<RegisteredCodeWitnessTestProvider>
        .into_iter()
        .flat_map(|provider| (provider.build)())
        .map(|test| test.name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_history_sensitive_names_registered() {
    let names = generated_test_names();

    assert!(
        names.iter().any(|name| {
            name.contains("/from_init/when_StartFast/")
                && name.contains("/cold_start-0")
                && name.contains("/via_")
        }),
        "missing from_init scenario in witness names: {names:#?}"
    );
    assert!(
        names.iter().any(|name| {
            name.contains("/after_StartFast/when_Stop/")
                && name.contains("/fast_stop-0")
                && name.contains("/via_")
        }),
        "missing fast-path stop scenario in witness names: {names:#?}"
    );
    assert!(
        names.iter().any(|name| {
            name.contains("/after_StartSlow__FinishSlow/when_Stop/")
                && name.contains("/slow_stop-0")
                && name.contains("/via_")
        }),
        "missing slow-path stop scenario in witness names: {names:#?}"
    );
}

#[doc(hidden)]
pub fn __nirvash_code_witness_main_marker() {}

fn main() {
    assert_history_sensitive_names_registered();
    nirvash_core::conformance::run_registered_code_witness_tests();
}
