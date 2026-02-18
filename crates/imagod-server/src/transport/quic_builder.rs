use std::sync::Arc;

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;

use super::{DATAGRAM_BUFFER_BYTES, STAGE_TRANSPORT};

pub(crate) fn build_quic_server_config(
    tls: rustls::ServerConfig,
) -> Result<quinn::ServerConfig, ImagodError> {
    let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("quic tls conversion failed: {e}"),
        )
    })?;

    let mut quic_server = quinn::ServerConfig::with_crypto(Arc::new(quic_tls));
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_send_buffer_size(DATAGRAM_BUFFER_BYTES);
    transport.datagram_receive_buffer_size(Some(DATAGRAM_BUFFER_BYTES));
    quic_server.transport_config(Arc::new(transport));

    Ok(quic_server)
}
