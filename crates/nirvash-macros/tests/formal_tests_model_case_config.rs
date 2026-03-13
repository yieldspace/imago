use nirvash::{ModelBackend, ModelCheckConfig};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{FiniteModelDomain as FormalFiniteModelDomain, formal_tests, subsystem_spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
struct State {
    busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, FormalFiniteModelDomain)]
enum Action {
    Start,
}

#[derive(Default)]
struct ConfiguredSpec;

#[subsystem_spec(model_cases(configured_model_cases))]
impl FrontendSpec for ConfiguredSpec {
    type State = State;
    type Action = Action;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State { busy: false }]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![Action::Start]
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_macros::nirvash_transition_program! {
            rule start when matches!(action, Action::Start) && !prev.busy => {
                set busy <= true;
            }
        })
    }
}

fn configured_model_cases() -> Vec<ModelInstance<State, Action>> {
    let checker_config = ModelCheckConfig {
        backend: Some(ModelBackend::Symbolic),
        ..ModelCheckConfig::default()
    };
    let doc_checker_config = ModelCheckConfig {
        backend: Some(ModelBackend::Explicit),
        ..ModelCheckConfig::default()
    };
    vec![
        ModelInstance::default()
            .with_check_deadlocks(false)
            .with_checker_config(checker_config)
            .with_doc_checker_config(doc_checker_config),
    ]
}

#[formal_tests(spec = ConfiguredSpec)]
const _: () = ();

#[test]
fn formal_tests_accept_model_cases_with_non_copy_configs() {
    let spec = ConfiguredSpec;
    let case = <ConfiguredSpec as FrontendSpec>::model_instances(&spec)
        .into_iter()
        .next()
        .expect("configured case");
    assert_eq!(case.checker_config().backend, Some(ModelBackend::Symbolic));
    assert_eq!(
        case.doc_checker_config()
            .expect("doc checker config")
            .backend,
        Some(ModelBackend::Explicit)
    );
}
