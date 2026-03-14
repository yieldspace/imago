use std::collections::VecDeque;

use z3::{
    Model, Solver,
    ast::{Bool, Int},
};

use nirvash_lower::{
    BoolExpr, CheckerSpec, Counterexample, CounterexampleKind, ExplorationMode, Fairness, Ltl,
    ModelBackend, ModelCheckConfig, ModelCheckError, ModelCheckResult, ModelInstance,
    ReachableGraphEdge, ReachableGraphSnapshot, StepExpr, SymbolicStateSchema,
    SymbolicSupportIssue, Trace, TraceStep, TransitionProgram, UpdateAst, UpdateOp,
};

use crate::smt::{
    assert_in_domain, block_current_model, bool_and, bool_or, decode_int, encode_rule_transition,
    encode_state_bool, encode_step_bool, solver_is_sat,
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

type TraceList<S, A> = Vec<Trace<S, A>>;
type MaybeTrace<T> = Option<Trace<<T as CheckerSpec>::State, <T as CheckerSpec>::Action>>;
type SymbolicSuccessor<T> = (
    TraceStep<<T as CheckerSpec>::Action>,
    <T as CheckerSpec>::State,
);
type MaybePredecessor<T> = Option<(Vec<usize>, TraceStep<<T as CheckerSpec>::Action>)>;
type MaybeBlockedPath<T> = Option<(Vec<Vec<usize>>, Vec<TraceStep<<T as CheckerSpec>::Action>>)>;

const AUTO_DIRECT_SMT_DEPTH_CAP: usize = 4;

impl<S, A> ReachableGraph<S, A> {
    fn state_index(&self, state: &S) -> Option<usize>
    where
        S: PartialEq,
    {
        self.states.iter().position(|candidate| candidate == state)
    }
}

#[derive(Debug, Clone)]
struct StateVars {
    fields: Vec<Int>,
}

impl StateVars {
    fn new<S>(prefix: &str, schema: &SymbolicStateSchema<S>) -> Self {
        Self {
            fields: schema
                .fields()
                .iter()
                .enumerate()
                .map(|(index, _)| Int::new_const(format!("{prefix}_{index}")))
                .collect(),
        }
    }

    fn assert_domains<S>(&self, solver: &Solver, schema: &SymbolicStateSchema<S>) {
        for (value, field) in self.fields.iter().zip(schema.fields()) {
            assert_in_domain(solver, value, field.domain_size());
        }
    }

    fn fix_to_state<S>(&self, solver: &Solver, schema: &SymbolicStateSchema<S>, state: &S) {
        for (value, field) in self.fields.iter().zip(schema.fields()) {
            solver.assert(value.eq(Int::from_u64(field.read_index(state) as u64)));
        }
    }

    fn decode_indices(&self, model: &Model) -> Option<Vec<usize>> {
        self.fields
            .iter()
            .map(|value| decode_int(model, value))
            .collect::<Option<Vec<_>>>()
    }

    fn decode<S: Clone>(&self, model: &Model, schema: &SymbolicStateSchema<S>) -> Option<S> {
        let indices = self.decode_indices(model)?;
        Some(schema.rebuild_from_indices(&indices))
    }

    fn all_values(&self) -> Vec<Int> {
        self.fields.clone()
    }
}

#[derive(Debug, Clone)]
struct LassoVars {
    states: Vec<StateVars>,
    steps: Vec<Int>,
    loop_start: Int,
    terminal: Int,
}

impl LassoVars {
    fn new<S>(prefix: &str, len: usize, schema: &SymbolicStateSchema<S>) -> Self {
        Self {
            states: (0..len)
                .map(|index| StateVars::new(&format!("{prefix}_state_{index}"), schema))
                .collect(),
            steps: (0..len)
                .map(|index| Int::new_const(format!("{prefix}_step_{index}")))
                .collect(),
            loop_start: Int::new_const(format!("{prefix}_loop_start")),
            terminal: Int::new_const(format!("{prefix}_terminal")),
        }
    }

    fn assert_domains<S>(
        &self,
        solver: &Solver,
        schema: &SymbolicStateSchema<S>,
        step_domain_size: usize,
    ) {
        for state in &self.states {
            state.assert_domains(solver, schema);
        }
        for step in &self.steps {
            assert_in_domain(solver, step, step_domain_size);
        }
        assert_in_domain(solver, &self.loop_start, self.states.len());
        assert_in_domain(solver, &self.terminal, 2);
    }

    fn decode<S: Clone, A: Clone>(
        &self,
        model: &z3::Model,
        schema: &SymbolicStateSchema<S>,
        actions: &[A],
    ) -> Option<Trace<S, A>> {
        let states = self
            .states
            .iter()
            .map(|state| state.decode(model, schema))
            .collect::<Option<Vec<_>>>()?;
        let loop_start = decode_int(model, &self.loop_start)?;
        let terminal = decode_int(model, &self.terminal)? != 0;
        let last_index = self.steps.len() - 1;
        let mut steps = Vec::with_capacity(self.steps.len());
        for (index, step) in self.steps.iter().enumerate() {
            let step_code = decode_int(model, step)?;
            if terminal && index == last_index {
                steps.push(TraceStep::Stutter);
                continue;
            }
            if step_code == 0 {
                steps.push(TraceStep::Stutter);
                continue;
            }
            let action = actions.get(step_code - 1)?.clone();
            steps.push(TraceStep::Action(action));
        }
        Some(Trace::new(states, steps, loop_start))
    }

    fn all_values(&self) -> Vec<Int> {
        let mut values = Vec::new();
        for state in &self.states {
            values.extend(state.all_values());
        }
        values.extend(self.steps.clone());
        values.push(self.loop_start.clone());
        values.push(self.terminal.clone());
        values
    }
}

pub struct SymbolicModelChecker<'a, T: CheckerSpec> {
    spec: &'a T,
    model_case: ModelInstance<T::State, T::Action>,
    config: ModelCheckConfig,
}

impl<'a, T> SymbolicModelChecker<'a, T>
where
    T: CheckerSpec,
    T::State: PartialEq + 'static,
    T::Action: PartialEq + 'static,
{
    pub fn new(spec: &'a T) -> Self {
        let model_case = spec
            .model_instances()
            .into_iter()
            .next()
            .unwrap_or_default();
        Self::for_case(spec, model_case)
    }

    pub fn for_case(spec: &'a T, model_case: ModelInstance<T::State, T::Action>) -> Self {
        let check_deadlocks = model_case.check_deadlocks();
        let config = model_case.checker_config();
        let model_case = model_case
            .with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..config
            })
            .with_check_deadlocks(check_deadlocks)
            .with_resolved_backend(ModelBackend::Symbolic);
        let config = model_case.effective_checker_config();
        Self {
            spec,
            model_case,
            config,
        }
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        let check_deadlocks = config.check_deadlocks;
        let mut model_case = spec
            .model_instances()
            .into_iter()
            .next()
            .unwrap_or_default();
        model_case = model_case
            .with_checker_config(ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..config
            })
            .with_check_deadlocks(check_deadlocks)
            .with_resolved_backend(ModelBackend::Symbolic);
        Self::for_case(spec, model_case)
    }

    pub fn reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.ensure_no_explicit_only_reducers()?;
        let graph = self.build_bridge_reachable_graph(self.doc_reachable_graph_config())?;
        Ok(self.snapshot_from_graph(&graph))
    }

    pub fn full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.ensure_no_explicit_only_reducers()?;
        let graph = self.build_bridge_reachable_graph(self.config.clone())?;
        self.ensure_untruncated(&graph)?;
        Ok(self.snapshot_from_graph(&graph))
    }

    pub fn check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.ensure_no_explicit_only_reducers()?;
        self.ensure_symbolic_invariants_ast_native()?;
        match self.config.symbolic.safety {
            nirvash::SymbolicSafetyEngine::Bmc => self.check_invariants_bmc(),
            nirvash::SymbolicSafetyEngine::KInduction => self.check_invariants_kinduction(),
            nirvash::SymbolicSafetyEngine::PdrIc3 => self.check_invariants_pdr(),
        }
    }

    pub fn check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.ensure_no_explicit_only_reducers()?;
        if !self.config.check_deadlocks {
            return Ok(self.empty_result());
        }
        match self.config.exploration {
            ExplorationMode::ReachableGraph => self.check_deadlocks_bmc(),
            ExplorationMode::BoundedLasso => self.check_deadlocks_lasso(),
        }
    }

    pub fn check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.ensure_no_explicit_only_reducers()?;
        self.ensure_symbolic_properties_ast_native()?;
        if self.direct_properties().is_empty() {
            return Ok(self.empty_result());
        }
        match self.config.symbolic.temporal {
            nirvash::SymbolicTemporalEngine::BoundedLasso => self.check_properties_lasso(),
            nirvash::SymbolicTemporalEngine::LivenessToSafety => {
                self.check_properties_liveness_to_safety()
            }
        }
    }

    pub fn check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let mut result = self.empty_result();

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

    pub fn backend(&self) -> ModelBackend {
        ModelBackend::Symbolic
    }

    pub fn doc_backend(&self) -> ModelBackend {
        ModelBackend::Symbolic
    }

    fn empty_result(&self) -> ModelCheckResult<T::State, T::Action> {
        ModelCheckResult::with_tier(self.model_case.soundness_tier())
    }

    fn build_bridge_reachable_graph(
        &self,
        config: ModelCheckConfig,
    ) -> Result<ReachableGraph<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_constraints_ast_native()?;
        let program = self.direct_transition_program()?;
        let schema = self.direct_state_schema()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        if self.model_case.sound_reduction().is_some() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic reachable-graph backend does not support sound reductions for spec `{}`",
                self.spec.frontend_name(),
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
            let Some(index) = self.push_state(&mut graph, state, None, 0, &mut queue, &config)?
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

            for (step, next_state) in self.enumerate_symbolic_successors(&current)? {
                let Some(next_index) = self.push_state(
                    &mut graph,
                    next_state,
                    Some((index, step.clone())),
                    next_depth,
                    &mut queue,
                    &config,
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
                        if self.transition_limit_reached(&graph, &config) {
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
        let mut config = self
            .model_case
            .doc_checker_config()
            .unwrap_or_else(|| self.config.clone());
        config.exploration = ExplorationMode::ReachableGraph;
        config.stop_on_first_violation = false;
        config
    }

    fn check_invariants_bmc(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        let program = self.direct_transition_program()?;
        let actions = self.action_domain();
        self.ensure_symbolic_stutter_is_identity()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
        let max_depth = self.bmc_max_depth(&schema);

        for predicate in self.direct_invariants() {
            if let Some(trace) = self.find_kinduction_counterexample(
                &schema, &program, &actions, &predicate, max_depth,
            )? {
                return Ok(ModelCheckResult::with_violation(Counterexample {
                    kind: CounterexampleKind::Invariant,
                    name: predicate.name().to_owned(),
                    trace,
                    soundness_tier: self.model_case.soundness_tier(),
                }));
            }
        }

        Ok(self.empty_result())
    }

    fn check_invariants_kinduction(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        let program = self.direct_transition_program()?;
        let actions = self.action_domain();
        self.ensure_symbolic_stutter_is_identity()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
        let max_depth = self.kinduction_max_depth(&schema);

        for predicate in self.direct_invariants() {
            if let Some(trace) = self.find_kinduction_counterexample(
                &schema, &program, &actions, &predicate, max_depth,
            )? {
                return Ok(ModelCheckResult::with_violation(Counterexample {
                    kind: CounterexampleKind::Invariant,
                    name: predicate.name().to_owned(),
                    trace,
                    soundness_tier: self.model_case.soundness_tier(),
                }));
            }
            if self.invariant_is_kinductive(&schema, &program, &actions, &predicate, max_depth)? {
                continue;
            }
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic k-induction did not converge for invariant `{}` in spec `{}` within depth {}",
                predicate.name(),
                self.spec.frontend_name(),
                max_depth,
            )));
        }

        Ok(self.empty_result())
    }

    fn check_invariants_pdr(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        let program = self.direct_transition_program()?;
        let actions = self.action_domain();
        self.ensure_symbolic_stutter_is_identity()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
        let max_frames = self.pdr_max_frames(&schema);

        for predicate in self.direct_invariants() {
            let mut frames: Vec<Vec<Vec<usize>>> = vec![Vec::new(), Vec::new()];
            let mut proved = false;

            for level in 1..=max_frames.max(1) {
                while let Some(target) =
                    self.find_pdr_bad_state(&schema, &predicate, &frames, level)?
                {
                    if let Some((state_keys, steps)) = self.block_pdr_state(
                        &schema,
                        &program,
                        &actions,
                        &mut frames,
                        level,
                        &target,
                    )? {
                        let states = state_keys
                            .iter()
                            .map(|key| schema.rebuild_from_indices(key))
                            .collect::<Vec<_>>();
                        return Ok(ModelCheckResult::with_violation(Counterexample {
                            kind: CounterexampleKind::Invariant,
                            name: predicate.name().to_owned(),
                            trace: self.terminal_trace(states, steps),
                            soundness_tier: self.model_case.soundness_tier(),
                        }));
                    }
                }

                if frames.len() == level + 1 {
                    frames.push(Vec::new());
                }
                self.propagate_pdr_frames(&schema, &program, &actions, &mut frames, level)?;
                if self.frame_sets_equal(&frames[level], &frames[level + 1]) {
                    proved = true;
                    break;
                }
            }

            if !proved {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic PDR/IC3 did not converge for invariant `{}` in spec `{}` within {} frames",
                    predicate.name(),
                    self.spec.frontend_name(),
                    max_frames.max(1),
                )));
            }
        }

        Ok(self.empty_result())
    }

    #[allow(dead_code)]
    fn check_invariants_bridge_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        self.ensure_symbolic_schema_covers_invariants(&schema)?;
        let graph = self.build_bridge_reachable_graph(self.config.clone())?;
        self.ensure_untruncated(&graph)?;
        for (index, state) in graph.states.iter().enumerate() {
            for predicate in self.direct_invariants() {
                if !predicate.eval(state) {
                    return Ok(ModelCheckResult::with_violation(Counterexample {
                        kind: CounterexampleKind::Invariant,
                        name: predicate.name().to_owned(),
                        trace: self.trace_to_state(&graph, index),
                        soundness_tier: self.model_case.soundness_tier(),
                    }));
                }
            }
        }

        Ok(self.empty_result())
    }

    fn check_deadlocks_bmc(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        let program = self.direct_transition_program()?;
        let actions = self.action_domain();
        self.ensure_symbolic_stutter_is_identity()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        let max_depth = self.deadlock_max_depth(&schema);

        if let Some(trace) =
            self.find_deadlock_counterexample(&schema, &program, &actions, max_depth)?
        {
            return Ok(ModelCheckResult::with_violation(Counterexample {
                kind: CounterexampleKind::Deadlock,
                name: "deadlock".to_owned(),
                trace,
                soundness_tier: self.model_case.soundness_tier(),
            }));
        }

        Ok(self.empty_result())
    }

    #[allow(dead_code)]
    fn check_deadlocks_bridge_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let graph = self.build_bridge_reachable_graph(self.config.clone())?;
        self.ensure_untruncated(&graph)?;
        if let Some(deadlock) = graph.deadlocks.first() {
            return Ok(ModelCheckResult::with_violation(Counterexample {
                kind: CounterexampleKind::Deadlock,
                name: "deadlock".to_owned(),
                trace: self.trace_to_state(&graph, *deadlock),
                soundness_tier: self.model_case.soundness_tier(),
            }));
        }

        Ok(self.empty_result())
    }

    #[allow(dead_code)]
    fn check_properties_bridge_graph(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        self.ensure_symbolic_schema_covers_temporal(&schema)?;
        let graph = self.build_bridge_reachable_graph(self.config.clone())?;
        self.ensure_untruncated(&graph)?;
        let traces = self.graph_lasso_traces(&graph);
        let mut best: Option<Counterexample<T::State, T::Action>> = None;

        for property in self.direct_properties() {
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
                            soundness_tier: self.model_case.soundness_tier(),
                        },
                    );
                }
            }
        }

        Ok(best.map_or_else(|| self.empty_result(), ModelCheckResult::with_violation))
    }

    fn check_deadlocks_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let mut best = None;
        for trace in self.collect_direct_lasso_traces()? {
            let state = trace
                .states()
                .last()
                .expect("trace always has at least one state");
            let has_non_stutter = self
                .enumerate_symbolic_successors(state)?
                .into_iter()
                .any(|(step, _)| matches!(step, TraceStep::Action(_)));
            if has_non_stutter {
                continue;
            }
            self.consider_violation(
                &mut best,
                Counterexample {
                    kind: CounterexampleKind::Deadlock,
                    name: "deadlock".to_owned(),
                    trace: self.terminal_trace(
                        trace.states().to_vec(),
                        trace.steps()[..trace.len() - 1].to_vec(),
                    ),
                    soundness_tier: self.model_case.soundness_tier(),
                },
            );
        }
        Ok(best.map_or_else(|| self.empty_result(), ModelCheckResult::with_violation))
    }

    fn check_properties_lasso(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        let schema = self.direct_state_schema()?;
        self.ensure_symbolic_schema_covers_temporal(&schema)?;
        let traces = self.collect_direct_lasso_traces()?;
        let mut best: Option<Counterexample<T::State, T::Action>> = None;

        for property in self.direct_properties() {
            let description = property.describe();
            for trace in &traces {
                if !self.trace_satisfies_fairness_direct(trace) {
                    continue;
                }
                if !self.eval_formula(trace, &property)[0] {
                    self.consider_violation(
                        &mut best,
                        Counterexample {
                            kind: CounterexampleKind::Property,
                            name: description.clone(),
                            trace: trace.clone(),
                            soundness_tier: self.model_case.soundness_tier(),
                        },
                    );
                }
            }
        }

        Ok(best.map_or_else(|| self.empty_result(), ModelCheckResult::with_violation))
    }

    fn check_properties_liveness_to_safety(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.check_properties_lasso()
    }

    fn push_state(
        &self,
        graph: &mut ReachableGraph<T::State, T::Action>,
        state: T::State,
        parent: Option<(usize, TraceStep<T::Action>)>,
        depth: usize,
        queue: &mut VecDeque<usize>,
        config: &ModelCheckConfig,
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
            stutter_omitted: false,
            soundness_tier: self.model_case.soundness_tier(),
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

    fn action_domain(&self) -> Vec<T::Action> {
        self.spec.actions()
    }

    fn step_domain_size(&self) -> usize {
        self.action_domain().len() + 1
    }

    fn ensure_symbolic_stutter_is_identity(&self) -> Result<(), ModelCheckError> {
        Ok(())
    }

    fn encode_state_predicate(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        state: &StateVars,
        predicate: &BoolExpr<T::State>,
    ) -> Bool {
        let full_paths = predicate.symbolic_full_read_paths();
        encode_state_bool(
            schema,
            &state.fields,
            predicate.symbolic_state_paths().as_slice(),
            full_paths.contains(&"state"),
            |value| predicate.eval(value),
        )
    }

    fn encode_step_predicate(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        prev: &StateVars,
        step: &Int,
        next: &StateVars,
        predicate: &StepExpr<T::State, T::Action>,
        actions: &[T::Action],
    ) -> Bool {
        let full_paths = predicate.symbolic_full_read_paths();
        encode_step_bool(
            schema,
            &prev.fields,
            step,
            &next.fields,
            actions,
            predicate.symbolic_state_paths().as_slice(),
            full_paths
                .iter()
                .any(|path| matches!(*path, "prev" | "next")),
            |prev, action, next| predicate.eval(prev, action, next),
        )
    }

    fn symbolic_state_space_bound(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        schema
            .fields()
            .iter()
            .fold(1usize, |acc, field| {
                acc.saturating_mul(field.domain_size().max(1))
            })
            .max(1)
    }

    fn kinduction_max_depth(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        let configured = self.config.symbolic.k_induction.max_depth;
        if configured != 0 {
            return configured;
        }
        self.symbolic_state_space_bound(schema).saturating_sub(1)
    }

    fn bmc_max_depth(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        self.config.bounded_depth.unwrap_or_else(|| {
            self.symbolic_state_space_bound(schema)
                .saturating_sub(1)
                .min(AUTO_DIRECT_SMT_DEPTH_CAP)
        })
    }

    fn deadlock_max_depth(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        self.config.bounded_depth.unwrap_or_else(|| {
            self.symbolic_state_space_bound(schema)
                .saturating_sub(1)
                .min(AUTO_DIRECT_SMT_DEPTH_CAP)
        })
    }

    fn temporal_max_depth(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        self.config.bounded_depth.unwrap_or_else(|| {
            self.symbolic_state_space_bound(schema)
                .min(AUTO_DIRECT_SMT_DEPTH_CAP)
        })
    }

    fn pdr_max_frames(&self, schema: &SymbolicStateSchema<T::State>) -> usize {
        let configured = self.config.symbolic.pdr.max_frames;
        if configured != 0 {
            return configured;
        }
        self.symbolic_state_space_bound(schema)
    }

    fn encode_state_key_eq(&self, state: &StateVars, key: &[usize]) -> Bool {
        bool_and(
            &state
                .fields
                .iter()
                .zip(key.iter().copied())
                .map(|(field, index)| field.eq(Int::from_u64(index as u64)))
                .collect::<Vec<_>>(),
        )
    }

    fn encode_state_key_neq(&self, state: &StateVars, key: &[usize]) -> Bool {
        bool_or(
            &state
                .fields
                .iter()
                .zip(key.iter().copied())
                .map(|(field, index)| field.eq(Int::from_u64(index as u64)).not())
                .collect::<Vec<_>>(),
        )
    }

    fn encode_states_not_equal(&self, lhs: &StateVars, rhs: &StateVars) -> Bool {
        bool_or(
            &lhs.fields
                .iter()
                .zip(rhs.fields.iter())
                .map(|(lhs, rhs)| lhs.eq(rhs).not())
                .collect::<Vec<_>>(),
        )
    }

    fn decode_path_trace(
        &self,
        model: &Model,
        schema: &SymbolicStateSchema<T::State>,
        states: &[StateVars],
        steps: &[Int],
        actions: &[T::Action],
    ) -> Option<Trace<T::State, T::Action>> {
        let states = states
            .iter()
            .map(|state| state.decode(model, schema))
            .collect::<Option<Vec<_>>>()?;
        let mut trace_steps = Vec::with_capacity(steps.len());
        for step in steps {
            let step_code = decode_int(model, step)?;
            if step_code == 0 {
                trace_steps.push(TraceStep::Stutter);
                continue;
            }
            trace_steps.push(TraceStep::Action(actions.get(step_code - 1)?.clone()));
        }
        Some(self.terminal_trace(states, trace_steps))
    }

    fn find_kinduction_counterexample(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        predicate: &BoolExpr<T::State>,
        max_depth: usize,
    ) -> Result<MaybeTrace<T>, ModelCheckError> {
        for depth in 0..=max_depth {
            let states = (0..=depth)
                .map(|index| {
                    StateVars::new(&format!("kinduction_base_state_{depth}_{index}"), schema)
                })
                .collect::<Vec<_>>();
            let steps = (0..depth)
                .map(|index| Int::new_const(format!("kinduction_base_step_{depth}_{index}")))
                .collect::<Vec<_>>();
            let solver = Solver::new();

            for state in &states {
                state.assert_domains(&solver, schema);
                self.assert_state_constraints(&solver, schema, state);
            }
            for step in &steps {
                assert_in_domain(&solver, step, self.step_domain_size());
            }
            solver.assert(self.encode_initial_state_formula(schema, &states[0])?);
            for index in 0..depth {
                solver.assert(self.encode_transition_formula(
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    program,
                    actions,
                ));
                self.assert_action_constraints(
                    &solver,
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    actions,
                );
            }
            for state in states.iter().take(depth) {
                solver.assert(self.encode_state_predicate(schema, state, predicate));
            }
            solver.assert(
                self.encode_state_predicate(schema, &states[depth], predicate)
                    .not(),
            );

            while solver_is_sat(&solver) {
                let Some(model) = solver.get_model() else {
                    break;
                };
                let Some(trace) = self.decode_path_trace(&model, schema, &states, &steps, actions)
                else {
                    break;
                };
                let final_state = trace
                    .states()
                    .last()
                    .expect("trace always has at least one state");
                if !predicate.eval(final_state) {
                    return Ok(Some(trace));
                }
                block_current_model(&solver, &model, &states[depth].all_values());
            }
        }
        Ok(None)
    }

    fn find_deadlock_counterexample(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        max_depth: usize,
    ) -> Result<MaybeTrace<T>, ModelCheckError> {
        for depth in 0..=max_depth {
            let states = (0..=depth)
                .map(|index| StateVars::new(&format!("deadlock_state_{depth}_{index}"), schema))
                .collect::<Vec<_>>();
            let steps = (0..depth)
                .map(|index| Int::new_const(format!("deadlock_step_{depth}_{index}")))
                .collect::<Vec<_>>();
            let solver = Solver::new();

            for state in &states {
                state.assert_domains(&solver, schema);
                self.assert_state_constraints(&solver, schema, state);
            }
            for step in &steps {
                assert_in_domain(&solver, step, self.step_domain_size());
            }
            solver.assert(self.encode_initial_state_formula(schema, &states[0])?);
            for index in 0..depth {
                solver.assert(self.encode_transition_formula(
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    program,
                    actions,
                ));
                self.assert_action_constraints(
                    &solver,
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    actions,
                );
            }

            while solver_is_sat(&solver) {
                let Some(model) = solver.get_model() else {
                    break;
                };
                let Some(trace) = self.decode_path_trace(&model, schema, &states, &steps, actions)
                else {
                    break;
                };
                let terminal_state = trace
                    .states()
                    .last()
                    .expect("trace always has at least one state");
                let has_non_stutter = self
                    .enumerate_symbolic_successors_with_program(schema, terminal_state, program)?
                    .into_iter()
                    .any(|(step, _)| matches!(step, TraceStep::Action(_)));
                if !has_non_stutter {
                    return Ok(Some(trace));
                }
                block_current_model(&solver, &model, &states[depth].all_values());
            }
        }

        Ok(None)
    }

    fn invariant_is_kinductive(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        predicate: &BoolExpr<T::State>,
        max_depth: usize,
    ) -> Result<bool, ModelCheckError> {
        for depth in 0..=max_depth {
            let states = (0..=(depth + 1))
                .map(|index| {
                    StateVars::new(&format!("kinduction_step_state_{depth}_{index}"), schema)
                })
                .collect::<Vec<_>>();
            let steps = (0..=depth)
                .map(|index| Int::new_const(format!("kinduction_step_{depth}_{index}")))
                .collect::<Vec<_>>();
            let solver = Solver::new();

            for state in &states {
                state.assert_domains(&solver, schema);
                self.assert_state_constraints(&solver, schema, state);
            }
            for step in &steps {
                assert_in_domain(&solver, step, self.step_domain_size());
            }
            for index in 0..=depth {
                solver.assert(self.encode_transition_formula(
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    program,
                    actions,
                ));
                self.assert_action_constraints(
                    &solver,
                    schema,
                    &states[index],
                    &steps[index],
                    &states[index + 1],
                    actions,
                );
                solver.assert(self.encode_state_predicate(schema, &states[index], predicate));
            }
            solver.assert(
                self.encode_state_predicate(schema, &states[depth + 1], predicate)
                    .not(),
            );
            for lhs in 0..=depth {
                for rhs in (lhs + 1)..=depth {
                    solver.assert(self.encode_states_not_equal(&states[lhs], &states[rhs]));
                }
            }

            if !solver_is_sat(&solver) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn frame_contains_key(&self, frame: &[Vec<usize>], key: &[usize]) -> bool {
        frame.iter().any(|candidate| candidate.as_slice() == key)
    }

    fn add_frame_key(&self, frame: &mut Vec<Vec<usize>>, key: &[usize]) {
        if !self.frame_contains_key(frame, key) {
            frame.push(key.to_vec());
        }
    }

    fn frame_sets_equal(&self, lhs: &[Vec<usize>], rhs: &[Vec<usize>]) -> bool {
        lhs.len() == rhs.len()
            && lhs
                .iter()
                .all(|candidate| self.frame_contains_key(rhs, candidate))
    }

    fn assert_pdr_frame(
        &self,
        solver: &Solver,
        schema: &SymbolicStateSchema<T::State>,
        state: &StateVars,
        frames: &[Vec<Vec<usize>>],
        level: usize,
    ) -> Result<(), ModelCheckError> {
        if level == 0 {
            solver.assert(self.encode_initial_state_formula(schema, state)?);
        } else {
            for key in &frames[level] {
                solver.assert(self.encode_state_key_neq(state, key));
            }
        }
        self.assert_state_constraints(solver, schema, state);
        Ok(())
    }

    fn find_pdr_bad_state(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        predicate: &BoolExpr<T::State>,
        frames: &[Vec<Vec<usize>>],
        level: usize,
    ) -> Result<Option<Vec<usize>>, ModelCheckError> {
        let state = StateVars::new(&format!("pdr_bad_state_{level}"), schema);
        let solver = Solver::new();
        state.assert_domains(&solver, schema);
        self.assert_pdr_frame(&solver, schema, &state, frames, level)?;
        solver.assert(self.encode_state_predicate(schema, &state, predicate).not());
        if !solver_is_sat(&solver) {
            return Ok(None);
        }
        let Some(model) = solver.get_model() else {
            return Ok(None);
        };
        Ok(state.decode_indices(&model))
    }

    fn find_pdr_predecessor(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        frames: &[Vec<Vec<usize>>],
        level: usize,
        target: &[usize],
    ) -> Result<MaybePredecessor<T>, ModelCheckError> {
        let prev = StateVars::new(&format!("pdr_prev_{level}"), schema);
        let next = StateVars::new(&format!("pdr_next_{level}"), schema);
        let step = Int::new_const(format!("pdr_step_{level}"));
        let solver = Solver::new();

        prev.assert_domains(&solver, schema);
        next.assert_domains(&solver, schema);
        assert_in_domain(&solver, &step, self.step_domain_size());
        self.assert_pdr_frame(&solver, schema, &prev, frames, level)?;
        solver.assert(self.encode_state_key_eq(&next, target));
        self.assert_state_constraints(&solver, schema, &next);
        solver
            .assert(self.encode_transition_formula(schema, &prev, &step, &next, program, actions));
        self.assert_action_constraints(&solver, schema, &prev, &step, &next, actions);

        if !solver_is_sat(&solver) {
            return Ok(None);
        }
        let Some(model) = solver.get_model() else {
            return Ok(None);
        };
        let Some(prev_key) = prev.decode_indices(&model) else {
            return Ok(None);
        };
        let Some(step_code) = decode_int(&model, &step) else {
            return Ok(None);
        };
        let step_value = if step_code == 0 {
            TraceStep::Stutter
        } else {
            TraceStep::Action(
                actions
                    .get(step_code - 1)
                    .cloned()
                    .expect("step code in action domain"),
            )
        };
        Ok(Some((prev_key, step_value)))
    }

    fn block_pdr_state(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        frames: &mut [Vec<Vec<usize>>],
        level: usize,
        target: &[usize],
    ) -> Result<MaybeBlockedPath<T>, ModelCheckError> {
        if level == 0 {
            return Ok(Some((vec![target.to_vec()], Vec::new())));
        }

        while let Some((predecessor, step)) =
            self.find_pdr_predecessor(schema, program, actions, frames, level - 1, target)?
        {
            if let Some((mut states, mut steps)) =
                self.block_pdr_state(schema, program, actions, frames, level - 1, &predecessor)?
            {
                states.push(target.to_vec());
                steps.push(step);
                return Ok(Some((states, steps)));
            }
        }

        for frame in frames.iter_mut().take(level + 1).skip(1) {
            self.add_frame_key(frame, target);
        }
        Ok(None)
    }

    fn propagate_pdr_frames(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        frames: &mut [Vec<Vec<usize>>],
        level: usize,
    ) -> Result<(), ModelCheckError> {
        for frame_index in 1..=level {
            let blocked = frames[frame_index].clone();
            for key in blocked {
                if self.frame_contains_key(&frames[frame_index + 1], &key) {
                    continue;
                }
                if self
                    .find_pdr_predecessor(schema, program, actions, frames, frame_index, &key)?
                    .is_none()
                {
                    self.add_frame_key(&mut frames[frame_index + 1], &key);
                }
            }
        }
        Ok(())
    }

    fn encode_initial_state_formula(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        state: &StateVars,
    ) -> Result<Bool, ModelCheckError> {
        let clauses = self
            .initial_states_filtered()?
            .into_iter()
            .map(|initial| {
                let equalities = schema
                    .fields()
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        state.fields[index].eq(Int::from_u64(field.read_index(&initial) as u64))
                    })
                    .collect::<Vec<_>>();
                bool_and(&equalities)
            })
            .collect::<Vec<_>>();
        Ok(bool_or(&clauses))
    }

    fn encode_transition_formula(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        prev: &StateVars,
        step: &Int,
        next: &StateVars,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
    ) -> Bool {
        let mut clauses = Vec::new();

        let mut stutter = prev
            .fields
            .iter()
            .zip(next.fields.iter())
            .map(|(prev, next)| next.eq(prev))
            .collect::<Vec<_>>();
        stutter.push(step.eq(Int::from_u64(0)));
        clauses.push(bool_and(&stutter));

        clauses.extend(program.rules().iter().map(|rule| {
            encode_rule_transition(schema, &prev.fields, step, &next.fields, actions, rule)
        }));

        bool_or(&clauses)
    }

    fn assert_state_constraints(
        &self,
        solver: &Solver,
        schema: &SymbolicStateSchema<T::State>,
        state: &StateVars,
    ) {
        for constraint in self.model_case.state_constraints() {
            solver.assert(self.encode_state_predicate(schema, state, constraint));
        }
    }

    fn encode_action_constraints_formula(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        prev: &StateVars,
        step: &Int,
        next: &StateVars,
        actions: &[T::Action],
    ) -> Bool {
        let mut clauses = Vec::new();
        for constraint in self.model_case.action_constraints() {
            let allowed = self.encode_step_predicate(schema, prev, step, next, constraint, actions);
            clauses.push(bool_or(&[step.eq(Int::from_u64(0)), allowed]));
        }
        bool_and(&clauses)
    }

    fn assert_action_constraints(
        &self,
        solver: &Solver,
        schema: &SymbolicStateSchema<T::State>,
        prev: &StateVars,
        step: &Int,
        next: &StateVars,
        actions: &[T::Action],
    ) {
        solver.assert(self.encode_action_constraints_formula(schema, prev, step, next, actions));
    }

    fn enumerate_symbolic_successors_with_program(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        state: &T::State,
        program: &TransitionProgram<T::State, T::Action>,
    ) -> Result<Vec<SymbolicSuccessor<T>>, ModelCheckError> {
        let actions = self.action_domain();
        let prev = StateVars::new("prev", schema);
        let next = StateVars::new("next", schema);
        let step = Int::new_const("step");
        let solver = Solver::new();

        prev.assert_domains(&solver, schema);
        next.assert_domains(&solver, schema);
        assert_in_domain(&solver, &step, self.step_domain_size());
        prev.fix_to_state(&solver, schema, state);
        solver
            .assert(self.encode_transition_formula(schema, &prev, &step, &next, program, &actions));
        self.assert_state_constraints(&solver, schema, &next);
        self.assert_action_constraints(&solver, schema, &prev, &step, &next, &actions);

        let mut values = Vec::new();
        while solver_is_sat(&solver) {
            let Some(model) = solver.get_model() else {
                break;
            };
            let Some(step_code) = decode_int(&model, &step) else {
                break;
            };
            let Some(next_state) = next.decode(&model, schema) else {
                break;
            };
            let edge = if step_code == 0 {
                (TraceStep::Stutter, next_state)
            } else {
                let action = actions
                    .get(step_code - 1)
                    .cloned()
                    .expect("step code in action domain");
                (TraceStep::Action(action), next_state)
            };
            if !values.iter().any(|(_, candidate)| candidate == &edge) {
                values.push((step_code, edge));
            }
            let mut block = prev.all_values();
            block.extend(next.all_values());
            block.push(step.clone());
            block_current_model(&solver, &model, &block);
        }

        values.sort_by(|(lhs_code, (_, lhs_state)), (rhs_code, (_, rhs_state))| {
            let lhs_rank = if *lhs_code == 0 {
                actions.len() + 1
            } else {
                *lhs_code
            };
            let rhs_rank = if *rhs_code == 0 {
                actions.len() + 1
            } else {
                *rhs_code
            };
            lhs_rank.cmp(&rhs_rank).then_with(|| {
                schema
                    .read_indices(lhs_state)
                    .cmp(&schema.read_indices(rhs_state))
            })
        });

        Ok(values.into_iter().map(|(_, edge)| edge).collect())
    }

    fn enumerate_symbolic_successors(
        &self,
        state: &T::State,
    ) -> Result<Vec<SymbolicSuccessor<T>>, ModelCheckError> {
        let program = self.direct_transition_program()?;
        let schema = self.direct_state_schema()?;

        self.enumerate_symbolic_successors_with_program(&schema, state, &program)
    }

    fn collect_direct_lasso_traces(
        &self,
    ) -> Result<TraceList<T::State, T::Action>, ModelCheckError> {
        self.ensure_symbolic_constraints_ast_native()?;
        self.ensure_symbolic_stutter_is_identity()?;
        let program = self.direct_transition_program()?;
        let schema = self.direct_state_schema()?;
        self.ensure_symbolic_schema_covers_program(&schema, &program)?;
        self.ensure_symbolic_schema_covers_model_case_constraints(&schema)?;
        let actions = self.action_domain();
        let mut traces = Vec::new();

        for len in 1..=self.temporal_max_depth(&schema) + 1 {
            traces.extend(
                self.collect_direct_lasso_traces_of_length(&schema, &program, &actions, len)?,
            );
        }

        Ok(traces)
    }

    fn collect_direct_lasso_traces_of_length(
        &self,
        schema: &SymbolicStateSchema<T::State>,
        program: &TransitionProgram<T::State, T::Action>,
        actions: &[T::Action],
        len: usize,
    ) -> Result<TraceList<T::State, T::Action>, ModelCheckError> {
        let vars = LassoVars::new("trace", len, schema);
        let solver = Solver::new();
        vars.assert_domains(&solver, schema, self.step_domain_size());
        solver.assert(self.encode_initial_state_formula(schema, &vars.states[0])?);

        for state in &vars.states {
            self.assert_state_constraints(&solver, schema, state);
        }

        for index in 0..len.saturating_sub(1) {
            solver.assert(self.encode_transition_formula(
                schema,
                &vars.states[index],
                &vars.steps[index],
                &vars.states[index + 1],
                program,
                actions,
            ));
            solver.assert(self.encode_action_constraints_formula(
                schema,
                &vars.states[index],
                &vars.steps[index],
                &vars.states[index + 1],
                actions,
            ));
        }

        let last = len - 1;
        let mut loop_cases = vec![bool_and(&[
            vars.terminal.eq(Int::from_u64(1)),
            vars.loop_start.eq(Int::from_u64(last as u64)),
            vars.steps[last].eq(Int::from_u64(0)),
        ])];

        for target in 0..len {
            loop_cases.push(bool_and(&[
                vars.terminal.eq(Int::from_u64(0)),
                vars.loop_start.eq(Int::from_u64(target as u64)),
                self.encode_transition_formula(
                    schema,
                    &vars.states[last],
                    &vars.steps[last],
                    &vars.states[target],
                    program,
                    actions,
                ),
                self.encode_action_constraints_formula(
                    schema,
                    &vars.states[last],
                    &vars.steps[last],
                    &vars.states[target],
                    actions,
                ),
            ]));
        }

        solver.assert(bool_or(&loop_cases));

        let mut traces = Vec::new();
        while solver_is_sat(&solver) {
            let Some(model) = solver.get_model() else {
                break;
            };
            let Some(trace) = vars.decode(&model, schema, actions) else {
                break;
            };
            if !traces.contains(&trace) {
                traces.push(trace);
            }
            block_current_model(&solver, &model, &vars.all_values());
        }

        Ok(traces)
    }

    fn symbolic_artifacts(&self) -> &nirvash_lower::SymbolicArtifacts<T::State, T::Action> {
        self.spec.symbolic_artifacts()
    }

    fn normalized_core(&self) -> Result<&nirvash_lower::NormalizedSpecCore, ModelCheckError> {
        self.spec.normalized_core().map_err(|err| {
            self.symbolic_ast_required_error(format!(
                "symbolic backend failed to normalize spec `{}` core: {err}",
                self.spec.frontend_name(),
            ))
        })
    }

    fn ensure_normalized_fragment_supported(&self) -> Result<(), ModelCheckError> {
        let normalized = self.normalized_core()?;
        let profile = normalized.fragment_profile();
        if profile.symbolic_supported {
            return Ok(());
        }
        let reasons = profile.symbolic_unsupported_reasons().join(", ");
        Err(self.symbolic_ast_required_error(format!(
            "symbolic backend requires spec `{}` normalized core to avoid unsupported fragments: {reasons}",
            self.spec.frontend_name(),
        )))
    }

    fn symbolic_support_issue_error(&self, issue: &SymbolicSupportIssue) -> ModelCheckError {
        self.symbolic_ast_required_error(issue.to_string())
    }

    fn ensure_direct_fragment_supported(
        &self,
        backend_fragment: &'static str,
    ) -> Result<(), ModelCheckError> {
        self.ensure_normalized_fragment_supported()?;
        if let Some(issue) = self
            .symbolic_artifacts()
            .first_issue_for_fragment(backend_fragment)
        {
            return Err(self.symbolic_support_issue_error(issue));
        }
        Ok(())
    }

    fn direct_transition_program(
        &self,
    ) -> Result<TransitionProgram<T::State, T::Action>, ModelCheckError> {
        self.ensure_direct_fragment_supported("direct_smt.transition")?;
        let Some(program) = self.symbolic_artifacts().transition_program().cloned() else {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires spec `{}` to lower an AST-native transition program into direct SMT artifacts",
                self.spec.frontend_name(),
            )));
        };
        if !program.is_ast_native() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires spec `{}` transition program `{}` to be AST-native",
                self.spec.frontend_name(),
                program.name(),
            )));
        }
        if let Some(node) = program.first_unencodable_symbolic_node() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend requires transition program `{}` for spec `{}` to register helper/effect `{}` for symbolic use",
                program.name(),
                self.spec.frontend_name(),
                node,
            )));
        }
        Ok(program)
    }

    fn direct_state_schema(&self) -> Result<SymbolicStateSchema<T::State>, ModelCheckError> {
        self.ensure_normalized_fragment_supported()?;
        self.symbolic_artifacts().state_schema().cloned().ok_or_else(|| {
            self.symbolic_ast_required_error(format!(
                "symbolic backend requires state `{}` to implement SymbolicEncoding and lower a symbolic encoding schema",
                std::any::type_name::<T::State>(),
            ))
        })
    }

    fn direct_invariants(&self) -> Vec<BoolExpr<T::State>> {
        self.symbolic_artifacts().invariants().to_vec()
    }

    fn direct_properties(&self) -> Vec<Ltl<T::State, T::Action>> {
        self.symbolic_artifacts().properties().to_vec()
    }

    fn direct_fairness(&self) -> Vec<Fairness<T::State, T::Action>> {
        self.symbolic_artifacts().fairness().to_vec()
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
                self.spec.frontend_name(),
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
                    self.spec.frontend_name(),
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
                    self.spec.frontend_name(),
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
        for invariant in self.direct_invariants() {
            if let Some(node) = invariant.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic reachable-graph backend requires invariant `{}` for spec `{}` to register helper `{}` for symbolic use",
                    invariant.name(),
                    self.spec.frontend_name(),
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
        for property in self.direct_properties() {
            if let Some(node) = property.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to register helper `{}` for symbolic use",
                    property.describe(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
            self.ensure_symbolic_schema_covers_paths(
                schema,
                format!("property `{}`", property.describe()),
                property.symbolic_state_paths(),
            )?;
        }
        for fairness in self.direct_fairness() {
            if let Some(node) = fairness.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to register helper `{}` for symbolic use",
                    fairness.name(),
                    self.spec.frontend_name(),
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
        match update {
            UpdateAst::Sequence(ops) => {
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
            }
            UpdateAst::Choice(choice) => {
                for target in choice.write_paths() {
                    if *target != "self" && !schema.has_path(target) {
                        return Err(self.symbolic_ast_required_error(format!(
                            "symbolic backend requires state schema for `{}` to expose field `{}`",
                            std::any::type_name::<T::State>(),
                            target,
                        )));
                    }
                }
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
                    self.spec.frontend_name(),
                )));
            }
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires state constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
        }
        for constraint in self.model_case.action_constraints() {
            if !constraint.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires action constraint `{}` for spec `{}` to be AST-native",
                    constraint.name(),
                    self.spec.frontend_name(),
                )));
            }
            if let Some(node) = constraint.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires action constraint `{}` for spec `{}` to register helper `{}` for symbolic use",
                    constraint.name(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
        }
        Ok(())
    }

    fn ensure_symbolic_invariants_ast_native(&self) -> Result<(), ModelCheckError> {
        for invariant in self.direct_invariants() {
            if !invariant.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires invariant `{}` for spec `{}` to be AST-native",
                    invariant.name(),
                    self.spec.frontend_name(),
                )));
            }
            if let Some(node) = invariant.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires invariant `{}` for spec `{}` to register helper `{}` for symbolic use",
                    invariant.name(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
        }
        Ok(())
    }

    fn ensure_symbolic_properties_ast_native(&self) -> Result<(), ModelCheckError> {
        for property in self.direct_properties() {
            if !property.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to be AST-native",
                    property.describe(),
                    self.spec.frontend_name(),
                )));
            }
            if let Some(node) = property.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires property `{}` for spec `{}` to register helper `{}` for symbolic use",
                    property.describe(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
        }
        for fairness in self.direct_fairness() {
            if !fairness.is_ast_native() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to be AST-native",
                    fairness.name(),
                    self.spec.frontend_name(),
                )));
            }
            if let Some(node) = fairness.first_unencodable_symbolic_node() {
                return Err(self.symbolic_ast_required_error(format!(
                    "symbolic backend requires fairness `{}` for spec `{}` to register helper `{}` for symbolic use",
                    fairness.name(),
                    self.spec.frontend_name(),
                    node,
                )));
            }
        }
        Ok(())
    }

    fn ensure_no_explicit_only_reducers(&self) -> Result<(), ModelCheckError> {
        if self
            .model_case
            .heuristic_reduction()
            .and_then(|reduction| reduction.state_projection())
            .is_some()
        {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend does not support heuristic state projection for model case `{}` in spec `{}`",
                self.model_case.label(),
                self.spec.frontend_name(),
            )));
        }
        if self
            .model_case
            .heuristic_reduction()
            .and_then(|reduction| reduction.action_pruning())
            .is_some()
        {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend does not support heuristic action pruning for model case `{}` in spec `{}`",
                self.model_case.label(),
                self.spec.frontend_name(),
            )));
        }
        if self.model_case.sound_reduction().is_some() {
            return Err(self.symbolic_ast_required_error(format!(
                "symbolic backend does not support sound reductions for model case `{}` in spec `{}`",
                self.model_case.label(),
                self.spec.frontend_name(),
            )));
        }
        Ok(())
    }

    fn symbolic_ast_required_error(&self, message: String) -> ModelCheckError {
        ModelCheckError::UnsupportedConfiguration(Box::leak(message.into_boxed_str()))
    }

    fn canonicalize_state(&self, state: &T::State) -> T::State {
        self.model_case
            .sound_reduction()
            .and_then(|reduction| reduction.symmetry())
            .map(|symmetry| symmetry.canonicalize(state))
            .unwrap_or_else(|| state.clone())
    }

    fn state_constraints_allow(&self, state: &T::State) -> bool {
        self.model_case
            .state_constraints()
            .iter()
            .all(|constraint: &BoolExpr<T::State>| constraint.eval(state))
    }

    #[allow(dead_code)]
    fn trace_to_state(
        &self,
        graph: &ReachableGraph<T::State, T::Action>,
        target: usize,
    ) -> Trace<T::State, T::Action> {
        let (states, steps) = self.reconstruct_path(graph, target);
        self.terminal_trace(states, steps)
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn trace_satisfies_fairness_graph(
        &self,
        trace: &Trace<T::State, T::Action>,
        graph: &ReachableGraph<T::State, T::Action>,
    ) -> bool {
        self.direct_fairness()
            .into_iter()
            .all(|fairness| self.eval_fairness_graph(trace, graph, fairness))
    }

    #[allow(dead_code)]
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

    fn trace_satisfies_fairness_direct(&self, trace: &Trace<T::State, T::Action>) -> bool {
        self.direct_fairness()
            .into_iter()
            .all(|fairness| self.eval_fairness_direct(trace, fairness))
    }

    fn eval_fairness_direct(
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
            self.enumerate_symbolic_successors(&trace.states()[index])
                .unwrap_or_else(|error| {
                    panic!("symbolic lasso successor enumeration failed: {error:?}")
                })
                .into_iter()
                .filter_map(|(step, next)| match step {
                    TraceStep::Action(action) => Some((action, next)),
                    TraceStep::Stutter => None,
                })
                .any(|(action, next)| predicate.eval(&trace.states()[index], &action, &next))
        });
        let enabled_all = trace.cycle_indices().all(|index| {
            self.enumerate_symbolic_successors(&trace.states()[index])
                .unwrap_or_else(|error| {
                    panic!("symbolic lasso successor enumeration failed: {error:?}")
                })
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
                    self.enumerate_symbolic_successors(state)
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
