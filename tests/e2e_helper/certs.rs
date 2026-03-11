use anyhow::{Context, Result, bail};
use rcgen::{KeyPair, PKCS_ED25519};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct KeyMaterial {
    pub server_key_path: PathBuf,
    pub admin_key_path: PathBuf,
    pub client_key_path: PathBuf,
    pub server_public_hex: String,
    pub admin_public_hex: String,
    pub client_public_hex: String,
}

pub fn generate_key_material(cert_dir: &Path) -> Result<KeyMaterial> {
    fs::create_dir_all(cert_dir)
        .with_context(|| format!("failed to create cert dir: {}", cert_dir.display()))?;

    let server_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate server keypair")?;
    let admin_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate admin keypair")?;
    let client_key =
        KeyPair::generate_for(&PKCS_ED25519).context("failed to generate client keypair")?;

    let server_key_path = cert_dir.join("s.key");
    let admin_key_path = cert_dir.join("a.key");
    let client_key_path = cert_dir.join("c.key");

    fs::write(&server_key_path, server_key.serialize_pem())
        .with_context(|| format!("failed to write server key: {}", server_key_path.display()))?;
    fs::write(&admin_key_path, admin_key.serialize_pem())
        .with_context(|| format!("failed to write admin key: {}", admin_key_path.display()))?;
    fs::write(&client_key_path, client_key.serialize_pem())
        .with_context(|| format!("failed to write client key: {}", client_key_path.display()))?;

    Ok(KeyMaterial {
        server_key_path,
        admin_key_path,
        client_key_path,
        server_public_hex: public_key_hex(&server_key)?,
        admin_public_hex: public_key_hex(&admin_key)?,
        client_public_hex: public_key_hex(&client_key)?,
    })
}

pub fn write_local_ssh_config(home_dir: &Path) -> Result<PathBuf> {
    let ssh_dir = home_dir.join(".ssh");
    fs::create_dir_all(&ssh_dir)
        .with_context(|| format!("failed to create ssh dir: {}", ssh_dir.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to chmod ssh dir: {}", ssh_dir.display()))?;

    let config_path = ssh_dir.join("config");
    let body = "\
Host *
    BatchMode yes
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
";
    fs::write(&config_path, body)
        .with_context(|| format!("failed to write ssh config: {}", config_path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod ssh config: {}", config_path.display()))?;
    Ok(config_path)
}

fn public_key_hex(key_pair: &KeyPair) -> Result<String> {
    let raw = key_pair.public_key_raw();
    if raw.len() != 32 {
        bail!("unexpected Ed25519 public key length");
    }
    Ok(hex::encode(raw))
}
