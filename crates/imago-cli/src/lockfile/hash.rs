use std::{fs, io::Read, path::Path};

use anyhow::{Context, anyhow};
use sha2::{Digest, Sha256};

pub trait DigestProvider {
    fn compute_sha256_hex(&self, path: &Path) -> anyhow::Result<String>;
    fn compute_path_digest_hex(&self, path: &Path) -> anyhow::Result<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Sha256DigestProvider;

impl DigestProvider for Sha256DigestProvider {
    fn compute_sha256_hex(&self, path: &Path) -> anyhow::Result<String> {
        compute_sha256_hex(path)
    }

    fn compute_path_digest_hex(&self, path: &Path) -> anyhow::Result<String> {
        compute_path_digest_hex(path)
    }
}

pub(crate) fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect file for sha256: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "symlink paths are not allowed while hashing: {}",
            path.display()
        ));
    }
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, path, "file for sha256")?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to read path for digest: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "symlink paths are not allowed while hashing: {}",
            path.display()
        ));
    }
    if metadata.is_file() {
        return compute_sha256_hex(path);
    }
    if !metadata.is_dir() {
        return Err(anyhow!("path is not file or directory: {}", path.display()));
    }

    let mut stack = vec![path.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read directory for digest: {}", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read directory entry while hashing {}",
                    dir.display()
                )
            })?;
            let entry_path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;
            if file_type.is_symlink() {
                return Err(anyhow!(
                    "symlink paths are not allowed while hashing: {}",
                    entry_path.display()
                ));
            }
            if file_type.is_dir() {
                stack.push(entry_path);
            } else if file_type.is_file() {
                files.push(entry_path);
            }
        }
    }
    files.sort();

    let mut hasher = Sha256::new();
    for file in files {
        let rel = file
            .strip_prefix(path)
            .with_context(|| format!("failed to relativize digest path: {}", file.display()))?;
        hasher.update(normalized_path_to_string(rel).as_bytes());
        hasher.update([0]);
        hash_file_into(&mut hasher, &file, "directory digest file")?;
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn normalized_path_to_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_file_into(hasher: &mut Sha256, path: &Path, context_label: &str) -> anyhow::Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read {}: {}", context_label, path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{compute_path_digest_hex, compute_sha256_hex};

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-lockfile-hash-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &PathBuf, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    #[test]
    fn compute_path_digest_hex_is_deterministic_for_same_tree_with_different_creation_order() {
        let root = new_temp_dir("deterministic");
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir_all(&a).expect("a dir should be created");
        fs::create_dir_all(&b).expect("b dir should be created");

        write(&a.join("nested/one.txt"), b"one");
        write(&a.join("two.txt"), b"two");

        write(&b.join("two.txt"), b"two");
        write(&b.join("nested/one.txt"), b"one");

        let digest_a = compute_path_digest_hex(&a).expect("digest should compute");
        let digest_b = compute_path_digest_hex(&b).expect("digest should compute");
        assert_eq!(digest_a, digest_b);
    }

    #[test]
    fn compute_path_digest_hex_matches_file_sha_for_single_file() {
        let root = new_temp_dir("single-file");
        let file = root.join("file.wit");
        write(&file, b"package demo:test@0.1.0;\n");

        let path_digest = compute_path_digest_hex(&file).expect("path digest should compute");
        let sha = compute_sha256_hex(&file).expect("sha should compute");
        assert_eq!(path_digest, sha);
    }

    #[cfg(unix)]
    #[test]
    fn compute_path_digest_hex_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("symlink");
        let file = root.join("real.wit");
        let link = root.join("link.wit");
        write(&file, b"package demo:test@0.1.0;\n");
        symlink(&file, &link).expect("symlink should be created");

        let err = compute_path_digest_hex(&link).expect_err("symlink path must fail");
        assert!(err.to_string().contains("symlink paths are not allowed"));
    }
}
