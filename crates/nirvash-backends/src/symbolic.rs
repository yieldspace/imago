use std::collections::VecDeque;

use nirvash::{
    BoolExpr, Counterexample, CounterexampleKind, ExplorationMode, Fairness, Ltl, ModelCase,
    ModelCaseSource, ModelCheckConfig, ModelCheckError, ModelCheckResult, ReachableGraphEdge,
    ReachableGraphSnapshot, Signature, StepExpr, SymbolicStateSchema, TemporalSpec, Trace,
    TraceStep, TransitionProgram, UpdateAst, UpdateOp,
};

use z3::{
    SatResult, Solver,
    ast::{Bool, Int},
};

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

impl<S, A> ReachableGraph<S, A> {
    fn state_index(&self, state: &S) -> Option<usize>
    where
        S: PartialEq,
    {
        self.states.iter().position(|candidate| candidate == state)
    }
}

#[derive(Debug, Clone)]
struct CandidateGraph<S, A> {
    states: Vec<S>,
    edges: Vec<Vec<GraphEdge<A>>>,
    initial_indices: Vec<usize>,
}

impl<S, A> CandidateGraph<S, A> {
    fn state_index(&self, state: &S) -> Option<usize>
    where
        S: PartialEq,
    {
        self.states.iter().position(|candidate| candidate == state)
    }
}

pub struct SymbolicModelChecker<'a, T: TemporalSpec + ModelCaseSource> {
    spec: &'a T,
    model_case: ModelCase<T::State, T::Action>,
    config: ModelCheckConfig,
}

impl<'a, T> SymbolicModelChecker<'a, T>
where
    T: TemporalSpec + ModelCaseSource,
    T::State: PartialEq + Signature,
    T::Action: PartialEq,
{
    pub fn for_case(spec: &'a T, model_case: ModelCase<T::State, T::Action>) -> Self {
        let config = model_case.effective_checker_config();
        Self {
            spec,
            model_case,
            config,
        }
    }

    pub fn reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_relation_reachable_graph(self.doc_reachable_graph_config())?;
        Ok(self.snapshot_from_graph(&graph))
    }

    pub fn full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_relation_reachable_graph(self.config)?;
        self.ensure_untruncated(&graph)?;
        Ok(self.snapshot_from_graph(&graph))
    }

    pub fn check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_invariants_ast_native()?;
        match self.config.exploration {
            ExplorationMode::ReachableGraph => self.check_invariants_graph(),
            ExplorationMode::BoundedLasso => self.check_invariants_lasso(),
        }
    }

    pub fn check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        if !self.config.check_deadlocks {
            return Ok(ModelCheckResult::ok());
        }
        match self.config.exploration {
            ExplorationMode::ReachableGraph => self.check_deadlocks_graph(),
            ExplorationMode::BoundedLasso => self.check_deadlocks_lasso(),
        }
    }

    pub fn check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_properties_ast_native()?;
        if self.spec.properties().is_empty() {
            return Ok(ModelCheckResult::ok());
        }

        if self.model_case.symmetry().is_some()
            && (!self.spec.properties().is_empty() || !self.spec.fairness().is_empty())
        {
            return Err(ModelCheckError::UnsupportedConfiguration(
                "symmetry reduction cannot be combined with temporal properties or fairness",
            ));
        }
        match self.config.exploration {
            ExplorationMode::ReachableGraph => self.check_properties_graph(),
            ExplorationMode::BoundedLasso => self.check_properties_lasso(),
        }
    }

    pub fn check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
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

    fn build_reachable_graph(
        &self,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        let candidate = self.build_candidate_graph()?;
        let reachable = self.solve_reachability(&candidate)?;
        self.materialize_reachable_graph(&candidate, &reachable, self.config)
    }

    fn build_relation_reachable_graph(
        &self,
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_constraints_ast_native()?;
        let program = self.symbolic_transition_program()?;
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        if self.model_case.symmetry().is_some() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic reachable-graph backend does not support symmetry reduction for spec `{}`",
                self.spec.name(),
            )));
        }

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

        for state in self.initial_states_filtered()? {
            let Some(index) = self.push_state(&mut graph, state, None, 0, &mut queue, config)?
            else {
                break;
            };
            if !graph.initial_indices.contains(&index) {
                graph.initial_indices.push(index);
            }
        }

        while let Some(index) = queue.pop_front() {
            if graph.truncated {
                break;
            }

            let current = graph.states[index].clone();
            let next_depth = graph.depths[index] + 1;
            let mut edges = Vec::new();

            for (step, next_state) in self.relation_successors(&current)? {
                let Some(next_index) = self.push_state(
                    &mut graph,
                    next_state,
                    Some((index, step.clone())),
                    next_depth,
                    &mut queue,
                    config,
                )?
                else {
                    break;
                };

                let materialized = GraphEdge {
                    step,
                    target: next_index,
                };
                if !edges.contains(&materialized) {
                    if !materialized.is_stutter() {
                        if self.transition_limit_reached(&graph, config) {
                            graph.truncated = true;
                            break;
                        }
                        graph.transitions += 1;
                    }
                    edges.push(materialized);
                }
            }

            if !graph.truncated && edges.iter().all(GraphEdge::is_stutter) {
                graph.deadlocks.push(index);
            }

            graph.edges[index] = edges;
        }

        Ok(graph)
    }

    fn doc_reachable_graph_config(&self) -> ModelCheckConfig {
        let mut config = self.model_case.doc_checker_config().unwrap_or(self.config);
        config.exploration = ExplorationMode::ReachableGraph;
        config.stop_on_first_violation = false;
        config
    }

    fn check_invariants_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_relation_reachable_graph(self.config)?;
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
        let graph = self.build_relation_reachable_graph(self.config)?;
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
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        let mut best = None;
        for &initial in &graph.initial_indices {
            self.search_invariants_lasso_graph(&graph, vec![initial], Vec::new(), &mut best);
        }
        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn check_deadlocks_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        let mut best = None;
        for &initial in &graph.initial_indices {
            self.search_deadlocks_lasso_graph(&graph, vec![initial], Vec::new(), &mut best);
        }
        Ok(best.map_or_else(ModelCheckResult::ok, ModelCheckResult::with_violation))
    }

    fn check_properties_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_reachable_graph()?;
        self.ensure_untruncated(&graph)?;
        let traces = self.bounded_graph_lasso_traces(&graph);
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

    fn build_candidate_graph(
        &self,
    ) -> Result<CandidateGraph<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_constraints_ast_native()?;
        let _ = self.symbolic_transition_program()?;
        let mut states = Vec::new();
        for state in T::State::bounded_domain().into_vec() {
            let canonical = self.canonicalize_state(&state);
            if !self.state_constraints_allow(&canonical) || states.contains(&canonical) {
                continue;
            }
            states.push(canonical);
        }

        let initial_states = self.initial_states_filtered()?;
        let mut initial_indices = Vec::new();
        for state in initial_states {
            let Some(index) = states.iter().position(|candidate| candidate == &state) else {
                return Err(self.symbolic_missing_initial_error(&state));
            };
            if !initial_indices.contains(&index) {
                initial_indices.push(index);
            }
        }

        let mut edges = Vec::with_capacity(states.len());
        for state in &states {
            edges.push(self.candidate_edges_for_state(state, &states)?);
        }

        Ok(CandidateGraph {
            states,
            edges,
            initial_indices,
        })
    }

    fn candidate_edges_for_state(
        &self,
        state: &T::State,
        domain_states: &[T::State],
    ) -> Result<Vec<GraphEdge<T::Action>>, ModelCheckError> {
        let mut edges = Vec::new();
        for (action, next_raw) in self.ast_transition_successors(state)? {
            let next = self.canonicalize_state(&next_raw);
            if !self.state_constraints_allow(&next) {
                continue;
            }
            let Some(target) = domain_states
                .iter()
                .position(|candidate| candidate == &next)
            else {
                return Err(self.symbolic_missing_successor_error(state, &action, &next));
            };
            let edge = GraphEdge {
                step: TraceStep::Action(action),
                target,
            };
            if !edges.contains(&edge) {
                edges.push(edge);
            }
        }

        if self.spec.allow_stutter() {
            let stutter = self.canonicalize_state(&self.spec.stutter_state(state));
            if self.state_constraints_allow(&stutter) {
                let Some(target) = domain_states
                    .iter()
                    .position(|candidate| candidate == &stutter)
                else {
                    return Err(self.symbolic_missing_stutter_error(state, &stutter));
                };
                let edge = GraphEdge {
                    step: TraceStep::Stutter,
                    target,
                };
                if !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
        }

        Ok(edges)
    }

    fn solve_reachability(
        &self,
        candidate: &CandidateGraph<T::State, T::Action>,
    ) -> Result<Vec<bool>, ModelCheckError> {
        if candidate.initial_indices.is_empty() {
            return Err(ModelCheckError::NoInitialStates);
        }

        let state_count = candidate.states.len();
        let depth_bound = state_count.saturating_sub(1);
        let predecessors = self.predecessor_indices(candidate);
        let solver = Solver::new();
        let mut reach = Vec::with_capacity(depth_bound + 1);

        for depth in 0..=depth_bound {
            let mut layer = Vec::with_capacity(state_count);
            for index in 0..state_count {
                layer.push(Bool::new_const(format!("reachable_{depth}_{index}")));
            }
            reach.push(layer);
        }

        for index in 0..state_count {
            let initial = Bool::from_bool(candidate.initial_indices.contains(&index));
            let formula = reach[0][index].eq(&initial);
            solver.assert(&formula);
        }

        for depth in 1..=depth_bound {
            for index in 0..state_count {
                let mut disjuncts = Vec::with_capacity(predecessors[index].len() + 1);
                disjuncts.push(reach[depth - 1][index].clone());
                for &predecessor in &predecessors[index] {
                    disjuncts.push(reach[depth - 1][predecessor].clone());
                }
                let rhs = bool_or(&disjuncts);
                let formula = reach[depth][index].eq(&rhs);
                solver.assert(&formula);
            }
        }

        match solver.check() {
            SatResult::Sat => {}
            SatResult::Unsat | SatResult::Unknown => {
                return Err(ModelCheckError::UnsupportedConfiguration(
                    "symbolic reachability encoding did not yield a satisfiable model",
                ));
            }
        }

        let Some(model) = solver.get_model() else {
            return Err(ModelCheckError::UnsupportedConfiguration(
                "symbolic backend could not obtain a model from z3",
            ));
        };

        let final_layer = &reach[depth_bound];
        let mut reachable = Vec::with_capacity(state_count);
        for formula in final_layer {
            let value = model
                .eval(formula, true)
                .and_then(|ast| ast.as_bool())
                .unwrap_or(false);
            reachable.push(value);
        }
        Ok(reachable)
    }

    fn predecessor_indices(
        &self,
        candidate: &CandidateGraph<T::State, T::Action>,
    ) -> Vec<Vec<usize>> {
        let mut predecessors = vec![Vec::new(); candidate.states.len()];
        for (source, edges) in candidate.edges.iter().enumerate() {
            for edge in edges {
                if !predecessors[edge.target].contains(&source) {
                    predecessors[edge.target].push(source);
                }
            }
        }
        predecessors
    }

    fn materialize_reachable_graph(
        &self,
        candidate: &CandidateGraph<T::State, T::Action>,
        reachable: &[bool],
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
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

        for &candidate_index in &candidate.initial_indices {
            if !reachable[candidate_index] {
                continue;
            }
            let state = candidate.states[candidate_index].clone();
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
            let current_candidate_index = candidate
                .state_index(&current)
                .expect("materialized graph state must exist in candidate graph");
            let mut edges = Vec::new();

            for edge in &candidate.edges[current_candidate_index] {
                if !reachable[edge.target] {
                    continue;
                }

                let next_state = candidate.states[edge.target].clone();
                let Some(next_index) = self.push_state(
                    &mut graph,
                    next_state,
                    Some((index, edge.step.clone())),
                    next_depth,
                    &mut queue,
                    config,
                )?
                else {
                    break;
                };

                let materialized = GraphEdge {
                    step: edge.step.clone(),
                    target: next_index,
                };
                if !edges.contains(&materialized) {
                    if !materialized.is_stutter() {
                        if self.transition_limit_reached(&graph, config) {
                            graph.truncated = true;
                            break;
                        }
                        graph.transitions += 1;
                    }
                    edges.push(materialized);
                }
            }

            if !graph.truncated && edges.iter().all(GraphEdge::is_stutter) {
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

    fn ast_transition_successors(
        &self,
        state: &T::State,
    ) -> Result<Vec<(T::Action, T::State)>, ModelCheckError> {
        let program = self.symbolic_transition_program()?;
        let mut values = Vec::new();
        for action in self.spec.actions() {
            if values.iter().any(|(candidate, _)| candidate == &action) {
                continue;
            }
            for successor in program.successors(state, &action) {
                let next = successor.into_next();
                if !self.action_constraints_allow(state, &action, &next) {
                    continue;
                }
                let edge = (action.clone(), next);
                if !values.contains(&edge) {
                    values.push(edge);
                }
            }
        }
        Ok(values)
    }

    fn relation_successors(
        &self,
        state: &T::State,
    ) -> Result<Vec<(TraceStep<T::Action>, T::State)>, ModelCheckError> {
        let schema = self.symbolic_state_schema()?;
        let program = self.symbolic_transition_program()?;
        let mut values = Vec::new();

        for action in self.spec.actions() {
            for successor in program.successors(state, &action) {
                let next_concrete = successor.into_next();
                if !self.action_constraints_allow(state, &action, &next_concrete) {
                    continue;
                }
                let next =
                    self.solve_concrete_successor(&schema, &next_concrete, program.name())?;
                if !self.state_constraints_allow(&next) {
                    continue;
                }
                let edge = (TraceStep::Action(action.clone()), next);
                if !values.contains(&edge) {
                    values.push(edge);
                }
            }
        }

        if self.spec.allow_stutter() {
            let stutter = self.spec.stutter_state(state);
            if stutter != *state {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend requires spec `{}` stutter_state() to be identity",
                    self.spec.name(),
                )));
            }
            if self.state_constraints_allow(&stutter) {
                let next = self.solve_concrete_successor(&schema, &stutter, "stutter")?;
                let edge = (TraceStep::Stutter, next);
                if !values.contains(&edge) {
                    values.push(edge);
                }
            }
        }

        Ok(values)
    }

    fn symbolic_transition_program(
        &self,
    ) -> Result<TransitionProgram<T::State, T::Action>, ModelCheckError> {
        let Some(program) = self.spec.transition_program() else {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires spec `{}` to implement transition_program() with AST-native rules",
                self.spec.name(),
            )));
        };
        if !program.is_ast_native() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires spec `{}` transition program `{}` to be AST-native",
                self.spec.name(),
                program.name(),
            )));
        }
        if let Some(node) = program.first_unencodable_symbolic_node() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires transition program `{}` for spec `{}` to register helper/effect `{}` for symbolic use",
                program.name(),
                self.spec.name(),
                node,
            )));
        }
        Ok(program)
    }

    fn symbolic_state_schema(&self) -> Result<SymbolicStateSchema<T::State>, ModelCheckError> {
        nirvash::registry::lookup_symbolic_state_schema::<T::State>().ok_or_else(|| {
            self.symbolic_ast_required_error(format!(
                "symbolic backend requires state `{}` to implement SymbolicStateSpec",
                std::any::type_name::<T::State>(),
            ))
        })
    }

    fn ensure_symbolic_schema_covers_program(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
    ) -> Result<(), ModelCheckError> {
        for rule in program.rules() {
            let Some(update) = rule.update_ast() else {
                continue;
            };
            self.ensure_symbolic_schema_covers_update(schema, update)?;
        }
        Ok(())
    }

    fn ensure_symbolic_schema_covers_update(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        update: &UpdateAst<T::State, T::Action>,
    ) -> Result<(), ModelCheckError> {
        let UpdateAst::Sequence(ops) = update;
        for op in ops {
            let target = match op {
                UpdateOp::Assign { target, .. }
                | UpdateOp::SetInsert { target, .. }
                | UpdateOp::SetRemove { target, .. } => *target,
                UpdateOp::Effect { .. } => continue,
            };
            if target != "self" && !schema.has_path(target) {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires state schema for `{}` to expose field `{}`",
                    std::any::type_name::<T::State>(),
                    target,
                )));
            }
        }
        Ok(())
    }

    fn solve_concrete_successor(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        concrete: &T::State,
        relation_name: &'static str,
    ) -> Result<T::State, ModelCheckError> {
        let solver = Solver::new();
        let indices = schema.read_indices(concrete);
        let mut vars = Vec::with_capacity(schema.fields().len());

        for (field_index, field) in schema.fields().iter().enumerate() {
            let var = Int::new_const(format!("next_{}_{}", field_index, field.path()));
            solver.assert(var.ge(Int::from_i64(0)));
            solver.assert(var.lt(Int::from_i64(field.domain_size() as i64)));
            solver.assert(var.eq(Int::from_i64(indices[field_index] as i64)));
            vars.push(var);
        }

        match solver.check() {
            SatResult::Sat => {}
            SatResult::Unsat | SatResult::Unknown => {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend could not encode relation `{}` for spec `{}`",
                    relation_name,
                    self.spec.name(),
                )));
            }
        }

        let Some(model) = solver.get_model() else {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic reachable-graph backend could not obtain a z3 model for relation `{}` in spec `{}`",
                relation_name,
                self.spec.name(),
            )));
        };

        let mut rebuilt_indices = Vec::with_capacity(vars.len());
        for (field, var) in schema.fields().iter().zip(&vars) {
            let value = model
                .eval(var, true)
                .and_then(|ast| ast.as_i64())
                .ok_or_else(|| {
                    self.symbolic_ast_required_error(format!(
                        "symbolic reachable-graph backend could not read field `{}` from z3 model",
                        field.path(),
                    ))
                })?;
            rebuilt_indices.push(value as usize);
        }

        Ok(schema.rebuild_from_indices(&rebuilt_indices))
    }

    fn ensure_symbolic_constraints_ast_native(&self) -> Result<(), ModelCheckError> {
        for constraint in self.model_case.state_constraints() {
            if !constraint.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires state constraint `{}` for spec `{}` to be AST-native",
                    constraint.name(),
                    self.spec.name(),
                )));
            }
        }
        for constraint in self.model_case.action_constraints() {
            if !constraint.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires action constraint `{}` for spec `{}` to be AST-native",
                    constraint.name(),
                    self.spec.name(),
                )));
            }
        }
        Ok(())
    }

    fn ensure_symbolic_invariants_ast_native(&self) -> Result<(), ModelCheckError> {
        for invariant in self.spec.invariants() {
            if !invariant.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires invariant `{}` for spec `{}` to be AST-native",
                    invariant.name(),
                    self.spec.name(),
                )));
            }
        }
        Ok(())
    }

    fn ensure_symbolic_properties_ast_native(&self) -> Result<(), ModelCheckError> {
        for property in self.spec.properties() {
            if !property.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to be AST-native",
                    property.describe(),
                    self.spec.name(),
                )));
            }
        }
        for fairness in self.spec.fairness() {
            if !fairness.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to be AST-native",
                    fairness.name(),
                    self.spec.name(),
                )));
            }
        }
        Ok(())
    }

    fn symbolic_ast_required_error(&self, message: String) -> ModelCheckError {
        ModelCheckError::UnsupportedConfiguration(Box::leak(message.into_boxed_str()))
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
        let successors = self
            .ast_transition_successors(state)
            .unwrap_or_else(|error| {
                panic!("symbolic constrained successor enumeration failed: {error:?}")
            });
        for (action, next_raw) in successors {
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

    fn search_invariants_lasso_graph(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        path_states: Vec<usize>,
        path_steps: Vec<TraceStep<T::Action>>,
        best: &mut Option<Counterexample<T::State, T::Action>>,
    ) {
        let depth = path_steps.len();
        let current = *path_states.last().expect("path has at least one state");

        for predicate in self.spec.invariants() {
            if !predicate.eval(&graph.states[current]) {
                let states = path_states
                    .iter()
                    .map(|index| graph.states[*index].clone())
                    .collect();
                self.consider_violation(
                    best,
                    Counterexample {
                        kind: CounterexampleKind::Invariant,
                        name: predicate.name().to_owned(),
                        trace: self.terminal_trace(states, path_steps.clone()),
                    },
                );
                return;
            }
        }

        if self.reached_bounded_depth(depth) {
            return;
        }

        for edge in &graph.edges[current] {
            let mut next_states = path_states.clone();
            next_states.push(edge.target);
            let mut next_steps = path_steps.clone();
            next_steps.push(edge.step.clone());
            self.search_invariants_lasso_graph(graph, next_states, next_steps, best);
        }
    }

    fn search_deadlocks_lasso_graph(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        path_states: Vec<usize>,
        path_steps: Vec<TraceStep<T::Action>>,
        best: &mut Option<Counterexample<T::State, T::Action>>,
    ) {
        let depth = path_steps.len();
        let current = *path_states.last().expect("path has at least one state");

        let has_non_stutter = graph.edges[current]
            .iter()
            .any(|edge| matches!(edge.step, TraceStep::Action(_)));
        if !has_non_stutter {
            let states = path_states
                .iter()
                .map(|index| graph.states[*index].clone())
                .collect();
            self.consider_violation(
                best,
                Counterexample {
                    kind: CounterexampleKind::Deadlock,
                    name: "deadlock".to_owned(),
                    trace: self.terminal_trace(states, path_steps.clone()),
                },
            );
            return;
        }

        if self.reached_bounded_depth(depth) {
            return;
        }

        for edge in &graph.edges[current] {
            let mut next_states = path_states.clone();
            next_states.push(edge.target);
            let mut next_steps = path_steps.clone();
            next_steps.push(edge.step.clone());
            self.search_deadlocks_lasso_graph(graph, next_states, next_steps, best);
        }
    }

    fn graph_lasso_traces(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> Vec<Trace<T::State, T::Action>> {
        let mut traces = Vec::new();
        for &initial in &graph.initial_indices {
            self.enumerate_graph_lassos(graph, vec![initial], Vec::new(), &mut traces);
        }
        traces
    }

    fn bounded_graph_lasso_traces(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> Vec<Trace<T::State, T::Action>> {
        let mut traces = Vec::new();
        for &initial in &graph.initial_indices {
            self.enumerate_bounded_graph_lassos(graph, vec![initial], Vec::new(), &mut traces);
        }
        traces
    }

    fn enumerate_graph_lassos(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        path_states: Vec<usize>,
        path_steps: Vec<TraceStep<T::Action>>,
        traces: &mut Vec<Trace<T::State, T::Action>>,
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

            let mut next_states = path_states.clone();
            next_states.push(edge.target);
            let mut next_steps = path_steps.clone();
            next_steps.push(edge.step.clone());
            self.enumerate_graph_lassos(graph, next_states, next_steps, traces);
        }
    }

    fn enumerate_bounded_graph_lassos(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        path_states: Vec<usize>,
        path_steps: Vec<TraceStep<T::Action>>,
        traces: &mut Vec<Trace<T::State, T::Action>>,
    ) {
        let states = path_states
            .iter()
            .map(|index| graph.states[*index].clone())
            .collect();
        traces.push(self.terminal_trace(states, path_steps.clone()));

        if self.reached_bounded_depth(path_steps.len()) {
            return;
        }

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

            let mut next_states = path_states.clone();
            next_states.push(edge.target);
            let mut next_steps = path_steps.clone();
            next_steps.push(edge.step.clone());
            self.enumerate_bounded_graph_lassos(graph, next_states, next_steps, traces);
        }
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

    fn symbolic_missing_initial_error(&self, state: &T::State) -> ModelCheckError {
        ModelCheckError::UnsupportedConfiguration(Box::leak(
            format!(
                "symbolic backend requires Signature::bounded_domain() to contain all constrained initial, successor, and stutter states; missing initial state: {:?}",
                state
            )
            .into_boxed_str(),
        ))
    }

    fn symbolic_missing_successor_error(
        &self,
        prev: &T::State,
        action: &T::Action,
        next: &T::State,
    ) -> ModelCheckError {
        ModelCheckError::UnsupportedConfiguration(Box::leak(
            format!(
                "symbolic backend requires Signature::bounded_domain() to contain all constrained initial, successor, and stutter states; missing successor: {:?} -- {:?} --> {:?}",
                prev, action, next
            )
            .into_boxed_str(),
        ))
    }

    fn symbolic_missing_stutter_error(&self, prev: &T::State, next: &T::State) -> ModelCheckError {
        ModelCheckError::UnsupportedConfiguration(Box::leak(
            format!(
                "symbolic backend requires Signature::bounded_domain() to contain all constrained initial, successor, and stutter states; missing stutter successor: {:?} -> {:?}",
                prev, next
            )
            .into_boxed_str(),
        ))
    }
}

fn bool_or(values: &[Bool]) -> Bool {
    match values {
        [] => Bool::from_bool(false),
        [single] => single.clone(),
        _ => Bool::or(values),
    }
}
