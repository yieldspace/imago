use std::{sync::Arc, time::Duration};

use imagod_common::ImagodError;
use imagod_config::RuntimeConfig;
use imagod_spec::ErrorCode;

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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::path::PathBuf;

    use imagod_config::{ImagodConfig, RuntimeConfig, TlsConfig};

    use super::build_quic_server_config;

    fn test_server_key_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/local-imagod-plugin-hello/certs/server.key")
    }

    fn sample_config() -> ImagodConfig {
        ImagodConfig {
            listen_addr: "127.0.0.1:4443".to_string(),
            tls: TlsConfig {
                server_key: test_server_key_path(),
                admin_public_keys: Vec::new(),
                client_public_keys: Vec::new(),
                known_public_keys: std::collections::BTreeMap::new(),
            },
            storage_root: PathBuf::from("/tmp/imago-test-storage"),
            runtime: RuntimeConfig::default(),
            server_version: "imagod/test".to_string(),
        }
    }

    #[test]
    fn given_valid_runtime_timeouts__when_build_quic_server_config__then_succeeds() {
        let config = sample_config();
        let tls = crate::transport::tls_material::build_tls_server_config(&config)
            .expect("tls config should build in test fixture");

        let built = build_quic_server_config(tls, &config.runtime);
        assert!(built.is_ok(), "quic config should build");
    }

    #[test]
    fn given_extreme_idle_timeout__when_build_quic_server_config__then_bad_request_is_returned() {
        let mut config = sample_config();
        config.runtime.transport_max_idle_timeout_secs = u64::MAX;
        let tls = crate::transport::tls_material::build_tls_server_config(&config)
            .expect("tls config should build in test fixture");

        let err = build_quic_server_config(tls, &config.runtime)
            .expect_err("too large idle timeout must fail");
        assert_eq!(err.code, imagod_spec::ErrorCode::BadRequest);
        assert_eq!(err.stage, super::STAGE_TRANSPORT);
        assert!(
            err.message
                .contains("runtime.transport_max_idle_timeout_secs is too large"),
            "unexpected message: {}",
            err.message
        );
    }
}
