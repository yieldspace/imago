use imagod_spec::MessageType;
use nirvash_core::concurrent::{ConcurrentAction, ConcurrentTransitionSystem};
use nirvash_core::{
    ActionConstraint, ModelCase, ModelCaseSource, ModelCheckConfig, Signature as _,
    StateConstraint, StatePredicate, TemporalSpec, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{action_constraint, invariant, state_constraint, system_spec};

use crate::{
    CommandKind, CommandLifecycleState, CommandProtocolAction, OperationPhase, PluginKind,
    atoms::{
        CommandEventAtom, LogChunkAtom, RemoteAuthorityAtom, RequestKindAtom, RpcCallAtom,
        RpcConnectionAtom, RunnerAtom, ServiceAtom, SessionAtom, StreamAtom, binding_target_for,
        binding_target_service, service_runner,
    },
    command_protocol::{CommandProtocolSpec, CommandProtocolState},
    deploy::{DeployAction, DeploySpec, DeployState},
    manager_runtime::{
        ManagerRuntimeAction, ManagerRuntimePhase, ManagerRuntimeSpec, ManagerRuntimeState,
    },
    plugin_platform::{PluginPlatformAction, PluginPlatformSpec, PluginPlatformState},
    rpc::{RpcAction, RpcSpec, RpcState},
    session_auth::{SessionAuthAction, SessionAuthSpec, SessionAuthState},
    session_transport::{SessionTransportAction, SessionTransportSpec, SessionTransportState},
    shutdown_flow::{ShutdownFlowAction, ShutdownFlowSpec, ShutdownFlowState, ShutdownPhase},
    supervision::{SupervisionAction, SupervisionSpec, SupervisionState},
    wire_protocol::{WireProtocolAction, WireProtocolSpec, WireProtocolState},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemState {
    pub manager: ManagerRuntimeState,
    pub session: SessionTransportState,
    pub session_auth: SessionAuthState,
    pub wire: WireProtocolState,
    pub command: CommandProtocolState,
    pub deploy: DeployState,
    pub supervision: SupervisionState,
    pub rpc: RpcState,
    pub plugin: PluginPlatformState,
    pub shutdown: ShutdownFlowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemAtomicAction {
    Manager(ManagerRuntimeAction),
    Session(SessionTransportAction),
    SessionAuth(SessionAuthAction),
    Wire(WireProtocolAction),
    Command(CommandProtocolAction),
    Deploy(DeployAction),
    Supervision(SupervisionAction),
    Rpc(RpcAction),
    Plugin(PluginPlatformAction),
    Shutdown(ShutdownFlowAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemResourceKey {
    Manager,
    Session,
    Command,
    Plugin,
    Shutdown,
    Service(ServiceAtom),
    Runner(RunnerAtom),
    Deploy(ServiceAtom),
    Stream(StreamAtom),
    Authority(RemoteAuthorityAtom),
    RpcConnection(RpcConnectionAtom),
    RpcCall(RpcCallAtom),
}

pub type SystemAction = ConcurrentAction<SystemAtomicAction>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemEffect {
    Response(StreamAtom, RequestKindAtom),
    CommandEvent(StreamAtom, CommandEventAtom),
    LogChunk(StreamAtom, LogChunkAtom),
    LogsEnd(StreamAtom),
    AuthorizationRejected(StreamAtom, RequestKindAtom),
    ShutdownComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemMessageBinding {
    Request(RequestKindAtom),
    Response(RequestKindAtom),
    CommandEvent,
    LogChunk,
    LogsEnd,
}

pub const fn system_message_binding(message_type: MessageType) -> SystemMessageBinding {
    match message_type {
        MessageType::HelloNegotiate => {
            SystemMessageBinding::Request(RequestKindAtom::HelloNegotiate)
        }
        MessageType::DeployPrepare => SystemMessageBinding::Request(RequestKindAtom::DeployPrepare),
        MessageType::ArtifactPush => SystemMessageBinding::Request(RequestKindAtom::ArtifactPush),
        MessageType::ArtifactCommit => {
            SystemMessageBinding::Request(RequestKindAtom::ArtifactCommit)
        }
        MessageType::CommandStart => SystemMessageBinding::Request(RequestKindAtom::CommandStart),
        MessageType::CommandEvent => SystemMessageBinding::CommandEvent,
        MessageType::StateRequest => SystemMessageBinding::Request(RequestKindAtom::StateRequest),
        MessageType::StateResponse => SystemMessageBinding::Response(RequestKindAtom::StateRequest),
        MessageType::ServicesList => SystemMessageBinding::Request(RequestKindAtom::ServicesList),
        MessageType::CommandCancel => SystemMessageBinding::Request(RequestKindAtom::CommandCancel),
        MessageType::LogsRequest => SystemMessageBinding::Request(RequestKindAtom::LogsRequest),
        MessageType::LogsChunk => SystemMessageBinding::LogChunk,
        MessageType::LogsEnd => SystemMessageBinding::LogsEnd,
        MessageType::RpcInvoke => SystemMessageBinding::Request(RequestKindAtom::RpcInvoke),
        MessageType::BindingsCertUpload => {
            SystemMessageBinding::Request(RequestKindAtom::BindingsCertUpload)
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemSpec;

impl SystemSpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> SystemState {
        SystemState {
            manager: ManagerRuntimeState {
                phase: ManagerRuntimePhase::Listening,
                config_loaded: true,
                created_default: false,
                plugin_gc: crate::manager_runtime::TaskState::Succeeded,
                boot_restore: crate::manager_runtime::TaskState::Succeeded,
            },
            session: SessionTransportSpec::new().initial_state(),
            session_auth: SessionAuthSpec::new().initial_state(),
            wire: WireProtocolSpec::new().initial_state(),
            command: CommandProtocolSpec::new().initial_state(),
            deploy: DeploySpec::new().initial_state(),
            supervision: SupervisionSpec::new().initial_state(),
            rpc: RpcSpec::new().initial_state(),
            plugin: PluginPlatformSpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        }
    }

    pub fn boot_state(&self) -> SystemState {
        SystemState {
            manager: ManagerRuntimeSpec::new().initial_state(),
            session: SessionTransportSpec::new().initial_state(),
            session_auth: SessionAuthSpec::new().initial_state(),
            wire: WireProtocolSpec::new().initial_state(),
            command: CommandProtocolSpec::new().initial_state(),
            deploy: DeploySpec::new().initial_state(),
            supervision: SupervisionSpec::new().initial_state(),
            rpc: RpcSpec::new().initial_state(),
            plugin: PluginPlatformSpec::new().initial_state(),
            shutdown: ShutdownFlowSpec::new().initial_state(),
        }
    }
}

impl ConcurrentTransitionSystem for SystemSpec {
    type State = SystemState;
    type AtomicAction = SystemAtomicAction;
    type ResourceKey = SystemResourceKey;

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.boot_state()]
    }

    fn atomic_actions(&self) -> Vec<Self::AtomicAction> {
        let mut actions = Vec::new();
        actions.extend(
            <ManagerRuntimeAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Manager),
        );
        actions.extend(
            <SessionTransportAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Session),
        );
        actions.extend(
            <SessionAuthAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::SessionAuth),
        );
        actions.extend(
            <WireProtocolAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Wire),
        );
        actions.extend(
            <CommandProtocolAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Command),
        );
        actions.extend(
            <DeployAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Deploy),
        );
        actions.extend(
            <SupervisionAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Supervision),
        );
        actions.extend(
            <RpcAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Rpc),
        );
        actions.extend(
            <PluginPlatformAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Plugin),
        );
        actions.extend(
            <ShutdownFlowAction as nirvash_core::ActionVocabulary>::action_vocabulary()
                .into_iter()
                .map(SystemAtomicAction::Shutdown),
        );
        actions
    }

    fn enabled_atomic_actions(&self, state: &Self::State) -> Vec<Self::AtomicAction> {
        system_atomic_candidates(state)
            .into_iter()
            .filter(|action| self.atomic_transition(state, action).is_some())
            .collect()
    }

    fn atomic_transition(
        &self,
        prev: &Self::State,
        action: &Self::AtomicAction,
    ) -> Option<Self::State> {
        let manager_spec = ManagerRuntimeSpec::new();
        let session_spec = SessionTransportSpec::new();
        let session_auth_spec = SessionAuthSpec::new();
        let wire_spec = WireProtocolSpec::new();
        let command_spec = CommandProtocolSpec::new();
        let deploy_spec = DeploySpec::new();
        let supervision_spec = SupervisionSpec::new();
        let rpc_spec = RpcSpec::new();
        let plugin_spec = PluginPlatformSpec::new();
        let shutdown_spec = ShutdownFlowSpec::new();

        let mut candidate = prev.clone();
        match action {
            SystemAtomicAction::Manager(manager_action) => {
                candidate.manager = manager_spec.transition(&prev.manager, manager_action)?;
            }
            SystemAtomicAction::Session(session_action) => {
                if !session_action_allowed(prev, session_action) {
                    return None;
                }
                candidate.session = session_spec.transition(&prev.session, session_action)?;
            }
            SystemAtomicAction::SessionAuth(session_auth_action) => {
                if !session_auth_action_allowed(prev, session_auth_action) {
                    return None;
                }
                if matches!(session_auth_action, SessionAuthAction::AcceptSession(_)) {
                    candidate.session = session_spec
                        .transition(&prev.session, &SessionTransportAction::AcceptSession)?;
                }
                candidate.session_auth =
                    session_auth_spec.transition(&prev.session_auth, session_auth_action)?;
            }
            SystemAtomicAction::Wire(wire_action) => {
                if !wire_action_allowed(prev, wire_action) {
                    return None;
                }
                candidate.wire = wire_spec.transition(&prev.wire, wire_action)?;
                if let WireProtocolAction::BindingsCertUpload(stream) = wire_action {
                    candidate.session_auth = session_auth_spec.transition(
                        &candidate.session_auth,
                        &SessionAuthAction::UploadClientAuthority(stream_authority(*stream)),
                    )?;
                }
            }
            SystemAtomicAction::Command(command_action) => {
                if !command_action_allowed(prev, command_action) {
                    return None;
                }
                candidate.command = command_spec.transition(&prev.command, command_action)?;
            }
            SystemAtomicAction::Deploy(deploy_action) => {
                if !deploy_action_allowed(prev, deploy_action) {
                    return None;
                }
                candidate.deploy = deploy_spec.transition(&prev.deploy, deploy_action)?;
            }
            SystemAtomicAction::Supervision(supervision_action) => {
                if !supervision_action_allowed(prev, supervision_action) {
                    return None;
                }
                candidate.supervision =
                    supervision_spec.transition(&prev.supervision, supervision_action)?;
            }
            SystemAtomicAction::Rpc(rpc_action) => {
                if !rpc_action_allowed(prev, rpc_action) {
                    return None;
                }
                candidate.rpc = rpc_spec.transition(&prev.rpc, rpc_action)?;
            }
            SystemAtomicAction::Plugin(plugin_action) => {
                if !plugin_action_allowed(prev, plugin_action) {
                    return None;
                }
                candidate.plugin = plugin_spec.transition(&prev.plugin, plugin_action)?;
            }
            SystemAtomicAction::Shutdown(shutdown_action) => {
                if !shutdown_action_allowed(prev, shutdown_action) {
                    return None;
                }
                candidate.shutdown = shutdown_spec.transition(&prev.shutdown, shutdown_action)?;
            }
        }

        multi_service_state_valid(&candidate).then_some(candidate)
    }

    fn footprint_reads(
        &self,
        action: &Self::AtomicAction,
    ) -> std::collections::BTreeSet<Self::ResourceKey> {
        match action {
            SystemAtomicAction::Manager(manager_action) => manager_read_resources(*manager_action),
            SystemAtomicAction::Session(session_action) => session_read_resources(*session_action),
            SystemAtomicAction::SessionAuth(session_auth_action) => {
                session_auth_read_resources(*session_auth_action)
            }
            SystemAtomicAction::Wire(wire_action) => wire_read_resources(*wire_action),
            SystemAtomicAction::Command(command_action) => command_read_resources(command_action),
            SystemAtomicAction::Deploy(deploy_action) => deploy_read_resources(*deploy_action),
            SystemAtomicAction::Supervision(supervision_action) => {
                supervision_read_resources(*supervision_action)
            }
            SystemAtomicAction::Rpc(rpc_action) => rpc_read_resources(*rpc_action),
            SystemAtomicAction::Plugin(plugin_action) => plugin_read_resources(plugin_action),
            SystemAtomicAction::Shutdown(shutdown_action) => {
                shutdown_read_resources(*shutdown_action)
            }
        }
    }

    fn footprint_writes(
        &self,
        action: &Self::AtomicAction,
    ) -> std::collections::BTreeSet<Self::ResourceKey> {
        match action {
            SystemAtomicAction::Manager(manager_action) => manager_write_resources(*manager_action),
            SystemAtomicAction::Session(session_action) => session_write_resources(*session_action),
            SystemAtomicAction::SessionAuth(session_auth_action) => {
                session_auth_write_resources(*session_auth_action)
            }
            SystemAtomicAction::Wire(wire_action) => wire_write_resources(*wire_action),
            SystemAtomicAction::Command(command_action) => command_write_resources(command_action),
            SystemAtomicAction::Deploy(deploy_action) => deploy_write_resources(*deploy_action),
            SystemAtomicAction::Supervision(supervision_action) => {
                supervision_write_resources(*supervision_action)
            }
            SystemAtomicAction::Rpc(rpc_action) => rpc_write_resources(*rpc_action),
            SystemAtomicAction::Plugin(plugin_action) => plugin_write_resources(plugin_action),
            SystemAtomicAction::Shutdown(shutdown_action) => {
                shutdown_write_resources(*shutdown_action)
            }
        }
    }
}

fn system_model_cases() -> Vec<ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>>> {
    vec![
        boot_gc_and_restore_case(),
        session_auth_and_authorize_case(),
        hello_negotiation_and_limits_case(),
        deploy_upload_and_commit_case(),
        command_start_event_flow_case(),
        state_request_and_cancel_case(),
        services_list_merge_case(),
        logs_request_snapshot_and_follow_case(),
        bindings_cert_upload_updates_authorization_case(),
        parallel_deploy_and_start_case(),
        service_scoped_rollback_case(),
        local_rpc_happy_path_case(),
        local_rpc_denied_case(),
        remote_rpc_connection_lifecycle_case(),
        shutdown_blocks_new_rpc_case(),
        graceful_shutdown_and_force_fallback_case(),
        maintenance_reap_and_idle_tick_case(),
    ]
}

fn system_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: Some(12),
        max_states: Some(2048),
        max_transitions: Some(8192),
        check_deadlocks: false,
        stop_on_first_violation: false,
    }
}

fn system_doc_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: Some(10),
        max_states: Some(192),
        max_transitions: Some(768),
        check_deadlocks: false,
        stop_on_first_violation: false,
    }
}

fn focused_system_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: Some(8),
        max_states: Some(512),
        max_transitions: Some(2048),
        check_deadlocks: false,
        stop_on_first_violation: false,
    }
}

fn focused_system_doc_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: Some(8),
        max_states: Some(96),
        max_transitions: Some(384),
        check_deadlocks: false,
        stop_on_first_violation: false,
    }
}

fn shutdown_system_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        exploration: nirvash_core::ExplorationMode::ReachableGraph,
        bounded_depth: Some(10),
        max_states: Some(2048),
        max_transitions: Some(8192),
        check_deadlocks: false,
        stop_on_first_violation: false,
    }
}

fn system_atomic_candidates(state: &SystemState) -> Vec<SystemAtomicAction> {
    let mut actions = Vec::new();

    match state.manager.phase {
        ManagerRuntimePhase::Booting => {
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::LoadExistingConfig,
            ));
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::CreateDefaultConfig,
            ));
        }
        ManagerRuntimePhase::ConfigReady => {
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::RunPluginGcSucceeded,
            ));
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::RunPluginGcFailed,
            ));
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::StartListening,
            ));
        }
        ManagerRuntimePhase::Restoring => {
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::RunBootRestoreSucceeded,
            ));
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::RunBootRestoreFailed,
            ));
        }
        ManagerRuntimePhase::Listening => {
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::BeginShutdown,
            ));
        }
        ManagerRuntimePhase::ShutdownRequested => {
            actions.push(SystemAtomicAction::Manager(
                ManagerRuntimeAction::FinishShutdown,
            ));
        }
        ManagerRuntimePhase::Stopped => {}
    }

    if !state.session.shutdown_requested && !state.session.at_capacity() {
        actions.push(SystemAtomicAction::Session(
            SessionTransportAction::AcceptSession,
        ));
        for session in [SessionAtom::Session0, SessionAtom::Session1] {
            if !state.session_auth.session_accepted(session) {
                actions.push(SystemAtomicAction::SessionAuth(
                    SessionAuthAction::AcceptSession(session),
                ));
            }
        }
    }
    if state.session.has_active_sessions() {
        actions.push(SystemAtomicAction::Session(
            SessionTransportAction::JoinSession,
        ));
    }
    if state.session.at_capacity() || state.session.shutdown_requested {
        actions.push(SystemAtomicAction::Session(
            SessionTransportAction::RejectTooMany,
        ));
    }
    if !state.session.shutdown_requested
        && !matches!(state.shutdown.phase, ShutdownPhase::Completed)
    {
        actions.push(SystemAtomicAction::Session(
            SessionTransportAction::BeginShutdown,
        ));
    }

    if state.session.has_active_sessions() {
        for session in [SessionAtom::Session0, SessionAtom::Session1] {
            if state.session_auth.session_accepted(session)
                && !state.session_auth.session_authenticated(session)
            {
                actions.push(SystemAtomicAction::SessionAuth(
                    SessionAuthAction::AuthenticateAdmin(session),
                ));
                actions.push(SystemAtomicAction::SessionAuth(
                    SessionAuthAction::AuthenticateClient(session),
                ));
            }
        }
        if state
            .session_auth
            .any_authenticated_as(crate::atoms::SessionRoleAtom::Admin)
        {
            for kind in [
                RequestKindAtom::DeployPrepare,
                RequestKindAtom::ArtifactPush,
                RequestKindAtom::ArtifactCommit,
                RequestKindAtom::CommandStart,
                RequestKindAtom::StateRequest,
                RequestKindAtom::ServicesList,
                RequestKindAtom::CommandCancel,
            ] {
                actions.push(SystemAtomicAction::SessionAuth(
                    SessionAuthAction::AuthorizeAdmin(StreamAtom::Stream0, kind),
                ));
            }
            for kind in [
                RequestKindAtom::LogsRequest,
                RequestKindAtom::BindingsCertUpload,
            ] {
                actions.push(SystemAtomicAction::SessionAuth(
                    SessionAuthAction::AuthorizeAdmin(StreamAtom::Stream1, kind),
                ));
            }
        }
        if state
            .session_auth
            .any_authenticated_as(crate::atoms::SessionRoleAtom::Client)
        {
            actions.push(SystemAtomicAction::SessionAuth(
                SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::HelloNegotiate,
                ),
            ));
            actions.push(SystemAtomicAction::SessionAuth(
                SessionAuthAction::AuthorizeClient(StreamAtom::Stream0, RequestKindAtom::RpcInvoke),
            ));
        }
        actions.push(SystemAtomicAction::SessionAuth(
            SessionAuthAction::RejectUnauthorized(
                StreamAtom::Stream0,
                RequestKindAtom::ServicesList,
            ),
        ));
    }
    if !state
        .session_auth
        .authority_uploaded(RemoteAuthorityAtom::Edge0)
    {
        actions.push(SystemAtomicAction::SessionAuth(
            SessionAuthAction::UploadClientAuthority(RemoteAuthorityAtom::Edge0),
        ));
    }
    if !state
        .session_auth
        .authority_uploaded(RemoteAuthorityAtom::Edge1)
    {
        actions.push(SystemAtomicAction::SessionAuth(
            SessionAuthAction::UploadClientAuthority(RemoteAuthorityAtom::Edge1),
        ));
    }

    if state
        .session_auth
        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::HelloNegotiate)
    {
        actions.push(SystemAtomicAction::Wire(
            WireProtocolAction::HelloNegotiate(StreamAtom::Stream0),
        ));
    }
    for (kind, action) in [
        (
            RequestKindAtom::DeployPrepare,
            WireProtocolAction::DeployPrepare(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::ArtifactPush,
            WireProtocolAction::ArtifactPush(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::ArtifactCommit,
            WireProtocolAction::ArtifactCommit(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::CommandStart,
            WireProtocolAction::CommandStart(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::StateRequest,
            WireProtocolAction::StateRequest(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::ServicesList,
            WireProtocolAction::ServicesList(StreamAtom::Stream0),
        ),
        (
            RequestKindAtom::CommandCancel,
            WireProtocolAction::CommandCancel(StreamAtom::Stream0),
        ),
    ] {
        if state
            .session_auth
            .stream_authorized(StreamAtom::Stream0, kind)
        {
            actions.push(SystemAtomicAction::Wire(action));
        }
    }
    if state
        .session_auth
        .stream_authorized(StreamAtom::Stream0, RequestKindAtom::RpcInvoke)
    {
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::RpcInvoke(
            StreamAtom::Stream0,
        )));
    }
    if state
        .session_auth
        .stream_authorized(StreamAtom::Stream1, RequestKindAtom::LogsRequest)
    {
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::LogsRequest(
            StreamAtom::Stream1,
        )));
    }
    if state.wire.logs_acknowledged(StreamAtom::Stream1) {
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::LogsChunk(
            StreamAtom::Stream1,
            LogChunkAtom::Chunk0,
        )));
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::LogsEnd(
            StreamAtom::Stream1,
        )));
    }
    if state.command.tracked {
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
            StreamAtom::Stream0,
            CommandEventAtom::Accepted,
        )));
        actions.push(SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
            StreamAtom::Stream0,
            CommandEventAtom::Succeeded,
        )));
    }
    if state
        .session_auth
        .stream_authorized(StreamAtom::Stream1, RequestKindAtom::BindingsCertUpload)
    {
        actions.push(SystemAtomicAction::Wire(
            WireProtocolAction::BindingsCertUpload(StreamAtom::Stream1),
        ));
    }

    if !state.command.tracked {
        actions.push(SystemAtomicAction::Command(CommandProtocolAction::Start(
            CommandKind::Deploy,
        )));
    }
    if matches!(
        state.command.lifecycle_state,
        Some(CommandLifecycleState::Accepted)
    ) {
        actions.push(SystemAtomicAction::Command(
            CommandProtocolAction::SetRunning,
        ));
    }
    if matches!(
        state.command.lifecycle_state,
        Some(CommandLifecycleState::Running)
    ) {
        actions.push(SystemAtomicAction::Command(
            CommandProtocolAction::MarkSpawned,
        ));
        actions.push(SystemAtomicAction::Command(
            CommandProtocolAction::RequestCancel,
        ));
    }
    if matches!(
        state.command.lifecycle_state,
        Some(CommandLifecycleState::Running)
    ) && matches!(state.command.phase, Some(OperationPhase::Spawned))
    {
        actions.push(SystemAtomicAction::Command(
            CommandProtocolAction::FinishSucceeded,
        ));
    }
    if state
        .command
        .lifecycle_state
        .is_some_and(CommandLifecycleState::is_terminal)
    {
        actions.push(SystemAtomicAction::Command(CommandProtocolAction::Remove));
    }

    for service in [ServiceAtom::Service0, ServiceAtom::Service1] {
        for action in [
            DeployAction::AdvanceUpload(service),
            DeployAction::CommitUpload(service),
            DeployAction::AdvanceRelease(service),
            DeployAction::SetRestartPolicy(service),
            DeployAction::TriggerRollback(service),
            DeployAction::FinishRollback(service),
        ] {
            actions.push(SystemAtomicAction::Deploy(action));
        }
        for action in [
            SupervisionAction::PrepareEndpoint(service),
            SupervisionAction::AdvanceBootstrap(service),
            SupervisionAction::StartServing(service),
            SupervisionAction::RequestStop(service),
            SupervisionAction::ReapService(service),
        ] {
            actions.push(SystemAtomicAction::Supervision(action));
        }
    }

    for action in [
        RpcAction::GrantBinding(ServiceAtom::Service0),
        RpcAction::ResolveLocal(ServiceAtom::Service0),
        RpcAction::RejectLocal(ServiceAtom::Service0),
        RpcAction::ConnectRemote(ServiceAtom::Service0),
        RpcAction::InvokeRemote(ServiceAtom::Service0),
        RpcAction::RejectRemoteInvoke(ServiceAtom::Service0),
        RpcAction::CompleteRemoteCall(ServiceAtom::Service0),
        RpcAction::DisconnectRemote(ServiceAtom::Service0),
    ] {
        actions.push(SystemAtomicAction::Rpc(action));
    }

    if !state.plugin.plugin_registered() {
        actions.push(SystemAtomicAction::Plugin(
            PluginPlatformAction::RegisterPlugin(PluginKind::Wasm),
        ));
    } else if !state.plugin.graph_classified() {
        actions.push(SystemAtomicAction::Plugin(
            PluginPlatformAction::ClassifyGraphAcyclic,
        ));
    } else if !state.plugin.provider_decided() {
        actions.push(SystemAtomicAction::Plugin(
            PluginPlatformAction::ResolveProviderDependency,
        ));
    } else if !state.plugin.capability_decided() {
        actions.push(SystemAtomicAction::Plugin(
            PluginPlatformAction::AllowCapability,
        ));
    } else if !state.plugin.http_outbound_enabled() {
        actions.push(SystemAtomicAction::Plugin(
            PluginPlatformAction::AllowHttpHost,
        ));
    }

    match state.shutdown.phase {
        ShutdownPhase::Idle => actions.push(SystemAtomicAction::Shutdown(
            ShutdownFlowAction::ReceiveSignal,
        )),
        ShutdownPhase::SignalReceived => actions.push(SystemAtomicAction::Shutdown(
            ShutdownFlowAction::StopAccepting,
        )),
        ShutdownPhase::DrainingSessions => {
            actions.push(SystemAtomicAction::Shutdown(
                ShutdownFlowAction::DrainSessions,
            ));
        }
        ShutdownPhase::StoppingServices => {
            actions.push(SystemAtomicAction::Shutdown(
                ShutdownFlowAction::StopServicesGraceful,
            ));
            actions.push(SystemAtomicAction::Shutdown(
                ShutdownFlowAction::StopServicesForced,
            ));
        }
        ShutdownPhase::StoppingMaintenance => {
            actions.push(SystemAtomicAction::Shutdown(
                ShutdownFlowAction::StopMaintenance,
            ));
            actions.push(SystemAtomicAction::Shutdown(ShutdownFlowAction::Finalize));
        }
        ShutdownPhase::Completed => {}
    }

    actions
}

fn boot_gc_and_restore_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("boot_gc_and_restore")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn session_auth_and_authorize_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>>
{
    ModelCase::new("session_auth_and_authorize")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn hello_negotiation_and_limits_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("hello_negotiation_and_limits")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn deploy_upload_and_commit_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("deploy_upload_and_commit")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn command_start_event_flow_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("command_start_event_flow")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn state_request_and_cancel_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("state_request_and_cancel")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn services_list_merge_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("services_list_merge")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn logs_request_snapshot_and_follow_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("logs_request_snapshot_and_follow")
        .with_checker_config(system_checker_config())
        .with_doc_checker_config(system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn bindings_cert_upload_updates_authorization_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("bindings_cert_upload_updates_authorization")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn parallel_deploy_and_start_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>>
{
    ModelCase::new("parallel_deploy_and_start")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn service_scoped_rollback_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("service_scoped_rollback")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn local_rpc_happy_path_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("local_rpc_happy_path")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn local_rpc_denied_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("local_rpc_denied_or_target_missing")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn remote_rpc_connection_lifecycle_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("remote_rpc_connection_lifecycle")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[allow(dead_code)]
fn shutdown_blocks_new_rpc_case() -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("shutdown_blocks_new_rpc_and_drains_services")
        .with_checker_config(shutdown_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn graceful_shutdown_and_force_fallback_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("graceful_shutdown_and_force_fallback")
        .with_checker_config(shutdown_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

fn maintenance_reap_and_idle_tick_case()
-> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ModelCase::new("maintenance_reap_and_idle_tick")
        .with_checker_config(focused_system_checker_config())
        .with_doc_checker_config(focused_system_doc_checker_config())
        .with_check_deadlocks(false)
}

#[action_constraint(SystemSpec, cases("boot_gc_and_restore"))]
fn boot_gc_and_restore_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("boot_gc_and_restore_actions", |_, action, _| {
        action.arity() == 1 && action.atoms().iter().all(boot_to_listening_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("session_auth_and_authorize"))]
fn session_auth_and_authorize_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("session_auth_and_authorize_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(session_auth_and_authorize_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("hello_negotiation_and_limits"))]
fn hello_negotiation_and_limits_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("hello_negotiation_and_limits_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(hello_negotiation_and_limits_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("deploy_upload_and_commit"))]
fn deploy_upload_and_commit_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("deploy_upload_and_commit_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(deploy_upload_and_commit_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("command_start_event_flow"))]
fn command_start_event_flow_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("command_start_event_flow_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(command_start_event_flow_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("state_request_and_cancel"))]
fn state_request_and_cancel_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("state_request_and_cancel_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(state_request_and_cancel_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("services_list_merge"))]
fn services_list_merge_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("services_list_merge_actions", |_, action, _| {
        action.arity() <= 2 && action.atoms().iter().all(services_list_merge_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("logs_request_snapshot_and_follow"))]
fn logs_request_snapshot_and_follow_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new(
        "logs_request_snapshot_and_follow_actions",
        |_, action, _| {
            action.arity() <= 2
                && action
                    .atoms()
                    .iter()
                    .all(logs_request_snapshot_and_follow_atom_allowed)
        },
    )
}

#[action_constraint(SystemSpec, cases("bindings_cert_upload_updates_authorization"))]
fn bindings_cert_upload_updates_authorization_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new(
        "bindings_cert_upload_updates_authorization_actions",
        |_, action, _| {
            action.arity() == 1
                && action
                    .atoms()
                    .iter()
                    .all(bindings_cert_upload_updates_authorization_atom_allowed)
        },
    )
}

#[action_constraint(SystemSpec, cases("parallel_deploy_and_start"))]
fn parallel_deploy_and_start_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("parallel_deploy_and_start_actions", |_, action, _| {
        action.arity() <= 2
            && action
                .atoms()
                .iter()
                .all(parallel_deploy_and_start_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("service_scoped_rollback"))]
fn service_scoped_rollback_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("service_scoped_rollback_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(service_scoped_rollback_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("local_rpc_happy_path"))]
fn local_rpc_happy_path_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("local_rpc_happy_path_actions", |_, action, _| {
        action.arity() == 1 && action.atoms().iter().all(local_rpc_happy_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("local_rpc_denied_or_target_missing"))]
fn local_rpc_denied_actions() -> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>>
{
    ActionConstraint::new("local_rpc_denied_actions", |_, action, _| {
        action.arity() == 1 && action.atoms().iter().all(local_rpc_denied_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("remote_rpc_connection_lifecycle"))]
fn remote_rpc_connection_lifecycle_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("remote_rpc_connection_lifecycle_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(remote_rpc_connection_atom_allowed)
    })
}

#[state_constraint(SystemSpec, cases("shutdown_blocks_new_rpc_and_drains_services"))]
fn shutdown_service1_quiescent() -> StateConstraint<SystemState> {
    StateConstraint::new("shutdown_service1_quiescent", |state: &SystemState| {
        state.deploy.service_is_quiescent(ServiceAtom::Service1)
            && state
                .supervision
                .service_is_quiescent(ServiceAtom::Service1)
            && state.rpc.service_is_quiescent(ServiceAtom::Service1)
    })
}

#[action_constraint(SystemSpec, cases("shutdown_blocks_new_rpc_and_drains_services"))]
fn shutdown_blocks_new_rpc_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("shutdown_blocks_new_rpc_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(shutdown_blocks_new_rpc_atom_allowed)
    })
}

#[action_constraint(SystemSpec, cases("graceful_shutdown_and_force_fallback"))]
fn graceful_shutdown_and_force_fallback_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new(
        "graceful_shutdown_and_force_fallback_actions",
        |_, action, _| {
            action.arity() == 1
                && action
                    .atoms()
                    .iter()
                    .all(graceful_shutdown_and_force_fallback_atom_allowed)
        },
    )
}

#[action_constraint(SystemSpec, cases("maintenance_reap_and_idle_tick"))]
fn maintenance_reap_and_idle_tick_actions()
-> ActionConstraint<SystemState, ConcurrentAction<SystemAtomicAction>> {
    ActionConstraint::new("maintenance_reap_and_idle_tick_actions", |_, action, _| {
        action.arity() == 1
            && action
                .atoms()
                .iter()
                .all(maintenance_reap_and_idle_tick_atom_allowed)
    })
}

#[invariant(SystemSpec)]
fn running_services_require_promoted_release() -> StatePredicate<SystemState> {
    StatePredicate::new("running_services_require_promoted_release", |state| {
        ServiceAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|service| {
                (!state.supervision.service_is_ready(service)
                    && !state.supervision.service_is_running(service)
                    && !state.supervision.service_is_stopping(service))
                    || state.deploy.release_promoted(service)
            })
    })
}

#[invariant(SystemSpec)]
fn local_rpc_resolution_requires_ready_target() -> StatePredicate<SystemState> {
    StatePredicate::new("local_rpc_resolution_requires_ready_target", |state| {
        ServiceAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|source| {
                !state.rpc.has_local_resolution_for(source) || {
                    let target = binding_target_service(binding_target_for(source));
                    state.rpc.binding_allowed(source)
                        && state.supervision.service_is_ready(target)
                        && state.supervision.service_is_running(target)
                }
            })
    })
}

#[invariant(SystemSpec)]
fn remote_rpc_connections_require_running_owner() -> StatePredicate<SystemState> {
    StatePredicate::new("remote_rpc_connections_require_running_owner", |state| {
        ServiceAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|source| {
                !state.rpc.has_remote_connection_for(source)
                    || state.supervision.service_is_running(source)
            })
    })
}

#[invariant(SystemSpec)]
fn shutdown_requires_session_gate_and_manager_shutdown() -> StatePredicate<SystemState> {
    StatePredicate::new(
        "shutdown_requires_session_gate_and_manager_shutdown",
        |state| {
            matches!(
                state.shutdown.phase,
                ShutdownPhase::Idle | ShutdownPhase::SignalReceived
            ) || (state.session.shutdown_requested
                && matches!(
                    state.manager.phase,
                    ManagerRuntimePhase::ShutdownRequested | ManagerRuntimePhase::Stopped
                ))
        },
    )
}

#[invariant(SystemSpec)]
fn active_command_requires_listening_manager() -> StatePredicate<SystemState> {
    StatePredicate::new("active_command_requires_listening_manager", |state| {
        !matches!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        ) || matches!(state.manager.phase, ManagerRuntimePhase::Listening)
    })
}

#[invariant(SystemSpec)]
fn dependency_provider_requires_acyclic_plugin_graph() -> StatePredicate<SystemState> {
    StatePredicate::new(
        "dependency_provider_requires_acyclic_plugin_graph",
        |state| !state.plugin.provider_is_dependency() || state.plugin.graph_is_acyclic(),
    )
}

#[invariant(SystemSpec)]
fn non_hello_wire_requests_require_authorized_streams() -> StatePredicate<SystemState> {
    StatePredicate::new(
        "non_hello_wire_requests_require_authorized_streams",
        |state| {
            StreamAtom::bounded_domain()
                .into_vec()
                .into_iter()
                .all(|stream| {
                    RequestKindAtom::bounded_domain()
                        .into_vec()
                        .into_iter()
                        .all(|kind| {
                            !state.wire.saw_request(stream, kind)
                                || !request_kind_requires_authorization(kind)
                                || state.session_auth.stream_authorized(stream, kind)
                        })
                })
        },
    )
}

#[invariant(SystemSpec)]
fn cert_upload_updates_dynamic_authority() -> StatePredicate<SystemState> {
    StatePredicate::new("cert_upload_updates_dynamic_authority", |state| {
        StreamAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|stream| {
                !state
                    .wire
                    .saw_request(stream, RequestKindAtom::BindingsCertUpload)
                    || state
                        .session_auth
                        .authority_uploaded(stream_authority(stream))
            })
    })
}

#[system_spec(
    model_cases(system_model_cases),
    subsystems(
        "manager_runtime",
        "session_transport",
        "session_auth",
        "wire_protocol",
        "command_protocol",
        "deploy",
        "supervision",
        "rpc",
        "plugin_platform",
        "shutdown_flow"
    )
)]
impl TransitionSystem for SystemSpec {
    type State = SystemState;
    type Action = ConcurrentAction<SystemAtomicAction>;

    fn name(&self) -> &'static str {
        "system"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        <Self as ConcurrentTransitionSystem>::initial_states(self)
    }

    fn actions(&self) -> Vec<Self::Action> {
        self.atomic_actions()
            .into_iter()
            .map(ConcurrentAction::from_atomic)
            .collect()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.synthesized_transition(state, action)
    }

    fn successors(&self, state: &Self::State) -> Vec<(Self::Action, Self::State)> {
        self.synthesized_successors(state)
    }

    fn successors_constrained(
        &self,
        state: &Self::State,
        action_allowed: &dyn Fn(&Self::Action, &Self::State) -> bool,
    ) -> Vec<(Self::Action, Self::State)> {
        self.synthesized_successors_filtered(state, action_allowed)
    }
}

impl ProtocolConformanceSpec for SystemSpec {
    type ExpectedOutput = Vec<SystemEffect>;
    type SummaryState = SystemState;
    type SummaryOutput = Vec<SystemEffect>;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        let mut effects = next
            .map(|next_state| {
                action
                    .atoms()
                    .iter()
                    .flat_map(|atom| expected_effects_for_atomic(prev, atom, next_state))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if next.is_some_and(|state| {
            !matches!(prev.shutdown.phase, ShutdownPhase::Completed)
                && matches!(state.shutdown.phase, ShutdownPhase::Completed)
        }) {
            effects.push(SystemEffect::ShutdownComplete);
        }
        effects
    }

    fn abstract_state(&self, observed: &Self::SummaryState) -> Self::State {
        observed.clone()
    }

    fn abstract_output(&self, observed: &Self::SummaryOutput) -> Self::ExpectedOutput {
        observed.clone()
    }
}

fn expected_effects_for_atomic(
    _prev: &SystemState,
    action: &SystemAtomicAction,
    _next: &SystemState,
) -> Vec<SystemEffect> {
    match action {
        SystemAtomicAction::SessionAuth(SessionAuthAction::RejectUnauthorized(stream, kind)) => {
            vec![SystemEffect::AuthorizationRejected(*stream, *kind)]
        }
        SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(stream, event)) => {
            vec![SystemEffect::CommandEvent(*stream, *event)]
        }
        SystemAtomicAction::Wire(WireProtocolAction::LogsChunk(stream, chunk)) => {
            vec![SystemEffect::LogChunk(*stream, *chunk)]
        }
        SystemAtomicAction::Wire(WireProtocolAction::LogsEnd(stream)) => {
            vec![SystemEffect::LogsEnd(*stream)]
        }
        SystemAtomicAction::Wire(wire_action) => request_kind_for_wire_action(*wire_action)
            .map(|(stream, kind)| vec![SystemEffect::Response(stream, kind)])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn request_kind_for_wire_action(
    action: WireProtocolAction,
) -> Option<(StreamAtom, RequestKindAtom)> {
    match action {
        WireProtocolAction::HelloNegotiate(stream) => {
            Some((stream, RequestKindAtom::HelloNegotiate))
        }
        WireProtocolAction::DeployPrepare(stream) => Some((stream, RequestKindAtom::DeployPrepare)),
        WireProtocolAction::ArtifactPush(stream) => Some((stream, RequestKindAtom::ArtifactPush)),
        WireProtocolAction::ArtifactCommit(stream) => {
            Some((stream, RequestKindAtom::ArtifactCommit))
        }
        WireProtocolAction::CommandStart(stream) => Some((stream, RequestKindAtom::CommandStart)),
        WireProtocolAction::StateRequest(stream) => Some((stream, RequestKindAtom::StateRequest)),
        WireProtocolAction::ServicesList(stream) => Some((stream, RequestKindAtom::ServicesList)),
        WireProtocolAction::CommandCancel(stream) => Some((stream, RequestKindAtom::CommandCancel)),
        WireProtocolAction::LogsRequest(stream) => Some((stream, RequestKindAtom::LogsRequest)),
        WireProtocolAction::RpcInvoke(stream) => Some((stream, RequestKindAtom::RpcInvoke)),
        WireProtocolAction::BindingsCertUpload(stream) => {
            Some((stream, RequestKindAtom::BindingsCertUpload))
        }
        WireProtocolAction::CommandEvent(_, _)
        | WireProtocolAction::LogsChunk(_, _)
        | WireProtocolAction::LogsEnd(_) => None,
    }
}

#[nirvash_macros::formal_tests(
    spec = SystemSpec,
    composition = composition
)]
const _: () = ();

fn session_action_allowed(prev: &SystemState, action: &SessionTransportAction) -> bool {
    match action {
        SessionTransportAction::AcceptSession => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        SessionTransportAction::RejectTooMany => {
            prev.session.shutdown_requested || prev.session.at_capacity()
        }
        SessionTransportAction::JoinSession => {
            prev.session.has_active_sessions()
                && !matches!(prev.shutdown.phase, ShutdownPhase::Completed)
        }
        SessionTransportAction::BeginShutdown => matches!(
            prev.manager.phase,
            ManagerRuntimePhase::Listening | ManagerRuntimePhase::ShutdownRequested
        ),
    }
}

fn session_auth_action_allowed(prev: &SystemState, action: &SessionAuthAction) -> bool {
    match action {
        SessionAuthAction::AcceptSession(_) => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        SessionAuthAction::AuthenticateAdmin(_)
        | SessionAuthAction::AuthenticateClient(_)
        | SessionAuthAction::AuthenticateUnknown(_) => prev.session.has_active_sessions(),
        SessionAuthAction::AuthorizeAdmin(_, _) => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        SessionAuthAction::AuthorizeClient(_, kind) => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
                && (matches!(kind, RequestKindAtom::HelloNegotiate)
                    || (matches!(kind, RequestKindAtom::RpcInvoke)
                        && (prev
                            .session_auth
                            .authority_uploaded(RemoteAuthorityAtom::Edge0)
                            || prev
                                .session_auth
                                .authority_uploaded(RemoteAuthorityAtom::Edge1))))
        }
        SessionAuthAction::RejectUnauthorized(_, _) => true,
        SessionAuthAction::ReadTimeout(_) | SessionAuthAction::CloseStream(_) => {
            !matches!(prev.shutdown.phase, ShutdownPhase::Completed)
        }
        SessionAuthAction::UploadClientAuthority(_) => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
    }
}

fn wire_stream(action: WireProtocolAction) -> StreamAtom {
    match action {
        WireProtocolAction::HelloNegotiate(stream)
        | WireProtocolAction::DeployPrepare(stream)
        | WireProtocolAction::ArtifactPush(stream)
        | WireProtocolAction::ArtifactCommit(stream)
        | WireProtocolAction::CommandStart(stream)
        | WireProtocolAction::CommandEvent(stream, _)
        | WireProtocolAction::StateRequest(stream)
        | WireProtocolAction::ServicesList(stream)
        | WireProtocolAction::CommandCancel(stream)
        | WireProtocolAction::LogsRequest(stream)
        | WireProtocolAction::LogsChunk(stream, _)
        | WireProtocolAction::LogsEnd(stream)
        | WireProtocolAction::RpcInvoke(stream)
        | WireProtocolAction::BindingsCertUpload(stream) => stream,
    }
}

fn wire_request_kind(action: WireProtocolAction) -> Option<RequestKindAtom> {
    match action {
        WireProtocolAction::HelloNegotiate(_) => Some(RequestKindAtom::HelloNegotiate),
        WireProtocolAction::DeployPrepare(_) => Some(RequestKindAtom::DeployPrepare),
        WireProtocolAction::ArtifactPush(_) => Some(RequestKindAtom::ArtifactPush),
        WireProtocolAction::ArtifactCommit(_) => Some(RequestKindAtom::ArtifactCommit),
        WireProtocolAction::CommandStart(_) => Some(RequestKindAtom::CommandStart),
        WireProtocolAction::CommandEvent(_, _) => Some(RequestKindAtom::CommandEvent),
        WireProtocolAction::StateRequest(_) => Some(RequestKindAtom::StateRequest),
        WireProtocolAction::ServicesList(_) => Some(RequestKindAtom::ServicesList),
        WireProtocolAction::CommandCancel(_) => Some(RequestKindAtom::CommandCancel),
        WireProtocolAction::LogsRequest(_) => Some(RequestKindAtom::LogsRequest),
        WireProtocolAction::LogsChunk(_, _) => Some(RequestKindAtom::LogsChunk),
        WireProtocolAction::LogsEnd(_) => Some(RequestKindAtom::LogsEnd),
        WireProtocolAction::RpcInvoke(_) => Some(RequestKindAtom::RpcInvoke),
        WireProtocolAction::BindingsCertUpload(_) => Some(RequestKindAtom::BindingsCertUpload),
    }
}

fn request_kind_requires_authorization(kind: RequestKindAtom) -> bool {
    !matches!(
        kind,
        RequestKindAtom::HelloNegotiate
            | RequestKindAtom::CommandEvent
            | RequestKindAtom::LogsChunk
            | RequestKindAtom::LogsEnd
    )
}

fn stream_authority(stream: StreamAtom) -> RemoteAuthorityAtom {
    match stream {
        StreamAtom::Stream0 => RemoteAuthorityAtom::Edge0,
        StreamAtom::Stream1 => RemoteAuthorityAtom::Edge1,
    }
}

fn wire_action_allowed(prev: &SystemState, action: &WireProtocolAction) -> bool {
    let stream = wire_stream(*action);
    match action {
        WireProtocolAction::HelloNegotiate(_) => {
            prev.session.has_active_sessions()
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        WireProtocolAction::DeployPrepare(_)
        | WireProtocolAction::ArtifactPush(_)
        | WireProtocolAction::ArtifactCommit(_)
        | WireProtocolAction::CommandStart(_)
        | WireProtocolAction::StateRequest(_)
        | WireProtocolAction::ServicesList(_)
        | WireProtocolAction::CommandCancel(_)
        | WireProtocolAction::BindingsCertUpload(_) => {
            let kind = wire_request_kind(*action).expect("request kind should exist");
            prev.session_auth.stream_authorized(stream, kind)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        WireProtocolAction::RpcInvoke(_) => {
            prev.session_auth
                .stream_authorized(stream, RequestKindAtom::RpcInvoke)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        WireProtocolAction::CommandEvent(_, _) => {
            prev.command.tracked && prev.wire.saw_request(stream, RequestKindAtom::CommandStart)
        }
        WireProtocolAction::LogsRequest(_) => {
            prev.session_auth
                .stream_authorized(stream, RequestKindAtom::LogsRequest)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
                && prev.supervision.service_is_running(ServiceAtom::Service0)
        }
        WireProtocolAction::LogsChunk(_, _) | WireProtocolAction::LogsEnd(_) => {
            prev.wire.logs_acknowledged(stream)
        }
    }
}

fn command_action_allowed(prev: &SystemState, action: &CommandProtocolAction) -> bool {
    match action {
        CommandProtocolAction::Start(_) | CommandProtocolAction::SetRunning => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        _ => true,
    }
}

fn deploy_action_allowed(prev: &SystemState, _action: &DeployAction) -> bool {
    matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
        && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
}

fn supervision_action_allowed(prev: &SystemState, action: &SupervisionAction) -> bool {
    match action {
        SupervisionAction::PrepareEndpoint(_) | SupervisionAction::AdvanceBootstrap(_) => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        SupervisionAction::StartServing(service) => {
            prev.deploy.release_promoted(*service)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        SupervisionAction::RequestStop(_) | SupervisionAction::ReapService(_) => {
            !matches!(prev.shutdown.phase, ShutdownPhase::Idle)
                || matches!(prev.manager.phase, ManagerRuntimePhase::ShutdownRequested)
        }
    }
}

fn rpc_action_allowed(prev: &SystemState, action: &RpcAction) -> bool {
    match action {
        RpcAction::GrantBinding(source) => {
            prev.supervision.service_is_running(*source)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::ResolveLocal(source) => {
            let target = binding_target_service(binding_target_for(*source));
            prev.rpc.binding_allowed(*source)
                && prev.supervision.service_is_ready(target)
                && prev.supervision.service_is_running(target)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::RejectLocal(source) => {
            let target = binding_target_service(binding_target_for(*source));
            ((!prev.rpc.binding_allowed(*source)
                || !prev.supervision.service_is_ready(target)
                || !prev.supervision.service_is_running(target))
                && prev.supervision.service_is_running(*source)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening))
                || !matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::ConnectRemote(source) => {
            prev.supervision.service_is_running(*source)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::InvokeRemote(source) => {
            prev.rpc.binding_allowed(*source)
                && prev.rpc.has_remote_connection_for(*source)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::RejectRemoteInvoke(source) => {
            ((!prev.rpc.binding_allowed(*source) || !prev.rpc.has_remote_connection_for(*source))
                && prev.supervision.service_is_running(*source)
                && matches!(prev.manager.phase, ManagerRuntimePhase::Listening))
                || !matches!(prev.shutdown.phase, ShutdownPhase::Idle)
        }
        RpcAction::CompleteRemoteCall(_) | RpcAction::DisconnectRemote(_) => true,
    }
}

fn plugin_action_allowed(prev: &SystemState, _action: &PluginPlatformAction) -> bool {
    matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
        && matches!(prev.shutdown.phase, ShutdownPhase::Idle)
}

fn shutdown_action_allowed(prev: &SystemState, action: &ShutdownFlowAction) -> bool {
    match action {
        ShutdownFlowAction::ReceiveSignal => {
            matches!(prev.manager.phase, ManagerRuntimePhase::Listening)
        }
        ShutdownFlowAction::StopServicesGraceful => ServiceAtom::bounded_domain()
            .into_vec()
            .into_iter()
            .all(|service| !prev.supervision.service_is_running(service)),
        ShutdownFlowAction::Finalize => {
            ServiceAtom::bounded_domain()
                .into_vec()
                .into_iter()
                .all(|service| !prev.supervision.service_is_running(service))
                && !prev.rpc.has_remote_connection_for(ServiceAtom::Service0)
                && !prev.rpc.has_remote_connection_for(ServiceAtom::Service1)
        }
        _ => true,
    }
}

fn multi_service_state_valid(state: &SystemState) -> bool {
    state_respects_spec(&ManagerRuntimeSpec::new(), &state.manager)
        && state_respects_spec(&SessionTransportSpec::new(), &state.session)
        && state_respects_spec(&SessionAuthSpec::new(), &state.session_auth)
        && state_respects_spec(&WireProtocolSpec::new(), &state.wire)
        && state_respects_spec(&CommandProtocolSpec::new(), &state.command)
        && state_respects_spec(&DeploySpec::new(), &state.deploy)
        && state_respects_spec(&SupervisionSpec::new(), &state.supervision)
        && state_respects_spec(&RpcSpec::new(), &state.rpc)
        && state_respects_spec(&PluginPlatformSpec::new(), &state.plugin)
        && state_respects_spec(&ShutdownFlowSpec::new(), &state.shutdown)
        && running_services_require_promoted_release().eval(state)
        && local_rpc_resolution_requires_ready_target().eval(state)
        && remote_rpc_connections_require_running_owner().eval(state)
        && shutdown_requires_session_gate_and_manager_shutdown().eval(state)
        && active_command_requires_listening_manager().eval(state)
        && dependency_provider_requires_acyclic_plugin_graph().eval(state)
        && non_hello_wire_requests_require_authorized_streams().eval(state)
        && cert_upload_updates_dynamic_authority().eval(state)
}

fn state_respects_spec<T>(spec: &T, state: &T::State) -> bool
where
    T: TemporalSpec + ModelCaseSource,
{
    spec.invariants()
        .iter()
        .all(|predicate| predicate.eval(state))
        && spec
            .model_cases()
            .iter()
            .flat_map(|model_case| model_case.state_constraints().iter())
            .all(|constraint| constraint.eval(state))
}

fn manager_read_resources(
    _action: ManagerRuntimeAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Manager])
}

fn manager_write_resources(
    _action: ManagerRuntimeAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Manager])
}

fn session_read_resources(
    action: SessionTransportAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Manager, Session, Shutdown};

    match action {
        SessionTransportAction::AcceptSession => {
            std::collections::BTreeSet::from([Session, Manager, Shutdown])
        }
        SessionTransportAction::RejectTooMany => std::collections::BTreeSet::from([Session]),
        SessionTransportAction::JoinSession => {
            std::collections::BTreeSet::from([Session, Shutdown])
        }
        SessionTransportAction::BeginShutdown => {
            std::collections::BTreeSet::from([Session, Manager])
        }
    }
}

fn session_write_resources(
    _action: SessionTransportAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Session])
}

fn session_auth_read_resources(
    action: SessionAuthAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Authority, Manager, Session, Shutdown, Stream};

    match action {
        SessionAuthAction::AcceptSession(_)
        | SessionAuthAction::AuthenticateAdmin(_)
        | SessionAuthAction::AuthenticateClient(_)
        | SessionAuthAction::AuthenticateUnknown(_) => {
            std::collections::BTreeSet::from([Session, Manager, Shutdown])
        }
        SessionAuthAction::AuthorizeAdmin(stream, _)
        | SessionAuthAction::AuthorizeClient(stream, _)
        | SessionAuthAction::RejectUnauthorized(stream, _)
        | SessionAuthAction::ReadTimeout(stream)
        | SessionAuthAction::CloseStream(stream) => {
            std::collections::BTreeSet::from([Stream(stream), Session, Shutdown])
        }
        SessionAuthAction::UploadClientAuthority(authority) => {
            std::collections::BTreeSet::from([Authority(authority), Manager, Shutdown])
        }
    }
}

fn session_auth_write_resources(
    action: SessionAuthAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Authority, Session, Stream};

    match action {
        SessionAuthAction::AcceptSession(_)
        | SessionAuthAction::AuthenticateAdmin(_)
        | SessionAuthAction::AuthenticateClient(_)
        | SessionAuthAction::AuthenticateUnknown(_) => std::collections::BTreeSet::from([Session]),
        SessionAuthAction::AuthorizeAdmin(stream, _)
        | SessionAuthAction::AuthorizeClient(stream, _)
        | SessionAuthAction::RejectUnauthorized(stream, _)
        | SessionAuthAction::ReadTimeout(stream)
        | SessionAuthAction::CloseStream(stream) => {
            std::collections::BTreeSet::from([Stream(stream)])
        }
        SessionAuthAction::UploadClientAuthority(authority) => {
            std::collections::BTreeSet::from([Authority(authority)])
        }
    }
}

fn wire_read_resources(
    action: WireProtocolAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Authority, Command, Manager, Session, Shutdown, Stream};

    let mut resources = std::collections::BTreeSet::from([Stream(wire_stream(action)), Session]);
    if matches!(
        action,
        WireProtocolAction::DeployPrepare(_)
            | WireProtocolAction::ArtifactPush(_)
            | WireProtocolAction::ArtifactCommit(_)
            | WireProtocolAction::CommandStart(_)
            | WireProtocolAction::StateRequest(_)
            | WireProtocolAction::ServicesList(_)
            | WireProtocolAction::CommandCancel(_)
            | WireProtocolAction::LogsRequest(_)
            | WireProtocolAction::RpcInvoke(_)
            | WireProtocolAction::BindingsCertUpload(_)
    ) {
        resources.insert(Manager);
        resources.insert(Shutdown);
    }
    if matches!(
        action,
        WireProtocolAction::CommandStart(_)
            | WireProtocolAction::CommandEvent(_, _)
            | WireProtocolAction::CommandCancel(_)
    ) {
        resources.insert(Command);
    }
    if matches!(action, WireProtocolAction::BindingsCertUpload(_)) {
        resources.insert(Authority(stream_authority(wire_stream(action))));
    }
    resources
}

fn wire_write_resources(
    action: WireProtocolAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Authority, Command, Stream};

    let mut resources = std::collections::BTreeSet::from([Stream(wire_stream(action))]);
    if matches!(
        action,
        WireProtocolAction::CommandStart(_)
            | WireProtocolAction::CommandEvent(_, _)
            | WireProtocolAction::CommandCancel(_)
    ) {
        resources.insert(Command);
    }
    if matches!(action, WireProtocolAction::BindingsCertUpload(_)) {
        resources.insert(Authority(stream_authority(wire_stream(action))));
    }
    resources
}

fn command_read_resources(
    action: &CommandProtocolAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Command, Manager, Shutdown};

    match action {
        CommandProtocolAction::Start(_) | CommandProtocolAction::SetRunning => {
            std::collections::BTreeSet::from([Command, Manager, Shutdown])
        }
        _ => std::collections::BTreeSet::from([Command]),
    }
}

fn command_write_resources(
    _action: &CommandProtocolAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Command])
}

fn deploy_service(action: DeployAction) -> ServiceAtom {
    match action {
        DeployAction::AdvanceUpload(service)
        | DeployAction::CommitUpload(service)
        | DeployAction::AdvanceRelease(service)
        | DeployAction::SetRestartPolicy(service)
        | DeployAction::TriggerRollback(service)
        | DeployAction::FinishRollback(service) => service,
    }
}

fn deploy_read_resources(action: DeployAction) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Deploy, Manager, Service, Shutdown};

    let service = deploy_service(action);
    std::collections::BTreeSet::from([Deploy(service), Service(service), Manager, Shutdown])
}

fn deploy_write_resources(action: DeployAction) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Deploy, Service};

    let service = deploy_service(action);
    std::collections::BTreeSet::from([Deploy(service), Service(service)])
}

fn supervision_service(action: SupervisionAction) -> ServiceAtom {
    match action {
        SupervisionAction::PrepareEndpoint(service)
        | SupervisionAction::AdvanceBootstrap(service)
        | SupervisionAction::StartServing(service)
        | SupervisionAction::RequestStop(service)
        | SupervisionAction::ReapService(service) => service,
    }
}

fn supervision_read_resources(
    action: SupervisionAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Deploy, Manager, Runner, Service, Shutdown};

    let service = supervision_service(action);
    std::collections::BTreeSet::from([
        Service(service),
        Runner(service_runner(service)),
        Deploy(service),
        Manager,
        Shutdown,
    ])
}

fn supervision_write_resources(
    action: SupervisionAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Runner, Service};

    let service = supervision_service(action);
    std::collections::BTreeSet::from([Service(service), Runner(service_runner(service))])
}

fn rpc_read_resources(action: RpcAction) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Manager, Shutdown};

    let mut resources = rpc_write_resources(action);
    if matches!(
        action,
        RpcAction::GrantBinding(_)
            | RpcAction::ResolveLocal(_)
            | RpcAction::RejectLocal(_)
            | RpcAction::ConnectRemote(_)
            | RpcAction::InvokeRemote(_)
            | RpcAction::RejectRemoteInvoke(_)
    ) {
        resources.insert(Manager);
        resources.insert(Shutdown);
    }
    resources
}

fn rpc_write_resources(action: RpcAction) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{RpcCall, RpcConnection, Service};

    let source = rpc_source(action);
    let target = binding_target_service(binding_target_for(source));
    let connection = rpc_connection_for_source(source);
    let call = rpc_call_for_source(source);
    match action {
        RpcAction::GrantBinding(_) => {
            std::collections::BTreeSet::from([Service(source), Service(target)])
        }
        RpcAction::ResolveLocal(_) | RpcAction::RejectLocal(_) => {
            std::collections::BTreeSet::from([Service(source), Service(target), RpcCall(call)])
        }
        RpcAction::ConnectRemote(_) | RpcAction::DisconnectRemote(_) => {
            std::collections::BTreeSet::from([Service(source), RpcConnection(connection)])
        }
        RpcAction::InvokeRemote(_)
        | RpcAction::RejectRemoteInvoke(_)
        | RpcAction::CompleteRemoteCall(_) => std::collections::BTreeSet::from([
            Service(source),
            Service(target),
            RpcConnection(connection),
            RpcCall(call),
        ]),
    }
}

fn plugin_read_resources(
    _action: &PluginPlatformAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Manager, Plugin, Shutdown};

    std::collections::BTreeSet::from([Plugin, Manager, Shutdown])
}

fn plugin_write_resources(
    _action: &PluginPlatformAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Plugin])
}

fn shutdown_read_resources(
    action: ShutdownFlowAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    use SystemResourceKey::{Manager, RpcConnection, Runner, Service, Session, Shutdown};

    match action {
        ShutdownFlowAction::ReceiveSignal => std::collections::BTreeSet::from([Shutdown, Manager]),
        ShutdownFlowAction::StopAccepting | ShutdownFlowAction::DrainSessions => {
            std::collections::BTreeSet::from([Shutdown, Session])
        }
        ShutdownFlowAction::StopServicesGraceful | ShutdownFlowAction::StopServicesForced => {
            std::collections::BTreeSet::from([
                Shutdown,
                Service(ServiceAtom::Service0),
                Service(ServiceAtom::Service1),
                Runner(RunnerAtom::Runner0),
                Runner(RunnerAtom::Runner1),
            ])
        }
        ShutdownFlowAction::Finalize => std::collections::BTreeSet::from([
            Shutdown,
            Service(ServiceAtom::Service0),
            Service(ServiceAtom::Service1),
            RpcConnection(RpcConnectionAtom::Connection0),
            RpcConnection(RpcConnectionAtom::Connection1),
        ]),
        ShutdownFlowAction::StopMaintenance => std::collections::BTreeSet::from([Shutdown]),
    }
}

fn shutdown_write_resources(
    _action: ShutdownFlowAction,
) -> std::collections::BTreeSet<SystemResourceKey> {
    std::collections::BTreeSet::from([SystemResourceKey::Shutdown])
}

fn rpc_source(action: RpcAction) -> ServiceAtom {
    match action {
        RpcAction::GrantBinding(source)
        | RpcAction::ResolveLocal(source)
        | RpcAction::RejectLocal(source)
        | RpcAction::ConnectRemote(source)
        | RpcAction::InvokeRemote(source)
        | RpcAction::RejectRemoteInvoke(source)
        | RpcAction::CompleteRemoteCall(source)
        | RpcAction::DisconnectRemote(source) => source,
    }
}

fn rpc_connection_for_source(source: ServiceAtom) -> RpcConnectionAtom {
    match source {
        ServiceAtom::Service0 => RpcConnectionAtom::Connection0,
        ServiceAtom::Service1 => RpcConnectionAtom::Connection1,
    }
}

fn rpc_call_for_source(source: ServiceAtom) -> RpcCallAtom {
    match source {
        ServiceAtom::Service0 => RpcCallAtom::Call0,
        ServiceAtom::Service1 => RpcCallAtom::Call1,
    }
}

fn boot_to_listening_atom_allowed(action: &SystemAtomicAction) -> bool {
    matches!(
        action,
        SystemAtomicAction::Manager(ManagerRuntimeAction::LoadExistingConfig)
            | SystemAtomicAction::Manager(ManagerRuntimeAction::RunPluginGcSucceeded)
            | SystemAtomicAction::Manager(ManagerRuntimeAction::RunBootRestoreSucceeded)
    )
}

fn session_auth_and_authorize_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::DeployPrepare,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session1,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateClient(
                    SessionAtom::Session1
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::UploadClientAuthority(
                    RemoteAuthorityAtom::Edge0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::RpcInvoke,
                ))
        )
}

fn hello_negotiation_and_limits_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateClient(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::HelloNegotiate,
                ))
                | SystemAtomicAction::Wire(
                    WireProtocolAction::HelloNegotiate(StreamAtom::Stream0,)
                )
        )
}

fn deploy_upload_and_commit_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::DeployPrepare,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream1,
                    RequestKindAtom::ArtifactCommit,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::DeployPrepare(StreamAtom::Stream0,))
                | SystemAtomicAction::Wire(
                    WireProtocolAction::ArtifactCommit(StreamAtom::Stream1,)
                )
                | SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0))
                | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0))
                | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0))
        )
}

fn command_start_event_flow_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::CommandStart,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::CommandStart(StreamAtom::Stream0,))
                | SystemAtomicAction::Command(CommandProtocolAction::Start(CommandKind::Deploy,))
                | SystemAtomicAction::Command(CommandProtocolAction::SetRunning)
                | SystemAtomicAction::Command(CommandProtocolAction::MarkSpawned)
                | SystemAtomicAction::Command(CommandProtocolAction::FinishSucceeded)
                | SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
                    StreamAtom::Stream0,
                    CommandEventAtom::Accepted,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
                    StreamAtom::Stream0,
                    CommandEventAtom::Succeeded,
                ))
        )
}

fn state_request_and_cancel_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::StateRequest,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream1,
                    RequestKindAtom::CommandCancel,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::StateRequest(StreamAtom::Stream0,))
                | SystemAtomicAction::Wire(WireProtocolAction::CommandCancel(StreamAtom::Stream1,))
                | SystemAtomicAction::Command(CommandProtocolAction::Start(CommandKind::Deploy,))
                | SystemAtomicAction::Command(CommandProtocolAction::SetRunning)
                | SystemAtomicAction::Command(CommandProtocolAction::SnapshotRunning)
                | SystemAtomicAction::Command(CommandProtocolAction::RequestCancel)
        )
}

fn services_list_merge_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream0,
                    RequestKindAtom::ServicesList,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::ServicesList(StreamAtom::Stream0,))
        )
        || matches!(
            action,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0,))
                | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0,))
                | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0,))
                | SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                    ServiceAtom::Service0,
                ))
                | SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                    ServiceAtom::Service0,
                ))
                | SystemAtomicAction::Supervision(SupervisionAction::StartServing(
                    ServiceAtom::Service0,
                ))
        )
}

fn logs_request_snapshot_and_follow_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream1,
                    RequestKindAtom::LogsRequest,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::LogsRequest(StreamAtom::Stream1,))
                | SystemAtomicAction::Wire(WireProtocolAction::LogsChunk(
                    StreamAtom::Stream1,
                    LogChunkAtom::Chunk0,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::LogsEnd(StreamAtom::Stream1,))
        )
        || matches!(
            action,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0,))
                | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0,))
                | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0,))
                | SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                    ServiceAtom::Service0
                ))
                | SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                    ServiceAtom::Service0
                ))
                | SystemAtomicAction::Supervision(SupervisionAction::StartServing(
                    ServiceAtom::Service0
                ))
        )
}

#[allow(dead_code)]
fn bindings_cert_upload_updates_authorization_atom_allowed(action: &SystemAtomicAction) -> bool {
    boot_to_listening_atom_allowed(action)
        || matches!(
            action,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                    SessionAtom::Session0
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                    StreamAtom::Stream1,
                    RequestKindAtom::BindingsCertUpload,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::BindingsCertUpload(
                    StreamAtom::Stream1,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                    SessionAtom::Session1,
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateClient(
                    SessionAtom::Session1
                ))
                | SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeClient(
                    StreamAtom::Stream0,
                    RequestKindAtom::RpcInvoke,
                ))
                | SystemAtomicAction::Wire(WireProtocolAction::RpcInvoke(StreamAtom::Stream0,))
        )
}

#[allow(dead_code)]
fn parallel_deploy_and_start_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    match action {
        SystemAtomicAction::Deploy(deploy_action) => matches!(
            deploy_action,
            DeployAction::AdvanceUpload(_)
                | DeployAction::CommitUpload(_)
                | DeployAction::AdvanceRelease(_)
        ),
        SystemAtomicAction::Supervision(supervision_action) => matches!(
            supervision_action,
            SupervisionAction::PrepareEndpoint(_)
                | SupervisionAction::AdvanceBootstrap(_)
                | SupervisionAction::StartServing(_)
        ),
        _ => false,
    }
}

#[allow(dead_code)]
fn service_scoped_rollback_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    match action {
        SystemAtomicAction::Deploy(deploy_action) => match deploy_action {
            DeployAction::AdvanceUpload(_)
            | DeployAction::CommitUpload(_)
            | DeployAction::AdvanceRelease(_) => true,
            DeployAction::SetRestartPolicy(ServiceAtom::Service0)
            | DeployAction::TriggerRollback(ServiceAtom::Service0)
            | DeployAction::FinishRollback(ServiceAtom::Service0) => true,
            DeployAction::SetRestartPolicy(ServiceAtom::Service1)
            | DeployAction::TriggerRollback(ServiceAtom::Service1)
            | DeployAction::FinishRollback(ServiceAtom::Service1) => false,
        },
        SystemAtomicAction::Supervision(supervision_action) => matches!(
            supervision_action,
            SupervisionAction::PrepareEndpoint(ServiceAtom::Service1)
                | SupervisionAction::AdvanceBootstrap(ServiceAtom::Service1)
                | SupervisionAction::StartServing(ServiceAtom::Service1)
                | SupervisionAction::RequestStop(ServiceAtom::Service0)
                | SupervisionAction::ReapService(ServiceAtom::Service0)
        ),
        _ => false,
    }
}

#[allow(dead_code)]
fn local_rpc_happy_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    match action {
        SystemAtomicAction::Deploy(deploy_action) => matches!(
            deploy_action,
            DeployAction::AdvanceUpload(_)
                | DeployAction::CommitUpload(_)
                | DeployAction::AdvanceRelease(_)
        ),
        SystemAtomicAction::Supervision(supervision_action) => matches!(
            supervision_action,
            SupervisionAction::PrepareEndpoint(_)
                | SupervisionAction::AdvanceBootstrap(_)
                | SupervisionAction::StartServing(_)
        ),
        SystemAtomicAction::Rpc(RpcAction::GrantBinding(ServiceAtom::Service0))
        | SystemAtomicAction::Rpc(RpcAction::ResolveLocal(ServiceAtom::Service0)) => true,
        _ => false,
    }
}

#[allow(dead_code)]
fn local_rpc_denied_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    matches!(
        action,
        SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0))
            | SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::StartServing(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0))
    )
}

#[allow(dead_code)]
fn remote_rpc_connection_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    matches!(
        action,
        SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0))
            | SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::StartServing(
                ServiceAtom::Service0
            ))
            | SystemAtomicAction::Rpc(RpcAction::GrantBinding(ServiceAtom::Service0))
            | SystemAtomicAction::Rpc(RpcAction::ConnectRemote(ServiceAtom::Service0))
            | SystemAtomicAction::Rpc(RpcAction::InvokeRemote(ServiceAtom::Service0))
            | SystemAtomicAction::Rpc(RpcAction::CompleteRemoteCall(ServiceAtom::Service0))
            | SystemAtomicAction::Rpc(RpcAction::DisconnectRemote(ServiceAtom::Service0))
    )
}

#[allow(dead_code)]
fn shutdown_blocks_new_rpc_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    match action {
        SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown)
        | SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown)
        | SystemAtomicAction::Session(SessionTransportAction::BeginShutdown)
        | SystemAtomicAction::Shutdown(_) => true,
        SystemAtomicAction::Deploy(deploy_action) => matches!(
            deploy_action,
            DeployAction::AdvanceUpload(ServiceAtom::Service0)
                | DeployAction::CommitUpload(ServiceAtom::Service0)
                | DeployAction::AdvanceRelease(ServiceAtom::Service0)
        ),
        SystemAtomicAction::Supervision(supervision_action) => matches!(
            supervision_action,
            SupervisionAction::PrepareEndpoint(ServiceAtom::Service0)
                | SupervisionAction::AdvanceBootstrap(ServiceAtom::Service0)
                | SupervisionAction::StartServing(ServiceAtom::Service0)
                | SupervisionAction::RequestStop(ServiceAtom::Service0)
        ),
        SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0)) => true,
        _ => false,
    }
}

fn graceful_shutdown_and_force_fallback_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    match action {
        SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown)
        | SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown)
        | SystemAtomicAction::Session(SessionTransportAction::BeginShutdown)
        | SystemAtomicAction::Shutdown(_) => true,
        SystemAtomicAction::Deploy(deploy_action) => matches!(
            deploy_action,
            DeployAction::AdvanceUpload(ServiceAtom::Service0)
                | DeployAction::CommitUpload(ServiceAtom::Service0)
                | DeployAction::AdvanceRelease(ServiceAtom::Service0)
        ),
        SystemAtomicAction::Supervision(supervision_action) => matches!(
            supervision_action,
            SupervisionAction::PrepareEndpoint(ServiceAtom::Service0)
                | SupervisionAction::AdvanceBootstrap(ServiceAtom::Service0)
                | SupervisionAction::StartServing(ServiceAtom::Service0)
                | SupervisionAction::RequestStop(ServiceAtom::Service0)
                | SupervisionAction::ReapService(ServiceAtom::Service0)
        ),
        _ => false,
    }
}

fn maintenance_reap_and_idle_tick_atom_allowed(action: &SystemAtomicAction) -> bool {
    if boot_to_listening_atom_allowed(action) {
        return true;
    }

    matches!(
        action,
        SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0))
            | SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0))
            | SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(
                ServiceAtom::Service0,
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(
                ServiceAtom::Service0,
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::StartServing(
                ServiceAtom::Service0,
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::RequestStop(
                ServiceAtom::Service0,
            ))
            | SystemAtomicAction::Supervision(SupervisionAction::ReapService(
                ServiceAtom::Service0,
            ))
            | SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown)
            | SystemAtomicAction::Session(SessionTransportAction::BeginShutdown)
            | SystemAtomicAction::Shutdown(ShutdownFlowAction::ReceiveSignal)
            | SystemAtomicAction::Shutdown(ShutdownFlowAction::StopAccepting)
            | SystemAtomicAction::Shutdown(ShutdownFlowAction::DrainSessions)
    )
}

#[cfg(test)]
mod tests {
    use nirvash_core::ModelChecker;

    use super::*;

    fn model_case(
        spec: &SystemSpec,
        label: &str,
    ) -> ModelCase<SystemState, ConcurrentAction<SystemAtomicAction>> {
        spec.model_cases()
            .into_iter()
            .find(|model_case| model_case.label() == label)
            .expect("model case should exist")
    }

    fn reachable_snapshot_for_case(
        spec: &SystemSpec,
        label: &str,
    ) -> nirvash_core::ReachableGraphSnapshot<SystemState, ConcurrentAction<SystemAtomicAction>>
    {
        ModelChecker::for_case(spec, model_case(spec, label))
            .full_reachable_graph_snapshot()
            .expect("snapshot should build")
    }

    fn listening_state(spec: &SystemSpec) -> SystemState {
        spec.initial_state()
    }

    fn step(spec: &SystemSpec, state: &SystemState, action: SystemAtomicAction) -> SystemState {
        spec.atomic_transition(state, &action)
            .unwrap_or_else(|| panic!("action should succeed: {action:?}"))
    }

    fn deploy_and_start_service(
        spec: &SystemSpec,
        state: &SystemState,
        service: ServiceAtom,
    ) -> SystemState {
        let state = step(
            spec,
            state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::CommitUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::PrepareEndpoint(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::AdvanceBootstrap(service)),
        );
        step(
            spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::StartServing(service)),
        )
    }

    fn deploy_release_only(
        spec: &SystemSpec,
        state: &SystemState,
        service: ServiceAtom,
    ) -> SystemState {
        let state = step(
            spec,
            state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::CommitUpload(service)),
        );
        let state = step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
        );
        step(
            spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(service)),
        )
    }

    #[test]
    fn boot_case_reaches_listening_with_completed_boot_tasks() {
        let spec = SystemSpec::new();
        let snapshot = reachable_snapshot_for_case(&spec, "boot_gc_and_restore");

        assert!(snapshot.states.iter().any(|state| {
            matches!(state.manager.phase, ManagerRuntimePhase::Listening)
                && matches!(
                    state.manager.plugin_gc,
                    crate::manager_runtime::TaskState::Succeeded
                )
                && matches!(
                    state.manager.boot_restore,
                    crate::manager_runtime::TaskState::Succeeded
                )
        }));
    }

    #[test]
    fn hello_case_records_response_without_prior_authorization() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession),
        );
        let action = ConcurrentAction::from_atomic(SystemAtomicAction::Wire(
            WireProtocolAction::HelloNegotiate(StreamAtom::Stream0),
        ));
        let next = spec
            .transition(&state, &action)
            .expect("hello.negotiate should be accepted on an active session");

        assert!(
            next.wire
                .saw_request(StreamAtom::Stream0, RequestKindAtom::HelloNegotiate)
        );
        assert_eq!(
            spec.expected_output(&state, &action, Some(&next)),
            vec![SystemEffect::Response(
                StreamAtom::Stream0,
                RequestKindAtom::HelloNegotiate,
            )]
        );
    }

    #[test]
    fn session_auth_case_authorizes_admin_and_client_streams() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session1,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::UploadClientAuthority(
                RemoteAuthorityAtom::Edge0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::DeployPrepare,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateClient(
                SessionAtom::Session1,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeClient(
                StreamAtom::Stream0,
                RequestKindAtom::RpcInvoke,
            )),
        );

        assert!(
            state
                .session_auth
                .stream_authorized(StreamAtom::Stream0, RequestKindAtom::DeployPrepare)
        );
        assert!(
            state
                .session_auth
                .stream_authorized(StreamAtom::Stream0, RequestKindAtom::RpcInvoke)
        );
    }

    #[test]
    fn logs_case_reaches_ack_chunk_and_end() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream1,
                RequestKindAtom::LogsRequest,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::LogsRequest(StreamAtom::Stream1)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::LogsChunk(
                StreamAtom::Stream1,
                LogChunkAtom::Chunk0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::LogsEnd(StreamAtom::Stream1)),
        );

        assert!(state.wire.logs_acknowledged(StreamAtom::Stream1));
        assert!(state.wire.log_stream_ended(StreamAtom::Stream1));
    }

    #[test]
    fn deploy_upload_and_commit_case_promotes_release() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::DeployPrepare,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::DeployPrepare(StreamAtom::Stream0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::CommitUpload(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream1,
                RequestKindAtom::ArtifactCommit,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::ArtifactCommit(StreamAtom::Stream1)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::AdvanceRelease(ServiceAtom::Service0)),
        );

        assert!(state.deploy.release_promoted(ServiceAtom::Service0));
        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream1, RequestKindAtom::ArtifactCommit)
        );
    }

    #[test]
    fn bindings_cert_case_records_authority_and_rpc_request() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream1,
                RequestKindAtom::BindingsCertUpload,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::BindingsCertUpload(StreamAtom::Stream1)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session1,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateClient(
                SessionAtom::Session1,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeClient(
                StreamAtom::Stream0,
                RequestKindAtom::RpcInvoke,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::RpcInvoke(StreamAtom::Stream0)),
        );

        assert!(
            state
                .session_auth
                .authority_uploaded(RemoteAuthorityAtom::Edge1)
        );
        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream0, RequestKindAtom::RpcInvoke)
        );
    }

    #[test]
    fn command_start_event_flow_case_records_wire_events() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::CommandStart,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::CommandStart(StreamAtom::Stream0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::Start(CommandKind::Deploy)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
                StreamAtom::Stream0,
                CommandEventAtom::Accepted,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::SetRunning),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::MarkSpawned),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::FinishSucceeded),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::CommandEvent(
                StreamAtom::Stream0,
                CommandEventAtom::Succeeded,
            )),
        );

        assert!(
            state
                .wire
                .saw_command_event(StreamAtom::Stream0, CommandEventAtom::Accepted)
        );
        assert!(
            state
                .wire
                .saw_command_event(StreamAtom::Stream0, CommandEventAtom::Succeeded)
        );
        assert_eq!(
            state.command.lifecycle_state,
            Some(CommandLifecycleState::Succeeded)
        );
    }

    #[test]
    fn parallel_deploy_and_start_case_contains_parallel_steps() {
        let spec = SystemSpec::new();
        let state = listening_state(&spec);
        let parallel_upload = ConcurrentAction::new(vec![
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0)),
            SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service1)),
        ])
        .expect("parallel action should build");

        assert!(
            spec.synthesized_transition(&state, &parallel_upload)
                .is_some()
        );

        let state = deploy_and_start_service(&spec, &state, ServiceAtom::Service0);
        let state = deploy_and_start_service(&spec, &state, ServiceAtom::Service1);

        assert!(state.deploy.release_promoted(ServiceAtom::Service0));
        assert!(state.deploy.release_promoted(ServiceAtom::Service1));
        assert!(state.supervision.service_is_running(ServiceAtom::Service0));
        assert!(state.supervision.service_is_running(ServiceAtom::Service1));
    }

    #[test]
    fn service_scoped_rollback_case_preserves_other_service() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service1);
        let state = deploy_release_only(&spec, &state, ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::SetRestartPolicy(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Deploy(DeployAction::TriggerRollback(ServiceAtom::Service0)),
        );

        assert!(state.deploy.rollback_pending(ServiceAtom::Service0));
        assert!(state.supervision.service_is_running(ServiceAtom::Service1));
    }

    #[test]
    fn local_rpc_happy_path_case_resolves_local_call() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = deploy_and_start_service(&spec, &state, ServiceAtom::Service1);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::GrantBinding(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::ResolveLocal(ServiceAtom::Service0)),
        );

        assert!(state.rpc.has_local_resolution_for(ServiceAtom::Service0));
        assert!(state.supervision.service_is_running(ServiceAtom::Service1));
    }

    #[test]
    fn local_rpc_denied_case_reaches_rejection() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0)),
        );

        assert!(state.rpc.has_denied_local_call_for(ServiceAtom::Service0));
    }

    #[test]
    fn state_request_and_cancel_case_marks_cancel_requested() {
        let spec = SystemSpec::new();
        let state = step(
            &spec,
            &listening_state(&spec),
            SystemAtomicAction::Command(CommandProtocolAction::Start(CommandKind::Deploy)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::SetRunning),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::StateRequest,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream1,
                RequestKindAtom::CommandCancel,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::StateRequest(StreamAtom::Stream0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::SnapshotRunning),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::CommandCancel(StreamAtom::Stream1)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Command(CommandProtocolAction::RequestCancel),
        );

        assert!(state.command.cancel_requested);
        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream0, RequestKindAtom::StateRequest)
        );
        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream1, RequestKindAtom::CommandCancel)
        );
    }

    #[test]
    fn remote_rpc_connection_lifecycle_case_completes_and_disconnects() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = deploy_and_start_service(&spec, &state, ServiceAtom::Service1);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::GrantBinding(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::ConnectRemote(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::InvokeRemote(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::CompleteRemoteCall(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::DisconnectRemote(ServiceAtom::Service0)),
        );

        assert!(
            state
                .rpc
                .has_completed_remote_call_for(ServiceAtom::Service0)
        );
        assert!(!state.rpc.has_remote_connection_for(ServiceAtom::Service0));
    }

    #[test]
    fn services_list_case_observes_running_and_deployed_services() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = deploy_release_only(&spec, &state, ServiceAtom::Service1);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Session(SessionTransportAction::AcceptSession),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AcceptSession(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthenticateAdmin(
                SessionAtom::Session0,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::SessionAuth(SessionAuthAction::AuthorizeAdmin(
                StreamAtom::Stream0,
                RequestKindAtom::ServicesList,
            )),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Wire(WireProtocolAction::ServicesList(StreamAtom::Stream0)),
        );

        assert!(state.supervision.service_is_running(ServiceAtom::Service0));
        assert!(state.deploy.release_promoted(ServiceAtom::Service1));
        assert!(
            state
                .wire
                .saw_request(StreamAtom::Stream0, RequestKindAtom::ServicesList)
        );
    }

    #[test]
    fn shutdown_case_blocks_new_rpc_and_drains_services() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::ReceiveSignal),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Session(SessionTransportAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopAccepting),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::DrainSessions),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::RequestStop(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopServicesGraceful),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopMaintenance),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::Finalize),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Rpc(RpcAction::RejectLocal(ServiceAtom::Service0)),
        );

        assert!(matches!(state.shutdown.phase, ShutdownPhase::Completed));
        assert!(!state.supervision.service_is_running(ServiceAtom::Service0));
        assert!(state.rpc.has_denied_local_call_for(ServiceAtom::Service0));
    }

    #[test]
    fn graceful_shutdown_force_case_marks_forced_stop_attempt() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::ReceiveSignal),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Session(SessionTransportAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopAccepting),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::DrainSessions),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::RequestStop(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::ReapService(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopServicesForced),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopMaintenance),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::Finalize),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Manager(ManagerRuntimeAction::FinishShutdown),
        );

        assert!(state.shutdown.forced_stop_attempted);
        assert!(matches!(state.shutdown.phase, ShutdownPhase::Completed));
        assert!(matches!(state.manager.phase, ManagerRuntimePhase::Stopped));
    }

    #[test]
    fn maintenance_reap_case_reaps_service_to_quiescent() {
        let spec = SystemSpec::new();
        let state = deploy_and_start_service(&spec, &listening_state(&spec), ServiceAtom::Service0);
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::ReceiveSignal),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Session(SessionTransportAction::BeginShutdown),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::StopAccepting),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Shutdown(ShutdownFlowAction::DrainSessions),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::RequestStop(ServiceAtom::Service0)),
        );
        let state = step(
            &spec,
            &state,
            SystemAtomicAction::Supervision(SupervisionAction::ReapService(ServiceAtom::Service0)),
        );

        assert!(!state.supervision.service_is_running(ServiceAtom::Service0));
        assert!(!state.supervision.service_is_stopping(ServiceAtom::Service0));
    }

    #[test]
    fn boot_initial_state_exposes_boot_actions_only() {
        let spec = SystemSpec::new();
        let enabled = spec.enabled_atomic_actions(&spec.boot_state());

        assert!(enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Manager(ManagerRuntimeAction::LoadExistingConfig)
            )
        }));
        assert!(enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Manager(ManagerRuntimeAction::CreateDefaultConfig)
            )
        }));
        assert!(!enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Command(CommandProtocolAction::Start(_))
            )
        }));
    }

    #[test]
    fn listening_state_exposes_session_command_and_plugin_actions() {
        let spec = SystemSpec::new();
        let enabled = spec.enabled_atomic_actions(&listening_state(&spec));

        assert!(enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Session(SessionTransportAction::AcceptSession)
            )
        }));
        assert!(enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Command(CommandProtocolAction::Start(_))
            )
        }));
        assert!(enabled.iter().any(|action| {
            matches!(
                action,
                SystemAtomicAction::Plugin(PluginPlatformAction::RegisterPlugin(_))
            )
        }));
    }

    #[test]
    fn manager_shutdown_conflicts_with_new_work_via_declared_reads() {
        let spec = SystemSpec::new();
        assert!(spec.actions_conflict(
            &SystemAtomicAction::Manager(ManagerRuntimeAction::BeginShutdown),
            &SystemAtomicAction::Deploy(DeployAction::AdvanceUpload(ServiceAtom::Service0,)),
        ));
    }

    #[test]
    fn public_message_types_bind_to_system_surface() {
        let cases = [
            (
                MessageType::HelloNegotiate,
                SystemMessageBinding::Request(RequestKindAtom::HelloNegotiate),
            ),
            (
                MessageType::DeployPrepare,
                SystemMessageBinding::Request(RequestKindAtom::DeployPrepare),
            ),
            (
                MessageType::ArtifactPush,
                SystemMessageBinding::Request(RequestKindAtom::ArtifactPush),
            ),
            (
                MessageType::ArtifactCommit,
                SystemMessageBinding::Request(RequestKindAtom::ArtifactCommit),
            ),
            (
                MessageType::CommandStart,
                SystemMessageBinding::Request(RequestKindAtom::CommandStart),
            ),
            (
                MessageType::CommandEvent,
                SystemMessageBinding::CommandEvent,
            ),
            (
                MessageType::StateRequest,
                SystemMessageBinding::Request(RequestKindAtom::StateRequest),
            ),
            (
                MessageType::StateResponse,
                SystemMessageBinding::Response(RequestKindAtom::StateRequest),
            ),
            (
                MessageType::ServicesList,
                SystemMessageBinding::Request(RequestKindAtom::ServicesList),
            ),
            (
                MessageType::CommandCancel,
                SystemMessageBinding::Request(RequestKindAtom::CommandCancel),
            ),
            (
                MessageType::LogsRequest,
                SystemMessageBinding::Request(RequestKindAtom::LogsRequest),
            ),
            (MessageType::LogsChunk, SystemMessageBinding::LogChunk),
            (MessageType::LogsEnd, SystemMessageBinding::LogsEnd),
            (
                MessageType::RpcInvoke,
                SystemMessageBinding::Request(RequestKindAtom::RpcInvoke),
            ),
            (
                MessageType::BindingsCertUpload,
                SystemMessageBinding::Request(RequestKindAtom::BindingsCertUpload),
            ),
        ];

        for (message_type, expected) in cases {
            assert_eq!(system_message_binding(message_type), expected);
        }
    }
}
