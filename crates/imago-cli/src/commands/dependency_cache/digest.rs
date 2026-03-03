use std::{
    fs,
    io::Read,
    path::{Component, Path},
};

use anyhow::{Context, anyhow};
use sha2::{Digest, Sha256};

pub(super) fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
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
    Ok(hex::encode(hasher.finalize()))
}

pub(super) fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
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
    Ok(hex::encode(hasher.finalize()))
}

fn normalized_path_to_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().replace('\\', "/")),
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
            "imago-cli-dependency-cache-digest-tests-{test_name}-{}-{}",
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
    fn compute_path_digest_hex_is_order_independent_for_directory_entries() {
        let root = new_temp_dir("deterministic");
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        fs::create_dir_all(&dir_a).expect("dir a should be created");
        fs::create_dir_all(&dir_b).expect("dir b should be created");

        write(&dir_a.join("nested/one.wit"), b"one");
        write(&dir_a.join("two.wit"), b"two");
        write(&dir_b.join("two.wit"), b"two");
        write(&dir_b.join("nested/one.wit"), b"one");

        let digest_a = compute_path_digest_hex(&dir_a).expect("digest should compute");
        let digest_b = compute_path_digest_hex(&dir_b).expect("digest should compute");
        assert_eq!(digest_a, digest_b);
    }

    #[test]
    fn compute_path_digest_hex_matches_sha_for_file_path() {
        let root = new_temp_dir("file");
        let file = root.join("package.wit");
        write(&file, b"package test:demo@0.1.0;\n");

        let digest = compute_path_digest_hex(&file).expect("path digest should compute");
        let sha = compute_sha256_hex(&file).expect("sha should compute");
        assert_eq!(digest, sha);
    }

    #[cfg(unix)]
    #[test]
    fn compute_sha256_hex_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("sha-symlink");
        let file = root.join("real.wit");
        let link = root.join("link.wit");
        write(&file, b"package demo:test@0.1.0;\n");
        symlink(&file, &link).expect("symlink should be created");

        let err = compute_sha256_hex(&link).expect_err("symlink path must fail");
        assert!(err.to_string().contains("symlink paths are not allowed"));
    }

    #[cfg(unix)]
    #[test]
    fn compute_path_digest_hex_rejects_symlink_entry_in_directory() {
        use std::os::unix::fs::symlink;

        let root = new_temp_dir("dir-symlink-entry");
        let dir = root.join("wit");
        fs::create_dir_all(&dir).expect("wit dir should be created");
        write(&dir.join("package.wit"), b"package demo:test@0.1.0;\n");
        symlink(dir.join("package.wit"), dir.join("link.wit")).expect("symlink should be created");

        let err = compute_path_digest_hex(&dir).expect_err("symlink entry must fail");
        assert!(err.to_string().contains("symlink paths are not allowed"));
    }

    #[cfg(unix)]
    #[test]
    fn compute_path_digest_hex_rejects_non_file_non_directory() {
        let err = compute_path_digest_hex(std::path::Path::new("/dev/null"))
            .expect_err("character device should be rejected");
        assert!(err.to_string().contains("not file or directory"));
    }
}
