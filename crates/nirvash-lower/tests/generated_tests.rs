use nirvash_ir::{
    ActionExpr, ComparisonOp, Definition, SpecCore, StateExpr, TemporalExpr, UpdateExpr,
    UpdateOpDecl, ValueExpr, VarDecl, ViewExpr,
};
use nirvash_lower::{
    BoundaryMining, ExecutableSemantics, LoweredSpec, ModelBackend, ModelCheckConfig,
    ModelInstance, SymbolicArtifacts,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
}

fn structured_core() -> SpecCore {
    SpecCore {
        vars: vec![VarDecl {
            name: "state".to_owned(),
        }],
        defs: vec![Definition {
            name: "frontend".to_owned(),
            body: "GeneratedTestsSpec".to_owned(),
        }],
        init: StateExpr::True,
        next: ActionExpr::Unchanged(vec!["state".to_owned()]),
        fairness: Vec::new(),
        invariants: Vec::new(),
        temporal_props: Vec::new(),
    }
}

fn normalized_boundary_core() -> SpecCore {
    SpecCore {
        vars: vec![VarDecl {
            name: "count".to_owned(),
        }],
        defs: vec![Definition {
            name: "frontend".to_owned(),
            body: "GeneratedTestsSpec".to_owned(),
        }],
        init: StateExpr::Compare {
            op: ComparisonOp::Eq,
            lhs: ValueExpr::Field("count".to_owned()),
            rhs: ValueExpr::Literal("0".to_owned()),
        },
        next: ActionExpr::BoxAction {
            action: Box::new(ActionExpr::Rule {
                name: "start".to_owned(),
                guard: Box::new(ActionExpr::Compare {
                    op: ComparisonOp::Lt,
                    lhs: ValueExpr::Field("count".to_owned()),
                    rhs: ValueExpr::Literal("2".to_owned()),
                }),
                update: UpdateExpr::Sequence(vec![
                    UpdateOpDecl::Assign {
                        target: "count".to_owned(),
                        value: ValueExpr::Literal("3".to_owned()),
                    },
                    UpdateOpDecl::SetInsert {
                        target: "seen".to_owned(),
                        item: ValueExpr::Literal("Start".to_owned()),
                    },
                ]),
            }),
            view: ViewExpr::Vars,
        },
        fairness: Vec::new(),
        invariants: vec![StateExpr::Compare {
            op: ComparisonOp::Ge,
            lhs: ValueExpr::Field("count".to_owned()),
            rhs: ValueExpr::Literal("1".to_owned()),
        }],
        temporal_props: vec![TemporalExpr::Always(Box::new(TemporalExpr::Enabled(
            ActionExpr::Ref("start".to_owned()),
        )))],
    }
}

fn lowered_spec() -> LoweredSpec<'static, State, Action> {
    LoweredSpec::new(
        "GeneratedTestsSpec",
        structured_core(),
        vec![
            ModelInstance::new("explicit-small").with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::default()
            }),
        ],
        std::any::type_name::<State>(),
        std::any::type_name::<Action>(),
        SymbolicArtifacts::new(None, None, Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        ExecutableSemantics::new(
            vec![State::Idle],
            vec![Action::Start, Action::Stop],
            None,
            |state, action| match (state, action) {
                (State::Idle, Action::Start) => vec![State::Busy],
                _ => Vec::new(),
            },
            |state| match state {
                State::Idle => vec![(Action::Start, State::Busy)],
                State::Busy => Vec::new(),
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(ModelBackend::Explicit),
        ),
    )
}

#[test]
fn generated_test_core_reports_metadata() {
    let lowered = lowered_spec();
    let core = lowered.generated_test_core();

    assert_eq!(core.spec_name, "GeneratedTestsSpec");
    assert_eq!(core.state_ty, std::any::type_name::<State>());
    assert_eq!(core.action_ty, std::any::type_name::<Action>());
    assert_eq!(core.default_model_backend, Some(ModelBackend::Explicit));
    assert_eq!(core.model_cases, vec!["explicit-small"]);
}

#[test]
fn generated_test_domains_collect_state_and_action_candidates() {
    let lowered = lowered_spec();
    let domains = lowered.generated_test_domains();

    assert_eq!(domains.states, vec![State::Idle, State::Busy]);
    assert_eq!(domains.actions, vec![Action::Start, Action::Stop]);
}

#[test]
fn boundary_mining_tracks_boundary_and_terminal_states() {
    let lowered = lowered_spec();
    let mining = BoundaryMining::mine(&lowered);
    let boundaries = lowered.generated_test_boundaries();

    assert_eq!(mining.initial_states, vec![State::Idle]);
    assert_eq!(boundaries.initial_states, vec![State::Idle]);
    assert_eq!(boundaries.boundary_states, vec![State::Busy]);
    assert_eq!(boundaries.terminal_states, vec![State::Busy]);
    assert_eq!(boundaries.transitions.len(), 1);
    assert_eq!(boundaries.transitions[0].prev, State::Idle);
    assert_eq!(boundaries.transitions[0].action, Action::Start);
    assert_eq!(boundaries.transitions[0].next, State::Busy);
}

#[test]
fn boundary_mining_extracts_normalized_core_literals_and_thresholds() {
    let lowered = LoweredSpec::new(
        "GeneratedTestsSpec",
        normalized_boundary_core(),
        vec![
            ModelInstance::new("explicit-small").with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Explicit),
                ..ModelCheckConfig::default()
            }),
        ],
        std::any::type_name::<State>(),
        std::any::type_name::<Action>(),
        SymbolicArtifacts::new(None, None, Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        ExecutableSemantics::new(
            vec![State::Idle],
            vec![Action::Start, Action::Stop],
            None,
            |state, action| match (state, action) {
                (State::Idle, Action::Start) => vec![State::Busy],
                _ => Vec::new(),
            },
            |state| match state {
                State::Idle => vec![(Action::Start, State::Busy)],
                State::Busy => Vec::new(),
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(ModelBackend::Explicit),
        ),
    );

    let domains = lowered.generated_test_domains();
    let boundaries = lowered.generated_test_boundaries();

    assert_eq!(domains.catalog.comparison_literals, vec!["0", "2", "1"]);
    assert_eq!(domains.catalog.cardinality_thresholds, vec![0, 2, 3, 1]);
    assert_eq!(domains.catalog.action_literals, vec!["start"]);
    assert!(
        domains
            .catalog
            .update_literals
            .iter()
            .any(|value| value.contains("assign:count"))
    );
    assert!(
        domains
            .catalog
            .update_literals
            .iter()
            .any(|value| value.contains("insert:seen"))
    );
    assert!(
        domains
            .catalog
            .temporal_bad_prefix_guards
            .iter()
            .any(|value| value.contains("Always"))
    );
    assert_eq!(boundaries.catalog, domains.catalog);
}
