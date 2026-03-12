use nirvash::{ModelBackend, ModelCase, ModelCheckConfig, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, formal_tests, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
struct State {
    busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalSignature)]
enum Action {
    Start,
}

#[derive(Default)]
struct ConfiguredSpec;

#[subsystem_spec(model_cases(configured_model_cases))]
impl TransitionSystem for ConfiguredSpec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State { busy: false }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start]
    }

    fn transition_program(&self) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_macros::nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && !prev.busy => {
                set busy <= true;
            }
        })
    }
}

fn configured_model_cases() -> Vec<ModelCase<State, Action>> {
    let checker_config = ModelCheckConfig {
        backend: Some(ModelBackend::Symbolic),
        ..ModelCheckConfig::default()
    };
    let doc_checker_config = ModelCheckConfig {
        backend: Some(ModelBackend::Explicit),
        ..ModelCheckConfig::default()
    };
    vec![ModelCase::default()
        .with_check_deadlocks(false)
        .with_checker_config(checker_config)
        .with_doc_checker_config(doc_checker_config)]
}

#[formal_tests(spec = ConfiguredSpec)]
const _: () = ();

#[test]
fn formal_tests_accept_model_cases_with_non_copy_configs() {
    let spec = ConfiguredSpec;
    let case = <ConfiguredSpec as nirvash::ModelCaseSource>::model_cases(&spec)
        .into_iter()
        .next()
        .expect("configured case");
    assert_eq!(case.checker_config().backend, Some(ModelBackend::Symbolic));
    assert_eq!(
        case.doc_checker_config().expect("doc checker config").backend,
        Some(ModelBackend::Explicit)
    );
}
