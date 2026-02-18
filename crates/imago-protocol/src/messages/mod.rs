use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod artifact;
pub mod command;
pub mod hello;
pub mod log;

pub use artifact::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
    ArtifactPushRequest, ArtifactStatus, ByteRange, DeployPrepareRequest, DeployPrepareResponse,
};
pub use command::{
    CommandCancelRequest, CommandCancelResponse, CommandEvent, CommandEventType, CommandPayload,
    CommandStartRequest, CommandStartResponse, CommandState, CommandType, DeployCommandPayload,
    RunCommandPayload, StateRequest, StateResponse, StopCommandPayload,
};
pub use hello::{HelloNegotiateRequest, HelloNegotiateResponse};
pub use log::{LogChunk, LogEnd, LogError, LogErrorCode, LogRequest, LogStreamKind};

pub type StringMap = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(rename = "command.cancel")]
    CommandCancel,
    #[serde(rename = "logs.request")]
    LogsRequest,
    #[serde(rename = "logs.chunk")]
    LogsChunk,
    #[serde(rename = "logs.end")]
    LogsEnd,
}

#[cfg(test)]
include!("tests.rs");
