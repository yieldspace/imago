use nirvash::{RelSet, Relation2, summarize_doc_graph_state};
use nirvash_macros::{FiniteModelDomain, RelAtom, RelationalState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, FiniteModelDomain, RelAtom)]
enum Atom {
    Root,
    Dependency,
}

#[derive(Debug, RelationalState)]
struct DemoState {
    requires: Relation2<Atom, Atom>,
    allowed: RelSet<Atom>,
    counter: u8,
}

#[test]
fn derive_relational_state_registers_relation_schema_and_summary() {
    let state = DemoState {
        requires: Relation2::from_pairs([(Atom::Root, Atom::Dependency)]),
        allowed: RelSet::from_items([Atom::Root]),
        counter: 7,
    };
    assert_eq!(state.counter, 7);

    let summarized = summarize_doc_graph_state(&state);

    assert!(summarized.summary.contains("requires = Root->Dependency"));
    assert_eq!(
        summarized
            .relation_fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["requires", "allowed"]
    );
    assert_eq!(
        summarized
            .relation_schema
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["requires", "allowed"]
    );
    assert!(summarized.full.contains("counter: 7"));
}
