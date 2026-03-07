use crate::StepPredicate;

#[derive(Debug, Clone, Copy)]
pub enum Fairness<S, A> {
    Weak(StepPredicate<S, A>),
    Strong(StepPredicate<S, A>),
}

impl<S, A> Fairness<S, A> {
    pub const fn weak(predicate: StepPredicate<S, A>) -> Self {
        Self::Weak(predicate)
    }

    pub const fn strong(predicate: StepPredicate<S, A>) -> Self {
        Self::Strong(predicate)
    }

    pub const fn predicate(&self) -> StepPredicate<S, A> {
        match self {
            Self::Weak(predicate) | Self::Strong(predicate) => *predicate,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.predicate().name()
    }
}
