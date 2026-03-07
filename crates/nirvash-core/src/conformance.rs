pub use crate::system::{ActionApplier, ExpectedStep, StateObserver, TransitionSystem};
pub use crate::{ModelChecker, ReachableGraphSnapshot, Signature};

/// Spec-side contract for replaying runtime behavior against a transition system.
pub trait ProtocolConformanceSpec: TransitionSystem {
    type ExpectedOutput: Clone + std::fmt::Debug + PartialEq + Eq;
    type ObservedState: Clone + std::fmt::Debug;
    type ObservedOutput: Clone + std::fmt::Debug;

    fn expected_step(
        &self,
        prev: &Self::State,
        action: &Self::Action,
    ) -> ExpectedStep<Self::State, Self::ExpectedOutput>;

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State;

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput;
}

/// Binding between a spec and a concrete runtime implementation.
#[allow(async_fn_in_trait)]
pub trait ProtocolRuntimeBinding<Spec>
where
    Spec: ProtocolConformanceSpec,
{
    type Runtime: ActionApplier<Action = Spec::Action, Output = Spec::ObservedOutput, Context = Self::Context>
        + StateObserver<ObservedState = Spec::ObservedState, Context = Self::Context>;
    type Context: Clone;

    async fn fresh_runtime(spec: &Spec) -> Self::Runtime;

    fn context(spec: &Spec) -> Self::Context;
}
