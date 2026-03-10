use nirvash_core::{TransitionSystem, collect_doc_graph_specs, format_doc_graph_action};

#[derive(Clone, Copy, Debug, PartialEq, Eq, nirvash_macros::Signature)]
enum InnerAction {
    /// Inner action
    Inner,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, nirvash_macros::Signature, nirvash_macros::ActionVocabulary,
)]
enum WrapperAction {
    /// Explicit wrapper
    /// Second line should be ignored.
    Explicit(InnerAction),
    Delegated(InnerAction),
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, nirvash_macros::Signature)]
enum DemoState {
    Idle,
    Busy,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, nirvash_macros::Signature, nirvash_macros::ActionVocabulary,
)]
enum DemoAction {
    /// Start demo
    #[viz(compact_label = "start", scenario_priority = 7)]
    Start,
    /// Reset demo
    Reset,
}

#[derive(Debug, Default, Clone, Copy)]
struct DemoSpec;

#[nirvash_macros::subsystem_spec]
impl TransitionSystem for DemoSpec {
    type State = DemoState;
    type Action = DemoAction;

    fn name(&self) -> &'static str {
        "demo_action_docs"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![DemoState::Idle]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        match (state, action) {
            (DemoState::Idle, DemoAction::Start) => Some(DemoState::Busy),
            (DemoState::Busy, DemoAction::Reset) => Some(DemoState::Idle),
            _ => None,
        }
    }
}

#[nirvash_macros::formal_tests(spec = DemoSpec)]
const _: () = ();

#[test]
fn action_vocabulary_derive_uses_signature_domain() {
    assert_eq!(
        <WrapperAction as nirvash_core::ActionVocabulary>::action_vocabulary(),
        vec![
            WrapperAction::Explicit(InnerAction::Inner),
            WrapperAction::Delegated(InnerAction::Inner),
            WrapperAction::Missing,
        ]
    );
}

#[test]
fn signature_derive_registers_action_docs_and_delegates_single_field_wrappers() {
    assert_eq!(
        format_doc_graph_action(&WrapperAction::Explicit(InnerAction::Inner)),
        "Explicit wrapper"
    );
    assert_eq!(
        format_doc_graph_action(&WrapperAction::Delegated(InnerAction::Inner)),
        "Inner action"
    );
    assert_eq!(format_doc_graph_action(&WrapperAction::Missing), "Missing");
}

#[test]
fn formal_tests_use_doc_driven_edge_labels() {
    let spec = collect_doc_graph_specs()
        .into_iter()
        .find(|spec| spec.spec_name == "DemoSpec")
        .expect("demo spec should be registered");
    let case = spec.cases.into_iter().next().expect("default case");
    assert_eq!(case.graph.edges[0][0].label, "Start demo");
    assert_eq!(case.graph.edges[0][0].compact_label.as_deref(), Some("start"));
    assert_eq!(case.graph.edges[0][0].scenario_priority, Some(7));
}
