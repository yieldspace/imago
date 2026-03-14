mod planner;

pub use nirvash_lower::{
    CheckerSpec, Counterexample, CounterexampleKind, ExplorationMode, FiniteModelDomain,
    LoweredSpec, ModelBackend, ModelCheckConfig, ModelCheckError, ModelCheckResult, ModelInstance,
    ReachableGraphSnapshot, Trace,
};
pub use planner::*;

type TraceVec<T> = Vec<Trace<<T as CheckerSpec>::State, <T as CheckerSpec>::Action>>;

pub struct ExplicitModelChecker<'a, T: CheckerSpec>(nirvash_backends::ExplicitModelChecker<'a, T>);

impl<'a, T> ExplicitModelChecker<'a, T>
where
    T: CheckerSpec,
    T::State: PartialEq + FiniteModelDomain + Send + Sync,
    T::Action: PartialEq + Send + Sync,
{
    pub fn new(spec: &'a T) -> Self {
        Self(nirvash_backends::ExplicitModelChecker::new(spec))
    }

    pub fn for_case(spec: &'a T, model_case: ModelInstance<T::State, T::Action>) -> Self {
        Self(nirvash_backends::ExplicitModelChecker::for_case(
            spec, model_case,
        ))
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        Self(nirvash_backends::ExplicitModelChecker::with_config(
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

    pub fn simulate(&self) -> Result<TraceVec<T>, ModelCheckError> {
        self.0.simulate()
    }

    pub fn candidate_traces(&self) -> Result<TraceVec<T>, ModelCheckError> {
        self.0.candidate_traces()
    }

    pub fn backend(&self) -> ModelBackend {
        self.0.backend()
    }

    pub fn doc_backend(&self) -> ModelBackend {
        self.0.doc_backend()
    }
}

pub struct SymbolicModelChecker<'a, T: CheckerSpec>(nirvash_backends::SymbolicModelChecker<'a, T>);

impl<'a, T> SymbolicModelChecker<'a, T>
where
    T: CheckerSpec,
    T::State: PartialEq + 'static,
    T::Action: PartialEq + 'static,
{
    pub fn new(spec: &'a T) -> Self {
        Self(nirvash_backends::SymbolicModelChecker::new(spec))
    }

    pub fn for_case(spec: &'a T, model_case: ModelInstance<T::State, T::Action>) -> Self {
        Self(nirvash_backends::SymbolicModelChecker::for_case(
            spec, model_case,
        ))
    }

    pub fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
        Self(nirvash_backends::SymbolicModelChecker::with_config(
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate as subject;
    use crate::CounterexampleKind;
    use nirvash::{
        BoolExpr, CounterexampleMinimization, ExplicitCheckpointOptions,
        ExplicitDistributedOptions, ExplicitParallelOptions, ExplicitReachabilityStrategy,
        ExplicitSimulationOptions, ExplicitStateCompression, ExplicitStateStorage, ExplorationMode,
        ExprDomain, GuardExpr, Ltl, StepExpr, SymbolicKInductionOptions, SymbolicModelCheckOptions,
        SymbolicPdrOptions, SymbolicSafetyEngine, SymbolicTemporalEngine, TraceStep,
        TransitionProgram, TransitionRule, TrustTier, UpdateOp, UpdateProgram, UpdateValueExprAst,
    };
    use nirvash_lower::{
        ClaimedReduction, FrontendSpec, HeuristicActionPruning, HeuristicReduction,
        HeuristicStateProjection, LoweringCx, ModelBackend, ModelCheckConfig, ModelInstance,
        PorReduction, ProofObligation, ProofObligationKind, ReductionClaim, StateQuotientReduction,
        TemporalSpec,
    };
    use nirvash_macros::{
        FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
        nirvash_transition_program,
    };

    enum TestChecker<'a, T: subject::CheckerSpec>
    where
        T::State: PartialEq + subject::FiniteModelDomain + Send + Sync + 'static,
        T::Action: PartialEq + Send + Sync + 'static,
    {
        Explicit(subject::ExplicitModelChecker<'a, T>),
        Symbolic(subject::SymbolicModelChecker<'a, T>),
    }

    impl<'a, T> TestChecker<'a, T>
    where
        T: subject::CheckerSpec,
        T::State: PartialEq + subject::FiniteModelDomain + Send + Sync + 'static,
        T::Action: PartialEq + Send + Sync + 'static,
    {
        fn with_config(spec: &'a T, config: ModelCheckConfig) -> Self {
            match config.backend.unwrap_or(
                spec.default_model_backend()
                    .unwrap_or(ModelBackend::Explicit),
            ) {
                ModelBackend::Explicit => {
                    Self::Explicit(subject::ExplicitModelChecker::with_config(spec, config))
                }
                ModelBackend::Symbolic => {
                    Self::Symbolic(subject::SymbolicModelChecker::with_config(spec, config))
                }
            }
        }

        fn for_case(
            spec: &'a T,
            model_case: ModelInstance<
                <T as subject::CheckerSpec>::State,
                <T as subject::CheckerSpec>::Action,
            >,
        ) -> Self {
            let resolved_model_case = model_case.with_resolved_backend(
                spec.default_model_backend()
                    .unwrap_or(ModelBackend::Explicit),
            );
            match resolved_model_case
                .effective_checker_config()
                .backend
                .unwrap_or(ModelBackend::Explicit)
            {
                ModelBackend::Explicit => Self::Explicit(subject::ExplicitModelChecker::for_case(
                    spec,
                    resolved_model_case,
                )),
                ModelBackend::Symbolic => Self::Symbolic(subject::SymbolicModelChecker::for_case(
                    spec,
                    resolved_model_case,
                )),
            }
        }

        fn full_reachable_graph_snapshot(
            &self,
        ) -> Result<subject::ReachableGraphSnapshot<T::State, T::Action>, subject::ModelCheckError>
        {
            match self {
                Self::Explicit(checker) => checker.full_reachable_graph_snapshot(),
                Self::Symbolic(checker) => checker.full_reachable_graph_snapshot(),
            }
        }

        fn check_invariants(
            &self,
        ) -> Result<subject::ModelCheckResult<T::State, T::Action>, subject::ModelCheckError>
        {
            match self {
                Self::Explicit(checker) => checker.check_invariants(),
                Self::Symbolic(checker) => checker.check_invariants(),
            }
        }

        fn check_deadlocks(
            &self,
        ) -> Result<subject::ModelCheckResult<T::State, T::Action>, subject::ModelCheckError>
        {
            match self {
                Self::Explicit(checker) => checker.check_deadlocks(),
                Self::Symbolic(checker) => checker.check_deadlocks(),
            }
        }

        fn check_properties(
            &self,
        ) -> Result<subject::ModelCheckResult<T::State, T::Action>, subject::ModelCheckError>
        {
            match self {
                Self::Explicit(checker) => checker.check_properties(),
                Self::Symbolic(checker) => checker.check_properties(),
            }
        }

        fn check_all(
            &self,
        ) -> Result<subject::ModelCheckResult<T::State, T::Action>, subject::ModelCheckError>
        {
            match self {
                Self::Explicit(checker) => checker.check_all(),
                Self::Symbolic(checker) => checker.check_all(),
            }
        }

        fn simulate(&self) -> Result<crate::TraceVec<T>, subject::ModelCheckError> {
            match self {
                Self::Explicit(checker) => checker.simulate(),
                Self::Symbolic(_) => Err(subject::ModelCheckError::UnsupportedConfiguration(
                    "simulation requires the explicit backend",
                )),
            }
        }

        fn candidate_traces(&self) -> Result<crate::TraceVec<T>, subject::ModelCheckError> {
            match self {
                Self::Explicit(checker) => checker.candidate_traces(),
                Self::Symbolic(_) => Err(subject::ModelCheckError::UnsupportedConfiguration(
                    "candidate trace enumeration requires the explicit backend",
                )),
            }
        }

        fn backend(&self) -> ModelBackend {
            match self {
                Self::Explicit(checker) => checker.backend(),
                Self::Symbolic(checker) => checker.backend(),
            }
        }
    }

    macro_rules! frontend_name {
        () => {
            fn frontend_name(&self) -> &'static str {
                std::any::type_name::<Self>()
            }
        };
    }

    fn lower_spec<T>(spec: &T) -> nirvash_lower::LoweredSpec<'_, T::State, T::Action>
    where
        T: TemporalSpec,
        T::State: PartialEq + nirvash_lower::FiniteModelDomain,
        T::Action: PartialEq,
    {
        let mut lowering_cx = LoweringCx;
        spec.lower(&mut lowering_cx).expect("spec should lower")
    }

    #[derive(
        Debug,
        Clone,
        Copy,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        FormalFiniteModelDomain,
        FormalSymbolicEncoding,
    )]
    enum Slot {
        Zero,
        One,
        Two,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum QuantAction {
        Advance,
        Reset,
    }

    #[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
    struct QuantState {
        ready: bool,
        slot: Slot,
    }

    #[derive(
        Debug,
        Clone,
        Copy,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        FormalFiniteModelDomain,
        FormalSymbolicEncoding,
    )]
    enum OptionPhase {
        Busy,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum OptionAction {
        Start,
        Clear,
    }

    #[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
    struct OptionState {
        phase: Option<OptionPhase>,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct OptionAstNativeSpec;

    impl FrontendSpec for OptionAstNativeSpec {
        type State = OptionState;
        type Action = OptionAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![OptionState { phase: None }]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![OptionAction::Start, OptionAction::Clear]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(nirvash_transition_program! {
                rule activate when prev.phase.is_none() && matches!(action, OptionAction::Start) => {
                    set phase <= Some(OptionPhase::Busy);
                }

                rule clear when prev.phase == Some(OptionPhase::Busy)
                    && matches!(action, OptionAction::Clear) => {
                    set phase <= None;
                }
            })
        }

        fn default_model_backend(&self) -> Option<ModelBackend> {
            Some(ModelBackend::Symbolic)
        }
    }

    impl TemporalSpec for OptionAstNativeSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![]
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct StructuralQuantifierSpec;

    impl StructuralQuantifierSpec {
        fn next_slot(slot: Slot) -> Slot {
            match slot {
                Slot::Zero => Slot::One,
                Slot::One | Slot::Two => Slot::Two,
            }
        }
    }

    impl FrontendSpec for StructuralQuantifierSpec {
        type State = QuantState;
        type Action = QuantAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![QuantState {
                ready: true,
                slot: Slot::Zero,
            }]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![QuantAction::Advance, QuantAction::Reset]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "structural_quantifiers",
                vec![
                    TransitionRule::ast(
                        "advance",
                        GuardExpr::exists_in(
                            "advance_ready",
                            ExprDomain::new("flags", [false, true]),
                            "flag && prev.ready && action == advance",
                            &["prev.ready"],
                            |prev: &QuantState, action: &QuantAction, flag: &bool| {
                                *flag && prev.ready && matches!(action, QuantAction::Advance)
                            },
                        ),
                        UpdateProgram::ast(
                            "advance",
                            vec![UpdateOp::assign_ast(
                                "slot",
                                UpdateValueExprAst::builtin_pure_call_with_paths(
                                    "next_slot",
                                    &["prev.slot"],
                                ),
                                |prev: &QuantState,
                                 state: &mut QuantState,
                                 action: &QuantAction| {
                                    if matches!(action, QuantAction::Advance) {
                                        state.slot = Self::next_slot(prev.slot);
                                    }
                                },
                            )],
                        ),
                    ),
                    TransitionRule::ast(
                        "reset",
                        GuardExpr::builtin_pure_call(
                            "is_reset",
                            |_prev: &QuantState, action: &QuantAction| {
                                matches!(action, QuantAction::Reset)
                            },
                        ),
                        UpdateProgram::ast(
                            "reset",
                            vec![UpdateOp::assign_ast(
                                "slot",
                                UpdateValueExprAst::literal("Slot::Zero"),
                                |_prev: &QuantState,
                                 state: &mut QuantState,
                                 _action: &QuantAction| {
                                    state.slot = Slot::Zero;
                                },
                            )],
                        ),
                    ),
                ],
            ))
        }

        fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
            vec![
                ModelInstance::new("structural_quantifiers").with_action_constraint(
                    StepExpr::exists_in(
                        "known_next_slot",
                        ExprDomain::of_finite_model_domain("slots"),
                        "candidate == next.slot",
                        &["next.slot"],
                        |_prev: &QuantState,
                         _action: &QuantAction,
                         next: &QuantState,
                         candidate: &Slot| *candidate == next.slot,
                    ),
                ),
            ]
        }

        fn default_model_backend(&self) -> Option<ModelBackend> {
            Some(ModelBackend::Symbolic)
        }
    }

    impl TemporalSpec for StructuralQuantifierSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![BoolExpr::forall_in(
                "slot_tautology",
                ExprDomain::of_finite_model_domain("slots"),
                "matches!(candidate, Slot::Zero | Slot::One | Slot::Two)",
                &["state.slot"],
                |_state: &QuantState, candidate: &Slot| {
                    matches!(candidate, Slot::Zero | Slot::One | Slot::Two)
                },
            )]
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct UnsafeInvariantSpec;

    impl FrontendSpec for UnsafeInvariantSpec {
        type State = QuantState;
        type Action = QuantAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![QuantState {
                ready: true,
                slot: Slot::Zero,
            }]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![QuantAction::Advance, QuantAction::Reset]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "unsafe_invariant",
                vec![TransitionRule::ast(
                    "advance",
                    GuardExpr::builtin_pure_call(
                        "is_advance",
                        |_prev: &QuantState, action: &QuantAction| {
                            matches!(action, QuantAction::Advance)
                        },
                    ),
                    UpdateProgram::ast(
                        "advance",
                        vec![UpdateOp::assign_ast(
                            "ready",
                            UpdateValueExprAst::literal("false"),
                            |_prev: &QuantState, state: &mut QuantState, _action: &QuantAction| {
                                state.ready = false;
                            },
                        )],
                    ),
                )],
            ))
        }

        fn default_model_backend(&self) -> Option<ModelBackend> {
            Some(ModelBackend::Symbolic)
        }
    }

    impl TemporalSpec for UnsafeInvariantSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            vec![BoolExpr::builtin_pure_call_with_paths(
                "ready_is_true",
                &["ready"],
                |state: &QuantState| state.ready,
            )]
        }
    }

    #[test]
    fn symbolic_backend_rejects_stringly_quantifiers_in_normalized_core() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let checker = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                ..ModelCheckConfig::default()
            },
        );

        let snapshot = checker
            .full_reachable_graph_snapshot()
            .expect_err("unsupported normalized fragments should fail closed");
        let invariants = checker
            .check_invariants()
            .expect_err("symbolic invariant checks should reject the same fragment");

        assert_eq!(checker.backend(), ModelBackend::Symbolic);
        assert!(matches!(
            snapshot,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
        assert!(matches!(
            invariants,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
    }

    #[test]
    fn symbolic_backend_accepts_option_ast_native_transition_program() {
        let spec = OptionAstNativeSpec;
        let program = spec
            .transition_program()
            .expect("option spec should expose a transition program");
        let lowered = lower_spec(&spec);
        let checker = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                ..ModelCheckConfig::default()
            },
        );

        let snapshot = checker
            .full_reachable_graph_snapshot()
            .expect("symbolic backend should encode Option AST-native transition programs");
        let result = checker
            .check_all()
            .expect("symbolic backend should check Option AST-native specs");

        assert!(program.is_ast_native());
        assert_eq!(program.first_unencodable_symbolic_node(), None);
        assert_eq!(checker.backend(), ModelBackend::Symbolic);
        assert!(!snapshot.truncated);
        assert_eq!(
            snapshot.states,
            vec![
                OptionState { phase: None },
                OptionState {
                    phase: Some(OptionPhase::Busy),
                },
            ]
        );
        assert_eq!(snapshot.initial_indices, vec![0]);
        assert_eq!(
            snapshot.edges[0],
            vec![nirvash::ReachableGraphEdge {
                action: OptionAction::Start,
                target: 1,
            }]
        );
        assert_eq!(
            snapshot.edges[1],
            vec![nirvash::ReachableGraphEdge {
                action: OptionAction::Clear,
                target: 0,
            }]
        );
        assert!(snapshot.deadlocks.is_empty());
        assert!(result.is_ok());
    }

    #[test]
    fn symbolic_k_induction_fail_closes_on_unsupported_normalized_fragment() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                max_states: Some(1),
                symbolic: SymbolicModelCheckOptions::current()
                    .with_safety(SymbolicSafetyEngine::KInduction)
                    .with_k_induction(SymbolicKInductionOptions::current().with_max_depth(2)),
                ..ModelCheckConfig::default()
            },
        )
        .check_invariants()
        .expect_err("k-induction should fail closed on unsupported normalized fragments");

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
    }

    #[test]
    fn symbolic_pdr_fail_closes_on_unsupported_normalized_fragment() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                max_states: Some(1),
                symbolic: SymbolicModelCheckOptions::current()
                    .with_safety(SymbolicSafetyEngine::PdrIc3)
                    .with_pdr(SymbolicPdrOptions::current().with_max_frames(3)),
                ..ModelCheckConfig::default()
            },
        )
        .check_invariants()
        .expect_err("PDR should fail closed on unsupported normalized fragments");

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
    }

    #[test]
    fn symbolic_k_induction_finds_invariant_counterexample() {
        let spec = UnsafeInvariantSpec;
        let lowered = lower_spec(&spec);
        let result = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                symbolic: SymbolicModelCheckOptions::current()
                    .with_safety(SymbolicSafetyEngine::KInduction)
                    .with_k_induction(SymbolicKInductionOptions::current().with_max_depth(2)),
                ..ModelCheckConfig::default()
            },
        )
        .check_invariants()
        .expect("k-induction should return a counterexample");

        let violation = &result.violations()[0];
        assert_eq!(violation.trace.states().len(), 2);
        assert_eq!(
            violation.trace.steps()[0],
            TraceStep::Action(QuantAction::Advance)
        );
        assert!(!violation.trace.states()[1].ready);
    }

    #[test]
    fn symbolic_pdr_finds_invariant_counterexample() {
        let spec = UnsafeInvariantSpec;
        let lowered = lower_spec(&spec);
        let result = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                symbolic: SymbolicModelCheckOptions::current()
                    .with_safety(SymbolicSafetyEngine::PdrIc3)
                    .with_pdr(SymbolicPdrOptions::current().with_max_frames(3)),
                ..ModelCheckConfig::default()
            },
        )
        .check_invariants()
        .expect("PDR should return a counterexample");

        let violation = &result.violations()[0];
        assert_eq!(violation.trace.states().len(), 2);
        assert_eq!(
            violation.trace.steps()[0],
            TraceStep::Action(QuantAction::Advance)
        );
        assert!(!violation.trace.states()[1].ready);
    }

    #[test]
    fn symbolic_bmc_finds_deadlock_without_bridge_graph() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let result = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                ..ModelCheckConfig::default()
            },
        )
        .check_deadlocks()
        .expect("symbolic BMC deadlock check should succeed");

        let violation = &result.violations()[0];
        assert_eq!(violation.kind, CounterexampleKind::Deadlock);
        assert_eq!(
            violation.trace.states().last(),
            Some(&SimulationState::Done)
        );
    }

    #[test]
    fn symbolic_temporal_checks_fail_close_on_unsupported_normalized_fragment() {
        let spec = SymbolicTemporalSpec;
        let lowered = lower_spec(&spec);
        let bounded_lasso = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                bounded_depth: Some(3),
                symbolic: SymbolicModelCheckOptions::current()
                    .with_temporal(SymbolicTemporalEngine::BoundedLasso),
                ..ModelCheckConfig::default()
            },
        )
        .check_properties()
        .expect_err("bounded lasso should fail closed on unsupported normalized fragments");
        let liveness_to_safety = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                exploration: ExplorationMode::ReachableGraph,
                bounded_depth: Some(3),
                symbolic: SymbolicModelCheckOptions::current()
                    .with_temporal(SymbolicTemporalEngine::LivenessToSafety),
                ..ModelCheckConfig::default()
            },
        )
        .check_properties()
        .expect_err("liveness-to-safety should fail closed on unsupported normalized fragments");

        assert!(matches!(
            bounded_lasso,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
        assert!(matches!(
            liveness_to_safety,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("stringly quantifiers")
        ));
    }

    #[derive(
        Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding,
    )]
    enum SimulationState {
        Left,
        Right,
        Done,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum SimulationAction {
        Finish,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum CandidateTraceAction {
        Start,
        Tick,
    }

    #[derive(
        Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding,
    )]
    enum CandidateTraceState {
        Idle,
        Loop,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum CounterexampleAction {
        TakeLong,
        TakeShort,
        Advance,
        Finish,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain)]
    enum CounterexampleState {
        Start,
        Long1,
        Long2,
        Done,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct CounterexampleSpec;

    impl FrontendSpec for CounterexampleSpec {
        type State = CounterexampleState;
        type Action = CounterexampleAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![CounterexampleState::Start]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![
                CounterexampleAction::TakeLong,
                CounterexampleAction::TakeShort,
                CounterexampleAction::Advance,
                CounterexampleAction::Finish,
            ]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (CounterexampleState::Start, CounterexampleAction::TakeLong) => {
                    Some(CounterexampleState::Long1)
                }
                (CounterexampleState::Start, CounterexampleAction::TakeShort) => {
                    Some(CounterexampleState::Done)
                }
                (CounterexampleState::Long1, CounterexampleAction::Advance) => {
                    Some(CounterexampleState::Long2)
                }
                (CounterexampleState::Long2, CounterexampleAction::Finish) => {
                    Some(CounterexampleState::Done)
                }
                _ => None,
            }
        }
    }

    impl TemporalSpec for CounterexampleSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::Always(Box::new(Ltl::Pred(
                BoolExpr::builtin_pure_call("not_done", |state: &CounterexampleState| {
                    !matches!(state, CounterexampleState::Done)
                }),
            )))]
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct SymbolicTemporalSpec;

    impl FrontendSpec for SymbolicTemporalSpec {
        type State = QuantState;
        type Action = QuantAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            StructuralQuantifierSpec.initial_states()
        }

        fn actions(&self) -> Vec<Self::Action> {
            StructuralQuantifierSpec.actions()
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            StructuralQuantifierSpec.transition_program()
        }

        fn model_instances(&self) -> Vec<ModelInstance<Self::State, Self::Action>> {
            StructuralQuantifierSpec.model_instances()
        }

        fn default_model_backend(&self) -> Option<ModelBackend> {
            Some(ModelBackend::Symbolic)
        }
    }

    impl TemporalSpec for SymbolicTemporalSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::Always(Box::new(Ltl::Pred(
                BoolExpr::builtin_pure_call_with_paths(
                    "slot_is_not_two",
                    &["slot"],
                    |state: &QuantState| !matches!(state.slot, Slot::Two),
                ),
            )))]
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct SimulationSpec;

    #[derive(Debug, Clone, Copy, Default)]
    struct CandidateTraceSpec;

    impl FrontendSpec for SimulationSpec {
        type State = SimulationState;
        type Action = SimulationAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![SimulationState::Left, SimulationState::Right]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![SimulationAction::Finish]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(TransitionProgram::named(
                "simulation",
                vec![
                    TransitionRule::ast(
                        "finish_left",
                        GuardExpr::builtin_pure_call(
                            "is_finish_left",
                            |prev: &SimulationState, action: &SimulationAction| {
                                matches!(
                                    (prev, action),
                                    (SimulationState::Left, SimulationAction::Finish)
                                )
                            },
                        ),
                        UpdateProgram::ast(
                            "finish_left",
                            vec![UpdateOp::assign_ast(
                                "self",
                                UpdateValueExprAst::literal("SimulationState::Done"),
                                |_prev: &SimulationState,
                                 state: &mut SimulationState,
                                 _action: &SimulationAction| {
                                    *state = SimulationState::Done;
                                },
                            )],
                        ),
                    ),
                    TransitionRule::ast(
                        "finish_right",
                        GuardExpr::builtin_pure_call(
                            "is_finish_right",
                            |prev: &SimulationState, action: &SimulationAction| {
                                matches!(
                                    (prev, action),
                                    (SimulationState::Right, SimulationAction::Finish)
                                )
                            },
                        ),
                        UpdateProgram::ast(
                            "finish_right",
                            vec![UpdateOp::assign_ast(
                                "self",
                                UpdateValueExprAst::literal("SimulationState::Done"),
                                |_prev: &SimulationState,
                                 state: &mut SimulationState,
                                 _action: &SimulationAction| {
                                    *state = SimulationState::Done;
                                },
                            )],
                        ),
                    ),
                ],
            ))
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (SimulationState::Left, SimulationAction::Finish)
                | (SimulationState::Right, SimulationAction::Finish) => Some(SimulationState::Done),
                (SimulationState::Done, SimulationAction::Finish) => None,
            }
        }
    }

    impl TemporalSpec for SimulationSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    impl FrontendSpec for CandidateTraceSpec {
        type State = CandidateTraceState;
        type Action = CandidateTraceAction;

        frontend_name!();

        fn initial_states(&self) -> Vec<Self::State> {
            vec![CandidateTraceState::Idle]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![CandidateTraceAction::Start, CandidateTraceAction::Tick]
        }

        fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
            match (state, action) {
                (CandidateTraceState::Idle, CandidateTraceAction::Start) => {
                    Some(CandidateTraceState::Loop)
                }
                (CandidateTraceState::Loop, CandidateTraceAction::Tick) => {
                    Some(CandidateTraceState::Loop)
                }
                _ => None,
            }
        }
    }

    impl TemporalSpec for CandidateTraceSpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    fn checkpoint_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "nirvash-check-{label}-{}-{stamp}.json",
            std::process::id()
        ))
    }

    #[test]
    fn explicit_simulation_is_seed_reproducible() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let explicit = nirvash::ExplicitModelCheckOptions::current()
            .with_simulation(ExplicitSimulationOptions::new(2, 4, 1));
        let config = ModelCheckConfig::reachable_graph().with_explicit_options(explicit);

        let left_run = TestChecker::with_config(&lowered, config.clone())
            .simulate()
            .expect("explicit simulation should run");
        let left_run_again = TestChecker::with_config(&lowered, config)
            .simulate()
            .expect("explicit simulation should be reproducible");
        let right_run = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_simulation(ExplicitSimulationOptions::new(2, 4, 2)),
            ),
        )
        .simulate()
        .expect("different seed should still run");

        assert_eq!(left_run, left_run_again);
        assert_eq!(left_run.len(), 2);
        assert_eq!(left_run[0].states()[0], SimulationState::Left);
        assert_eq!(right_run[0].states()[0], SimulationState::Right);
        assert!(matches!(
            left_run[0].steps().last(),
            Some(TraceStep::Stutter)
        ));
    }

    #[test]
    fn explicit_candidate_traces_include_terminal_and_looping_paths() {
        let spec = CandidateTraceSpec;
        let lowered = lower_spec(&spec);
        let traces = TestChecker::with_config(&lowered, ModelCheckConfig::bounded_lasso(2))
            .candidate_traces()
            .expect("explicit candidate traces should enumerate");

        assert!(traces.iter().any(|trace| {
            trace.states() == [CandidateTraceState::Idle]
                && trace.steps() == [TraceStep::Stutter]
                && trace.loop_start() == 0
        }));
        assert!(traces.iter().any(|trace| {
            trace.states() == [CandidateTraceState::Idle, CandidateTraceState::Loop]
                && trace.steps()
                    == [
                        TraceStep::Action(CandidateTraceAction::Start),
                        TraceStep::Action(CandidateTraceAction::Tick),
                    ]
                && trace.loop_start() == 1
        }));
    }

    #[test]
    fn explicit_reachable_graph_matches_fingerprinted_storage() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let fingerprinted = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_state_storage(ExplicitStateStorage::InMemoryFingerprinted),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("fingerprinted storage snapshot");

        assert_eq!(fingerprinted, exact);
    }

    #[test]
    fn explicit_reachable_graph_matches_domain_index_compression() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let compressed = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_compression(ExplicitStateCompression::DomainIndex),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("compressed storage snapshot");

        assert_eq!(compressed, exact);
    }

    #[test]
    fn explicit_reachable_graph_roundtrips_checkpoint_file() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let path = checkpoint_path("reachable-graph");
        let explicit = nirvash::ExplicitModelCheckOptions::current().with_checkpoint(
            ExplicitCheckpointOptions::at_path(path.display().to_string()),
        );
        let config = ModelCheckConfig::reachable_graph().with_explicit_options(explicit);

        let first = TestChecker::with_config(&lowered, config.clone())
            .full_reachable_graph_snapshot()
            .expect("checkpointed snapshot");
        let second = TestChecker::with_config(&lowered, config)
            .full_reachable_graph_snapshot()
            .expect("resumed checkpointed snapshot");

        assert_eq!(second, first);
        assert!(path.exists());
        fs::remove_file(path).expect("cleanup checkpoint file");
    }

    #[test]
    fn explicit_reachable_graph_roundtrips_checkpoint_file_with_domain_index_compression() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let path = checkpoint_path("reachable-graph-compressed");
        let explicit = nirvash::ExplicitModelCheckOptions::current()
            .with_compression(ExplicitStateCompression::DomainIndex)
            .with_checkpoint(ExplicitCheckpointOptions::at_path(
                path.display().to_string(),
            ));
        let config = ModelCheckConfig::reachable_graph().with_explicit_options(explicit);

        let first = TestChecker::with_config(&lowered, config.clone())
            .full_reachable_graph_snapshot()
            .expect("checkpointed compressed snapshot");
        let second = TestChecker::with_config(&lowered, config)
            .full_reachable_graph_snapshot()
            .expect("resumed checkpointed compressed snapshot");

        assert_eq!(second, first);
        assert!(path.exists());
        fs::remove_file(path).expect("cleanup checkpoint file");
    }

    #[test]
    fn explicit_reachable_graph_matches_parallel_frontier_strategy() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let parallel = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_reachability(ExplicitReachabilityStrategy::ParallelFrontier)
                    .with_parallel(ExplicitParallelOptions::current().with_workers(2)),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("parallel frontier snapshot");

        assert_eq!(parallel, exact);
    }

    #[test]
    fn explicit_reachable_graph_matches_distributed_frontier_strategy() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let distributed = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph().with_explicit_options(
                nirvash::ExplicitModelCheckOptions::current()
                    .with_reachability(ExplicitReachabilityStrategy::DistributedFrontier)
                    .with_distributed(ExplicitDistributedOptions::current().with_shards(3)),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("distributed frontier snapshot");

        assert_eq!(distributed, exact);
    }

    #[test]
    fn explicit_view_abstraction_collapses_equivalent_states() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let reduced = TestChecker::for_case(
            &lowered,
            ModelInstance::new("collapse_pending").with_heuristic_reduction(
                HeuristicReduction::new().with_state_projection(HeuristicStateProjection::new(
                    "collapse_pending",
                    |state: &SimulationState| match state {
                        SimulationState::Left | SimulationState::Right => "pending".to_owned(),
                        SimulationState::Done => "done".to_owned(),
                    },
                )),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("view-abstracted snapshot");

        assert_eq!(exact.states.len(), 3);
        assert_eq!(exact.trust_tier, TrustTier::Exact);
        assert_eq!(reduced.states.len(), 2);
        assert_eq!(reduced.initial_indices, vec![0]);
        assert_eq!(reduced.edges.len(), 2);
        assert_eq!(reduced.edges[0].len(), 1);
        assert_eq!(reduced.edges[0][0].action, SimulationAction::Finish);
        assert_eq!(reduced.edges[0][0].target, 1);
        assert_eq!(reduced.trust_tier, TrustTier::Heuristic);
    }

    #[test]
    fn explicit_partial_order_reduction_prunes_action_branches() {
        let spec = CounterexampleSpec;
        let lowered = lower_spec(&spec);
        let exact = TestChecker::with_config(&lowered, ModelCheckConfig::reachable_graph())
            .full_reachable_graph_snapshot()
            .expect("exact storage snapshot");
        let reduced = TestChecker::for_case(
            &lowered,
            ModelInstance::new("prefer_short_path").with_heuristic_reduction(
                HeuristicReduction::new().with_action_pruning(HeuristicActionPruning::new(
                    "prefer_short_path",
                    |state: &CounterexampleState, action: &CounterexampleAction| match state {
                        CounterexampleState::Start => {
                            matches!(action, CounterexampleAction::TakeShort)
                        }
                        CounterexampleState::Long1 => {
                            matches!(action, CounterexampleAction::Advance)
                        }
                        CounterexampleState::Long2 => {
                            matches!(action, CounterexampleAction::Finish)
                        }
                        CounterexampleState::Done => false,
                    },
                )),
            ),
        )
        .full_reachable_graph_snapshot()
        .expect("partial-order reduced snapshot");

        assert_eq!(exact.states.len(), 4);
        assert_eq!(
            reduced.states,
            vec![CounterexampleState::Start, CounterexampleState::Done]
        );
        assert_eq!(reduced.edges.len(), 2);
        assert_eq!(reduced.edges[0].len(), 1);
        assert_eq!(reduced.edges[0][0].action, CounterexampleAction::TakeShort);
        assert_eq!(reduced.edges[0][0].target, 1);
        assert_eq!(reduced.trust_tier, TrustTier::Heuristic);
    }

    #[test]
    fn explicit_sound_reduction_marks_snapshot_and_result_tier() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let model_case = ModelInstance::new("identity_quotient").with_claimed_reduction(
            ClaimedReduction::new().with_quotient(
                ReductionClaim::new(StateQuotientReduction::new(
                    "identity_quotient",
                    |state: &SimulationState| format!("{state:?}"),
                ))
                .with_obligation(ProofObligation::new(
                    "identity_quotient_sound".to_owned(),
                    ProofObligationKind::StateQuotientReduction,
                    "THEOREM identity_quotient_sound == QuotientSound".to_owned(),
                    "(assert QuotientSound)".to_owned(),
                )),
            ),
        );

        let snapshot = TestChecker::for_case(&lowered, model_case.clone())
            .full_reachable_graph_snapshot()
            .expect("sound-reduced snapshot");
        let result = TestChecker::for_case(&lowered, model_case)
            .check_all()
            .expect("sound-reduced check");

        assert_eq!(snapshot.trust_tier, TrustTier::ClaimedReduction);
        assert_eq!(result.trust_tier(), TrustTier::ClaimedReduction);
    }

    #[test]
    fn parallel_frontier_rejects_model_case_constraints() {
        let spec = SimulationSpec;
        let lowered = lower_spec(&spec);
        let model_case = ModelInstance::new("parallel_constraints")
            .with_checker_config(
                ModelCheckConfig::reachable_graph().with_explicit_options(
                    nirvash::ExplicitModelCheckOptions::current()
                        .with_reachability(ExplicitReachabilityStrategy::ParallelFrontier)
                        .with_parallel(ExplicitParallelOptions::current().with_workers(2)),
                ),
            )
            .with_state_constraint(BoolExpr::pure_call(
                "always_true_constraint",
                |_state: &SimulationState| true,
            ));

        let err = TestChecker::for_case(&lowered, model_case)
            .full_reachable_graph_snapshot()
            .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("parallel frontier exploration")
        ));
    }

    #[test]
    fn symbolic_backend_rejects_view_abstraction() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::for_case(
            &lowered,
            ModelInstance::new("symbolic_view")
                .with_checker_config(ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    exploration: ExplorationMode::ReachableGraph,
                    ..ModelCheckConfig::default()
                })
                .with_heuristic_reduction(HeuristicReduction::new().with_state_projection(
                    HeuristicStateProjection::new("ready_only", |state: &QuantState| {
                        format!("ready={}", state.ready)
                    }),
                )),
        )
        .full_reachable_graph_snapshot()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("heuristic state projection")
        ));
    }

    #[test]
    fn symbolic_backend_rejects_partial_order_reduction() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::for_case(
            &lowered,
            ModelInstance::new("symbolic_por")
                .with_checker_config(ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    exploration: ExplorationMode::ReachableGraph,
                    ..ModelCheckConfig::default()
                })
                .with_heuristic_reduction(HeuristicReduction::new().with_action_pruning(
                    HeuristicActionPruning::new(
                        "advance_only",
                        |_state: &QuantState, action: &QuantAction| {
                            matches!(action, QuantAction::Advance)
                        },
                    ),
                )),
        )
        .full_reachable_graph_snapshot()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("heuristic action pruning")
        ));
    }

    #[test]
    fn symbolic_backend_rejects_claimed_reduction() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::for_case(
            &lowered,
            ModelInstance::new("symbolic_sound_reduction")
                .with_checker_config(ModelCheckConfig {
                    backend: Some(ModelBackend::Symbolic),
                    exploration: ExplorationMode::ReachableGraph,
                    ..ModelCheckConfig::default()
                })
                .with_claimed_reduction(
                    ClaimedReduction::new().with_por(
                        ReductionClaim::new(PorReduction::new(
                            "advance_only",
                            |_state: &QuantState, action: &QuantAction| {
                                matches!(action, QuantAction::Advance)
                            },
                        ))
                        .with_obligation(ProofObligation::new(
                            "verified_por_sound".to_owned(),
                            ProofObligationKind::PorReduction,
                            "THEOREM verified_por_sound == PORSound".to_owned(),
                            "(assert PORSound)".to_owned(),
                        )),
                    ),
                ),
        )
        .full_reachable_graph_snapshot()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("claimed/certified reductions")
        ));
    }

    #[test]
    fn counterexample_minimization_prefers_shorter_property_trace() {
        let spec = CounterexampleSpec;
        let lowered = lower_spec(&spec);
        let without = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph()
                .with_counterexample_minimization(CounterexampleMinimization::None),
        )
        .check_properties()
        .expect("property check should run");
        let with = TestChecker::with_config(
            &lowered,
            ModelCheckConfig::reachable_graph()
                .with_counterexample_minimization(CounterexampleMinimization::ShortestTrace),
        )
        .check_properties()
        .expect("property check should run");

        let without_trace = &without.violations()[0].trace;
        let with_trace = &with.violations()[0].trace;

        assert!(without_trace.len() > with_trace.len());
        assert_eq!(
            without_trace.steps()[0],
            TraceStep::Action(CounterexampleAction::TakeLong)
        );
        assert_eq!(
            with_trace.steps()[0],
            TraceStep::Action(CounterexampleAction::TakeShort)
        );
    }

    #[test]
    fn symbolic_backend_rejects_simulation_mode() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::reachable_graph()
            },
        )
        .simulate()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("simulation")
        ));
    }

    #[test]
    fn symbolic_backend_rejects_candidate_trace_enumeration() {
        let spec = StructuralQuantifierSpec;
        let lowered = lower_spec(&spec);
        let err = TestChecker::with_config(
            &lowered,
            ModelCheckConfig {
                backend: Some(ModelBackend::Symbolic),
                ..ModelCheckConfig::bounded_lasso(2)
            },
        )
        .candidate_traces()
        .unwrap_err();

        assert!(matches!(
            err,
            nirvash::ModelCheckError::UnsupportedConfiguration(message)
                if message.contains("candidate trace enumeration")
        ));
    }
}
