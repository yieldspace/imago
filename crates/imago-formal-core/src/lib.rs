mod checker;
mod domain;
mod fairness;
mod ltl;
mod predicate;
mod symmetry;
mod system;
mod trace;

pub use checker::{
    Counterexample, CounterexampleKind, ExplorationMode, ModelCheckConfig, ModelCheckError,
    ModelCheckResult, ModelChecker,
};
pub use domain::{BoundedDomain, OpaqueModelValue, Signature};
pub use fairness::Fairness;
pub use ltl::Ltl;
pub use predicate::{ActionConstraint, StateConstraint, StatePredicate, StepPredicate};
pub use symmetry::SymmetryReducer;
pub use system::{SystemComposition, TemporalSpec, TransitionSystem};
pub use trace::{Trace, TraceStep};

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestState {
        Idle,
        Busy,
    }

    impl Signature for TestState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestAction {
        Start,
        Stop,
    }

    impl Signature for TestAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start, Self::Stop])
        }
    }

    fn is_idle(state: &TestState) -> bool {
        matches!(state, TestState::Idle)
    }

    fn starts_work(prev: &TestState, action: &TestAction, next: &TestState) -> bool {
        matches!(
            (prev, action, next),
            (TestState::Idle, TestAction::Start, TestState::Busy)
        )
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TestSpec;

    impl TransitionSystem for TestSpec {
        type State = TestState;
        type Action = TestAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, TestState::Idle)
        }

        fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
            matches!(
                (prev, action, next),
                (TestState::Idle, TestAction::Start, TestState::Busy)
                    | (TestState::Busy, TestAction::Stop, TestState::Idle)
            )
        }
    }

    impl TemporalSpec for TestSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            vec![StatePredicate::new("known_state", |_| true)]
        }

        fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>> {
            vec![StepPredicate::new("double_start", |prev, action, _| {
                matches!((prev, action), (TestState::Busy, TestAction::Start))
            })]
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![
                Ltl::leads_to(
                    Ltl::pred(StatePredicate::new("busy", |state| {
                        matches!(state, TestState::Busy)
                    })),
                    Ltl::pred(StatePredicate::new("idle", |state| {
                        matches!(state, TestState::Idle)
                    })),
                ),
                Ltl::always(Ltl::implies(
                    Ltl::enabled(StepPredicate::new("can_stop", |prev, action, next| {
                        matches!(
                            (prev, action, next),
                            (TestState::Busy, TestAction::Stop, TestState::Idle)
                        )
                    })),
                    Ltl::eventually(Ltl::pred(StatePredicate::new("idle_again", |state| {
                        matches!(state, TestState::Idle)
                    }))),
                )),
            ]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::strong(StepPredicate::new(
                "stop",
                |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (TestState::Busy, TestAction::Stop, TestState::Idle)
                    )
                },
            ))]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockState {
        Idle,
        Busy,
    }

    impl Signature for DeadlockState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DeadlockAction {
        Start,
    }

    impl Signature for DeadlockAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DeadlockSpec;

    impl TransitionSystem for DeadlockSpec {
        type State = DeadlockState;
        type Action = DeadlockAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, DeadlockState::Idle)
        }

        fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
            matches!(
                (prev, action, next),
                (
                    DeadlockState::Idle,
                    DeadlockAction::Start,
                    DeadlockState::Busy
                )
            )
        }
    }

    impl TemporalSpec for DeadlockSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>> {
            Vec::new()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterState {
        Cold,
        Warm,
    }

    impl Signature for StutterState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Cold, Self::Warm])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StutterAction {
        Tick,
    }

    impl Signature for StutterAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Tick])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct StutterSpec;

    impl TransitionSystem for StutterSpec {
        type State = StutterState;
        type Action = StutterAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, StutterState::Cold)
        }

        fn next(&self, _: &Self::State, _: &Self::Action, _: &Self::State) -> bool {
            false
        }

        fn stutter_state(&self, state: &Self::State) -> Self::State {
            match state {
                StutterState::Cold => StutterState::Warm,
                StutterState::Warm => StutterState::Warm,
            }
        }
    }

    impl TemporalSpec for StutterSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(StatePredicate::new(
                "warm",
                |state| matches!(state, StutterState::Warm),
            )))]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ControlState {
        Idle,
        Busy,
        Blocked,
    }

    impl Signature for ControlState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Idle, Self::Busy, Self::Blocked])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ControlAction {
        Start,
        Stop,
        Block,
    }

    impl Signature for ControlAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Start, Self::Stop, Self::Block])
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct ConstraintSpec {
        constrained: bool,
    }

    impl TransitionSystem for ConstraintSpec {
        type State = ControlState;
        type Action = ControlAction;

        fn init(&self, state: &Self::State) -> bool {
            matches!(state, ControlState::Idle)
        }

        fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
            matches!(
                (prev, action, next),
                (ControlState::Idle, ControlAction::Start, ControlState::Busy)
                    | (ControlState::Busy, ControlAction::Stop, ControlState::Idle)
                    | (
                        ControlState::Busy,
                        ControlAction::Block,
                        ControlState::Blocked
                    )
                    | (
                        ControlState::Blocked,
                        ControlAction::Stop,
                        ControlState::Idle
                    )
            )
        }
    }

    impl TemporalSpec for ConstraintSpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>> {
            Vec::new()
        }

        fn state_constraints(&self) -> Vec<StateConstraint<Self::State>> {
            if self.constrained {
                vec![StateConstraint::new("exclude_blocked", |state| {
                    !matches!(state, ControlState::Blocked)
                })]
            } else {
                Vec::new()
            }
        }

        fn action_constraints(&self) -> Vec<ActionConstraint<Self::State, Self::Action>> {
            if self.constrained {
                vec![ActionConstraint::new("disallow_block", |_, action, _| {
                    !matches!(action, ControlAction::Block)
                })]
            } else {
                Vec::new()
            }
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::leads_to(
                Ltl::pred(StatePredicate::new("busy", |state| {
                    matches!(state, ControlState::Busy)
                })),
                Ltl::pred(StatePredicate::new("idle", |state| {
                    matches!(state, ControlState::Idle)
                })),
            )]
        }

        fn fairness(&self) -> Vec<Fairness<Self::State, Self::Action>> {
            vec![Fairness::weak(StepPredicate::new(
                "stop_progress",
                |prev, action, next| {
                    matches!(
                        (prev, action, next),
                        (ControlState::Busy, ControlAction::Stop, ControlState::Idle)
                    )
                },
            ))]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SymmetryState {
        Left,
        Right,
    }

    impl Signature for SymmetryState {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Left, Self::Right])
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SymmetryAction {
        Swap,
    }

    impl Signature for SymmetryAction {
        fn bounded_domain() -> BoundedDomain<Self> {
            BoundedDomain::new(vec![Self::Swap])
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct SymmetrySpec;

    impl TransitionSystem for SymmetrySpec {
        type State = SymmetryState;
        type Action = SymmetryAction;

        fn init(&self, _: &Self::State) -> bool {
            true
        }

        fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
            matches!(
                (prev, action, next),
                (
                    SymmetryState::Left,
                    SymmetryAction::Swap,
                    SymmetryState::Right
                ) | (
                    SymmetryState::Right,
                    SymmetryAction::Swap,
                    SymmetryState::Left
                )
            )
        }
    }

    impl TemporalSpec for SymmetrySpec {
        fn invariants(&self) -> Vec<StatePredicate<Self::State>> {
            Vec::new()
        }

        fn illegal_transitions(&self) -> Vec<StepPredicate<Self::State, Self::Action>> {
            Vec::new()
        }

        fn properties(&self) -> Vec<Ltl<Self::State, Self::Action>> {
            vec![Ltl::eventually(Ltl::pred(StatePredicate::new(
                "left",
                |state| matches!(state, SymmetryState::Left),
            )))]
        }

        fn symmetry(&self) -> Option<SymmetryReducer<Self::State>> {
            Some(SymmetryReducer::new("collapse_lr", |_| SymmetryState::Left))
        }
    }

    #[test]
    fn bounded_domain_expands_cartesian_product() {
        let domain = TestState::bounded_domain().product(&TestAction::bounded_domain());
        assert_eq!(domain.len(), 4);
        assert!(
            domain
                .iter()
                .any(|(state, action)| *state == TestState::Busy && *action == TestAction::Stop)
        );
    }

    #[test]
    fn ltl_ast_builders_preserve_formula_shape() {
        let idle = StatePredicate::new("idle", is_idle);
        let start = StepPredicate::new("start", starts_work);

        let formula = Ltl::always(Ltl::until(
            Ltl::pred(idle),
            Ltl::and(Ltl::step(start), Ltl::enabled(start)),
        ));
        match formula {
            Ltl::Always(inner) => match *inner {
                Ltl::Until(lhs, rhs) => {
                    assert!(matches!(*lhs, Ltl::Pred(_)));
                    assert!(matches!(*rhs, Ltl::And(_, _)));
                }
                other => panic!("unexpected inner formula: {other:?}"),
            },
            other => panic!("unexpected formula: {other:?}"),
        }
    }

    #[test]
    fn predicates_evaluate_against_state_and_transition() {
        let idle = StatePredicate::new("idle", is_idle);
        let start = StepPredicate::new("start", starts_work);

        assert!(idle.eval(&TestState::Idle));
        assert!(!idle.eval(&TestState::Busy));
        assert!(start.eval(&TestState::Idle, &TestAction::Start, &TestState::Busy));
        assert!(!start.eval(&TestState::Busy, &TestAction::Stop, &TestState::Idle));
    }

    #[test]
    fn system_composition_collects_typed_spec_fragments() {
        let composition = SystemComposition::new("test-system")
            .with_subsystem("manager")
            .with_subsystem("runtime")
            .with_invariant(StatePredicate::new("idle", is_idle))
            .with_illegal_transition(StepPredicate::new("start", starts_work))
            .with_property(Ltl::eventually(Ltl::pred(StatePredicate::new(
                "idle", is_idle,
            ))))
            .with_fairness(Fairness::weak(StepPredicate::new("start", starts_work)))
            .with_checker_config(ModelCheckConfig::bounded_lasso(5));

        assert_eq!(composition.name(), "test-system");
        assert_eq!(composition.subsystems(), ["manager", "runtime"]);
        assert_eq!(composition.invariants().len(), 1);
        assert_eq!(composition.illegal_transitions().len(), 1);
        assert_eq!(composition.properties().len(), 1);
        assert_eq!(composition.fairness().len(), 1);
        assert_eq!(composition.checker_config().bounded_depth, Some(5));
        assert_eq!(
            composition.checker_config().exploration,
            ExplorationMode::BoundedLasso
        );
    }

    #[test]
    fn quantified_builders_expand_over_signature_domains() {
        let formula =
            Ltl::<TestState, TestAction>::forall::<TestAction, _>(|action| match action {
                TestAction::Start | TestAction::Stop => {
                    Ltl::pred(StatePredicate::new("idle", is_idle))
                }
            });

        assert!(formula.describe().contains("/\\"));
    }

    #[test]
    fn model_checker_accepts_simple_lasso_spec() {
        let spec = TestSpec;
        let checker = ModelChecker::with_config(&spec, ModelCheckConfig::bounded_lasso(3));
        assert!(checker.check_invariants().unwrap().is_ok());
        assert!(checker.check_illegal_transitions().unwrap().is_ok());
        assert!(checker.check_deadlocks().unwrap().is_ok());
        assert!(checker.check_properties().unwrap().is_ok());
    }

    #[test]
    fn model_checker_detects_deadlocks_and_respects_toggle() {
        let spec = DeadlockSpec;
        let deadlocks = ModelChecker::new(&spec).check_deadlocks().unwrap();
        assert!(!deadlocks.is_ok());
        assert!(matches!(
            deadlocks.violations()[0].kind,
            CounterexampleKind::Deadlock
        ));

        let checker = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        );
        assert!(checker.check_deadlocks().unwrap().is_ok());
    }

    #[test]
    fn stutter_state_can_drive_temporal_progress() {
        let spec = StutterSpec;
        let checker = ModelChecker::with_config(
            &spec,
            ModelCheckConfig {
                check_deadlocks: false,
                ..ModelCheckConfig::default()
            },
        );
        assert!(checker.check_properties().unwrap().is_ok());
    }

    #[test]
    fn constraints_prune_problematic_edges_from_the_graph() {
        let unconstrained = ConstraintSpec { constrained: false };
        let constrained = ConstraintSpec { constrained: true };
        let config = ModelCheckConfig {
            check_deadlocks: false,
            ..ModelCheckConfig::default()
        };

        assert!(
            !ModelChecker::with_config(&unconstrained, config)
                .check_properties()
                .unwrap()
                .is_ok()
        );
        assert!(
            ModelChecker::with_config(&constrained, config)
                .check_properties()
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn symmetry_with_temporal_properties_fails_closed() {
        let spec = SymmetrySpec;
        let err = ModelChecker::new(&spec).check_properties().unwrap_err();
        assert!(matches!(err, ModelCheckError::UnsupportedConfiguration(_)));
    }

    #[test]
    fn opaque_model_values_are_bounded() {
        struct OpaqueTag;

        let domain = OpaqueModelValue::<OpaqueTag, 3>::bounded_domain().into_vec();
        assert_eq!(domain.len(), 3);
        assert_eq!(domain[0].index(), 0);
        assert_eq!(domain[2].index(), 2);
    }
}
