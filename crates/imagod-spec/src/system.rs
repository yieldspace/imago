//! Canonical system-core state and daemon-visible events.

use std::any::{Any, TypeId};

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
    /// Load manager configuration and record whether defaults were created.
    #[viz(compact_label = "load-config", scenario_priority = 100)]
    LoadConfig(bool),
    /// Finish manager restore and begin accepting control traffic.
    #[viz(compact_label = "restore", scenario_priority = 95)]
    FinishRestore,
    /// Accept a transport session from the given principal.
    #[viz(compact_label = "accept-session", scenario_priority = 80)]
    AcceptSession(SessionId, TransportPrincipal),
    /// Bind an authenticated role to an accepted session.
    #[viz(compact_label = "auth-session", scenario_priority = 78)]
    AuthenticateSession(SessionId, SessionRole),
    /// Drain a session as part of shutdown or admission control.
    #[viz(compact_label = "drain-session", scenario_priority = 52)]
    DrainSession(SessionId),
    /// Submit an external control-plane message on a session.
    #[viz(compact_label = "request-msg", scenario_priority = 70)]
    RequestMessage(SessionId, ExternalMessage),
    /// Complete the currently active message on a session.
    #[viz(compact_label = "complete-msg", scenario_priority = 68)]
    CompleteMessage(SessionId),
    /// Materialize a service candidate before activation.
    #[viz(compact_label = "prepare-svc", scenario_priority = 60)]
    PrepareService(ServiceId),
    /// Commit a prepared service candidate.
    #[viz(compact_label = "commit-svc", scenario_priority = 58)]
    CommitService(ServiceId),
    /// Promote a committed service candidate to the active revision.
    #[viz(compact_label = "promote-svc", scenario_priority = 56)]
    PromoteService(ServiceId),
    /// Start a promoted service runtime.
    #[viz(compact_label = "start-svc", scenario_priority = 54)]
    StartService(ServiceId),
    /// Stop a running service runtime.
    #[viz(compact_label = "stop-svc", scenario_priority = 50)]
    StopService(ServiceId),
    /// Verify manager authorization for a service actor.
    #[viz(compact_label = "verify-auth", scenario_priority = 48)]
    VerifyManagerAuth(ServiceId),
    /// Grant a binding between service interfaces.
    #[viz(compact_label = "grant-bind", scenario_priority = 46)]
    GrantBinding(BindingGrantId),
    /// Register a trusted remote authority for cross-node RPC.
    #[viz(compact_label = "register-authz", scenario_priority = 44)]
    RegisterRemoteAuthority(RemoteAuthorityId),
    /// Start a command lifecycle slot.
    #[viz(compact_label = "start-cmd", scenario_priority = 42)]
    StartCommand(CommandKind),
    /// Request cancellation of the active command slot.
    #[viz(compact_label = "cancel-cmd", scenario_priority = 40)]
    RequestCommandCancel,
    /// Finish the active command slot with a terminal outcome.
    #[viz(compact_label = "finish-cmd", scenario_priority = 38)]
    FinishCommand(CommandTerminalState),
    /// Invoke a local RPC between two services.
    #[viz(compact_label = "rpc-local", scenario_priority = 36)]
    InvokeLocalRpc(ServiceId, ServiceId, InterfaceId),
    /// Establish a remote RPC connection for a service actor.
    #[viz(compact_label = "rpc-connect", scenario_priority = 34)]
    ConnectRemoteRpc(ServiceId, RemoteAuthorityId),
    /// Invoke a remote RPC through a trusted authority.
    #[viz(compact_label = "rpc-remote", scenario_priority = 32)]
    InvokeRemoteRpc(ServiceId, RemoteAuthorityId, ServiceId, InterfaceId),
    /// Disconnect a service from its remote RPC authority.
    #[viz(compact_label = "rpc-disconnect", scenario_priority = 30)]
    DisconnectRemoteRpc(ServiceId),
    /// Request manager shutdown.
    #[viz(compact_label = "shutdown", scenario_priority = 28)]
    RequestShutdown,
    /// Confirm that all sessions have drained.
    #[viz(compact_label = "sessions-drained", scenario_priority = 24)]
    ConfirmSessionsDrained,
    /// Confirm that all services have stopped.
    #[viz(compact_label = "services-stopped", scenario_priority = 22)]
    ConfirmServicesStopped,
    /// Confirm that maintenance work has stopped.
    #[viz(compact_label = "maintenance-stopped", scenario_priority = 20)]
    ConfirmMaintenanceStopped,
    /// Complete manager shutdown and enter the stopped phase.
    #[viz(compact_label = "shutdown-complete", scenario_priority = 18)]
    CompleteShutdown,
}

fn session_actor_name(session: SessionId) -> &'static str {
    match session {
        SessionId::Session0 => "Session0",
        SessionId::Session1 => "Session1",
    }
}

fn service_actor_name(service: ServiceId) -> &'static str {
    match service {
        ServiceId::Service0 => "Service0",
        ServiceId::Service1 => "Service1",
    }
}

fn authority_actor_name(authority: RemoteAuthorityId) -> &'static str {
    match authority {
        RemoteAuthorityId::Authority0 => "Authority0",
        RemoteAuthorityId::Authority1 => "Authority1",
    }
}

fn external_message_label(message: ExternalMessage) -> &'static str {
    match message {
        ExternalMessage::HelloNegotiate => "hello.negotiate",
        ExternalMessage::DeployPrepare => "deploy.prepare",
        ExternalMessage::ArtifactPush => "artifact.push",
        ExternalMessage::ArtifactCommit => "artifact.commit",
        ExternalMessage::CommandStart => "command.start",
        ExternalMessage::ServicesList => "services.list",
        ExternalMessage::StateRequest => "state.request",
        ExternalMessage::CommandCancel => "command.cancel",
        ExternalMessage::LogsRequest => "logs.request",
        ExternalMessage::RpcInvoke => "rpc.invoke",
        ExternalMessage::BindingsCertUpload => "bindings.cert.upload",
    }
}

fn interface_label(interface: InterfaceId) -> &'static str {
    match interface {
        InterfaceId::ControlApi => "control-api",
        InterfaceId::LogsApi => "logs-api",
    }
}

fn manager_do(
    label: impl Into<String>,
    compact_label: &'static str,
    scenario_priority: i32,
) -> nirvash::DocGraphActionPresentation {
    let label = label.into();
    nirvash::DocGraphActionPresentation::with_steps(
        label.clone(),
        Vec::new(),
        vec![nirvash::DocGraphProcessStep::for_actor(
            "Manager",
            nirvash::DocGraphProcessKind::Do,
            label,
        )],
    )
    .with_compact_label(compact_label)
    .with_scenario_priority(scenario_priority)
}

fn interaction_presentation(
    label: impl Into<String>,
    compact_label: &'static str,
    scenario_priority: i32,
    from: impl Into<String>,
    to: impl Into<String>,
) -> nirvash::DocGraphActionPresentation {
    let label = label.into();
    let from = from.into();
    let to = to.into();
    nirvash::DocGraphActionPresentation::with_steps(
        label.clone(),
        vec![nirvash::DocGraphInteractionStep::between(
            from.clone(),
            to.clone(),
            label.clone(),
        )],
        vec![
            nirvash::DocGraphProcessStep::for_actor(
                from,
                nirvash::DocGraphProcessKind::Send,
                label.clone(),
            ),
            nirvash::DocGraphProcessStep::for_actor(
                to,
                nirvash::DocGraphProcessKind::Receive,
                label,
            ),
        ],
    )
    .with_compact_label(compact_label)
    .with_scenario_priority(scenario_priority)
}

fn system_event_type_id() -> TypeId {
    TypeId::of::<SystemEvent>()
}

fn system_event_presentation(value: &dyn Any) -> Option<nirvash::DocGraphActionPresentation> {
    let event = value
        .downcast_ref::<SystemEvent>()
        .expect("registered system event presentation downcast");

    Some(match event {
        SystemEvent::LoadConfig(created_default) => manager_do(
            if *created_default {
                "Manager loads config and creates defaults"
            } else {
                "Manager loads existing config"
            },
            "load-config",
            100,
        ),
        SystemEvent::FinishRestore => manager_do("Manager finishes restore", "finish-restore", 95),
        SystemEvent::AcceptSession(session, principal) => interaction_presentation(
            format!("accept {:?} transport", principal),
            "accept-session",
            80,
            session_actor_name(*session),
            "Manager",
        ),
        SystemEvent::AuthenticateSession(session, role) => interaction_presentation(
            format!("authenticate {:?} session", role),
            "auth-session",
            78,
            session_actor_name(*session),
            "Manager",
        ),
        SystemEvent::DrainSession(session) => interaction_presentation(
            "drain session",
            "drain-session",
            52,
            "Manager",
            session_actor_name(*session),
        ),
        SystemEvent::RequestMessage(session, message) => interaction_presentation(
            external_message_label(*message),
            "request-msg",
            70,
            session_actor_name(*session),
            "Manager",
        ),
        SystemEvent::CompleteMessage(session) => interaction_presentation(
            "complete message",
            "complete-msg",
            68,
            "Manager",
            session_actor_name(*session),
        ),
        SystemEvent::PrepareService(service) => manager_do(
            format!("prepare {}", service_actor_name(*service)),
            "prepare-svc",
            60,
        ),
        SystemEvent::CommitService(service) => manager_do(
            format!("commit {}", service_actor_name(*service)),
            "commit-svc",
            58,
        ),
        SystemEvent::PromoteService(service) => manager_do(
            format!("promote {}", service_actor_name(*service)),
            "promote-svc",
            56,
        ),
        SystemEvent::StartService(service) => manager_do(
            format!("start {}", service_actor_name(*service)),
            "start-svc",
            54,
        ),
        SystemEvent::StopService(service) => manager_do(
            format!("stop {}", service_actor_name(*service)),
            "stop-svc",
            50,
        ),
        SystemEvent::VerifyManagerAuth(service) => manager_do(
            format!("verify manager auth for {}", service_actor_name(*service)),
            "verify-auth",
            48,
        ),
        SystemEvent::GrantBinding(grant) => {
            manager_do(format!("grant binding {:?}", grant), "grant-binding", 46)
        }
        SystemEvent::RegisterRemoteAuthority(authority) => manager_do(
            format!("register {}", authority_actor_name(*authority)),
            "register-authority",
            44,
        ),
        SystemEvent::StartCommand(kind) => {
            manager_do(format!("start {:?} command", kind), "start-cmd", 42)
        }
        SystemEvent::RequestCommandCancel => manager_do("request command cancel", "cancel-cmd", 40),
        SystemEvent::FinishCommand(state) => {
            manager_do(format!("finish command as {:?}", state), "finish-cmd", 38)
        }
        SystemEvent::InvokeLocalRpc(from, to, interface) => interaction_presentation(
            format!("invoke local {}", interface_label(*interface)),
            "rpc-local",
            36,
            service_actor_name(*from),
            service_actor_name(*to),
        ),
        SystemEvent::ConnectRemoteRpc(service, authority) => interaction_presentation(
            "connect remote rpc",
            "rpc-connect",
            34,
            service_actor_name(*service),
            authority_actor_name(*authority),
        ),
        SystemEvent::InvokeRemoteRpc(service, authority, target, interface) => {
            let label = format!("invoke remote {}", interface_label(*interface));
            nirvash::DocGraphActionPresentation::with_steps(
                label.clone(),
                vec![nirvash::DocGraphInteractionStep::between(
                    service_actor_name(*service),
                    authority_actor_name(*authority),
                    label.clone(),
                )],
                vec![
                    nirvash::DocGraphProcessStep::for_actor(
                        service_actor_name(*service),
                        nirvash::DocGraphProcessKind::Send,
                        label.clone(),
                    ),
                    nirvash::DocGraphProcessStep::for_actor(
                        authority_actor_name(*authority),
                        nirvash::DocGraphProcessKind::Receive,
                        label.clone(),
                    ),
                    nirvash::DocGraphProcessStep::for_actor(
                        service_actor_name(*target),
                        nirvash::DocGraphProcessKind::Do,
                        format!("handle {}", interface_label(*interface)),
                    ),
                ],
            )
            .with_compact_label("rpc-remote")
            .with_scenario_priority(32)
        }
        SystemEvent::DisconnectRemoteRpc(service) => interaction_presentation(
            "disconnect remote rpc",
            "rpc-disconnect",
            30,
            service_actor_name(*service),
            "Authority",
        ),
        SystemEvent::RequestShutdown => manager_do("request shutdown", "shutdown", 28),
        SystemEvent::ConfirmSessionsDrained => {
            manager_do("confirm sessions drained", "sessions-drained", 24)
        }
        SystemEvent::ConfirmServicesStopped => {
            manager_do("confirm services stopped", "services-stopped", 22)
        }
        SystemEvent::ConfirmMaintenanceStopped => {
            manager_do("confirm maintenance stopped", "maintenance-stopped", 20)
        }
        SystemEvent::CompleteShutdown => manager_do("complete shutdown", "shutdown-complete", 18),
    })
}

nirvash::inventory::submit! {
    nirvash::RegisteredActionDocPresentation {
        value_type_id: system_event_type_id,
        format: system_event_presentation,
    }
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
