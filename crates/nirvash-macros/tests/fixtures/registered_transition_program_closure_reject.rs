use nirvash_macros::{Signature as FormalSignature, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
}

struct Spec;

#[subsystem_spec]
impl ::nirvash_core::TransitionSystem for Spec {
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
    ) -> Option<::nirvash_core::TransitionProgram<Self::State, Self::Action>> {
        Some(::nirvash_core::TransitionProgram::named(
            "spec",
            vec![::nirvash_core::TransitionRule::new(
                "start",
                |state, action| matches!((state, action), (State::Idle, Action::Start)),
                ::nirvash_core::UpdateProgram::new("to_busy", |_, _| State::Busy),
            )],
        ))
    }
}

fn main() {}
