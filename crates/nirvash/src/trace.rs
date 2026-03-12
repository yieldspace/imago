#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceStep<A> {
    Action(A),
    Stutter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace<S, A> {
    states: Vec<S>,
    steps: Vec<TraceStep<A>>,
    loop_start: usize,
}

impl<S, A> Trace<S, A> {
    pub fn new(states: Vec<S>, steps: Vec<TraceStep<A>>, loop_start: usize) -> Self {
        debug_assert!(!states.is_empty());
        debug_assert_eq!(states.len(), steps.len());
        debug_assert!(loop_start < states.len());
        Self {
            states,
            steps,
            loop_start,
        }
    }

    pub fn states(&self) -> &[S] {
        &self.states
    }

    pub fn steps(&self) -> &[TraceStep<A>] {
        &self.steps
    }

    pub const fn loop_start(&self) -> usize {
        self.loop_start
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn next_index(&self, index: usize) -> usize {
        if index + 1 < self.states.len() {
            index + 1
        } else {
            self.loop_start
        }
    }

    pub fn cycle_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.loop_start..self.states.len()
    }

    pub fn cycle_len(&self) -> usize {
        self.states.len() - self.loop_start
    }

    pub fn stutter_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TraceStep::Stutter))
            .count()
    }

    pub fn minimization_key(&self) -> (usize, usize, usize) {
        (self.len(), self.cycle_len(), self.stutter_count())
    }
}
