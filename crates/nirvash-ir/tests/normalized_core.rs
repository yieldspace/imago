use nirvash_ir::{
    ActionExpr, ComparisonOp, CoreNormalizationError, Definition, FairnessDecl, QuantifierKind,
    SpecCore, StateExpr, TemporalExpr, ValueExpr, VarDecl, ViewExpr,
};

fn structured_core() -> SpecCore {
    SpecCore {
        vars: vec![VarDecl {
            name: "state".to_owned(),
        }],
        defs: vec![Definition {
            name: "frontend".to_owned(),
            body: "StructuredSpec".to_owned(),
        }],
        init: StateExpr::True,
        next: ActionExpr::Unchanged(vec!["state".to_owned()]),
        fairness: Vec::new(),
        invariants: vec![StateExpr::Compare {
            op: ComparisonOp::Eq,
            lhs: ValueExpr::Field("state".to_owned()),
            rhs: ValueExpr::Literal("ready".to_owned()),
        }],
        temporal_props: Vec::new(),
    }
}

#[test]
fn normalizes_structured_core_as_supported_fragment() {
    let normalized = structured_core()
        .normalize()
        .expect("structured core normalizes");

    assert!(!normalized.fragment_profile.has_opaque_nodes);
    assert!(!normalized.fragment_profile.has_stringly_nodes);
    assert!(normalized.fragment_profile.symbolic_supported);
    assert!(normalized.fragment_profile.proof_supported);
}

#[test]
fn classifies_stringly_and_opaque_fragments() {
    let normalized = SpecCore {
        init: StateExpr::Opaque("init".to_owned()),
        next: ActionExpr::Quantified {
            kind: QuantifierKind::Exists,
            domain: "DOMAIN".to_owned(),
            body: "predicate".to_owned(),
            read_paths: vec!["state.counter".to_owned()],
            symbolic_supported: false,
        },
        fairness: vec![FairnessDecl::WF {
            view: ViewExpr::Vars,
            action: ActionExpr::Ref("Step".to_owned()),
        }],
        temporal_props: vec![TemporalExpr::Ref("EventuallyReady".to_owned())],
        ..structured_core()
    }
    .normalize()
    .expect("fragment profile still computes");

    assert!(normalized.fragment_profile.has_opaque_nodes);
    assert!(normalized.fragment_profile.has_stringly_nodes);
    assert!(normalized.fragment_profile.has_stringly_quantifiers);
    assert!(normalized.fragment_profile.has_temporal_props);
    assert!(normalized.fragment_profile.has_fairness);
    assert!(!normalized.fragment_profile.symbolic_supported);
    assert!(!normalized.fragment_profile.proof_supported);
}

#[test]
fn rejects_empty_variable_names_during_normalization() {
    let error = SpecCore {
        vars: vec![VarDecl {
            name: String::new(),
        }],
        ..structured_core()
    }
    .normalize()
    .expect_err("empty variable names should be rejected");

    assert_eq!(
        error,
        CoreNormalizationError::EmptyVariableName { index: 0 }
    );
}
