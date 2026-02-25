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

/// CBOR serialization and deserialization helpers.
pub use cbor::{CborError, from_cbor, to_cbor};
/// Common protocol envelope used on stream/datagram payloads.
pub use envelope::ProtocolEnvelope;
/// Structured protocol error types shared across client/server.
pub use error::{ErrorCode, StructuredError};
/// Message payload types and routing tags.
pub use messages::{
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
/// Validation trait and reusable validation error type.
pub use validate::{Validate, ValidationError};
