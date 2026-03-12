use std::collections::VecDeque;

use nirvash::{
    BoolExpr, Counterexample, CounterexampleKind, ExplorationMode, Fairness, Ltl, ModelBackend,
    ModelCase, ModelCaseSource, ModelCheckConfig, ModelCheckError, ModelCheckResult,
    ReachableGraphEdge, ReachableGraphSnapshot, Signature, StepExpr, TemporalSpec, Trace,
    TraceStep,
};

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
    T: TemporalSpec + ModelCaseSource,
    T::State: PartialEq + Signature,
    T::Action: PartialEq,
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
        self.build_reachable_graph_with_config(self.config)
    }

    fn build_reachable_graph_for_docs(
        &self,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        let mut config = self.model_case.doc_checker_config().unwrap_or(self.config);
        config.exploration = ExplorationMode::ReachableGraph;
        config.stop_on_first_violation = false;
        self.build_reachable_graph_with_config(config)
    }

    fn build_reachable_graph_with_config(
        &self,
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        let initial_states = self.initial_states_filtered()?;
        let mut graph = ReachableGraph {
            states: Vec::new(),
            edges: Vec::new(),
            initial_indices: Vec::new(),
            parents: Vec::new(),
            depths: Vec::new(),
            deadlocks: Vec::new(),
            transitions: 0,
            truncated: false,
        };
        let mut queue = VecDeque::new();

        for state in initial_states {
            let Some(index) = self.push_state(&mut graph, state, None, 0, &mut queue, config)?
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

        while let Some(index) = queue.pop_front() {
            if graph.truncated {
                break;
            }
            let current = graph.states[index].clone();
            let next_depth = graph.depths[index] + 1;
            let mut edges = Vec::new();

            for (action, next_raw) in self.spec.successors_constrained(&current, &|action, next| {
                self.action_constraints_allow(&current, action, next)
            }) {
                let next = self.canonicalize_state(&next_raw);
                if !self.state_constraints_allow(&next) {
                    continue;
                }

                let Some(next_index) = self.push_state(
                    &mut graph,
                    next,
                    Some((index, TraceStep::Action(action.clone()))),
                    next_depth,
                    &mut queue,
                    config,
                )?
                else {
                    break;
                };
                let edge = GraphEdge {
                    step: TraceStep::Action(action),
                    target: next_index,
                };
                if !edges.contains(&edge) {
                    if self.transition_limit_reached(&graph, config) {
                        graph.truncated = true;
                        break;
                    }
                    edges.push(edge);
                    graph.transitions += 1;
                }
            }

            if !graph.truncated && self.spec.allow_stutter() {
                let stutter = self.canonicalize_state(&self.spec.stutter_state(&current));
                if self.state_constraints_allow(&stutter) {
                    let Some(next_index) = self.push_state(
                        &mut graph,
                        stutter,
                        Some((index, TraceStep::Stutter)),
                        next_depth,
                        &mut queue,
                        config,
                    )?
                    else {
                        graph.truncated = true;
                        break;
                    };
                    let edge = GraphEdge {
                        step: TraceStep::Stutter,
                        target: next_index,
                    };
                    if !edges.contains(&edge) {
                        edges.push(edge);
                    }
                }
            }

            if edges.iter().all(GraphEdge::is_stutter) {
                graph.deadlocks.push(index);
            }

            graph.edges[index] = edges;
        }

        Ok(graph)
    }

    fn push_state(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state: T::State,
        parent: Option<(usize, TraceStep<T::Action>)>,
        depth: usize,
        queue: &mut VecDeque<usize>,
        config: ModelCheckConfig,
    ) -> Result<Option<usize>, ModelCheckError> {
        if let Some(existing) = graph.state_index(&state) {
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
        queue.push_back(index);
        Ok(Some(index))
    }

    fn state_limit_reached(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        config: ModelCheckConfig,
    ) -> bool {
        config
            .max_states
            .is_some_and(|max_states| graph.states.len() >= max_states)
    }

    fn transition_limit_reached(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        config: ModelCheckConfig,
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
        let replace = best
            .as_ref()
            .is_none_or(|current| candidate.trace.len() < current.trace.len());
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
