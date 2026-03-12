use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

use nirvash::{
    BoolExpr, Counterexample, CounterexampleKind, ExplorationMode, Fairness, Ltl, ModelBackend,
    ModelCase, ModelCaseSource, ModelCheckConfig, ModelCheckError, ModelCheckResult,
    ReachableGraphEdge, ReachableGraphSnapshot, Signature, StepExpr, TemporalSpec, Trace,
    TraceStep,
};
use serde::{Deserialize, Serialize};

use crate::symbolic::SymbolicModelChecker;

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphEdge<A> {
    step: TraceStep<A>,
    target: usize,
}

impl<A> GraphEdge<A> {
    fn is_stutter(&self) -> bool {
        matches!(self.step, TraceStep::Stutter)
    }
}

#[derive(Debug, Clone)]
struct FrontierExpansion<S, A> {
    source: usize,
    successors: Vec<(TraceStep<A>, S)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointGraphEdge {
    step: CheckpointStep,
    target: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointGraphParent {
    source: usize,
    step: CheckpointStep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CheckpointStep {
    Action(usize),
    Stutter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReachableGraphCheckpoint {
    spec_name: String,
    exploration: ExplorationMode,
    state_storage: nirvash::ExplicitStateStorage,
    states: Vec<usize>,
    edges: Vec<Vec<CheckpointGraphEdge>>,
    initial_indices: Vec<usize>,
    parents: Vec<Option<CheckpointGraphParent>>,
    depths: Vec<usize>,
    deadlocks: Vec<usize>,
    transitions: usize,
    truncated: bool,
    frontier: Vec<usize>,
}

#[derive(Debug, Clone)]
enum ExplicitStateIndex {
    Exact,
    Fingerprinted { buckets: HashMap<u64, Vec<usize>> },
}

impl ExplicitStateIndex {
    fn new(storage: nirvash::ExplicitStateStorage) -> Self {
        match storage {
            nirvash::ExplicitStateStorage::InMemoryExact => Self::Exact,
            nirvash::ExplicitStateStorage::InMemoryFingerprinted => Self::Fingerprinted {
                buckets: HashMap::new(),
            },
        }
    }

    fn from_states<S>(storage: nirvash::ExplicitStateStorage, states: &[S]) -> Self
    where
        S: std::fmt::Debug,
    {
        let mut index = Self::new(storage);
        for state_index in 0..states.len() {
            index.record_state(states, state_index);
        }
        index
    }

    fn state_index<S>(&self, states: &[S], state: &S) -> Option<usize>
    where
        S: PartialEq + std::fmt::Debug,
    {
        match self {
            Self::Exact => states.iter().position(|candidate| candidate == state),
            Self::Fingerprinted { buckets } => {
                let fingerprint = fingerprint_debug(state);
                buckets.get(&fingerprint).and_then(|candidates| {
                    candidates
                        .iter()
                        .copied()
                        .find(|index| states[*index] == *state)
                })
            }
        }
    }

    fn record_state<S>(&mut self, states: &[S], state_index: usize)
    where
        S: std::fmt::Debug,
    {
        let Self::Fingerprinted { buckets } = self else {
            return;
        };
        let fingerprint = fingerprint_debug(&states[state_index]);
        buckets.entry(fingerprint).or_default().push(state_index);
    }
}

fn fingerprint_debug<T: std::fmt::Debug>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    format!("{value:?}").hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy)]
struct SimulationRng {
    state: u64,
}

impl SimulationRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn pick_index(&mut self, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        Some((self.next_u64() as usize) % len)
    }
}

#[derive(Debug, Clone)]
struct ReachableGraph<S, A> {
    states: Vec<S>,
    edges: Vec<Vec<GraphEdge<A>>>,
    initial_indices: Vec<usize>,
    parents: Vec<Option<(usize, TraceStep<A>)>>,
    depths: Vec<usize>,
    deadlocks: Vec<usize>,
    transitions: usize,
    truncated: bool,
}

type TraceList<S, A> = Vec<Trace<S, A>>;

impl<S, A> ReachableGraph<S, A> {
    fn state_index(&self, state: &S) -> Option<usize>
    where
        S: PartialEq,
    {
        self.states.iter().position(|candidate| candidate == state)
    }
}

pub struct BackendModelChecker<'a, T: TemporalSpec + ModelCaseSource> {
    spec: &'a T,
    model_case: ModelCase<T::State, T::Action>,
    config: ModelCheckConfig,
}

impl<'a, T> BackendModelChecker<'a, T>
where
    T: TemporalSpec + ModelCaseSource + Sync,
    T::State: PartialEq + Signature + Send + Sync,
    T::Action: PartialEq + Send + Sync,
{
    pub fn new(spec: &'a T) -> Self {
        let model_case = spec.model_cases().into_iter().next().unwrap_or_default();
        Self::for_case(spec, model_case)
    }

    pub fn for_case(spec: &'a T, model_case: ModelCase<T::State, T::Action>) -> Self {
        let model_case = model_case.with_resolved_backend(
            spec.default_model_backend()
                .unwrap_or(ModelBackend::Explicit),
        );
        let config = model_case.effective_checker_config();
        Self {
            spec,
            model_case,
            config,
        }
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        let check_deadlocks = config.check_deadlocks;
        let backend = config
            .backend
            .or(spec.default_model_backend())
            .unwrap_or(ModelBackend::Explicit);
        let mut model_case = spec.model_cases().into_iter().next().unwrap_or_default();
        model_case = model_case
            .with_checker_config(ModelCheckConfig {
                backend: Some(backend),
                ..config
            })
            .with_check_deadlocks(check_deadlocks);
        Self::for_case(spec, model_case)
    }

    pub fn reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        match self.resolved_doc_backend() {
            ModelBackend::Explicit => {
                let graph = self.build_reachable_graph_for_docs()?;
                Ok(self.snapshot_from_graph(&graph))
            }
            ModelBackend::Symbolic => self.symbolic_reachable_graph_snapshot(),
        }
    }

    pub fn full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        match self.resolved_backend() {
            ModelBackend::Explicit => {
                let graph = self.build_reachable_graph()?;
                self.ensure_untruncated(&graph)?;
                Ok(self.snapshot_from_graph(&graph))
            }
            ModelBackend::Symbolic => self.symbolic_full_reachable_graph_snapshot(),
        }
    }

    pub fn check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        match self.resolved_backend() {
            ModelBackend::Explicit => match self.config.exploration {
                ExplorationMode::ReachableGraph => self.check_invariants_graph(),
                ExplorationMode::BoundedLasso => self.check_invariants_lasso(),
            },
            ModelBackend::Symbolic => self.symbolic_check_invariants(),
        }
    }

    pub fn check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        if !self.config.check_deadlocks {
            return Ok(ModelCheckResult::ok());
        }

        match self.resolved_backend() {
            ModelBackend::Explicit => match self.config.exploration {
                ExplorationMode::ReachableGraph => self.check_deadlocks_graph(),
                ExplorationMode::BoundedLasso => self.check_deadlocks_lasso(),
            },
            ModelBackend::Symbolic => self.symbolic_check_deadlocks(),
        }
    }

    pub fn check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        if self.spec.properties().is_empty() {
            return Ok(ModelCheckResult::ok());
        }

        match self.resolved_backend() {
            ModelBackend::Explicit => match self.config.exploration {
                ExplorationMode::ReachableGraph => self.check_properties_graph(),
                ExplorationMode::BoundedLasso => self.check_properties_lasso(),
            },
            ModelBackend::Symbolic => self.symbolic_check_properties(),
        }
    }

    pub fn check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        match self.resolved_backend() {
            ModelBackend::Explicit => {
                let mut result = ModelCheckResult::ok();

                let invariants = self.check_invariants()?;
                if self.config.stop_on_first_violation && !invariants.is_ok() {
                    return Ok(invariants);
                }
                result.extend(invariants);

                let deadlocks = self.check_deadlocks()?;
                if self.config.stop_on_first_violation && !deadlocks.is_ok() {
                    return Ok(deadlocks);
                }
                result.extend(deadlocks);

                let properties = self.check_properties()?;
                if self.config.stop_on_first_violation && !properties.is_ok() {
                    return Ok(properties);
                }
                result.extend(properties);

                Ok(result)
            }
            ModelBackend::Symbolic => self.symbolic_check_all(),
        }
    }

    pub fn simulate(&self) -> Result<Vec<Trace<T::State, T::Action>>, ModelCheckError> {
        match self.resolved_backend() {
            ModelBackend::Explicit => self.simulate_explicit(),
            ModelBackend::Symbolic => Err(ModelCheckError::UnsupportedConfiguration(
                "simulation is only supported by the explicit backend",
            )),
        }
    }

    pub fn backend(&self) -> ModelBackend {
        self.resolved_backend()
    }

    pub fn doc_backend(&self) -> ModelBackend {
        self.resolved_doc_backend()
    }

    fn resolved_backend(&self) -> ModelBackend {
        self.config.backend.unwrap_or(ModelBackend::Explicit)
    }

    fn resolved_doc_backend(&self) -> ModelBackend {
        self.model_case
            .doc_checker_config()
            .and_then(|config| config.backend)
            .unwrap_or(ModelBackend::Explicit)
    }

    fn symbolic_checker(&self) -> SymbolicModelChecker<'a, T> {
        SymbolicModelChecker::for_case(self.spec, self.model_case.clone())
    }

    fn symbolic_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().reachable_graph_snapshot()
    }

    fn symbolic_full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().full_reachable_graph_snapshot()
    }

    fn symbolic_check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().check_invariants()
    }

    fn symbolic_check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().check_deadlocks()
    }

    fn symbolic_check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().check_properties()
    }

    fn symbolic_check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.symbolic_checker().check_all()
    }

    fn simulate_explicit(&self) -> Result<Vec<Trace<T::State, T::Action>>, ModelCheckError> {
        let initial_states = self.initial_states_filtered()?;
        let simulation = self.config.explicit.simulation;
        let mut rng = SimulationRng::new(simulation.seed);
        let mut traces = Vec::with_capacity(simulation.runs);

        for _ in 0..simulation.runs {
            let initial_index = rng
                .pick_index(initial_states.len())
                .expect("initial state list is non-empty");
            traces.push(self.simulate_trace_from(
                initial_states[initial_index].clone(),
                simulation.max_depth,
                &mut rng,
            ));
        }

        Ok(traces)
    }

    fn check_invariants_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        for (index, state) in graph.states.iter().enumerate() {
            for predicate in self.spec.invariants() {
                if !predicate.eval(state) {
                    return Ok(ModelCheckResult::with_violation(Counterexample {
                        kind: CounterexampleKind::Invariant,
                        name: predicate.name().to_owned(),
                        trace: self.trace_to_state(&graph, index),
                    }));
                }
            }
        }

        Ok(ModelCheckResult::ok())
    }

    fn check_deadlocks_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        if let Some(deadlock) = graph.deadlocks.first() {
            return Ok(ModelCheckResult::with_violation(Counterexample {
                kind: CounterexampleKind::Deadlock,
                name: "deadlock".to_owned(),
                trace: self.trace_to_state(&graph, *deadlock),
            }));
        }

        Ok(ModelCheckResult::ok())
    }

    fn check_properties_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        let traces = self.graph_lasso_traces(&graph);
        let mut best: Option<Counterexample<T::State, T::Action>> = None;

        for property in self.spec.properties() {
            let description = property.describe();
            for trace in &traces {
                if !self.trace_satisfies_fairness_graph(trace, &graph) {
                    continue;
                }
                if !self.eval_formula(trace, &property)[0] {
                    self.consider_violation(
                        &mut best,
                        Counterexample {
                            kind: CounterexampleKind::Property,
                            name: description.clone(),
                            trace: trace.clone(),
                        },
                    );
                }
            }
        }

        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn check_invariants_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let mut best = None;
        for init in self.initial_states_filtered()? {
            self.search_invariants_lasso(vec![init], Vec::new(), &mut best);
        }
        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn check_deadlocks_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let mut best = None;
        for init in self.initial_states_filtered()? {
            self.search_deadlocks_lasso(vec![init], Vec::new(), &mut best);
        }
        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn check_properties_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let traces = self.bounded_lasso_traces()?;
        let mut best = None;
        for property in self.spec.properties() {
            let description = property.describe();
            for trace in &traces {
                if !self.trace_satisfies_fairness_lasso(trace) {
                    continue;
                }
                if !self.eval_formula(trace, &property)[0] {
                    self.consider_violation(
                        &mut best,
                        Counterexample {
                            kind: CounterexampleKind::Property,
                            name: description.clone(),
                            trace: trace.clone(),
                        },
                    );
                }
            }
        }

        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn build_reachable_graph(
        &self,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        self.build_reachable_graph_with_config(self.config.clone())
    }

    fn build_reachable_graph_for_docs(
        &self,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        let mut config = self
            .model_case
            .doc_checker_config()
            .unwrap_or_else(|| self.config.clone());
        config.exploration = ExplorationMode::ReachableGraph;
        config.stop_on_first_violation = false;
        self.build_reachable_graph_with_config(config)
    }

    fn build_reachable_graph_with_config(
        &self,
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        let initial_states = self.initial_states_filtered()?;
        let (mut graph, mut state_index, mut frontier) = self
            .load_reachable_graph_checkpoint(&config)?
            .unwrap_or_else(|| {
                (
                    ReachableGraph {
                        states: Vec::new(),
                        edges: Vec::new(),
                        initial_indices: Vec::new(),
                        parents: Vec::new(),
                        depths: Vec::new(),
                        deadlocks: Vec::new(),
                        transitions: 0,
                        truncated: false,
                    },
                    ExplicitStateIndex::new(config.explicit.state_storage),
                    Vec::new(),
                )
            });
        let distributed_shards = config.explicit.distributed.shards.max(1);

        if frontier.is_empty() && graph.states.is_empty() {
            for state in initial_states {
                let Some(index) = self.push_state_flat(
                    &mut graph,
                    &mut state_index,
                    state,
                    None,
                    0,
                    &mut frontier,
                    &config,
                )?
                else {
                    break;
                };
                if !graph.initial_indices.contains(&index) {
                    graph.initial_indices.push(index);
                }
                if graph.truncated {
                    break;
                }
            }
            self.save_reachable_graph_checkpoint(&graph, &frontier, &config, 0)?;
        }

        let mut completed_frontiers = 0usize;
        match config.explicit.reachability {
            nirvash::ExplicitReachabilityStrategy::BreadthFirst
            | nirvash::ExplicitReachabilityStrategy::ParallelFrontier => {
                while !frontier.is_empty() {
                    if graph.truncated {
                        break;
                    }
                    let current_frontier = std::mem::take(&mut frontier);
                    let expansions =
                        self.expand_frontier_batch(&graph, &current_frontier, &config)?;
                    let mut next_frontier = Vec::new();
                    self.merge_expansions_flat(
                        &mut graph,
                        &mut state_index,
                        expansions,
                        &mut next_frontier,
                        &config,
                    )?;

                    completed_frontiers += 1;
                    frontier = next_frontier;
                    self.save_reachable_graph_checkpoint(
                        &graph,
                        &frontier,
                        &config,
                        completed_frontiers,
                    )?;
                }
            }
            nirvash::ExplicitReachabilityStrategy::DistributedFrontier => {
                let mut frontier_shards = self.partition_frontier(
                    &graph,
                    std::mem::take(&mut frontier),
                    distributed_shards,
                );
                while frontier_shards.iter().any(|shard| !shard.is_empty()) {
                    if graph.truncated {
                        break;
                    }
                    for shard_index in 0..frontier_shards.len() {
                        if graph.truncated {
                            break;
                        }
                        let current_frontier = std::mem::take(&mut frontier_shards[shard_index]);
                        if current_frontier.is_empty() {
                            continue;
                        }
                        let expansions =
                            self.expand_frontier_batch(&graph, &current_frontier, &config)?;
                        self.merge_expansions_sharded(
                            &mut graph,
                            &mut state_index,
                            expansions,
                            &mut frontier_shards,
                            distributed_shards,
                            &config,
                        )?;
                    }

                    completed_frontiers += 1;
                    frontier = frontier_shards
                        .iter()
                        .flat_map(|shard| shard.iter().copied())
                        .collect();
                    self.save_reachable_graph_checkpoint(
                        &graph,
                        &frontier,
                        &config,
                        completed_frontiers,
                    )?;
                }
            }
        }

        Ok(graph)
    }

    fn load_reachable_graph_checkpoint(
        &self,
        config: &ModelCheckConfig,
    ) -> Result<
        Option<(
            ReachableGraph<T::State, T::Action>,
            ExplicitStateIndex,
            Vec<usize>,
        )>,
        ModelCheckError,
    > {
        let checkpoint = &config.explicit.checkpoint;
        let Some(path) = checkpoint.path.as_deref() else {
            return Ok(None);
        };
        if !checkpoint.resume || !Path::new(path).exists() {
            return Ok(None);
        }

        let bytes =
            fs::read(path).map_err(|error| ModelCheckError::CheckpointIo(error.to_string()))?;
        let saved: ReachableGraphCheckpoint = serde_json::from_slice(&bytes)
            .map_err(|error| ModelCheckError::CheckpointIo(error.to_string()))?;
        if saved.spec_name != self.spec.name()
            || saved.exploration != config.exploration
            || saved.state_storage != config.explicit.state_storage
        {
            return Err(ModelCheckError::CheckpointIo(format!(
                "checkpoint at `{path}` does not match the current explicit exploration config"
            )));
        }

        let state_domain = T::State::bounded_domain().into_vec();
        let actions = self.spec.actions();
        let states = saved
            .states
            .iter()
            .map(|index| {
                state_domain.get(*index).cloned().ok_or_else(|| {
                    ModelCheckError::CheckpointIo(format!(
                        "checkpoint state index {index} is outside the bounded domain"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let edges = saved
            .edges
            .iter()
            .map(|edges| {
                edges
                    .iter()
                    .map(|edge| {
                        Ok(GraphEdge {
                            step: self.decode_checkpoint_step(&edge.step, &actions)?,
                            target: edge.target,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parents = saved
            .parents
            .iter()
            .map(|parent| {
                parent
                    .as_ref()
                    .map(|parent| {
                        Ok((
                            parent.source,
                            self.decode_checkpoint_step(&parent.step, &actions)?,
                        ))
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let graph = ReachableGraph {
            states,
            edges,
            initial_indices: saved.initial_indices,
            parents,
            depths: saved.depths,
            deadlocks: saved.deadlocks,
            transitions: saved.transitions,
            truncated: saved.truncated,
        };
        let state_index =
            ExplicitStateIndex::from_states(config.explicit.state_storage, &graph.states);

        Ok(Some((graph, state_index, saved.frontier)))
    }

    fn save_reachable_graph_checkpoint(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        frontier: &[usize],
        config: &ModelCheckConfig,
        completed_frontiers: usize,
    ) -> Result<(), ModelCheckError> {
        let checkpoint = &config.explicit.checkpoint;
        let Some(path) = checkpoint.path.as_deref() else {
            return Ok(());
        };
        if completed_frontiers % checkpoint.save_every_frontiers != 0 {
            return Ok(());
        }

        let state_domain = T::State::bounded_domain().into_vec();
        let actions = self.spec.actions();
        let snapshot = ReachableGraphCheckpoint {
            spec_name: self.spec.name().to_owned(),
            exploration: config.exploration,
            state_storage: config.explicit.state_storage,
            states: graph
                .states
                .iter()
                .map(|state| self.state_domain_index(&state_domain, state))
                .collect::<Result<Vec<_>, _>>()?,
            edges: graph
                .edges
                .iter()
                .map(|edges| {
                    edges
                        .iter()
                        .map(|edge| {
                            Ok(CheckpointGraphEdge {
                                step: self.encode_checkpoint_step(&actions, &edge.step)?,
                                target: edge.target,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()?,
            initial_indices: graph.initial_indices.clone(),
            parents: graph
                .parents
                .iter()
                .map(|parent| {
                    parent
                        .as_ref()
                        .map(|(source, step)| {
                            Ok(CheckpointGraphParent {
                                source: *source,
                                step: self.encode_checkpoint_step(&actions, step)?,
                            })
                        })
                        .transpose()
                })
                .collect::<Result<Vec<_>, _>>()?,
            depths: graph.depths.clone(),
            deadlocks: graph.deadlocks.clone(),
            transitions: graph.transitions,
            truncated: graph.truncated,
            frontier: frontier.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| ModelCheckError::CheckpointIo(error.to_string()))?;
        fs::write(path, bytes).map_err(|error| ModelCheckError::CheckpointIo(error.to_string()))
    }

    fn state_domain_index(
        &self,
        state_domain: &[T::State],
        state: &T::State,
    ) -> Result<usize, ModelCheckError> {
        state_domain
            .iter()
            .position(|candidate| candidate == state)
            .ok_or(ModelCheckError::UnsupportedConfiguration(
                "checkpoint requires every reachable state to appear in T::State::bounded_domain()",
            ))
    }

    fn encode_checkpoint_step(
        &self,
        actions: &[T::Action],
        step: &TraceStep<T::Action>,
    ) -> Result<CheckpointStep, ModelCheckError> {
        match step {
            TraceStep::Action(action) => actions
                .iter()
                .position(|candidate| candidate == action)
                .map(CheckpointStep::Action)
                .ok_or(ModelCheckError::UnsupportedConfiguration(
                    "checkpoint requires every explicit action to appear in spec.actions()",
                )),
            TraceStep::Stutter => Ok(CheckpointStep::Stutter),
        }
    }

    fn decode_checkpoint_step(
        &self,
        step: &CheckpointStep,
        actions: &[T::Action],
    ) -> Result<TraceStep<T::Action>, ModelCheckError> {
        match step {
            CheckpointStep::Action(index) => actions
                .get(*index)
                .cloned()
                .map(TraceStep::Action)
                .ok_or_else(|| {
                    ModelCheckError::CheckpointIo(format!(
                        "checkpoint action index {index} is outside spec.actions()"
                    ))
                }),
            CheckpointStep::Stutter => Ok(TraceStep::Stutter),
        }
    }

    fn expand_frontier_batch(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        frontier: &[usize],
        config: &ModelCheckConfig,
    ) -> Result<Vec<FrontierExpansion<T::State, T::Action>>, ModelCheckError> {
        if matches!(
            config.explicit.reachability,
            nirvash::ExplicitReachabilityStrategy::ParallelFrontier
        ) {
            if !self.model_case.state_constraints().is_empty()
                || !self.model_case.action_constraints().is_empty()
                || self.model_case.symmetry().is_some()
            {
                return Err(ModelCheckError::UnsupportedConfiguration(
                    "parallel frontier exploration does not support state/action constraints or symmetry reduction",
                ));
            }
            return Ok(self.expand_frontier_parallel(
                graph,
                frontier,
                config.explicit.parallel.workers,
            ));
        }

        Ok(frontier
            .iter()
            .copied()
            .map(|index| self.expand_frontier_state(index, graph.states[index].clone()))
            .collect())
    }

    fn expand_frontier_parallel(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        frontier: &[usize],
        workers: usize,
    ) -> Vec<FrontierExpansion<T::State, T::Action>> {
        if workers <= 1 || frontier.len() <= 1 {
            return frontier
                .iter()
                .copied()
                .map(|index| self.expand_frontier_state(index, graph.states[index].clone()))
                .collect();
        }

        let tasks = frontier
            .iter()
            .copied()
            .map(|index| (index, graph.states[index].clone()))
            .collect::<Vec<_>>();
        let chunk_size = tasks.len().div_ceil(workers);
        let spec = self.spec;
        let allow_stutter = spec.allow_stutter();
        let mut expansions = Vec::with_capacity(tasks.len());
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for chunk in tasks.chunks(chunk_size.max(1)) {
                let owned = chunk.to_vec();
                handles.push(scope.spawn(move || {
                    owned
                        .into_iter()
                        .map(|(index, state)| {
                            let mut successors = spec
                                .successors(&state)
                                .into_iter()
                                .map(|(action, next)| (TraceStep::Action(action), next))
                                .collect::<Vec<_>>();
                            if allow_stutter {
                                successors.push((TraceStep::Stutter, spec.stutter_state(&state)));
                            }
                            FrontierExpansion {
                                source: index,
                                successors,
                            }
                        })
                        .collect::<Vec<_>>()
                }));
            }
            for handle in handles {
                expansions.extend(
                    handle
                        .join()
                        .expect("parallel frontier worker should not panic"),
                );
            }
        });
        expansions.sort_by_key(|expansion| expansion.source);
        expansions
    }

    fn expand_frontier_state(
        &self,
        index: usize,
        state: T::State,
    ) -> FrontierExpansion<T::State, T::Action> {
        FrontierExpansion {
            source: index,
            successors: self.constrained_successors(&state),
        }
    }

    fn merge_expansions_flat(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state_index: &mut ExplicitStateIndex,
        expansions: Vec<FrontierExpansion<T::State, T::Action>>,
        frontier: &mut Vec<usize>,
        config: &ModelCheckConfig,
    ) -> Result<(), ModelCheckError> {
        for expansion in expansions {
            if graph.truncated {
                break;
            }
            self.merge_expansion(
                graph,
                state_index,
                expansion,
                &mut |next_index, _| frontier.push(next_index),
                config,
            )?;
        }
        Ok(())
    }

    fn merge_expansions_sharded(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state_index: &mut ExplicitStateIndex,
        expansions: Vec<FrontierExpansion<T::State, T::Action>>,
        frontier_shards: &mut [Vec<usize>],
        shards: usize,
        config: &ModelCheckConfig,
    ) -> Result<(), ModelCheckError> {
        for expansion in expansions {
            if graph.truncated {
                break;
            }
            self.merge_expansion(
                graph,
                state_index,
                expansion,
                &mut |next_index, next_state| {
                    let shard = self.state_shard(next_state, shards);
                    frontier_shards[shard].push(next_index);
                },
                config,
            )?;
        }
        Ok(())
    }

    fn merge_expansion(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state_index: &mut ExplicitStateIndex,
        expansion: FrontierExpansion<T::State, T::Action>,
        push_frontier: &mut dyn FnMut(usize, &T::State),
        config: &ModelCheckConfig,
    ) -> Result<(), ModelCheckError> {
        let next_depth = graph.depths[expansion.source] + 1;
        let mut edges = Vec::new();
        for (step, next_state) in expansion.successors {
            let Some(next_index) = self.push_state_with(
                graph,
                state_index,
                next_state,
                Some((expansion.source, step.clone())),
                next_depth,
                push_frontier,
                config,
            )?
            else {
                graph.truncated = true;
                break;
            };

            let edge = GraphEdge {
                step,
                target: next_index,
            };
            if !edges.contains(&edge) {
                if !edge.is_stutter() {
                    if self.transition_limit_reached(graph, config) {
                        graph.truncated = true;
                        break;
                    }
                    graph.transitions += 1;
                }
                edges.push(edge);
            }
        }

        if edges.iter().all(GraphEdge::is_stutter) {
            graph.deadlocks.push(expansion.source);
        }
        graph.edges[expansion.source] = edges;
        Ok(())
    }

    fn partition_frontier(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        frontier: Vec<usize>,
        shards: usize,
    ) -> Vec<Vec<usize>> {
        let mut frontier_shards = vec![Vec::new(); shards.max(1)];
        for index in frontier {
            let shard = self.state_shard(&graph.states[index], shards);
            frontier_shards[shard].push(index);
        }
        frontier_shards
    }

    fn state_shard(&self, state: &T::State, shards: usize) -> usize {
        (fingerprint_debug(state) as usize) % shards.max(1)
    }

    fn push_state_flat(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state_index: &mut ExplicitStateIndex,
        state: T::State,
        parent: Option<(usize, TraceStep<T::Action>)>,
        depth: usize,
        frontier: &mut Vec<usize>,
        config: &ModelCheckConfig,
    ) -> Result<Option<usize>, ModelCheckError> {
        self.push_state_with(
            graph,
            state_index,
            state,
            parent,
            depth,
            &mut |index, _| frontier.push(index),
            config,
        )
    }

    fn push_state_with(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state_index: &mut ExplicitStateIndex,
        state: T::State,
        parent: Option<(usize, TraceStep<T::Action>)>,
        depth: usize,
        push_frontier: &mut dyn FnMut(usize, &T::State),
        config: &ModelCheckConfig,
    ) -> Result<Option<usize>, ModelCheckError> {
        if let Some(existing) = state_index.state_index(&graph.states, &state) {
            return Ok(Some(existing));
        }

        if self.state_limit_reached(graph, config) {
            graph.truncated = true;
            return Ok(None);
        }

        graph.states.push(state);
        graph.edges.push(Vec::new());
        graph.parents.push(parent);
        graph.depths.push(depth);
        let index = graph.states.len() - 1;
        state_index.record_state(&graph.states, index);
        push_frontier(index, &graph.states[index]);
        Ok(Some(index))
    }

    fn state_limit_reached(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        config: &ModelCheckConfig,
    ) -> bool {
        config
            .max_states
            .is_some_and(|max_states| graph.states.len() >= max_states)
    }

    fn transition_limit_reached(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        config: &ModelCheckConfig,
    ) -> bool {
        config
            .max_transitions
            .is_some_and(|max_transitions| graph.transitions >= max_transitions)
    }

    fn initial_states_filtered(&self) -> Result<Vec<T::State>, ModelCheckError> {
        let states = self
            .spec
            .initial_states()
            .into_iter()
            .map(|state| self.canonicalize_state(&state))
            .filter(|state| self.state_constraints_allow(state))
            .fold(Vec::new(), |mut acc, state| {
                if !acc.contains(&state) {
                    acc.push(state);
                }
                acc
            });

        if states.is_empty() {
            return Err(ModelCheckError::NoInitialStates);
        }

        Ok(states)
    }

    fn canonicalize_state(&self, state: &T::State) -> T::State {
        self.model_case
            .symmetry()
            .map(|symmetry| symmetry.canonicalize(state))
            .unwrap_or_else(|| state.clone())
    }

    fn state_constraints_allow(&self, state: &T::State) -> bool {
        self.model_case
            .state_constraints()
            .iter()
            .all(|constraint: &BoolExpr<T::State>| constraint.eval(state))
    }

    fn action_constraints_allow(
        &self,
        prev: &T::State,
        action: &T::Action,
        next: &T::State,
    ) -> bool {
        self.model_case
            .action_constraints()
            .iter()
            .all(|constraint: &StepExpr<T::State, T::Action>| constraint.eval(prev, action, next))
    }

    fn constrained_successors(&self, state: &T::State) -> Vec<(TraceStep<T::Action>, T::State)> {
        let mut values = Vec::new();
        for (action, next_raw) in self.spec.successors_constrained(state, &|action, next| {
            self.action_constraints_allow(state, action, next)
        }) {
            let next = self.canonicalize_state(&next_raw);
            if !self.state_constraints_allow(&next) {
                continue;
            }
            values.push((TraceStep::Action(action), next));
        }

        if self.spec.allow_stutter() {
            let stutter = self.canonicalize_state(&self.spec.stutter_state(state));
            if self.state_constraints_allow(&stutter) {
                values.push((TraceStep::Stutter, stutter));
            }
        }

        values
    }

    fn ensure_untruncated(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> Result<(), ModelCheckError> {
        if graph.truncated {
            return Err(ModelCheckError::ExplorationLimitReached {
                states: graph.states.len(),
                transitions: graph.transitions,
            });
        }
        Ok(())
    }

    fn snapshot_from_graph(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> ReachableGraphSnapshot<T::State, T::Action> {
        ReachableGraphSnapshot {
            states: graph.states.clone(),
            edges: graph
                .edges
                .iter()
                .map(|edges| {
                    edges
                        .iter()
                        .filter_map(|edge| match &edge.step {
                            TraceStep::Action(action) => Some(ReachableGraphEdge {
                                action: action.clone(),
                                target: edge.target,
                            }),
                            TraceStep::Stutter => None,
                        })
                        .collect()
                })
                .collect(),
            initial_indices: graph.initial_indices.clone(),
            deadlocks: graph.deadlocks.clone(),
            truncated: graph.truncated,
            stutter_omitted: self.spec.allow_stutter(),
        }
    }

    fn search_invariants_lasso(
        &self,
        states: Vec<T::State>,
        steps: Vec<TraceStep<T::Action>>,
        best: &mut Option<Counterexample<T::State, T::Action>>,
    ) {
        let depth = steps.len();
        let current = states.last().expect("states always non-empty");

        for predicate in self.spec.invariants() {
            if !predicate.eval(current) {
                self.consider_violation(
                    best,
                    Counterexample {
                        kind: CounterexampleKind::Invariant,
                        name: predicate.name().to_owned(),
                        trace: self.terminal_trace(states.clone(), steps.clone()),
                    },
                );
                return;
            }
        }

        if self.reached_bounded_depth(depth) {
            return;
        }

        for (step, next) in self.constrained_successors(current) {
            let mut next_states = states.clone();
            next_states.push(next);
            let mut next_steps = steps.clone();
            next_steps.push(step);
            self.search_invariants_lasso(next_states, next_steps, best);
        }
    }

    fn search_deadlocks_lasso(
        &self,
        states: Vec<T::State>,
        steps: Vec<TraceStep<T::Action>>,
        best: &mut Option<Counterexample<T::State, T::Action>>,
    ) {
        let depth = steps.len();
        let current = states.last().expect("states always non-empty");

        let has_non_stutter = self
            .constrained_successors(current)
            .iter()
            .any(|(step, _)| matches!(step, TraceStep::Action(_)));
        if !has_non_stutter {
            self.consider_violation(
                best,
                Counterexample {
                    kind: CounterexampleKind::Deadlock,
                    name: "deadlock".to_owned(),
                    trace: self.terminal_trace(states.clone(), steps.clone()),
                },
            );
            return;
        }

        if self.reached_bounded_depth(depth) {
            return;
        }

        for (step, next) in self.constrained_successors(current) {
            let mut next_states = states.clone();
            next_states.push(next);
            let mut next_steps = steps.clone();
            next_steps.push(step);
            self.search_deadlocks_lasso(next_states, next_steps, best);
        }
    }

    fn bounded_lasso_traces(&self) -> Result<TraceList<T::State, T::Action>, ModelCheckError> {
        let mut traces = Vec::new();
        for init in self.initial_states_filtered()? {
            self.enumerate_lasso(vec![init], Vec::new(), &mut traces);
        }
        Ok(traces)
    }

    fn simulate_trace_from(
        &self,
        initial: T::State,
        max_depth: usize,
        rng: &mut SimulationRng,
    ) -> Trace<T::State, T::Action> {
        let mut states = vec![initial];
        let mut steps = Vec::new();

        while steps.len() < max_depth {
            let current = states.last().expect("states always non-empty");
            let successors = self.constrained_successors(current);
            let Some(successor_index) = rng.pick_index(successors.len()) else {
                return self.terminal_trace(states, steps);
            };
            let (step, next) = successors[successor_index].clone();

            if let Some(loop_start) = states.iter().position(|state| state == &next) {
                steps.push(step);
                return Trace::new(states, steps, loop_start);
            }

            states.push(next);
            steps.push(step);
        }

        self.terminal_trace(states, steps)
    }

    fn enumerate_lasso(
        &self,
        states: Vec<T::State>,
        steps: Vec<TraceStep<T::Action>>,
        traces: &mut TraceList<T::State, T::Action>,
    ) {
        traces.push(self.terminal_trace(states.clone(), steps.clone()));

        if self.reached_bounded_depth(steps.len()) {
            return;
        }

        let current = states.last().expect("states always non-empty");
        for (step, next) in self.constrained_successors(current) {
            if let Some(loop_start) = states.iter().position(|state| state == &next) {
                let mut lasso_steps = steps.clone();
                lasso_steps.push(step);
                traces.push(Trace::new(states.clone(), lasso_steps, loop_start));
                continue;
            }

            let mut next_states = states.clone();
            next_states.push(next);
            let mut next_steps = steps.clone();
            next_steps.push(step);
            self.enumerate_lasso(next_states, next_steps, traces);
        }
    }

    fn graph_lasso_traces(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> TraceList<T::State, T::Action> {
        let mut traces = Vec::new();
        for &initial in &graph.initial_indices {
            self.enumerate_graph_lassos(graph, vec![initial], Vec::new(), &mut traces);
        }
        traces
    }

    fn enumerate_graph_lassos(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        path_states: Vec<usize>,
        path_steps: Vec<TraceStep<T::Action>>,
        traces: &mut TraceList<T::State, T::Action>,
    ) {
        let current = *path_states.last().expect("path has at least one state");
        for edge in &graph.edges[current] {
            if let Some(loop_start) = path_states.iter().position(|state| *state == edge.target) {
                let states = path_states
                    .iter()
                    .map(|index| graph.states[*index].clone())
                    .collect();
                let mut steps = path_steps.clone();
                steps.push(edge.step.clone());
                traces.push(Trace::new(states, steps, loop_start));
                continue;
            }

            if matches!(self.config.exploration, ExplorationMode::BoundedLasso)
                && self.reached_bounded_depth(path_steps.len() + 1)
            {
                continue;
            }

            let mut next_states = path_states.clone();
            next_states.push(edge.target);
            let mut next_steps = path_steps.clone();
            next_steps.push(edge.step.clone());
            self.enumerate_graph_lassos(graph, next_states, next_steps, traces);
        }
    }

    fn trace_to_state(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        target: usize,
    ) -> Trace<T::State, T::Action> {
        let (states, steps) = self.reconstruct_path(graph, target);
        self.terminal_trace(states, steps)
    }

    fn reconstruct_path(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        target: usize,
    ) -> (Vec<T::State>, Vec<TraceStep<T::Action>>) {
        let mut states = vec![target];
        let mut steps = Vec::new();
        let mut cursor = target;
        while let Some((parent, step)) = &graph.parents[cursor] {
            states.push(*parent);
            steps.push(step.clone());
            cursor = *parent;
        }
        states.reverse();
        steps.reverse();
        let states = states
            .into_iter()
            .map(|index| graph.states[index].clone())
            .collect();
        (states, steps)
    }

    fn terminal_trace(
        &self,
        states: Vec<T::State>,
        steps: Vec<TraceStep<T::Action>>,
    ) -> Trace<T::State, T::Action> {
        let mut trace_steps = steps;
        trace_steps.push(TraceStep::Stutter);
        let loop_start = trace_steps.len() - 1;
        Trace::new(states, trace_steps, loop_start)
    }

    fn reached_bounded_depth(&self, depth: usize) -> bool {
        matches!(self.config.exploration, ExplorationMode::BoundedLasso)
            && self
                .config
                .bounded_depth
                .is_some_and(|bounded_depth| depth >= bounded_depth)
    }

    fn consider_violation(
        &self,
        best: &mut Option<Counterexample<T::State, T::Action>>,
        candidate: Counterexample<T::State, T::Action>,
    ) {
        let replace =
            best.as_ref()
                .is_none_or(|current| match self.config.counterexample_minimization {
                    nirvash::CounterexampleMinimization::None => false,
                    nirvash::CounterexampleMinimization::ShortestTrace => {
                        candidate.trace.minimization_key() < current.trace.minimization_key()
                    }
                });
        if replace {
            *best = Some(candidate);
        }
    }

    fn trace_satisfies_fairness_graph(
        &self,
        trace: &Trace<T::State, T::Action>,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> bool {
        self.spec
            .fairness()
            .into_iter()
            .all(|fairness| self.eval_fairness_graph(trace, graph, fairness))
    }

    fn eval_fairness_graph(
        &self,
        trace: &Trace<T::State, T::Action>,
        graph: &ReachableGraph<T::State, T::Action>,
        fairness: Fairness<T::State, T::Action>,
    ) -> bool {
        let predicate = fairness.predicate();
        let occurs = trace.cycle_indices().any(|index| {
            let next_index = trace.next_index(index);
            match &trace.steps()[index] {
                TraceStep::Action(action) => {
                    predicate.eval(&trace.states()[index], action, &trace.states()[next_index])
                }
                TraceStep::Stutter => false,
            }
        });
        let enabled_any = trace.cycle_indices().any(|index| {
            graph
                .state_index(&trace.states()[index])
                .into_iter()
                .flat_map(|state_index| &graph.edges[state_index])
                .filter_map(|edge| match &edge.step {
                    TraceStep::Action(action) => Some((action, edge.target)),
                    TraceStep::Stutter => None,
                })
                .any(|(action, target)| {
                    predicate.eval(&trace.states()[index], action, &graph.states[target])
                })
        });
        let enabled_all = trace.cycle_indices().all(|index| {
            graph
                .state_index(&trace.states()[index])
                .into_iter()
                .flat_map(|state_index| &graph.edges[state_index])
                .filter_map(|edge| match &edge.step {
                    TraceStep::Action(action) => Some((action, edge.target)),
                    TraceStep::Stutter => None,
                })
                .any(|(action, target)| {
                    predicate.eval(&trace.states()[index], action, &graph.states[target])
                })
        });

        match fairness {
            Fairness::Weak(_) => !enabled_all || occurs,
            Fairness::Strong(_) => !enabled_any || occurs,
        }
    }

    fn trace_satisfies_fairness_lasso(&self, trace: &Trace<T::State, T::Action>) -> bool {
        self.spec
            .fairness()
            .into_iter()
            .all(|fairness| self.eval_fairness_lasso(trace, fairness))
    }

    fn eval_fairness_lasso(
        &self,
        trace: &Trace<T::State, T::Action>,
        fairness: Fairness<T::State, T::Action>,
    ) -> bool {
        let predicate = fairness.predicate();
        let occurs = trace.cycle_indices().any(|index| {
            let next_index = trace.next_index(index);
            match &trace.steps()[index] {
                TraceStep::Action(action) => {
                    predicate.eval(&trace.states()[index], action, &trace.states()[next_index])
                }
                TraceStep::Stutter => false,
            }
        });
        let enabled_any = trace.cycle_indices().any(|index| {
            self.constrained_successors(&trace.states()[index])
                .into_iter()
                .filter_map(|(step, next)| match step {
                    TraceStep::Action(action) => Some((action, next)),
                    TraceStep::Stutter => None,
                })
                .any(|(action, next)| predicate.eval(&trace.states()[index], &action, &next))
        });
        let enabled_all = trace.cycle_indices().all(|index| {
            self.constrained_successors(&trace.states()[index])
                .into_iter()
                .filter_map(|(step, next)| match step {
                    TraceStep::Action(action) => Some((action, next)),
                    TraceStep::Stutter => None,
                })
                .any(|(action, next)| predicate.eval(&trace.states()[index], &action, &next))
        });

        match fairness {
            Fairness::Weak(_) => !enabled_all || occurs,
            Fairness::Strong(_) => !enabled_any || occurs,
        }
    }

    fn eval_formula(
        &self,
        trace: &Trace<T::State, T::Action>,
        formula: &Ltl<T::State, T::Action>,
    ) -> Vec<bool> {
        let len = trace.len();
        match formula {
            Ltl::True => vec![true; len],
            Ltl::False => vec![false; len],
            Ltl::Pred(predicate) => trace
                .states()
                .iter()
                .map(|state| predicate.eval(state))
                .collect(),
            Ltl::StepPred(predicate) => (0..len)
                .map(|index| {
                    let next_index = trace.next_index(index);
                    match &trace.steps()[index] {
                        TraceStep::Action(action) => predicate.eval(
                            &trace.states()[index],
                            action,
                            &trace.states()[next_index],
                        ),
                        TraceStep::Stutter => false,
                    }
                })
                .collect(),
            Ltl::Not(inner) => self
                .eval_formula(trace, inner)
                .into_iter()
                .map(|value| !value)
                .collect(),
            Ltl::And(lhs, rhs) => self
                .eval_formula(trace, lhs)
                .into_iter()
                .zip(self.eval_formula(trace, rhs))
                .map(|(lhs, rhs)| lhs && rhs)
                .collect(),
            Ltl::Or(lhs, rhs) => self
                .eval_formula(trace, lhs)
                .into_iter()
                .zip(self.eval_formula(trace, rhs))
                .map(|(lhs, rhs)| lhs || rhs)
                .collect(),
            Ltl::Implies(lhs, rhs) => self
                .eval_formula(trace, lhs)
                .into_iter()
                .zip(self.eval_formula(trace, rhs))
                .map(|(lhs, rhs)| !lhs || rhs)
                .collect(),
            Ltl::Next(inner) => {
                let inner = self.eval_formula(trace, inner);
                (0..len)
                    .map(|index| inner[trace.next_index(index)])
                    .collect()
            }
            Ltl::Always(inner) => self.eval_always(trace, &self.eval_formula(trace, inner)),
            Ltl::Eventually(inner) => self.eval_eventually(trace, &self.eval_formula(trace, inner)),
            Ltl::Until(lhs, rhs) => self.eval_until(
                trace,
                &self.eval_formula(trace, lhs),
                &self.eval_formula(trace, rhs),
            ),
            Ltl::Enabled(predicate) => trace
                .states()
                .iter()
                .map(|state| {
                    self.constrained_successors(state)
                        .into_iter()
                        .filter_map(|(step, next)| match step {
                            TraceStep::Action(action) => Some((action, next)),
                            TraceStep::Stutter => None,
                        })
                        .any(|(action, next)| predicate.eval(state, &action, &next))
                })
                .collect(),
        }
    }

    fn eval_eventually(&self, trace: &Trace<T::State, T::Action>, inner: &[bool]) -> Vec<bool> {
        let len = trace.len();
        let mut result = inner.to_vec();
        let mut changed = true;
        while changed {
            changed = false;
            for index in (0..len).rev() {
                let candidate = inner[index] || result[trace.next_index(index)];
                if candidate != result[index] {
                    result[index] = candidate;
                    changed = true;
                }
            }
        }
        result
    }

    fn eval_always(&self, trace: &Trace<T::State, T::Action>, inner: &[bool]) -> Vec<bool> {
        let len = trace.len();
        let mut result = vec![true; len];
        let mut changed = true;
        while changed {
            changed = false;
            for index in (0..len).rev() {
                let candidate = inner[index] && result[trace.next_index(index)];
                if candidate != result[index] {
                    result[index] = candidate;
                    changed = true;
                }
            }
        }
        result
    }

    fn eval_until(
        &self,
        trace: &Trace<T::State, T::Action>,
        lhs: &[bool],
        rhs: &[bool],
    ) -> Vec<bool> {
        let len = trace.len();
        let mut result = rhs.to_vec();
        let mut changed = true;
        while changed {
            changed = false;
            for index in (0..len).rev() {
                let candidate = rhs[index] || (lhs[index] && result[trace.next_index(index)]);
                if candidate != result[index] {
                    result[index] = candidate;
                    changed = true;
                }
            }
        }
        result
    }
}
