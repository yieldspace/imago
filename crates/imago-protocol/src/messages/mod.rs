//! Message payload domains and wire-level routing tags.
//!
//! Each submodule maps to one protocol concern (artifact transfer, commands,
//! logs, RPC, and service status). `MessageType` is the canonical route key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod artifact;
pub mod bindings;
pub mod command;
pub mod hello;
pub mod log;
pub mod rpc;
pub mod service;

pub use artifact::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
    ArtifactPushRequest, ArtifactStatus, ByteRange, DeployPrepareRequest, DeployPrepareResponse,
};
pub use bindings::{BindingsCertUploadRequest, BindingsCertUploadResponse};
pub use command::{
    CommandCancelRequest, CommandCancelResponse, CommandEvent, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployCommandPayload,
    RunCommandPayload, StateRequest, StateResponse, StopCommandPayload,
};
pub use hello::{HelloNegotiateRequest, HelloNegotiateResponse};
pub use log::{LogChunk, LogEnd, LogError, LogErrorCode, LogRequest, LogStreamKind};
pub use rpc::{RpcInvokeError, RpcInvokeRequest, RpcInvokeResponse, RpcInvokeTargetService};
pub use service::{ServiceListRequest, ServiceListResponse, ServiceState, ServiceStatusEntry};

pub type StringMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Canonical envelope route keys exchanged between `imago-cli` and `imagod`.
pub enum MessageType {
    #[serde(rename = "hello.negotiate")]
    HelloNegotiate,
    #[serde(rename = "deploy.prepare")]
    DeployPrepare,
    #[serde(rename = "artifact.push")]
    ArtifactPush,
    #[serde(rename = "artifact.commit")]
    ArtifactCommit,
    #[serde(rename = "command.start")]
    CommandStart,
    #[serde(rename = "command.event")]
    CommandEvent,
    #[serde(rename = "state.request")]
    StateRequest,
    #[serde(rename = "state.response")]
    StateResponse,
    #[serde(rename = "services.list")]
    ServicesList,
    #[serde(rename = "command.cancel")]
    CommandCancel,
    #[serde(rename = "logs.request")]
    LogsRequest,
    #[serde(rename = "logs.chunk")]
    LogsChunk,
    #[serde(rename = "logs.end")]
    LogsEnd,
    #[serde(rename = "rpc.invoke")]
    RpcInvoke,
    #[serde(rename = "bindings.cert.upload")]
    BindingsCertUpload,
}

#[cfg(test)]
include!("tests.rs");
