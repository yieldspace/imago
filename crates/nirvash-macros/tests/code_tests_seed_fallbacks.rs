#![allow(unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::{GeneratedBinding, ProjectedState, SpecOracle};
use nirvash_lower::{FiniteModelDomain, FrontendSpec, TemporalSpec};
use nirvash_macros::{nirvash_binding, nirvash_project, nirvash_project_output};
use proptest::strategy::Strategy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    Idle,
}

impl FiniteModelDomain for State {
    fn finite_domain() -> nirvash::BoundedDomain<Self> {
        nirvash::BoundedDomain::new(vec![Self::Idle])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DefaultOnlyKey {
    flag: bool,
}

impl Default for DefaultOnlyKey {
    fn default() -> Self {
        Self { flag: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ArbitraryOnlyKey {
    token: u8,
}

impl proptest::arbitrary::Arbitrary for ArbitraryOnlyKey {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        proptest::sample::select(vec![
            ArbitraryOnlyKey { token: 7 },
            ArbitraryOnlyKey { token: 9 },
        ])
        .boxed()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Action {
    DefaultPut(DefaultOnlyKey),
    RandomPut(ArbitraryOnlyKey),
}

impl FiniteModelDomain for Action {
    fn finite_domain() -> nirvash::BoundedDomain<Self> {
        nirvash::BoundedDomain::new(Vec::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Output {
    Ack,
}

#[derive(Default)]
struct Spec;

impl FrontendSpec for Spec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        Vec::new()
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

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Binding;

#[nirvash_binding(spec = Spec)]
impl Binding {
    #[nirvash(action = Action::DefaultPut)]
    fn default_put(&mut self, _key: DefaultOnlyKey) -> Output {
        Output::Ack
    }

    #[nirvash(action = Action::RandomPut)]
    fn random_put(&mut self, _key: ArbitraryOnlyKey) -> Output {
        Output::Ack
    }

    #[nirvash_project]
    fn project(&self) -> ProjectedState<State> {
        ProjectedState::Exact(State::Idle)
    }

    #[nirvash_project_output]
    fn project_output(_action: &Action, output: &Output) -> Output {
        output.clone()
    }
}

#[test]
fn generated_action_candidates_include_default_and_arbitrary_payloads() {
    let actions = <Binding as GeneratedBinding<Spec>>::generated_action_candidates(
        &Spec,
        &nirvash_conformance::small::<Spec>(),
    )
    .expect("generated action candidates");

    assert!(actions.contains(&Action::DefaultPut(DefaultOnlyKey::default())));
    assert!(
        actions.iter().any(|action| {
            matches!(action, Action::RandomPut(ArbitraryOnlyKey { token: 7 | 9 }))
        })
    );
}
