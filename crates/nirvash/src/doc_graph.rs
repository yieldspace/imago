use std::{collections::BTreeMap, collections::BTreeSet, fmt::Debug};

use serde::{Deserialize, Serialize};

use crate::{
    BoolExpr, ModelBackend, RelationFieldSchema, RelationFieldSummary, SoundnessTier,
    collect_relational_state_schema, collect_relational_state_summary,
    registry::{lookup_action_doc_label, lookup_action_doc_presentation},
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
    pub soundness_tier: SoundnessTier,
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
    pub compact_label: Option<String>,
    pub scenario_priority: Option<i32>,
    pub interaction_steps: Vec<DocGraphInteractionStep>,
    pub process_steps: Vec<DocGraphProcessStep>,
    pub target: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphInteractionStep {
    pub from: Option<String>,
    pub to: Option<String>,
    pub label: String,
}

impl DocGraphInteractionStep {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            from: None,
            to: None,
            label: label.into(),
        }
    }

    pub fn between(
        from: impl Into<String>,
        to: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            from: Some(from.into()),
            to: Some(to.into()),
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocGraphProcessKind {
    Do,
    Send,
    Receive,
    Wait,
    Emit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphProcessStep {
    pub actor: Option<String>,
    pub kind: DocGraphProcessKind,
    pub label: String,
}

impl DocGraphProcessStep {
    pub fn new(kind: DocGraphProcessKind, label: impl Into<String>) -> Self {
        Self {
            actor: None,
            kind,
            label: label.into(),
        }
    }

    pub fn for_actor(
        actor: impl Into<String>,
        kind: DocGraphProcessKind,
        label: impl Into<String>,
    ) -> Self {
        Self {
            actor: Some(actor.into()),
            kind,
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGraphActionPresentation {
    pub label: String,
    pub compact_label: Option<String>,
    pub scenario_priority: Option<i32>,
    pub interaction_steps: Vec<DocGraphInteractionStep>,
    pub process_steps: Vec<DocGraphProcessStep>,
}

impl DocGraphActionPresentation {
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        Self::with_steps(
            label.clone(),
            Vec::new(),
            vec![DocGraphProcessStep::new(DocGraphProcessKind::Do, label)],
        )
    }

    pub fn with_steps(
        label: impl Into<String>,
        interaction_steps: Vec<DocGraphInteractionStep>,
        process_steps: Vec<DocGraphProcessStep>,
    ) -> Self {
        let label = label.into();
        let process_steps = if process_steps.is_empty() {
            vec![DocGraphProcessStep::new(
                DocGraphProcessKind::Do,
                label.clone(),
            )]
        } else {
            process_steps
        };
        Self {
            label,
            compact_label: None,
            scenario_priority: None,
            interaction_steps,
            process_steps,
        }
    }

    pub fn with_compact_label(mut self, compact_label: impl Into<String>) -> Self {
        let compact_label = compact_label.into();
        if !compact_label.trim().is_empty() {
            self.compact_label = Some(compact_label);
        }
        self
    }

    pub fn with_scenario_priority(mut self, scenario_priority: i32) -> Self {
        self.scenario_priority = Some(scenario_priority);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocGraphReductionMode {
    Full,
    BoundaryPaths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SpecVizKind {
    Subsystem,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VizPolicy {
    pub max_inline_states: usize,
    pub max_scenarios: usize,
    pub focus_path_limit: usize,
    pub large_graph_threshold: usize,
}

impl Default for VizPolicy {
    fn default() -> Self {
        Self {
            max_inline_states: 50,
            max_scenarios: 4,
            focus_path_limit: 24,
            large_graph_threshold: 50,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVizRegistrationSet {
    pub invariants: Vec<String>,
    pub properties: Vec<String>,
    pub fairness: Vec<String>,
    pub state_constraints: Vec<String>,
    pub action_constraints: Vec<String>,
    pub symmetries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpecVizMetadata {
    pub spec_id: String,
    pub kind: Option<SpecVizKind>,
    pub state_ty: String,
    pub action_ty: String,
    pub model_cases: Option<String>,
    pub subsystems: Vec<SpecVizSubsystem>,
    pub registrations: SpecVizRegistrationSet,
    pub policy: VizPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegisteredSubsystemSpec {
    pub spec_id: &'static str,
    pub label: &'static str,
}

impl RegisteredSubsystemSpec {
    pub const fn new(spec_id: &'static str, label: &'static str) -> Self {
        Self { spec_id, label }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SpecVizSubsystem {
    pub spec_id: String,
    pub label: String,
}

impl SpecVizSubsystem {
    pub fn new(spec_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            spec_id: spec_id.into(),
            label: label.into(),
        }
    }

    pub fn from_registered(value: RegisteredSubsystemSpec) -> Self {
        Self::new(value.spec_id, value.label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVizActionDescriptor {
    pub label: String,
    pub compact_label: Option<String>,
    pub scenario_priority: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VizScenarioKind {
    DeadlockPath,
    FocusPath,
    CycleWitness,
    HappyPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VizScenarioStep {
    pub source: usize,
    pub target: usize,
    pub label: String,
    pub compact_label: Option<String>,
    pub scenario_priority: Option<i32>,
    pub interaction_steps: Vec<DocGraphInteractionStep>,
    pub process_steps: Vec<DocGraphProcessStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VizScenario {
    pub label: String,
    pub kind: VizScenarioKind,
    pub state_path: Vec<usize>,
    pub steps: Vec<VizScenarioStep>,
    pub actors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVizCaseStats {
    pub full_state_count: usize,
    pub full_edge_count: usize,
    pub reduced_state_count: usize,
    pub reduced_edge_count: usize,
    pub focus_state_count: usize,
    pub deadlock_count: usize,
    pub truncated: bool,
    pub stutter_omitted: bool,
    pub large_graph_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVizCase {
    pub label: String,
    pub backend: ModelBackend,
    pub graph: DocGraphSnapshot,
    pub reduced_graph: ReducedDocGraph,
    pub focus_graph: Option<ReducedDocGraph>,
    pub scenarios: Vec<VizScenario>,
    pub actors: Vec<String>,
    pub loop_groups: Vec<Vec<usize>>,
    pub stats: SpecVizCaseStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecVizBundle {
    pub spec_name: String,
    pub metadata: SpecVizMetadata,
    pub action_vocabulary: Vec<SpecVizActionDescriptor>,
    pub relation_schema: Vec<RelationFieldSchema>,
    pub cases: Vec<SpecVizCase>,
}

#[derive(Debug, Clone)]
pub struct DocGraphPolicy<S> {
    pub reduction: DocGraphReductionMode,
    pub focus_states: Vec<BoolExpr<S>>,
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

    pub fn with_focus_state(mut self, predicate: BoolExpr<S>) -> Self {
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
    pub backend: ModelBackend,
    pub soundness_tier: SoundnessTier,
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

pub trait SpecVizProvider {
    fn spec_name(&self) -> &'static str;

    fn bundle(&self) -> SpecVizBundle;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpecVizProviderKind {
    MetadataOnly,
    RuntimeGraph,
}

#[derive(Clone, Copy)]
pub struct RegisteredSpecVizProvider {
    pub spec_name: &'static str,
    pub build: fn() -> Box<dyn SpecVizProvider>,
    pub kind: SpecVizProviderKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedDocGraphNode {
    pub original_index: usize,
    pub state: DocGraphState,
    pub is_initial: bool,
    pub is_deadlock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedDocGraphEdge {
    pub source: usize,
    pub target: usize,
    pub label: String,
    pub collapsed_state_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedDocGraph {
    pub states: Vec<ReducedDocGraphNode>,
    pub edges: Vec<ReducedDocGraphEdge>,
    pub truncated: bool,
    pub stutter_omitted: bool,
}

inventory::collect!(RegisteredDocGraphProvider);
inventory::collect!(RegisteredSpecVizProvider);

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

pub fn describe_doc_graph_action<T>(value: &T) -> DocGraphActionPresentation
where
    T: Debug + 'static,
{
    if let Some(presentation) = lookup_action_doc_presentation(value as &dyn std::any::Any) {
        return presentation;
    }

    if let Some(label) = lookup_action_doc_label(value as &dyn std::any::Any) {
        return DocGraphActionPresentation::new(label);
    }

    DocGraphActionPresentation::new(format!("{value:?}"))
}

pub fn format_doc_graph_action<T>(value: &T) -> String
where
    T: Debug + 'static,
{
    describe_doc_graph_action(value).label
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

pub fn visit_spec_viz_bundles(mut visitor: impl FnMut(SpecVizBundle)) {
    for entry in inventory::iter::<RegisteredSpecVizProvider> {
        let provider = (entry.build)();
        visitor(provider.bundle());
    }
}

pub fn collect_spec_viz_provider_registrations() -> Vec<RegisteredSpecVizProvider> {
    inventory::iter::<RegisteredSpecVizProvider>
        .into_iter()
        .copied()
        .collect()
}

pub fn collect_primary_spec_viz_provider_registrations() -> Vec<RegisteredSpecVizProvider> {
    let mut registrations_by_name = BTreeMap::<&'static str, RegisteredSpecVizProvider>::new();
    for registration in collect_spec_viz_provider_registrations() {
        match registrations_by_name.get_mut(registration.spec_name) {
            Some(existing) if registration.kind > existing.kind => *existing = registration,
            Some(_) => {}
            None => {
                registrations_by_name.insert(registration.spec_name, registration);
            }
        }
    }
    registrations_by_name.into_values().collect()
}

pub fn collect_spec_viz_bundles() -> Vec<SpecVizBundle> {
    let mut bundles_by_name = BTreeMap::<String, SpecVizBundle>::new();
    visit_spec_viz_bundles(|candidate| upsert_spec_viz_bundle(&mut bundles_by_name, candidate));
    let mut bundles = bundles_by_name.into_values().collect::<Vec<_>>();
    bundles.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));
    bundles
}

pub fn upsert_spec_viz_bundle(
    bundles_by_name: &mut BTreeMap<String, SpecVizBundle>,
    candidate: SpecVizBundle,
) {
    if let Some(existing) = bundles_by_name.get_mut(&candidate.spec_name) {
        merge_spec_viz_bundle(existing, candidate);
    } else {
        bundles_by_name.insert(candidate.spec_name.clone(), candidate);
    }
}

pub fn merge_spec_viz_bundle(target: &mut SpecVizBundle, candidate: SpecVizBundle) {
    if target.cases.len() < candidate.cases.len() {
        target.cases = candidate.cases;
    }
    if target.metadata.spec_id.is_empty() {
        target.metadata.spec_id = candidate.metadata.spec_id.clone();
    }
    if target.metadata.kind.is_none() {
        target.metadata.kind = candidate.metadata.kind;
    }
    if target.metadata.model_cases.is_none() {
        target.metadata.model_cases = candidate.metadata.model_cases;
    }
    if target.metadata.subsystems.is_empty() {
        target.metadata.subsystems = candidate.metadata.subsystems;
    }
    if target.metadata.registrations.invariants.is_empty() {
        target.metadata.registrations.invariants = candidate.metadata.registrations.invariants;
    }
    if target.metadata.registrations.properties.is_empty() {
        target.metadata.registrations.properties = candidate.metadata.registrations.properties;
    }
    if target.metadata.registrations.fairness.is_empty() {
        target.metadata.registrations.fairness = candidate.metadata.registrations.fairness;
    }
    if target.metadata.registrations.state_constraints.is_empty() {
        target.metadata.registrations.state_constraints =
            candidate.metadata.registrations.state_constraints;
    }
    if target.metadata.registrations.action_constraints.is_empty() {
        target.metadata.registrations.action_constraints =
            candidate.metadata.registrations.action_constraints;
    }
    if target.metadata.registrations.symmetries.is_empty() {
        target.metadata.registrations.symmetries = candidate.metadata.registrations.symmetries;
    }
}

pub fn reduce_doc_graph(snapshot: &DocGraphSnapshot) -> ReducedDocGraph {
    match snapshot.reduction {
        DocGraphReductionMode::Full => full_doc_graph(snapshot),
        DocGraphReductionMode::BoundaryPaths => boundary_path_reduced_graph(snapshot),
    }
}

impl SpecVizBundle {
    pub fn from_doc_graph_spec(
        spec_name: impl Into<String>,
        metadata: SpecVizMetadata,
        cases: Vec<DocGraphCase>,
    ) -> Self {
        let mut relation_schema = Vec::new();
        let mut action_vocabulary = Vec::new();
        let cases = cases
            .into_iter()
            .map(|case| {
                let viz_case = SpecVizCase::from_doc_graph_case(&metadata.policy, case);
                for state in &viz_case.graph.states {
                    for field in &state.relation_schema {
                        if !relation_schema.contains(field) {
                            relation_schema.push(field.clone());
                        }
                    }
                }
                for outgoing in &viz_case.graph.edges {
                    for edge in outgoing {
                        let descriptor = SpecVizActionDescriptor {
                            label: edge.label.clone(),
                            compact_label: edge.compact_label.clone(),
                            scenario_priority: edge.scenario_priority,
                        };
                        if !action_vocabulary.contains(&descriptor) {
                            action_vocabulary.push(descriptor);
                        }
                    }
                }
                viz_case
            })
            .collect();

        Self {
            spec_name: spec_name.into(),
            metadata,
            action_vocabulary,
            relation_schema,
            cases,
        }
    }
}

impl SpecVizCase {
    fn from_doc_graph_case(policy: &VizPolicy, case: DocGraphCase) -> Self {
        let DocGraphCase {
            label,
            backend,
            soundness_tier: _,
            graph,
        } = case;
        let reduced_graph = reduce_doc_graph(&graph);
        let loop_groups = collect_loop_groups(&graph);
        let scenarios = select_viz_scenarios(&graph, policy);
        let focus_graph = (reduced_graph.states.len() > policy.large_graph_threshold)
            .then(|| build_focus_graph(&graph, &scenarios))
            .flatten();
        let actors = collect_case_actors(&graph);
        let full_edge_count = graph.edges.iter().map(Vec::len).sum();
        let reduced_edge_count = reduced_graph.edges.len();
        let focus_state_count = focus_graph
            .as_ref()
            .map(|graph| graph.states.len())
            .unwrap_or(reduced_graph.states.len());
        let large_graph_fallback = reduced_graph.states.len() > policy.large_graph_threshold;

        Self {
            label,
            backend,
            graph,
            reduced_graph,
            focus_graph,
            scenarios,
            actors,
            loop_groups,
            stats: SpecVizCaseStats {
                full_state_count: 0,
                full_edge_count,
                reduced_state_count: 0,
                reduced_edge_count,
                focus_state_count,
                deadlock_count: 0,
                truncated: false,
                stutter_omitted: false,
                large_graph_fallback,
            },
        }
        .with_graph_stats()
    }

    fn with_graph_stats(mut self) -> Self {
        self.stats.full_state_count = self.graph.states.len();
        self.stats.reduced_state_count = self.reduced_graph.states.len();
        self.stats.deadlock_count = self.graph.deadlocks.len();
        self.stats.truncated = self.graph.truncated;
        self.stats.stutter_omitted = self.graph.stutter_omitted;
        self
    }
}

fn collect_case_actors(graph: &DocGraphSnapshot) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for outgoing in &graph.edges {
        for edge in outgoing {
            for actor in edge
                .interaction_steps
                .iter()
                .flat_map(|step| step.from.iter().chain(step.to.iter()))
            {
                if seen.insert(actor.clone()) {
                    ordered.push(actor.clone());
                }
            }
            for actor in edge
                .process_steps
                .iter()
                .filter_map(|step| step.actor.as_ref())
            {
                if seen.insert(actor.clone()) {
                    ordered.push(actor.clone());
                }
            }
        }
    }
    ordered
}

fn select_viz_scenarios(graph: &DocGraphSnapshot, policy: &VizPolicy) -> Vec<VizScenario> {
    let mut scenarios = Vec::new();
    let mut seen_paths = BTreeSet::new();

    if let Some(path) = shortest_path_to_any(graph, &graph.deadlocks) {
        push_viz_scenario(
            &mut scenarios,
            &mut seen_paths,
            graph,
            VizScenarioKind::DeadlockPath,
            "deadlock shortest path",
            path,
        );
    }

    let focus_targets = graph
        .focus_indices
        .iter()
        .copied()
        .filter(|index| !graph.deadlocks.contains(index))
        .collect::<Vec<_>>();
    if let Some(path) = shortest_path_to_any(graph, &focus_targets) {
        push_viz_scenario(
            &mut scenarios,
            &mut seen_paths,
            graph,
            VizScenarioKind::FocusPath,
            "focus predicate shortest path",
            path,
        );
    }

    if let Some(path) = cycle_witness_path(graph) {
        push_viz_scenario(
            &mut scenarios,
            &mut seen_paths,
            graph,
            VizScenarioKind::CycleWitness,
            "cycle witness",
            path,
        );
    }

    if let Some(path) = happy_path(graph) {
        push_viz_scenario(
            &mut scenarios,
            &mut seen_paths,
            graph,
            VizScenarioKind::HappyPath,
            "happy path",
            path,
        );
    }

    scenarios.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then(left.label.cmp(&right.label))
            .then(left.state_path.cmp(&right.state_path))
    });
    scenarios.truncate(policy.max_scenarios);
    scenarios
}

fn push_viz_scenario(
    scenarios: &mut Vec<VizScenario>,
    seen_paths: &mut BTreeSet<Vec<usize>>,
    graph: &DocGraphSnapshot,
    kind: VizScenarioKind,
    label: &'static str,
    state_path: Vec<usize>,
) {
    if state_path.len() < 2 || !seen_paths.insert(state_path.clone()) {
        return;
    }

    let mut steps = Vec::new();
    let mut actors = BTreeSet::new();
    for window in state_path.windows(2) {
        let Some(edge) = graph.edges[window[0]]
            .iter()
            .find(|edge| edge.target == window[1])
        else {
            return;
        };
        for actor in edge
            .interaction_steps
            .iter()
            .flat_map(|step| step.from.iter().chain(step.to.iter()))
        {
            actors.insert(actor.clone());
        }
        steps.push(VizScenarioStep {
            source: window[0],
            target: window[1],
            label: edge.label.clone(),
            compact_label: edge.compact_label.clone(),
            scenario_priority: edge.scenario_priority,
            interaction_steps: edge.interaction_steps.clone(),
            process_steps: edge.process_steps.clone(),
        });
    }

    scenarios.push(VizScenario {
        label: label.to_owned(),
        kind,
        state_path,
        steps,
        actors: actors.into_iter().collect(),
    });
}

fn shortest_path_to_any(graph: &DocGraphSnapshot, targets: &[usize]) -> Option<Vec<usize>> {
    let target_set = targets.iter().copied().collect::<BTreeSet<_>>();
    if target_set.is_empty() {
        return None;
    }

    let mut queue = std::collections::VecDeque::new();
    let mut parent = BTreeMap::new();
    let mut visited = BTreeSet::new();

    let mut initials = graph.initial_indices.clone();
    initials.sort_unstable();
    for initial in initials {
        visited.insert(initial);
        queue.push_back(initial);
    }

    while let Some(state) = queue.pop_front() {
        if target_set.contains(&state) {
            return Some(reconstruct_state_path(state, &parent));
        }

        let mut outgoing = graph.edges[state]
            .iter()
            .map(|edge| {
                (
                    edge.scenario_priority.unwrap_or_default(),
                    edge.target,
                    edge.label.as_str(),
                )
            })
            .collect::<Vec<_>>();
        outgoing.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(left.2.cmp(right.2))
                .then(left.1.cmp(&right.1))
        });

        for (_, target, _) in outgoing {
            if visited.insert(target) {
                parent.insert(target, state);
                queue.push_back(target);
            }
        }
    }

    None
}

fn reconstruct_state_path(mut current: usize, parent: &BTreeMap<usize, usize>) -> Vec<usize> {
    let mut path = vec![current];
    while let Some(prev) = parent.get(&current).copied() {
        path.push(prev);
        current = prev;
    }
    path.reverse();
    path
}

fn happy_path(graph: &DocGraphSnapshot) -> Option<Vec<usize>> {
    let terminals = graph
        .edges
        .iter()
        .enumerate()
        .filter_map(|(index, outgoing)| {
            (outgoing.is_empty() && !graph.deadlocks.contains(&index)).then_some(index)
        })
        .collect::<Vec<_>>();
    if !terminals.is_empty() {
        return shortest_path_to_any(graph, &terminals);
    }

    let mut candidates = (0..graph.states.len())
        .map(|index| shortest_path_to_any(graph, &[index]))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .as_ref()
            .map(Vec::len)
            .cmp(&left.as_ref().map(Vec::len))
            .then(left.cmp(right))
    });
    candidates.into_iter().flatten().next()
}

fn cycle_witness_path(graph: &DocGraphSnapshot) -> Option<Vec<usize>> {
    let loop_groups = collect_loop_groups(graph);
    let group = loop_groups.into_iter().next()?;
    let entry = *group.iter().min()?;
    let entry_path = shortest_path_to_any(graph, &[entry])?;
    if graph.edges[entry].iter().any(|edge| edge.target == entry) {
        let mut path = entry_path;
        path.push(entry);
        return Some(path);
    }

    let group_set = group.into_iter().collect::<BTreeSet<_>>();
    for edge in &graph.edges[entry] {
        if !group_set.contains(&edge.target) {
            continue;
        }
        if let Some(mut suffix) = shortest_path_within(graph, edge.target, entry, &group_set) {
            let mut path = entry_path.clone();
            path.push(edge.target);
            if !suffix.is_empty() {
                suffix.remove(0);
            }
            path.extend(suffix);
            return Some(path);
        }
    }
    None
}

fn shortest_path_within(
    graph: &DocGraphSnapshot,
    start: usize,
    target: usize,
    allowed: &BTreeSet<usize>,
) -> Option<Vec<usize>> {
    let mut queue = std::collections::VecDeque::from([start]);
    let mut parent = BTreeMap::new();
    let mut visited = BTreeSet::from([start]);

    while let Some(state) = queue.pop_front() {
        if state == target {
            return Some(reconstruct_state_path(state, &parent));
        }
        let mut outgoing = graph.edges[state]
            .iter()
            .filter(|edge| allowed.contains(&edge.target))
            .map(|edge| (edge.target, edge.label.as_str()))
            .collect::<Vec<_>>();
        outgoing.sort_by(|left, right| left.1.cmp(right.1).then(left.0.cmp(&right.0)));
        for (next, _) in outgoing {
            if visited.insert(next) {
                parent.insert(next, state);
                queue.push_back(next);
            }
        }
    }

    None
}

fn build_focus_graph(
    graph: &DocGraphSnapshot,
    scenarios: &[VizScenario],
) -> Option<ReducedDocGraph> {
    let mut nodes = BTreeSet::new();
    let mut edges = Vec::new();
    for scenario in scenarios {
        for state in &scenario.state_path {
            nodes.insert(*state);
        }
        for step in &scenario.steps {
            edges.push((step.source, step.target, step.label.clone()));
        }
    }

    if nodes.is_empty() {
        return None;
    }

    let states = nodes
        .iter()
        .copied()
        .map(|index| ReducedDocGraphNode {
            original_index: index,
            state: graph.states[index].clone(),
            is_initial: graph.initial_indices.contains(&index),
            is_deadlock: graph.deadlocks.contains(&index),
        })
        .collect::<Vec<_>>();
    let edges = edges
        .into_iter()
        .map(|(source, target, label)| ReducedDocGraphEdge {
            source,
            target,
            label,
            collapsed_state_indices: Vec::new(),
        })
        .collect();

    Some(ReducedDocGraph {
        states,
        edges,
        truncated: graph.truncated,
        stutter_omitted: graph.stutter_omitted,
    })
}

fn collect_loop_groups(graph: &DocGraphSnapshot) -> Vec<Vec<usize>> {
    struct Tarjan<'a> {
        graph: &'a DocGraphSnapshot,
        index: usize,
        stack: Vec<usize>,
        on_stack: BTreeSet<usize>,
        indices: BTreeMap<usize, usize>,
        lowlinks: BTreeMap<usize, usize>,
        groups: Vec<Vec<usize>>,
    }

    impl<'a> Tarjan<'a> {
        fn visit(&mut self, state: usize) {
            self.indices.insert(state, self.index);
            self.lowlinks.insert(state, self.index);
            self.index += 1;
            self.stack.push(state);
            self.on_stack.insert(state);

            for edge in &self.graph.edges[state] {
                if !self.indices.contains_key(&edge.target) {
                    self.visit(edge.target);
                    let next_low = *self.lowlinks.get(&edge.target).expect("target visited");
                    let low = self.lowlinks.get_mut(&state).expect("state lowlink exists");
                    *low = (*low).min(next_low);
                } else if self.on_stack.contains(&edge.target) {
                    let target_index =
                        *self.indices.get(&edge.target).expect("target index exists");
                    let low = self.lowlinks.get_mut(&state).expect("state lowlink exists");
                    *low = (*low).min(target_index);
                }
            }

            if self.indices.get(&state) == self.lowlinks.get(&state) {
                let mut group = Vec::new();
                while let Some(candidate) = self.stack.pop() {
                    self.on_stack.remove(&candidate);
                    group.push(candidate);
                    if candidate == state {
                        break;
                    }
                }
                let has_cycle = group.len() > 1
                    || group.iter().any(|member| {
                        self.graph.edges[*member]
                            .iter()
                            .any(|edge| edge.target == *member)
                    });
                if has_cycle {
                    group.sort_unstable();
                    self.groups.push(group);
                }
            }
        }
    }

    let mut tarjan = Tarjan {
        graph,
        index: 0,
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        indices: BTreeMap::new(),
        lowlinks: BTreeMap::new(),
        groups: Vec::new(),
    };
    for state in 0..graph.states.len() {
        if !tarjan.indices.contains_key(&state) {
            tarjan.visit(state);
        }
    }
    tarjan.groups.sort();
    tarjan.groups
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
        BoundedDomain, FiniteModelDomain, RegisteredActionDocLabel,
        RegisteredActionDocPresentation, RegisteredRelationalState, RelAtom, RelSet, Relation2,
        RelationField, RelationalState,
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

    impl FiniteModelDomain for DemoAtom {
        fn finite_domain() -> BoundedDomain<Self> {
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

    #[test]
    fn primary_provider_prefers_runtime_graph_registration() {
        fn metadata_build() -> Box<dyn SpecVizProvider> {
            struct MetadataOnlyProvider;
            impl SpecVizProvider for MetadataOnlyProvider {
                fn spec_name(&self) -> &'static str {
                    "DemoSpec"
                }

                fn bundle(&self) -> SpecVizBundle {
                    SpecVizBundle {
                        spec_name: "DemoSpec".to_owned(),
                        metadata: SpecVizMetadata::default(),
                        action_vocabulary: Vec::new(),
                        relation_schema: Vec::new(),
                        cases: Vec::new(),
                    }
                }
            }

            Box::new(MetadataOnlyProvider)
        }

        fn runtime_build() -> Box<dyn SpecVizProvider> {
            struct RuntimeProvider;
            impl SpecVizProvider for RuntimeProvider {
                fn spec_name(&self) -> &'static str {
                    "DemoSpec"
                }

                fn bundle(&self) -> SpecVizBundle {
                    SpecVizBundle {
                        spec_name: "DemoSpec".to_owned(),
                        metadata: SpecVizMetadata::default(),
                        action_vocabulary: Vec::new(),
                        relation_schema: Vec::new(),
                        cases: vec![SpecVizCase {
                            label: "default".to_owned(),
                            backend: ModelBackend::Explicit,
                            graph: DocGraphSnapshot {
                                states: Vec::new(),
                                edges: Vec::new(),
                                initial_indices: Vec::new(),
                                deadlocks: Vec::new(),
                                truncated: false,
                                stutter_omitted: false,
                                focus_indices: Vec::new(),
                                reduction: DocGraphReductionMode::BoundaryPaths,
                                max_edge_actions_in_label: 2,
                            },
                            reduced_graph: ReducedDocGraph {
                                states: Vec::new(),
                                edges: Vec::new(),
                                truncated: false,
                                stutter_omitted: false,
                            },
                            focus_graph: None,
                            scenarios: Vec::new(),
                            actors: Vec::new(),
                            loop_groups: Vec::new(),
                            stats: SpecVizCaseStats {
                                full_state_count: 0,
                                full_edge_count: 0,
                                reduced_state_count: 0,
                                reduced_edge_count: 0,
                                focus_state_count: 0,
                                large_graph_fallback: false,
                            },
                        }],
                    }
                }
            }

            Box::new(RuntimeProvider)
        }

        let mut registrations_by_name = BTreeMap::new();
        for registration in [
            RegisteredSpecVizProvider {
                spec_name: "DemoSpec",
                build: metadata_build,
                kind: SpecVizProviderKind::MetadataOnly,
            },
            RegisteredSpecVizProvider {
                spec_name: "DemoSpec",
                build: runtime_build,
                kind: SpecVizProviderKind::RuntimeGraph,
            },
        ] {
            match registrations_by_name.get_mut(registration.spec_name) {
                Some(existing) if registration.kind > existing.kind => *existing = registration,
                Some(_) => {}
                None => {
                    registrations_by_name.insert(registration.spec_name, registration);
                }
            }
        }

        let selected = registrations_by_name
            .remove("DemoSpec")
            .expect("primary registration");
        assert_eq!(selected.kind, SpecVizProviderKind::RuntimeGraph);
        assert_eq!(selected.build().bundle().cases.len(), 1);
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

    fn nested_doc_action_presentation(value: &dyn Any) -> Option<DocGraphActionPresentation> {
        nested_doc_action_format(value).map(DocGraphActionPresentation::new)
    }

    inventory::submit! {
        RegisteredActionDocLabel {
            value_type_id: nested_doc_action_type_id,
            format: nested_doc_action_format,
        }
    }

    inventory::submit! {
        RegisteredActionDocPresentation {
            value_type_id: nested_doc_action_type_id,
            format: nested_doc_action_presentation,
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

    fn wrapper_doc_action_presentation(value: &dyn Any) -> Option<DocGraphActionPresentation> {
        let value = value
            .downcast_ref::<WrapperDocAction>()
            .expect("registered action doc downcast");
        match value {
            WrapperDocAction::Direct => Some(DocGraphActionPresentation::with_steps(
                "direct action doc",
                vec![DocGraphInteractionStep::between(
                    "Wrapper",
                    "Observer",
                    "direct action doc",
                )],
                vec![DocGraphProcessStep::for_actor(
                    "Wrapper",
                    DocGraphProcessKind::Emit,
                    "direct action doc",
                )],
            )),
            WrapperDocAction::Delegated(inner) => Some(describe_doc_graph_action(inner)),
            WrapperDocAction::Missing => None,
        }
    }

    inventory::submit! {
        RegisteredActionDocLabel {
            value_type_id: wrapper_doc_action_type_id,
            format: wrapper_doc_action_format,
        }
    }

    inventory::submit! {
        RegisteredActionDocPresentation {
            value_type_id: wrapper_doc_action_type_id,
            format: wrapper_doc_action_presentation,
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

        let presentation = describe_doc_graph_action(&WrapperDocAction::Direct);
        assert_eq!(presentation.label, "direct action doc");
        assert_eq!(
            presentation.interaction_steps,
            vec![DocGraphInteractionStep::between(
                "Wrapper",
                "Observer",
                "direct action doc",
            )]
        );
        assert_eq!(
            presentation.process_steps,
            vec![DocGraphProcessStep::for_actor(
                "Wrapper",
                DocGraphProcessKind::Emit,
                "direct action doc",
            )]
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

    fn demo_edge(label: &str, target: usize) -> DocGraphEdge {
        let presentation = DocGraphActionPresentation::new(label);
        DocGraphEdge {
            label: label.to_owned(),
            compact_label: presentation.compact_label,
            scenario_priority: presentation.scenario_priority,
            interaction_steps: presentation.interaction_steps,
            process_steps: presentation.process_steps,
            target,
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
                vec![demo_edge("A", 1)],
                vec![demo_edge("B", 2)],
                vec![demo_edge("C", 3), demo_edge("D", 4)],
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
                vec![demo_edge("Advance", 1)],
                vec![demo_edge("Loop", 1)],
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
                vec![demo_edge("A", 1)],
                vec![demo_edge("B", 2)],
                vec![demo_edge("C", 3)],
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
                    demo_edge("Start(CommandKind::Deploy)", 1),
                    demo_edge("SetRunning", 1),
                    demo_edge("RequestCancel", 1),
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
