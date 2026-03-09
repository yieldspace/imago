//! Public wire-contract surface owned by `imagod-spec`.

pub use crate::envelope::ProtocolEnvelope;
pub use crate::error::{ErrorCode, StructuredError};
pub use crate::messages::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
    ArtifactPushRequest, ArtifactStatus, BindingsCertUploadRequest, BindingsCertUploadResponse,
    ByteRange, CommandCancelRequest, CommandCancelResponse, CommandEvent, CommandEventType,
    CommandPayload, CommandStartRequest, CommandStartResponse, CommandState, CommandType,
    DeployCommandPayload, DeployPrepareRequest, DeployPrepareResponse, HelloNegotiateRequest,
    HelloNegotiateResponse, LogChunk, LogEnd, LogError, LogErrorCode, LogRequest, LogStreamKind,
    MessageType, RpcInvokeError, RpcInvokeRequest, RpcInvokeResponse, RpcInvokeTargetService,
    RunCommandPayload, ServiceListRequest, ServiceListResponse, ServiceState, ServiceStatusEntry,
    StateRequest, StateResponse, StopCommandPayload,
};
pub use crate::validate::{Validate, ValidationError};

/// Current wire protocol version emitted during `hello.negotiate`.
pub const PROTOCOL_VERSION: &str = "0.1.0";
/// Supported peer protocol versions for this build.
pub const SUPPORTED_PROTOCOL_VERSION_RANGE: &str = ">=0.1.0,<0.2.0";
