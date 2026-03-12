pub use nirvash::{
    Counterexample, CounterexampleKind, ExplorationMode, ModelBackend, ModelCase, ModelCaseSource,
    ModelCheckConfig, ModelCheckError, ModelCheckResult, ReachableGraphSnapshot, TemporalSpec,
};

pub struct ModelChecker<'a, T: TemporalSpec + ModelCaseSource>(
    nirvash_backends::BackendModelChecker<'a, T>,
);

impl<'a, T> ModelChecker<'a, T>
where
    T: TemporalSpec + ModelCaseSource,
    T::State: PartialEq + nirvash::Signature,
    T::Action: PartialEq,
{
    pub fn new(spec: &'a T) -> Self {
        Self(nirvash_backends::BackendModelChecker::new(spec))
    }

    pub fn for_case(spec: &'a T, model_case: ModelCase<T::State, T::Action>) -> Self {
        Self(nirvash_backends::BackendModelChecker::for_case(
            spec, model_case,
        ))
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        Self(nirvash_backends::BackendModelChecker::with_config(
            spec, config,
        ))
    }

    pub fn reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.0.reachable_graph_snapshot()
    }

    pub fn full_reachable_graph_snapshot(
        &self,
    ) -> Result<ReachableGraphSnapshot<T::State, T::Action>, ModelCheckError> {
        self.0.full_reachable_graph_snapshot()
    }

    pub fn check_invariants(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_invariants()
    }

    pub fn check_deadlocks(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_deadlocks()
    }

    pub fn check_properties(
        &self,
    ) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_properties()
    }

    pub fn check_all(&self) -> Result<ModelCheckResult<T::State, T::Action>, ModelCheckError> {
        self.0.check_all()
    }

    pub fn backend(&self) -> ModelBackend {
        self.0.backend()
    }

    pub fn doc_backend(&self) -> ModelBackend {
        self.0.doc_backend()
    }
}
