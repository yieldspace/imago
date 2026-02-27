use std::{io::BufReader, path::Path, sync::Arc};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_config::ImagodConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, SubjectPublicKeyInfoDer, UnixTime};

use super::STAGE_TRANSPORT;
use crate::protocol_handler::{
    is_tls_client_key_allowlisted, sync_dynamic_public_keys_from_config,
};

const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub(crate) fn build_tls_server_config(
    config: &ImagodConfig,
) -> Result<rustls::ServerConfig, ImagodError> {
    let provider = web_transport_quinn::crypto::default_provider();
    let server_key = load_server_raw_public_key(&config.tls.server_key, &provider)?;
    sync_dynamic_public_keys_from_config(config)?;

    let client_verifier =
        RawPublicKeyClientVerifier::new(provider.signature_verification_algorithms);

    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_TRANSPORT,
                format!("TLS protocol setup failed: {e}"),
            )
        })?
        .with_client_cert_verifier(Arc::new(client_verifier))
        .with_cert_resolver(Arc::new(
            rustls::server::AlwaysResolvesServerRawPublicKeys::new(server_key),
        ));

    tls.alpn_protocols = vec![web_transport_quinn::ALPN.as_bytes().to_vec()];
    tls.max_early_data_size = 0;

    Ok(tls)
}

#[derive(Debug)]
struct RawPublicKeyClientVerifier {
    supported_algs: rustls::crypto::WebPkiSupportedAlgorithms,
    root_hint_subjects: Vec<rustls::DistinguishedName>,
}

impl RawPublicKeyClientVerifier {
    fn new(supported_algs: rustls::crypto::WebPkiSupportedAlgorithms) -> Self {
        Self {
            supported_algs,
            root_hint_subjects: Vec::new(),
        }
    }
}

impl rustls::server::danger::ClientCertVerifier for RawPublicKeyClientVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &self.root_hint_subjects
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::General(
                "client raw public key authentication does not accept intermediates".to_string(),
            ));
        }

        let client_key =
            extract_ed25519_raw_public_key(end_entity.as_ref(), "client raw public key")
                .map_err(rustls::Error::General)?;

        if !is_tls_client_key_allowlisted(&client_key) {
            return Err(rustls::Error::General(
                "client raw public key is not present in tls.client_public_keys/admin_public_keys"
                    .to_string(),
            ));
        }

        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "TLS1.2 client signatures are not supported for raw public keys".to_string(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        let spki = SubjectPublicKeyInfoDer::from(cert.as_ref());
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &spki,
            dss,
            &self.supported_algs,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

fn load_server_raw_public_key(
    path: &Path,
    provider: &web_transport_quinn::crypto::Provider,
) -> Result<Arc<rustls::sign::CertifiedKey>, ImagodError> {
    let private_key = load_private_key(path)?;
    let signing_key = provider
        .key_provider
        .load_private_key(private_key)
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_TRANSPORT,
                format!("failed to load key {}: {e}", path.display()),
            )
        })?;

    if signing_key.algorithm() != rustls::SignatureAlgorithm::ED25519 {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            "server key must be ed25519 for raw public key TLS",
        ));
    }

    let spki = signing_key.public_key().ok_or_else(|| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("failed to derive public key from {}", path.display()),
        )
    })?;

    extract_ed25519_raw_public_key(spki.as_ref(), "server raw public key").map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_TRANSPORT,
            format!("invalid server key {}: {e}", path.display()),
        )
    })?;

    Ok(Arc::new(rustls::sign::CertifiedKey::new(
        vec![CertificateDer::from(spki.as_ref().to_vec())],
        signing_key,
    )))
}

fn extract_ed25519_raw_public_key(spki_der: &[u8], label: &str) -> Result<[u8; 32], String> {
    if spki_der.len() != ED25519_SPKI_PREFIX.len() + 32 {
        return Err(format!(
            "{label} must be ed25519 (expected 32-byte raw key)"
        ));
    }

    if !spki_der.starts_with(&ED25519_SPKI_PREFIX) {
        return Err(format!("{label} must be ed25519"));
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&spki_der[ED25519_SPKI_PREFIX.len()..]);
    Ok(out)
}

/// Loads a private key from a PEM file.
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

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    use std::{collections::BTreeMap, path::Path, path::PathBuf, sync::Mutex, time::Duration};

    use super::*;
    use crate::protocol_handler::{
        replace_dynamic_public_keys_for_tests, upsert_dynamic_client_public_key,
    };
    use imagod_config::{ImagodConfig, RuntimeConfig, TlsConfig};

    static DYNAMIC_KEYS_TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn ed25519_spki_from_raw(raw: [u8; 32]) -> Vec<u8> {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&raw);
        spki
    }

    fn hex_32(byte: u8) -> String {
        let mut out = String::with_capacity(64);
        for _ in 0..32 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    fn test_server_key_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/local-imagod-plugin-hello/certs/server.key")
    }

    fn sample_config_with_server_key(path: PathBuf) -> ImagodConfig {
        ImagodConfig {
            listen_addr: "127.0.0.1:4443".to_string(),
            tls: TlsConfig {
                server_key: path,
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
    fn extracts_ed25519_raw_public_key_from_spki() {
        let raw = [0x7au8; 32];
        let extracted =
            extract_ed25519_raw_public_key(&ed25519_spki_from_raw(raw), "client raw public key")
                .expect("ed25519 key should parse");
        assert_eq!(extracted, raw);
    }

    #[test]
    fn rejects_non_ed25519_spki_prefix() {
        let mut spki = ed25519_spki_from_raw([0x11u8; 32]);
        spki[8] = 0x71;

        let err = extract_ed25519_raw_public_key(&spki, "client raw public key")
            .expect_err("non-ed25519 key should be rejected");
        assert!(err.contains("ed25519"));
    }

    #[test]
    fn verifier_accepts_allowlisted_client_key() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        let key = [0x11u8; 32];
        replace_dynamic_public_keys_for_tests(&[], &[key]);

        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );

        let cert = CertificateDer::from(ed25519_spki_from_raw(key));
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));
        let result = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[],
            now,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn verifier_rejects_non_allowlisted_client_key() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        replace_dynamic_public_keys_for_tests(&[], &[[0x11u8; 32]]);

        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );

        let cert = CertificateDer::from(ed25519_spki_from_raw([0x22u8; 32]));
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));
        let err = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[],
            now,
        )
        .expect_err("missing key should be rejected");

        match err {
            rustls::Error::General(msg) => assert!(msg.contains("tls.client_public_keys")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn verifier_reflects_dynamic_allowlist_update() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        replace_dynamic_public_keys_for_tests(&[], &[]);

        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );
        let cert = CertificateDer::from(ed25519_spki_from_raw([0x44u8; 32]));
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));
        let initial = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[],
            now,
        );
        assert!(initial.is_err());

        upsert_dynamic_client_public_key(&hex_32(0x44))
            .expect("dynamic allowlist update should succeed");
        let updated = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[],
            now,
        );
        assert!(updated.is_ok());
    }

    #[test]
    fn verifier_rejects_non_ed25519_client_key() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        replace_dynamic_public_keys_for_tests(&[], &[[0x11u8; 32]]);

        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );

        let mut non_ed25519 = ed25519_spki_from_raw([0x11u8; 32]);
        non_ed25519[8] = 0x71;
        let cert = CertificateDer::from(non_ed25519);
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));

        let err = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[],
            now,
        )
        .expect_err("non-ed25519 key should be rejected");

        match err {
            rustls::Error::General(msg) => assert!(msg.contains("ed25519")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn verifier_supports_only_ed25519_signature_scheme() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        replace_dynamic_public_keys_for_tests(&[], &[[0x11u8; 32]]);

        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );

        let schemes =
            rustls::server::danger::ClientCertVerifier::supported_verify_schemes(&verifier);
        assert_eq!(schemes, vec![rustls::SignatureScheme::ED25519]);
    }

    #[test]
    fn given_intermediate_certs__when_verify_client_cert__then_rejected() {
        let _guard = DYNAMIC_KEYS_TEST_MUTEX
            .lock()
            .expect("mutex lock should succeed");
        let key = [0x11u8; 32];
        replace_dynamic_public_keys_for_tests(&[], &[key]);
        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );
        let cert = CertificateDer::from(ed25519_spki_from_raw(key));
        let intermediate = CertificateDer::from(ed25519_spki_from_raw([0x22u8; 32]));
        let now = UnixTime::since_unix_epoch(Duration::from_secs(0));

        let err = rustls::server::danger::ClientCertVerifier::verify_client_cert(
            &verifier,
            &cert,
            &[intermediate],
            now,
        )
        .expect_err("intermediates should be rejected");
        match err {
            rustls::Error::General(message) => {
                assert!(message.contains("does not accept intermediates"))
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn given_verifier__when_query_root_hints_and_raw_key_requirement__then_empty_hints_and_true() {
        let verifier = RawPublicKeyClientVerifier::new(
            web_transport_quinn::crypto::default_provider().signature_verification_algorithms,
        );
        let hints = rustls::server::danger::ClientCertVerifier::root_hint_subjects(&verifier);
        assert!(
            hints.is_empty(),
            "raw public key auth should not use DN hints"
        );
        assert!(rustls::server::danger::ClientCertVerifier::requires_raw_public_keys(&verifier));
    }

    #[test]
    fn given_valid_server_key__when_build_tls_server_config__then_alpn_and_early_data_are_configured()
     {
        let config = sample_config_with_server_key(test_server_key_path());
        let tls = build_tls_server_config(&config).expect("tls config build should succeed");

        assert_eq!(tls.max_early_data_size, 0);
        assert_eq!(
            tls.alpn_protocols,
            vec![web_transport_quinn::ALPN.as_bytes()]
        );
    }

    #[test]
    fn given_missing_server_key_file__when_build_tls_server_config__then_bad_request_is_returned() {
        let config = sample_config_with_server_key(PathBuf::from("/no/such/server.key"));
        let err = build_tls_server_config(&config).expect_err("missing key file must fail");

        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(err.stage, STAGE_TRANSPORT);
        assert!(
            err.message.contains("failed to open key"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn given_malformed_or_missing_private_key__when_load_private_key__then_bad_request_is_returned()
    {
        let missing = load_private_key(Path::new("/no/such/private-key.pem"))
            .expect_err("missing key should fail");
        assert_eq!(missing.code, ErrorCode::BadRequest);
        assert_eq!(missing.stage, STAGE_TRANSPORT);
        assert!(missing.message.contains("failed to open key"));

        let malformed_path = std::env::temp_dir().join(format!(
            "imagod-tls-material-malformed-key-{}.pem",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &malformed_path,
            "-----BEGIN PRIVATE KEY-----\nnot-base64\n-----END PRIVATE KEY-----\n",
        )
        .expect("malformed key write should succeed");
        let malformed = load_private_key(&malformed_path)
            .expect_err("malformed key should fail to parse or be treated as missing");
        assert_eq!(malformed.code, ErrorCode::BadRequest);
        assert_eq!(malformed.stage, STAGE_TRANSPORT);
        assert!(
            malformed.message.contains("failed to parse key")
                || malformed.message.contains("private key is missing"),
            "unexpected malformed-key message: {}",
            malformed.message
        );
        let _ = std::fs::remove_file(malformed_path);
    }

    #[test]
    fn given_invalid_spki_shape__when_extract_ed25519_raw_public_key__then_error_mentions_expected_format()
     {
        let short = vec![0u8; ED25519_SPKI_PREFIX.len() + 31];
        let short_err = extract_ed25519_raw_public_key(&short, "client raw public key")
            .expect_err("short key must be rejected");
        assert!(short_err.contains("expected 32-byte raw key"));

        let mut wrong_prefix = ed25519_spki_from_raw([0x33u8; 32]);
        wrong_prefix[8] = 0x71;
        let prefix_err = extract_ed25519_raw_public_key(&wrong_prefix, "client raw public key")
            .expect_err("wrong prefix must be rejected");
        assert!(prefix_err.contains("must be ed25519"));
    }
}
