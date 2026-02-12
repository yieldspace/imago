//! Network-facing server components for `imagod`.

/// Protocol envelope dispatch and command/event orchestration bridge.
pub mod protocol_handler;
/// QUIC/WebTransport endpoint construction with mTLS validation.
pub mod transport;

/// Re-export of the protocol session handler.
pub use protocol_handler::ProtocolHandler;
/// Re-export of server transport builder.
pub use transport::build_server;
