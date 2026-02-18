//! WebTransport server bootstrap with TLS and client certificate validation.

mod quic_builder;
mod server_builder;
mod tls_material;

pub(crate) const STAGE_TRANSPORT: &str = "transport.setup";
pub(crate) const DATAGRAM_BUFFER_BYTES: usize = 1024 * 1024;

pub use server_builder::build_server;
