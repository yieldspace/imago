use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ED25519,
};
use time::{Duration, OffsetDateTime};

use crate::cli::CertsGenerateArgs;

use super::CommandResult;

const GITIGNORE_CONTENT: &str = "*\n!.gitignore\n";

#[derive(Debug)]
struct OutputPaths {
    ca_crt: PathBuf,
    ca_key: PathBuf,
    server_crt: PathBuf,
    server_key: PathBuf,
    client_crt: PathBuf,
    client_key: PathBuf,
    gitignore: PathBuf,
}

pub fn run_generate(args: CertsGenerateArgs) -> CommandResult {
    match run_generate_inner(args) {
        Ok(paths) => {
            println!("generated certificates:");
            println!("  {}", paths.ca_crt.display());
            println!("  {}", paths.ca_key.display());
            println!("  {}", paths.server_crt.display());
            println!("  {}", paths.server_key.display());
            println!("  {}", paths.client_crt.display());
            println!("  {}", paths.client_key.display());
            println!("  {}", paths.gitignore.display());
            println!("private keys are sensitive. do not commit or share them.");

            CommandResult {
                exit_code: 0,
                stderr: None,
            }
        }
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(err.to_string()),
        },
    }
}

fn run_generate_inner(args: CertsGenerateArgs) -> anyhow::Result<OutputPaths> {
    let out_dir = args.out_dir;
    let server_ip: IpAddr = args
        .server_ip
        .parse()
        .with_context(|| format!("invalid --server-ip: {}", args.server_ip))?;

    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create out dir: {}", out_dir.display()))?;

    let paths = OutputPaths {
        ca_crt: out_dir.join("ca.crt"),
        ca_key: out_dir.join("ca.key"),
        server_crt: out_dir.join("server.crt"),
        server_key: out_dir.join("server.key"),
        client_crt: out_dir.join("client.crt"),
        client_key: out_dir.join("client.key"),
        gitignore: out_dir.join(".gitignore"),
    };

    ensure_writable_targets(&paths, args.force)?;

    let now = OffsetDateTime::now_utc();
    let not_before = now - Duration::days(1);
    let not_after = now + Duration::days(i64::from(args.days));

    let ca_key = KeyPair::generate_for(&PKCS_ED25519).context("failed to generate CA keypair")?;
    let ca_key_pem = ca_key.serialize_pem();

    let mut ca_params = CertificateParams::new(Vec::<String>::new())
        .context("failed to build CA certificate params")?;
    ca_params.not_before = not_before;
    ca_params.not_after = not_after;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "imago local ca");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];

    let ca_cert = ca_params
        .self_signed(&ca_key)
        .context("failed to generate CA certificate")?;
    let ca_cert_pem = ca_cert.pem();
    let ca_issuer = Issuer::new(ca_params, ca_key);

    let server_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate server keypair")?;
    let server_key_pem = server_key.serialize_pem();
    let mut server_params =
        CertificateParams::new(vec![args.server_name.clone(), server_ip.to_string()])
            .context("failed to build server certificate params")?;
    server_params.not_before = not_before;
    server_params.not_after = not_after;
    server_params.use_authority_key_identifier_extension = true;
    server_params.distinguished_name = DistinguishedName::new();
    server_params
        .distinguished_name
        .push(DnType::CommonName, args.server_name.clone());
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let server_cert = server_params
        .signed_by(&server_key, &ca_issuer)
        .context("failed to generate server certificate")?;
    let server_cert_pem = server_cert.pem();

    let client_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate client keypair")?;
    let client_key_pem = client_key.serialize_pem();
    let mut client_params = CertificateParams::new(Vec::<String>::new())
        .context("failed to build client certificate params")?;
    client_params.not_before = not_before;
    client_params.not_after = not_after;
    client_params.use_authority_key_identifier_extension = true;
    client_params.distinguished_name = DistinguishedName::new();
    client_params
        .distinguished_name
        .push(DnType::CommonName, "imago local client");
    client_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    let client_cert = client_params
        .signed_by(&client_key, &ca_issuer)
        .context("failed to generate client certificate")?;
    let client_cert_pem = client_cert.pem();

    write_text(&paths.ca_crt, &ca_cert_pem)?;
    write_text(&paths.ca_key, &ca_key_pem)?;
    write_text(&paths.server_crt, &server_cert_pem)?;
    write_text(&paths.server_key, &server_key_pem)?;
    write_text(&paths.client_crt, &client_cert_pem)?;
    write_text(&paths.client_key, &client_key_pem)?;
    write_text(&paths.gitignore, GITIGNORE_CONTENT)?;

    Ok(paths)
}

fn ensure_writable_targets(paths: &OutputPaths, force: bool) -> anyhow::Result<()> {
    let all_paths = [
        &paths.ca_crt,
        &paths.ca_key,
        &paths.server_crt,
        &paths.server_key,
        &paths.client_crt,
        &paths.client_key,
        &paths.gitignore,
    ];

    if force {
        return Ok(());
    }

    let mut existing = Vec::new();
    for path in all_paths {
        if path.exists() {
            existing.push(path.display().to_string());
        }
    }

    if existing.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "output files already exist:\n{}\nrerun with --force to overwrite",
        existing.join("\n")
    ))
}

fn write_text(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generates_all_files_and_valid_pem() {
        let dir = temp_dir("generates_all_files_and_valid_pem");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let paths = run_generate_inner(args).expect("certificate generation should succeed");

        assert!(paths.ca_crt.exists());
        assert!(paths.ca_key.exists());
        assert!(paths.server_crt.exists());
        assert!(paths.server_key.exists());
        assert!(paths.client_crt.exists());
        assert!(paths.client_key.exists());
        assert!(paths.gitignore.exists());

        let gitignore = std::fs::read_to_string(&paths.gitignore).expect("read .gitignore");
        assert_eq!(gitignore, GITIGNORE_CONTENT);

        assert_has_certificate(&paths.ca_crt);
        assert_has_certificate(&paths.server_crt);
        assert_has_certificate(&paths.client_crt);
        assert_has_private_key(&paths.ca_key);
        assert_has_private_key(&paths.server_key);
        assert_has_private_key(&paths.client_key);

        cleanup(&dir);
    }

    #[test]
    fn fails_without_force_when_file_exists() {
        let dir = temp_dir("fails_without_force_when_file_exists");
        let existing = dir.join("ca.crt");
        std::fs::write(&existing, "dummy").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let err = run_generate_inner(args).expect_err("generation should fail");
        let message = err.to_string();
        assert!(message.contains("--force"));
        assert!(message.contains("ca.crt"));

        cleanup(&dir);
    }

    #[test]
    fn force_overwrites_existing_outputs() {
        let dir = temp_dir("force_overwrites_existing_outputs");
        let existing = dir.join("ca.crt");
        std::fs::write(&existing, "old").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: true,
        };

        let paths = run_generate_inner(args).expect("generation with --force should succeed");
        let ca_crt = std::fs::read_to_string(paths.ca_crt).expect("read ca certificate");
        assert!(ca_crt.contains("BEGIN CERTIFICATE"));

        cleanup(&dir);
    }

    fn assert_has_certificate(path: &Path) {
        let file = std::fs::File::open(path).expect("open cert");
        let mut reader = BufReader::new(file);
        let certs = rustls_pemfile::certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .expect("parse cert PEM");
        assert!(
            !certs.is_empty(),
            "cert should not be empty: {}",
            path.display()
        );
    }

    fn assert_has_private_key(path: &Path) {
        let file = std::fs::File::open(path).expect("open key");
        let mut reader = BufReader::new(file);
        let key = rustls_pemfile::private_key(&mut reader)
            .expect("parse key PEM")
            .expect("key should exist");
        let key_bytes = key.secret_der();
        assert!(
            !key_bytes.is_empty(),
            "key should not be empty: {}",
            path.display()
        );
    }

    fn temp_dir(test_name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("imago-cli-certs-{test_name}-{ts}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }
}
