mod checker;
mod planning;
mod smt;
pub mod symbolic;

pub type ExplicitModelChecker<'a, T> = checker::ExplicitModelChecker<'a, T>;
pub type SymbolicModelChecker<'a, T> = symbolic::SymbolicModelChecker<'a, T>;
pub use planning::{
    CoveredTransition, ExplicitSuiteCase, ExplicitSuiteCover, SharedPrefixGroup,
    SymbolicTraceConstraint, build_explicit_suite_cover, build_symbolic_trace_constraint,
    share_trace_prefixes,
};
