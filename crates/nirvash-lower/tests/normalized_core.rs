use std::ptr;

use nirvash_ir::{ActionExpr, Definition, SpecCore, StateExpr, VarDecl};
use nirvash_lower::{ExecutableSemantics, LoweredSpec, SymbolicArtifacts};

fn structured_core() -> SpecCore {
    SpecCore {
        vars: vec![VarDecl {
            name: "state".to_owned(),
        }],
        defs: vec![Definition {
            name: "frontend".to_owned(),
            body: "LoweredSpecDemo".to_owned(),
        }],
        init: StateExpr::True,
        next: ActionExpr::Unchanged(vec!["state".to_owned()]),
        fairness: Vec::new(),
        invariants: Vec::new(),
        temporal_props: Vec::new(),
    }
}

fn lowered_spec() -> LoweredSpec<'static, bool, bool> {
    LoweredSpec::new(
        "LoweredSpecDemo",
        structured_core(),
        Vec::new(),
        std::any::type_name::<bool>(),
        std::any::type_name::<bool>(),
        SymbolicArtifacts::new(None, None, Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        ExecutableSemantics::new(
            vec![false],
            vec![true],
            None,
            |_, _| vec![false],
            |_| vec![(true, false)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
        ),
    )
}

#[test]
fn normalized_core_matches_direct_normalization() {
    let lowered = lowered_spec();
    let expected = lowered
        .core()
        .normalize()
        .expect("direct normalization succeeds");
    let actual = lowered
        .normalized_core()
        .expect("lowered spec normalization succeeds");

    assert_eq!(actual, &expected);
}

#[test]
fn normalized_core_reuses_cached_result() {
    let lowered = lowered_spec();
    let first = lowered
        .normalized_core()
        .expect("first normalization succeeds");
    let second = lowered
        .normalized_core()
        .expect("second normalization succeeds");

    assert!(ptr::eq(first, second));
}
