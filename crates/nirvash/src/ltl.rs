use std::collections::BTreeSet;

use crate::{BoolExpr, FiniteModelDomain, StepExpr};

#[derive(Debug, Clone)]
pub enum Ltl<S, A> {
    True,
    False,
    Pred(BoolExpr<S>),
    StepPred(StepExpr<S, A>),
    Not(Box<Ltl<S, A>>),
    And(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Or(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Implies(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Next(Box<Ltl<S, A>>),
    Always(Box<Ltl<S, A>>),
    Eventually(Box<Ltl<S, A>>),
    Until(Box<Ltl<S, A>>, Box<Ltl<S, A>>),
    Enabled(StepExpr<S, A>),
}

impl<S: 'static, A: 'static> Ltl<S, A> {
    pub const fn truth() -> Self {
        Self::True
    }

    pub const fn falsity() -> Self {
        Self::False
    }

    pub fn pred(predicate: BoolExpr<S>) -> Self {
        Self::Pred(predicate)
    }

    pub fn step(predicate: StepExpr<S, A>) -> Self {
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

    pub fn enabled(predicate: StepExpr<S, A>) -> Self {
        Self::Enabled(predicate)
    }

    pub fn leads_to(lhs: Ltl<S, A>, rhs: Ltl<S, A>) -> Self {
        Self::always(Self::implies(lhs, Self::eventually(rhs)))
    }

    pub fn forall<T, F>(mut build: F) -> Self
    where
        T: FiniteModelDomain,
        F: FnMut(T) -> Ltl<S, A>,
    {
        T::finite_domain()
            .into_vec()
            .into_iter()
            .fold(Self::truth(), |acc, value| Self::and(acc, build(value)))
    }

    pub fn exists<T, F>(mut build: F) -> Self
    where
        T: FiniteModelDomain,
        F: FnMut(T) -> Ltl<S, A>,
    {
        let mut iter = T::finite_domain().into_vec().into_iter();
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

    pub fn is_ast_native(&self) -> bool {
        match self {
            Self::True | Self::False => true,
            Self::Pred(predicate) => predicate.is_ast_native(),
            Self::StepPred(predicate) | Self::Enabled(predicate) => predicate.is_ast_native(),
            Self::Not(inner)
            | Self::Next(inner)
            | Self::Always(inner)
            | Self::Eventually(inner) => inner.is_ast_native(),
            Self::And(lhs, rhs)
            | Self::Or(lhs, rhs)
            | Self::Implies(lhs, rhs)
            | Self::Until(lhs, rhs) => lhs.is_ast_native() && rhs.is_ast_native(),
        }
    }

    pub fn first_unencodable_symbolic_node(&self) -> Option<&'static str> {
        match self {
            Self::True | Self::False => None,
            Self::Pred(predicate) => predicate.first_unencodable_symbolic_node(),
            Self::StepPred(predicate) | Self::Enabled(predicate) => {
                predicate.first_unencodable_symbolic_node()
            }
            Self::Not(inner)
            | Self::Next(inner)
            | Self::Always(inner)
            | Self::Eventually(inner) => inner.first_unencodable_symbolic_node(),
            Self::And(lhs, rhs)
            | Self::Or(lhs, rhs)
            | Self::Implies(lhs, rhs)
            | Self::Until(lhs, rhs) => lhs
                .first_unencodable_symbolic_node()
                .or_else(|| rhs.first_unencodable_symbolic_node()),
        }
    }

    pub fn symbolic_state_paths(&self) -> Vec<&'static str> {
        let mut paths = BTreeSet::new();
        self.collect_symbolic_state_paths(&mut paths);
        paths.into_iter().collect()
    }

    fn collect_symbolic_state_paths(&self, paths: &mut BTreeSet<&'static str>) {
        match self {
            Self::True | Self::False => {}
            Self::Pred(predicate) => {
                for path in predicate.symbolic_state_paths() {
                    paths.insert(path);
                }
            }
            Self::StepPred(predicate) | Self::Enabled(predicate) => {
                for path in predicate.symbolic_state_paths() {
                    paths.insert(path);
                }
            }
            Self::Not(inner)
            | Self::Next(inner)
            | Self::Always(inner)
            | Self::Eventually(inner) => inner.collect_symbolic_state_paths(paths),
            Self::And(lhs, rhs)
            | Self::Or(lhs, rhs)
            | Self::Implies(lhs, rhs)
            | Self::Until(lhs, rhs) => {
                lhs.collect_symbolic_state_paths(paths);
                rhs.collect_symbolic_state_paths(paths);
            }
        }
    }
}
