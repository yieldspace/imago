use std::{io::BufReader, path::Path, sync::Arc};

use imago_protocol::ErrorCode;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use web_transport_quinn::Server;

use crate::{config::ImagodConfig, error::ImagodError};

const STAGE_TRANSPORT: &str = "transport.setup";

pub fn build_server(config: &ImagodConfig) -> Result<Server, ImagodError> {
    let listen_addr = config.listen_addr.parse().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("listen_addr parse failed: {e}"),
        )
    })?;

    let server_chain = load_certs(&config.tls.server_cert)?;
    let server_key = load_private_key(&config.tls.server_key)?;
    let client_ca = load_certs(&config.tls.client_ca_cert)?;

    let mut roots = rustls::RootCertStore::empty();
    for cert in client_ca {
        roots.add(cert).map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("invalid client CA cert: {e}"),
            )
        })?;
    }

    let client_verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("client verifier setup failed: {e}"),
            )
        })?;

    let mut tls = rustls::ServerConfig::builder_with_provider(
        web_transport_quinn::crypto::default_provider(),
    )
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("TLS protocol setup failed: {e}"),
        )
    })?
    .with_client_cert_verifier(client_verifier)
    .with_single_cert(server_chain, server_key)
    .map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("server certificate setup failed: {e}"),
        )
    })?;
    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];
    tls.max_early_data_size = u32::MAX;

    let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("quic tls conversion failed: {e}"),
        )
    })?;
    let quic_server = quinn::ServerConfig::with_crypto(Arc::new(quic_tls));
    let endpoint = quinn::Endpoint::server(quic_server, listen_addr).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("endpoint bind failed: {e}"),
        )
    })?;

    Ok(web_transport_quinn::Server::new(endpoint))
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, ImagodError> {
    let file = std::fs::File::open(path).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("failed to open cert {}: {e}", path.display()),
        )
    })?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("failed to parse cert {}: {e}", path.display()),
            )
        })?;
    if certs.is_empty() {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("certificate file is empty: {}", path.display()),
        ));
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, ImagodError> {
    let file = std::fs::File::open(path).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("failed to open key {}: {e}", path.display()),
        )
    })?;
    let mut reader = BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("failed to parse key {}: {e}", path.display()),
            )
        })?
        .ok_or_else(|| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("private key is missing: {}", path.display()),
            )
        })?;
    Ok(key)
}
