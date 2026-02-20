use anyhow::{Context, Result, bail};
use rcgen::{KeyPair, PKCS_ED25519};
use std::fs;
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

#[derive(Debug, Clone)]
pub struct KnownHostEntry {
    pub authority: String,
    pub public_key_hex: String,
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

pub fn write_known_hosts(home_dir: &Path, entries: &[KnownHostEntry]) -> Result<PathBuf> {
    let known_hosts_path = home_dir.join(".imago").join("known_hosts");
    let parent = known_hosts_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "failed to resolve parent for known_hosts: {}",
            known_hosts_path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create known_hosts dir: {}", parent.display()))?;

    let mut body = String::new();
    for entry in entries {
        if entry.authority.is_empty() || entry.public_key_hex.is_empty() {
            bail!("known_hosts entry requires non-empty authority and key");
        }
        body.push_str(&entry.authority);
        body.push('\t');
        body.push_str(&entry.public_key_hex);
        body.push('\n');
    }

    fs::write(&known_hosts_path, body).with_context(|| {
        format!(
            "failed to write known_hosts: {}",
            known_hosts_path.display()
        )
    })?;
    Ok(known_hosts_path)
}

fn public_key_hex(key_pair: &KeyPair) -> Result<String> {
    let raw = key_pair.public_key_raw();
    if raw.len() != 32 {
        bail!("unexpected Ed25519 public key length");
    }
    Ok(hex::encode(raw))
}
