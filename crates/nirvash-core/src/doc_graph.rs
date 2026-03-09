use std::{collections::BTreeMap, collections::BTreeSet, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::{
    RelationFieldSchema, RelationFieldSummary, StatePredicate, collect_relational_state_schema,
    collect_relational_state_summary, registry::lookup_action_doc_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableGraphEdge<A> {
    pub action: A,
    pub target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachableGraphSnapshot<S, A> {
    pub states: Vec<S>,
    pub edges: Vec<Vec<ReachableGraphEdge<A>>>,
    pub initial_indices: Vec<usize>,
    pub deadlocks: Vec<usize>,
    pub truncated: bool,
    pub stutter_omitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphState {
    pub summary: String,
    pub full: String,
    pub relation_fields: Vec<RelationFieldSummary>,
    pub relation_schema: Vec<RelationFieldSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphEdge {
    pub label: String,
    pub target: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocGraphReductionMode {
    Full,
    BoundaryPaths,
}

#[derive(Debug, Clone)]
pub struct DocGraphPolicy<S> {
    pub reduction: DocGraphReductionMode,
    pub focus_states: Vec<StatePredicate<S>>,
    pub max_edge_actions_in_label: usize,
}

impl<S> DocGraphPolicy<S> {
    pub fn full() -> Self {
        Self {
            reduction: DocGraphReductionMode::Full,
            focus_states: Vec::new(),
            max_edge_actions_in_label: 2,
        }
    }

    pub fn boundary_paths() -> Self {
        Self::default()
    }

    pub fn with_focus_state(mut self, predicate: StatePredicate<S>) -> Self {
        self.focus_states.push(predicate);
        self
    }

    pub fn with_max_edge_actions_in_label(mut self, max_edge_actions_in_label: usize) -> Self {
        self.max_edge_actions_in_label = max_edge_actions_in_label.max(1);
        self
    }
}

impl<S> Default for DocGraphPolicy<S> {
    fn default() -> Self {
        Self {
            reduction: DocGraphReductionMode::BoundaryPaths,
            focus_states: Vec::new(),
            max_edge_actions_in_label: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphSnapshot {
    pub states: Vec<DocGraphState>,
    pub edges: Vec<Vec<DocGraphEdge>>,
    pub initial_indices: Vec<usize>,
    pub deadlocks: Vec<usize>,
    pub truncated: bool,
    pub stutter_omitted: bool,
    pub focus_indices: Vec<usize>,
    pub reduction: DocGraphReductionMode,
    pub max_edge_actions_in_label: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphCase {
    pub label: String,
    pub graph: DocGraphSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphSpec {
    pub spec_name: String,
    pub cases: Vec<DocGraphCase>,
}

pub trait DocGraphProvider {
    fn spec_name(&self) -> &'static str;

    fn cases(&self) -> Vec<DocGraphCase>;
}

pub struct RegisteredDocGraphProvider {
    pub spec_name: &'static str,
    pub build: fn() -> Box<dyn DocGraphProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedDocGraphNode {
    pub original_index: usize,
    pub state: DocGraphState,
    pub is_initial: bool,
    pub is_deadlock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedDocGraphEdge {
    pub source: usize,
    pub target: usize,
    pub label: String,
    pub collapsed_state_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedDocGraph {
    pub states: Vec<ReducedDocGraphNode>,
    pub edges: Vec<ReducedDocGraphEdge>,
    pub truncated: bool,
    pub stutter_omitted: bool,
}

inventory::collect!(RegisteredDocGraphProvider);

pub fn summarize_doc_graph_state<T>(state: &T) -> DocGraphState
where
    T: Debug + 'static,
{
    let full = format!("{state:#?}");
    let relation_fields = collect_relational_state_summary(state);
    let relation_schema = collect_relational_state_schema::<T>();
    let base_summary = summarize_doc_graph_text(&full);
    let summary = if relation_fields.is_empty() {
        base_summary
    } else {
        summarize_doc_graph_text(&format!(
            "{}\n{}",
            relation_fields
                .iter()
                .map(|field| field.notation.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            base_summary
        ))
    };
    DocGraphState {
        summary,
        full,
        relation_fields,
        relation_schema,
    }
}

pub fn format_doc_graph_action<T>(value: &T) -> String
where
    T: Debug + 'static,
{
    lookup_action_doc_label(value as &dyn std::any::Any).unwrap_or_else(|| format!("{value:?}"))
}

pub fn summarize_doc_graph_text(input: &str) -> String {
    const MAX_LINES: usize = 8;
    const MAX_CHARS: usize = 240;

    let mut lines = Vec::new();
    let mut total_chars = 0usize;

    for raw_line in input.lines() {
        if lines.len() == MAX_LINES {
            break;
        }

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let available = MAX_CHARS.saturating_sub(total_chars);
        if available == 0 {
            break;
        }

        let mut line = trimmed.chars().take(available).collect::<String>();
        let raw_len = trimmed.chars().count();
        total_chars += line.chars().count();
        if raw_len > line.chars().count() {
            if !line.ends_with("...") {
                line.push_str("...");
            }
            lines.push(line);
            return lines.join("\n");
        }

        lines.push(line);
    }

    if lines.is_empty() {
        return input.trim().chars().take(MAX_CHARS).collect();
    }

    let consumed_lines = input.lines().filter(|line| !line.trim().is_empty()).count();
    let consumed_chars = lines.iter().map(|line| line.chars().count()).sum::<usize>();
    if (consumed_lines > lines.len() || input.chars().count() > consumed_chars)
        && let Some(last) = lines.last_mut()
        && !last.ends_with("...")
    {
        if last.chars().count() + 3 > MAX_CHARS && !last.is_empty() {
            let mut shortened = last
                .chars()
                .take(last.chars().count().saturating_sub(3))
                .collect::<String>();
            shortened.push_str("...");
            *last = shortened;
        } else {
            last.push_str("...");
        }
    }

    lines.join("\n")
}

pub fn collect_doc_graph_specs() -> Vec<DocGraphSpec> {
    let mut specs = inventory::iter::<RegisteredDocGraphProvider>
        .into_iter()
        .map(|entry| {
            let provider = (entry.build)();
            DocGraphSpec {
                spec_name: provider.spec_name().to_owned(),
                cases: provider.cases(),
            }
        })
        .collect::<Vec<_>>();
    specs.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));
    specs
}

pub fn reduce_doc_graph(snapshot: &DocGraphSnapshot) -> ReducedDocGraph {
    match snapshot.reduction {
        DocGraphReductionMode::Full => full_doc_graph(snapshot),
        DocGraphReductionMode::BoundaryPaths => boundary_path_reduced_graph(snapshot),
    }
}

fn full_doc_graph(snapshot: &DocGraphSnapshot) -> ReducedDocGraph {
    ReducedDocGraph {
        states: (0..snapshot.states.len())
            .map(|index| ReducedDocGraphNode {
                original_index: index,
                state: snapshot.states[index].clone(),
                is_initial: snapshot.initial_indices.contains(&index),
                is_deadlock: snapshot.deadlocks.contains(&index),
            })
            .collect(),
        edges: snapshot
            .edges
            .iter()
            .enumerate()
            .flat_map(|(source, outgoing)| {
                outgoing.iter().map(move |edge| ReducedDocGraphEdge {
                    source,
                    target: edge.target,
                    label: summarize_reduced_edge_labels(
                        &[edge.label.as_str()],
                        snapshot.max_edge_actions_in_label,
                    ),
                    collapsed_state_indices: Vec::new(),
                })
            })
            .collect(),
        truncated: snapshot.truncated,
        stutter_omitted: snapshot.stutter_omitted,
    }
}

fn boundary_path_reduced_graph(snapshot: &DocGraphSnapshot) -> ReducedDocGraph {
    let keep_indices = keep_state_indices(snapshot);
    let keep = (0..snapshot.states.len())
        .map(|index| keep_indices.contains(&index))
        .collect::<Vec<_>>();

    let states = keep_indices
        .iter()
        .copied()
        .map(|index| ReducedDocGraphNode {
            original_index: index,
            state: snapshot.states[index].clone(),
            is_initial: snapshot.initial_indices.contains(&index),
            is_deadlock: snapshot.deadlocks.contains(&index),
        })
        .collect::<Vec<_>>();

    let mut edges = Vec::new();
    for &source in &keep_indices {
        for edge in &snapshot.edges[source] {
            edges.push(collapse_edge_path(snapshot, &keep, source, edge));
        }
    }
    let edges = coalesce_reduced_edges(edges, snapshot.max_edge_actions_in_label);

    ReducedDocGraph {
        states,
        edges,
        truncated: snapshot.truncated,
        stutter_omitted: snapshot.stutter_omitted,
    }
}

fn keep_state_indices(snapshot: &DocGraphSnapshot) -> Vec<usize> {
    let in_degree = incoming_edge_counts(snapshot);

    (0..snapshot.states.len())
        .filter(|&index| {
            snapshot.initial_indices.contains(&index)
                || snapshot.deadlocks.contains(&index)
                || snapshot.focus_indices.contains(&index)
                || snapshot.edges[index]
                    .iter()
                    .any(|edge| edge.target == index)
                || in_degree[index] != 1
                || snapshot.edges[index].len() != 1
        })
        .collect()
}

fn incoming_edge_counts(snapshot: &DocGraphSnapshot) -> Vec<usize> {
    let mut counts = vec![0; snapshot.states.len()];
    for outgoing in &snapshot.edges {
        for edge in outgoing {
            if let Some(count) = counts.get_mut(edge.target) {
                *count += 1;
            }
        }
    }
    counts
}

fn collapse_edge_path(
    snapshot: &DocGraphSnapshot,
    keep: &[bool],
    source: usize,
    first_edge: &DocGraphEdge,
) -> ReducedDocGraphEdge {
    let mut labels = vec![first_edge.label.as_str()];
    let mut collapsed_state_indices = Vec::new();
    let mut current = first_edge.target;
    let mut visited = BTreeSet::new();

    while !keep[current] && visited.insert(current) {
        collapsed_state_indices.push(current);
        let outgoing = &snapshot.edges[current];
        if outgoing.len() != 1 {
            break;
        }
        let next_edge = &outgoing[0];
        labels.push(next_edge.label.as_str());
        current = next_edge.target;
    }

    ReducedDocGraphEdge {
        source,
        target: current,
        label: summarize_reduced_edge_labels(&labels, snapshot.max_edge_actions_in_label),
        collapsed_state_indices,
    }
}

fn coalesce_reduced_edges(
    edges: Vec<ReducedDocGraphEdge>,
    max_edge_actions_in_label: usize,
) -> Vec<ReducedDocGraphEdge> {
    let mut groups = BTreeMap::<(usize, usize, Vec<usize>), Vec<String>>::new();
    for edge in edges {
        groups
            .entry((edge.source, edge.target, edge.collapsed_state_indices))
            .or_default()
            .push(edge.label);
    }

    groups
        .into_iter()
        .map(
            |((source, target, collapsed_state_indices), labels)| ReducedDocGraphEdge {
                source,
                target,
                label: summarize_parallel_edge_labels(&labels, max_edge_actions_in_label),
                collapsed_state_indices,
            },
        )
        .collect()
}

fn summarize_reduced_edge_labels(labels: &[&str], max_edge_actions_in_label: usize) -> String {
    let summarized = labels
        .iter()
        .map(|label| summarize_single_edge_action_label(label))
        .collect::<Vec<_>>();

    match summarized.len() {
        0 => String::new(),
        1 => summarized[0].clone(),
        len if len <= max_edge_actions_in_label => summarized.join(" -> "),
        len => format!(
            "{} -> ... -> {} ({len} steps)",
            summarized.first().expect("first"),
            summarized.last().expect("last")
        ),
    }
}

fn summarize_parallel_edge_labels(labels: &[String], max_edge_actions_in_label: usize) -> String {
    let mut unique = BTreeSet::new();
    let summarized = labels
        .iter()
        .filter(|label| unique.insert((*label).clone()))
        .cloned()
        .collect::<Vec<_>>();

    match summarized.len() {
        0 => String::new(),
        1 => summarized[0].clone(),
        len if len <= max_edge_actions_in_label => summarized.join(" | "),
        len => format!(
            "{} | ... | {} ({len} actions)",
            summarized.first().expect("first"),
            summarized.last().expect("last")
        ),
    }
}

fn summarize_single_edge_action_label(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Some(index) = trimmed.find('{') {
        return format!("{}{{...}}", trimmed[..index].trim_end());
    }
    if let Some(index) = trimmed.find('(') {
        return format!("{}(...)", trimmed[..index].trim_end());
    }
    if let Some(index) = trimmed.find('[') {
        return format!("{}[...]", trimmed[..index].trim_end());
    }

    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::{Any, TypeId};

    use crate::{
        BoundedDomain, RegisteredActionDocLabel, RegisteredRelationalState, RelAtom, RelSet,
        Relation2, RelationField, RelationalState, Signature,
    };

    #[test]
    fn summarize_doc_graph_state_preserves_full_and_shortens_summary() {
        #[derive(Debug)]
        struct DemoState {
            phase: &'static str,
            counter: u32,
            notes: &'static str,
        }

        let state = DemoState {
            phase: "Listening",
            counter: 7,
            notes: "A long note that should still remain visible in the full debug representation",
        };
        let _ = (&state.phase, state.counter, &state.notes);
        let summarized = summarize_doc_graph_state(&state);

        assert!(summarized.full.contains("phase: \"Listening\""));
        assert!(summarized.summary.contains("phase: \"Listening\""));
        assert!(summarized.summary.lines().count() <= 8);
        assert!(summarized.summary.len() <= 243);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum DemoAtom {
        Root,
        Dependency,
    }

    impl Signature for DemoAtom {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Root, Self::Dependency])
        }
    }

    impl RelAtom for DemoAtom {
        fn rel_index(&self) -> usize {
            match self {
                Self::Root => 0,
                Self::Dependency => 1,
            }
        }

        fn rel_from_index(index: usize) -> Option<Self> {
            match index {
                0 => Some(Self::Root),
                1 => Some(Self::Dependency),
                _ => None,
            }
        }
    }

    #[derive(Debug)]
    struct RelationalDemoState {
        requires: Relation2<DemoAtom, DemoAtom>,
        allowed: RelSet<DemoAtom>,
    }

    impl RelationalState for RelationalDemoState {
        fn relation_schema() -> Vec<crate::RelationFieldSchema> {
            vec![
                <Relation2<DemoAtom, DemoAtom> as RelationField>::relation_schema("requires"),
                <RelSet<DemoAtom> as RelationField>::relation_schema("allowed"),
            ]
        }

        fn relation_summary(&self) -> Vec<crate::RelationFieldSummary> {
            vec![
                self.requires.relation_summary("requires"),
                self.allowed.relation_summary("allowed"),
            ]
        }
    }

    fn relational_demo_type_id() -> TypeId {
        TypeId::of::<RelationalDemoState>()
    }

    fn relational_demo_schema() -> Vec<crate::RelationFieldSchema> {
        <RelationalDemoState as RelationalState>::relation_schema()
    }

    fn relational_demo_summary(value: &dyn Any) -> Vec<crate::RelationFieldSummary> {
        value
            .downcast_ref::<RelationalDemoState>()
            .expect("registered relational state downcast")
            .relation_summary()
    }

    inventory::submit! {
        RegisteredRelationalState {
            state_type_id: relational_demo_type_id,
            relation_schema: relational_demo_schema,
            relation_summary: relational_demo_summary,
        }
    }

    #[derive(Debug)]
    enum NestedDocAction {
        Inner,
    }

    fn nested_doc_action_type_id() -> TypeId {
        TypeId::of::<NestedDocAction>()
    }

    fn nested_doc_action_format(value: &dyn Any) -> Option<String> {
        let value = value
            .downcast_ref::<NestedDocAction>()
            .expect("registered action doc downcast");
        match value {
            NestedDocAction::Inner => Some("inner action doc".to_owned()),
        }
    }

    inventory::submit! {
        RegisteredActionDocLabel {
            value_type_id: nested_doc_action_type_id,
            format: nested_doc_action_format,
        }
    }

    #[derive(Debug)]
    enum WrapperDocAction {
        Direct,
        Delegated(NestedDocAction),
        Missing,
    }

    fn wrapper_doc_action_type_id() -> TypeId {
        TypeId::of::<WrapperDocAction>()
    }

    fn wrapper_doc_action_format(value: &dyn Any) -> Option<String> {
        let value = value
            .downcast_ref::<WrapperDocAction>()
            .expect("registered action doc downcast");
        match value {
            WrapperDocAction::Direct => Some("direct action doc".to_owned()),
            WrapperDocAction::Delegated(inner) => Some(format_doc_graph_action(inner)),
            WrapperDocAction::Missing => None,
        }
    }

    inventory::submit! {
        RegisteredActionDocLabel {
            value_type_id: wrapper_doc_action_type_id,
            format: wrapper_doc_action_format,
        }
    }

    #[test]
    fn summarize_doc_graph_state_captures_relation_schema_and_notation() {
        let state = RelationalDemoState {
            requires: Relation2::from_pairs([(DemoAtom::Root, DemoAtom::Dependency)]),
            allowed: RelSet::from_items([DemoAtom::Root]),
        };

        let summarized = summarize_doc_graph_state(&state);

        assert!(summarized.summary.contains("requires = Root->Dependency"));
        assert_eq!(summarized.relation_fields.len(), 2);
        assert_eq!(summarized.relation_schema.len(), 2);
        assert_eq!(summarized.relation_schema[0].name, "requires");
        assert_eq!(summarized.relation_schema[1].name, "allowed");
    }

    #[test]
    fn format_doc_graph_action_prefers_registered_doc_and_delegates_single_field_wrappers() {
        assert_eq!(
            format_doc_graph_action(&WrapperDocAction::Direct),
            "direct action doc"
        );
        assert_eq!(
            format_doc_graph_action(&WrapperDocAction::Delegated(NestedDocAction::Inner)),
            "inner action doc"
        );
        assert_eq!(
            format_doc_graph_action(&WrapperDocAction::Missing),
            "Missing"
        );
    }

    #[test]
    fn summarize_doc_graph_text_truncates_large_payloads() {
        let text = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        let summarized = summarize_doc_graph_text(text);

        assert!(summarized.contains("line1"));
        assert!(summarized.contains("line8"));
        assert!(!summarized.contains("line10"));
        assert!(summarized.ends_with("..."));
    }

    fn demo_state(name: &str) -> DocGraphState {
        DocGraphState {
            summary: name.to_owned(),
            full: name.to_owned(),
            relation_fields: Vec::new(),
            relation_schema: Vec::new(),
        }
    }

    fn demo_snapshot() -> DocGraphSnapshot {
        DocGraphSnapshot {
            states: vec![
                demo_state("S0"),
                demo_state("S1"),
                demo_state("S2"),
                demo_state("S3"),
                demo_state("S4"),
            ],
            edges: vec![
                vec![DocGraphEdge {
                    label: "A".to_owned(),
                    target: 1,
                }],
                vec![DocGraphEdge {
                    label: "B".to_owned(),
                    target: 2,
                }],
                vec![
                    DocGraphEdge {
                        label: "C".to_owned(),
                        target: 3,
                    },
                    DocGraphEdge {
                        label: "D".to_owned(),
                        target: 4,
                    },
                ],
                Vec::new(),
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![3, 4],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        }
    }

    #[test]
    fn boundary_path_reduction_collapses_linear_chain_into_one_edge() {
        let reduced = reduce_doc_graph(&demo_snapshot());

        assert_eq!(
            reduced
                .states
                .iter()
                .map(|state| state.original_index)
                .collect::<Vec<_>>(),
            vec![0, 2, 3, 4]
        );
        assert!(
            reduced
                .edges
                .iter()
                .any(|edge| edge.source == 0 && edge.target == 2 && edge.label == "A -> B")
        );
    }

    #[test]
    fn boundary_path_reduction_preserves_branches_and_deadlocks() {
        let reduced = reduce_doc_graph(&demo_snapshot());

        assert!(
            reduced
                .edges
                .iter()
                .any(|edge| edge.source == 2 && edge.target == 3 && edge.label == "C")
        );
        assert!(
            reduced
                .edges
                .iter()
                .any(|edge| edge.source == 2 && edge.target == 4 && edge.label == "D")
        );
        assert!(reduced.states.iter().any(|state| state.is_deadlock));
    }

    #[test]
    fn boundary_path_reduction_preserves_self_loops_and_focus_states() {
        let snapshot = DocGraphSnapshot {
            states: vec![demo_state("S0"), demo_state("S1"), demo_state("S2")],
            edges: vec![
                vec![DocGraphEdge {
                    label: "Advance".to_owned(),
                    target: 1,
                }],
                vec![DocGraphEdge {
                    label: "Loop".to_owned(),
                    target: 1,
                }],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![2],
            truncated: false,
            stutter_omitted: false,
            focus_indices: vec![1],
            reduction: DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        };

        let reduced = reduce_doc_graph(&snapshot);

        assert!(reduced.states.iter().any(|state| state.original_index == 1));
        assert!(
            reduced
                .edges
                .iter()
                .any(|edge| edge.source == 1 && edge.target == 1 && edge.label == "Loop")
        );
    }

    #[test]
    fn boundary_path_reduction_summarizes_long_edge_paths() {
        let snapshot = DocGraphSnapshot {
            states: vec![
                demo_state("S0"),
                demo_state("S1"),
                demo_state("S2"),
                demo_state("S3"),
            ],
            edges: vec![
                vec![DocGraphEdge {
                    label: "A".to_owned(),
                    target: 1,
                }],
                vec![DocGraphEdge {
                    label: "B".to_owned(),
                    target: 2,
                }],
                vec![DocGraphEdge {
                    label: "C".to_owned(),
                    target: 3,
                }],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![3],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        };

        let reduced = reduce_doc_graph(&snapshot);

        assert!(reduced.edges.iter().any(|edge| edge.source == 0
            && edge.target == 3
            && edge.label == "A -> ... -> C (3 steps)"));
    }

    #[test]
    fn boundary_path_reduction_coalesces_parallel_edges() {
        let snapshot = DocGraphSnapshot {
            states: vec![demo_state("S0"), demo_state("S1")],
            edges: vec![
                vec![
                    DocGraphEdge {
                        label: "Start(CommandKind::Deploy)".to_owned(),
                        target: 1,
                    },
                    DocGraphEdge {
                        label: "SetRunning".to_owned(),
                        target: 1,
                    },
                    DocGraphEdge {
                        label: "RequestCancel".to_owned(),
                        target: 1,
                    },
                ],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![1],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        };

        let reduced = reduce_doc_graph(&snapshot);

        assert_eq!(reduced.edges.len(), 1);
        assert_eq!(
            reduced.edges[0].label,
            "Start(...) | ... | RequestCancel (3 actions)"
        );
    }
}
