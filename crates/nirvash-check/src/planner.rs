use nirvash::TraceStep;
use nirvash_backends::{
    build_explicit_suite_cover, share_trace_prefixes, symbolic::trace_constraints,
};
use nirvash_lower::{
    CheckerSpec, FiniteModelDomain, ModelCheckConfig, ModelCheckError, ModelInstance, Trace,
};

use crate::ExplicitModelChecker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTransition<S, A> {
    pub prev: S,
    pub action: A,
    pub next: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTraceConstraint<S, A> {
    pub states: Vec<S>,
    pub steps: Vec<TraceStep<A>>,
    pub loop_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedObligationKind<S, A> {
    ExplicitTraceCover {
        edge: PlannedTransition<S, A>,
        trace: Trace<S, A>,
    },
    PropertyPrefix {
        prefix: Vec<TraceStep<A>>,
        traces: Vec<Trace<S, A>>,
    },
    SymbolicTraceConstraint {
        constraint: PlannedTraceConstraint<S, A>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedObligation<S, A> {
    pub id: String,
    pub kind: PlannedObligationKind<S, A>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanningCoverageGoal {
    Transitions,
    TransitionPairs(usize),
    GuardBoundaries,
    PropertyPrefixes,
    TraceConstraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerSeedProfile<S, A> {
    pub labels: Vec<String>,
    pub state_hints: Vec<S>,
    pub action_hints: Vec<A>,
}

impl<S, A> Default for PlannerSeedProfile<S, A> {
    fn default() -> Self {
        Self {
            labels: Vec::new(),
            state_hints: Vec::new(),
            action_hints: Vec::new(),
        }
    }
}

impl<S, A> PlannerSeedProfile<S, A> {
    fn allows(&self, label: &str) -> bool {
        self.labels.is_empty()
            || self
                .labels
                .iter()
                .any(|seed_label| seed_label == "all" || seed_label == label)
    }
}

pub trait ObligationPlanner<T: CheckerSpec> {
    fn obligations(
        &self,
        lowered: &T,
        model: &ModelInstance<T::State, T::Action>,
        coverage: &[PlanningCoverageGoal],
        seeds: &PlannerSeedProfile<T::State, T::Action>,
    ) -> Result<Vec<PlannedObligation<T::State, T::Action>>, ModelCheckError>;
}

#[derive(Debug, Clone, Default)]
pub struct ExplicitObligationPlanner {
    config: Option<ModelCheckConfig>,
}

impl ExplicitObligationPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: ModelCheckConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

impl<T> ObligationPlanner<T> for ExplicitObligationPlanner
where
    T: CheckerSpec,
    T::State: Clone + Eq + PartialEq + FiniteModelDomain + Send + Sync,
    T::Action: Clone + Eq + PartialEq + Send + Sync,
{
    fn obligations(
        &self,
        lowered: &T,
        model: &ModelInstance<T::State, T::Action>,
        coverage: &[PlanningCoverageGoal],
        seeds: &PlannerSeedProfile<T::State, T::Action>,
    ) -> Result<Vec<PlannedObligation<T::State, T::Action>>, ModelCheckError> {
        if !wants_transitions(coverage) || !seeds.allows("explicit_suite") {
            return Ok(Vec::new());
        }
        let Some(model_case) = resolve_model_case(lowered, model, self.config.clone()) else {
            return Ok(Vec::new());
        };
        let traces = ExplicitModelChecker::for_case(lowered, model_case).candidate_traces()?;
        let cover = build_explicit_suite_cover(&traces);
        Ok(cover
            .cases
            .into_iter()
            .enumerate()
            .map(|(index, case)| PlannedObligation {
                id: format!("explicit-cover-{index}"),
                kind: PlannedObligationKind::ExplicitTraceCover {
                    edge: PlannedTransition {
                        prev: case.edge.prev,
                        action: case.edge.action,
                        next: case.edge.next,
                    },
                    trace: case.trace,
                },
            })
            .collect())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PropertyPrefixPlanner {
    config: Option<ModelCheckConfig>,
}

impl PropertyPrefixPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: ModelCheckConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

impl<T> ObligationPlanner<T> for PropertyPrefixPlanner
where
    T: CheckerSpec,
    T::State: Clone + Eq + PartialEq + FiniteModelDomain + Send + Sync,
    T::Action: Clone + Eq + PartialEq + Send + Sync,
{
    fn obligations(
        &self,
        lowered: &T,
        model: &ModelInstance<T::State, T::Action>,
        coverage: &[PlanningCoverageGoal],
        seeds: &PlannerSeedProfile<T::State, T::Action>,
    ) -> Result<Vec<PlannedObligation<T::State, T::Action>>, ModelCheckError> {
        if !wants_property_prefixes(coverage) || !seeds.allows("property_prefixes") {
            return Ok(Vec::new());
        }
        let Some(model_case) = resolve_model_case(lowered, model, self.config.clone()) else {
            return Ok(Vec::new());
        };
        let traces = ExplicitModelChecker::for_case(lowered, model_case).candidate_traces()?;
        Ok(share_trace_prefixes(&traces)
            .into_iter()
            .enumerate()
            .map(|(index, group)| PlannedObligation {
                id: format!("property-prefix-{index}"),
                kind: PlannedObligationKind::PropertyPrefix {
                    prefix: group.prefix,
                    traces: group.traces,
                },
            })
            .collect())
    }
}

#[derive(Debug, Clone, Default)]
pub struct TraceConstraintPlanner {
    config: Option<ModelCheckConfig>,
}

impl TraceConstraintPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: ModelCheckConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    fn obligations_from_traces<T>(
        &self,
        lowered: &T,
        traces: &[Trace<T::State, T::Action>],
    ) -> Result<Vec<PlannedObligation<T::State, T::Action>>, ModelCheckError>
    where
        T: CheckerSpec,
        T::State: Clone + Eq,
        T::Action: Clone + Eq,
    {
        if traces.is_empty() {
            return Err(ModelCheckError::UnsupportedConfiguration(
                "trace constraints require at least one candidate trace",
            ));
        }
        if lowered.symbolic_artifacts().state_schema().is_none() {
            return Err(ModelCheckError::UnsupportedConfiguration(
                "trace constraints require a symbolic state schema",
            ));
        }
        if !lowered.symbolic_artifacts().issues().is_empty() {
            return Err(ModelCheckError::UnsupportedConfiguration(
                "trace constraints require a symbolically supported spec",
            ));
        }

        Ok(traces
            .iter()
            .enumerate()
            .map(|(index, trace)| {
                let constraint = trace_constraints::build(trace);
                PlannedObligation {
                    id: format!("trace-constraint-{index}"),
                    kind: PlannedObligationKind::SymbolicTraceConstraint {
                        constraint: PlannedTraceConstraint {
                            states: constraint.states,
                            steps: constraint.steps,
                            loop_start: constraint.loop_start,
                        },
                    },
                }
            })
            .collect())
    }
}

impl<T> ObligationPlanner<T> for TraceConstraintPlanner
where
    T: CheckerSpec,
    T::State: Clone + Eq + PartialEq + FiniteModelDomain + Send + Sync,
    T::Action: Clone + Eq + PartialEq + Send + Sync,
{
    fn obligations(
        &self,
        lowered: &T,
        model: &ModelInstance<T::State, T::Action>,
        coverage: &[PlanningCoverageGoal],
        seeds: &PlannerSeedProfile<T::State, T::Action>,
    ) -> Result<Vec<PlannedObligation<T::State, T::Action>>, ModelCheckError> {
        if !wants_trace_constraints(coverage) || !seeds.allows("trace_constraints") {
            return Ok(Vec::new());
        }
        let Some(model_case) = resolve_model_case(lowered, model, self.config.clone()) else {
            return Ok(Vec::new());
        };
        let traces = ExplicitModelChecker::for_case(lowered, model_case).candidate_traces()?;
        self.obligations_from_traces(lowered, &traces)
    }
}

fn wants_transitions(coverage: &[PlanningCoverageGoal]) -> bool {
    coverage.is_empty()
        || coverage.iter().any(|goal| {
            matches!(
                goal,
                PlanningCoverageGoal::Transitions
                    | PlanningCoverageGoal::TransitionPairs(_)
                    | PlanningCoverageGoal::GuardBoundaries
            )
        })
}

fn wants_property_prefixes(coverage: &[PlanningCoverageGoal]) -> bool {
    coverage.is_empty()
        || coverage
            .iter()
            .any(|goal| matches!(goal, PlanningCoverageGoal::PropertyPrefixes))
}

fn wants_trace_constraints(coverage: &[PlanningCoverageGoal]) -> bool {
    coverage.is_empty()
        || coverage.iter().any(|goal| {
            matches!(
                goal,
                PlanningCoverageGoal::TraceConstraints | PlanningCoverageGoal::PropertyPrefixes
            )
        })
}

fn model_exists<T: CheckerSpec>(lowered: &T, model: &ModelInstance<T::State, T::Action>) -> bool {
    let available = lowered.model_instances();
    available.is_empty()
        || available
            .iter()
            .any(|candidate| candidate.label() == model.label())
}

fn resolve_model_case<T: CheckerSpec>(
    lowered: &T,
    model: &ModelInstance<T::State, T::Action>,
    config: Option<ModelCheckConfig>,
) -> Option<ModelInstance<T::State, T::Action>> {
    if !model_exists(lowered, model) {
        return None;
    }
    let mut resolved = model.clone();
    if let Some(config) = config {
        resolved = resolved.with_checker_config(config);
    }
    Some(resolved)
}
