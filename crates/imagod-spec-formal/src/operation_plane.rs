use nirvash::{BoolExpr, Fairness, Ltl, RelSet, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain, RelationalState,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, state_constraint,
    subsystem_spec,
};

use crate::{
    CommandErrorKind, CommandKind, CommandLifecycleState,
    atoms::{RemoteAuthorityAtom, ServiceAtom},
    bounds::doc_cap_focus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum RpcOutcome {
    None,
    LocalResolved,
    LocalDenied,
    RemoteConnected,
    RemoteCompleted,
    RemoteDenied,
    RemoteDisconnected,
}

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelationalState,
)]
pub struct OperationPlaneState {
    pub bound_services: RelSet<ServiceAtom>,
    pub remote_connections: RelSet<RemoteAuthorityAtom>,
    pub command_kind: Option<CommandKind>,
    pub command_state: Option<CommandLifecycleState>,
    pub cancel_requested: bool,
    pub local_rpc_target: Option<ServiceAtom>,
    pub remote_rpc_target: Option<ServiceAtom>,
    pub remote_rpc_authority: Option<RemoteAuthorityAtom>,
    pub last_rpc_outcome: RpcOutcome,
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
pub enum OperationPlaneAction {
    /// Grant one binding.
    GrantBinding(ServiceAtom),
    /// Revoke one binding.
    RevokeBinding(ServiceAtom),
    /// Start the command slot.
    StartCommand(CommandKind),
    /// Move the command slot to running.
    MarkCommandRunning,
    /// Request command cancellation.
    RequestCommandCancel,
    /// Finish the command successfully.
    FinishCommandSucceeded,
    /// Finish the command with failure.
    FinishCommandFailed(CommandErrorKind),
    /// Finish the command as canceled.
    FinishCommandCanceled,
    /// Clear the terminal command slot.
    ClearCommandSlot,
    /// Start a local RPC.
    StartLocalRpc(ServiceAtom),
    /// Complete a local RPC.
    CompleteLocalRpc(ServiceAtom),
    /// Deny a local RPC.
    DenyLocalRpc(ServiceAtom),
    /// Start a remote RPC.
    StartRemoteRpc(ServiceAtom, RemoteAuthorityAtom),
    /// Complete a remote RPC.
    CompleteRemoteRpc(ServiceAtom, RemoteAuthorityAtom),
    /// Deny a remote RPC.
    DenyRemoteRpc(ServiceAtom, RemoteAuthorityAtom),
    /// Disconnect a remote authority.
    DisconnectRemote(RemoteAuthorityAtom),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OperationPlaneSpec;

impl OperationPlaneSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> OperationPlaneState {
        OperationPlaneState {
            bound_services: RelSet::empty(),
            remote_connections: RelSet::empty(),
            command_kind: None,
            command_state: None,
            cancel_requested: false,
            local_rpc_target: None,
            remote_rpc_target: None,
            remote_rpc_authority: None,
            last_rpc_outcome: RpcOutcome::None,
        }
    }
}

fn operation_plane_model_cases() -> Vec<ModelInstance<OperationPlaneState, OperationPlaneAction>> {
    vec![
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

#[state_constraint(OperationPlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_state() -> BoolExpr<OperationPlaneState> {
    nirvash_expr! { symbolic_focus_state(state) =>
        !state.bound_services.contains(&ServiceAtom::Service1)
            && !state.remote_connections.contains(&RemoteAuthorityAtom::Edge1)
            && state.local_rpc_target != Some(ServiceAtom::Service1)
            && state.remote_rpc_target != Some(ServiceAtom::Service1)
            && state.remote_rpc_authority != Some(RemoteAuthorityAtom::Edge1)
    }
}

#[action_constraint(OperationPlaneSpec, cases("explicit_focus", "symbolic_focus"))]
fn symbolic_focus_actions() -> nirvash::StepExpr<OperationPlaneState, OperationPlaneAction> {
    nirvash_step_expr! { symbolic_focus_actions(_prev, action, _next) =>
        matches!(
            action,
            OperationPlaneAction::GrantBinding(ServiceAtom::Service0)
                | OperationPlaneAction::RevokeBinding(ServiceAtom::Service0)
                | OperationPlaneAction::StartCommand(_)
                | OperationPlaneAction::MarkCommandRunning
                | OperationPlaneAction::RequestCommandCancel
                | OperationPlaneAction::FinishCommandSucceeded
                | OperationPlaneAction::FinishCommandFailed(CommandErrorKind::Internal)
                | OperationPlaneAction::FinishCommandCanceled
                | OperationPlaneAction::ClearCommandSlot
                | OperationPlaneAction::StartLocalRpc(ServiceAtom::Service0)
                | OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service0)
                | OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service0)
                | OperationPlaneAction::StartRemoteRpc(
                    ServiceAtom::Service0,
                    RemoteAuthorityAtom::Edge0
                )
                | OperationPlaneAction::CompleteRemoteRpc(
                    ServiceAtom::Service0,
                    RemoteAuthorityAtom::Edge0
                )
                | OperationPlaneAction::DenyRemoteRpc(
                    ServiceAtom::Service0,
                    RemoteAuthorityAtom::Edge0
                )
                | OperationPlaneAction::DisconnectRemote(RemoteAuthorityAtom::Edge0)
        )
    }
}

#[invariant(OperationPlaneSpec)]
fn command_slot_is_coherent() -> BoolExpr<OperationPlaneState> {
    nirvash_expr! { command_slot_is_coherent(state) =>
        state.command_kind.is_some() == state.command_state.is_some()
    }
}

#[invariant(OperationPlaneSpec)]
fn local_and_remote_rpc_do_not_overlap() -> BoolExpr<OperationPlaneState> {
    nirvash_expr! { local_and_remote_rpc_do_not_overlap(state) =>
        state.local_rpc_target.is_none() || state.remote_rpc_target.is_none()
    }
}

#[invariant(OperationPlaneSpec)]
fn remote_rpc_authority_matches_target() -> BoolExpr<OperationPlaneState> {
    nirvash_expr! { remote_rpc_authority_matches_target(state) =>
        state.remote_rpc_target.is_some() == state.remote_rpc_authority.is_some()
    }
}

#[property(OperationPlaneSpec)]
fn command_start_leads_to_terminal_or_clear() -> Ltl<OperationPlaneState, OperationPlaneAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { command_inflight(state) =>
            matches!(
                state.command_state,
                Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
            )
        }),
        Ltl::pred(nirvash_expr! { command_terminal_or_clear(state) =>
            state.command_state.is_none()
                || state.command_state.is_some_and(CommandLifecycleState::is_terminal)
        }),
    )
}

#[property(OperationPlaneSpec)]
fn remote_rpc_leads_to_quiescence() -> Ltl<OperationPlaneState, OperationPlaneAction> {
    Ltl::leads_to(
        Ltl::pred(
            nirvash_expr! { remote_rpc_inflight(state) => state.remote_rpc_target.is_some() },
        ),
        Ltl::pred(nirvash_expr! { remote_rpc_cleared(state) => state.remote_rpc_target.is_none() }),
    )
}

#[fairness(OperationPlaneSpec)]
fn command_finish_progress() -> Fairness<OperationPlaneState, OperationPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { command_finish_progress(prev, action, next) =>
            matches!(prev.command_state, Some(CommandLifecycleState::Running))
                && matches!(
                    action,
                    OperationPlaneAction::FinishCommandSucceeded
                        | OperationPlaneAction::FinishCommandFailed(_)
                        | OperationPlaneAction::FinishCommandCanceled
                )
                && next.command_state.is_some_and(CommandLifecycleState::is_terminal)
        },
    )
}

#[fairness(OperationPlaneSpec)]
fn remote_rpc_progress() -> Fairness<OperationPlaneState, OperationPlaneAction> {
    Fairness::weak(
        nirvash_step_expr! { remote_rpc_progress(prev, action, next) =>
            prev.remote_rpc_target.is_some()
                && matches!(
                    action,
                    OperationPlaneAction::CompleteRemoteRpc(_, _)
                        | OperationPlaneAction::DisconnectRemote(_)
                )
                && next.remote_rpc_target.is_none()
        },
    )
}

#[subsystem_spec(model_cases(operation_plane_model_cases))]
impl FrontendSpec for OperationPlaneSpec {
    type State = OperationPlaneState;
    type Action = OperationPlaneAction;

    fn frontend_name(&self) -> &'static str {
        "operation_plane"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule grant_binding_0 when matches!(action, OperationPlaneAction::GrantBinding(ServiceAtom::Service0))
                && !prev.bound_services.contains(&ServiceAtom::Service0) => {
                insert bound_services <= ServiceAtom::Service0;
            }

            rule grant_binding_1 when matches!(action, OperationPlaneAction::GrantBinding(ServiceAtom::Service1))
                && !prev.bound_services.contains(&ServiceAtom::Service1) => {
                insert bound_services <= ServiceAtom::Service1;
            }

            rule revoke_binding_0 when matches!(action, OperationPlaneAction::RevokeBinding(ServiceAtom::Service0))
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.local_rpc_target != Some(ServiceAtom::Service0)
                && prev.remote_rpc_target != Some(ServiceAtom::Service0) => {
                remove bound_services <= ServiceAtom::Service0;
            }

            rule revoke_binding_1 when matches!(action, OperationPlaneAction::RevokeBinding(ServiceAtom::Service1))
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.local_rpc_target != Some(ServiceAtom::Service1)
                && prev.remote_rpc_target != Some(ServiceAtom::Service1) => {
                remove bound_services <= ServiceAtom::Service1;
            }

            rule start_command when matches!(action, OperationPlaneAction::StartCommand(_))
                && prev.command_state.is_none() => {
                set command_kind <= if matches!(action, OperationPlaneAction::StartCommand(CommandKind::Deploy)) {
                    Some(CommandKind::Deploy)
                } else if matches!(action, OperationPlaneAction::StartCommand(CommandKind::Run)) {
                    Some(CommandKind::Run)
                } else {
                    Some(CommandKind::Stop)
                };
                set command_state <= Some(CommandLifecycleState::Accepted);
                set cancel_requested <= false;
            }

            rule mark_command_running when matches!(action, OperationPlaneAction::MarkCommandRunning)
                && matches!(prev.command_state, Some(CommandLifecycleState::Accepted)) => {
                set command_state <= Some(CommandLifecycleState::Running);
            }

            rule request_command_cancel when matches!(action, OperationPlaneAction::RequestCommandCancel)
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                ) => {
                set cancel_requested <= true;
            }

            rule finish_command_succeeded when matches!(action, OperationPlaneAction::FinishCommandSucceeded)
                && matches!(prev.command_state, Some(CommandLifecycleState::Running)) => {
                set command_state <= Some(CommandLifecycleState::Succeeded);
                set cancel_requested <= false;
            }

            rule finish_command_failed when matches!(action, OperationPlaneAction::FinishCommandFailed(_))
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                ) => {
                set command_state <= Some(CommandLifecycleState::Failed);
                set cancel_requested <= false;
            }

            rule finish_command_canceled when matches!(action, OperationPlaneAction::FinishCommandCanceled)
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                )
                && prev.cancel_requested => {
                set command_state <= Some(CommandLifecycleState::Canceled);
                set cancel_requested <= false;
            }

            rule clear_command_slot when matches!(action, OperationPlaneAction::ClearCommandSlot)
                && prev.command_state.is_some_and(CommandLifecycleState::is_terminal) => {
                set command_kind <= None;
                set command_state <= None;
                set cancel_requested <= false;
            }

            rule start_local_rpc_0 when matches!(action, OperationPlaneAction::StartLocalRpc(ServiceAtom::Service0))
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set local_rpc_target <= Some(ServiceAtom::Service0);
                set last_rpc_outcome <= RpcOutcome::None;
            }

            rule start_local_rpc_1 when matches!(action, OperationPlaneAction::StartLocalRpc(ServiceAtom::Service1))
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set local_rpc_target <= Some(ServiceAtom::Service1);
                set last_rpc_outcome <= RpcOutcome::None;
            }

            rule complete_local_rpc_0 when matches!(action, OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service0))
                && prev.local_rpc_target == Some(ServiceAtom::Service0) => {
                set local_rpc_target <= None;
                set last_rpc_outcome <= RpcOutcome::LocalResolved;
            }

            rule complete_local_rpc_1 when matches!(action, OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service1))
                && prev.local_rpc_target == Some(ServiceAtom::Service1) => {
                set local_rpc_target <= None;
                set last_rpc_outcome <= RpcOutcome::LocalResolved;
            }

            rule deny_local_rpc_0 when matches!(action, OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service0))
                && !prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::LocalDenied;
            }

            rule deny_local_rpc_1 when matches!(action, OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service1))
                && !prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::LocalDenied;
            }

            rule start_remote_rpc_0 when matches!(action, OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service0, _))
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                insert remote_connections <= if matches!(
                    action,
                    OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge0)
                ) {
                    RemoteAuthorityAtom::Edge0
                } else {
                    RemoteAuthorityAtom::Edge1
                };
                set remote_rpc_target <= Some(ServiceAtom::Service0);
                set remote_rpc_authority <= if matches!(
                    action,
                    OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge0)
                ) {
                    Some(RemoteAuthorityAtom::Edge0)
                } else {
                    Some(RemoteAuthorityAtom::Edge1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteConnected;
            }

            rule start_remote_rpc_1 when matches!(action, OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service1, _))
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                insert remote_connections <= if matches!(
                    action,
                    OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge0)
                ) {
                    RemoteAuthorityAtom::Edge0
                } else {
                    RemoteAuthorityAtom::Edge1
                };
                set remote_rpc_target <= Some(ServiceAtom::Service1);
                set remote_rpc_authority <= if matches!(
                    action,
                    OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge0)
                ) {
                    Some(RemoteAuthorityAtom::Edge0)
                } else {
                    Some(RemoteAuthorityAtom::Edge1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteConnected;
            }

            rule complete_remote_rpc_0_edge0 when matches!(action, OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge0))
                && prev.remote_rpc_target == Some(ServiceAtom::Service0)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc_0_edge1 when matches!(action, OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge1))
                && prev.remote_rpc_target == Some(ServiceAtom::Service0)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc_1_edge0 when matches!(action, OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge0))
                && prev.remote_rpc_target == Some(ServiceAtom::Service1)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc_1_edge1 when matches!(action, OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge1))
                && prev.remote_rpc_target == Some(ServiceAtom::Service1)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule deny_remote_rpc_0 when matches!(action, OperationPlaneAction::DenyRemoteRpc(ServiceAtom::Service0, _))
                && !prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule deny_remote_rpc_1 when matches!(action, OperationPlaneAction::DenyRemoteRpc(ServiceAtom::Service1, _))
                && !prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule disconnect_remote_edge0 when matches!(action, OperationPlaneAction::DisconnectRemote(RemoteAuthorityAtom::Edge0))
                && prev.remote_connections.contains(&RemoteAuthorityAtom::Edge0) => {
                remove remote_connections <= RemoteAuthorityAtom::Edge0;
                set remote_rpc_target <= if prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) {
                    None
                } else {
                    state.remote_rpc_target
                };
                set remote_rpc_authority <= if prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) {
                    None
                } else {
                    state.remote_rpc_authority
                };
                set last_rpc_outcome <= RpcOutcome::RemoteDisconnected;
            }

            rule disconnect_remote_edge1 when matches!(action, OperationPlaneAction::DisconnectRemote(RemoteAuthorityAtom::Edge1))
                && prev.remote_connections.contains(&RemoteAuthorityAtom::Edge1) => {
                remove remote_connections <= RemoteAuthorityAtom::Edge1;
                set remote_rpc_target <= if prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) {
                    None
                } else {
                    state.remote_rpc_target
                };
                set remote_rpc_authority <= if prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) {
                    None
                } else {
                    state.remote_rpc_authority
                };
                set last_rpc_outcome <= RpcOutcome::RemoteDisconnected;
            }
        })
    }
}

#[nirvash_macros::formal_tests(spec = OperationPlaneSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check as checks;

    fn case_by_label(
        spec: &OperationPlaneSpec,
        label: &str,
    ) -> nirvash_lower::ModelInstance<OperationPlaneState, OperationPlaneAction> {
        spec.model_instances()
            .into_iter()
            .find(|case| case.label() == label)
            .expect("model case should exist")
    }

    fn bounded_parity_case(
        case: nirvash_lower::ModelInstance<OperationPlaneState, OperationPlaneAction>,
    ) -> nirvash_lower::ModelInstance<OperationPlaneState, OperationPlaneAction> {
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
        let spec = OperationPlaneSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_case = bounded_parity_case(case_by_label(&spec, "explicit_focus"));
        let symbolic_case = bounded_parity_case(case_by_label(&spec, "symbolic_focus"));

        let explicit_snapshot =
            checks::ExplicitModelChecker::for_case(&lowered, explicit_case.clone())
                .reachable_graph_snapshot()
                .expect("explicit operation snapshot");
        let symbolic_snapshot =
            checks::SymbolicModelChecker::for_case(&lowered, symbolic_case.clone())
                .reachable_graph_snapshot()
                .expect("symbolic operation snapshot");
        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }

    #[test]
    fn option_surface_is_symbolic_encodable() {
        let spec = OperationPlaneSpec::new();
        let program = spec
            .transition_program()
            .expect("operation plane should expose a transition program");

        assert!(program.is_ast_native());
        assert_eq!(program.first_unencodable_symbolic_node(), None);
    }
}
