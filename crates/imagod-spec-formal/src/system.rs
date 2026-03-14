use nirvash::{BoolExpr, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    invariant, nirvash_expr, nirvash_step_expr, nirvash_transition_program, system_spec,
};

use crate::bounds::{MAX_LASSO_DEPTH, doc_cap_focus, doc_cap_surface};
use imagod_spec::{
    AuthorizationDecision, BindingGrantId, ExternalMessage, InterfaceId, MaintenancePhase,
    ManagerAuthState, ManagerPhase, ManagerShutdownPhase, OperationPermission, RemoteAuthorityId,
    RpcOutcome, ServiceId, ServiceLifecyclePhase, SessionAuthState, SessionId, SessionRequestState,
    SessionRole, SystemEvent, SystemStateFragment, TransportPrincipal,
};

pub type SystemState = SystemStateFragment;
pub type SystemAction = SystemEvent;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpec;

impl SystemSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SystemState {
        SystemState::new()
    }
}

fn base_model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_system_surface")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_surface())
            .with_check_deadlocks(false),
        ModelInstance::new("explicit_shutdown_progress")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::bounded_lasso(MAX_LASSO_DEPTH)
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
        ModelInstance::new("explicit_multi_service_scenario")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_surface())
            .with_check_deadlocks(false),
        ModelInstance::new("explicit_symbolic_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false)
            .with_action_constraint(symbolic_focus_actions()),
        ModelInstance::new("symbolic_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false)
            .with_action_constraint(symbolic_focus_actions()),
    ]
}

fn symbolic_focus_actions() -> nirvash::StepExpr<SystemState, SystemAction> {
    nirvash_step_expr! { symbolic_focus_actions(_prev, action, _next) =>
        matches!(action,
            SystemEvent::LoadConfig(false)
                | SystemEvent::FinishRestore
                | SystemEvent::AcceptSession(SessionId::Session0, TransportPrincipal::Client)
                | SystemEvent::AuthenticateSession(SessionId::Session0, SessionRole::Client)
                | SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::HelloNegotiate)
                | SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::RpcInvoke)
                | SystemEvent::CompleteMessage(SessionId::Session0)
                | SystemEvent::PrepareService(ServiceId::Service0)
                | SystemEvent::CommitService(ServiceId::Service0)
                | SystemEvent::PromoteService(ServiceId::Service0)
                | SystemEvent::StartService(ServiceId::Service0)
                | SystemEvent::RequestShutdown
                | SystemEvent::DrainSession(SessionId::Session0)
                | SystemEvent::ConfirmSessionsDrained
                | SystemEvent::StopService(ServiceId::Service0)
                | SystemEvent::ConfirmServicesStopped
                | SystemEvent::ConfirmMaintenanceStopped
                | SystemEvent::CompleteShutdown
        )
    }
}

fn system_model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    let mut cases = base_model_cases();
    cases.extend(crate::manager_view::model_cases());
    cases.extend(crate::control_view::model_cases());
    cases.extend(crate::service_view::model_cases());
    cases.extend(crate::operation_view::model_cases());
    cases.extend(crate::authz_view::model_cases());
    cases
}

#[invariant(SystemSpec)]
fn shutdown_disables_new_control() -> BoolExpr<SystemState> {
    nirvash_expr! { shutdown_disables_new_control(state) =>
        state.manager_phase != ManagerPhase::ShutdownRequested || !state.manager_accepts_control
    }
}

#[invariant(SystemSpec)]
fn completed_shutdown_is_quiescent() -> BoolExpr<SystemState> {
    nirvash_expr! { completed_shutdown_is_quiescent(state) =>
        state.manager_shutdown_phase != ManagerShutdownPhase::Completed
            || (
                state.manager_phase == ManagerPhase::Stopped
                    && !state.manager_accepts_control
                    && !state.active_sessions.contains(&SessionId::Session0)
                    && !state.active_sessions.contains(&SessionId::Session1)
                    && state.command_state == None
                    && state.local_rpc_target == None
                    && state.remote_rpc_target == None
                    && state.remote_rpc_authority == None
            )
    }
}

#[invariant(SystemSpec)]
fn allowed_remote_rpc_requires_target_and_authority() -> BoolExpr<SystemState> {
    nirvash_expr! { allowed_remote_rpc_requires_target_and_authority(state) =>
        state.last_operation_decision != AuthorizationDecision::Allow
            || state.last_operation_permission != Some(OperationPermission::RemoteInvoke)
            || (
                state.remote_rpc_target != None
                    && state.remote_rpc_authority != None
                    && (
                        state.remote_rpc_target == Some(ServiceId::Service0)
                            && state.service0_lifecycle == ServiceLifecyclePhase::Running
                        || state.remote_rpc_target == Some(ServiceId::Service1)
                            && state.service1_lifecycle == ServiceLifecyclePhase::Running
                    )
            )
    }
}

#[system_spec(model_cases(system_model_cases))]
impl FrontendSpec for SystemSpec {
    type State = SystemState;
    type Action = SystemAction;

    fn frontend_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule load_config_created_default
                when matches!(action, SystemEvent::LoadConfig(true))
                    && prev.manager_phase == ManagerPhase::Booting => {
                set manager_config_loaded <= true;
                set manager_created_default <= true;
                set manager_phase <= ManagerPhase::ConfigReady;
            }

            rule load_config_existing
                when matches!(action, SystemEvent::LoadConfig(false))
                    && prev.manager_phase == ManagerPhase::Booting => {
                set manager_config_loaded <= true;
                set manager_created_default <= false;
                set manager_phase <= ManagerPhase::ConfigReady;
            }

            rule finish_restore
                when matches!(action, SystemEvent::FinishRestore)
                    && prev.manager_phase == ManagerPhase::ConfigReady => {
                set manager_phase <= ManagerPhase::Listening;
                set manager_accepts_control <= true;
                set maintenance_phase <= MaintenancePhase::Running;
            }

            rule accept_session0
                when matches!(action, SystemEvent::AcceptSession(SessionId::Session0, _))
                    && prev.manager_accepts_control
                    && prev.session0_auth == SessionAuthState::Disconnected => {
                insert active_sessions <= SessionId::Session0;
                set session0_principal <= if matches!(action, SystemEvent::AcceptSession(SessionId::Session0, TransportPrincipal::Admin)) {
                    TransportPrincipal::Admin
                } else if matches!(action, SystemEvent::AcceptSession(SessionId::Session0, TransportPrincipal::Client)) {
                    TransportPrincipal::Client
                } else if matches!(action, SystemEvent::AcceptSession(SessionId::Session0, TransportPrincipal::ServiceRunner)) {
                    TransportPrincipal::ServiceRunner
                } else {
                    TransportPrincipal::Unknown
                };
                set session0_auth <= SessionAuthState::Accepted;
                set session0_role <= SessionRole::Unknown;
                set session0_request <= SessionRequestState::Idle;
            }

            rule accept_session1
                when matches!(action, SystemEvent::AcceptSession(SessionId::Session1, _))
                    && prev.manager_accepts_control
                    && prev.session1_auth == SessionAuthState::Disconnected => {
                insert active_sessions <= SessionId::Session1;
                set session1_principal <= if matches!(action, SystemEvent::AcceptSession(SessionId::Session1, TransportPrincipal::Admin)) {
                    TransportPrincipal::Admin
                } else if matches!(action, SystemEvent::AcceptSession(SessionId::Session1, TransportPrincipal::Client)) {
                    TransportPrincipal::Client
                } else if matches!(action, SystemEvent::AcceptSession(SessionId::Session1, TransportPrincipal::ServiceRunner)) {
                    TransportPrincipal::ServiceRunner
                } else {
                    TransportPrincipal::Unknown
                };
                set session1_auth <= SessionAuthState::Accepted;
                set session1_role <= SessionRole::Unknown;
                set session1_request <= SessionRequestState::Idle;
            }

            rule authenticate_session0
                when matches!(action, SystemEvent::AuthenticateSession(SessionId::Session0, _))
                    && prev.session0_auth == SessionAuthState::Accepted => {
                set session0_auth <= SessionAuthState::Authenticated;
                set session0_role <= if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session0, SessionRole::Admin)) {
                    SessionRole::Admin
                } else if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session0, SessionRole::Client)) {
                    SessionRole::Client
                } else if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session0, SessionRole::ServiceRunner)) {
                    SessionRole::ServiceRunner
                } else {
                    SessionRole::Unknown
                };
            }

            rule authenticate_session1
                when matches!(action, SystemEvent::AuthenticateSession(SessionId::Session1, _))
                    && prev.session1_auth == SessionAuthState::Accepted => {
                set session1_auth <= SessionAuthState::Authenticated;
                set session1_role <= if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session1, SessionRole::Admin)) {
                    SessionRole::Admin
                } else if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session1, SessionRole::Client)) {
                    SessionRole::Client
                } else if matches!(action, SystemEvent::AuthenticateSession(SessionId::Session1, SessionRole::ServiceRunner)) {
                    SessionRole::ServiceRunner
                } else {
                    SessionRole::Unknown
                };
            }

            rule drain_session0
                when matches!(action, SystemEvent::DrainSession(SessionId::Session0))
                    && prev.active_sessions.contains(&SessionId::Session0) => {
                remove active_sessions <= SessionId::Session0;
                set session0_auth <= SessionAuthState::Drained;
                set session0_request <= SessionRequestState::Idle;
            }

            rule drain_session1
                when matches!(action, SystemEvent::DrainSession(SessionId::Session1))
                    && prev.active_sessions.contains(&SessionId::Session1) => {
                remove active_sessions <= SessionId::Session1;
                set session1_auth <= SessionAuthState::Drained;
                set session1_request <= SessionRequestState::Idle;
            }

            rule request_message_session0_hello
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::HelloNegotiate))
                    && prev.session0_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session0);
                set last_message <= Some(ExternalMessage::HelloNegotiate);
                set last_message_decision <= if prev.session0_principal == TransportPrincipal::Unknown {
                    AuthorizationDecision::DenyUnknownPrincipal
                } else {
                    AuthorizationDecision::Allow
                };
                set session0_request <= if prev.session0_principal == TransportPrincipal::Unknown {
                    SessionRequestState::Denied
                } else {
                    SessionRequestState::Pending
                };
            }

            rule request_message_session0_command_start
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::CommandStart))
                    && prev.session0_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session0);
                set last_message <= Some(ExternalMessage::CommandStart);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session0_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session0_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session0_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session0_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session0_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session0_logs
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::LogsRequest))
                    && prev.session0_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session0);
                set last_message <= Some(ExternalMessage::LogsRequest);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session0_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session0_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session0_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session0_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session0_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session0_rpc
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::RpcInvoke))
                    && prev.session0_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session0);
                set last_message <= Some(ExternalMessage::RpcInvoke);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session0_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session0_role == SessionRole::Admin || prev.session0_role == SessionRole::Client {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session0_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session0_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session0_role == SessionRole::Admin || prev.session0_role == SessionRole::Client {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session0_bindings
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session0, ExternalMessage::BindingsCertUpload))
                    && prev.session0_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session0);
                set last_message <= Some(ExternalMessage::BindingsCertUpload);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session0_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session0_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session0_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session0_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session0_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session0_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session1_hello
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session1, ExternalMessage::HelloNegotiate))
                    && prev.session1_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session1);
                set last_message <= Some(ExternalMessage::HelloNegotiate);
                set last_message_decision <= if prev.session1_principal == TransportPrincipal::Unknown {
                    AuthorizationDecision::DenyUnknownPrincipal
                } else {
                    AuthorizationDecision::Allow
                };
                set session1_request <= if prev.session1_principal == TransportPrincipal::Unknown {
                    SessionRequestState::Denied
                } else {
                    SessionRequestState::Pending
                };
            }

            rule request_message_session1_command_start
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session1, ExternalMessage::CommandStart))
                    && prev.session1_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session1);
                set last_message <= Some(ExternalMessage::CommandStart);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session1_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session1_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session1_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session1_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session1_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session1_logs
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session1, ExternalMessage::LogsRequest))
                    && prev.session1_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session1);
                set last_message <= Some(ExternalMessage::LogsRequest);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session1_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session1_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session1_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session1_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session1_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session1_rpc
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session1, ExternalMessage::RpcInvoke))
                    && prev.session1_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session1);
                set last_message <= Some(ExternalMessage::RpcInvoke);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session1_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session1_role == SessionRole::Admin || prev.session1_role == SessionRole::Client {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session1_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session1_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session1_role == SessionRole::Admin || prev.session1_role == SessionRole::Client {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule request_message_session1_bindings
                when matches!(action, SystemEvent::RequestMessage(SessionId::Session1, ExternalMessage::BindingsCertUpload))
                    && prev.session1_auth != SessionAuthState::Disconnected => {
                set last_message_session <= Some(SessionId::Session1);
                set last_message <= Some(ExternalMessage::BindingsCertUpload);
                set last_message_decision <= if !prev.manager_accepts_control {
                    AuthorizationDecision::DenyManagerNotListening
                } else if prev.session1_auth == SessionAuthState::Drained {
                    AuthorizationDecision::DenySessionDrained
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    AuthorizationDecision::DenySessionNotAuthenticated
                } else if prev.session1_role == SessionRole::Admin {
                    AuthorizationDecision::Allow
                } else {
                    AuthorizationDecision::DenyMessageNotAllowed
                };
                set session1_request <= if !prev.manager_accepts_control {
                    SessionRequestState::Denied
                } else if prev.session1_auth == SessionAuthState::Drained {
                    SessionRequestState::Denied
                } else if prev.session1_auth != SessionAuthState::Authenticated {
                    SessionRequestState::Denied
                } else if prev.session1_role == SessionRole::Admin {
                    SessionRequestState::Pending
                } else {
                    SessionRequestState::Denied
                };
            }

            rule complete_message_session0
                when matches!(action, SystemEvent::CompleteMessage(SessionId::Session0))
                    && prev.session0_request == SessionRequestState::Pending => {
                set session0_request <= SessionRequestState::Completed;
            }

            rule complete_message_session1
                when matches!(action, SystemEvent::CompleteMessage(SessionId::Session1))
                    && prev.session1_request == SessionRequestState::Pending => {
                set session1_request <= SessionRequestState::Completed;
            }

            rule prepare_service
                when matches!(action, SystemEvent::PrepareService(_))
                    && prev.manager_phase == ManagerPhase::Listening => {
                set service0_lifecycle <= if matches!(action, SystemEvent::PrepareService(ServiceId::Service0)) {
                    ServiceLifecyclePhase::Prepared
                } else {
                    state.service0_lifecycle
                };
                set service1_lifecycle <= if matches!(action, SystemEvent::PrepareService(ServiceId::Service1)) {
                    ServiceLifecyclePhase::Prepared
                } else {
                    state.service1_lifecycle
                };
            }

            rule commit_service
                when matches!(action, SystemEvent::CommitService(_)) => {
                set service0_lifecycle <= if matches!(action, SystemEvent::CommitService(ServiceId::Service0))
                    && prev.service0_lifecycle == ServiceLifecyclePhase::Prepared {
                    ServiceLifecyclePhase::Committed
                } else {
                    state.service0_lifecycle
                };
                set service1_lifecycle <= if matches!(action, SystemEvent::CommitService(ServiceId::Service1))
                    && prev.service1_lifecycle == ServiceLifecyclePhase::Prepared {
                    ServiceLifecyclePhase::Committed
                } else {
                    state.service1_lifecycle
                };
            }

            rule promote_service
                when matches!(action, SystemEvent::PromoteService(_)) => {
                set service0_lifecycle <= if matches!(action, SystemEvent::PromoteService(ServiceId::Service0))
                    && prev.service0_lifecycle == ServiceLifecyclePhase::Committed {
                    ServiceLifecyclePhase::Promoted
                } else {
                    state.service0_lifecycle
                };
                set service1_lifecycle <= if matches!(action, SystemEvent::PromoteService(ServiceId::Service1))
                    && prev.service1_lifecycle == ServiceLifecyclePhase::Committed {
                    ServiceLifecyclePhase::Promoted
                } else {
                    state.service1_lifecycle
                };
            }

            rule start_service
                when matches!(action, SystemEvent::StartService(_))
                    && prev.manager_phase == ManagerPhase::Listening => {
                set service0_lifecycle <= if matches!(action, SystemEvent::StartService(ServiceId::Service0))
                    && prev.service0_lifecycle == ServiceLifecyclePhase::Promoted {
                    ServiceLifecyclePhase::Running
                } else {
                    state.service0_lifecycle
                };
                set service1_lifecycle <= if matches!(action, SystemEvent::StartService(ServiceId::Service1))
                    && prev.service1_lifecycle == ServiceLifecyclePhase::Promoted {
                    ServiceLifecyclePhase::Running
                } else {
                    state.service1_lifecycle
                };
            }

            rule stop_service
                when matches!(action, SystemEvent::StopService(_)) => {
                set service0_lifecycle <= if matches!(action, SystemEvent::StopService(ServiceId::Service0))
                    && prev.service0_lifecycle == ServiceLifecyclePhase::Running {
                    ServiceLifecyclePhase::Stopping
                } else {
                    state.service0_lifecycle
                };
                set service1_lifecycle <= if matches!(action, SystemEvent::StopService(ServiceId::Service1))
                    && prev.service1_lifecycle == ServiceLifecyclePhase::Running {
                    ServiceLifecyclePhase::Stopping
                } else {
                    state.service1_lifecycle
                };
            }

            rule verify_manager_auth
                when matches!(action, SystemEvent::VerifyManagerAuth(_))
                    && prev.manager_phase == ManagerPhase::Listening => {
                set service0_manager_auth <= if matches!(action, SystemEvent::VerifyManagerAuth(ServiceId::Service0)) {
                    ManagerAuthState::Verified
                } else {
                    state.service0_manager_auth
                };
                set service1_manager_auth <= if matches!(action, SystemEvent::VerifyManagerAuth(ServiceId::Service1)) {
                    ManagerAuthState::Verified
                } else {
                    state.service1_manager_auth
                };
            }

            rule grant_binding
                when matches!(action, SystemEvent::GrantBinding(_))
                    && prev.manager_phase == ManagerPhase::Listening => {
                insert binding_grants <= if matches!(action, SystemEvent::GrantBinding(BindingGrantId::Service0ToService1ControlApi)) {
                    BindingGrantId::Service0ToService1ControlApi
                } else if matches!(action, SystemEvent::GrantBinding(BindingGrantId::Service0ToService1LogsApi)) {
                    BindingGrantId::Service0ToService1LogsApi
                } else if matches!(action, SystemEvent::GrantBinding(BindingGrantId::Service1ToService0ControlApi)) {
                    BindingGrantId::Service1ToService0ControlApi
                } else {
                    BindingGrantId::Service1ToService0LogsApi
                };
            }

            rule register_remote_authority
                when matches!(action, SystemEvent::RegisterRemoteAuthority(_))
                    && prev.manager_phase == ManagerPhase::Listening => {
                insert trusted_authorities <= if matches!(action, SystemEvent::RegisterRemoteAuthority(RemoteAuthorityId::Authority0)) {
                    RemoteAuthorityId::Authority0
                } else {
                    RemoteAuthorityId::Authority1
                };
            }

            rule start_command
                when matches!(action, SystemEvent::StartCommand(_))
                    && prev.manager_accepts_control
                    && prev.command_state == None => {
                set command_kind <= if matches!(action, SystemEvent::StartCommand(imagod_spec::command_contract::CommandKind::Deploy)) {
                    Some(imagod_spec::command_contract::CommandKind::Deploy)
                } else if matches!(action, SystemEvent::StartCommand(imagod_spec::command_contract::CommandKind::Run)) {
                    Some(imagod_spec::command_contract::CommandKind::Run)
                } else {
                    Some(imagod_spec::command_contract::CommandKind::Stop)
                };
                set command_state <= Some(imagod_spec::command_contract::CommandLifecycleState::Accepted);
                set command_cancel_requested <= false;
            }

            rule request_command_cancel
                when matches!(action, SystemEvent::RequestCommandCancel)
                    && prev.command_state == Some(imagod_spec::command_contract::CommandLifecycleState::Accepted)
                        || prev.command_state == Some(imagod_spec::command_contract::CommandLifecycleState::Running) => {
                set command_cancel_requested <= true;
                set last_operation_permission <= Some(OperationPermission::CancelCommand);
                set last_operation_decision <= AuthorizationDecision::Allow;
            }

            rule finish_command
                when matches!(action, SystemEvent::FinishCommand(_))
                    && prev.command_state != None => {
                set command_state <= if matches!(action, SystemEvent::FinishCommand(imagod_spec::CommandTerminalState::Succeeded)) {
                    Some(imagod_spec::command_contract::CommandLifecycleState::Succeeded)
                } else if matches!(action, SystemEvent::FinishCommand(imagod_spec::CommandTerminalState::Failed)) {
                    Some(imagod_spec::command_contract::CommandLifecycleState::Failed)
                } else {
                    Some(imagod_spec::command_contract::CommandLifecycleState::Canceled)
                };
            }

            rule resolve_invocation_target_allowed
                when (
                    matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, ServiceId::Service1, InterfaceId::ControlApi))
                        && prev.service0_manager_auth == ManagerAuthState::Verified
                        && prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                        && prev.service1_lifecycle == ServiceLifecyclePhase::Running
                ) || (
                    matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service1, ServiceId::Service0, InterfaceId::ControlApi))
                        && prev.service1_manager_auth == ManagerAuthState::Verified
                        && prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi)
                        && prev.service0_lifecycle == ServiceLifecyclePhase::Running
                ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, _, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::ResolveInvocationTarget);
                set last_operation_decision <= AuthorizationDecision::Allow;
                set local_rpc_target <= if matches!(action, SystemEvent::InvokeLocalRpc(_, ServiceId::Service0, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_rpc_outcome <= RpcOutcome::LocalInvoked;
            }

            rule resolve_invocation_target_denied
                when matches!(action, SystemEvent::InvokeLocalRpc(_, _, InterfaceId::ControlApi))
                    && !(
                        matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, ServiceId::Service1, InterfaceId::ControlApi))
                            && prev.service0_manager_auth == ManagerAuthState::Verified
                            && prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                            && prev.service1_lifecycle == ServiceLifecyclePhase::Running
                        || matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service1, ServiceId::Service0, InterfaceId::ControlApi))
                            && prev.service1_manager_auth == ManagerAuthState::Verified
                            && prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi)
                            && prev.service0_lifecycle == ServiceLifecyclePhase::Running
                    ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, _, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::ResolveInvocationTarget);
                set last_operation_decision <= if matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, _, _))
                    && prev.service0_manager_auth != ManagerAuthState::Verified
                    || matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service1, _, _))
                        && prev.service1_manager_auth != ManagerAuthState::Verified {
                    AuthorizationDecision::DenyManagerAuthMissing
                } else if matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service0, ServiceId::Service1, _))
                    && !prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                    || matches!(action, SystemEvent::InvokeLocalRpc(ServiceId::Service1, ServiceId::Service0, _))
                        && !prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi) {
                    AuthorizationDecision::DenyBindingMissing
                } else {
                    AuthorizationDecision::DenyTargetServiceNotRunning
                };
                set last_rpc_outcome <= RpcOutcome::LocalDenied;
            }

            rule connect_remote_allowed
                when (
                    matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0))
                        && prev.service0_manager_auth == ManagerAuthState::Verified
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                ) || (
                    matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0))
                        && prev.service1_manager_auth == ManagerAuthState::Verified
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                ) || (
                    matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority1))
                        && prev.service0_manager_auth == ManagerAuthState::Verified
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority1)
                ) || (
                    matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority1))
                        && prev.service1_manager_auth == ManagerAuthState::Verified
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority1)
                ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteConnect);
                set last_operation_decision <= AuthorizationDecision::Allow;
                set service0_remote_connection <= if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0)) {
                    Some(RemoteAuthorityId::Authority0)
                } else if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority1)) {
                    Some(RemoteAuthorityId::Authority1)
                } else {
                    state.service0_remote_connection
                };
                set service1_remote_connection <= if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0)) {
                    Some(RemoteAuthorityId::Authority0)
                } else if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority1)) {
                    Some(RemoteAuthorityId::Authority1)
                } else {
                    state.service1_remote_connection
                };
                set remote_rpc_authority <= if matches!(action, SystemEvent::ConnectRemoteRpc(_, RemoteAuthorityId::Authority0)) {
                    Some(RemoteAuthorityId::Authority0)
                } else {
                    Some(RemoteAuthorityId::Authority1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteConnected;
            }

            rule connect_remote_denied
                when matches!(action, SystemEvent::ConnectRemoteRpc(_, _))
                    && !(
                        matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0))
                            && prev.service0_manager_auth == ManagerAuthState::Verified
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                        || matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0))
                            && prev.service1_manager_auth == ManagerAuthState::Verified
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                        || matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority1))
                            && prev.service0_manager_auth == ManagerAuthState::Verified
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority1)
                        || matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority1))
                            && prev.service1_manager_auth == ManagerAuthState::Verified
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority1)
                    ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteConnect);
                set last_operation_decision <= if matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service0, _))
                    && prev.service0_manager_auth != ManagerAuthState::Verified
                    || matches!(action, SystemEvent::ConnectRemoteRpc(ServiceId::Service1, _))
                        && prev.service1_manager_auth != ManagerAuthState::Verified {
                    AuthorizationDecision::DenyManagerAuthMissing
                } else {
                    AuthorizationDecision::DenyRemoteAuthorityUnknown
                };
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule invoke_remote_allowed
                when (
                    matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0, ServiceId::Service1, InterfaceId::ControlApi))
                        && prev.service0_manager_auth == ManagerAuthState::Verified
                        && prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                        && prev.service0_remote_connection == Some(RemoteAuthorityId::Authority0)
                        && prev.service1_lifecycle == ServiceLifecyclePhase::Running
                ) || (
                    matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0, ServiceId::Service0, InterfaceId::ControlApi))
                        && prev.service1_manager_auth == ManagerAuthState::Verified
                        && prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi)
                        && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                        && prev.service1_remote_connection == Some(RemoteAuthorityId::Authority0)
                        && prev.service0_lifecycle == ServiceLifecyclePhase::Running
                ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, _, _, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteInvoke);
                set last_operation_decision <= AuthorizationDecision::Allow;
                set remote_rpc_target <= if matches!(action, SystemEvent::InvokeRemoteRpc(_, _, ServiceId::Service0, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set remote_rpc_authority <= if matches!(action, SystemEvent::InvokeRemoteRpc(_, RemoteAuthorityId::Authority0, _, _)) {
                    Some(RemoteAuthorityId::Authority0)
                } else {
                    Some(RemoteAuthorityId::Authority1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteInvoked;
            }

            rule invoke_remote_denied
                when matches!(action, SystemEvent::InvokeRemoteRpc(_, _, _, InterfaceId::ControlApi))
                    && !(
                        matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0, ServiceId::Service1, InterfaceId::ControlApi))
                            && prev.service0_manager_auth == ManagerAuthState::Verified
                            && prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                            && prev.service0_remote_connection == Some(RemoteAuthorityId::Authority0)
                            && prev.service1_lifecycle == ServiceLifecyclePhase::Running
                        || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0, ServiceId::Service0, InterfaceId::ControlApi))
                            && prev.service1_manager_auth == ManagerAuthState::Verified
                            && prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi)
                            && prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                            && prev.service1_remote_connection == Some(RemoteAuthorityId::Authority0)
                            && prev.service0_lifecycle == ServiceLifecyclePhase::Running
                    ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, _, _, _)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteInvoke);
                set last_operation_decision <= if matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, _, _, _))
                    && prev.service0_manager_auth != ManagerAuthState::Verified
                    || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, _, _, _))
                        && prev.service1_manager_auth != ManagerAuthState::Verified {
                    AuthorizationDecision::DenyManagerAuthMissing
                } else if matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, _, ServiceId::Service1, _))
                    && !prev.binding_grants.contains(&BindingGrantId::Service0ToService1ControlApi)
                    || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, _, ServiceId::Service0, _))
                        && !prev.binding_grants.contains(&BindingGrantId::Service1ToService0ControlApi) {
                    AuthorizationDecision::DenyBindingMissing
                } else if matches!(action, SystemEvent::InvokeRemoteRpc(_, RemoteAuthorityId::Authority0, _, _))
                    && !prev.trusted_authorities.contains(&RemoteAuthorityId::Authority0)
                    || matches!(action, SystemEvent::InvokeRemoteRpc(_, RemoteAuthorityId::Authority1, _, _))
                        && !prev.trusted_authorities.contains(&RemoteAuthorityId::Authority1) {
                    AuthorizationDecision::DenyRemoteAuthorityUnknown
                } else if matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0, _, _))
                    && prev.service0_remote_connection != Some(RemoteAuthorityId::Authority0)
                    || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority0, _, _))
                        && prev.service1_remote_connection != Some(RemoteAuthorityId::Authority0)
                    || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority1, _, _))
                        && prev.service0_remote_connection != Some(RemoteAuthorityId::Authority1)
                    || matches!(action, SystemEvent::InvokeRemoteRpc(ServiceId::Service1, RemoteAuthorityId::Authority1, _, _))
                        && prev.service1_remote_connection != Some(RemoteAuthorityId::Authority1) {
                    AuthorizationDecision::DenyRemoteConnectionMissing
                } else {
                    AuthorizationDecision::DenyTargetServiceNotRunning
                };
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule disconnect_remote_allowed
                when matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service0))
                    && prev.service0_remote_connection != None
                    || matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service1))
                        && prev.service1_remote_connection != None => {
                set last_operation_actor <= if matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service0)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteDisconnect);
                set last_operation_decision <= AuthorizationDecision::Allow;
                set service0_remote_connection <= if matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service0)) {
                    None
                } else {
                    state.service0_remote_connection
                };
                set service1_remote_connection <= if matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service1)) {
                    None
                } else {
                    state.service1_remote_connection
                };
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteDisconnected;
            }

            rule disconnect_remote_denied
                when matches!(action, SystemEvent::DisconnectRemoteRpc(_))
                    && !(
                        matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service0))
                            && prev.service0_remote_connection != None
                        || matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service1))
                            && prev.service1_remote_connection != None
                    ) => {
                set last_operation_actor <= if matches!(action, SystemEvent::DisconnectRemoteRpc(ServiceId::Service0)) {
                    Some(ServiceId::Service0)
                } else {
                    Some(ServiceId::Service1)
                };
                set last_operation_permission <= Some(OperationPermission::RemoteDisconnect);
                set last_operation_decision <= AuthorizationDecision::DenyRemoteConnectionMissing;
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule request_shutdown
                when matches!(action, SystemEvent::RequestShutdown)
                    && prev.manager_phase == ManagerPhase::Listening => {
                set manager_phase <= ManagerPhase::ShutdownRequested;
                set manager_shutdown_phase <= ManagerShutdownPhase::DrainingSessions;
                set manager_accepts_control <= false;
            }

            rule confirm_sessions_drained
                when matches!(action, SystemEvent::ConfirmSessionsDrained)
                    && prev.manager_shutdown_phase == ManagerShutdownPhase::DrainingSessions => {
                remove active_sessions <= SessionId::Session0;
                remove active_sessions <= SessionId::Session1;
                set session0_auth <= if prev.session0_auth == SessionAuthState::Disconnected {
                    SessionAuthState::Disconnected
                } else {
                    SessionAuthState::Drained
                };
                set session1_auth <= if prev.session1_auth == SessionAuthState::Disconnected {
                    SessionAuthState::Disconnected
                } else {
                    SessionAuthState::Drained
                };
                set session0_request <= SessionRequestState::Idle;
                set session1_request <= SessionRequestState::Idle;
                set manager_shutdown_phase <= ManagerShutdownPhase::StoppingServices;
            }

            rule confirm_services_stopped
                when matches!(action, SystemEvent::ConfirmServicesStopped)
                    && prev.manager_shutdown_phase == ManagerShutdownPhase::StoppingServices => {
                set service0_lifecycle <= if prev.service0_lifecycle == ServiceLifecyclePhase::Absent {
                    ServiceLifecyclePhase::Absent
                } else {
                    ServiceLifecyclePhase::Reaped
                };
                set service1_lifecycle <= if prev.service1_lifecycle == ServiceLifecyclePhase::Absent {
                    ServiceLifecyclePhase::Absent
                } else {
                    ServiceLifecyclePhase::Reaped
                };
                set service0_remote_connection <= None;
                set service1_remote_connection <= None;
                set local_rpc_target <= None;
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set command_kind <= None;
                set command_state <= None;
                set command_cancel_requested <= false;
                set manager_shutdown_phase <= ManagerShutdownPhase::StoppingMaintenance;
                set last_rpc_outcome <= RpcOutcome::RemoteDisconnected;
            }

            rule confirm_maintenance_stopped
                when matches!(action, SystemEvent::ConfirmMaintenanceStopped)
                    && prev.manager_shutdown_phase == ManagerShutdownPhase::StoppingMaintenance => {
                set maintenance_phase <= MaintenancePhase::Stopped;
            }

            rule complete_shutdown
                when matches!(action, SystemEvent::CompleteShutdown)
                    && prev.manager_shutdown_phase == ManagerShutdownPhase::StoppingMaintenance
                    && prev.maintenance_phase == MaintenancePhase::Stopped => {
                set manager_phase <= ManagerPhase::Stopped;
                set manager_shutdown_phase <= ManagerShutdownPhase::Completed;
                set manager_accepts_control <= false;
            }
        })
    }
}

#[cfg(not(test))]
#[nirvash_macros::formal_tests(spec = SystemSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check as checks;
    use nirvash_lower::{FrontendSpec, TemporalSpec};

    #[derive(Debug, Clone, Copy)]
    struct FocusedParitySpec;

    impl FrontendSpec for FocusedParitySpec {
        type State = SystemState;
        type Action = SystemAction;

        fn frontend_name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }

        fn initial_states(&self) -> Vec<Self::State> {
            vec![SystemSpec::new().initial_state()]
        }

        fn actions(&self) -> Vec<Self::Action> {
            vec![
                SystemAction::LoadConfig(false),
                SystemAction::FinishRestore,
                SystemAction::AcceptSession(SessionId::Session0, TransportPrincipal::Client),
                SystemAction::AuthenticateSession(SessionId::Session0, SessionRole::Client),
                SystemAction::RequestMessage(SessionId::Session0, ExternalMessage::HelloNegotiate),
                SystemAction::CompleteMessage(SessionId::Session0),
                SystemAction::PrepareService(ServiceId::Service0),
                SystemAction::CommitService(ServiceId::Service0),
                SystemAction::PromoteService(ServiceId::Service0),
                SystemAction::StartService(ServiceId::Service0),
                SystemAction::RegisterRemoteAuthority(RemoteAuthorityId::Authority0),
                SystemAction::RequestShutdown,
                SystemAction::DrainSession(SessionId::Session0),
                SystemAction::ConfirmSessionsDrained,
                SystemAction::StopService(ServiceId::Service0),
                SystemAction::ConfirmServicesStopped,
                SystemAction::ConfirmMaintenanceStopped,
                SystemAction::CompleteShutdown,
            ]
        }

        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            SystemSpec::new().transition_program()
        }
    }

    impl TemporalSpec for FocusedParitySpec {
        fn invariants(&self) -> Vec<BoolExpr<Self::State>> {
            Vec::new()
        }
    }

    fn apply_deterministic(
        spec: &SystemSpec,
        state: &SystemState,
        action: SystemAction,
    ) -> SystemState {
        let program = spec
            .transition_program()
            .expect("system transition program");
        let successors = program.successors(state, &action);
        assert_eq!(successors.len(), 1, "action should be deterministic");
        successors
            .into_iter()
            .next()
            .expect("one deterministic successor")
            .into_next()
    }

    #[test]
    fn shutdown_transition_propagates_to_sessions_services_and_rpc() {
        let spec = SystemSpec::new();
        let mut state = spec.initial_state();
        state = apply_deterministic(&spec, &state, SystemAction::LoadConfig(false));
        state = apply_deterministic(&spec, &state, SystemAction::FinishRestore);
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::AcceptSession(SessionId::Session0, TransportPrincipal::Admin),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::AuthenticateSession(SessionId::Session0, SessionRole::Admin),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::PrepareService(ServiceId::Service0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::CommitService(ServiceId::Service0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::PromoteService(ServiceId::Service0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::StartService(ServiceId::Service0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::VerifyManagerAuth(ServiceId::Service0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::RegisterRemoteAuthority(RemoteAuthorityId::Authority0),
        );
        state = apply_deterministic(
            &spec,
            &state,
            SystemAction::ConnectRemoteRpc(ServiceId::Service0, RemoteAuthorityId::Authority0),
        );
        state = apply_deterministic(&spec, &state, SystemAction::RequestShutdown);
        state = apply_deterministic(&spec, &state, SystemAction::ConfirmSessionsDrained);
        state = apply_deterministic(&spec, &state, SystemAction::ConfirmServicesStopped);
        state = apply_deterministic(&spec, &state, SystemAction::ConfirmMaintenanceStopped);
        state = apply_deterministic(&spec, &state, SystemAction::CompleteShutdown);

        assert_eq!(state.manager_phase, ManagerPhase::Stopped);
        assert_eq!(
            state.manager_shutdown_phase,
            ManagerShutdownPhase::Completed
        );
        assert!(!state.active_sessions.contains(&SessionId::Session0));
        assert!(!state.active_sessions.contains(&SessionId::Session1));
        assert_eq!(state.session0_auth, SessionAuthState::Drained);
        assert_eq!(state.service0_lifecycle, ServiceLifecyclePhase::Reaped);
        assert_eq!(state.remote_rpc_authority, None);
        assert_eq!(state.remote_rpc_target, None);
    }

    #[test]
    fn system_event_doc_presentation_uses_canonical_actors() {
        let request = nirvash::describe_doc_graph_action(&SystemAction::RequestMessage(
            SessionId::Session0,
            ExternalMessage::RpcInvoke,
        ));
        assert_eq!(request.label, "rpc.invoke");
        assert_eq!(
            request.interaction_steps,
            vec![nirvash::DocGraphInteractionStep::between(
                "Session0",
                "Manager",
                "rpc.invoke",
            )]
        );

        let local_rpc = nirvash::describe_doc_graph_action(&SystemAction::InvokeLocalRpc(
            ServiceId::Service0,
            ServiceId::Service1,
            InterfaceId::ControlApi,
        ));
        assert_eq!(local_rpc.label, "invoke local control-api");
        assert_eq!(
            local_rpc.interaction_steps,
            vec![nirvash::DocGraphInteractionStep::between(
                "Service0",
                "Service1",
                "invoke local control-api",
            )]
        );

        let remote_rpc = nirvash::describe_doc_graph_action(&SystemAction::InvokeRemoteRpc(
            ServiceId::Service0,
            RemoteAuthorityId::Authority0,
            ServiceId::Service1,
            InterfaceId::LogsApi,
        ));
        assert_eq!(remote_rpc.label, "invoke remote logs-api");
        assert_eq!(
            remote_rpc.interaction_steps,
            vec![nirvash::DocGraphInteractionStep::between(
                "Service0",
                "Authority0",
                "invoke remote logs-api",
            )]
        );
        assert!(remote_rpc.process_steps.iter().any(|step| {
            step.actor.as_deref() == Some("Service1")
                && matches!(step.kind, nirvash::DocGraphProcessKind::Do)
                && step.label == "handle logs-api"
        }));

        let shutdown = nirvash::describe_doc_graph_action(&SystemAction::RequestShutdown);
        assert_eq!(shutdown.label, "request shutdown");
        assert!(shutdown.interaction_steps.is_empty());
        assert_eq!(
            shutdown.process_steps,
            vec![nirvash::DocGraphProcessStep::for_actor(
                "Manager",
                nirvash::DocGraphProcessKind::Do,
                "request shutdown",
            )]
        );
    }

    #[test]
    fn view_model_cases_use_projected_doc_state() {
        let mut state = SystemSpec::new().initial_state();
        state.manager_phase = ManagerPhase::Listening;
        state.manager_accepts_control = true;
        state.session0_auth = SessionAuthState::Authenticated;
        state.service0_lifecycle = ServiceLifecyclePhase::Running;
        state.last_message_decision = AuthorizationDecision::Allow;
        state.last_operation_decision = AuthorizationDecision::Allow;

        let cases = [
            (
                "Manager View",
                "ManagerViewState",
                crate::manager_view::model_cases(),
            ),
            (
                "Control View",
                "ControlViewState",
                crate::control_view::model_cases(),
            ),
            (
                "Service View",
                "ServiceViewState",
                crate::service_view::model_cases(),
            ),
            (
                "Operation View",
                "OperationViewState",
                crate::operation_view::model_cases(),
            ),
            (
                "Authorization View",
                "AuthzViewState",
                crate::authz_view::model_cases(),
            ),
        ];

        for (surface, projection, cases) in cases {
            let case = cases
                .into_iter()
                .next()
                .expect("view should expose at least one model case");
            assert_eq!(case.doc_surface(), Some(surface));
            let projection_fn = case
                .doc_state_projection()
                .expect("view should expose doc projection");
            assert_eq!(projection_fn.label, projection);
            let projected = projection_fn.summarize(&state);
            assert!(projected.full.contains(projection));
            assert!(!projected.full.contains("SystemStateFragment"));
        }
    }

    #[test]
    fn explicit_and_symbolic_focused_cases_agree() {
        let spec = FocusedParitySpec;
        let lowered = crate::lowered_spec(&spec);

        let explicit_snapshot = checks::ExplicitModelChecker::new(&lowered)
            .check_invariants()
            .expect("explicit focused invariant check");
        let symbolic_snapshot = checks::SymbolicModelChecker::new(&lowered)
            .check_invariants()
            .expect("symbolic focused invariant check");

        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }
}
