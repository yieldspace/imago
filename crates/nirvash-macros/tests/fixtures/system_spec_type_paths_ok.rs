use nirvash::{BoolExpr, TransitionProgram};
use nirvash_lower::FrontendSpec;
use nirvash_macros::{invariant, nirvash_expr, nirvash_transition_program, subsystem_spec, system_spec};

mod child {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ChildState;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ChildAction;

    pub struct ChildSpec;

    #[subsystem_spec]
    impl FrontendSpec for ChildSpec {
        type State = ChildState;
        type Action = ChildAction;

        fn frontend_name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }

        fn initial_states(&self) -> Vec<Self::State> {
            vec![ChildState]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![ChildAction]
        }

        fn transition_program(
            &self,
        ) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(nirvash_transition_program! {
                rule step when true => {
                    set self <= ChildState;
                }
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RootState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RootAction;

struct RootSpec;

#[system_spec(subsystems(crate::child::ChildSpec))]
impl FrontendSpec for RootSpec {
    type State = RootState;
    type Action = RootAction;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![RootState]
    }

    fn actions(&self) -> Vec<Self::Action> {
        vec![RootAction]
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule step when true => {
                set self <= RootState;
            }
        })
    }
}

#[invariant(RootSpec)]
fn root_invariant() -> BoolExpr<RootState> {
    nirvash_expr! { root_invariant(_state) => true }
}

fn main() {
    let spec = RootSpec;
    let composition = spec.composition();
    assert_eq!(composition.subsystems(), RootSpec::registered_subsystems());
    assert_eq!(
        RootSpec::registered_subsystems()[0],
        nirvash::RegisteredSubsystemSpec::new("crate::child::ChildSpec", "ChildSpec")
    );
}
