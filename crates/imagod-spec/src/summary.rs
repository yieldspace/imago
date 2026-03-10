//! Bounded contract probes and summaries used to connect concrete runtime state to formal models.

use serde::{Deserialize, Serialize};

use crate::{CommandProtocolObservedState, CommandProtocolOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryServiceId {
    #[default]
    Service0,
    Service1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryStreamId {
    #[default]
    Stream0,
    Stream1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummarySessionRole {
    #[default]
    Admin,
    Client,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryRequestKind {
    #[default]
    HelloNegotiate,
    DeployPrepare,
    ArtifactPush,
    ArtifactCommit,
    CommandStart,
    StateRequest,
    ServicesList,
    CommandCancel,
    LogsRequest,
    RpcInvoke,
    BindingsCertUpload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryCommandEvent {
    #[default]
    Accepted,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryLogChunk {
    #[default]
    Chunk0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryTaskState {
    #[default]
    NotStarted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryTaskKind {
    #[default]
    PluginGc,
    BootRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SummaryShutdownPhase {
    #[default]
    Idle,
    SignalReceived,
    DrainingSessions,
    StoppingServices,
    StoppingMaintenance,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShutdownStateSummary {
    pub phase: SummaryShutdownPhase,
    pub accepts_stopped: bool,
    pub sessions_drained: bool,
    pub services_stopped: bool,
    pub maintenance_stopped: bool,
    pub forced_stop_attempted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContractEffectSummary {
    RequestObserved(SummaryStreamId, SummaryRequestKind),
    Response(SummaryStreamId, SummaryRequestKind),
    AuthorizationGranted(SummaryStreamId, SummaryRequestKind),
    CommandEvent(SummaryStreamId, SummaryCommandEvent),
    LogChunk(SummaryStreamId, SummaryLogChunk),
    LogsEnd(SummaryStreamId),
    AuthorizationRejected(SummaryStreamId, SummaryRequestKind),
    LocalRpcResolved(SummaryServiceId),
    LocalRpcDenied(SummaryServiceId),
    RemoteRpcConnected(SummaryServiceId),
    RemoteRpcCompleted(SummaryServiceId),
    RemoteRpcDisconnected(SummaryServiceId),
    RemoteRpcDenied(SummaryServiceId),
    TaskMilestone(SummaryTaskKind, SummaryTaskState),
    ShutdownComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouterOutputSummary {
    pub effects: Vec<ContractEffectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionAuthOutputSummary {
    pub effects: Vec<ContractEffectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogsOutputSummary {
    pub effects: Vec<ContractEffectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeOutputSummary {
    pub effects: Vec<ContractEffectSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ManagerRuntimeOutputSummary {
    pub effects: Vec<ContractEffectSummary>,
}

pub type CommandStateSummary = CommandProtocolObservedState;
pub type CommandOutputSummary = CommandProtocolOutput;
pub type CommandProbeState = CommandStateSummary;
pub type CommandProbeOutput = CommandOutputSummary;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouterProbeOutput {
    pub output: RouterOutputSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionAuthProbeOutput {
    pub output: SessionAuthOutputSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogsProbeOutput {
    pub output: LogsOutputSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeProbeOutput {
    pub output: RuntimeOutputSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ManagerRuntimeProbeOutput {
    pub output: ManagerRuntimeOutputSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterProbeState {
    pub active_session: bool,
    pub role: Option<SummarySessionRole>,
    pub deploy_prepare_authorized: bool,
    pub artifact_push_authorized: bool,
    pub artifact_commit_authorized: bool,
    pub state_request_authorized: bool,
    pub services_list_authorized: bool,
    pub command_cancel_authorized: bool,
    pub rpc_invoke_authorized: bool,
    pub bindings_cert_upload_authorized: bool,
    pub authority_uploaded: bool,
}

impl RouterProbeState {
    pub const fn initial_admin_stream() -> Self {
        Self {
            active_session: true,
            role: Some(SummarySessionRole::Admin),
            deploy_prepare_authorized: true,
            artifact_push_authorized: true,
            artifact_commit_authorized: true,
            state_request_authorized: true,
            services_list_authorized: true,
            command_cancel_authorized: true,
            rpc_invoke_authorized: true,
            bindings_cert_upload_authorized: true,
            authority_uploaded: false,
        }
    }
}

impl Default for RouterProbeState {
    fn default() -> Self {
        Self::initial_admin_stream()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterStateSummary {
    pub active_session: bool,
    pub role: Option<SummarySessionRole>,
    pub deploy_prepare_authorized: bool,
    pub artifact_push_authorized: bool,
    pub artifact_commit_authorized: bool,
    pub state_request_authorized: bool,
    pub services_list_authorized: bool,
    pub command_cancel_authorized: bool,
    pub rpc_invoke_authorized: bool,
    pub bindings_cert_upload_authorized: bool,
    pub authority_uploaded: bool,
}

impl From<RouterProbeState> for RouterStateSummary {
    fn from(probe: RouterProbeState) -> Self {
        Self {
            active_session: probe.active_session,
            role: probe.role,
            deploy_prepare_authorized: probe.deploy_prepare_authorized,
            artifact_push_authorized: probe.artifact_push_authorized,
            artifact_commit_authorized: probe.artifact_commit_authorized,
            state_request_authorized: probe.state_request_authorized,
            services_list_authorized: probe.services_list_authorized,
            command_cancel_authorized: probe.command_cancel_authorized,
            rpc_invoke_authorized: probe.rpc_invoke_authorized,
            bindings_cert_upload_authorized: probe.bindings_cert_upload_authorized,
            authority_uploaded: probe.authority_uploaded,
        }
    }
}

impl Default for RouterStateSummary {
    fn default() -> Self {
        RouterProbeState::default().into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionAuthProbeState {
    pub active_session: bool,
    pub shutdown_requested: bool,
    pub role: Option<SummarySessionRole>,
    pub read_timed_out: bool,
    pub stream_closed: bool,
    pub client_authority_uploaded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionAuthStateSummary {
    pub active_session: bool,
    pub shutdown_requested: bool,
    pub role: Option<SummarySessionRole>,
    pub read_timed_out: bool,
    pub stream_closed: bool,
    pub client_authority_uploaded: bool,
}

impl From<SessionAuthProbeState> for SessionAuthStateSummary {
    fn from(probe: SessionAuthProbeState) -> Self {
        Self {
            active_session: probe.active_session,
            shutdown_requested: probe.shutdown_requested,
            role: probe.role,
            read_timed_out: probe.read_timed_out,
            stream_closed: probe.stream_closed,
            client_authority_uploaded: probe.client_authority_uploaded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsProbeState {
    pub service_running: bool,
    pub logs_authorized: bool,
    pub stream_open: bool,
    pub chunk_pending: bool,
    pub completed: bool,
}

impl LogsProbeState {
    pub const fn initial_running_service() -> Self {
        Self {
            service_running: true,
            logs_authorized: true,
            stream_open: false,
            chunk_pending: false,
            completed: false,
        }
    }
}

impl Default for LogsProbeState {
    fn default() -> Self {
        Self::initial_running_service()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsStateSummary {
    pub service_running: bool,
    pub logs_authorized: bool,
    pub stream_open: bool,
    pub chunk_pending: bool,
    pub completed: bool,
}

impl From<LogsProbeState> for LogsStateSummary {
    fn from(probe: LogsProbeState) -> Self {
        Self {
            service_running: probe.service_running,
            logs_authorized: probe.logs_authorized,
            stream_open: probe.stream_open,
            chunk_pending: probe.chunk_pending,
            completed: probe.completed,
        }
    }
}

impl Default for LogsStateSummary {
    fn default() -> Self {
        LogsProbeState::default().into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeProbeState {
    pub service0_promoted: bool,
    pub service1_promoted: bool,
    pub service0_running: bool,
    pub service1_running: bool,
    pub service0_reaped: bool,
    pub service1_reaped: bool,
    pub service0_rolled_back: bool,
    pub binding_granted_service0: bool,
    pub remote_connected: bool,
    pub manager_shutdown_started: bool,
    pub manager_stopped: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ShutdownStateSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeStateSummary {
    pub service0_promoted: bool,
    pub service1_promoted: bool,
    pub service0_running: bool,
    pub service1_running: bool,
    pub service0_reaped: bool,
    pub service1_reaped: bool,
    pub service0_rolled_back: bool,
    pub binding_granted_service0: bool,
    pub remote_connected: bool,
    pub manager_shutdown_started: bool,
    pub manager_stopped: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ShutdownStateSummary,
}

impl From<RuntimeProbeState> for RuntimeStateSummary {
    fn from(probe: RuntimeProbeState) -> Self {
        Self {
            service0_promoted: probe.service0_promoted,
            service1_promoted: probe.service1_promoted,
            service0_running: probe.service0_running,
            service1_running: probe.service1_running,
            service0_reaped: probe.service0_reaped,
            service1_reaped: probe.service1_reaped,
            service0_rolled_back: probe.service0_rolled_back,
            binding_granted_service0: probe.binding_granted_service0,
            remote_connected: probe.remote_connected,
            manager_shutdown_started: probe.manager_shutdown_started,
            manager_stopped: probe.manager_stopped,
            session_shutdown_requested: probe.session_shutdown_requested,
            shutdown: probe.shutdown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ManagerRuntimeProbeState {
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: SummaryTaskState,
    pub boot_restore: SummaryTaskState,
    pub listening: bool,
    pub manager_shutdown_started: bool,
    pub manager_stopped: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ShutdownStateSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ManagerRuntimeStateSummary {
    pub config_loaded: bool,
    pub created_default: bool,
    pub plugin_gc: SummaryTaskState,
    pub boot_restore: SummaryTaskState,
    pub listening: bool,
    pub manager_shutdown_started: bool,
    pub manager_stopped: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ShutdownStateSummary,
}

impl From<ManagerRuntimeProbeState> for ManagerRuntimeStateSummary {
    fn from(probe: ManagerRuntimeProbeState) -> Self {
        Self {
            config_loaded: probe.config_loaded,
            created_default: probe.created_default,
            plugin_gc: probe.plugin_gc,
            boot_restore: probe.boot_restore,
            listening: probe.listening,
            manager_shutdown_started: probe.manager_shutdown_started,
            manager_stopped: probe.manager_stopped,
            session_shutdown_requested: probe.session_shutdown_requested,
            shutdown: probe.shutdown,
        }
    }
}
