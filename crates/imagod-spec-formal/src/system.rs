use nirvash::{BoolExpr, Fairness, Ltl, RelSet, TransitionProgram};
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, RelationalState,
    SymbolicEncoding as FormalSymbolicEncoding, action_constraint, fairness, invariant,
    nirvash_expr, nirvash_step_expr, nirvash_transition_program, property, state_constraint,
    system_spec,
};

use crate::{
    CommandErrorKind, CommandKind, CommandLifecycleState,
    atoms::{RemoteAuthorityAtom, ServiceAtom, SessionAtom, SessionRoleAtom, StreamAtom},
    bounds::{MAX_LASSO_DEPTH, MaintenanceTicks, doc_cap_focus, doc_cap_surface},
    control_plane::{ControlPlaneAction, RequestPhase},
    manager_plane::{ManagerPhase, ManagerPlaneAction},
    operation_plane::{OperationPlaneAction, RpcOutcome},
    service_plane::{ServiceLifecyclePhase, ServicePlaneAction},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
pub enum SystemAction {
    Manager(ManagerPlaneAction),
    Control(ControlPlaneAction),
    Service(ServicePlaneAction),
    Operation(OperationPlaneAction),
}

#[derive(
    Debug, Clone, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding, RelationalState,
)]
#[finite_model_domain(custom)]
pub struct SystemState {
    pub config_loaded: bool,
    pub created_default: bool,
    pub manager_phase: ManagerPhase,
    pub accepts_control: bool,
    pub shutdown_started: bool,
    pub services_drained: bool,
    pub maintenance_stopped: bool,
    pub maintenance_ticks: MaintenanceTicks,
    pub active_sessions: RelSet<SessionAtom>,
    pub admin_sessions: RelSet<SessionAtom>,
    pub client_sessions: RelSet<SessionAtom>,
    pub active_streams: RelSet<StreamAtom>,
    pub authority_uploaded: bool,
    pub last_role: Option<SessionRoleAtom>,
    pub request_phase: RequestPhase,
    pub service0: ServiceLifecyclePhase,
    pub service1: ServiceLifecyclePhase,
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

nirvash::finite_model_domain_spec!(
    SystemStateFiniteModelDomainSpec for SystemState,
    representatives = system_state_domain()
);

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpec;

impl SystemSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SystemState {
        SystemState {
            config_loaded: false,
            created_default: false,
            manager_phase: ManagerPhase::Booting,
            accepts_control: false,
            shutdown_started: false,
            services_drained: false,
            maintenance_stopped: false,
            maintenance_ticks: MaintenanceTicks::new(0).expect("within bounds"),
            active_sessions: RelSet::empty(),
            admin_sessions: RelSet::empty(),
            client_sessions: RelSet::empty(),
            active_streams: RelSet::empty(),
            authority_uploaded: false,
            last_role: None,
            request_phase: RequestPhase::Idle,
            service0: ServiceLifecyclePhase::Absent,
            service1: ServiceLifecyclePhase::Absent,
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

    fn actions_union(&self) -> Vec<SystemAction> {
        let mut actions = Vec::new();
        actions.extend(
            <ManagerPlaneAction as nirvash::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAction::Manager),
        );
        actions.extend(
            <ControlPlaneAction as nirvash::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAction::Control),
        );
        actions.extend(
            <ServicePlaneAction as nirvash::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAction::Service),
        );
        actions.extend(
            <OperationPlaneAction as nirvash::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAction::Operation),
        );
        actions
    }
}

fn system_state_domain() -> Vec<SystemState> {
    let spec = SystemSpec::new();
    let initial = spec.initial_state();

    let mut listening = initial.clone();
    listening.config_loaded = true;
    listening.manager_phase = ManagerPhase::Listening;
    listening.accepts_control = true;

    let mut dual_sessions = listening.clone();
    dual_sessions.active_sessions.insert(SessionAtom::Session0);
    dual_sessions.active_sessions.insert(SessionAtom::Session1);
    dual_sessions.admin_sessions.insert(SessionAtom::Session0);
    dual_sessions.client_sessions.insert(SessionAtom::Session1);
    dual_sessions.active_streams.insert(StreamAtom::Stream0);
    dual_sessions.active_streams.insert(StreamAtom::Stream1);
    dual_sessions.last_role = Some(SessionRoleAtom::Client);
    dual_sessions.request_phase = RequestPhase::Pending;

    let mut service_running = listening.clone();
    service_running.service0 = ServiceLifecyclePhase::Running;
    service_running.service1 = ServiceLifecyclePhase::Promoted;
    service_running.bound_services.insert(ServiceAtom::Service0);
    service_running.command_kind = Some(CommandKind::Run);
    service_running.command_state = Some(CommandLifecycleState::Running);

    let mut local_rpc = service_running.clone();
    local_rpc.local_rpc_target = Some(ServiceAtom::Service0);

    let mut remote_rpc = service_running.clone();
    remote_rpc
        .remote_connections
        .insert(RemoteAuthorityAtom::Edge0);
    remote_rpc.remote_rpc_target = Some(ServiceAtom::Service0);
    remote_rpc.remote_rpc_authority = Some(RemoteAuthorityAtom::Edge0);
    remote_rpc.last_rpc_outcome = RpcOutcome::RemoteConnected;

    let mut shutdown = service_running.clone();
    shutdown.manager_phase = ManagerPhase::ShutdownRequested;
    shutdown.accepts_control = false;
    shutdown.shutdown_started = true;
    shutdown.request_phase = RequestPhase::Idle;
    shutdown.active_streams = RelSet::empty();
    shutdown.command_state = None;
    shutdown.command_kind = None;
    shutdown.cancel_requested = false;

    let mut stopped = shutdown.clone();
    stopped.services_drained = true;
    stopped.maintenance_stopped = true;
    stopped.manager_phase = ManagerPhase::Stopped;
    stopped.service0 = ServiceLifecyclePhase::Reaped;
    stopped.remote_connections = RelSet::empty();
    stopped.remote_rpc_target = None;
    stopped.remote_rpc_authority = None;
    stopped.local_rpc_target = None;

    vec![
        initial,
        listening,
        dual_sessions,
        service_running,
        local_rpc,
        remote_rpc,
        shutdown,
        stopped,
    ]
}

fn system_model_cases() -> Vec<ModelInstance<SystemState, SystemAction>> {
    vec![
        ModelInstance::new("explicit_control_service_surface")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Explicit),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_surface())
            .with_check_deadlocks(false),
        ModelInstance::new("explicit_control_rpc_focus")
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
        ModelInstance::new("symbolic_control_rpc_focus")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::reachable_graph()
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
        ModelInstance::new("symbolic_shutdown_progress")
            .with_checker_config(nirvash::ModelCheckConfig {
                backend: Some(nirvash::ModelBackend::Symbolic),
                ..nirvash::ModelCheckConfig::bounded_lasso(MAX_LASSO_DEPTH)
            })
            .with_doc_checker_config(doc_cap_focus())
            .with_check_deadlocks(false),
    ]
}

#[action_constraint(
    SystemSpec,
    cases("explicit_control_service_surface", "explicit_shutdown_progress")
)]
fn explicit_surface_actions() -> nirvash::StepExpr<SystemState, SystemAction> {
    nirvash_step_expr! { explicit_surface_actions(_prev, action, _next) =>
        matches!(action, SystemAction::Manager(_))
            || matches!(
                action,
                SystemAction::Control(ControlPlaneAction::AcceptSession(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::AcceptSession(SessionAtom::Session1))
                    | SystemAction::Control(ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::AuthenticateClient(SessionAtom::Session1))
                    | SystemAction::Control(ControlPlaneAction::OpenRequest(
                        StreamAtom::Stream0,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::CompleteResponse(
                        StreamAtom::Stream0,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::CloseStream(StreamAtom::Stream0))
                    | SystemAction::Control(ControlPlaneAction::OpenRequest(
                        StreamAtom::Stream1,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::StartLogFollow(StreamAtom::Stream1))
                    | SystemAction::Control(ControlPlaneAction::FinishLogFollow(StreamAtom::Stream1))
                    | SystemAction::Control(ControlPlaneAction::UploadAuthority(SessionAtom::Session1))
            )
            || matches!(
                action,
                SystemAction::Service(ServicePlaneAction::UploadArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::CommitArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::PromoteArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::PrepareService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::StartService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::StopService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::ReapService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::UploadArtifact(ServiceAtom::Service1))
                    | SystemAction::Service(ServicePlaneAction::CommitArtifact(ServiceAtom::Service1))
                    | SystemAction::Service(ServicePlaneAction::PromoteArtifact(ServiceAtom::Service1))
                    | SystemAction::Service(ServicePlaneAction::TriggerRollback(ServiceAtom::Service1))
                    | SystemAction::Service(ServicePlaneAction::FinishRollback(ServiceAtom::Service1))
            )
            || matches!(
                action,
                SystemAction::Operation(OperationPlaneAction::GrantBinding(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::GrantBinding(ServiceAtom::Service1))
                    | SystemAction::Operation(OperationPlaneAction::StartCommand(CommandKind::Run))
                    | SystemAction::Operation(OperationPlaneAction::MarkCommandRunning)
                    | SystemAction::Operation(OperationPlaneAction::FinishCommandSucceeded)
                    | SystemAction::Operation(OperationPlaneAction::ClearCommandSlot)
                    | SystemAction::Operation(OperationPlaneAction::StartLocalRpc(
                        ServiceAtom::Service0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::CompleteLocalRpc(
                        ServiceAtom::Service0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
            )
    }
}

#[state_constraint(
    SystemSpec,
    cases(
        "explicit_control_rpc_focus",
        "symbolic_control_rpc_focus",
        "symbolic_shutdown_progress"
    )
)]
fn symbolic_focus_state() -> BoolExpr<SystemState> {
    nirvash_expr! { symbolic_focus_state(state) =>
        !state.active_sessions.contains(&SessionAtom::Session1)
            && !state.admin_sessions.contains(&SessionAtom::Session1)
            && !state.client_sessions.contains(&SessionAtom::Session1)
            && !state.active_streams.contains(&StreamAtom::Stream1)
            && !state.bound_services.contains(&ServiceAtom::Service1)
            && !state.remote_connections.contains(&RemoteAuthorityAtom::Edge1)
            && state.service1 == ServiceLifecyclePhase::Absent
            && state.local_rpc_target != Some(ServiceAtom::Service1)
            && state.remote_rpc_target != Some(ServiceAtom::Service1)
            && state.remote_rpc_authority != Some(RemoteAuthorityAtom::Edge1)
    }
}

#[action_constraint(
    SystemSpec,
    cases(
        "explicit_control_rpc_focus",
        "symbolic_control_rpc_focus",
        "symbolic_shutdown_progress"
    )
)]
fn symbolic_focus_actions() -> nirvash::StepExpr<SystemState, SystemAction> {
    nirvash_step_expr! { symbolic_focus_actions(_prev, action, _next) =>
        matches!(action, SystemAction::Manager(_))
            || matches!(
                action,
                SystemAction::Control(ControlPlaneAction::AcceptSession(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::AuthenticateClient(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::OpenRequest(
                        StreamAtom::Stream0,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::CompleteResponse(
                        StreamAtom::Stream0,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::RejectRequest(
                        StreamAtom::Stream0,
                        crate::RequestKindAtom::ServicesList
                    ))
                    | SystemAction::Control(ControlPlaneAction::StartLogFollow(StreamAtom::Stream0))
                    | SystemAction::Control(ControlPlaneAction::FinishLogFollow(StreamAtom::Stream0))
                    | SystemAction::Control(ControlPlaneAction::UploadAuthority(SessionAtom::Session0))
                    | SystemAction::Control(ControlPlaneAction::CloseStream(StreamAtom::Stream0))
                    | SystemAction::Control(ControlPlaneAction::DrainSession(SessionAtom::Session0))
            )
            || matches!(
                action,
                SystemAction::Service(ServicePlaneAction::UploadArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::CommitArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::PromoteArtifact(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::PrepareService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::StartService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::StopService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::ReapService(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::TriggerRollback(ServiceAtom::Service0))
                    | SystemAction::Service(ServicePlaneAction::FinishRollback(ServiceAtom::Service0))
            )
            || matches!(
                action,
                SystemAction::Operation(OperationPlaneAction::GrantBinding(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::RevokeBinding(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::StartCommand(_))
                    | SystemAction::Operation(OperationPlaneAction::MarkCommandRunning)
                    | SystemAction::Operation(OperationPlaneAction::RequestCommandCancel)
                    | SystemAction::Operation(OperationPlaneAction::FinishCommandSucceeded)
                    | SystemAction::Operation(OperationPlaneAction::FinishCommandFailed(
                        CommandErrorKind::Internal
                    ))
                    | SystemAction::Operation(OperationPlaneAction::FinishCommandCanceled)
                    | SystemAction::Operation(OperationPlaneAction::ClearCommandSlot)
                    | SystemAction::Operation(OperationPlaneAction::StartLocalRpc(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service0))
                    | SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::DenyRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                    | SystemAction::Operation(OperationPlaneAction::DisconnectRemote(
                        RemoteAuthorityAtom::Edge0
                    ))
            )
    }
}

#[invariant(SystemSpec)]
fn control_and_operation_require_manager_uptime() -> BoolExpr<SystemState> {
    nirvash_expr! { control_and_operation_require_manager_uptime(state) =>
        !(
            state.active_sessions.some()
                || state.active_streams.some()
                || state.command_state.is_some()
                || state.local_rpc_target.is_some()
                || state.remote_rpc_target.is_some()
        )
            || matches!(
                state.manager_phase,
                ManagerPhase::Listening | ManagerPhase::Maintenance | ManagerPhase::ShutdownRequested
            )
    }
}

#[invariant(SystemSpec)]
fn stopped_manager_is_quiescent() -> BoolExpr<SystemState> {
    nirvash_expr! { stopped_manager_is_quiescent(state) =>
        !matches!(state.manager_phase, ManagerPhase::Stopped)
            || (
                !state.accepts_control
                    && state.active_streams.no()
                    && state.command_state.is_none()
                    && state.local_rpc_target.is_none()
                    && state.remote_rpc_target.is_none()
            )
    }
}

#[invariant(SystemSpec)]
fn rpc_target_requires_binding_and_running_service() -> BoolExpr<SystemState> {
    nirvash_expr! { rpc_target_requires_binding_and_running_service(state) =>
        (state.local_rpc_target != Some(ServiceAtom::Service0)
            || (state.bound_services.contains(&ServiceAtom::Service0)
                && matches!(state.service0, ServiceLifecyclePhase::Running)))
            && (state.local_rpc_target != Some(ServiceAtom::Service1)
                || (state.bound_services.contains(&ServiceAtom::Service1)
                    && matches!(state.service1, ServiceLifecyclePhase::Running)))
            && (state.remote_rpc_target != Some(ServiceAtom::Service0)
                || (state.bound_services.contains(&ServiceAtom::Service0)
                    && matches!(state.service0, ServiceLifecyclePhase::Running)))
            && (state.remote_rpc_target != Some(ServiceAtom::Service1)
                || (state.bound_services.contains(&ServiceAtom::Service1)
                    && matches!(state.service1, ServiceLifecyclePhase::Running)))
    }
}

#[invariant(SystemSpec)]
fn shutdown_stops_accepting_new_control() -> BoolExpr<SystemState> {
    nirvash_expr! { shutdown_stops_accepting_new_control(state) =>
        !state.shutdown_started || !state.accepts_control
    }
}

#[property(SystemSpec)]
fn shutdown_started_leads_to_stopped() -> Ltl<SystemState, SystemAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { shutdown_started(state) => state.shutdown_started }),
        Ltl::pred(
            nirvash_expr! { stopped(state) => matches!(state.manager_phase, ManagerPhase::Stopped) },
        ),
    )
}

#[property(SystemSpec)]
fn active_rpc_leads_to_quiescence() -> Ltl<SystemState, SystemAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { active_rpc(state) =>
            state.local_rpc_target.is_some() || state.remote_rpc_target.is_some()
        }),
        Ltl::pred(nirvash_expr! { quiescent_rpc(state) =>
            state.local_rpc_target.is_none() && state.remote_rpc_target.is_none()
        }),
    )
}

#[fairness(SystemSpec)]
fn shutdown_finish_progress() -> Fairness<SystemState, SystemAction> {
    Fairness::weak(
        nirvash_step_expr! { shutdown_finish_progress(prev, action, next) =>
            matches!(prev.manager_phase, ManagerPhase::ShutdownRequested)
                && prev.services_drained
                && prev.maintenance_stopped
                && matches!(action, SystemAction::Manager(ManagerPlaneAction::FinishShutdown))
                && matches!(next.manager_phase, ManagerPhase::Stopped)
        },
    )
}

#[fairness(SystemSpec)]
fn rpc_progress() -> Fairness<SystemState, SystemAction> {
    Fairness::weak(nirvash_step_expr! { rpc_progress(prev, action, next) =>
        (prev.local_rpc_target.is_some()
            && matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteLocalRpc(_)))
            && next.local_rpc_target.is_none())
        || (prev.remote_rpc_target.is_some()
            && matches!(
                action,
                SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(_, _))
                    | SystemAction::Operation(OperationPlaneAction::DisconnectRemote(_))
            )
            && next.remote_rpc_target.is_none())
    })
}

#[system_spec(
    model_cases(system_model_cases),
    subsystems(
        crate::manager_plane::ManagerPlaneSpec,
        crate::control_plane::ControlPlaneSpec,
        crate::service_plane::ServicePlaneSpec,
        crate::operation_plane::OperationPlaneSpec
    )
)]
impl FrontendSpec for SystemSpec {
    type State = SystemState;
    type Action = SystemAction;

    fn frontend_name(&self) -> &'static str {
        "system"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        self.actions_union()
    }

    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule load_existing_config when matches!(action, SystemAction::Manager(ManagerPlaneAction::LoadExistingConfig))
                && matches!(prev.manager_phase, ManagerPhase::Booting) => {
                set config_loaded <= true;
                set created_default <= false;
                set manager_phase <= ManagerPhase::ConfigReady;
            }

            rule create_default_config when matches!(action, SystemAction::Manager(ManagerPlaneAction::CreateDefaultConfig))
                && matches!(prev.manager_phase, ManagerPhase::Booting) => {
                set config_loaded <= true;
                set created_default <= true;
                set manager_phase <= ManagerPhase::ConfigReady;
            }

            rule start_restore when matches!(action, SystemAction::Manager(ManagerPlaneAction::StartRestore))
                && matches!(prev.manager_phase, ManagerPhase::ConfigReady)
                && prev.config_loaded => {
                set manager_phase <= ManagerPhase::Restoring;
            }

            rule finish_restore when matches!(action, SystemAction::Manager(ManagerPlaneAction::FinishRestore))
                && matches!(prev.manager_phase, ManagerPhase::Restoring) => {
                set manager_phase <= ManagerPhase::Listening;
                set accepts_control <= true;
            }

            rule tick_maintenance when matches!(action, SystemAction::Manager(ManagerPlaneAction::TickMaintenance))
                && matches!(prev.manager_phase, ManagerPhase::Listening | ManagerPhase::Maintenance)
                && !prev.shutdown_started
                && !prev.maintenance_ticks.is_max() => {
                set manager_phase <= ManagerPhase::Maintenance;
                set maintenance_ticks <= prev.maintenance_ticks.saturating_inc();
            }

            rule begin_shutdown when matches!(action, SystemAction::Manager(ManagerPlaneAction::BeginShutdown))
                && matches!(prev.manager_phase, ManagerPhase::Listening | ManagerPhase::Maintenance) => {
                set manager_phase <= ManagerPhase::ShutdownRequested;
                set accepts_control <= false;
                set shutdown_started <= true;
            }

            rule drain_services when matches!(action, SystemAction::Manager(ManagerPlaneAction::DrainServices))
                && matches!(prev.manager_phase, ManagerPhase::ShutdownRequested)
                && !matches!(prev.service0, ServiceLifecyclePhase::Running | ServiceLifecyclePhase::Stopping)
                && !matches!(prev.service1, ServiceLifecyclePhase::Running | ServiceLifecyclePhase::Stopping) => {
                set services_drained <= true;
            }

            rule stop_maintenance when matches!(action, SystemAction::Manager(ManagerPlaneAction::StopMaintenance))
                && matches!(prev.manager_phase, ManagerPhase::ShutdownRequested) => {
                set maintenance_stopped <= true;
            }

            rule finish_shutdown when matches!(action, SystemAction::Manager(ManagerPlaneAction::FinishShutdown))
                && matches!(prev.manager_phase, ManagerPhase::ShutdownRequested)
                && prev.services_drained
                && prev.maintenance_stopped => {
                set manager_phase <= ManagerPhase::Stopped;
            }

            rule accept_session0 when matches!(action, SystemAction::Control(ControlPlaneAction::AcceptSession(SessionAtom::Session0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && !prev.active_sessions.contains(&SessionAtom::Session0) => {
                insert active_sessions <= SessionAtom::Session0;
                set request_phase <= RequestPhase::Idle;
            }

            rule accept_session1 when matches!(action, SystemAction::Control(ControlPlaneAction::AcceptSession(SessionAtom::Session1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && !prev.active_sessions.contains(&SessionAtom::Session1) => {
                insert active_sessions <= SessionAtom::Session1;
                set request_phase <= RequestPhase::Idle;
            }

            rule authenticate_admin0 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session0)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session0) => {
                insert admin_sessions <= SessionAtom::Session0;
                remove client_sessions <= SessionAtom::Session0;
                set last_role <= Some(SessionRoleAtom::Admin);
            }

            rule authenticate_admin1 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateAdmin(SessionAtom::Session1)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session1) => {
                insert admin_sessions <= SessionAtom::Session1;
                remove client_sessions <= SessionAtom::Session1;
                set last_role <= Some(SessionRoleAtom::Admin);
            }

            rule authenticate_client0 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateClient(SessionAtom::Session0)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session0) => {
                insert client_sessions <= SessionAtom::Session0;
                remove admin_sessions <= SessionAtom::Session0;
                set last_role <= Some(SessionRoleAtom::Client);
            }

            rule authenticate_client1 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateClient(SessionAtom::Session1)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session1) => {
                insert client_sessions <= SessionAtom::Session1;
                remove admin_sessions <= SessionAtom::Session1;
                set last_role <= Some(SessionRoleAtom::Client);
            }

            rule authenticate_unknown0 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session0)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session0) => {
                remove admin_sessions <= SessionAtom::Session0;
                remove client_sessions <= SessionAtom::Session0;
                set last_role <= Some(SessionRoleAtom::Unknown);
            }

            rule authenticate_unknown1 when matches!(action, SystemAction::Control(ControlPlaneAction::AuthenticateUnknown(SessionAtom::Session1)))
                && prev.accepts_control
                && prev.active_sessions.contains(&SessionAtom::Session1) => {
                remove admin_sessions <= SessionAtom::Session1;
                remove client_sessions <= SessionAtom::Session1;
                set last_role <= Some(SessionRoleAtom::Unknown);
            }

            rule open_request0 when matches!(action, SystemAction::Control(ControlPlaneAction::OpenRequest(StreamAtom::Stream0, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && !prev.active_streams.contains(&StreamAtom::Stream0) => {
                insert active_streams <= StreamAtom::Stream0;
                set request_phase <= RequestPhase::Pending;
            }

            rule open_request1 when matches!(action, SystemAction::Control(ControlPlaneAction::OpenRequest(StreamAtom::Stream1, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && !prev.active_streams.contains(&StreamAtom::Stream1) => {
                insert active_streams <= StreamAtom::Stream1;
                set request_phase <= RequestPhase::Pending;
            }

            rule complete_response0 when matches!(action, SystemAction::Control(ControlPlaneAction::CompleteResponse(StreamAtom::Stream0, _)))
                && prev.active_streams.contains(&StreamAtom::Stream0)
                && matches!(prev.request_phase, RequestPhase::Pending) => {
                set request_phase <= RequestPhase::Responded;
            }

            rule complete_response1 when matches!(action, SystemAction::Control(ControlPlaneAction::CompleteResponse(StreamAtom::Stream1, _)))
                && prev.active_streams.contains(&StreamAtom::Stream1)
                && matches!(prev.request_phase, RequestPhase::Pending) => {
                set request_phase <= RequestPhase::Responded;
            }

            rule reject_request0 when matches!(action, SystemAction::Control(ControlPlaneAction::RejectRequest(StreamAtom::Stream0, _)))
                && prev.active_streams.contains(&StreamAtom::Stream0)
                && matches!(prev.request_phase, RequestPhase::Pending) => {
                set request_phase <= RequestPhase::Rejected;
            }

            rule reject_request1 when matches!(action, SystemAction::Control(ControlPlaneAction::RejectRequest(StreamAtom::Stream1, _)))
                && prev.active_streams.contains(&StreamAtom::Stream1)
                && matches!(prev.request_phase, RequestPhase::Pending) => {
                set request_phase <= RequestPhase::Rejected;
            }

            rule start_log_follow0 when matches!(action, SystemAction::Control(ControlPlaneAction::StartLogFollow(StreamAtom::Stream0)))
                && prev.active_streams.contains(&StreamAtom::Stream0)
                && !prev.shutdown_started => {
                set request_phase <= RequestPhase::FollowingLogs0;
            }

            rule start_log_follow1 when matches!(action, SystemAction::Control(ControlPlaneAction::StartLogFollow(StreamAtom::Stream1)))
                && prev.active_streams.contains(&StreamAtom::Stream1)
                && !prev.shutdown_started => {
                set request_phase <= RequestPhase::FollowingLogs1;
            }

            rule finish_log_follow0 when matches!(action, SystemAction::Control(ControlPlaneAction::FinishLogFollow(StreamAtom::Stream0)))
                && matches!(prev.request_phase, RequestPhase::FollowingLogs0) => {
                remove active_streams <= StreamAtom::Stream0;
                set request_phase <= RequestPhase::Idle;
            }

            rule finish_log_follow1 when matches!(action, SystemAction::Control(ControlPlaneAction::FinishLogFollow(StreamAtom::Stream1)))
                && matches!(prev.request_phase, RequestPhase::FollowingLogs1) => {
                remove active_streams <= StreamAtom::Stream1;
                set request_phase <= RequestPhase::Idle;
            }

            rule upload_authority0 when matches!(action, SystemAction::Control(ControlPlaneAction::UploadAuthority(SessionAtom::Session0)))
                && prev.accepts_control
                && prev.client_sessions.contains(&SessionAtom::Session0) => {
                set authority_uploaded <= true;
            }

            rule upload_authority1 when matches!(action, SystemAction::Control(ControlPlaneAction::UploadAuthority(SessionAtom::Session1)))
                && prev.accepts_control
                && prev.client_sessions.contains(&SessionAtom::Session1) => {
                set authority_uploaded <= true;
            }

            rule close_stream0 when matches!(action, SystemAction::Control(ControlPlaneAction::CloseStream(StreamAtom::Stream0)))
                && prev.active_streams.contains(&StreamAtom::Stream0) => {
                remove active_streams <= StreamAtom::Stream0;
                set request_phase <= RequestPhase::Idle;
            }

            rule close_stream1 when matches!(action, SystemAction::Control(ControlPlaneAction::CloseStream(StreamAtom::Stream1)))
                && prev.active_streams.contains(&StreamAtom::Stream1) => {
                remove active_streams <= StreamAtom::Stream1;
                set request_phase <= RequestPhase::Idle;
            }

            rule drain_session0 when matches!(action, SystemAction::Control(ControlPlaneAction::DrainSession(SessionAtom::Session0)))
                && prev.active_sessions.contains(&SessionAtom::Session0) => {
                remove active_sessions <= SessionAtom::Session0;
                remove admin_sessions <= SessionAtom::Session0;
                remove client_sessions <= SessionAtom::Session0;
                set request_phase <= RequestPhase::Idle;
            }

            rule drain_session1 when matches!(action, SystemAction::Control(ControlPlaneAction::DrainSession(SessionAtom::Session1)))
                && prev.active_sessions.contains(&SessionAtom::Session1) => {
                remove active_sessions <= SessionAtom::Session1;
                remove admin_sessions <= SessionAtom::Session1;
                remove client_sessions <= SessionAtom::Session1;
                set request_phase <= RequestPhase::Idle;
            }

            rule upload_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::UploadArtifact(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(
                    prev.service0,
                    ServiceLifecyclePhase::Absent | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped
                ) => {
                set service0 <= ServiceLifecyclePhase::Uploaded;
            }

            rule upload_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::UploadArtifact(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(
                    prev.service1,
                    ServiceLifecyclePhase::Absent | ServiceLifecyclePhase::RolledBack | ServiceLifecyclePhase::Reaped
                ) => {
                set service1 <= ServiceLifecyclePhase::Uploaded;
            }

            rule commit_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::CommitArtifact(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service0, ServiceLifecyclePhase::Uploaded) => {
                set service0 <= ServiceLifecyclePhase::Committed;
            }

            rule commit_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::CommitArtifact(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service1, ServiceLifecyclePhase::Uploaded) => {
                set service1 <= ServiceLifecyclePhase::Committed;
            }

            rule promote_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::PromoteArtifact(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service0, ServiceLifecyclePhase::Committed) => {
                set service0 <= ServiceLifecyclePhase::Promoted;
            }

            rule promote_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::PromoteArtifact(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service1, ServiceLifecyclePhase::Committed) => {
                set service1 <= ServiceLifecyclePhase::Promoted;
            }

            rule prepare_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::PrepareService(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service0, ServiceLifecyclePhase::Promoted) => {
                set service0 <= ServiceLifecyclePhase::Ready;
            }

            rule prepare_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::PrepareService(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && matches!(prev.service1, ServiceLifecyclePhase::Promoted) => {
                set service1 <= ServiceLifecyclePhase::Ready;
            }

            rule start_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::StartService(ServiceAtom::Service0)))
                && !prev.shutdown_started
                && matches!(prev.service0, ServiceLifecyclePhase::Ready) => {
                set service0 <= ServiceLifecyclePhase::Running;
            }

            rule start_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::StartService(ServiceAtom::Service1)))
                && !prev.shutdown_started
                && matches!(prev.service1, ServiceLifecyclePhase::Ready) => {
                set service1 <= ServiceLifecyclePhase::Running;
            }

            rule stop_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::StopService(ServiceAtom::Service0)))
                && matches!(prev.service0, ServiceLifecyclePhase::Running) => {
                set service0 <= ServiceLifecyclePhase::Stopping;
            }

            rule stop_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::StopService(ServiceAtom::Service1)))
                && matches!(prev.service1, ServiceLifecyclePhase::Running) => {
                set service1 <= ServiceLifecyclePhase::Stopping;
            }

            rule reap_service0 when matches!(action, SystemAction::Service(ServicePlaneAction::ReapService(ServiceAtom::Service0)))
                && matches!(prev.service0, ServiceLifecyclePhase::Stopping) => {
                set service0 <= ServiceLifecyclePhase::Reaped;
            }

            rule reap_service1 when matches!(action, SystemAction::Service(ServicePlaneAction::ReapService(ServiceAtom::Service1)))
                && matches!(prev.service1, ServiceLifecyclePhase::Stopping) => {
                set service1 <= ServiceLifecyclePhase::Reaped;
            }

            rule trigger_rollback0 when matches!(action, SystemAction::Service(ServicePlaneAction::TriggerRollback(ServiceAtom::Service0)))
                && matches!(
                    prev.service0,
                    ServiceLifecyclePhase::Committed | ServiceLifecyclePhase::Promoted | ServiceLifecyclePhase::Ready
                ) => {
                set service0 <= ServiceLifecyclePhase::RollbackPending;
            }

            rule trigger_rollback1 when matches!(action, SystemAction::Service(ServicePlaneAction::TriggerRollback(ServiceAtom::Service1)))
                && matches!(
                    prev.service1,
                    ServiceLifecyclePhase::Committed | ServiceLifecyclePhase::Promoted | ServiceLifecyclePhase::Ready
                ) => {
                set service1 <= ServiceLifecyclePhase::RollbackPending;
            }

            rule finish_rollback0 when matches!(action, SystemAction::Service(ServicePlaneAction::FinishRollback(ServiceAtom::Service0)))
                && matches!(prev.service0, ServiceLifecyclePhase::RollbackPending) => {
                set service0 <= ServiceLifecyclePhase::RolledBack;
            }

            rule finish_rollback1 when matches!(action, SystemAction::Service(ServicePlaneAction::FinishRollback(ServiceAtom::Service1)))
                && matches!(prev.service1, ServiceLifecyclePhase::RollbackPending) => {
                set service1 <= ServiceLifecyclePhase::RolledBack;
            }

            rule grant_binding0 when matches!(action, SystemAction::Operation(OperationPlaneAction::GrantBinding(ServiceAtom::Service0)))
                && !prev.bound_services.contains(&ServiceAtom::Service0)
                && !prev.shutdown_started => {
                insert bound_services <= ServiceAtom::Service0;
            }

            rule grant_binding1 when matches!(action, SystemAction::Operation(OperationPlaneAction::GrantBinding(ServiceAtom::Service1)))
                && !prev.bound_services.contains(&ServiceAtom::Service1)
                && !prev.shutdown_started => {
                insert bound_services <= ServiceAtom::Service1;
            }

            rule revoke_binding0 when matches!(action, SystemAction::Operation(OperationPlaneAction::RevokeBinding(ServiceAtom::Service0)))
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && prev.local_rpc_target != Some(ServiceAtom::Service0)
                && prev.remote_rpc_target != Some(ServiceAtom::Service0) => {
                remove bound_services <= ServiceAtom::Service0;
            }

            rule revoke_binding1 when matches!(action, SystemAction::Operation(OperationPlaneAction::RevokeBinding(ServiceAtom::Service1)))
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && prev.local_rpc_target != Some(ServiceAtom::Service1)
                && prev.remote_rpc_target != Some(ServiceAtom::Service1) => {
                remove bound_services <= ServiceAtom::Service1;
            }

            rule start_command when matches!(action, SystemAction::Operation(OperationPlaneAction::StartCommand(_)))
                && prev.accepts_control
                && !prev.shutdown_started
                && prev.command_state.is_none() => {
                set command_kind <= if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartCommand(CommandKind::Deploy))
                ) {
                    Some(CommandKind::Deploy)
                } else if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartCommand(CommandKind::Run))
                ) {
                    Some(CommandKind::Run)
                } else {
                    Some(CommandKind::Stop)
                };
                set command_state <= Some(CommandLifecycleState::Accepted);
                set cancel_requested <= false;
            }

            rule mark_command_running when matches!(action, SystemAction::Operation(OperationPlaneAction::MarkCommandRunning))
                && matches!(prev.command_state, Some(CommandLifecycleState::Accepted)) => {
                set command_state <= Some(CommandLifecycleState::Running);
            }

            rule request_command_cancel when matches!(action, SystemAction::Operation(OperationPlaneAction::RequestCommandCancel))
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                ) => {
                set cancel_requested <= true;
            }

            rule finish_command_succeeded when matches!(action, SystemAction::Operation(OperationPlaneAction::FinishCommandSucceeded))
                && matches!(prev.command_state, Some(CommandLifecycleState::Running)) => {
                set command_state <= Some(CommandLifecycleState::Succeeded);
                set cancel_requested <= false;
            }

            rule finish_command_failed when matches!(action, SystemAction::Operation(OperationPlaneAction::FinishCommandFailed(_)))
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                ) => {
                set command_state <= Some(CommandLifecycleState::Failed);
                set cancel_requested <= false;
            }

            rule finish_command_canceled when matches!(action, SystemAction::Operation(OperationPlaneAction::FinishCommandCanceled))
                && matches!(
                    prev.command_state,
                    Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
                )
                && prev.cancel_requested => {
                set command_state <= Some(CommandLifecycleState::Canceled);
                set cancel_requested <= false;
            }

            rule clear_command_slot when matches!(action, SystemAction::Operation(OperationPlaneAction::ClearCommandSlot))
                && prev.command_state.is_some_and(CommandLifecycleState::is_terminal) => {
                set command_kind <= None;
                set command_state <= None;
                set cancel_requested <= false;
            }

            rule start_local_rpc0 when matches!(action, SystemAction::Operation(OperationPlaneAction::StartLocalRpc(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && matches!(prev.service0, ServiceLifecyclePhase::Running)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set local_rpc_target <= Some(ServiceAtom::Service0);
                set last_rpc_outcome <= RpcOutcome::None;
            }

            rule start_local_rpc1 when matches!(action, SystemAction::Operation(OperationPlaneAction::StartLocalRpc(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && matches!(prev.service1, ServiceLifecyclePhase::Running)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set local_rpc_target <= Some(ServiceAtom::Service1);
                set last_rpc_outcome <= RpcOutcome::None;
            }

            rule complete_local_rpc0 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service0)))
                && prev.local_rpc_target == Some(ServiceAtom::Service0) => {
                set local_rpc_target <= None;
                set last_rpc_outcome <= RpcOutcome::LocalResolved;
            }

            rule complete_local_rpc1 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteLocalRpc(ServiceAtom::Service1)))
                && prev.local_rpc_target == Some(ServiceAtom::Service1) => {
                set local_rpc_target <= None;
                set last_rpc_outcome <= RpcOutcome::LocalResolved;
            }

            rule deny_local_rpc0 when matches!(action, SystemAction::Operation(OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service0)))
                && prev.accepts_control
                && !prev.shutdown_started
                && (!prev.bound_services.contains(&ServiceAtom::Service0)
                    || !matches!(prev.service0, ServiceLifecyclePhase::Running))
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::LocalDenied;
            }

            rule deny_local_rpc1 when matches!(action, SystemAction::Operation(OperationPlaneAction::DenyLocalRpc(ServiceAtom::Service1)))
                && prev.accepts_control
                && !prev.shutdown_started
                && (!prev.bound_services.contains(&ServiceAtom::Service1)
                    || !matches!(prev.service1, ServiceLifecyclePhase::Running))
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::LocalDenied;
            }

            rule start_remote_rpc0 when matches!(action, SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service0, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && prev.bound_services.contains(&ServiceAtom::Service0)
                && matches!(prev.service0, ServiceLifecyclePhase::Running)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                insert remote_connections <= if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                ) {
                    RemoteAuthorityAtom::Edge0
                } else {
                    RemoteAuthorityAtom::Edge1
                };
                set remote_rpc_target <= Some(ServiceAtom::Service0);
                set remote_rpc_authority <= if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service0,
                        RemoteAuthorityAtom::Edge0
                    ))
                ) {
                    Some(RemoteAuthorityAtom::Edge0)
                } else {
                    Some(RemoteAuthorityAtom::Edge1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteConnected;
            }

            rule start_remote_rpc1 when matches!(action, SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(ServiceAtom::Service1, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && prev.bound_services.contains(&ServiceAtom::Service1)
                && matches!(prev.service1, ServiceLifecyclePhase::Running)
                && prev.local_rpc_target.is_none()
                && prev.remote_rpc_target.is_none() => {
                insert remote_connections <= if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service1,
                        RemoteAuthorityAtom::Edge0
                    ))
                ) {
                    RemoteAuthorityAtom::Edge0
                } else {
                    RemoteAuthorityAtom::Edge1
                };
                set remote_rpc_target <= Some(ServiceAtom::Service1);
                set remote_rpc_authority <= if matches!(
                    action,
                    SystemAction::Operation(OperationPlaneAction::StartRemoteRpc(
                        ServiceAtom::Service1,
                        RemoteAuthorityAtom::Edge0
                    ))
                ) {
                    Some(RemoteAuthorityAtom::Edge0)
                } else {
                    Some(RemoteAuthorityAtom::Edge1)
                };
                set last_rpc_outcome <= RpcOutcome::RemoteConnected;
            }

            rule complete_remote_rpc0_edge0 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge0)))
                && prev.remote_rpc_target == Some(ServiceAtom::Service0)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc0_edge1 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service0, RemoteAuthorityAtom::Edge1)))
                && prev.remote_rpc_target == Some(ServiceAtom::Service0)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc1_edge0 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge0)))
                && prev.remote_rpc_target == Some(ServiceAtom::Service1)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge0) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule complete_remote_rpc1_edge1 when matches!(action, SystemAction::Operation(OperationPlaneAction::CompleteRemoteRpc(ServiceAtom::Service1, RemoteAuthorityAtom::Edge1)))
                && prev.remote_rpc_target == Some(ServiceAtom::Service1)
                && prev.remote_rpc_authority == Some(RemoteAuthorityAtom::Edge1) => {
                set remote_rpc_target <= None;
                set remote_rpc_authority <= None;
                set last_rpc_outcome <= RpcOutcome::RemoteCompleted;
            }

            rule deny_remote_rpc0 when matches!(action, SystemAction::Operation(OperationPlaneAction::DenyRemoteRpc(ServiceAtom::Service0, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && (!prev.bound_services.contains(&ServiceAtom::Service0)
                    || !matches!(prev.service0, ServiceLifecyclePhase::Running))
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule deny_remote_rpc1 when matches!(action, SystemAction::Operation(OperationPlaneAction::DenyRemoteRpc(ServiceAtom::Service1, _)))
                && prev.accepts_control
                && !prev.shutdown_started
                && (!prev.bound_services.contains(&ServiceAtom::Service1)
                    || !matches!(prev.service1, ServiceLifecyclePhase::Running))
                && prev.remote_rpc_target.is_none() => {
                set last_rpc_outcome <= RpcOutcome::RemoteDenied;
            }

            rule disconnect_remote0 when matches!(action, SystemAction::Operation(OperationPlaneAction::DisconnectRemote(RemoteAuthorityAtom::Edge0)))
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

            rule disconnect_remote1 when matches!(action, SystemAction::Operation(OperationPlaneAction::DisconnectRemote(RemoteAuthorityAtom::Edge1)))
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

#[nirvash_macros::formal_tests(spec = SystemSpec)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use nirvash_check::ModelChecker;

    fn case_by_label(label: &str) -> ModelInstance<SystemState, SystemAction> {
        SystemSpec::new()
            .model_instances()
            .into_iter()
            .find(|case| case.label() == label)
            .expect("system case should exist")
    }

    fn bounded_parity_case(
        case: ModelInstance<SystemState, SystemAction>,
    ) -> ModelInstance<SystemState, SystemAction> {
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
        let spec = SystemSpec::new();
        let lowered = crate::lowered_spec(&spec);
        let explicit_case = bounded_parity_case(case_by_label("explicit_control_rpc_focus"));
        let symbolic_case = bounded_parity_case(case_by_label("symbolic_control_rpc_focus"));

        let explicit_snapshot = ModelChecker::for_case(&lowered, explicit_case.clone())
            .reachable_graph_snapshot()
            .expect("explicit system snapshot");
        let symbolic_snapshot = ModelChecker::for_case(&lowered, symbolic_case.clone())
            .reachable_graph_snapshot()
            .expect("symbolic system snapshot");
        assert_eq!(symbolic_snapshot, explicit_snapshot);
    }

    #[test]
    fn option_surface_is_symbolic_encodable() {
        let spec = SystemSpec::new();
        let program = spec
            .transition_program()
            .expect("system should expose a transition program");

        assert!(program.is_ast_native());
        assert_eq!(program.first_unencodable_symbolic_node(), None);
    }
}
