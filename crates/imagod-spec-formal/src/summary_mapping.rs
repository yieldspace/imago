use imagod_spec::{
    ContractEffectSummary, SummaryCommandEvent, SummaryLogChunk, SummaryRequestKind,
    SummarySessionRole, SummaryShutdownPhase, SummaryStreamId, SummaryTaskState,
};

use crate::{
    atoms::{
        CommandEventAtom, LogChunkAtom, RequestKindAtom, SessionRoleAtom, StreamAtom,
    },
    manager_runtime::TaskState,
    shutdown_flow::ShutdownPhase,
    system::SystemEffect,
};

pub const fn stream_atom(stream: SummaryStreamId) -> StreamAtom {
    match stream {
        SummaryStreamId::Stream0 => StreamAtom::Stream0,
        SummaryStreamId::Stream1 => StreamAtom::Stream1,
    }
}

pub const fn session_role_atom(role: SummarySessionRole) -> SessionRoleAtom {
    match role {
        SummarySessionRole::Admin => SessionRoleAtom::Admin,
        SummarySessionRole::Client => SessionRoleAtom::Client,
        SummarySessionRole::Unknown => SessionRoleAtom::Unknown,
    }
}

pub const fn request_kind_atom(kind: SummaryRequestKind) -> RequestKindAtom {
    match kind {
        SummaryRequestKind::HelloNegotiate => RequestKindAtom::HelloNegotiate,
        SummaryRequestKind::DeployPrepare => RequestKindAtom::DeployPrepare,
        SummaryRequestKind::ArtifactPush => RequestKindAtom::ArtifactPush,
        SummaryRequestKind::ArtifactCommit => RequestKindAtom::ArtifactCommit,
        SummaryRequestKind::CommandStart => RequestKindAtom::CommandStart,
        SummaryRequestKind::StateRequest => RequestKindAtom::StateRequest,
        SummaryRequestKind::ServicesList => RequestKindAtom::ServicesList,
        SummaryRequestKind::CommandCancel => RequestKindAtom::CommandCancel,
        SummaryRequestKind::LogsRequest => RequestKindAtom::LogsRequest,
        SummaryRequestKind::RpcInvoke => RequestKindAtom::RpcInvoke,
        SummaryRequestKind::BindingsCertUpload => RequestKindAtom::BindingsCertUpload,
    }
}

pub const fn command_event_atom(event: SummaryCommandEvent) -> CommandEventAtom {
    match event {
        SummaryCommandEvent::Accepted => CommandEventAtom::Accepted,
        SummaryCommandEvent::Running => CommandEventAtom::Running,
        SummaryCommandEvent::Succeeded => CommandEventAtom::Succeeded,
        SummaryCommandEvent::Failed => CommandEventAtom::Failed,
        SummaryCommandEvent::Canceled => CommandEventAtom::Canceled,
    }
}

pub const fn log_chunk_atom(chunk: SummaryLogChunk) -> LogChunkAtom {
    match chunk {
        SummaryLogChunk::Chunk0 => LogChunkAtom::Chunk0,
    }
}

pub const fn task_state(summary: SummaryTaskState) -> TaskState {
    match summary {
        SummaryTaskState::NotStarted => TaskState::NotStarted,
        SummaryTaskState::Succeeded => TaskState::Succeeded,
        SummaryTaskState::Failed => TaskState::Failed,
    }
}

pub const fn shutdown_phase(summary: SummaryShutdownPhase) -> ShutdownPhase {
    match summary {
        SummaryShutdownPhase::Idle => ShutdownPhase::Idle,
        SummaryShutdownPhase::SignalReceived => ShutdownPhase::SignalReceived,
        SummaryShutdownPhase::DrainingSessions => ShutdownPhase::DrainingSessions,
        SummaryShutdownPhase::StoppingServices => ShutdownPhase::StoppingServices,
        SummaryShutdownPhase::StoppingMaintenance => ShutdownPhase::StoppingMaintenance,
        SummaryShutdownPhase::Completed => ShutdownPhase::Completed,
    }
}

pub fn system_effects(effects: &[ContractEffectSummary]) -> Vec<SystemEffect> {
    effects
        .iter()
        .map(|effect| match effect {
            ContractEffectSummary::Response(stream, kind) => {
                SystemEffect::Response(stream_atom(*stream), request_kind_atom(*kind))
            }
            ContractEffectSummary::CommandEvent(stream, event) => {
                SystemEffect::CommandEvent(stream_atom(*stream), command_event_atom(*event))
            }
            ContractEffectSummary::LogChunk(stream, chunk) => {
                SystemEffect::LogChunk(stream_atom(*stream), log_chunk_atom(*chunk))
            }
            ContractEffectSummary::LogsEnd(stream) => SystemEffect::LogsEnd(stream_atom(*stream)),
            ContractEffectSummary::AuthorizationRejected(stream, kind) => {
                SystemEffect::AuthorizationRejected(stream_atom(*stream), request_kind_atom(*kind))
            }
            ContractEffectSummary::ShutdownComplete => SystemEffect::ShutdownComplete,
        })
        .collect()
}
