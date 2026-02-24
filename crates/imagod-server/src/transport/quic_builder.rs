use std::{sync::Arc, time::Duration};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_config::RuntimeConfig;

use super::{DATAGRAM_BUFFER_BYTES, STAGE_TRANSPORT};

pub(crate) fn build_quic_server_config(
    tls: rustls::ServerConfig,
    runtime: &RuntimeConfig,
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
    transport.keep_alive_interval(Some(Duration::from_secs(
        runtime.transport_keepalive_interval_secs,
    )));
    let idle_timeout =
        quinn::IdleTimeout::try_from(Duration::from_secs(runtime.transport_max_idle_timeout_secs))
            .map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_TRANSPORT,
                    format!("runtime.transport_max_idle_timeout_secs is too large: {e}"),
                )
            })?;
    transport.max_idle_timeout(Some(idle_timeout));
    quic_server.transport_config(Arc::new(transport));

    Ok(quic_server)
}
