use std::any::type_name;

use nirvash::TransitionProgram;
use nirvash_check::SymbolicModelChecker;
use nirvash_ir::{
    ActionExpr as IrActionExpr, Definition, SpecCore, StateExpr as IrStateExpr, VarDecl,
};
use nirvash_lower::{
    BoolExpr, ExecutableSemantics, FrontendSpec, LoweredSpec, LoweringCx, SymbolicArtifacts,
    TemporalSpec, lookup_symbolic_state_schema,
};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
    nirvash_expr, nirvash_transition_program,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum State {
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
enum Action {
    Start,
    Finish,
}

#[derive(Default, Clone, Copy)]
struct DemoSpec;

impl FrontendSpec for DemoSpec {
    type State = State;
    type Action = Action;

    fn frontend_name(&self) -> &'static str {
        "DemoSpec"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start, Action::Finish]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && matches!(prev, State::Idle) => {
                set self <= State::Busy;
            }

            rule finish when matches!(action, Action::Finish) && matches!(prev, State::Busy) => {
                set self <= State::Idle;
            }
        })
    }
}

impl TemporalSpec for DemoSpec {
    fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
        vec![nirvash_expr!(state_is_declared(state) => matches!(state, State::Idle | State::Busy))]
    }
}

fn invalid_core_spec() -> LoweredSpec<'static, State, Action> {
    let spec = DemoSpec;
    let transition_program = spec.transition_program();
    let invariants = spec.invariants();
    let properties = spec.properties();
    let fairness = spec.executable_fairness();
    let symbolic_artifacts = SymbolicArtifacts::new(
        lookup_symbolic_state_schema::<State>(),
        transition_program.clone(),
        invariants.clone(),
        properties.clone(),
        fairness.clone(),
        Vec::new(),
    );
    let transition_spec = DemoSpec;
    let successors_spec = DemoSpec;
    let executable = ExecutableSemantics::new(
        spec.initial_states(),
        spec.actions(),
        transition_program,
        move |state, action| transition_spec.transition_relation(state, action),
        move |state| successors_spec.successors(state),
        invariants,
        properties,
        fairness,
        spec.default_model_backend(),
    );
    let core = SpecCore {
        vars: vec![VarDecl {
            name: "state".to_owned(),
        }],
        defs: vec![Definition {
            name: "frontend".to_owned(),
            body: "DemoSpec".to_owned(),
        }],
        init: IrStateExpr::True,
        next: IrActionExpr::Opaque("next".to_owned()),
        fairness: Vec::new(),
        invariants: Vec::new(),
        temporal_props: Vec::new(),
    };

    LoweredSpec::new(
        "DemoSpec",
        core,
        Vec::new(),
        type_name::<State>(),
        type_name::<Action>(),
        symbolic_artifacts,
        executable,
    )
}

#[test]
fn symbolic_checker_accepts_supported_normalized_core() {
    let spec = DemoSpec;
    let lowered = spec.lower(&mut LoweringCx).expect("spec lowers");

    let result = SymbolicModelChecker::new(&lowered)
        .check_all()
        .expect("supported spec should be checkable");

    assert!(result.is_ok());
    assert!(
        lowered
            .normalized_core()
            .expect("core normalizes")
            .fragment_profile
            .symbolic_supported
    );
}

#[test]
fn symbolic_checker_rejects_opaque_normalized_core_before_direct_artifacts() {
    let lowered = invalid_core_spec();
    let error = SymbolicModelChecker::new(&lowered)
        .check_all()
        .expect_err("opaque normalized core must be rejected");
    let message = format!("{error:?}");

    assert!(message.contains("normalized core"));
    assert!(message.contains("opaque nodes"));
}
