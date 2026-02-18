use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_config::ImagodConfig;
use web_transport_quinn::Server;

use super::{
    STAGE_TRANSPORT, quic_builder::build_quic_server_config, tls_material::build_tls_server_config,
};

/// Builds a WebTransport server endpoint from validated configuration.
pub fn build_server(config: &ImagodConfig) -> Result<Server, ImagodError> {
    let listen_addr = config.listen_addr.parse().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("listen_addr parse failed: {e}"),
        )
    })?;

    let tls = build_tls_server_config(config)?;
    let quic_server = build_quic_server_config(tls)?;
    let endpoint = quinn::Endpoint::server(quic_server, listen_addr).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("endpoint bind failed: {e}"),
        )
    })?;

    Ok(web_transport_quinn::Server::new(endpoint))
}
