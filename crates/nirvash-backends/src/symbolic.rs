use std::collections::VecDeque;

use nirvash::{
    BoolExpr, Counterexample, CounterexampleKind, ExplorationMode, Fairness, Ltl, ModelCase,
    ModelCaseSource, ModelCheckConfig, ModelCheckError, ModelCheckResult, ReachableGraphEdge,
    ReachableGraphSnapshot, Signature, StepExpr, SymbolicStateSchema, TemporalSpec, Trace,
    TraceStep, TransitionProgram, UpdateAst, UpdateOp,
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

pub struct SymbolicModelChecker<'a, T: TemporalSpec + ModelCaseSource> {
    spec: &'a T,
    model_case: ModelCase<T::State, T::Action>,
    config: ModelCheckConfig,
}

impl<'a, T> SymbolicModelChecker<'a, T>
where
    T: TemporalSpec + ModelCaseSource,
    T::State: PartialEq + Signature + 'static,
    T::Action: PartialEq + 'static,
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

    fn build_relation_reachable_graph(
        &self,
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_constraints_ast_native()?;
        let program = self.symbolic_transition_program()?;
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
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
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
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
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_temporal(&schema)?;
        let graph = self.build_relation_reachable_graph(self.config)?;
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
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
        let graph = self.build_relation_reachable_graph(self.config)?;
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
        let graph = self.build_relation_reachable_graph(self.config)?;
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
        let schema = self.symbolic_state_schema()?;
        self.ensure_symbolic_schema_covers_temporal(&schema)?;
        let graph = self.build_relation_reachable_graph(self.config)?;
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

    fn relation_successors(
        &self,
        state: &T::State,
    ) -> Result<Vec<(TraceStep<T::Action>, T::State)>, ModelCheckError> {
        let program = self.symbolic_transition_program()?;
        let mut values = Vec::new();

        for action in self.spec.actions() {
            for successor in program.successors(state, &action) {
                let next_concrete = successor.into_next();
                if !self.action_constraints_allow(state, &action, &next_concrete) {
                    continue;
                }
                if !self.state_constraints_allow(&next_concrete) {
                    continue;
                }
                let edge = (TraceStep::Action(action.clone()), next_concrete);
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
                let edge = (TraceStep::Stutter, stutter);
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
        if let Some(effect_name) = program.effect_names().first() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic reachable-graph backend does not encode update effect `{}` in transition program `{}` for spec `{}`",
                effect_name,
                program.name(),
                self.spec.name(),
            )));
        }
        self.ensure_symbolic_schema_covers_paths(
            schema,
            format!("transition program `{}`", program.name()),
            program.symbolic_state_paths(),
        )?;
        for rule in program.rules() {
            let Some(update) = rule.update_ast() else {
                continue;
            };
            self.ensure_symbolic_schema_covers_update(schema, update)?;
        }
        Ok(())
    }

    fn ensure_symbolic_schema_covers_model_case_constraints(
        &self,
        schema: &SymbolicStateSchema<T::State>,
    ) -> Result<(), ModelCheckError> {
        for constraint in self.model_case.state_constraints() {
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend requires state constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("state constraint `{}`", constraint.name()),
                constraint.symbolic_state_paths(),
            )?;
        }
        for constraint in self.model_case.action_constraints() {
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend requires action constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("action constraint `{}`", constraint.name()),
                constraint.symbolic_state_paths(),
            )?;
        }
        Ok(())
    }

    fn ensure_symbolic_schema_covers_invariants(
        &self,
        schema: &SymbolicStateSchema<T::State>,
    ) -> Result<(), ModelCheckError> {
        for invariant in self.spec.invariants() {
            if let Some(node) = invariant.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend requires invariant `{}` for spec `{}` to register helper `{}` for symbolic use",
                    invariant.name(),
                    self.spec.name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("invariant `{}`", invariant.name()),
                invariant.symbolic_state_paths(),
            )?;
        }
        Ok(())
    }

    fn ensure_symbolic_schema_covers_temporal(
        &self,
        schema: &SymbolicStateSchema<T::State>,
    ) -> Result<(), ModelCheckError> {
        for property in self.spec.properties() {
            if let Some(node) = property.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to register helper `{}` for symbolic use",
                    property.describe(),
                    self.spec.name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("property `{}`", property.describe()),
                property.symbolic_state_paths(),
            )?;
        }
        for fairness in self.spec.fairness() {
            if let Some(node) = fairness.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to register helper `{}` for symbolic use",
                    fairness.name(),
                    self.spec.name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("fairness `{}`", fairness.name()),
                fairness.symbolic_state_paths(),
            )?;
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

    fn ensure_symbolic_schema_covers_paths<I>(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        context: String,
        paths: I,
    ) -> Result<(), ModelCheckError>
    where
        I: IntoIterator<Item = &'static str>,
    {
        for path in paths {
            if !schema.has_path(path) {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires state schema for `{}` to expose field `{}` referenced by {}",
                    std::any::type_name::<T::State>(),
                    path,
                    context,
                )));
            }
        }
        Ok(())
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
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires state constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.name(),
                    node,
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
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires action constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.name(),
                    node,
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
            if let Some(node) = invariant.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires invariant `{}` for spec `{}` to register helper `{}` for symbolic use",
                    invariant.name(),
                    self.spec.name(),
                    node,
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
            if let Some(node) = property.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to register helper `{}` for symbolic use",
                    property.describe(),
                    self.spec.name(),
                    node,
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
            if let Some(node) = fairness.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to register helper `{}` for symbolic use",
                    fairness.name(),
                    self.spec.name(),
                    node,
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
                    self.relation_successors(state)
                        .unwrap_or_else(|error| {
                            panic!("symbolic graph successor enumeration failed: {error:?}")
                        })
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
