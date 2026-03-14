use nirvash_lower::{Trace, TraceStep};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveredTransition<S, A> {
    pub prev: S,
    pub action: A,
    pub next: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitSuiteCase<S, A> {
    pub edge: CoveredTransition<S, A>,
    pub trace: Trace<S, A>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitSuiteCover<S, A> {
    pub cases: Vec<ExplicitSuiteCase<S, A>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPrefixGroup<S, A> {
    pub prefix: Vec<TraceStep<A>>,
    pub traces: Vec<Trace<S, A>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolicTraceConstraint<S, A> {
    pub states: Vec<S>,
    pub steps: Vec<TraceStep<A>>,
    pub loop_start: usize,
}

pub fn build_explicit_suite_cover<S, A>(traces: &[Trace<S, A>]) -> ExplicitSuiteCover<S, A>
where
    S: Clone + Eq,
    A: Clone + Eq,
{
    let mut sorted = traces.to_vec();
    sorted.sort_by_key(Trace::minimization_key);

    let mut covered = Vec::new();
    let mut cases = Vec::new();
    for trace in sorted {
        for (index, step) in trace.steps().iter().enumerate() {
            let TraceStep::Action(action) = step else {
                continue;
            };
            let next_index = trace.next_index(index);
            let edge = CoveredTransition {
                prev: trace.states()[index].clone(),
                action: action.clone(),
                next: trace.states()[next_index].clone(),
            };
            if covered.iter().any(|seen| seen == &edge) {
                continue;
            }
            covered.push(edge.clone());
            cases.push(ExplicitSuiteCase {
                edge,
                trace: trace.clone(),
            });
        }
    }

    ExplicitSuiteCover { cases }
}

pub fn share_trace_prefixes<S, A>(traces: &[Trace<S, A>]) -> Vec<SharedPrefixGroup<S, A>>
where
    S: Clone,
    A: Clone + Eq,
{
    let mut groups: Vec<(Option<TraceStep<A>>, Vec<Trace<S, A>>)> = Vec::new();

    for trace in traces.iter().cloned() {
        let key = trace.steps().first().cloned();
        if let Some((_, members)) = groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            members.push(trace);
            continue;
        }
        groups.push((key, vec![trace]));
    }

    groups
        .into_iter()
        .map(|(_, traces)| SharedPrefixGroup {
            prefix: common_prefix(&traces),
            traces,
        })
        .collect()
}

pub fn build_symbolic_trace_constraint<S, A>(trace: &Trace<S, A>) -> SymbolicTraceConstraint<S, A>
where
    S: Clone,
    A: Clone,
{
    SymbolicTraceConstraint {
        states: trace.states().to_vec(),
        steps: trace.steps().to_vec(),
        loop_start: trace.loop_start(),
    }
}

fn common_prefix<S, A>(traces: &[Trace<S, A>]) -> Vec<TraceStep<A>>
where
    A: Clone + Eq,
{
    let Some(first) = traces.first() else {
        return Vec::new();
    };
    let mut prefix = first.steps().to_vec();
    for trace in traces.iter().skip(1) {
        let common_len = prefix
            .iter()
            .zip(trace.steps())
            .take_while(|(lhs, rhs)| *lhs == *rhs)
            .count();
        prefix.truncate(common_len);
    }
    prefix
}
