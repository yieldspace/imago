use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    fmt::{self, Debug, Display},
    fs,
    hash::{Hash, Hasher},
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub use nirvash::TrustTier;
use nirvash_backends::symbolic::trace_constraints;
use nirvash_check::{
    ExplicitModelChecker, ExplicitObligationPlanner, ObligationPlanner, PlannedObligationKind,
    PlannerSeedProfile, PlanningCoverageGoal, PropertyPrefixPlanner,
};
use nirvash_lower::{
    FiniteModelDomain, FrontendSpec, LoweringCx, ModelBackend, ModelCheckConfig, ModelInstance,
    ReachableGraphSnapshot, TemporalSpec, Trace, TraceStep,
};
use nirvash_proof::{KaniConcretePlayback, NormalizedReplayBundle};
use proptest::{
    arbitrary::Arbitrary,
    strategy::{Strategy, ValueTree},
    test_runner::{Config as ProptestConfig, RngAlgorithm, TestRng, TestRunner},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[doc(hidden)]
pub use proptest as __nirvash_proptest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectedState<S> {
    Exact(S),
    Partial(S),
    Unknown,
}

impl<S> ProjectedState<S> {
    pub fn as_ref(&self) -> Option<&S> {
        match self {
            Self::Exact(state) | Self::Partial(state) => Some(state),
            Self::Unknown => None,
        }
    }

    pub fn matches(&self, candidate: &S) -> bool
    where
        S: PartialEq,
    {
        self.as_ref()
            .map(|state| state == candidate)
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservedEvent<A, Output> {
    Invoke { action: A },
    Return { action: A, output: Option<Output> },
    Update { var: String, value: Value },
    Stutter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedActionStep<S, A, Output> {
    pub action: A,
    pub output: Output,
    pub after: ProjectedState<S>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedActionTrace<S, A, Output> {
    pub initial: ProjectedState<S>,
    pub steps: Vec<ObservedActionStep<S, A, Output>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedObservedTrace<S, A, Output> {
    pub initial: ProjectedState<S>,
    pub events: Vec<ObservedEvent<A, Output>>,
}

pub trait SpecOracle: FrontendSpec {
    type ExpectedOutput: Clone + Debug + PartialEq + Eq;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestEnvironment {
    pub rng_seed: u64,
    pub clock_seed: u64,
    pub schedule_seed: u64,
}

impl Default for TestEnvironment {
    fn default() -> Self {
        Self {
            rng_seed: 0,
            clock_seed: 0,
            schedule_seed: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NormalizedFragmentInfo {
    pub symbolic_supported: bool,
    pub proof_supported: bool,
    pub has_opaque_nodes: bool,
    pub has_stringly_nodes: bool,
    pub has_temporal_props: bool,
    pub has_fairness: bool,
}

pub trait RuntimeBinding<Spec: SpecOracle>: Sized {
    type Sut;
    type Fixture: Send + Sync + 'static;
    type Output: Clone + Debug + Send + Sync + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    fn create(fixture: Self::Fixture) -> Result<Self::Sut, Self::Error>;

    fn reset(sut: &mut Self::Sut, fixture: Self::Fixture) -> Result<(), Self::Error> {
        *sut = Self::create(fixture)?;
        Ok(())
    }

    fn apply(
        sut: &mut Self::Sut,
        action: &Spec::Action,
        env: &mut TestEnvironment,
    ) -> Result<Self::Output, Self::Error>;

    fn project(sut: &Self::Sut) -> ProjectedState<Spec::State>;

    fn project_output(action: &Spec::Action, output: &Self::Output) -> Spec::ExpectedOutput;
}

pub trait TraceSink<Spec: SpecOracle> {
    fn record_update(&mut self, var: &'static str, value: Value);
}

pub trait TraceBinding<Spec: SpecOracle>: RuntimeBinding<Spec> {
    fn record_update(sut: &Self::Sut, output: &Self::Output, sink: &mut dyn TraceSink<Spec>);
}

pub trait ConcurrentBinding<Spec: SpecOracle>: RuntimeBinding<Spec>
where
    Self::Sut: Send + 'static,
{
}

pub trait GeneratedBinding<Spec: SpecOracle>: RuntimeBinding<Spec> {
    fn generated_fixture() -> SharedFixtureValue;

    fn generated_snapshot_fixture(value: &Value) -> Result<Self::Fixture, HarnessError> {
        let _ = value;
        Err(HarnessError::Binding(
            "snapshot fixture replay requires a deserializable fixture type".to_owned(),
        ))
    }

    fn generated_action_candidates(
        spec: &Spec,
        seeds: &SeedProfile<Spec>,
    ) -> Result<Vec<Spec::Action>, HarnessError>;

    fn run_generated_profile(
        spec: &Spec,
        metadata: &GeneratedSpecMetadata,
        profile: &TestProfile<Spec>,
        binding_name: &str,
        artifact_dir: &ArtifactDirPolicy,
        materialize_failures: bool,
    ) -> Result<(), HarnessError>
    where
        Spec: TemporalSpec,
        Spec::State: Clone
            + PartialEq
            + FiniteModelDomain
            + Serialize
            + DeserializeOwned
            + Send
            + Sync
            + 'static,
        Spec::Action:
            Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
        Spec::ExpectedOutput:
            Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static;

    fn run_generated_trace_profile(
        spec: &Spec,
        metadata: &GeneratedSpecMetadata,
        profile: &TestProfile<Spec>,
        binding_name: &str,
        artifact_dir: &ArtifactDirPolicy,
        materialize_failures: bool,
    ) -> Result<(), HarnessError>
    where
        Spec: TemporalSpec,
        Spec::State: Clone
            + PartialEq
            + FiniteModelDomain
            + Serialize
            + DeserializeOwned
            + Send
            + Sync
            + 'static,
        Spec::Action:
            Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
        Spec::ExpectedOutput:
            Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static;

    fn run_generated_concurrent_profile(
        spec: &Spec,
        metadata: &GeneratedSpecMetadata,
        profile: &TestProfile<Spec>,
        binding_name: &str,
        artifact_dir: &ArtifactDirPolicy,
        materialize_failures: bool,
    ) -> Result<(), HarnessError>
    where
        Spec: FrontendSpec,
        Spec::State: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static,
        Spec::Action:
            Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
        Spec::ExpectedOutput:
            Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static;

    fn supports_trace() -> bool {
        false
    }

    fn supports_concurrency() -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessKind {
    CanonicalPositive,
    Positive,
    Negative,
}

pub trait ProtocolInputWitnessCodec<Action>: Clone + Debug {
    fn canonical_positive(action: &Action) -> Self;

    fn positive_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    fn negative_family(action: &Action) -> Vec<Self> {
        vec![Self::canonical_positive(action)]
    }

    fn witness_name(_action: &Action, kind: WitnessKind, index: usize) -> String {
        match kind {
            WitnessKind::CanonicalPositive => "principal".to_owned(),
            WitnessKind::Positive => format!("positive_{index}"),
            WitnessKind::Negative => format!("negative_{index}"),
        }
    }
}

pub type SharedFixtureValue = Arc<dyn Any + Send + Sync>;
type SharedFixtureFactory = Arc<dyn Fn() -> SharedFixtureValue + Send + Sync + 'static>;

pub trait SnapshotFixture: Sized {
    fn from_snapshot(value: &Value) -> Result<Self, HarnessError>;
}

impl<T> SnapshotFixture for T
where
    T: DeserializeOwned,
{
    fn from_snapshot(value: &Value) -> Result<Self, HarnessError> {
        serde_json::from_value(value.clone()).map_err(|error| {
            HarnessError::Binding(format!("failed to decode fixture snapshot: {error}"))
        })
    }
}

struct SnapshotFixtureProbe<T>(PhantomData<T>);

trait MaybeDecodeSnapshotFixture<T> {
    fn decode(self, value: &Value) -> Option<Result<T, HarnessError>>;
}

impl<T> MaybeDecodeSnapshotFixture<T> for &SnapshotFixtureProbe<T>
where
    T: SnapshotFixture,
{
    fn decode(self, value: &Value) -> Option<Result<T, HarnessError>> {
        let _ = self;
        Some(T::from_snapshot(value))
    }
}

impl<T> MaybeDecodeSnapshotFixture<T> for &&SnapshotFixtureProbe<T> {
    fn decode(self, _value: &Value) -> Option<Result<T, HarnessError>> {
        let _ = self;
        None
    }
}

pub fn decode_snapshot_fixture<T>(value: &Value) -> Result<T, HarnessError> {
    (&SnapshotFixtureProbe::<T>(PhantomData))
        .decode(value)
        .unwrap_or_else(|| {
            Err(HarnessError::Binding(
                "snapshot fixture replay requires a deserializable fixture type".to_owned(),
            ))
        })
}

struct DefaultFixtureProbe<T>(PhantomData<T>);

trait MaybeConstructDefaultFixture<T> {
    fn construct_default(self) -> Option<T>;
}

impl<T> MaybeConstructDefaultFixture<T> for &DefaultFixtureProbe<T>
where
    T: Default,
{
    fn construct_default(self) -> Option<T> {
        let _ = self;
        Some(T::default())
    }
}

impl<T> MaybeConstructDefaultFixture<T> for &&DefaultFixtureProbe<T> {
    fn construct_default(self) -> Option<T> {
        let _ = self;
        None
    }
}

pub fn inferred_default_fixture<T>() -> Option<T> {
    (&DefaultFixtureProbe::<T>(PhantomData)).construct_default()
}

#[derive(Clone)]
pub enum FixtureSeed {
    Default,
    Factory(fn() -> SharedFixtureValue),
    Snapshot(Value),
    Value(SharedFixtureFactory),
}

impl Debug for FixtureSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("FixtureSeed::Default"),
            Self::Factory(_) => f.write_str("FixtureSeed::Factory(..)"),
            Self::Snapshot(value) => f.debug_tuple("FixtureSeed::Snapshot").field(value).finish(),
            Self::Value(_) => f.write_str("FixtureSeed::Value(..)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShrinkPolicy {
    None,
    ReplayOnly,
}

impl Default for ShrinkPolicy {
    fn default() -> Self {
        Self::ReplayOnly
    }
}

#[derive(Debug, Clone)]
pub struct SeedProfile<Spec: FrontendSpec> {
    pub label: &'static str,
    pub fixture: FixtureSeed,
    pub initial_state: Option<Spec::State>,
    pub typed: BTreeMap<String, Vec<Value>>,
    pub actions: BTreeMap<String, Vec<Spec::Action>>,
    pub environment: TestEnvironment,
    pub shrink: ShrinkPolicy,
}

impl<Spec> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    pub fn with(mut self, overrides: SeedOverrideSet<Spec>) -> Self {
        if let Some(fixture) = overrides.fixture {
            self.fixture = fixture;
        }
        if let Some(initial_state) = overrides.initial_state {
            self.initial_state = Some(initial_state);
        }
        self.typed.extend(overrides.typed);
        self.actions.extend(overrides.actions);
        if let Some(rng_seed) = overrides.rng_seed {
            self.environment.rng_seed = rng_seed;
        }
        if let Some(clock_seed) = overrides.clock_seed {
            self.environment.clock_seed = clock_seed;
        }
        if let Some(schedule_seed) = overrides.schedule_seed {
            self.environment.schedule_seed = schedule_seed;
        }
        self
    }

    pub fn with_fixture<T>(mut self, fixture: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.fixture = FixtureSeed::Value(Arc::new(move || Arc::new(fixture.clone())));
        self
    }

    pub fn with_fixture_factory(mut self, factory: fn() -> SharedFixtureValue) -> Self {
        self.fixture = FixtureSeed::Factory(factory);
        self
    }

    pub fn with_initial_state(mut self, state: Spec::State) -> Self {
        self.initial_state = Some(state);
        self
    }

    pub fn with_seed<T, I>(mut self, values: I) -> Self
    where
        T: Serialize + 'static,
        I: IntoIterator<Item = T>,
    {
        self.typed.insert(
            std::any::type_name::<T>().to_owned(),
            values
                .into_iter()
                .map(|value| serde_json::to_value(value).expect("type seed should serialize"))
                .collect(),
        );
        self
    }

    pub fn with_strategy<T, S>(mut self, strategy: S) -> Self
    where
        T: Serialize + 'static,
        S: Strategy<Value = T>,
    {
        self.typed.insert(
            std::any::type_name::<T>().to_owned(),
            sample_strategy_values(strategy, self.label)
                .expect("type strategy should serialize deterministic seeds"),
        );
        self
    }

    pub fn with_action_seed<I>(mut self, label: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = Spec::Action>,
    {
        self.actions
            .insert(label.into(), values.into_iter().collect());
        self
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.environment.rng_seed = seed;
        self
    }

    pub fn with_clock_seed(mut self, seed: u64) -> Self {
        self.environment.clock_seed = seed;
        self
    }

    pub fn with_schedule_seed(mut self, seed: u64) -> Self {
        self.environment.schedule_seed = seed;
        self
    }
}

#[derive(Debug, Clone)]
pub struct SeedOverrideSet<Spec: FrontendSpec> {
    pub fixture: Option<FixtureSeed>,
    pub initial_state: Option<Spec::State>,
    pub typed: BTreeMap<String, Vec<Value>>,
    pub actions: BTreeMap<String, Vec<Spec::Action>>,
    pub rng_seed: Option<u64>,
    pub clock_seed: Option<u64>,
    pub schedule_seed: Option<u64>,
}

impl<Spec> Default for SeedOverrideSet<Spec>
where
    Spec: FrontendSpec,
{
    fn default() -> Self {
        Self {
            fixture: None,
            initial_state: None,
            typed: BTreeMap::new(),
            actions: BTreeMap::new(),
            rng_seed: None,
            clock_seed: None,
            schedule_seed: None,
        }
    }
}

impl<Spec> SeedOverrideSet<Spec>
where
    Spec: FrontendSpec,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, overrides: Self) -> Self {
        if let Some(fixture) = overrides.fixture {
            self.fixture = Some(fixture);
        }
        if let Some(initial_state) = overrides.initial_state {
            self.initial_state = Some(initial_state);
        }
        self.typed.extend(overrides.typed);
        self.actions.extend(overrides.actions);
        if let Some(rng_seed) = overrides.rng_seed {
            self.rng_seed = Some(rng_seed);
        }
        if let Some(clock_seed) = overrides.clock_seed {
            self.clock_seed = Some(clock_seed);
        }
        if let Some(schedule_seed) = overrides.schedule_seed {
            self.schedule_seed = Some(schedule_seed);
        }
        self
    }

    pub fn with_fixture<T>(mut self, fixture: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.fixture = Some(FixtureSeed::Value(Arc::new(move || {
            Arc::new(fixture.clone())
        })));
        self
    }

    pub fn with_initial_state(mut self, state: Spec::State) -> Self {
        self.initial_state = Some(state);
        self
    }

    pub fn with_type_seed<T, I>(mut self, values: I) -> Self
    where
        T: Serialize + 'static,
        I: IntoIterator<Item = T>,
    {
        self.typed.insert(
            std::any::type_name::<T>().to_owned(),
            values
                .into_iter()
                .map(|value| serde_json::to_value(value).expect("type seed should serialize"))
                .collect(),
        );
        self
    }

    pub fn with_strategy<T, S>(mut self, strategy: S) -> Self
    where
        T: Serialize + 'static,
        S: Strategy<Value = T>,
    {
        self.typed.insert(
            std::any::type_name::<T>().to_owned(),
            sample_strategy_values(strategy, "override")
                .expect("type strategy should serialize deterministic seeds"),
        );
        self
    }

    pub fn with_action_seed<I>(mut self, label: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = Spec::Action>,
    {
        self.actions
            .insert(label.into(), values.into_iter().collect());
        self
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = Some(seed);
        self
    }

    pub fn with_clock_seed(mut self, seed: u64) -> Self {
        self.clock_seed = Some(seed);
        self
    }

    pub fn with_schedule_seed(mut self, seed: u64) -> Self {
        self.schedule_seed = Some(seed);
        self
    }
}

#[macro_export]
macro_rules! seeds {
    () => {
        $crate::SeedOverrideSet::new()
    };
    ($($tokens:tt)+) => {{
        let overrides = $crate::SeedOverrideSet::new();
        $crate::__nirvash_seed_overrides!(overrides; $($tokens)+)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nirvash_seed_overrides {
    ($overrides:expr;) => {
        $overrides
    };
    ($overrides:expr; fixture = $fixture:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_fixture($fixture);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; initial_state = $state:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_initial_state($state);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; strategy $ty:ty = $strategy:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_strategy::<$ty, _>($strategy);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; type $ty:ty = [$($values:expr),* $(,)?]; $($rest:tt)*) => {{
        let overrides = $overrides.with_type_seed::<$ty, _>([$($values),*]);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; action $label:ident = [$($values:expr),* $(,)?]; $($rest:tt)*) => {{
        let overrides = $overrides.with_action_seed(stringify!($label), [$($values),*]);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; rng = $seed:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_rng_seed($seed);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; clock = $seed:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_clock_seed($seed);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
    ($overrides:expr; schedule = $seed:expr; $($rest:tt)*) => {{
        let overrides = $overrides.with_schedule_seed($seed);
        $crate::__nirvash_seed_overrides!(overrides; $($rest)*)
    }};
}

#[macro_export]
macro_rules! profiles {
    ($($profile:expr),* $(,)?) => {
        vec![$($profile),*]
    };
}

pub fn small<Spec>() -> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    SeedProfile {
        label: "small",
        fixture: FixtureSeed::Default,
        initial_state: None,
        typed: BTreeMap::new(),
        actions: BTreeMap::new(),
        environment: TestEnvironment::default(),
        shrink: ShrinkPolicy::ReplayOnly,
    }
}

pub fn boundary<Spec>() -> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    let mut profile = small::<Spec>();
    profile.label = "boundary";
    profile
}

pub fn concurrent_small<Spec>() -> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    let mut profile = small::<Spec>();
    profile.label = "concurrent_small";
    profile.environment.schedule_seed = 1;
    profile
}

pub fn e2e_default<Spec>() -> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    let mut profile = small::<Spec>();
    profile.label = "e2e_default";
    profile
}

pub fn soak<Spec>() -> SeedProfile<Spec>
where
    Spec: FrontendSpec,
{
    let mut profile = small::<Spec>();
    profile.label = "soak";
    profile.environment.rng_seed = 7;
    profile.environment.schedule_seed = 7;
    profile
}

pub fn small_for<Spec>(spec: &Spec) -> SeedProfile<Spec>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut profile = small::<Spec>();
    apply_auto_seeds(spec, &mut profile);
    profile
}

pub fn boundary_for<Spec>(spec: &Spec) -> SeedProfile<Spec>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut profile = boundary::<Spec>();
    apply_auto_seeds(spec, &mut profile);
    profile
}

pub fn concurrent_small_for<Spec>(spec: &Spec) -> SeedProfile<Spec>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut profile = concurrent_small::<Spec>();
    apply_auto_seeds(spec, &mut profile);
    profile
}

pub fn e2e_default_for<Spec>(spec: &Spec) -> SeedProfile<Spec>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut profile = e2e_default::<Spec>();
    apply_auto_seeds(spec, &mut profile);
    profile
}

pub fn soak_for<Spec>(spec: &Spec) -> SeedProfile<Spec>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut profile = soak::<Spec>();
    apply_auto_seeds(spec, &mut profile);
    profile
}

pub fn small_keys<Spec, I, S>(keys: I) -> SeedOverrideSet<Spec>
where
    Spec: FrontendSpec,
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    SeedOverrideSet::new().with_type_seed::<String, _>(keys.into_iter().map(Into::into))
}

pub fn boundary_numbers<Spec, T>() -> SeedOverrideSet<Spec>
where
    Spec: FrontendSpec,
    T: From<u8> + Serialize + 'static,
{
    SeedOverrideSet::new()
        .with_type_seed::<T, _>([0_u8, 1_u8, 2_u8, 255_u8].into_iter().map(T::from))
}

pub fn smoke_fixture<Spec, T>(fixture: T) -> SeedOverrideSet<Spec>
where
    Spec: FrontendSpec,
    T: Clone + Send + Sync + 'static,
{
    SeedOverrideSet::new().with_fixture(fixture)
}

fn apply_auto_seeds<Spec>(spec: &Spec, profile: &mut SeedProfile<Spec>)
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    apply_auto_typed_seeds(spec, profile);
    let auto_actions = auto_action_candidates(spec);
    if !auto_actions.is_empty() {
        profile.actions.insert("~auto".to_owned(), auto_actions);
    }
}

fn apply_auto_typed_seeds<Spec>(spec: &Spec, profile: &mut SeedProfile<Spec>)
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain + Serialize,
    Spec::Action: PartialEq + FiniteModelDomain + Serialize,
{
    let mut state_values = Vec::new();
    for state in Spec::State::finite_domain().into_vec() {
        push_unique_json(
            &mut state_values,
            serde_json::to_value(state).expect("state seed should serialize"),
        );
    }

    let mut action_values = Vec::new();
    for action in Spec::Action::finite_domain().into_vec() {
        push_unique_json(
            &mut action_values,
            serde_json::to_value(action).expect("action seed should serialize"),
        );
    }

    let mut lowering_cx = LoweringCx;
    if let Ok(lowered) = spec.lower(&mut lowering_cx) {
        let boundaries = lowered.generated_test_boundaries();
        let boundary_states = boundaries.boundary_states;
        let initial_states = boundaries.initial_states;
        let terminal_states = boundaries.terminal_states;
        let boundary_literals = boundaries.catalog;
        let transitions = boundaries.transitions;
        for state in initial_states
            .into_iter()
            .chain(boundary_states)
            .chain(terminal_states)
        {
            push_unique_json(
                &mut state_values,
                serde_json::to_value(state).expect("state seed should serialize"),
            );
        }
        for action in lowered.generated_test_domains().actions {
            push_unique_json(
                &mut action_values,
                serde_json::to_value(action).expect("action seed should serialize"),
            );
        }
        for transition in transitions {
            push_unique_json(
                &mut action_values,
                serde_json::to_value(transition.action).expect("action seed should serialize"),
            );
        }
        insert_boundary_literal_seeds(profile, &boundary_literals);
    }

    if !state_values.is_empty() {
        profile.typed.insert(
            std::any::type_name::<Spec::State>().to_owned(),
            state_values,
        );
    }
    if !action_values.is_empty() {
        profile.typed.insert(
            std::any::type_name::<Spec::Action>().to_owned(),
            action_values,
        );
    }
}

fn auto_action_candidates<Spec>(spec: &Spec) -> Vec<Spec::Action>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::State: PartialEq + FiniteModelDomain,
    Spec::Action: PartialEq,
{
    let mut actions = Vec::new();
    for action in spec.actions() {
        push_unique(&mut actions, action);
    }

    let mut lowering_cx = LoweringCx;
    if let Ok(lowered) = spec.lower(&mut lowering_cx) {
        for action in lowered.generated_test_domains().actions {
            push_unique(&mut actions, action);
        }
        for transition in lowered.generated_test_boundaries().transitions {
            push_unique(&mut actions, transition.action);
        }
    }

    prune_action_candidates(spec, actions)
}

fn push_unique<T>(items: &mut Vec<T>, value: T)
where
    T: PartialEq,
{
    if !items.contains(&value) {
        items.push(value);
    }
}

fn push_unique_json(items: &mut Vec<Value>, value: Value) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn insert_typed_seed_json<Spec, T>(profile: &mut SeedProfile<Spec>, value: Value)
where
    Spec: FrontendSpec,
    T: 'static,
{
    let key = std::any::type_name::<T>().to_owned();
    let entry = profile.typed.entry(key).or_default();
    push_unique_json(entry, value);
}

fn insert_boundary_literal_seeds<Spec>(
    profile: &mut SeedProfile<Spec>,
    catalog: &nirvash_lower::BoundaryLiteralCatalog,
) where
    Spec: FrontendSpec,
{
    for literal in &catalog.comparison_literals {
        insert_boundary_scalar_seed::<Spec>(profile, literal);
    }
    for literal in &catalog.state_literals {
        insert_boundary_scalar_seed::<Spec>(profile, literal);
    }
    for literal in &catalog.action_literals {
        insert_typed_seed_json::<Spec, String>(profile, Value::String(literal.clone()));
    }
    for threshold in &catalog.cardinality_thresholds {
        let as_u64 = *threshold as u64;
        let as_i64 = *threshold as i64;
        insert_typed_seed_json::<Spec, usize>(profile, json!(threshold));
        insert_typed_seed_json::<Spec, u64>(profile, json!(as_u64));
        insert_typed_seed_json::<Spec, u32>(profile, json!(as_u64 as u32));
        insert_typed_seed_json::<Spec, i64>(profile, json!(as_i64));
        insert_typed_seed_json::<Spec, i32>(profile, json!(as_i64 as i32));
        insert_typed_seed_json::<Spec, isize>(profile, json!(as_i64 as isize));
    }
}

fn insert_boundary_scalar_seed<Spec>(profile: &mut SeedProfile<Spec>, literal: &str)
where
    Spec: FrontendSpec,
{
    if let Ok(value) = serde_json::from_str::<Value>(literal) {
        match value {
            Value::Bool(_) => insert_typed_seed_json::<Spec, bool>(profile, value),
            Value::Number(number) => {
                if let Some(unsigned) = number.as_u64() {
                    insert_typed_seed_json::<Spec, u64>(profile, json!(unsigned));
                    insert_typed_seed_json::<Spec, usize>(profile, json!(unsigned as usize));
                    insert_typed_seed_json::<Spec, u32>(profile, json!(unsigned as u32));
                }
                if let Some(signed) = number.as_i64() {
                    insert_typed_seed_json::<Spec, i64>(profile, json!(signed));
                    insert_typed_seed_json::<Spec, i32>(profile, json!(signed as i32));
                    insert_typed_seed_json::<Spec, isize>(profile, json!(signed as isize));
                }
            }
            Value::String(_) => insert_typed_seed_json::<Spec, String>(profile, value),
            _ => {}
        }
        return;
    }

    match literal {
        "true" => insert_typed_seed_json::<Spec, bool>(profile, json!(true)),
        "false" => insert_typed_seed_json::<Spec, bool>(profile, json!(false)),
        _ => insert_typed_seed_json::<Spec, String>(profile, Value::String(literal.to_owned())),
    }
}

fn prune_action_candidates<Spec>(spec: &Spec, candidates: Vec<Spec::Action>) -> Vec<Spec::Action>
where
    Spec: FrontendSpec,
    Spec::State: FiniteModelDomain,
    Spec::Action: PartialEq,
{
    let declared = spec.actions();
    let states = Spec::State::finite_domain().into_vec();
    let mut pruned = Vec::new();
    for action in candidates {
        let declared_match = declared.iter().any(|candidate| candidate == &action);
        let enabled_from_known_state = states
            .iter()
            .any(|state| !spec.transition_relation(state, &action).is_empty());
        if declared_match || enabled_from_known_state {
            push_unique(&mut pruned, action);
        }
    }
    if pruned.is_empty() { declared } else { pruned }
}

pub fn typed_seed_values<T, Spec>(seeds: &SeedProfile<Spec>) -> Result<Vec<T>, HarnessError>
where
    T: Clone + PartialEq + DeserializeOwned + 'static,
    Spec: FrontendSpec,
{
    let key = std::any::type_name::<T>();
    let Some(values) = seeds.typed.get(key) else {
        return Ok(Vec::new());
    };
    let mut decoded = Vec::new();
    for value in values {
        let candidate = serde_json::from_value::<T>(value.clone()).map_err(|error| {
            HarnessError::Binding(format!(
                "failed to decode typed seed `{key}` from generated profile: {error}"
            ))
        })?;
        push_unique(&mut decoded, candidate);
    }
    Ok(decoded)
}

pub fn push_unique_action<A>(items: &mut Vec<A>, value: A)
where
    A: PartialEq,
{
    push_unique(items, value);
}

#[doc(hidden)]
pub fn push_unique_seed_value<T>(items: &mut Vec<T>, value: T)
where
    T: PartialEq,
{
    push_unique(items, value);
}

pub fn typed_seed_candidates<T, Spec>(seeds: &SeedProfile<Spec>) -> Result<Vec<T>, HarnessError>
where
    Spec: FrontendSpec,
    T: Clone + DeserializeOwned + PartialEq + Serialize + 'static,
{
    let key = std::any::type_name::<T>().to_owned();
    let mut values = Vec::new();
    append_registered_finite_domain_values::<T>(&mut values);
    if let Some(serialized) = seeds.typed.get(&key) {
        for value in serialized {
            let decoded = T::from_snapshot(value)?;
            push_unique(&mut values, decoded);
        }
    }
    append_registered_strategy_values::<T>(&mut values, seeds.label)?;
    append_optional_arbitrary_values::<T>(&mut values, seeds.label);
    append_optional_default_value::<T>(&mut values);
    append_builtin_singleton_values::<T>(&mut values);
    Ok(values)
}

fn append_registered_finite_domain_values<T>(items: &mut Vec<T>)
where
    T: PartialEq + 'static,
{
    for value in nirvash::lookup_finite_domain_seed_values::<T>() {
        push_unique(items, value);
    }
}

#[doc(hidden)]
pub fn append_registered_strategy_seed_values<T>(
    items: &mut Vec<T>,
    profile_label: &str,
) -> Result<(), HarnessError>
where
    T: Clone + DeserializeOwned + PartialEq + 'static,
{
    append_registered_strategy_values(items, profile_label)
}

type SerializedSeedStrategy =
    Arc<dyn Fn(&str) -> Result<Vec<Value>, HarnessError> + Send + Sync + 'static>;

fn registered_seed_strategies() -> &'static Mutex<BTreeMap<String, SerializedSeedStrategy>> {
    static REGISTERED: OnceLock<Mutex<BTreeMap<String, SerializedSeedStrategy>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn register_seed_strategy<T, S, F>(factory: F)
where
    T: Serialize + 'static,
    S: Strategy<Value = T> + 'static,
    F: Fn() -> S + Send + Sync + 'static,
{
    let key = std::any::type_name::<T>().to_owned();
    let strategy: SerializedSeedStrategy =
        Arc::new(move |profile_label| sample_strategy_values(factory(), profile_label));
    registered_seed_strategies()
        .lock()
        .expect("registered seed strategy lock")
        .insert(key, strategy);
}

fn append_registered_strategy_values<T>(
    items: &mut Vec<T>,
    profile_label: &str,
) -> Result<(), HarnessError>
where
    T: Clone + DeserializeOwned + PartialEq + 'static,
{
    let key = std::any::type_name::<T>().to_owned();
    let strategy = registered_seed_strategies()
        .lock()
        .expect("registered seed strategy lock")
        .get(&key)
        .cloned();
    let Some(strategy) = strategy else {
        return Ok(());
    };
    for value in strategy(profile_label)? {
        let decoded = serde_json::from_value::<T>(value).map_err(|error| {
            HarnessError::Binding(format!(
                "failed to decode registered strategy seed `{key}`: {error}"
            ))
        })?;
        push_unique(items, decoded);
    }
    Ok(())
}

fn sample_strategy_values<T, S>(
    strategy: S,
    profile_label: &str,
) -> Result<Vec<Value>, HarnessError>
where
    T: Serialize,
    S: Strategy<Value = T>,
{
    let seed = deterministic_seed_bytes(std::any::type_name::<T>(), profile_label);
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::from_seed(RngAlgorithm::default(), &seed),
    );
    let mut values = Vec::new();
    for _ in 0..16 {
        if let Ok(tree) = strategy.new_tree(&mut runner) {
            push_unique_json(
                &mut values,
                serde_json::to_value(tree.current()).map_err(|error| {
                    HarnessError::Binding(format!(
                        "failed to encode strategy seed `{}`: {error}",
                        std::any::type_name::<T>()
                    ))
                })?,
            );
        }
    }
    values.sort_by(|lhs, rhs| {
        serde_json::to_string(lhs)
            .expect("serialized strategy seed should stringify")
            .cmp(&serde_json::to_string(rhs).expect("serialized strategy seed should stringify"))
    });
    Ok(values)
}

struct AutoSeedProbe<T>(PhantomData<T>);

trait MaybeAppendArbitrary<T> {
    fn append_arbitrary(self, items: &mut Vec<T>, profile_label: &str);
}

impl<T> MaybeAppendArbitrary<T> for &AutoSeedProbe<T>
where
    T: Arbitrary + Clone + PartialEq + 'static,
{
    fn append_arbitrary(self, items: &mut Vec<T>, profile_label: &str) {
        let _ = self;
        append_deterministic_arbitrary_values(items, profile_label);
    }
}

impl<T> MaybeAppendArbitrary<T> for &&AutoSeedProbe<T> {
    fn append_arbitrary(self, _items: &mut Vec<T>, _profile_label: &str) {
        let _ = self;
    }
}

trait MaybeAppendDefault<T> {
    fn append_default(self, items: &mut Vec<T>);
}

impl<T> MaybeAppendDefault<T> for &AutoSeedProbe<T>
where
    T: Clone + Default + PartialEq,
{
    fn append_default(self, items: &mut Vec<T>) {
        let _ = self;
        push_unique(items, T::default());
    }
}

impl<T> MaybeAppendDefault<T> for &&AutoSeedProbe<T> {
    fn append_default(self, _items: &mut Vec<T>) {
        let _ = self;
    }
}

fn append_optional_arbitrary_values<T>(items: &mut Vec<T>, profile_label: &str)
where
    T: Clone + PartialEq + 'static,
{
    (&AutoSeedProbe::<T>(PhantomData)).append_arbitrary(items, profile_label);
}

fn append_optional_default_value<T>(items: &mut Vec<T>)
where
    T: Clone + PartialEq,
{
    (&AutoSeedProbe::<T>(PhantomData)).append_default(items);
}

fn append_deterministic_arbitrary_values<T>(items: &mut Vec<T>, profile_label: &str)
where
    T: Arbitrary + Clone + PartialEq + 'static,
{
    let seed = deterministic_seed_bytes(std::any::type_name::<T>(), profile_label);
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::from_seed(RngAlgorithm::default(), &seed),
    );
    let strategy = <T as Arbitrary>::arbitrary();
    for _ in 0..16 {
        if let Ok(tree) = strategy.new_tree(&mut runner) {
            push_unique(items, tree.current());
        }
    }
}

#[doc(hidden)]
pub fn append_deterministic_arbitrary_seed_values<T>(items: &mut Vec<T>, profile_label: &str)
where
    T: Arbitrary + Clone + PartialEq + 'static,
{
    append_deterministic_arbitrary_values(items, profile_label);
}

#[doc(hidden)]
pub fn append_builtin_singleton_seed_values<T>(items: &mut Vec<T>)
where
    T: Clone + DeserializeOwned + PartialEq + 'static,
{
    append_builtin_singleton_values(items);
}

#[doc(hidden)]
pub fn finite_domain_seed_values<T>() -> Vec<T>
where
    T: FiniteModelDomain,
{
    T::finite_domain().into_vec()
}

#[macro_export]
macro_rules! monomorphic_typed_seed_candidates {
    ($ty:ty, $spec_ty:ty, $seeds:expr) => {{
        let __nirvash_seeds = $seeds;
        let mut __nirvash_values = ::std::vec::Vec::<$ty>::new();
        for __nirvash_value in ::nirvash::lookup_finite_domain_seed_values::<$ty>() {
            ::nirvash_conformance::push_unique_seed_value(&mut __nirvash_values, __nirvash_value);
        }
        {
            struct __NirvashFiniteDomainProbe<T>(::core::marker::PhantomData<T>);

            trait __NirvashMaybeFiniteDomain<T> {
                fn maybe_values(self) -> ::std::vec::Vec<T>;
            }

            impl<T> __NirvashMaybeFiniteDomain<T> for &__NirvashFiniteDomainProbe<T>
            where
                T: ::nirvash_lower::FiniteModelDomain,
            {
                fn maybe_values(self) -> ::std::vec::Vec<T> {
                    let _ = self;
                    ::nirvash_conformance::finite_domain_seed_values::<T>()
                }
            }

            impl<T> __NirvashMaybeFiniteDomain<T> for &&__NirvashFiniteDomainProbe<T> {
                fn maybe_values(self) -> ::std::vec::Vec<T> {
                    let _ = self;
                    ::std::vec::Vec::new()
                }
            }

            for __nirvash_value in
                (&__NirvashFiniteDomainProbe::<$ty>(::core::marker::PhantomData)).maybe_values()
            {
                ::nirvash_conformance::push_unique_seed_value(
                    &mut __nirvash_values,
                    __nirvash_value,
                );
            }
        }
        for __nirvash_value in
            ::nirvash_conformance::typed_seed_values::<$ty, $spec_ty>(__nirvash_seeds)?
        {
            ::nirvash_conformance::push_unique_seed_value(&mut __nirvash_values, __nirvash_value);
        }
        ::nirvash_conformance::append_registered_strategy_seed_values::<$ty>(
            &mut __nirvash_values,
            __nirvash_seeds.label,
        )?;
        {
            struct __NirvashArbitraryProbe<T>(::core::marker::PhantomData<T>);

            trait __NirvashMaybeArbitrary<T> {
                fn append(self, items: &mut ::std::vec::Vec<T>, profile_label: &str);
            }

            impl<T> __NirvashMaybeArbitrary<T> for &__NirvashArbitraryProbe<T>
            where
                T: ::nirvash_conformance::__nirvash_proptest::arbitrary::Arbitrary
                    + Clone
                    + PartialEq
                    + 'static,
            {
                fn append(self, items: &mut ::std::vec::Vec<T>, profile_label: &str) {
                    let _ = self;
                    ::nirvash_conformance::append_deterministic_arbitrary_seed_values::<T>(
                        items,
                        profile_label,
                    );
                }
            }

            impl<T> __NirvashMaybeArbitrary<T> for &&__NirvashArbitraryProbe<T> {
                fn append(self, _items: &mut ::std::vec::Vec<T>, _profile_label: &str) {
                    let _ = self;
                }
            }

            (&__NirvashArbitraryProbe::<$ty>(::core::marker::PhantomData))
                .append(&mut __nirvash_values, __nirvash_seeds.label);
        }
        {
            struct __NirvashDefaultProbe<T>(::core::marker::PhantomData<T>);

            trait __NirvashMaybeDefault<T> {
                fn append(self, items: &mut ::std::vec::Vec<T>);
            }

            impl<T> __NirvashMaybeDefault<T> for &__NirvashDefaultProbe<T>
            where
                T: ::core::default::Default + Clone + PartialEq,
            {
                fn append(self, items: &mut ::std::vec::Vec<T>) {
                    let _ = self;
                    ::nirvash_conformance::push_unique_seed_value(
                        items,
                        <T as ::core::default::Default>::default(),
                    );
                }
            }

            impl<T> __NirvashMaybeDefault<T> for &&__NirvashDefaultProbe<T> {
                fn append(self, _items: &mut ::std::vec::Vec<T>) {
                    let _ = self;
                }
            }

            (&__NirvashDefaultProbe::<$ty>(::core::marker::PhantomData))
                .append(&mut __nirvash_values);
        }
        ::nirvash_conformance::append_builtin_singleton_seed_values::<$ty>(&mut __nirvash_values);
        __nirvash_values
    }};
}

fn append_builtin_singleton_values<T>(items: &mut Vec<T>)
where
    T: Clone + DeserializeOwned + PartialEq + 'static,
{
    for candidate in [
        Value::Null,
        json!(false),
        json!(true),
        json!(0),
        json!(1),
        json!(""),
        json!([]),
        json!({}),
    ] {
        if let Ok(decoded) = serde_json::from_value::<T>(candidate) {
            push_unique(items, decoded);
        }
    }
}

fn deterministic_seed_bytes(type_label: &str, profile_label: &str) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    let mut state = stable_hash64(type_label);
    state ^= stable_hash64(profile_label).rotate_left(13);
    state ^= 0x9E37_79B9_7F4A_7C15_u64;
    for chunk in bytes.chunks_mut(8) {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    bytes
}

fn stable_hash64(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CoverageGoal {
    Transitions,
    TransitionPairs(usize),
    GuardBoundaries,
    PropertyPrefixes,
    Goal(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TraceValidationEngine {
    Explicit,
    Symbolic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EnginePlan {
    ExplicitSuite,
    ProptestOnline {
        cases: usize,
        max_steps: usize,
    },
    KaniBounded {
        depth: usize,
    },
    TraceValidation {
        engine: TraceValidationEngine,
    },
    LoomSmall {
        threads: usize,
        max_permutations: usize,
    },
    ShuttlePCT {
        depth: usize,
        runs: usize,
    },
}

#[derive(Debug, Clone)]
pub struct TestProfileBuilder<Spec: FrontendSpec> {
    label: &'static str,
    model_instance: ModelInstance<Spec::State, Spec::Action>,
    seeds: SeedProfile<Spec>,
    coverage: Vec<CoverageGoal>,
    engines: Vec<EnginePlan>,
}

#[derive(Debug, Clone)]
pub struct TestProfile<Spec: FrontendSpec> {
    pub label: &'static str,
    pub model_instance: ModelInstance<Spec::State, Spec::Action>,
    pub seeds: SeedProfile<Spec>,
    pub coverage: Vec<CoverageGoal>,
    pub engines: Vec<EnginePlan>,
}

impl<Spec> TestProfile<Spec>
where
    Spec: FrontendSpec,
{
    pub fn with(mut self, overrides: SeedOverrideSet<Spec>) -> Self {
        self.seeds = self.seeds.with(overrides);
        self
    }

    pub fn with_seeds(mut self, seeds: SeedProfile<Spec>) -> Self {
        self.seeds = seeds;
        self
    }

    pub fn with_coverage(mut self, coverage: impl IntoIterator<Item = CoverageGoal>) -> Self {
        self.coverage = coverage.into_iter().collect();
        self
    }

    pub fn with_engines(mut self, engines: impl IntoIterator<Item = EnginePlan>) -> Self {
        self.engines = engines.into_iter().collect();
        self
    }

    pub fn with_fixture_factory(mut self, factory: fn() -> SharedFixtureValue) -> Self {
        self.seeds = self.seeds.with_fixture_factory(factory);
        self
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.seeds.environment.rng_seed = seed;
        self
    }
}

impl<Spec> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    pub fn new(
        label: &'static str,
        model_instance: ModelInstance<Spec::State, Spec::Action>,
        seeds: SeedProfile<Spec>,
    ) -> Self {
        Self {
            label,
            model_instance,
            seeds,
            coverage: Vec::new(),
            engines: Vec::new(),
        }
    }

    pub fn coverage(mut self, coverage: impl IntoIterator<Item = CoverageGoal>) -> Self {
        self.coverage = coverage.into_iter().collect();
        self
    }

    pub fn engines(mut self, engines: impl IntoIterator<Item = EnginePlan>) -> Self {
        self.engines = engines.into_iter().collect();
        self
    }

    pub fn with_seeds(mut self, seeds: SeedProfile<Spec>) -> Self {
        self.seeds = seeds;
        self
    }

    pub fn with_engines(mut self, engines: Vec<EnginePlan>) -> Self {
        self.engines = engines;
        self
    }

    pub fn push_coverage(mut self, goal: CoverageGoal) -> Self {
        self.coverage.push(goal);
        self
    }

    pub fn push_engine(mut self, engine: EnginePlan) -> Self {
        self.engines.push(engine);
        self
    }

    pub fn with(mut self, overrides: SeedOverrideSet<Spec>) -> Self {
        self.seeds = self.seeds.with(overrides);
        self
    }

    pub fn with_fixture<T>(mut self, fixture: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.seeds = self.seeds.with_fixture(fixture);
        self
    }

    pub fn with_fixture_factory(mut self, factory: fn() -> SharedFixtureValue) -> Self {
        self.seeds = self.seeds.with_fixture_factory(factory);
        self
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.seeds = self.seeds.with_rng_seed(seed);
        self
    }

    pub fn with_clock_seed(mut self, seed: u64) -> Self {
        self.seeds = self.seeds.with_clock_seed(seed);
        self
    }

    pub fn with_schedule_seed(mut self, seed: u64) -> Self {
        self.seeds = self.seeds.with_schedule_seed(seed);
        self
    }

    pub fn with_model_instance(
        mut self,
        model_instance: ModelInstance<Spec::State, Spec::Action>,
    ) -> Self {
        self.model_instance = model_instance;
        self
    }

    pub fn build(self) -> TestProfile<Spec> {
        TestProfile {
            label: self.label,
            model_instance: self.model_instance,
            seeds: self.seeds,
            coverage: self.coverage,
            engines: self.engines,
        }
    }
}

pub fn smoke_default<Spec>(
    model_instance: ModelInstance<Spec::State, Spec::Action>,
) -> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    TestProfileBuilder::new("smoke_default", model_instance, small::<Spec>())
        .coverage([CoverageGoal::Transitions])
        .engines([EnginePlan::ExplicitSuite])
}

pub fn unit_default<Spec>(
    model_instance: ModelInstance<Spec::State, Spec::Action>,
) -> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    TestProfileBuilder::new("unit_default", model_instance, boundary::<Spec>())
        .coverage([
            CoverageGoal::Transitions,
            CoverageGoal::TransitionPairs(2),
            CoverageGoal::GuardBoundaries,
        ])
        .engines([
            EnginePlan::ExplicitSuite,
            EnginePlan::ProptestOnline {
                cases: 256,
                max_steps: 8,
            },
            EnginePlan::KaniBounded { depth: 4 },
        ])
}

pub fn boundary_default<Spec>(
    model_instance: ModelInstance<Spec::State, Spec::Action>,
) -> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    TestProfileBuilder::new("boundary_default", model_instance, boundary::<Spec>())
        .coverage([CoverageGoal::Transitions, CoverageGoal::GuardBoundaries])
        .engines([
            EnginePlan::ExplicitSuite,
            EnginePlan::ProptestOnline {
                cases: 128,
                max_steps: 4,
            },
        ])
}

pub fn e2e_profile<Spec>(
    model_instance: ModelInstance<Spec::State, Spec::Action>,
) -> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    TestProfileBuilder::new("e2e_default", model_instance, e2e_default::<Spec>())
        .coverage([CoverageGoal::PropertyPrefixes])
        .engines([EnginePlan::TraceValidation {
            engine: TraceValidationEngine::Explicit,
        }])
}

pub fn concurrency_default<Spec>(
    model_instance: ModelInstance<Spec::State, Spec::Action>,
) -> TestProfileBuilder<Spec>
where
    Spec: FrontendSpec,
{
    TestProfileBuilder::new(
        "concurrency_default",
        model_instance,
        concurrent_small::<Spec>(),
    )
    .coverage([CoverageGoal::Transitions, CoverageGoal::TransitionPairs(2)])
    .engines([
        EnginePlan::LoomSmall {
            threads: 2,
            max_permutations: 8,
        },
        EnginePlan::ShuttlePCT {
            depth: 2,
            runs: 2000,
        },
    ])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeneratedSpecMetadata {
    pub spec_name: &'static str,
    pub spec_slug: &'static str,
    pub export_module: &'static str,
    pub crate_package: &'static str,
    pub crate_manifest_dir: &'static str,
    pub normalized_fragment: NormalizedFragmentInfo,
    pub default_profiles: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactDirPolicy {
    pub base: PathBuf,
}

impl Default for ArtifactDirPolicy {
    fn default() -> Self {
        Self {
            base: target_nirvash_dir(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedHarnessPlan<Spec: FrontendSpec> {
    pub spec_name: &'static str,
    pub profiles: Vec<TestProfile<Spec>>,
    pub replay_dir: PathBuf,
    pub materialize_failures: bool,
    metadata: GeneratedSpecMetadata,
    artifact_dir: ArtifactDirPolicy,
}

impl<Spec> GeneratedHarnessPlan<Spec>
where
    Spec: FrontendSpec,
{
    pub fn new(
        metadata: GeneratedSpecMetadata,
        profiles: Vec<TestProfile<Spec>>,
        artifact_dir: ArtifactDirPolicy,
        materialize_failures: bool,
    ) -> Self {
        let replay_dir = artifact_dir.base.join("replay");
        Self {
            spec_name: metadata.spec_name,
            profiles,
            replay_dir,
            materialize_failures,
            metadata,
            artifact_dir,
        }
    }

    pub fn metadata(&self) -> &GeneratedSpecMetadata {
        &self.metadata
    }

    pub fn artifact_dir_policy(&self) -> &ArtifactDirPolicy {
        &self.artifact_dir
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObligationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedKaniObligation<A> {
    pub id: String,
    pub actions: Vec<A>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObligationKind<Spec: FrontendSpec> {
    Transition {
        action: Spec::Action,
    },
    TransitionPair {
        prefix: Vec<Spec::Action>,
    },
    GuardBoundary {
        action: Spec::Action,
        label: &'static str,
    },
    PropertyPrefix {
        property: &'static str,
        depth: usize,
    },
    Goal {
        label: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestObligation<Spec: FrontendSpec> {
    pub id: ObligationId,
    pub kind: ObligationKind<Spec>,
    pub trust_floor: TrustTier,
    pub witness_hint: Option<Trace<Spec::State, Spec::Action>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedReplayBundle<S, A, Output> {
    pub spec_name: String,
    pub profile: String,
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obligation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<Value>,
    pub detail: DetailedObservedTrace<S, A, Output>,
    pub action_trace: ObservedActionTrace<S, A, Output>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStepRefinementWitness<S, A> {
    pub index: usize,
    pub abstract_before: S,
    pub step: TraceStep<A>,
    pub abstract_after: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRefinementWitness<S, A> {
    pub abstract_trace: Trace<S, A>,
    pub steps: Vec<TraceStepRefinementWitness<S, A>>,
    pub model_case_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceRefinementError<S, A> {
    InitialStateMismatch {
        model_case_label: String,
        observed_initial: ProjectedState<S>,
        expected_initial: S,
    },
    ShapeMismatch {
        model_case_label: String,
        detail: String,
    },
    StepMismatch {
        model_case_label: String,
        index: usize,
        action: A,
        detail: String,
    },
    NoMatchingCandidate {
        model_case_label: String,
    },
}

impl<S, A> Display for TraceRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitialStateMismatch {
                model_case_label,
                observed_initial,
                expected_initial,
            } => write!(
                f,
                "model case `{model_case_label}` initial state mismatch: observed {observed_initial:?}, expected {expected_initial:?}"
            ),
            Self::ShapeMismatch {
                model_case_label,
                detail,
            } => write!(
                f,
                "model case `{model_case_label}` shape mismatch: {detail}"
            ),
            Self::StepMismatch {
                model_case_label,
                index,
                action,
                detail,
            } => write!(
                f,
                "model case `{model_case_label}` step {index} mismatch for action {action:?}: {detail}"
            ),
            Self::NoMatchingCandidate { model_case_label } => write!(
                f,
                "model case `{model_case_label}` found no candidate trace matching the observed trace"
            ),
        }
    }
}

impl<S, A> std::error::Error for TraceRefinementError<S, A>
where
    S: Debug,
    A: Debug,
{
}

#[derive(Debug)]
pub enum HarnessError {
    Binding(String),
    Refinement(String),
    Artifact(String),
}

impl Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binding(message) => write!(f, "{message}"),
            Self::Refinement(message) => write!(f, "{message}"),
            Self::Artifact(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for HarnessError {}

#[derive(Debug, Clone)]
struct PrefixStep<A> {
    action: A,
}

#[derive(Debug)]
struct ReplayOutcome<S, A, Output> {
    before: ProjectedState<S>,
    after: ProjectedState<S>,
    output: Output,
    detail: DetailedObservedTrace<S, A, Output>,
    action_trace: ObservedActionTrace<S, A, Output>,
}

struct DetailRecorder<A, Output> {
    events: Vec<ObservedEvent<A, Output>>,
}

impl<A, Output> Default for DetailRecorder<A, Output> {
    fn default() -> Self {
        Self { events: Vec::new() }
    }
}

impl<Spec> TraceSink<Spec> for DetailRecorder<Spec::Action, Spec::ExpectedOutput>
where
    Spec: SpecOracle,
{
    fn record_update(&mut self, var: &'static str, value: Value) {
        self.events.push(ObservedEvent::Update {
            var: var.to_owned(),
            value,
        });
    }
}

pub fn step_refines_relation<Spec>(
    spec: &Spec,
    before: &Spec::State,
    action: &Spec::Action,
    after: &Spec::State,
) -> bool
where
    Spec: FrontendSpec,
    Spec::State: PartialEq,
    Spec::Action: PartialEq,
{
    spec.transition_relation(before, action)
        .into_iter()
        .any(|candidate| candidate == *after)
}

pub fn assert_trace_refines<Spec>(
    spec: &Spec,
    model_case: ModelInstance<Spec::State, Spec::Action>,
    observed: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    engine: TraceValidationEngine,
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Send + Sync + 'static,
{
    let model_case_label = model_case.label().to_owned();
    let mut lowering_cx = LoweringCx;
    let lowered =
        spec.lower(&mut lowering_cx)
            .map_err(|error| TraceRefinementError::ShapeMismatch {
                model_case_label: model_case_label.clone(),
                detail: error.to_string(),
            })?;
    let mut config = ModelCheckConfig::bounded_lasso(observed.steps.len() + 1);
    config.backend = Some(match engine {
        TraceValidationEngine::Explicit => ModelBackend::Explicit,
        TraceValidationEngine::Symbolic => ModelBackend::Symbolic,
    });
    let case = model_case.clone().with_checker_config(config);
    let traces = match engine {
        TraceValidationEngine::Explicit => ExplicitModelChecker::for_case(&lowered, case)
            .candidate_traces()
            .map_err(|error| TraceRefinementError::ShapeMismatch {
                model_case_label: model_case_label.clone(),
                detail: format!("explicit trace search failed: {error:?}"),
            })?,
        TraceValidationEngine::Symbolic => {
            let observed_steps = observed
                .steps
                .iter()
                .map(|step| TraceStep::Action(step.action.clone()))
                .collect::<Vec<_>>();
            let mut state_hints = Vec::with_capacity(observed.steps.len() + 1);
            state_hints.push(observed.initial.as_ref());
            state_hints.extend(observed.steps.iter().map(|step| step.after.as_ref()));
            trace_constraints::matching_candidates_for_case(
                &lowered,
                case,
                &state_hints,
                &observed_steps,
            )
            .map_err(|error| TraceRefinementError::ShapeMismatch {
                model_case_label: model_case_label.clone(),
                detail: format!("symbolic constrained checking failed: {error:?}"),
            })?
        }
    };
    validate_against_candidates(spec, model_case.label(), observed, &traces)
}

pub fn require_trace_binding<Spec, Binding>()
where
    Spec: SpecOracle,
    Binding: GeneratedBinding<Spec> + TraceBinding<Spec>,
{
}

pub fn require_concurrent_binding<Spec, Binding>()
where
    Spec: SpecOracle,
    Binding: GeneratedBinding<Spec> + ConcurrentBinding<Spec>,
    Binding::Sut: Send + 'static,
{
}

pub fn run_profile<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    materialize_failures: bool,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    write_manifest(
        metadata,
        profile,
        binding_name,
        artifact_dir,
        materialize_failures,
    )?;
    for engine in &profile.engines {
        match engine {
            EnginePlan::ExplicitSuite => run_explicit_suite::<Spec, Binding>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
            )?,
            EnginePlan::ProptestOnline { cases, max_steps } => {
                run_proptest_online::<Spec, Binding>(
                    spec,
                    metadata,
                    profile,
                    binding_name,
                    artifact_dir,
                    *cases,
                    *max_steps,
                )?
            }
            EnginePlan::KaniBounded { .. } => {
                return Err(HarnessError::Binding(format!(
                    "KaniBounded profiles are materialized at build time; run `cargo nirvash materialize-tests --spec {} --binding {} --profile {}` first",
                    metadata.spec_name, binding_name, profile.label,
                )));
            }
            EnginePlan::TraceValidation { .. } => {
                return Err(HarnessError::Binding(
                    "trace validation profiles must be installed through trace_tests!".to_owned(),
                ));
            }
            EnginePlan::LoomSmall { .. } | EnginePlan::ShuttlePCT { .. } => {
                return Err(HarnessError::Binding(
                    "concurrency profiles must be installed through loom_tests! or tests!(binding = T, profiles = [generated::profiles::concurrency_default()])"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub fn normalized_fragment_info<Spec>(spec: &Spec) -> NormalizedFragmentInfo
where
    Spec: FrontendSpec + TemporalSpec,
{
    let mut lowering_cx = LoweringCx;
    match spec.lower(&mut lowering_cx) {
        Ok(lowered) => match lowered.normalized_core() {
            Ok(normalized) => {
                let profile = normalized.fragment_profile();
                NormalizedFragmentInfo {
                    symbolic_supported: profile.symbolic_supported,
                    proof_supported: profile.proof_supported,
                    has_opaque_nodes: profile.has_opaque_nodes,
                    has_stringly_nodes: profile.has_stringly_nodes,
                    has_temporal_props: profile.has_temporal_props,
                    has_fairness: profile.has_fairness,
                }
            }
            Err(_) => NormalizedFragmentInfo {
                symbolic_supported: false,
                proof_supported: false,
                has_opaque_nodes: true,
                has_stringly_nodes: true,
                has_temporal_props: true,
                has_fairness: true,
            },
        },
        Err(_) => NormalizedFragmentInfo {
            symbolic_supported: false,
            proof_supported: false,
            has_opaque_nodes: true,
            has_stringly_nodes: true,
            has_temporal_props: true,
            has_fairness: true,
        },
    }
}

pub fn run_trace_profile<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    materialize_failures: bool,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec> + TraceBinding<Spec>,
{
    write_manifest(
        metadata,
        profile,
        binding_name,
        artifact_dir,
        materialize_failures,
    )?;
    for engine in &profile.engines {
        match engine {
            EnginePlan::TraceValidation { engine } => run_trace_validation_suite::<Spec, Binding>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                engine.clone(),
            )?,
            EnginePlan::KaniBounded { depth } => run_kani_bounded::<Spec, Binding>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                *depth,
            )?,
            other => {
                return Err(HarnessError::Binding(format!(
                    "trace installer does not support engine {other:?}"
                )));
            }
        }
    }
    Ok(())
}

pub fn run_concurrent_profile<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    materialize_failures: bool,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + FrontendSpec,
    Spec::State: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec> + ConcurrentBinding<Spec>,
    Binding::Sut: Send + 'static,
{
    write_manifest(
        metadata,
        profile,
        binding_name,
        artifact_dir,
        materialize_failures,
    )?;
    for engine in &profile.engines {
        match engine {
            EnginePlan::LoomSmall {
                threads,
                max_permutations,
            } => {
                let execution = run_loom_small::<Spec, Binding>(
                    spec,
                    &profile.seeds,
                    *threads,
                    *max_permutations,
                )?;
                write_schedule_artifact(
                    metadata,
                    profile,
                    binding_name,
                    engine,
                    artifact_dir,
                    Some(&execution.schedule),
                )?;
                if let Err(error) = validate_observed_action_trace(spec, &execution.action_trace) {
                    persist_failure_with_context::<Spec>(
                        metadata,
                        profile,
                        "loom_small",
                        Some(binding_name),
                        None,
                        Some(execution.schedule.clone()),
                        artifact_dir,
                        &execution.detail,
                        &execution.action_trace,
                    )?;
                    return Err(error);
                }
                persist_failure_with_context::<Spec>(
                    metadata,
                    profile,
                    "loom_small",
                    Some(binding_name),
                    None,
                    Some(execution.schedule.clone()),
                    artifact_dir,
                    &execution.detail,
                    &execution.action_trace,
                )?;
            }
            EnginePlan::ShuttlePCT { depth, runs } => {
                let execution =
                    run_shuttle_pct::<Spec, Binding>(spec, &profile.seeds, *depth, *runs)?;
                write_schedule_artifact(
                    metadata,
                    profile,
                    binding_name,
                    engine,
                    artifact_dir,
                    Some(&execution.schedule),
                )?;
                if let Err(error) = validate_observed_action_trace(spec, &execution.action_trace) {
                    persist_failure_with_context::<Spec>(
                        metadata,
                        profile,
                        "shuttle_pct",
                        Some(binding_name),
                        None,
                        Some(execution.schedule.clone()),
                        artifact_dir,
                        &execution.detail,
                        &execution.action_trace,
                    )?;
                    return Err(error);
                }
                persist_failure_with_context::<Spec>(
                    metadata,
                    profile,
                    "shuttle_pct",
                    Some(binding_name),
                    None,
                    Some(execution.schedule.clone()),
                    artifact_dir,
                    &execution.detail,
                    &execution.action_trace,
                )?;
            }
            other => {
                return Err(HarnessError::Binding(format!(
                    "concurrency installer does not support engine {other:?}"
                )));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ConcurrentExecution<S, A, Output> {
    detail: DetailedObservedTrace<S, A, Output>,
    action_trace: ObservedActionTrace<S, A, Output>,
    schedule: Value,
}

pub fn replay_bundle_path(
    artifact_dir: &ArtifactDirPolicy,
    spec_slug: &str,
    profile: &str,
    engine: &str,
    suffix: &str,
) -> PathBuf {
    artifact_dir.base.join("replay").join(format!(
        "{}_{}_{}_{}",
        sanitize_path(spec_slug),
        sanitize_path(profile),
        sanitize_path(engine),
        suffix
    ))
}

pub fn write_replay_bundle<S, A, Output>(
    artifact_dir: &ArtifactDirPolicy,
    spec_slug: &str,
    profile: &str,
    engine: &str,
    bundle: &GeneratedReplayBundle<S, A, Output>,
) -> Result<(PathBuf, PathBuf), HarnessError>
where
    S: Serialize,
    A: Serialize,
    Output: Serialize,
{
    write_serialized_replay_bundle(
        artifact_dir,
        spec_slug,
        profile,
        engine,
        bundle,
        &bundle.detail,
    )
}

fn write_serialized_replay_bundle<S, A, Output>(
    artifact_dir: &ArtifactDirPolicy,
    spec_slug: &str,
    profile: &str,
    engine: &str,
    bundle: &impl Serialize,
    detail: &DetailedObservedTrace<S, A, Output>,
) -> Result<(PathBuf, PathBuf), HarnessError>
where
    S: Serialize,
    A: Serialize,
    Output: Serialize,
{
    let replay_dir = artifact_dir.base.join("replay");
    fs::create_dir_all(&replay_dir).map_err(|error| {
        HarnessError::Artifact(format!("failed to create replay directory: {error}"))
    })?;
    let json_path = replay_bundle_path(artifact_dir, spec_slug, profile, engine, "bundle.json");
    let ndjson_path = replay_bundle_path(artifact_dir, spec_slug, profile, engine, "bundle.ndjson");
    fs::write(
        &json_path,
        serde_json::to_vec_pretty(bundle).map_err(|error| {
            HarnessError::Artifact(format!("failed to serialize replay JSON: {error}"))
        })?,
    )
    .map_err(|error| HarnessError::Artifact(format!("failed to write replay JSON: {error}")))?;

    let mut lines = Vec::new();
    lines.push(
        serde_json::to_string(&json!({
            "kind": "initial",
            "state": detail.initial,
        }))
        .map_err(|error| {
            HarnessError::Artifact(format!("failed to encode initial NDJSON event: {error}"))
        })?,
    );
    for event in &detail.events {
        lines.push(serde_json::to_string(event).map_err(|error| {
            HarnessError::Artifact(format!("failed to encode NDJSON event: {error}"))
        })?);
    }
    fs::write(&ndjson_path, lines.join("\n")).map_err(|error| {
        HarnessError::Artifact(format!("failed to write replay NDJSON: {error}"))
    })?;
    Ok((json_path, ndjson_path))
}

pub fn replay_action_trace<Spec, Binding>(
    spec: &Spec,
    action_trace: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    fixture: Binding::Fixture,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: RuntimeBinding<Spec>,
{
    let mut sut = Binding::create(fixture)
        .map_err(|error| HarnessError::Binding(format!("failed to create runtime: {error}")))?;
    let mut env = TestEnvironment::default();
    let valid_initial_states = spec.initial_states();
    let initial = Binding::project(&sut);
    validate_projected_initial_state(&valid_initial_states, None, &initial, "replay")?;
    if !initial.matches_known(&action_trace.initial) {
        return Err(HarnessError::Refinement(format!(
            "replay initial projection mismatch: observed {initial:?}, expected {:?}",
            action_trace.initial
        )));
    }
    let mut before = action_trace
        .initial
        .as_ref()
        .cloned()
        .or_else(|| initial.as_ref().cloned())
        .ok_or_else(|| {
            HarnessError::Refinement("replay requires an initial projection".to_owned())
        })?;
    for (index, step) in action_trace.steps.iter().enumerate() {
        let output = Binding::apply(&mut sut, &step.action, &mut env).map_err(|error| {
            HarnessError::Binding(format!("replay action {index} failed: {error}"))
        })?;
        let projected_output = Binding::project_output(&step.action, &output);
        if projected_output != step.output {
            return Err(HarnessError::Refinement(format!(
                "replay output mismatch at step {index}: observed {projected_output:?}, expected {:?}",
                step.output
            )));
        }
        let after = Binding::project(&sut);
        if !after.matches_known(&step.after) {
            return Err(HarnessError::Refinement(format!(
                "replay state mismatch at step {index}: observed {after:?}, expected {:?}",
                step.after
            )));
        }
        before = after
            .as_ref()
            .cloned()
            .or_else(|| step.after.as_ref().cloned())
            .unwrap_or(before);
        let _ = before;
    }
    Ok(())
}

impl<S> ProjectedState<S>
where
    S: PartialEq + Clone,
{
    fn matches_known(&self, expected: &ProjectedState<S>) -> bool {
        match expected {
            ProjectedState::Exact(state) | ProjectedState::Partial(state) => self.matches(state),
            ProjectedState::Unknown => true,
        }
    }
}

fn validate_projected_initial_state<S>(
    valid_initial_states: &[S],
    expected_initial_state: Option<&S>,
    observed: &ProjectedState<S>,
    context: &str,
) -> Result<(), HarnessError>
where
    S: PartialEq,
{
    if let Some(expected) = expected_initial_state {
        return match observed.as_ref() {
            Some(state) if state == expected => Ok(()),
            Some(_) => Err(HarnessError::Refinement(format!(
                "{context} projected initial state did not match seeds.initial_state"
            ))),
            None => Err(HarnessError::Refinement(format!(
                "{context} requires a projected initial state to validate seeds.initial_state"
            ))),
        };
    }

    if let Some(state) = observed.as_ref() {
        if !valid_initial_states
            .iter()
            .any(|candidate| candidate == state)
        {
            return Err(HarnessError::Refinement(format!(
                "{context} projected initial state was not contained in spec.initial_states()"
            )));
        }
    }

    Ok(())
}

fn run_explicit_suite<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    _binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let lowered = lower_spec(spec)?;
    let snapshot = ExplicitModelChecker::for_case(&lowered, profile.model_instance.clone())
        .full_reachable_graph_snapshot()
        .map_err(|error| HarnessError::Refinement(format!("reachable graph failed: {error:?}")))?;
    let paths = canonical_paths(&snapshot);
    for (state_index, state) in snapshot.states.iter().enumerate() {
        for action in spec.actions() {
            let outcome = execute_from_path::<Spec, Binding>(
                spec,
                &profile.seeds,
                &paths[state_index],
                state,
                &action,
            )?;
            let expected_after = matching_successor(spec, state, &action, &outcome.after);
            let expected_output = spec.expected_output(state, &action, expected_after.as_ref());
            if expected_output != outcome.output {
                persist_failure::<Spec>(
                    metadata,
                    profile,
                    "explicit_suite",
                    artifact_dir,
                    &outcome.detail,
                    &outcome.action_trace,
                )?;
                return Err(HarnessError::Refinement(format!(
                    "explicit suite output mismatch for action {action:?}: observed {:?}, expected {expected_output:?}",
                    outcome.output
                )));
            }
            match expected_after {
                Some(next) => {
                    if !outcome.after.matches(&next) {
                        persist_failure::<Spec>(
                            metadata,
                            profile,
                            "explicit_suite",
                            artifact_dir,
                            &outcome.detail,
                            &outcome.action_trace,
                        )?;
                        return Err(HarnessError::Refinement(format!(
                            "explicit suite state mismatch for action {action:?}: observed {:?}, expected {next:?}",
                            outcome.after
                        )));
                    }
                }
                None => {
                    if !outcome.after.matches_known(&outcome.before) {
                        persist_failure::<Spec>(
                            metadata,
                            profile,
                            "explicit_suite",
                            artifact_dir,
                            &outcome.detail,
                            &outcome.action_trace,
                        )?;
                        return Err(HarnessError::Refinement(format!(
                            "disabled action {action:?} changed state: before {:?}, after {:?}",
                            outcome.before, outcome.after
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn run_proptest_online<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    _binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    cases: usize,
    max_steps: usize,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let action_candidates = generated_action_candidates::<Spec, Binding>(spec, &profile.seeds)?;
    if action_candidates.is_empty() {
        return Ok(());
    }
    let valid_initial_states = spec.initial_states();
    let planned_sequences = planner_guided_sequences(spec, profile, max_steps)?;
    let mut planned_use_counts = vec![0usize; planned_sequences.len()];
    let mut uncovered_boundary_actions = collect_boundary_actions(spec)?;
    let mut uncovered_transition_pairs = if profile
        .coverage
        .iter()
        .any(|goal| matches!(goal, CoverageGoal::TransitionPairs(2)))
    {
        all_action_pairs(&action_candidates)
    } else {
        Vec::new()
    };
    let mut rng = Lcg::new(profile.seeds.environment.rng_seed);
    for _case_index in 0..cases {
        let fixture = resolve_fixture::<Binding, Spec>(&profile.seeds)?;
        let mut sut = Binding::create(fixture)
            .map_err(|error| HarnessError::Binding(format!("failed to create runtime: {error}")))?;
        let mut env = profile.seeds.environment.clone();
        let initial = Binding::project(&sut);
        validate_projected_initial_state(
            &valid_initial_states,
            profile.seeds.initial_state.as_ref(),
            &initial,
            "proptest_online",
        )?;
        let mut before = initial.as_ref().cloned().ok_or_else(|| {
            HarnessError::Refinement("proptest run requires initial projection".to_owned())
        })?;
        let guided_prefix =
            choose_guided_sequence(&planned_sequences, &planned_use_counts, &mut rng)
                .map(|(index, sequence)| {
                    planned_use_counts[index] += 1;
                    sequence
                })
                .unwrap_or_default();
        let mut previous_action = None::<Spec::Action>;
        for step_index in 0..max_steps.max(1) {
            let action = if let Some(action) = guided_prefix.get(step_index) {
                action.clone()
            } else {
                choose_weighted_action(
                    &action_candidates,
                    previous_action.as_ref(),
                    &uncovered_transition_pairs,
                    &uncovered_boundary_actions,
                    &mut rng,
                )?
            };
            let output = Binding::apply(&mut sut, &action, &mut env).map_err(|error| {
                HarnessError::Binding(format!("proptest action {step_index} failed: {error}"))
            })?;
            let expected_output = Binding::project_output(&action, &output);
            let after = Binding::project(&sut);
            let matching = matching_successor(spec, &before, &action, &after);
            let expected = spec.expected_output(&before, &action, matching.as_ref());
            if expected != expected_output {
                let bundle = detailed_and_action_trace(&before, &action, &expected_output, &after);
                persist_failure::<Spec>(
                    metadata,
                    profile,
                    "proptest_online",
                    artifact_dir,
                    &bundle.0,
                    &bundle.1,
                )?;
                return Err(HarnessError::Refinement(format!(
                    "proptest output mismatch for action {action:?}: observed {expected_output:?}, expected {expected:?}"
                )));
            }
            if let Some(previous) = previous_action.take() {
                if let Some(index) = uncovered_transition_pairs
                    .iter()
                    .position(|candidate| candidate.0 == previous && candidate.1 == action)
                {
                    uncovered_transition_pairs.remove(index);
                }
                previous_action = Some(action.clone());
            } else {
                previous_action = Some(action.clone());
            }
            if let Some(index) = uncovered_boundary_actions
                .iter()
                .position(|candidate| candidate == &action)
            {
                uncovered_boundary_actions.remove(index);
            }
            before = after.as_ref().cloned().unwrap_or(before);
        }
    }
    Ok(())
}

fn generated_action_candidates<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
) -> Result<Vec<Spec::Action>, HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Debug + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let mut actions = Vec::new();
    for values in seeds.actions.values() {
        for action in values {
            push_unique(&mut actions, action.clone());
        }
    }
    for action in Binding::generated_action_candidates(spec, seeds)? {
        push_unique(&mut actions, action);
    }
    for action in auto_action_candidates(spec) {
        push_unique(&mut actions, action);
    }
    Ok(prune_action_candidates(spec, actions))
}

fn planner_guided_sequences<Spec>(
    spec: &Spec,
    profile: &TestProfile<Spec>,
    max_steps: usize,
) -> Result<Vec<Vec<Spec::Action>>, HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Send + Sync + 'static,
{
    let lowered = lower_spec(spec)?;
    let coverage = planner_coverage_goals(&profile.coverage);
    let seeds = planner_seed_profile(&profile.seeds);
    let mut sequences = Vec::new();
    let explicit = ExplicitObligationPlanner::new()
        .obligations(&lowered, &profile.model_instance, &coverage, &seeds)
        .map_err(|error| {
            HarnessError::Refinement(format!("proptest explicit planning failed: {error:?}"))
        })?;
    for obligation in explicit {
        if let PlannedObligationKind::ExplicitTraceCover { trace, .. } = obligation.kind {
            let actions = trace
                .steps()
                .iter()
                .filter_map(|step| match step {
                    TraceStep::Action(action) => Some(action.clone()),
                    TraceStep::Stutter => None,
                })
                .take(max_steps.max(1))
                .collect::<Vec<_>>();
            if !actions.is_empty() {
                push_unique(&mut sequences, actions);
            }
        }
    }
    let property = PropertyPrefixPlanner::new()
        .obligations(&lowered, &profile.model_instance, &coverage, &seeds)
        .map_err(|error| {
            HarnessError::Refinement(format!("proptest property planning failed: {error:?}"))
        })?;
    for obligation in property {
        if let PlannedObligationKind::PropertyPrefix { prefix, .. } = obligation.kind {
            let actions = prefix
                .into_iter()
                .filter_map(|step| match step {
                    TraceStep::Action(action) => Some(action),
                    TraceStep::Stutter => None,
                })
                .take(max_steps.max(1))
                .collect::<Vec<_>>();
            if !actions.is_empty() {
                push_unique(&mut sequences, actions);
            }
        }
    }
    Ok(sequences)
}

fn choose_guided_sequence<A>(
    sequences: &[Vec<A>],
    use_counts: &[usize],
    rng: &mut Lcg,
) -> Option<(usize, Vec<A>)>
where
    A: Clone,
{
    if sequences.is_empty() {
        return None;
    }
    let weights = use_counts
        .iter()
        .map(|count| if *count == 0 { 8usize } else { 1usize })
        .collect::<Vec<_>>();
    let index = choose_weighted_index(&weights, rng)?;
    Some((index, sequences[index].clone()))
}

fn choose_weighted_action<A>(
    actions: &[A],
    previous_action: Option<&A>,
    uncovered_pairs: &[(A, A)],
    uncovered_boundary_actions: &[A],
    rng: &mut Lcg,
) -> Result<A, HarnessError>
where
    A: Clone + PartialEq,
{
    let weights = actions
        .iter()
        .map(|action| {
            let mut weight = 1usize;
            if uncovered_boundary_actions
                .iter()
                .any(|candidate| candidate == action)
            {
                weight += 4;
            }
            if previous_action.is_some_and(|previous| {
                uncovered_pairs
                    .iter()
                    .any(|candidate| &candidate.0 == previous && &candidate.1 == action)
            }) {
                weight += 6;
            }
            weight
        })
        .collect::<Vec<_>>();
    choose_weighted_index(&weights, rng)
        .and_then(|index| actions.get(index))
        .cloned()
        .ok_or_else(|| {
            HarnessError::Binding("weighted action selection produced no candidate".to_owned())
        })
}

fn choose_weighted_index(weights: &[usize], rng: &mut Lcg) -> Option<usize> {
    let total = weights.iter().copied().sum::<usize>();
    if total == 0 {
        return None;
    }
    let mut target = rng.next(total);
    for (index, weight) in weights.iter().copied().enumerate() {
        if target < weight {
            return Some(index);
        }
        target -= weight;
    }
    weights.len().checked_sub(1)
}

fn all_action_pairs<A>(actions: &[A]) -> Vec<(A, A)>
where
    A: Clone + PartialEq,
{
    let mut pairs = Vec::new();
    for lhs in actions {
        for rhs in actions {
            push_unique(&mut pairs, (lhs.clone(), rhs.clone()));
        }
    }
    pairs
}

fn collect_boundary_actions<Spec>(spec: &Spec) -> Result<Vec<Spec::Action>, HarnessError>
where
    Spec: FrontendSpec + TemporalSpec,
    Spec::Action: PartialEq,
{
    let lowered = lower_spec(spec)?;
    let mut actions = Vec::new();
    for transition in lowered.generated_test_boundaries().transitions {
        push_unique(&mut actions, transition.action);
    }
    Ok(actions)
}

fn run_trace_validation_suite<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    _binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    engine: TraceValidationEngine,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec> + TraceBinding<Spec>,
{
    let lowered = lower_spec(spec)?;
    let snapshot = ExplicitModelChecker::for_case(&lowered, profile.model_instance.clone())
        .full_reachable_graph_snapshot()
        .map_err(|error| HarnessError::Refinement(format!("reachable graph failed: {error:?}")))?;
    let paths = canonical_paths(&snapshot);
    for (state_index, state) in snapshot.states.iter().enumerate() {
        if !snapshot.initial_indices.contains(&state_index) {
            continue;
        }
        for action in spec.actions() {
            if spec.transition_relation(state, &action).is_empty() {
                continue;
            }
            let outcome = execute_with_trace::<Spec, Binding>(
                spec,
                &profile.seeds,
                &paths[state_index],
                state,
                &action,
            )?;
            if let Err(error) = assert_trace_refines(
                spec,
                profile.model_instance.clone(),
                &outcome.action_trace,
                engine.clone(),
            ) {
                persist_failure::<Spec>(
                    metadata,
                    profile,
                    "trace_validation",
                    artifact_dir,
                    &outcome.detail,
                    &outcome.action_trace,
                )?;
                return Err(HarnessError::Refinement(error.to_string()));
            }
        }
    }
    Ok(())
}

fn run_kani_bounded<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    depth: usize,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    for (obligation_id, actions) in collect_kani_obligations(spec, profile, depth)? {
        let (detail, action_trace) =
            execute_action_sequence::<Spec, Binding>(spec, &profile.seeds, &actions)?;
        if let Err(error) = validate_observed_action_trace(spec, &action_trace) {
            persist_failure_with_context::<Spec>(
                metadata,
                profile,
                "kani_bounded",
                Some(binding_name),
                Some(&obligation_id),
                None,
                artifact_dir,
                &detail,
                &action_trace,
            )?;
            return Err(error);
        }
    }

    Ok(())
}

pub fn run_kani_obligation_slot<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    depth: usize,
    slot: usize,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let Some((obligation_id, actions)) = collect_kani_obligations(spec, profile, depth)?
        .into_iter()
        .nth(slot)
    else {
        return Ok(());
    };
    let (detail, action_trace) =
        execute_action_sequence::<Spec, Binding>(spec, &profile.seeds, &actions)?;
    if let Err(error) = validate_observed_action_trace(spec, &action_trace) {
        persist_failure_with_context::<Spec>(
            metadata,
            profile,
            "kani_bounded",
            Some(binding_name),
            Some(&obligation_id),
            None,
            artifact_dir,
            &detail,
            &action_trace,
        )?;
        return Err(error);
    }
    Ok(())
}

pub fn run_kani_obligation_by_id<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    depth: usize,
    obligation_id: &str,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let (_, actions) = collect_kani_obligations(spec, profile, depth)?
        .into_iter()
        .find(|candidate| candidate.0 == obligation_id)
        .ok_or_else(|| {
            HarnessError::Binding(format!(
                "kani bounded obligation `{obligation_id}` was not found"
            ))
        })?;
    let (detail, action_trace) =
        execute_action_sequence::<Spec, Binding>(spec, &profile.seeds, &actions)?;
    if let Err(error) = validate_observed_action_trace(spec, &action_trace) {
        persist_failure_with_context::<Spec>(
            metadata,
            profile,
            "kani_bounded",
            Some(binding_name),
            Some(obligation_id),
            None,
            artifact_dir,
            &detail,
            &action_trace,
        )?;
        return Err(error);
    }
    Ok(())
}

pub fn run_materialized_kani_obligation<Spec, Binding>(
    spec: &Spec,
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    obligation_id: &str,
    actions: &[Spec::Action],
) -> Result<(), HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone
        + PartialEq
        + FiniteModelDomain
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
{
    let (detail, action_trace) =
        execute_action_sequence::<Spec, Binding>(spec, &profile.seeds, actions)?;
    if let Err(error) = validate_observed_action_trace(spec, &action_trace) {
        persist_failure_with_context::<Spec>(
            metadata,
            profile,
            "kani_bounded",
            Some(binding_name),
            Some(obligation_id),
            None,
            artifact_dir,
            &detail,
            &action_trace,
        )?;
        return Err(error);
    }
    Ok(())
}

pub fn collect_kani_obligations<Spec>(
    spec: &Spec,
    profile: &TestProfile<Spec>,
    depth: usize,
) -> Result<Vec<(String, Vec<Spec::Action>)>, HarnessError>
where
    Spec: SpecOracle + TemporalSpec,
    Spec::State: Clone + PartialEq + FiniteModelDomain + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Send + Sync + 'static,
{
    let lowered = lower_spec(spec)?;
    let planner =
        ExplicitObligationPlanner::with_config(ModelCheckConfig::bounded_lasso(depth.max(1) + 1));
    let obligations = planner
        .obligations(
            &lowered,
            &profile.model_instance,
            &planner_coverage_goals(&profile.coverage),
            &planner_seed_profile(&profile.seeds),
        )
        .map_err(|error| {
            HarnessError::Refinement(format!("kani bounded planning failed: {error:?}"))
        })?;

    let mut seen = Vec::<Vec<Spec::Action>>::new();
    let mut selected = Vec::new();
    for obligation in obligations {
        let PlannedObligationKind::ExplicitTraceCover { trace, .. } = obligation.kind else {
            continue;
        };
        let actions = trace
            .steps()
            .iter()
            .filter_map(|step| match step {
                TraceStep::Action(action) => Some(action.clone()),
                TraceStep::Stutter => None,
            })
            .take(depth.max(1))
            .collect::<Vec<_>>();
        if actions.is_empty() || seen.iter().any(|candidate| candidate == &actions) {
            continue;
        }
        seen.push(actions.clone());
        selected.push((obligation.id, actions));
    }

    Ok(selected)
}

fn lower_spec<Spec>(
    spec: &Spec,
) -> Result<nirvash_lower::LoweredSpec<'_, Spec::State, Spec::Action>, HarnessError>
where
    Spec: FrontendSpec + TemporalSpec,
{
    let mut lowering_cx = LoweringCx;
    spec.lower(&mut lowering_cx)
        .map_err(|error| HarnessError::Refinement(format!("lowering failed: {error}")))
}

fn canonical_paths<S, A>(snapshot: &ReachableGraphSnapshot<S, A>) -> Vec<Vec<PrefixStep<A>>>
where
    S: Clone,
    A: Clone,
{
    let mut paths = vec![None; snapshot.states.len()];
    let mut queue = VecDeque::new();
    for &index in &snapshot.initial_indices {
        paths[index] = Some(Vec::new());
        queue.push_back(index);
    }
    while let Some(source) = queue.pop_front() {
        let prefix = paths[source]
            .clone()
            .expect("reachable state should already have a prefix path");
        for edge in &snapshot.edges[source] {
            if paths[edge.target].is_none() {
                let mut next = prefix.clone();
                next.push(PrefixStep {
                    action: edge.action.clone(),
                });
                paths[edge.target] = Some(next);
                queue.push_back(edge.target);
            }
        }
    }
    paths
        .into_iter()
        .map(|path| path.expect("reachable state should have a canonical path"))
        .collect()
}

fn execute_from_path<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    prefix: &[PrefixStep<Spec::Action>],
    _expected_state: &Spec::State,
    action: &Spec::Action,
) -> Result<ReplayOutcome<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec>,
{
    let fixture = resolve_fixture::<Binding, Spec>(seeds)?;
    let mut sut = Binding::create(fixture)
        .map_err(|error| HarnessError::Binding(format!("failed to create runtime: {error}")))?;
    let mut env = seeds.environment.clone();
    let mut before = Binding::project(&sut);
    let valid_initial_states = spec.initial_states();
    validate_projected_initial_state(
        &valid_initial_states,
        seeds.initial_state.as_ref(),
        &before,
        "explicit_suite",
    )?;
    for step in prefix {
        let _ = Binding::apply(&mut sut, &step.action, &mut env).map_err(|error| {
            HarnessError::Binding(format!("failed to replay canonical path action: {error}"))
        })?;
        before = Binding::project(&sut);
    }
    let output = Binding::apply(&mut sut, action, &mut env)
        .map_err(|error| HarnessError::Binding(format!("failed to execute action: {error}")))?;
    let projected_output = Binding::project_output(action, &output);
    let after = Binding::project(&sut);
    let (detail, action_trace) = detailed_and_action_trace(
        before
            .as_ref()
            .unwrap_or_else(|| panic!("path replay should yield an observable state")),
        action,
        &projected_output,
        &after,
    );
    Ok(ReplayOutcome {
        before,
        after,
        output: projected_output,
        detail,
        action_trace,
    })
}

fn execute_action_sequence<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    actions: &[Spec::Action],
) -> Result<
    (
        DetailedObservedTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
        ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    ),
    HarnessError,
>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec>,
{
    let fixture = resolve_fixture::<Binding, Spec>(seeds)?;
    let mut sut = Binding::create(fixture)
        .map_err(|error| HarnessError::Binding(format!("failed to create runtime: {error}")))?;
    let mut env = seeds.environment.clone();
    let initial = Binding::project(&sut);
    let valid_initial_states = spec.initial_states();
    validate_projected_initial_state(
        &valid_initial_states,
        seeds.initial_state.as_ref(),
        &initial,
        "bounded execution",
    )?;
    let _ = initial.as_ref().cloned().ok_or_else(|| {
        HarnessError::Refinement("bounded execution requires an initial projection".to_owned())
    })?;

    let mut detail = DetailedObservedTrace {
        initial: initial.clone(),
        events: Vec::new(),
    };
    let mut action_trace = ObservedActionTrace {
        initial,
        steps: Vec::new(),
    };

    for action in actions {
        detail.events.push(ObservedEvent::Invoke {
            action: action.clone(),
        });
        let output = Binding::apply(&mut sut, action, &mut env).map_err(|error| {
            HarnessError::Binding(format!(
                "failed to execute bounded action {action:?}: {error}"
            ))
        })?;
        let projected_output = Binding::project_output(action, &output);
        let after = Binding::project(&sut);
        detail.events.push(ObservedEvent::Return {
            action: action.clone(),
            output: Some(projected_output.clone()),
        });
        detail.events.push(ObservedEvent::Stutter);
        action_trace.steps.push(ObservedActionStep {
            action: action.clone(),
            output: projected_output,
            after,
        });
    }

    Ok((detail, action_trace))
}

fn selected_concurrent_actions<Spec>(spec: &Spec, threads: usize) -> Vec<Spec::Action>
where
    Spec: FrontendSpec,
    Spec::Action: Clone,
{
    let actions = spec.actions();
    let limit = threads.max(1).min(actions.len());
    actions.into_iter().take(limit).collect()
}

fn serial_concurrent_execution<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    actions: &[Spec::Action],
    schedule: Value,
) -> Result<ConcurrentExecution<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug + Serialize,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec>,
{
    let (detail, action_trace) = execute_action_sequence::<Spec, Binding>(spec, seeds, actions)?;
    Ok(ConcurrentExecution {
        detail,
        action_trace,
        schedule,
    })
}

#[cfg(feature = "loom")]
fn run_loom_small<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    threads: usize,
    max_permutations: usize,
) -> Result<ConcurrentExecution<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
    Binding::Sut: Send + 'static,
{
    let actions = selected_concurrent_actions(spec, threads);
    if actions.len() <= 1 {
        return serial_concurrent_execution::<Spec, Binding>(
            spec,
            seeds,
            &actions,
            json!({
                "engine": "loom_small",
                "status": "serial_fallback",
                "threads": threads,
                "max_permutations": max_permutations,
                "schedule_seed": seeds.environment.schedule_seed,
                "executed_actions": actions,
            }),
        );
    }

    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let errors = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let mut builder = loom::model::Builder::new();
    builder.max_threads = threads.max(actions.len()).max(2);
    builder.max_permutations = Some(max_permutations.max(1));

    let captured_run = captured.clone();
    let errors_run = errors.clone();
    let actions_run = actions.clone();
    let fixture_seed = seeds.fixture.clone();
    let env_run = seeds.environment.clone();
    let valid_initial_states = spec.initial_states();
    let expected_initial_state = seeds.initial_state.clone();
    builder.check(move || {
        let fixture = match resolve_fixture_seed::<Binding, Spec>(&fixture_seed) {
            Ok(fixture) => fixture,
            Err(error) => {
                let mut slot = errors_run.lock().expect("loom error slot");
                if slot.is_none() {
                    *slot = Some(format!("failed to resolve concurrent fixture: {error}"));
                }
                return;
            }
        };
        let initial_sut = match Binding::create(fixture) {
            Ok(sut) => sut,
            Err(error) => {
                let mut slot = errors_run.lock().expect("loom error slot");
                if slot.is_none() {
                    *slot = Some(format!("failed to create concurrent runtime: {error}"));
                }
                return;
            }
        };
        let initial = Binding::project(&initial_sut);
        if let Err(error) = validate_projected_initial_state(
            &valid_initial_states,
            expected_initial_state.as_ref(),
            &initial,
            "loom_small",
        ) {
            let mut slot = errors_run.lock().expect("loom error slot");
            if slot.is_none() {
                *slot = Some(error.to_string());
            }
            return;
        }
        let shared = loom::sync::Arc::new(loom::sync::Mutex::new((
            initial_sut,
            env_run.clone(),
            Vec::<ObservedEvent<Spec::Action, Spec::ExpectedOutput>>::new(),
            Vec::<ObservedActionStep<Spec::State, Spec::Action, Spec::ExpectedOutput>>::new(),
        )));
        let schedule = loom::sync::Arc::new(loom::sync::Mutex::new(Vec::<Spec::Action>::new()));
        let thread_errors = loom::sync::Arc::new(loom::sync::Mutex::new(None::<String>));

        let mut handles = Vec::new();
        for action in actions_run.clone() {
            let shared = shared.clone();
            let schedule = schedule.clone();
            let thread_errors = thread_errors.clone();
            handles.push(loom::thread::spawn(move || {
                let mut guard = shared.lock().expect("loom shared state");
                let (sut, env, events, steps) = &mut *guard;
                events.push(ObservedEvent::Invoke {
                    action: action.clone(),
                });
                let output = match Binding::apply(sut, &action, env) {
                    Ok(output) => output,
                    Err(error) => {
                        let mut slot = thread_errors.lock().expect("loom apply error");
                        if slot.is_none() {
                            *slot = Some(format!(
                                "failed to apply concurrent action {action:?}: {error}"
                            ));
                        }
                        return;
                    }
                };
                let projected_output = Binding::project_output(&action, &output);
                let after = Binding::project(sut);
                events.push(ObservedEvent::Return {
                    action: action.clone(),
                    output: Some(projected_output.clone()),
                });
                events.push(ObservedEvent::Stutter);
                steps.push(ObservedActionStep {
                    action: action.clone(),
                    output: projected_output,
                    after,
                });
                schedule.lock().expect("loom schedule").push(action);
            }));
        }

        for handle in handles {
            handle.join().expect("loom thread should join");
        }

        if let Some(error) = thread_errors.lock().expect("loom thread errors").clone() {
            let mut slot = errors_run.lock().expect("loom error slot");
            if slot.is_none() {
                *slot = Some(error);
            }
            return;
        }

        let guard = shared.lock().expect("loom shared state");
        let execution = ConcurrentExecution {
            detail: DetailedObservedTrace {
                initial: initial.clone(),
                events: guard.2.clone(),
            },
            action_trace: ObservedActionTrace {
                initial,
                steps: guard.3.clone(),
            },
            schedule: json!({
                "engine": "loom_small",
                "threads": threads,
                "max_permutations": max_permutations,
                "schedule_seed": env_run.schedule_seed,
                "executed_actions": schedule.lock().expect("loom schedule").clone(),
            }),
        };
        *captured_run.lock().expect("loom execution") = Some(execution);
    });

    if let Some(error) = errors.lock().expect("loom error slot").clone() {
        return Err(HarnessError::Binding(error));
    }

    captured
        .lock()
        .expect("loom execution")
        .clone()
        .ok_or_else(|| HarnessError::Binding("loom run did not capture an execution".to_owned()))
}

#[cfg(not(feature = "loom"))]
fn run_loom_small<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    threads: usize,
    max_permutations: usize,
) -> Result<ConcurrentExecution<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug + Serialize,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec>,
{
    let actions = selected_concurrent_actions(spec, threads);
    serial_concurrent_execution::<Spec, Binding>(
        spec,
        seeds,
        &actions,
        json!({
            "engine": "loom_small",
            "status": "feature_disabled",
            "threads": threads,
            "max_permutations": max_permutations,
            "schedule_seed": seeds.environment.schedule_seed,
            "executed_actions": actions,
        }),
    )
}

#[cfg(feature = "shuttle")]
fn run_shuttle_pct<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    depth: usize,
    runs: usize,
) -> Result<ConcurrentExecution<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::Action: Clone + PartialEq + Debug + Serialize + DeserializeOwned + Send + Sync + 'static,
    Spec::ExpectedOutput:
        Clone + Debug + PartialEq + Eq + Serialize + DeserializeOwned + Send + Sync + 'static,
    Binding: GeneratedBinding<Spec>,
    Binding::Sut: Send + 'static,
{
    let actions = selected_concurrent_actions(spec, depth.max(2));
    if actions.len() <= 1 {
        return serial_concurrent_execution::<Spec, Binding>(
            spec,
            seeds,
            &actions,
            json!({
                "engine": "shuttle_pct",
                "status": "serial_fallback",
                "depth": depth,
                "runs": runs,
                "schedule_seed": seeds.environment.schedule_seed,
                "executed_actions": actions,
            }),
        );
    }

    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let errors = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let captured_run = captured.clone();
    let errors_run = errors.clone();
    let actions_run = actions.clone();
    let fixture_seed = seeds.fixture.clone();
    let env_run = seeds.environment.clone();
    let valid_initial_states = spec.initial_states();
    let expected_initial_state = seeds.initial_state.clone();
    shuttle::check_pct(
        move || {
            let fixture = match resolve_fixture_seed::<Binding, Spec>(&fixture_seed) {
                Ok(fixture) => fixture,
                Err(error) => {
                    let mut slot = errors_run.lock().expect("shuttle error slot");
                    if slot.is_none() {
                        *slot = Some(format!("failed to resolve concurrent fixture: {error}"));
                    }
                    return;
                }
            };
            let initial_sut = match Binding::create(fixture) {
                Ok(sut) => sut,
                Err(error) => {
                    let mut slot = errors_run.lock().expect("shuttle error slot");
                    if slot.is_none() {
                        *slot = Some(format!("failed to create concurrent runtime: {error}"));
                    }
                    return;
                }
            };
            let initial = Binding::project(&initial_sut);
            if let Err(error) = validate_projected_initial_state(
                &valid_initial_states,
                expected_initial_state.as_ref(),
                &initial,
                "shuttle_pct",
            ) {
                let mut slot = errors_run.lock().expect("shuttle error slot");
                if slot.is_none() {
                    *slot = Some(error.to_string());
                }
                return;
            }
            let shared = shuttle::sync::Arc::new(shuttle::sync::Mutex::new((
                initial_sut,
                env_run.clone(),
                Vec::<ObservedEvent<Spec::Action, Spec::ExpectedOutput>>::new(),
                Vec::<ObservedActionStep<Spec::State, Spec::Action, Spec::ExpectedOutput>>::new(),
            )));
            let schedule =
                shuttle::sync::Arc::new(shuttle::sync::Mutex::new(Vec::<Spec::Action>::new()));
            let thread_errors = shuttle::sync::Arc::new(shuttle::sync::Mutex::new(None::<String>));

            let mut handles = Vec::new();
            for action in actions_run.clone() {
                let shared = shared.clone();
                let schedule = schedule.clone();
                let thread_errors = thread_errors.clone();
                handles.push(shuttle::thread::spawn(move || {
                    let mut guard = shared.lock().expect("shuttle shared state");
                    let (sut, env, events, steps) = &mut *guard;
                    events.push(ObservedEvent::Invoke {
                        action: action.clone(),
                    });
                    let output = match Binding::apply(sut, &action, env) {
                        Ok(output) => output,
                        Err(error) => {
                            let mut slot = thread_errors.lock().expect("shuttle apply error");
                            if slot.is_none() {
                                *slot = Some(format!(
                                    "failed to apply concurrent action {action:?}: {error}"
                                ));
                            }
                            return;
                        }
                    };
                    let projected_output = Binding::project_output(&action, &output);
                    let after = Binding::project(sut);
                    events.push(ObservedEvent::Return {
                        action: action.clone(),
                        output: Some(projected_output.clone()),
                    });
                    events.push(ObservedEvent::Stutter);
                    steps.push(ObservedActionStep {
                        action: action.clone(),
                        output: projected_output,
                        after,
                    });
                    schedule.lock().expect("shuttle schedule").push(action);
                }));
            }

            for handle in handles {
                handle.join().expect("shuttle thread should join");
            }

            if let Some(error) = thread_errors.lock().expect("shuttle thread errors").clone() {
                let mut slot = errors_run.lock().expect("shuttle error slot");
                if slot.is_none() {
                    *slot = Some(error);
                }
                return;
            }

            let guard = shared.lock().expect("shuttle shared state");
            let execution = ConcurrentExecution {
                detail: DetailedObservedTrace {
                    initial: initial.clone(),
                    events: guard.2.clone(),
                },
                action_trace: ObservedActionTrace {
                    initial,
                    steps: guard.3.clone(),
                },
                schedule: json!({
                    "engine": "shuttle_pct",
                    "depth": depth,
                    "runs": runs,
                    "schedule_seed": env_run.schedule_seed,
                    "executed_actions": schedule.lock().expect("shuttle schedule").clone(),
                }),
            };
            *captured_run.lock().expect("shuttle execution") = Some(execution);
        },
        runs.max(1),
        depth.max(1),
    );

    if let Some(error) = errors.lock().expect("shuttle error slot").clone() {
        return Err(HarnessError::Binding(error));
    }

    captured
        .lock()
        .expect("shuttle execution")
        .clone()
        .ok_or_else(|| HarnessError::Binding("shuttle run did not capture an execution".to_owned()))
}

#[cfg(not(feature = "shuttle"))]
fn run_shuttle_pct<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    depth: usize,
    runs: usize,
) -> Result<ConcurrentExecution<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug + Serialize,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec>,
{
    let actions = selected_concurrent_actions(spec, depth.max(2));
    serial_concurrent_execution::<Spec, Binding>(
        spec,
        seeds,
        &actions,
        json!({
            "engine": "shuttle_pct",
            "status": "feature_disabled",
            "depth": depth,
            "runs": runs,
            "schedule_seed": seeds.environment.schedule_seed,
            "executed_actions": actions,
        }),
    )
}

fn execute_with_trace<Spec, Binding>(
    spec: &Spec,
    seeds: &SeedProfile<Spec>,
    prefix: &[PrefixStep<Spec::Action>],
    _expected_state: &Spec::State,
    action: &Spec::Action,
) -> Result<ReplayOutcome<Spec::State, Spec::Action, Spec::ExpectedOutput>, HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
    Binding: GeneratedBinding<Spec> + TraceBinding<Spec>,
{
    let fixture = resolve_fixture::<Binding, Spec>(seeds)?;
    let mut sut = Binding::create(fixture)
        .map_err(|error| HarnessError::Binding(format!("failed to create runtime: {error}")))?;
    let initial = Binding::project(&sut);
    let valid_initial_states = spec.initial_states();
    validate_projected_initial_state(
        &valid_initial_states,
        seeds.initial_state.as_ref(),
        &initial,
        "trace_validation",
    )?;
    let mut env = seeds.environment.clone();
    for step in prefix {
        let _ = Binding::apply(&mut sut, &step.action, &mut env).map_err(|error| {
            HarnessError::Binding(format!("failed to replay canonical path action: {error}"))
        })?;
    }
    let before = Binding::project(&sut);
    let before_state = before.as_ref().cloned().ok_or_else(|| {
        HarnessError::Refinement("trace validation requires projected pre-state".to_owned())
    })?;
    let mut detail = DetailRecorder::default();
    detail.events.push(ObservedEvent::Invoke {
        action: action.clone(),
    });
    let output = Binding::apply(&mut sut, action, &mut env)
        .map_err(|error| HarnessError::Binding(format!("failed to execute action: {error}")))?;
    let projected_output = Binding::project_output(action, &output);
    Binding::record_update(&sut, &output, &mut detail);
    let after = Binding::project(&sut);
    detail.events.push(ObservedEvent::Return {
        action: action.clone(),
        output: Some(projected_output.clone()),
    });
    detail.events.push(ObservedEvent::Stutter);
    let action_trace = ObservedActionTrace {
        initial: before.clone(),
        steps: vec![ObservedActionStep {
            action: action.clone(),
            output: projected_output.clone(),
            after: after.clone(),
        }],
    };
    Ok(ReplayOutcome {
        before: ProjectedState::Exact(before_state),
        after,
        output: projected_output,
        detail: DetailedObservedTrace {
            initial: before,
            events: detail.events,
        },
        action_trace,
    })
}

fn matching_successor<Spec>(
    spec: &Spec,
    before: &Spec::State,
    action: &Spec::Action,
    observed_after: &ProjectedState<Spec::State>,
) -> Option<Spec::State>
where
    Spec: FrontendSpec,
    Spec::State: Clone + PartialEq,
{
    spec.transition_relation(before, action)
        .into_iter()
        .find(|candidate| observed_after.matches(candidate))
}

fn validate_against_candidates<Spec>(
    spec: &Spec,
    model_case_label: &str,
    observed: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    candidates: &[Trace<Spec::State, Spec::Action>],
) -> Result<
    TraceRefinementWitness<Spec::State, Spec::Action>,
    TraceRefinementError<Spec::State, Spec::Action>,
>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq,
{
    let Some(initial_state) = observed.initial.as_ref() else {
        return Err(TraceRefinementError::ShapeMismatch {
            model_case_label: model_case_label.to_owned(),
            detail: "observed trace requires an initial projected state".to_owned(),
        });
    };
    for candidate in candidates {
        if candidate.states().is_empty() || candidate.states()[0] != *initial_state {
            continue;
        }
        if candidate.steps().len() < observed.steps.len() {
            continue;
        }
        let mut witness_steps = Vec::new();
        let mut ok = true;
        for (index, observed_step) in observed.steps.iter().enumerate() {
            let Some(TraceStep::Action(action)) = candidate.steps().get(index) else {
                ok = false;
                break;
            };
            if action != &observed_step.action {
                ok = false;
                break;
            }
            let before = &candidate.states()[index];
            let after = &candidate.states()[index + 1];
            if !observed_step.after.matches(after) {
                ok = false;
                break;
            }
            let expected_output = spec.expected_output(before, action, Some(after));
            if expected_output != observed_step.output {
                ok = false;
                break;
            }
            witness_steps.push(TraceStepRefinementWitness {
                index,
                abstract_before: before.clone(),
                step: TraceStep::Action(action.clone()),
                abstract_after: after.clone(),
            });
        }
        if ok {
            return Ok(TraceRefinementWitness {
                abstract_trace: candidate.clone(),
                steps: witness_steps,
                model_case_label: model_case_label.to_owned(),
            });
        }
    }
    Err(TraceRefinementError::NoMatchingCandidate {
        model_case_label: model_case_label.to_owned(),
    })
}

fn resolve_fixture<Binding, Spec>(
    seeds: &SeedProfile<Spec>,
) -> Result<Binding::Fixture, HarnessError>
where
    Spec: SpecOracle,
    Binding: GeneratedBinding<Spec>,
{
    resolve_fixture_seed::<Binding, Spec>(&seeds.fixture)
}

fn resolve_fixture_seed<Binding, Spec>(
    fixture_seed: &FixtureSeed,
) -> Result<Binding::Fixture, HarnessError>
where
    Spec: SpecOracle,
    Binding: GeneratedBinding<Spec>,
{
    match fixture_seed {
        FixtureSeed::Value(factory) => factory()
            .downcast::<Binding::Fixture>()
            .map_err(|_| HarnessError::Binding("fixture override had the wrong type".to_owned()))
            .and_then(|value| {
                Arc::into_inner(value).ok_or_else(|| {
                    HarnessError::Binding(
                        "fixture override returned a shared Arc; expected a fresh fixture value"
                            .to_owned(),
                    )
                })
            }),
        FixtureSeed::Factory(factory) => factory()
            .downcast::<Binding::Fixture>()
            .map_err(|_| {
                HarnessError::Binding("fixture factory returned the wrong type".to_owned())
            })
            .and_then(|value| {
                Arc::into_inner(value).ok_or_else(|| {
                    HarnessError::Binding(
                        "fixture factory returned a shared Arc; expected a fresh fixture value"
                            .to_owned(),
                    )
                })
            }),
        FixtureSeed::Snapshot(value) => Binding::generated_snapshot_fixture(value),
        FixtureSeed::Default => Err(HarnessError::Binding(
            "profile did not provide a fixture factory; installer macro must inject one".to_owned(),
        )),
    }
}

fn detailed_and_action_trace<S, A, Output>(
    before: &S,
    action: &A,
    output: &Output,
    after: &ProjectedState<S>,
) -> (
    DetailedObservedTrace<S, A, Output>,
    ObservedActionTrace<S, A, Output>,
)
where
    S: Clone,
    A: Clone,
    Output: Clone,
{
    (
        DetailedObservedTrace {
            initial: ProjectedState::Exact(before.clone()),
            events: vec![
                ObservedEvent::Invoke {
                    action: action.clone(),
                },
                ObservedEvent::Return {
                    action: action.clone(),
                    output: Some(output.clone()),
                },
                ObservedEvent::Stutter,
            ],
        },
        ObservedActionTrace {
            initial: ProjectedState::Exact(before.clone()),
            steps: vec![ObservedActionStep {
                action: action.clone(),
                output: output.clone(),
                after: after.clone(),
            }],
        },
    )
}

fn validate_observed_action_trace<Spec>(
    spec: &Spec,
    action_trace: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Clone + PartialEq,
    Spec::Action: Clone + PartialEq + Debug,
    Spec::ExpectedOutput: Clone + Debug + PartialEq + Eq,
{
    let mut before = action_trace.initial.as_ref().cloned().ok_or_else(|| {
        HarnessError::Refinement("observed trace requires an initial state".to_owned())
    })?;

    for step in &action_trace.steps {
        let matching = matching_successor(spec, &before, &step.action, &step.after);
        let expected_output = spec.expected_output(&before, &step.action, matching.as_ref());
        if expected_output != step.output {
            return Err(HarnessError::Refinement(format!(
                "observed output mismatch for action {:?}: observed {:?}, expected {:?}",
                step.action, step.output, expected_output
            )));
        }
        if let Some(next) = matching {
            if !step.after.matches(&next) {
                return Err(HarnessError::Refinement(format!(
                    "observed state mismatch for action {:?}: observed {:?}, expected {:?}",
                    step.action, step.after, next
                )));
            }
            before = next;
        } else if !step
            .after
            .matches_known(&ProjectedState::Exact(before.clone()))
        {
            return Err(HarnessError::Refinement(format!(
                "disabled action {:?} changed state: before {:?}, after {:?}",
                step.action, before, step.after
            )));
        }
    }

    Ok(())
}

fn planner_coverage_goals(goals: &[CoverageGoal]) -> Vec<PlanningCoverageGoal> {
    goals
        .iter()
        .map(|goal| match goal {
            CoverageGoal::Transitions => PlanningCoverageGoal::Transitions,
            CoverageGoal::TransitionPairs(width) => PlanningCoverageGoal::TransitionPairs(*width),
            CoverageGoal::GuardBoundaries => PlanningCoverageGoal::GuardBoundaries,
            CoverageGoal::PropertyPrefixes => PlanningCoverageGoal::PropertyPrefixes,
            CoverageGoal::Goal(_) => PlanningCoverageGoal::Transitions,
        })
        .collect()
}

fn planner_seed_profile<Spec>(
    seeds: &SeedProfile<Spec>,
) -> PlannerSeedProfile<Spec::State, Spec::Action>
where
    Spec: FrontendSpec,
    Spec::State: Clone,
    Spec::Action: Clone,
{
    let mut labels = vec!["all".to_owned()];
    if seeds.initial_state.is_some() {
        labels.push("initial_state".to_owned());
    }
    labels.extend(seeds.typed.keys().cloned());
    labels.extend(seeds.actions.keys().cloned());
    labels.sort();
    labels.dedup();

    PlannerSeedProfile {
        labels,
        state_hints: seeds.initial_state.iter().cloned().collect(),
        action_hints: seeds
            .actions
            .values()
            .flat_map(|values| values.iter().cloned())
            .collect(),
    }
}

fn seed_metadata<Spec>(seeds: &SeedProfile<Spec>) -> Value
where
    Spec: FrontendSpec,
    Spec::State: Serialize,
{
    json!({
        "label": seeds.label,
        "initial_state": seeds.initial_state,
        "typed": seeds.typed.keys().collect::<Vec<_>>(),
        "actions": seeds.actions.keys().collect::<Vec<_>>(),
        "environment": seeds.environment,
        "shrink": format!("{:?}", seeds.shrink),
    })
}

fn persist_failure_with_context<Spec>(
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    engine: &str,
    binding_name: Option<&str>,
    obligation_id: Option<&str>,
    schedule: Option<Value>,
    artifact_dir: &ArtifactDirPolicy,
    detail: &DetailedObservedTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    action_trace: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Serialize,
    Spec::Action: Serialize,
    Spec::ExpectedOutput: Serialize,
{
    let bundle = GeneratedReplayBundle {
        spec_name: metadata.spec_name.to_owned(),
        profile: profile.label.to_owned(),
        engine: engine.to_owned(),
        binding: binding_name.map(ToOwned::to_owned),
        obligation_id: obligation_id.map(ToOwned::to_owned),
        schedule,
        seed: Some(seed_metadata(&profile.seeds)),
        detail: detail.clone(),
        action_trace: action_trace.clone(),
    };
    if engine.starts_with("kani_") || engine == "kani_bounded" {
        let normalized = NormalizedReplayBundle::from_kani_concrete(
            metadata.spec_name,
            profile.label,
            engine,
            KaniConcretePlayback {
                detail: serde_json::to_value(detail).map_err(|error| {
                    HarnessError::Artifact(format!("failed to encode kani replay detail: {error}"))
                })?,
                action_trace: serde_json::to_value(action_trace).map_err(|error| {
                    HarnessError::Artifact(format!(
                        "failed to encode kani replay action trace: {error}"
                    ))
                })?,
            },
        );
        let _ = write_serialized_replay_bundle(
            artifact_dir,
            metadata.spec_slug,
            profile.label,
            engine,
            &normalized,
            detail,
        )?;
    } else {
        let _ = write_replay_bundle(
            artifact_dir,
            metadata.spec_slug,
            profile.label,
            engine,
            &bundle,
        )?;
    }
    Ok(())
}

fn persist_failure<Spec>(
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    engine: &str,
    artifact_dir: &ArtifactDirPolicy,
    detail: &DetailedObservedTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
    action_trace: &ObservedActionTrace<Spec::State, Spec::Action, Spec::ExpectedOutput>,
) -> Result<(), HarnessError>
where
    Spec: SpecOracle,
    Spec::State: Serialize,
    Spec::Action: Serialize,
    Spec::ExpectedOutput: Serialize,
{
    persist_failure_with_context(
        metadata,
        profile,
        engine,
        None,
        None,
        None,
        artifact_dir,
        detail,
        action_trace,
    )
}

fn write_manifest<Spec>(
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    artifact_dir: &ArtifactDirPolicy,
    materialize_failures: bool,
) -> Result<(), HarnessError>
where
    Spec: FrontendSpec,
{
    let manifest_dir = artifact_dir.base.join("manifest");
    fs::create_dir_all(&manifest_dir).map_err(|error| {
        HarnessError::Artifact(format!("failed to create manifest directory: {error}"))
    })?;
    let manifest_path = manifest_dir.join(format!(
        "{}__{}__{}.json",
        sanitize_path(metadata.spec_slug),
        sanitize_path(binding_name),
        sanitize_path(profile.label),
    ));
    let manifest = json!({
        "spec": metadata.spec_name,
        "spec_slug": metadata.spec_slug,
        "spec_path": std::any::type_name::<Spec>(),
        "export_module": metadata.export_module,
        "crate_package": metadata.crate_package,
        "crate_manifest_dir": metadata.crate_manifest_dir,
        "default_profiles": metadata.default_profiles,
        "binding": binding_name,
        "binding_path": binding_name,
        "profile": profile.label,
        "engine": profile.engines,
        "coverage": profile.coverage,
        "replay_dir": artifact_dir.base.join("replay"),
        "materialize_failures": materialize_failures,
    });
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(|error| {
            HarnessError::Artifact(format!("failed to serialize manifest: {error}"))
        })?,
    )
    .map_err(|error| HarnessError::Artifact(format!("failed to write manifest: {error}")))?;
    Ok(())
}

fn write_schedule_artifact<Spec>(
    metadata: &GeneratedSpecMetadata,
    profile: &TestProfile<Spec>,
    binding_name: &str,
    engine: &EnginePlan,
    artifact_dir: &ArtifactDirPolicy,
    schedule: Option<&Value>,
) -> Result<(), HarnessError>
where
    Spec: FrontendSpec,
{
    let schedule_dir = artifact_dir.base.join("replay");
    fs::create_dir_all(&schedule_dir).map_err(|error| {
        HarnessError::Artifact(format!("failed to create replay directory: {error}"))
    })?;
    let path = replay_bundle_path(
        artifact_dir,
        metadata.spec_slug,
        profile.label,
        "schedule",
        "json",
    );
    fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "spec_name": metadata.spec_name,
            "profile": profile.label,
            "binding": binding_name,
            "engine": engine,
            "schedule_seed": profile.seeds.environment.schedule_seed,
            "schedule": schedule,
        }))
        .map_err(|error| {
            HarnessError::Artifact(format!("failed to serialize schedule artifact: {error}"))
        })?,
    )
    .map_err(|error| {
        HarnessError::Artifact(format!("failed to write schedule artifact: {error}"))
    })?;
    Ok(())
}

fn sanitize_path(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch,
            _ => '_',
        })
        .collect()
}

fn target_nirvash_dir() -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    target_dir.join("nirvash")
}

#[derive(Debug, Clone)]
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self, bound: usize) -> usize {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((self.state >> 32) as usize) % bound.max(1)
    }
}

pub fn read_replay_bundle<S, A, Output>(
    path: impl AsRef<Path>,
) -> Result<GeneratedReplayBundle<S, A, Output>, HarnessError>
where
    S: DeserializeOwned,
    A: DeserializeOwned,
    Output: DeserializeOwned,
{
    serde_json::from_slice(&fs::read(path.as_ref()).map_err(|error| {
        HarnessError::Artifact(format!("failed to read replay bundle: {error}"))
    })?)
    .map_err(|error| HarnessError::Artifact(format!("failed to decode replay bundle: {error}")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use nirvash::BoundedDomain;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    enum DemoState {
        Idle,
        Busy,
    }

    impl FiniteModelDomain for DemoState {
        fn finite_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    enum DemoAction {
        Start,
        Stop,
    }

    impl FiniteModelDomain for DemoAction {
        fn finite_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start, Self::Stop])
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    enum DemoOutput {
        Ack,
        Rejected,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    enum FiniteSeedKey {
        Alpha,
        Beta,
    }

    nirvash::inventory::submit! {
        nirvash::RegisteredFiniteDomainSeed {
            value_type_id: std::any::TypeId::of::<FiniteSeedKey>,
            values: || {
                vec![
                    Box::new(FiniteSeedKey::Alpha) as Box<dyn std::any::Any>,
                    Box::new(FiniteSeedKey::Beta) as Box<dyn std::any::Any>,
                ]
            },
        }
    }

    #[derive(Default)]
    struct DemoSpec;

    impl FrontendSpec for DemoSpec {
        type State = DemoState;
        type Action = DemoAction;

        fn frontend_name(&self) -> &'static str {
            "DemoSpec"
        }

        fn initial_states(&self) -> Vec<Self::State> {
            vec![DemoState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![DemoAction::Start, DemoAction::Stop]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (DemoState::Idle, DemoAction::Start) => Some(DemoState::Busy),
                (DemoState::Busy, DemoAction::Stop) => Some(DemoState::Idle),
                _ => None,
            }
        }
    }

    impl TemporalSpec for DemoSpec {
        fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl SpecOracle for DemoSpec {
        type ExpectedOutput = DemoOutput;

        fn expected_output(
            &self,
            prev: &Self::State,
            action: &Self::Action,
            next: Option<&Self::State>,
        ) -> Self::ExpectedOutput {
            match (prev, action, next) {
                (DemoState::Idle, DemoAction::Start, Some(DemoState::Busy))
                | (DemoState::Busy, DemoAction::Stop, Some(DemoState::Idle)) => DemoOutput::Ack,
                _ => DemoOutput::Rejected,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    struct DemoBinding {
        state: DemoState,
    }

    impl Default for DemoBinding {
        fn default() -> Self {
            Self {
                state: DemoState::Idle,
            }
        }
    }

    #[derive(Debug)]
    struct DemoError;

    impl Display for DemoError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("demo error")
        }
    }

    impl std::error::Error for DemoError {}

    impl RuntimeBinding<DemoSpec> for DemoBinding {
        type Sut = Self;
        type Fixture = Self;
        type Output = DemoOutput;
        type Error = DemoError;

        fn create(fixture: Self::Fixture) -> Result<Self::Sut, Self::Error> {
            Ok(fixture)
        }

        fn apply(
            sut: &mut Self::Sut,
            action: &DemoAction,
            _env: &mut TestEnvironment,
        ) -> Result<Self::Output, Self::Error> {
            Ok(match (sut.state, action) {
                (DemoState::Idle, DemoAction::Start) => {
                    sut.state = DemoState::Busy;
                    DemoOutput::Ack
                }
                (DemoState::Busy, DemoAction::Stop) => {
                    sut.state = DemoState::Idle;
                    DemoOutput::Ack
                }
                _ => DemoOutput::Rejected,
            })
        }

        fn project(sut: &Self::Sut) -> ProjectedState<DemoState> {
            ProjectedState::Exact(sut.state)
        }

        fn project_output(_action: &DemoAction, output: &Self::Output) -> DemoOutput {
            *output
        }
    }

    impl TraceBinding<DemoSpec> for DemoBinding {
        fn record_update(
            _sut: &Self::Sut,
            output: &Self::Output,
            sink: &mut dyn TraceSink<DemoSpec>,
        ) {
            sink.record_update(
                "last_output",
                serde_json::to_value(output).expect("serialize"),
            );
        }
    }

    impl ConcurrentBinding<DemoSpec> for DemoBinding {}

    impl GeneratedBinding<DemoSpec> for DemoBinding {
        fn generated_fixture() -> SharedFixtureValue {
            Arc::new(DemoBinding::default())
        }

        fn generated_action_candidates(
            _spec: &DemoSpec,
            seeds: &SeedProfile<DemoSpec>,
        ) -> Result<Vec<DemoAction>, HarnessError> {
            let mut actions = Vec::new();
            for values in seeds.actions.values() {
                for action in values {
                    push_unique(&mut actions, *action);
                }
            }
            if actions.is_empty() {
                actions.extend([DemoAction::Start, DemoAction::Stop]);
            }
            Ok(actions)
        }

        fn run_generated_profile(
            spec: &DemoSpec,
            metadata: &GeneratedSpecMetadata,
            profile: &TestProfile<DemoSpec>,
            binding_name: &str,
            artifact_dir: &ArtifactDirPolicy,
            materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            run_profile::<DemoSpec, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }

        fn run_generated_trace_profile(
            spec: &DemoSpec,
            metadata: &GeneratedSpecMetadata,
            profile: &TestProfile<DemoSpec>,
            binding_name: &str,
            artifact_dir: &ArtifactDirPolicy,
            materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            run_trace_profile::<DemoSpec, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }

        fn run_generated_concurrent_profile(
            spec: &DemoSpec,
            metadata: &GeneratedSpecMetadata,
            profile: &TestProfile<DemoSpec>,
            binding_name: &str,
            artifact_dir: &ArtifactDirPolicy,
            materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            run_concurrent_profile::<DemoSpec, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }

        fn supports_trace() -> bool {
            true
        }

        fn supports_concurrency() -> bool {
            true
        }
    }

    #[derive(Clone, Debug)]
    struct SnapshotBinding {
        state: DemoState,
    }

    impl RuntimeBinding<DemoSpec> for SnapshotBinding {
        type Sut = Self;
        type Fixture = u8;
        type Output = DemoOutput;
        type Error = DemoError;

        fn create(fixture: Self::Fixture) -> Result<Self::Sut, Self::Error> {
            let state = if fixture == 0 {
                DemoState::Idle
            } else {
                DemoState::Busy
            };
            Ok(Self { state })
        }

        fn apply(
            sut: &mut Self::Sut,
            action: &DemoAction,
            env: &mut TestEnvironment,
        ) -> Result<Self::Output, Self::Error> {
            DemoBinding::apply(&mut DemoBinding { state: sut.state }, action, env).map(|output| {
                sut.state = match (sut.state, action) {
                    (DemoState::Idle, DemoAction::Start) => DemoState::Busy,
                    (DemoState::Busy, DemoAction::Stop) => DemoState::Idle,
                    _ => sut.state,
                };
                output
            })
        }

        fn project(sut: &Self::Sut) -> ProjectedState<DemoState> {
            ProjectedState::Exact(sut.state)
        }

        fn project_output(_action: &DemoAction, output: &Self::Output) -> DemoOutput {
            *output
        }
    }

    impl GeneratedBinding<DemoSpec> for SnapshotBinding {
        fn generated_fixture() -> SharedFixtureValue {
            Arc::new(0_u8)
        }

        fn generated_snapshot_fixture(value: &Value) -> Result<Self::Fixture, HarnessError> {
            <u8 as SnapshotFixture>::from_snapshot(value)
        }

        fn generated_action_candidates(
            spec: &DemoSpec,
            _seeds: &SeedProfile<DemoSpec>,
        ) -> Result<Vec<DemoAction>, HarnessError> {
            Ok(spec.actions())
        }

        fn run_generated_profile(
            spec: &DemoSpec,
            metadata: &GeneratedSpecMetadata,
            profile: &TestProfile<DemoSpec>,
            binding_name: &str,
            artifact_dir: &ArtifactDirPolicy,
            materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            run_profile::<DemoSpec, Self>(
                spec,
                metadata,
                profile,
                binding_name,
                artifact_dir,
                materialize_failures,
            )
        }

        fn run_generated_trace_profile(
            _spec: &DemoSpec,
            _metadata: &GeneratedSpecMetadata,
            _profile: &TestProfile<DemoSpec>,
            _binding_name: &str,
            _artifact_dir: &ArtifactDirPolicy,
            _materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            Err(HarnessError::Binding(
                "trace support is not enabled".to_owned(),
            ))
        }

        fn run_generated_concurrent_profile(
            _spec: &DemoSpec,
            _metadata: &GeneratedSpecMetadata,
            _profile: &TestProfile<DemoSpec>,
            _binding_name: &str,
            _artifact_dir: &ArtifactDirPolicy,
            _materialize_failures: bool,
        ) -> Result<(), HarnessError> {
            Err(HarnessError::Binding(
                "concurrency support is not enabled".to_owned(),
            ))
        }
    }

    fn demo_factory() -> SharedFixtureValue {
        Arc::new(DemoBinding::default())
    }

    #[test]
    fn explicit_profile_runs() {
        let spec = DemoSpec;
        let profile = TestProfile {
            label: "unit_default",
            model_instance: ModelInstance::new("demo"),
            seeds: small::<DemoSpec>().with_fixture_factory(demo_factory),
            coverage: vec![CoverageGoal::Transitions],
            engines: vec![EnginePlan::ExplicitSuite],
        };
        let metadata = GeneratedSpecMetadata {
            spec_name: "DemoSpec",
            spec_slug: "DemoSpec",
            export_module: "crate::generated",
            crate_package: "nirvash-conformance",
            crate_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            normalized_fragment: normalized_fragment_info(&spec),
            default_profiles: &["unit_default"],
        };
        run_profile::<DemoSpec, DemoBinding>(
            &spec,
            &metadata,
            &profile,
            "DemoBinding",
            &ArtifactDirPolicy {
                base: std::env::temp_dir().join("nirvash_conformance_explicit"),
            },
            true,
        )
        .expect("explicit profile should pass");
    }

    #[test]
    fn kani_bounded_requires_materialized_harnesses() {
        let spec = DemoSpec;
        let profile = TestProfile {
            label: "kani_default",
            model_instance: ModelInstance::new("demo"),
            seeds: small::<DemoSpec>().with_fixture_factory(demo_factory),
            coverage: vec![CoverageGoal::Transitions],
            engines: vec![EnginePlan::KaniBounded { depth: 4 }],
        };
        let metadata = GeneratedSpecMetadata {
            spec_name: "DemoSpec",
            spec_slug: "DemoSpec",
            export_module: "crate::generated",
            crate_package: "nirvash-conformance",
            crate_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            normalized_fragment: normalized_fragment_info(&spec),
            default_profiles: &["kani_default"],
        };
        let error = run_profile::<DemoSpec, DemoBinding>(
            &spec,
            &metadata,
            &profile,
            "crate::DemoBinding",
            &ArtifactDirPolicy {
                base: std::env::temp_dir().join("nirvash_conformance_kani_runtime"),
            },
            true,
        )
        .expect_err("runtime Kani profile should require materialization");

        assert!(
            error
                .to_string()
                .contains("cargo nirvash materialize-tests --spec DemoSpec --binding crate::DemoBinding --profile kani_default")
        );
    }

    #[test]
    fn manifest_uses_plan_surface_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let spec = DemoSpec;
        let profile = TestProfile {
            label: "unit_default",
            model_instance: ModelInstance::new("demo"),
            seeds: small::<DemoSpec>().with_fixture_factory(demo_factory),
            coverage: vec![CoverageGoal::Transitions],
            engines: vec![EnginePlan::ExplicitSuite],
        };
        let metadata = GeneratedSpecMetadata {
            spec_name: "DemoSpec",
            spec_slug: "DemoSpec",
            export_module: "crate::generated",
            crate_package: "nirvash-conformance",
            crate_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            normalized_fragment: normalized_fragment_info(&spec),
            default_profiles: &["unit_default"],
        };

        run_profile::<DemoSpec, DemoBinding>(
            &spec,
            &metadata,
            &profile,
            "DemoBinding",
            &ArtifactDirPolicy {
                base: temp.path().join("nirvash_conformance_manifest"),
            },
            true,
        )
        .expect("profile should emit manifest");

        let manifest_path = temp
            .path()
            .join("nirvash_conformance_manifest")
            .join("manifest")
            .join("DemoSpec__DemoBinding__unit_default.json");
        let manifest: Value = serde_json::from_slice(&fs::read(manifest_path).expect("manifest"))
            .expect("decode manifest");
        assert_eq!(manifest["spec"], "DemoSpec");
        assert_eq!(manifest["binding"], "DemoBinding");
        assert_eq!(manifest["profile"], "unit_default");
        assert_eq!(manifest["materialize_failures"], true);
        assert!(manifest["replay_dir"].is_string());
        assert!(manifest["engine"].is_array());
    }

    #[test]
    fn trace_profile_runs() {
        let spec = DemoSpec;
        let profile = TestProfile {
            label: "e2e_default",
            model_instance: ModelInstance::new("demo"),
            seeds: e2e_default::<DemoSpec>().with_fixture_factory(demo_factory),
            coverage: vec![CoverageGoal::PropertyPrefixes],
            engines: vec![EnginePlan::TraceValidation {
                engine: TraceValidationEngine::Explicit,
            }],
        };
        let metadata = GeneratedSpecMetadata {
            spec_name: "DemoSpec",
            spec_slug: "DemoSpec",
            export_module: "crate::generated",
            crate_package: "nirvash-conformance",
            crate_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            normalized_fragment: normalized_fragment_info(&spec),
            default_profiles: &["e2e_default"],
        };
        run_trace_profile::<DemoSpec, DemoBinding>(
            &spec,
            &metadata,
            &profile,
            "DemoBinding",
            &ArtifactDirPolicy {
                base: std::env::temp_dir().join("nirvash_conformance_trace"),
            },
            true,
        )
        .expect("trace profile should pass");
    }

    #[test]
    fn trace_execution_records_invoke_update_return_and_stutter() {
        let outcome = execute_with_trace::<DemoSpec, DemoBinding>(
            &DemoSpec,
            &e2e_default::<DemoSpec>().with_fixture_factory(demo_factory),
            &[],
            &DemoState::Idle,
            &DemoAction::Start,
        )
        .expect("trace execution should succeed");

        assert_eq!(
            outcome.detail.events,
            vec![
                ObservedEvent::Invoke {
                    action: DemoAction::Start,
                },
                ObservedEvent::Update {
                    var: "last_output".to_owned(),
                    value: serde_json::to_value(DemoOutput::Ack).expect("serialize"),
                },
                ObservedEvent::Return {
                    action: DemoAction::Start,
                    output: Some(DemoOutput::Ack),
                },
                ObservedEvent::Stutter,
            ]
        );
    }

    #[test]
    fn snapshot_fixture_seed_restores_generated_fixture() {
        let seeds = SeedProfile::<DemoSpec> {
            label: "snapshot",
            fixture: FixtureSeed::Snapshot(serde_json::json!(1)),
            initial_state: None,
            typed: BTreeMap::new(),
            actions: BTreeMap::new(),
            environment: TestEnvironment::default(),
            shrink: ShrinkPolicy::ReplayOnly,
        };

        let fixture = resolve_fixture::<SnapshotBinding, DemoSpec>(&seeds)
            .expect("snapshot fixture should decode");
        assert_eq!(fixture, 1);
    }

    #[test]
    fn boundary_literals_seed_common_scalar_types() {
        let mut seeds = small::<DemoSpec>();
        insert_boundary_literal_seeds(
            &mut seeds,
            &nirvash_lower::BoundaryLiteralCatalog {
                comparison_literals: vec!["2".to_owned(), "true".to_owned(), "\"demo\"".to_owned()],
                cardinality_thresholds: vec![3],
                state_literals: vec!["false".to_owned()],
                action_literals: vec!["start".to_owned()],
                update_literals: Vec::new(),
                temporal_bad_prefix_guards: Vec::new(),
            },
        );

        let bools = typed_seed_values::<bool, DemoSpec>(&seeds).expect("decode bool seeds");
        let usizes = typed_seed_values::<usize, DemoSpec>(&seeds).expect("decode usize seeds");
        let strings = typed_seed_values::<String, DemoSpec>(&seeds).expect("decode string seeds");

        assert_eq!(bools, vec![true, false]);
        assert_eq!(usizes, vec![2, 3]);
        assert!(strings.contains(&"demo".to_owned()));
        assert!(strings.contains(&"start".to_owned()));
    }

    #[test]
    fn with_strategy_serializes_seed_values() {
        let seeds = small::<DemoSpec>()
            .with_strategy::<u64, _>(proptest::sample::select(vec![41_u64, 42_u64]));

        let values = typed_seed_values::<u64, DemoSpec>(&seeds).expect("decode strategy seeds");
        assert_eq!(values, vec![41, 42]);
    }

    #[test]
    fn registered_seed_strategy_precedes_arbitrary_and_singletons() {
        register_seed_strategy::<u32, _, _>(|| proptest::sample::select(vec![7_u32, 9_u32]));

        let candidates =
            typed_seed_candidates::<u32, DemoSpec>(&small::<DemoSpec>()).expect("seed candidates");

        assert!(candidates.starts_with(&[7, 9]));
        assert!(candidates.contains(&0));
        assert!(candidates.contains(&1));
    }

    #[test]
    fn registered_finite_domain_precedes_strategy_and_singletons() {
        let candidates = typed_seed_candidates::<FiniteSeedKey, DemoSpec>(&small::<DemoSpec>())
            .expect("finite-domain seed candidates");

        assert_eq!(candidates, vec![FiniteSeedKey::Alpha, FiniteSeedKey::Beta]);
    }

    #[test]
    fn seeds_macro_supports_initial_state_and_strategy() {
        let seeds = small::<DemoSpec>().with(seeds! {
            initial_state = DemoState::Idle;
            strategy u16 = proptest::sample::select(vec![5_u16, 6_u16]);
        });

        assert_eq!(seeds.initial_state, Some(DemoState::Idle));
        let values = typed_seed_values::<u16, DemoSpec>(&seeds).expect("decode strategy seeds");
        assert_eq!(values, vec![5, 6]);
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
    struct StrategyOnlyKey(u32);

    #[test]
    fn registered_strategy_supports_types_without_default_or_arbitrary() {
        register_seed_strategy::<StrategyOnlyKey, _, _>(|| {
            proptest::sample::select(vec![StrategyOnlyKey(3), StrategyOnlyKey(5)])
        });

        let candidates = typed_seed_candidates::<StrategyOnlyKey, DemoSpec>(&small::<DemoSpec>())
            .expect("seed candidates");

        assert!(candidates.starts_with(&[StrategyOnlyKey(3), StrategyOnlyKey(5)]));
    }

    #[test]
    fn sugar_overrides_build_seed_sets() {
        let seeded = small::<DemoSpec>().with(small_keys::<DemoSpec, _, _>(["a", "b"]));
        let boundary = small::<DemoSpec>().with(boundary_numbers::<DemoSpec, u64>());
        let fixture =
            small::<DemoSpec>().with(smoke_fixture::<DemoSpec, _>(DemoBinding::default()));

        assert_eq!(
            typed_seed_values::<String, DemoSpec>(&seeded).expect("string seeds"),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert_eq!(
            typed_seed_values::<u64, DemoSpec>(&boundary).expect("numeric seeds"),
            vec![0, 1, 2, 255]
        );
        assert_eq!(
            resolve_fixture::<DemoBinding, DemoSpec>(&fixture).expect("fixture seed"),
            DemoBinding::default()
        );
    }

    #[test]
    fn bounded_execution_rejects_initial_state_override_mismatch() {
        let error = execute_action_sequence::<DemoSpec, DemoBinding>(
            &DemoSpec,
            &small::<DemoSpec>()
                .with_fixture_factory(demo_factory)
                .with_initial_state(DemoState::Busy),
            &[DemoAction::Start],
        )
        .expect_err("mismatched initial state should fail");

        assert!(
            error
                .to_string()
                .contains("did not match seeds.initial_state")
        );
    }

    #[test]
    fn bounded_execution_rejects_non_initial_projected_state() {
        let error = execute_action_sequence::<DemoSpec, SnapshotBinding>(
            &DemoSpec,
            &small::<DemoSpec>().with_fixture(1_u8),
            &[DemoAction::Stop],
        )
        .expect_err("invalid projected initial state should fail");

        assert!(
            error
                .to_string()
                .contains("was not contained in spec.initial_states()")
        );
    }

    #[test]
    fn generated_action_candidates_prune_invalid_seeded_actions() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum NoisyAction {
            Start,
            Stop,
            Invalid,
        }

        #[derive(Default)]
        struct NoisySpec;

        impl FrontendSpec for NoisySpec {
            type State = DemoState;
            type Action = NoisyAction;

            fn initial_states(&self) -> Vec<Self::State> {
                vec![DemoState::Idle]
            }

            fn actions(&self) -> Vec<Self::Action> {
                vec![NoisyAction::Start, NoisyAction::Stop]
            }

            fn transition(
                &self,
                state: &Self::State,
                action: &Self::Action,
            ) -> Option<Self::State> {
                match (state, action) {
                    (DemoState::Idle, NoisyAction::Start) => Some(DemoState::Busy),
                    (DemoState::Busy, NoisyAction::Stop) => Some(DemoState::Idle),
                    _ => None,
                }
            }
        }

        impl TemporalSpec for NoisySpec {
            fn invariants(&self) -> Vec<nirvash::BoolExpr<Self::State>> {
                Vec::new()
            }
        }

        impl SpecOracle for NoisySpec {
            type ExpectedOutput = DemoOutput;

            fn expected_output(
                &self,
                _prev: &Self::State,
                _action: &Self::Action,
                next: Option<&Self::State>,
            ) -> Self::ExpectedOutput {
                if next.is_some() {
                    DemoOutput::Ack
                } else {
                    DemoOutput::Rejected
                }
            }
        }

        struct NoisyBinding;

        impl RuntimeBinding<NoisySpec> for NoisyBinding {
            type Sut = ();
            type Fixture = ();
            type Output = DemoOutput;
            type Error = DemoError;

            fn create(_fixture: Self::Fixture) -> Result<Self::Sut, Self::Error> {
                Ok(())
            }

            fn apply(
                _sut: &mut Self::Sut,
                _action: &NoisyAction,
                _env: &mut TestEnvironment,
            ) -> Result<Self::Output, Self::Error> {
                Ok(DemoOutput::Ack)
            }

            fn project(_sut: &Self::Sut) -> ProjectedState<DemoState> {
                ProjectedState::Unknown
            }

            fn project_output(_action: &NoisyAction, output: &Self::Output) -> DemoOutput {
                *output
            }
        }

        impl GeneratedBinding<NoisySpec> for NoisyBinding {
            fn generated_fixture() -> SharedFixtureValue {
                Arc::new(())
            }

            fn generated_action_candidates(
                _spec: &NoisySpec,
                _seeds: &SeedProfile<NoisySpec>,
            ) -> Result<Vec<NoisyAction>, HarnessError> {
                Ok(vec![NoisyAction::Invalid, NoisyAction::Start])
            }

            fn run_generated_profile(
                _spec: &NoisySpec,
                _metadata: &GeneratedSpecMetadata,
                _profile: &TestProfile<NoisySpec>,
                _binding_name: &str,
                _artifact_dir: &ArtifactDirPolicy,
                _materialize_failures: bool,
            ) -> Result<(), HarnessError> {
                Ok(())
            }

            fn run_generated_trace_profile(
                _spec: &NoisySpec,
                _metadata: &GeneratedSpecMetadata,
                _profile: &TestProfile<NoisySpec>,
                _binding_name: &str,
                _artifact_dir: &ArtifactDirPolicy,
                _materialize_failures: bool,
            ) -> Result<(), HarnessError> {
                Ok(())
            }

            fn run_generated_concurrent_profile(
                _spec: &NoisySpec,
                _metadata: &GeneratedSpecMetadata,
                _profile: &TestProfile<NoisySpec>,
                _binding_name: &str,
                _artifact_dir: &ArtifactDirPolicy,
                _materialize_failures: bool,
            ) -> Result<(), HarnessError> {
                Ok(())
            }
        }

        let actions = generated_action_candidates::<NoisySpec, NoisyBinding>(
            &NoisySpec,
            &small::<NoisySpec>(),
        )
        .expect("generated actions should prune invalid seeds");

        assert!(!actions.contains(&NoisyAction::Invalid));
        assert!(actions.contains(&NoisyAction::Start));
        assert!(actions.contains(&NoisyAction::Stop));
    }

    #[test]
    fn symbolic_trace_validation_fail_closes_for_unsupported_symbolic_spec() {
        let observed = ObservedActionTrace {
            initial: ProjectedState::Exact(DemoState::Idle),
            steps: vec![ObservedActionStep {
                action: DemoAction::Start,
                output: DemoOutput::Ack,
                after: ProjectedState::Exact(DemoState::Busy),
            }],
        };

        let error = assert_trace_refines(
            &DemoSpec,
            ModelInstance::new("demo"),
            &observed,
            TraceValidationEngine::Symbolic,
        )
        .expect_err("unsupported symbolic trace validation should fail closed");

        assert!(matches!(
            error,
            TraceRefinementError::ShapeMismatch { detail, .. }
                if detail.contains("symbolic constrained checking failed")
        ));
    }

    #[test]
    fn concurrent_profile_writes_schedule_artifact() {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile = TestProfile {
            label: "concurrency_default",
            model_instance: ModelInstance::new("demo"),
            seeds: concurrent_small::<DemoSpec>().with_fixture_factory(demo_factory),
            coverage: vec![CoverageGoal::Transitions, CoverageGoal::TransitionPairs(2)],
            engines: vec![EnginePlan::ShuttlePCT { depth: 2, runs: 8 }],
        };
        let metadata = GeneratedSpecMetadata {
            spec_name: "DemoSpec",
            spec_slug: "DemoSpec",
            export_module: "crate::generated",
            crate_package: "nirvash-conformance",
            crate_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            normalized_fragment: normalized_fragment_info(&DemoSpec),
            default_profiles: &["concurrency_default"],
        };

        run_concurrent_profile::<DemoSpec, DemoBinding>(
            &DemoSpec,
            &metadata,
            &profile,
            "crate::DemoBinding",
            &ArtifactDirPolicy {
                base: temp.path().join("nirvash_conformance_concurrent"),
            },
            true,
        )
        .expect("concurrent profile should persist schedule metadata");

        let replay_dir = temp
            .path()
            .join("nirvash_conformance_concurrent")
            .join("replay");
        let schedule_files = fs::read_dir(&replay_dir)
            .expect("replay dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("schedule_json"))
            })
            .collect::<Vec<_>>();
        assert_eq!(schedule_files.len(), 1);
        let artifact: Value =
            serde_json::from_slice(&fs::read(&schedule_files[0]).expect("schedule artifact"))
                .expect("decode schedule artifact");
        assert_eq!(artifact["binding"], "crate::DemoBinding");
        assert_eq!(artifact["schedule_seed"], 1);
        assert!(artifact["schedule"]["executed_actions"].is_array());

        let replay_bundle = fs::read_dir(&replay_dir)
            .expect("replay dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| {
                path.extension().and_then(|ext| ext.to_str()) == Some("json")
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.contains("shuttle_pct_bundle"))
            })
            .expect("concurrency replay bundle");
        let bundle: Value =
            serde_json::from_slice(&fs::read(&replay_bundle).expect("replay bundle"))
                .expect("decode replay bundle");
        assert_eq!(bundle["binding"], "crate::DemoBinding");
        assert_eq!(bundle["seed"]["environment"]["schedule_seed"], 1);
        assert!(bundle["schedule"]["executed_actions"].is_array());
    }
}
