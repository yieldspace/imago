use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use rcgen::{KeyPair, PKCS_ED25519};

use crate::cli::CertsGenerateArgs;

use super::CommandResult;

const GITIGNORE_CONTENT: &str = "*\n!.gitignore\n";

#[derive(Debug)]
struct OutputPaths {
    server_key: PathBuf,
    client_key: PathBuf,
    server_pub_hex: PathBuf,
    client_pub_hex: PathBuf,
    gitignore: PathBuf,
}

pub fn run_generate(args: CertsGenerateArgs) -> CommandResult {
    match run_generate_inner(args) {
        Ok(paths) => {
            println!("generated key material:");
            println!("  {}", paths.server_key.display());
            println!("  {}", paths.client_key.display());
            println!("  {}", paths.server_pub_hex.display());
            println!("  {}", paths.client_pub_hex.display());
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
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create out dir: {}", out_dir.display()))?;

    let paths = OutputPaths {
        server_key: out_dir.join("server.key"),
        client_key: out_dir.join("client.key"),
        server_pub_hex: out_dir.join("server.pub.hex"),
        client_pub_hex: out_dir.join("client.pub.hex"),
        gitignore: out_dir.join(".gitignore"),
    };

    ensure_writable_targets(&paths, args.force)?;

    let server_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate server keypair")?;
    let client_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate client keypair")?;

    write_private_key(&paths.server_key, &server_key.serialize_pem())?;
    write_private_key(&paths.client_key, &client_key.serialize_pem())?;
    write_text(
        &paths.server_pub_hex,
        &format!("{}\n", hex::encode(server_key.public_key_raw())),
    )?;
    write_text(
        &paths.client_pub_hex,
        &format!("{}\n", hex::encode(client_key.public_key_raw())),
    )?;
    write_text(&paths.gitignore, GITIGNORE_CONTENT)?;

    Ok(paths)
}

fn ensure_writable_targets(paths: &OutputPaths, force: bool) -> anyhow::Result<()> {
    let all_paths = [
        &paths.server_key,
        &paths.client_key,
        &paths.server_pub_hex,
        &paths.client_pub_hex,
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

#[cfg(unix)]
fn write_private_key(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(not(unix))]
fn write_private_key(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_text(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generates_all_files_and_valid_payloads() {
        let dir = temp_dir("generates_all_files_and_valid_payloads");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let paths = run_generate_inner(args).expect("key generation should succeed");

        assert!(paths.server_key.exists());
        assert!(paths.client_key.exists());
        assert!(paths.server_pub_hex.exists());
        assert!(paths.client_pub_hex.exists());
        assert!(paths.gitignore.exists());

        let gitignore = std::fs::read_to_string(&paths.gitignore).expect("read .gitignore");
        assert_eq!(gitignore, GITIGNORE_CONTENT);

        assert_has_private_key(&paths.server_key);
        assert_has_private_key(&paths.client_key);
        assert_public_key_hex(&paths.server_pub_hex);
        assert_public_key_hex(&paths.client_pub_hex);

        assert_public_key_matches_private(&paths.server_key, &paths.server_pub_hex);
        assert_public_key_matches_private(&paths.client_key, &paths.client_pub_hex);

        cleanup(&dir);
    }

    #[test]
    fn fails_without_force_when_file_exists() {
        let dir = temp_dir("fails_without_force_when_file_exists");
        let existing = dir.join("server.key");
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
        assert!(message.contains("server.key"));

        cleanup(&dir);
    }

    #[test]
    fn force_overwrites_existing_outputs() {
        let dir = temp_dir("force_overwrites_existing_outputs");
        let existing = dir.join("server.key");
        std::fs::write(&existing, "old").expect("create existing file");

        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: true,
        };

        let paths = run_generate_inner(args).expect("generation with --force should succeed");
        let server_key = std::fs::read_to_string(paths.server_key).expect("read server key");
        assert!(server_key.contains("BEGIN PRIVATE KEY"));

        cleanup(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_keys_are_written_with_strict_permissions() {
        let dir = temp_dir("private_keys_are_written_with_strict_permissions");
        let args = CertsGenerateArgs {
            out_dir: dir.clone(),
            server_name: "localhost".to_string(),
            server_ip: "127.0.0.1".to_string(),
            days: 3650,
            force: false,
        };

        let paths = run_generate_inner(args).expect("key generation should succeed");
        assert_mode_0600(&paths.server_key);
        assert_mode_0600(&paths.client_key);

        cleanup(&dir);
    }

    fn assert_public_key_hex(path: &Path) {
        let value = std::fs::read_to_string(path).expect("public key file should be readable");
        let trimmed = value.trim();
        let decoded = hex::decode(trimmed).expect("public key must be hex");
        assert_eq!(decoded.len(), 32, "ed25519 public key must be 32 bytes");
    }

    fn assert_public_key_matches_private(private_key_path: &Path, public_key_hex_path: &Path) {
        let private_key_pem =
            std::fs::read_to_string(private_key_path).expect("private key should be readable");
        let key_pair = KeyPair::from_pem(&private_key_pem).expect("private key should parse");
        let expected = hex::encode(key_pair.public_key_raw());

        let actual = std::fs::read_to_string(public_key_hex_path)
            .expect("public key should be readable")
            .trim()
            .to_string();
        assert_eq!(actual, expected);
    }

    fn assert_has_private_key(path: &Path) {
        let file = std::fs::File::open(path).expect("open key");
        let mut reader = std::io::BufReader::new(file);
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

    #[cfg(unix)]
    fn assert_mode_0600(path: &Path) {
        let mode = std::fs::metadata(path)
            .expect("metadata should be available")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode for {} should be 0600", path.display());
    }
}
