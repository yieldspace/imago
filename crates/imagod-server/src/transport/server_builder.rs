use imagod_common::ImagodError;
use imagod_config::ImagodConfig;
use imagod_spec::ErrorCode;
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
    let quic_server = build_quic_server_config(tls, &config.runtime)?;
    let endpoint = quinn::Endpoint::server(quic_server, listen_addr).map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_TRANSPORT,
            format!("endpoint bind failed: {e}"),
        )
    })?;

    Ok(web_transport_quinn::Server::new(endpoint))
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use std::{collections::BTreeMap, net::UdpSocket, path::PathBuf};

    use imagod_config::{ImagodConfig, RuntimeConfig, TlsConfig};

    use super::build_server;

    fn test_server_key_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/local-imagod-plugin-hello/certs/server.key")
    }

    fn sample_config(listen_addr: String) -> ImagodConfig {
        ImagodConfig {
            listen_addr,
            tls: TlsConfig {
                server_key: test_server_key_path(),
                admin_public_keys: Vec::new(),
                client_public_keys: Vec::new(),
                known_public_keys: BTreeMap::new(),
            },
            storage_root: PathBuf::from("/tmp/imago-test-storage"),
            runtime: RuntimeConfig::default(),
            server_version: "imagod/test".to_string(),
        }
    }

    #[test]
    fn given_invalid_listen_addr__when_build_server__then_bad_request_is_returned() {
        let config = sample_config("not-an-addr".to_string());

        let err = match build_server(&config) {
            Ok(_) => panic!("invalid listen_addr must fail"),
            Err(err) => err,
        };
        assert_eq!(err.code, imagod_spec::ErrorCode::BadRequest);
        assert_eq!(err.stage, super::STAGE_TRANSPORT);
        assert!(err.message.contains("listen_addr parse failed"));
    }

    #[test]
    fn given_valid_config__when_build_server__then_server_is_created() {
        let config = sample_config("127.0.0.1:0".to_string());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build");
        runtime.block_on(async {
            let server = build_server(&config).expect("valid config should build server");
            drop(server);
        });
    }

    #[test]
    fn given_port_already_bound__when_build_server__then_internal_error_is_returned() {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("udp bind should succeed");
        let addr = socket.local_addr().expect("bound addr should be available");
        let config = sample_config(addr.to_string());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build");
        runtime.block_on(async {
            let err = match build_server(&config) {
                Ok(_) => panic!("quic endpoint bind should fail"),
                Err(err) => err,
            };
            assert_eq!(err.code, imagod_spec::ErrorCode::Internal);
            assert_eq!(err.stage, super::STAGE_TRANSPORT);
            assert!(err.message.contains("endpoint bind failed"));
        });
    }
}
