use nirvash::{BoolExpr, Fairness, Ltl, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, state_constraint,
    subsystem_spec,
};

use crate::{
    atoms::ServiceAtom,
    bounds::{doc_cap_focus, doc_cap_surface},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum ServiceLifecyclePhase {
    Absent,
    Uploaded,
    Committed,
    Promoted,
    Ready,
    Running,
    Stopping,
    Reaped,
    RollbackPending,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub struct ServicePlaneState {
    pub service0: ServiceLifecyclePhase,
    pub service1: ServiceLifecyclePhase,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    ActionVocabulary,
)]
pub enum ServicePlaneAction {
    /// Upload one artifact.
    UploadArtifact(ServiceAtom),
    /// Commit an uploaded artifact.
    CommitArtifact(ServiceAtom),
    /// Promote a committed artifact.
    PromoteArtifact(ServiceAtom),
    /// Prepare a promoted service.
    PrepareService(ServiceAtom),
    /// Start a prepared service.
    StartService(ServiceAtom),
    /// Stop a running service.
    StopService(ServiceAtom),
    /// Reap a stopping service.
    ReapService(ServiceAtom),
    /// Trigger rollback.
    TriggerRollback(ServiceAtom),
    /// Finish rollback.
    FinishRollback(ServiceAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServicePlaneSpec;

impl ServicePlaneSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> ServicePlaneState {
        ServicePlaneState {
            service0: ServiceLifecyclePhase::Absent,
            service1: ServiceLifecyclePhase::Absent,
        }
    }
}

fn service_plane_model_cases() -> Vec<ModelInstance<ServicePlaneState, ServicePlaneAction>> {
    vec![
        ModelInstance::new("explicit_two_service_lifecycle")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_surface())
            .with_check_deadlocks(false),
        ModelInstance::new("explicit_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
        ModelInstance::new("symbolic_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
    ]
}

#[state_constraint(ServicePlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_state() -> BoolExpr<ServicePlaneState> {
    nirvash_expr! { symbolic_focus_state(state) =>
        matches!(state.service1, ServiceLifecyclePhase::Absent)
    }
}

#[action_constraint(ServicePlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_actions() -> nirvash::StepExpr<ServicePlaneState, ServicePlaneAction> {
    nirvash_step_expr! { symbolic_focus_actions(_prev, action, _next) =>
        matches!(
            action,
            ServicePlaneAction::UploadArtifact(ServiceAtom::Service0)
                | ServicePlaneAction::CommitArtifact(ServiceAtom::Service0)
                | ServicePlaneAction::PromoteArtifact(ServiceAtom::Service0)
                | ServicePlaneAction::PrepareService(ServiceAtom::Service0)
                | ServicePlaneAction::StartService(ServiceAtom::Service0)
                | ServicePlaneAction::StopService(ServiceAtom::Service0)
                | ServicePlaneAction::ReapService(ServiceAtom::Service0)
                | ServicePlaneAction::TriggerRollback(ServiceAtom::Service0)
                | ServicePlaneAction::FinishRollback(ServiceAtom::Service0)
        )
    }
}

#[invariant(ServicePlaneSpec)]
fn running_requires_ready_or_promoted() -> BoolExpr<ServicePlaneState> {
    nirvash_expr! { running_requires_ready_or_promoted(state) =>
        (!matches!(state.service0, ServiceLifecyclePhase::Running)
            || !matches!(state.service0, ServiceLifecyclePhase::RollbackPending | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped))
            && (!matches!(state.service1, ServiceLifecyclePhase::Running)
                || !matches!(state.service1, ServiceLifecyclePhase::RollbackPending | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped))
    }
}

#[invariant(ServicePlaneSpec)]
fn stopping_and_reaped_are_terminal_edges() -> BoolExpr<ServicePlaneState> {
    nirvash_expr! { stopping_and_reaped_are_terminal_edges(state) =>
        (!matches!(state.service0, ServiceLifecyclePhase::Reaped)
            || !matches!(state.service0, ServiceLifecyclePhase::Running))
            && (!matches!(state.service1, ServiceLifecyclePhase::Reaped)
                || !matches!(state.service1, ServiceLifecyclePhase::Running))
    }
}

#[property(ServicePlaneSpec)]
fn promoted_service_leads_to_running_or_rollback() -> Ltl<ServicePlaneState, ServicePlaneAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { promoted_service_exists(state) =>
            matches!(state.service0, ServiceLifecyclePhase::Promoted | ServiceLifecyclePhase::Ready)
                || matches!(state.service1, ServiceLifecyclePhase::Promoted | ServiceLifecyclePhase::Ready)
        }),
        Ltl::pred(nirvash_expr! { running_or_rollback_exists(state) =>
            matches!(state.service0, ServiceLifecyclePhase::Running | ServiceLifecyclePhase::RollbackPending | ServiceLifecyclePhase::RolledBack)
                || matches!(state.service1, ServiceLifecyclePhase::Running | ServiceLifecyclePhase::RollbackPending | ServiceLifecyclePhase::RolledBack)
        }),
    )
}

#[property(ServicePlaneSpec)]
fn stopping_service_leads_to_reaped() -> Ltl<ServicePlaneState, ServicePlaneAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { stopping_exists(state) =>
            matches!(state.service0, ServiceLifecyclePhase::Stopping)
                || matches!(state.service1, ServiceLifecyclePhase::Stopping)
        }),
        Ltl::pred(nirvash_expr! { reaped_exists(state) =>
            matches!(state.service0, ServiceLifecyclePhase::Reaped)
                || matches!(state.service1, ServiceLifecyclePhase::Reaped)
        }),
    )
}

#[fairness(ServicePlaneSpec)]
fn reap_progress() -> Fairness<ServicePlaneState, ServicePlaneAction> {
    Fairness::weak(nirvash_step_expr! { reap_progress(prev, action, next) =>
        (matches!(prev.service0, ServiceLifecyclePhase::Stopping)
            && matches!(action, ServicePlaneAction::ReapService(ServiceAtom::Service0))
            && matches!(next.service0, ServiceLifecyclePhase::Reaped))
        || (matches!(prev.service1, ServiceLifecyclePhase::Stopping)
            && matches!(action, ServicePlaneAction::ReapService(ServiceAtom::Service1))
            && matches!(next.service1, ServiceLifecyclePhase::Reaped))
    })
}

#[fairness(ServicePlaneSpec)]
fn rollback_progress() -> Fairness<ServicePlaneState, ServicePlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { rollback_progress(prev, action, next) =>
            (matches!(prev.service0, ServiceLifecyclePhase::RollbackPending)
                && matches!(action, ServicePlaneAction::FinishRollback(ServiceAtom::Service0))
                && matches!(next.service0, ServiceLifecyclePhase::RolledBack))
            || (matches!(prev.service1, ServiceLifecyclePhase::RollbackPending)
                && matches!(action, ServicePlaneAction::FinishRollback(ServiceAtom::Service1))
                && matches!(next.service1, ServiceLifecyclePhase::RolledBack))
        },
    )
}

#[subsystem_spec(model_cases(service_plane_model_cases))]
impl FrontendSpec for ServicePlaneSpec {
    type State = ServicePlaneState;
    type Action = ServicePlaneAction;

    fn frontend_name(&self) -> &'static str {
        "service_plane"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule upload_service0 when matches!(action, ServicePlaneAction::UploadArtifact(ServiceAtom::Service0))
                && matches!(
                    prev.service0,
                    ServiceLifecyclePhase::Absent | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped
                ) => {
                set service0 <= ServiceLifecyclePhase::Uploaded;
            }

            rule upload_service1 when matches!(action, ServicePlaneAction::UploadArtifact(ServiceAtom::Service1))
                && matches!(
                    prev.service1,
                    ServiceLifecyclePhase::Absent | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped
                ) => {
                set service1 <= ServiceLifecyclePhase::Uploaded;
            }

            rule commit_service0 when matches!(action, ServicePlaneAction::CommitArtifact(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Uploaded) => {
                set service0 <= ServiceLifecyclePhase::Committed;
            }

            rule commit_service1 when matches!(action, ServicePlaneAction::CommitArtifact(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Uploaded) => {
                set service1 <= ServiceLifecyclePhase::Committed;
            }

            rule promote_service0 when matches!(action, ServicePlaneAction::PromoteArtifact(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Committed) => {
                set service0 <= ServiceLifecyclePhase::Promoted;
            }

            rule promote_service1 when matches!(action, ServicePlaneAction::PromoteArtifact(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Committed) => {
                set service1 <= ServiceLifecyclePhase::Promoted;
            }

            rule prepare_service0 when matches!(action, ServicePlaneAction::PrepareService(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Promoted) => {
                set service0 <= ServiceLifecyclePhase::Ready;
            }

            rule prepare_service1 when matches!(action, ServicePlaneAction::PrepareService(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Promoted) => {
                set service1 <= ServiceLifecyclePhase::Ready;
            }

            rule start_service0 when matches!(action, ServicePlaneAction::StartService(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Ready) => {
                set service0 <= ServiceLifecyclePhase::Running;
            }

            rule start_service1 when matches!(action, ServicePlaneAction::StartService(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Ready) => {
                set service1 <= ServiceLifecyclePhase::Running;
            }

            rule stop_service0 when matches!(action, ServicePlaneAction::StopService(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Running) => {
                set service0 <= ServiceLifecyclePhase::Stopping;
            }

            rule stop_service1 when matches!(action, ServicePlaneAction::StopService(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Running) => {
                set service1 <= ServiceLifecyclePhase::Stopping;
            }

            rule reap_service0 when matches!(action, ServicePlaneAction::ReapService(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::Stopping) => {
                set service0 <= ServiceLifecyclePhase::Reaped;
            }

            rule reap_service1 when matches!(action, ServicePlaneAction::ReapService(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::Stopping) => {
                set service1 <= ServiceLifecyclePhase::Reaped;
            }

            rule trigger_rollback0 when matches!(action, ServicePlaneAction::TriggerRollback(ServiceAtom::Service0))
                && matches!(
                    prev.service0,
                    ServiceLifecyclePhase::Committed
                        | ServiceLifecyclePhase::Promoted
                        | ServiceLifecyclePhase::Ready
                ) => {
                set service0 <= ServiceLifecyclePhase::RollbackPending;
            }

            rule trigger_rollback1 when matches!(action, ServicePlaneAction::TriggerRollback(ServiceAtom::Service1))
                && matches!(
                    prev.service1,
                    ServiceLifecyclePhase::Committed
                        | ServiceLifecyclePhase::Promoted
                        | ServiceLifecyclePhase::Ready
                ) => {
                set service1 <= ServiceLifecyclePhase::RollbackPending;
            }

            rule finish_rollback0 when matches!(action, ServicePlaneAction::FinishRollback(ServiceAtom::Service0))
                && matches!(prev.service0, ServiceLifecyclePhase::RollbackPending) => {
                set service0 <= ServiceLifecyclePhase::RolledBack;
            }

            rule finish_rollback1 when matches!(action, ServicePlaneAction::FinishRollback(ServiceAtom::Service1))
                && matches!(prev.service1, ServiceLifecyclePhase::RollbackPending) => {
                set service1 <= ServiceLifecyclePhase::RolledBack;
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = ServicePlaneSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check as checks;

    fn case_by_label(
        spec: &ServicePlaneSpec,
        label: &str,
    ) -> nirvash_lower::ModelInstance<ServicePlaneState, ServicePlaneAction> {
        spec.model_instances()
            .into_iter()
            .find(|case| case.label() == label)
            .expect("model case should exist")
    }

    fn bounded_parity_case(
        case: nirvash_lower::ModelInstance<ServicePlaneState, ServicePlaneAction>,
    ) -> nirvash_lower::ModelInstance<ServicePlaneState, ServicePlaneAction> {
        let mut config = case.effective_checker_config();
        let doc_config = case.doc_checker_config().map(|mut config| {
            config.max_states = Some(64);
            config.max_transitions = Some(256);
            config
        });
        config.max_states = Some(64);
        config.max_transitions = Some(256);
        let case = case.with_checker_config(config);
        match doc_config {
            Some(doc_config) => case.with_doc_checker_config(doc_config),
            None => case,
        }
    }

    #[test]
    fn explicit_and_symbolic_backends_agree() {
        let spec = ServicePlaneSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_case = bounded_parity_case(case_by_label(&spec, "explicit_focus"));
        let symbolic_case = bounded_parity_case(case_by_label(&spec, "symbolic_focus"));

        let explicit_snapshot =
            checks::ExplicitModelChecker::for_case(&lowered, explicit_case.clone())
                .reachable_graph_snapshot()
                .expect("explicit service snapshot");
        let symbolic_snapshot =
            checks::SymbolicModelChecker::for_case(&lowered, symbolic_case.clone())
                .reachable_graph_snapshot()
                .expect("symbolic service snapshot");
        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }
}
