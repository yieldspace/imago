//! Canonical system-core state and daemon-visible events.

use nirvash::RelSet;
use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain, RelationalState,
    SymbolicEncoding as FormalSymbolicEncoding,
};

use crate::{
    authorization::{
        AuthorizationDecision, BindingGrantId, ExternalMessage, InterfaceId, OperationPermission,
        SessionRequestState,
    },
    identity::{
        RemoteAuthorityId, ServiceId, SessionAuthState, SessionId, SessionRole, TransportPrincipal,
    },
    manager::{MaintenancePhase, ManagerPhase, ManagerShutdownPhase},
    operation::{CommandKind, CommandLifecycleState, CommandTerminalState, ManagerAuthState},
    rpc::RpcOutcome,
    service::ServiceLifecyclePhase,
};

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, ActionVocabulary,
)]
pub enum SystemEvent {
    LoadConfig(bool),
    FinishRestore,
    AcceptSession(SessionId, TransportPrincipal),
    AuthenticateSession(SessionId, SessionRole),
    DrainSession(SessionId),
    RequestMessage(SessionId, ExternalMessage),
    CompleteMessage(SessionId),
    PrepareService(ServiceId),
    CommitService(ServiceId),
    PromoteService(ServiceId),
    StartService(ServiceId),
    StopService(ServiceId),
    VerifyManagerAuth(ServiceId),
    GrantBinding(BindingGrantId),
    RegisterRemoteAuthority(RemoteAuthorityId),
    StartCommand(CommandKind),
    RequestCommandCancel,
    FinishCommand(CommandTerminalState),
    InvokeLocalRpc(ServiceId, ServiceId, InterfaceId),
    ConnectRemoteRpc(ServiceId, RemoteAuthorityId),
    InvokeRemoteRpc(ServiceId, RemoteAuthorityId, ServiceId, InterfaceId),
    DisconnectRemoteRpc(ServiceId),
    RequestShutdown,
    ConfirmSessionsDrained,
    ConfirmServicesStopped,
    ConfirmMaintenanceStopped,
    CompleteShutdown,
}

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelationalState,
)]
#[finite_model_domain(custom)]
pub struct SystemStateFragment {
    pub manager_config_loaded: bool,
    pub manager_created_default: bool,
    pub manager_phase: ManagerPhase,
    pub manager_shutdown_phase: ManagerShutdownPhase,
    pub manager_accepts_control: bool,
    pub maintenance_phase: MaintenancePhase,
    pub active_sessions: RelSet<SessionId>,
    pub session0_principal: TransportPrincipal,
    pub session1_principal: TransportPrincipal,
    pub session0_auth: SessionAuthState,
    pub session1_auth: SessionAuthState,
    pub session0_role: SessionRole,
    pub session1_role: SessionRole,
    pub session0_request: SessionRequestState,
    pub session1_request: SessionRequestState,
    pub service0_lifecycle: ServiceLifecyclePhase,
    pub service1_lifecycle: ServiceLifecyclePhase,
    pub service0_manager_auth: ManagerAuthState,
    pub service1_manager_auth: ManagerAuthState,
    pub binding_grants: RelSet<BindingGrantId>,
    pub trusted_authorities: RelSet<RemoteAuthorityId>,
    pub service0_remote_connection: Option<RemoteAuthorityId>,
    pub service1_remote_connection: Option<RemoteAuthorityId>,
    pub command_kind: Option<CommandKind>,
    pub command_state: Option<CommandLifecycleState>,
    pub command_cancel_requested: bool,
    pub local_rpc_target: Option<ServiceId>,
    pub remote_rpc_target: Option<ServiceId>,
    pub remote_rpc_authority: Option<RemoteAuthorityId>,
    pub last_message_session: Option<SessionId>,
    pub last_message: Option<ExternalMessage>,
    pub last_message_decision: AuthorizationDecision,
    pub last_operation_actor: Option<ServiceId>,
    pub last_operation_permission: Option<OperationPermission>,
    pub last_operation_decision: AuthorizationDecision,
    pub last_rpc_outcome: RpcOutcome,
}

nirvash::finite_model_domain_spec!(
    SystemStateFragmentFiniteModelDomainSpec for SystemStateFragment,
    representatives = system_state_domain()
);

impl SystemStateFragment {
    pub fn new() -> Self {
        Self {
            manager_config_loaded: false,
            manager_created_default: false,
            manager_phase: ManagerPhase::Booting,
            manager_shutdown_phase: ManagerShutdownPhase::Idle,
            manager_accepts_control: false,
            maintenance_phase: MaintenancePhase::Running,
            active_sessions: RelSet::empty(),
            session0_principal: TransportPrincipal::Unknown,
            session1_principal: TransportPrincipal::Unknown,
            session0_auth: SessionAuthState::Disconnected,
            session1_auth: SessionAuthState::Disconnected,
            session0_role: SessionRole::Unknown,
            session1_role: SessionRole::Unknown,
            session0_request: SessionRequestState::Idle,
            session1_request: SessionRequestState::Idle,
            service0_lifecycle: ServiceLifecyclePhase::Absent,
            service1_lifecycle: ServiceLifecyclePhase::Absent,
            service0_manager_auth: ManagerAuthState::Missing,
            service1_manager_auth: ManagerAuthState::Missing,
            binding_grants: RelSet::empty(),
            trusted_authorities: RelSet::empty(),
            service0_remote_connection: None,
            service1_remote_connection: None,
            command_kind: None,
            command_state: None,
            command_cancel_requested: false,
            local_rpc_target: None,
            remote_rpc_target: None,
            remote_rpc_authority: None,
            last_message_session: None,
            last_message: None,
            last_message_decision: AuthorizationDecision::NotEvaluated,
            last_operation_actor: None,
            last_operation_permission: None,
            last_operation_decision: AuthorizationDecision::NotEvaluated,
            last_rpc_outcome: RpcOutcome::None,
        }
    }
}

fn system_state_domain() -> Vec<SystemStateFragment> {
    let initial = SystemStateFragment::new();

    let mut listening = initial.clone();
    listening.manager_config_loaded = true;
    listening.manager_phase = ManagerPhase::Listening;
    listening.manager_accepts_control = true;

    let mut authenticated_admin = listening.clone();
    authenticated_admin
        .active_sessions
        .insert(SessionId::Session0);
    authenticated_admin.session0_principal = TransportPrincipal::Admin;
    authenticated_admin.session0_auth = SessionAuthState::Authenticated;
    authenticated_admin.session0_role = SessionRole::Admin;
    authenticated_admin.last_message_session = Some(SessionId::Session0);
    authenticated_admin.last_message = Some(ExternalMessage::HelloNegotiate);
    authenticated_admin.last_message_decision = AuthorizationDecision::Allow;
    authenticated_admin.session0_request = SessionRequestState::Completed;

    let mut running_services = authenticated_admin.clone();
    running_services.service0_lifecycle = ServiceLifecyclePhase::Running;
    running_services.service1_lifecycle = ServiceLifecyclePhase::Running;
    running_services.service0_manager_auth = ManagerAuthState::Verified;
    running_services.service1_manager_auth = ManagerAuthState::Verified;
    running_services
        .binding_grants
        .insert(BindingGrantId::Service0ToService1ControlApi);
    running_services
        .trusted_authorities
        .insert(RemoteAuthorityId::Authority0);
    running_services.service0_remote_connection = Some(RemoteAuthorityId::Authority0);
    running_services.command_kind = Some(CommandKind::Run);
    running_services.command_state = Some(CommandLifecycleState::Running);
    running_services.last_operation_actor = Some(ServiceId::Service0);
    running_services.last_operation_permission = Some(OperationPermission::RemoteInvoke);
    running_services.last_operation_decision = AuthorizationDecision::Allow;
    running_services.remote_rpc_target = Some(ServiceId::Service1);
    running_services.remote_rpc_authority = Some(RemoteAuthorityId::Authority0);
    running_services.last_rpc_outcome = RpcOutcome::RemoteInvoked;

    let mut denied_client = listening.clone();
    denied_client.active_sessions.insert(SessionId::Session1);
    denied_client.session1_principal = TransportPrincipal::Client;
    denied_client.session1_auth = SessionAuthState::Authenticated;
    denied_client.session1_role = SessionRole::Client;
    denied_client.last_message_session = Some(SessionId::Session1);
    denied_client.last_message = Some(ExternalMessage::CommandStart);
    denied_client.last_message_decision = AuthorizationDecision::DenyMessageNotAllowed;
    denied_client.session1_request = SessionRequestState::Denied;

    let mut shutdown_requested = running_services.clone();
    shutdown_requested.manager_phase = ManagerPhase::ShutdownRequested;
    shutdown_requested.manager_shutdown_phase = ManagerShutdownPhase::DrainingSessions;
    shutdown_requested.manager_accepts_control = false;
    shutdown_requested.last_message_decision = AuthorizationDecision::NotEvaluated;
    shutdown_requested.last_operation_decision = AuthorizationDecision::NotEvaluated;

    let mut stopped = shutdown_requested.clone();
    stopped.active_sessions = RelSet::empty();
    stopped.session0_auth = SessionAuthState::Drained;
    stopped.session1_auth = SessionAuthState::Drained;
    stopped.session0_request = SessionRequestState::Idle;
    stopped.session1_request = SessionRequestState::Idle;
    stopped.service0_lifecycle = ServiceLifecyclePhase::Reaped;
    stopped.service1_lifecycle = ServiceLifecyclePhase::Reaped;
    stopped.service0_remote_connection = None;
    stopped.service1_remote_connection = None;
    stopped.local_rpc_target = None;
    stopped.remote_rpc_target = None;
    stopped.remote_rpc_authority = None;
    stopped.command_kind = None;
    stopped.command_state = None;
    stopped.command_cancel_requested = false;
    stopped.maintenance_phase = MaintenancePhase::Stopped;
    stopped.manager_phase = ManagerPhase::Stopped;
    stopped.manager_shutdown_phase = ManagerShutdownPhase::Completed;
    stopped.last_rpc_outcome = RpcOutcome::RemoteDisconnected;

    vec![
        initial,
        listening,
        authenticated_admin,
        running_services,
        denied_client,
        shutdown_requested,
        stopped,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_lower::FiniteModelDomain;

    #[test]
    fn representative_domain_covers_running_and_shutdown_shapes() {
        let states = SystemStateFragment::bounded_domain().into_vec();
        assert!(
            states
                .iter()
                .any(|state| state.service0_lifecycle.is_running())
        );
        assert!(
            states
                .iter()
                .any(|state| state.manager_shutdown_phase == ManagerShutdownPhase::Completed)
        );
    }

    #[test]
    fn denied_message_state_keeps_reason_in_domain() {
        let states = SystemStateFragment::bounded_domain().into_vec();
        assert!(states.iter().any(|state| {
            state.last_message_decision == AuthorizationDecision::DenyMessageNotAllowed
        }));
    }
}
