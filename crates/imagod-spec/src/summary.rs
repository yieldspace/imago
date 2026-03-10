//! Bounded contract summaries used to connect concrete runtime state to formal models.

use serde::{Deserialize, Serialize};

use crate::{CommandProtocolObservedState, CommandProtocolOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummaryServiceId {
    Service0,
    Service1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummaryStreamId {
    Stream0,
    Stream1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummarySessionRole {
    Admin,
    Client,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummaryRequestKind {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummaryCommandEvent {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SummaryLogChunk {
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
    Response(SummaryStreamId, SummaryRequestKind),
    CommandEvent(SummaryStreamId, SummaryCommandEvent),
    LogChunk(SummaryStreamId, SummaryLogChunk),
    LogsEnd(SummaryStreamId),
    AuthorizationRejected(SummaryStreamId, SummaryRequestKind),
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
    pub request: Option<SummaryRequestKind>,
    pub authority_uploaded: bool,
}

impl RouterStateSummary {
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
            request: None,
            authority_uploaded: false,
        }
    }
}

impl Default for RouterStateSummary {
    fn default() -> Self {
        Self::initial_admin_stream()
    }
}

pub type RouterProbeState = RouterStateSummary;
pub type RouterProbeOutput = RouterOutputSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionAuthStateSummary {
    pub active_session: bool,
    pub shutdown_requested: bool,
    pub role: Option<SummarySessionRole>,
    pub admin_services_list_authorized: bool,
    pub client_hello_authorized: bool,
    pub client_rpc_authorized: bool,
    pub unauthorized_services_list_rejected: bool,
    pub read_timed_out: bool,
    pub stream_closed: bool,
    pub client_authority_uploaded: bool,
}

pub type SessionAuthProbeState = SessionAuthStateSummary;
pub type SessionAuthProbeOutput = SessionAuthOutputSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsStateSummary {
    pub service_running: bool,
    pub logs_authorized: bool,
    pub acknowledged: bool,
    pub chunk_seen: bool,
    pub ended: bool,
}

impl LogsStateSummary {
    pub const fn initial_running_service() -> Self {
        Self {
            service_running: true,
            logs_authorized: true,
            acknowledged: false,
            chunk_seen: false,
            ended: false,
        }
    }
}

impl Default for LogsStateSummary {
    fn default() -> Self {
        Self::initial_running_service()
    }
}

pub type LogsProbeState = LogsStateSummary;
pub type LogsProbeOutput = LogsOutputSummary;

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
    pub local_rpc_resolved: bool,
    pub local_rpc_denied: bool,
    pub remote_connected: bool,
    pub remote_completed: bool,
    pub remote_disconnected: bool,
    pub remote_denied: bool,
    pub manager_shutdown_started: bool,
    pub manager_stopped: bool,
    pub session_shutdown_requested: bool,
    pub shutdown: ShutdownStateSummary,
}

pub type RuntimeProbeState = RuntimeStateSummary;
pub type RuntimeProbeOutput = RuntimeOutputSummary;

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

pub type ManagerRuntimeProbeState = ManagerRuntimeStateSummary;
pub type ManagerRuntimeProbeOutput = ManagerRuntimeOutputSummary;
