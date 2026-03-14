#![allow(dead_code, unused_imports)]

use nirvash::BoolExpr;
use nirvash_conformance::SpecOracle;
use nirvash_lower::{FrontendSpec, TemporalSpec};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, code_tests, nirvash_binding, nirvash_project,
    nirvash_project_output,
};

#[derive(Clone, Copy, Debug, Default)]
#[code_tests]
struct ConcurrentSpec;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum ConcurrentState {
    Idle,
    Busy,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, FormalFiniteModelDomain,
)]
enum ConcurrentAction {
    Enter,
    Leave,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ConcurrentOutput {
    Ack,
    Rejected,
}

impl FrontendSpec for ConcurrentSpec {
    type State = ConcurrentState;
    type Action = ConcurrentAction;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![ConcurrentState::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![ConcurrentAction::Enter, ConcurrentAction::Leave]
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (ConcurrentState::Idle, ConcurrentAction::Enter) => Some(ConcurrentState::Busy),
            (ConcurrentState::Busy, ConcurrentAction::Leave) => Some(ConcurrentState::Idle),
            _ => None,
        }
    }
}

impl TemporalSpec for ConcurrentSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        Vec::new()
    }
}

impl SpecOracle for ConcurrentSpec {
    type ExpectedOutput = ConcurrentOutput;

    fn expected_output(
        &self,
        _prev: &Self::State,
        _action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        if next.is_some() {
            ConcurrentOutput::Ack
        } else {
            ConcurrentOutput::Rejected
        }
    }
}

#[derive(Clone, Debug)]
struct ConcurrentBinding {
    state: ConcurrentState,
}

impl Default for ConcurrentBinding {
    fn default() -> Self {
        Self {
            state: ConcurrentState::Idle,
        }
    }
}

#[nirvash_binding(spec = ConcurrentSpec, concurrent)]
impl ConcurrentBinding {
    #[nirvash(action = ConcurrentAction::Enter)]
    fn enter(&mut self) -> ConcurrentOutput {
        match self.state {
            ConcurrentState::Idle => {
                self.state = ConcurrentState::Busy;
                ConcurrentOutput::Ack
            }
            ConcurrentState::Busy => ConcurrentOutput::Rejected,
        }
    }

    #[nirvash(action = ConcurrentAction::Leave)]
    fn leave(&mut self) -> ConcurrentOutput {
        match self.state {
            ConcurrentState::Busy => {
                self.state = ConcurrentState::Idle;
                ConcurrentOutput::Ack
            }
            ConcurrentState::Idle => ConcurrentOutput::Rejected,
        }
    }

    #[nirvash_project]
    fn project(&self) -> ConcurrentState {
        self.state
    }

    #[nirvash_project_output]
    fn project_output(_action: &ConcurrentAction, output: &ConcurrentOutput) -> ConcurrentOutput {
        output.clone()
    }
}

generated::install::loom_tests!(binding = ConcurrentBinding);
