mod checker;
mod smt;
mod symbolic;

pub type ExplicitModelChecker<'a, T> = checker::ExplicitModelChecker<'a, T>;
pub type SymbolicModelChecker<'a, T> = symbolic::SymbolicModelChecker<'a, T>;
