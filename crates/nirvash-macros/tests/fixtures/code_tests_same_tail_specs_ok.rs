#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::SpecOracle;
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding, nirvash_project,
    nirvash_project_output,
};

mod foo {
    use super::*;

    #[derive(Clone, Copy, Debug, Default)]
    #[code_tests]
    pub struct Spec;

    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        FormalFiniteModelDomain,
    )]
    pub enum State {
        Idle,
    }

    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        FormalFiniteModelDomain,
    )]
    pub enum Action {
        Tick,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum Output {
        Ack,
    }

    impl FrontendSpec for Spec {
        type State = State;
        type Action = Action;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![State::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![Action::Tick]
        }

        fn transition(&self, state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
            Some(*state)
        }
    }

    impl TemporalSpec for Spec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl SpecOracle for Spec {
        type ExpectedOutput = Output;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            Output::Ack
        }
    }
}

mod bar {
    use super::*;

    #[derive(Clone, Copy, Debug, Default)]
    #[code_tests]
    pub struct Spec;

    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        FormalFiniteModelDomain,
    )]
    pub enum State {
        Idle,
    }

    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        serde::Serialize,
        serde::Deserialize,
        FormalFiniteModelDomain,
    )]
    pub enum Action {
        Tick,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum Output {
        Ack,
    }

    impl FrontendSpec for Spec {
        type State = State;
        type Action = Action;

        fn initial_states(&self) -> Vec<Self::State> {
            vec![State::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![Action::Tick]
        }

        fn transition(&self, state: &Self::State, _action: &Self::Action) -> Option<Self::State> {
            Some(*state)
        }
    }

    impl TemporalSpec for Spec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl SpecOracle for Spec {
        type ExpectedOutput = Output;

        fn expected_output(
            &self,
            _prev: &Self::State,
            _action: &Self::Action,
            _next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            Output::Ack
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct FooBinding;

#[nirvash_binding(spec = foo::Spec)]
impl FooBinding {
    #[nirvash(action = foo::Action::Tick)]
    fn tick(&mut self) -> foo::Output {
        foo::Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> foo::State {
        foo::State::Idle
    }

    #[nirvash_project_output]
    fn project_output(_action: &foo::Action, output: &foo::Output) -> foo::Output {
        output.clone()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct BarBinding;

#[nirvash_binding(spec = bar::Spec)]
impl BarBinding {
    #[nirvash(action = bar::Action::Tick)]
    fn tick(&mut self) -> bar::Output {
        bar::Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> bar::State {
        bar::State::Idle
    }

    #[nirvash_project_output]
    fn project_output(_action: &bar::Action, output: &bar::Output) -> bar::Output {
        output.clone()
    }
}

nirvash::import_generated_tests! {
    spec = foo::Spec,
    binding = FooBinding,
}

nirvash::import_generated_tests! {
    spec = bar::Spec,
    binding = BarBinding,
}

fn main() {}
