//! Shared wire contracts for `imago-cli` and `imagod`.
//!
//! This crate is the protocol source of truth for:
//! - envelope identifiers and message routing tags
//! - request/response payload schemas
//! - structured error representation
//! - payload validation helpers used at trust boundaries

pub mod cbor;
pub mod envelope;
pub mod error;
pub mod messages;
pub mod validate;

/// Current wire protocol version emitted during `hello.negotiate`.
pub const PROTOCOL_VERSION: &str = "0.1.0";
/// Supported peer protocol versions for this build.
pub const SUPPORTED_PROTOCOL_VERSION_RANGE: &str = ">=0.1.0,<0.2.0";

/// CBOR serialization and deserialization helpers.
pub use cbor::{CborError, from_cbor, to_cbor};
/// Common protocol envelope used on stream/datagram payloads.
pub use envelope::ProtocolEnvelope;
/// Structured protocol error types shared across client/server.
pub use error::{ErrorCode, StructuredError};
/// Message payload types and routing tags.
pub use messages::{
    ArtifactCommitRequest, ArtifactCommitResponse, ArtifactPushAck, ArtifactPushChunkHeader,
    ArtifactPushRequest, ArtifactStatus, BindingsCertInspectRequest, BindingsCertInspectResponse,
    BindingsCertUploadRequest, BindingsCertUploadResponse, ByteRange, CommandCancelRequest,
    CommandCancelResponse, CommandEvent, CommandEventType, CommandPayload, CommandStartRequest,
    CommandStartResponse, CommandState, CommandType, DeployCommandPayload, DeployPrepareRequest,
    DeployPrepareResponse, HelloNegotiateRequest, HelloNegotiateResponse, LogChunk, LogEnd,
    LogError, LogErrorCode, LogRequest, LogStreamKind, MessageType, RpcInvokeError,
    RpcInvokeRequest, RpcInvokeResponse, RpcInvokeTargetService, RunCommandPayload,
    ServiceListRequest, ServiceListResponse, ServiceState, ServiceStatusEntry, StateRequest,
    StateResponse, StopCommandPayload,
};
/// Validation trait and reusable validation error type.
pub use validate::{Validate, ValidationError};
