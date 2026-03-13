use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
}

struct Spec;

#[subsystem_spec]
impl ::nirvash_lower::FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start]
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash::TransitionProgram::named(
            "spec",
            vec![::nirvash::TransitionRule::new(
                "start",
                |state, action| matches!((state, action), (State::Idle, Action::Start)),
                ::nirvash::UpdateProgram::new("to_busy", |_, _| State::Busy),
            )],
        ))
    }
}

fn main() {}
