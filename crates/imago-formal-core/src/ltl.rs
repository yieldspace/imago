use crate::{Signature, StatePredicate, StepPredicate};

#[derive(Debug, Clone)]
pub enum Ltl<S, A> {
    True,
    False,
    Pred(StatePredicate<S>),
    StepPred(StepPredicate<S, A>),
    Not(Box<Ltl<S, A>>),
    And(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Or(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Implies(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Next(Box<Ltl<S, A>>),
    Always(Box<Ltl<S, A>>),
    Eventually(Box<Ltl<S, A>>),
    Until(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Enabled(StepPredicate<S, A>),
}

impl<S, A> Ltl<S, A> {
    pub const fn truth() -> Self {
        Self::True
    }

    pub const fn falsity() -> Self {
        Self::False
    }

    pub fn pred(predicate: StatePredicate<S>) -> Self {
        Self::Pred(predicate)
    }

    pub fn step(predicate: StepPredicate<S, A>) -> Self {
        Self::StepPred(predicate)
    }

    pub fn negate(formula: Ltl<S, A>) -> Self {
        Self::Not(Box::new(formula))
    }

    pub fn and(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::And(Box::new(lhs), Box::new(rhs))
    }

    pub fn or(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::Or(Box::new(lhs), Box::new(rhs))
    }

    pub fn implies(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::Implies(Box::new(lhs), Box::new(rhs))
    }

    pub fn next(formula: Ltl<S, A>) -> Self {
        Self::Next(Box::new(formula))
    }

    pub fn always(formula: Ltl<S, A>) -> Self {
        Self::Always(Box::new(formula))
    }

    pub fn eventually(formula: Ltl<S, A>) -> Self {
        Self::Eventually(Box::new(formula))
    }

    pub fn until(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::Until(Box::new(lhs), Box::new(rhs))
    }

    pub fn enabled(predicate: StepPredicate<S, A>) -> Self {
        Self::Enabled(predicate)
    }

    pub fn leads_to(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::always(Self::implies(lhs, Self::eventually(rhs)))
    }

    pub fn forall<T, F>(mut build: F) -> Self
    where
        T: Signature,
        F: FnMut(T) -> Ltl<S, A>,
    {
        T::bounded_domain()
            .into_vec()
            .into_iter()
            .fold(Self::truth(), |acc, value| Self::and(acc, build(value)))
    }

    pub fn exists<T, F>(mut build: F) -> Self
    where
        T: Signature,
        F: FnMut(T) -> Ltl<S, A>,
    {
        let mut iter = T::bounded_domain().into_vec().into_iter();
        let Some(first) = iter.next() else {
            return Self::falsity();
        };

        iter.fold(build(first), |acc, value| Self::or(acc, build(value)))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::True => "true".to_owned(),
            Self::False => "false".to_owned(),
            Self::Pred(predicate) => predicate.name().to_owned(),
            Self::StepPred(predicate) => predicate.name().to_owned(),
            Self::Not(inner) => format!("!({})", inner.describe()),
            Self::And(lhs, rhs) => format!("({}) /\\ ({})", lhs.describe(), rhs.describe()),
            Self::Or(lhs, rhs) => format!("({}) \\/ ({})", lhs.describe(), rhs.describe()),
            Self::Implies(lhs, rhs) => format!("({}) => ({})", lhs.describe(), rhs.describe()),
            Self::Next(inner) => format!("X({})", inner.describe()),
            Self::Always(inner) => format!("[]({})", inner.describe()),
            Self::Eventually(inner) => format!("<>({})", inner.describe()),
            Self::Until(lhs, rhs) => format!("({}) U ({})", lhs.describe(), rhs.describe()),
            Self::Enabled(predicate) => format!("ENABLED({})", predicate.name()),
        }
    }
}
