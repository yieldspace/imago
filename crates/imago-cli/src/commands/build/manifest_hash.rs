use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use sha2::{Digest, Sha256};

use super::{AssetSource, Manifest, config_parse::normalized_path_to_string};

pub(super) fn compute_manifest_hash(
    project_root: &Path,
    main_path: &Path,
    assets: &[AssetSource],
    manifest: &Manifest,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, &project_root.join(main_path), "main wasm")?;

    let normalized_manifest =
        serde_json::to_vec(manifest).context("failed to serialize normalized manifest for hash")?;
    hasher.update(&normalized_manifest);

    let mut sorted_assets = assets.iter().collect::<Vec<_>>();
    sorted_assets.sort_by(|a, b| a.manifest_asset.path.cmp(&b.manifest_asset.path));
    for asset in sorted_assets {
        hash_file_into(
            &mut hasher,
            &project_root.join(&asset.source_path),
            "asset for hash",
        )?;
    }

    Ok(hex::encode(hasher.finalize()))
}

pub(super) fn compute_sha256_hex(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hash_file_into(&mut hasher, path, "file for sha256")?;
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn compute_path_digest_hex(path: &Path) -> anyhow::Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read path for digest: {}", path.display()))?;
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
            let entry_metadata = entry
                .metadata()
                .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;
            if entry_metadata.is_dir() {
                stack.push(entry_path);
            } else if entry_metadata.is_file() {
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

fn ensure_build_dir(project_root: &Path) -> anyhow::Result<PathBuf> {
    let build_dir = project_root.join("build");
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create build directory: {}", build_dir.display()))?;
    Ok(build_dir)
}

pub(super) fn materialize_hashed_wasm(
    project_root: &Path,
    source_main_path: &Path,
    service_name: &str,
) -> anyhow::Result<PathBuf> {
    let source = project_root.join(source_main_path);
    let digest = compute_sha256_hex(&source)?;
    let build_dir = ensure_build_dir(project_root)?;
    let file_name = format!("{digest}-{service_name}.wasm");
    let destination = build_dir.join(file_name);

    if destination.exists() {
        let metadata = fs::metadata(&destination).with_context(|| {
            format!(
                "failed to inspect materialized wasm: {}",
                destination.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(anyhow!(
                "materialized wasm path is not a file: {}",
                destination.display()
            ));
        }

        let existing_digest = compute_sha256_hex(&destination).with_context(|| {
            format!(
                "failed to verify materialized wasm hash: {}",
                destination.display()
            )
        })?;
        if existing_digest != digest {
            copy_materialized_wasm(&source, &destination)?;
        }
    } else {
        copy_materialized_wasm(&source, &destination)?;
    }

    Ok(PathBuf::from("build").join(
        destination
            .file_name()
            .ok_or_else(|| anyhow!("materialized wasm filename is missing"))?,
    ))
}

fn copy_materialized_wasm(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy wasm from {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use super::{
        AssetSource, Manifest, compute_manifest_hash, compute_sha256_hex, materialize_hashed_wasm,
    };

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-manifest-hash-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn sample_manifest() -> Manifest {
        Manifest {
            name: "svc".to_string(),
            main: "app.wasm".to_string(),
            app_type: "cli".to_string(),
            target: BTreeMap::new(),
            assets: Vec::new(),
            bindings: Vec::new(),
            http: None,
            socket: None,
            resources: None,
            dependencies: Vec::new(),
            capabilities: super::super::ManifestCapabilityPolicy::default(),
            hash: super::super::ManifestHash {
                algorithm: "sha256".to_string(),
                value: "placeholder".to_string(),
                targets: vec![],
            },
        }
    }

    #[test]
    fn compute_manifest_hash_is_independent_from_asset_input_order() {
        let root = new_temp_dir("manifest-hash-order");
        write(&root.join("main.wasm"), b"main");
        write(&root.join("assets/b.txt"), b"asset-b");
        write(&root.join("assets/a.txt"), b"asset-a");
        let manifest = sample_manifest();

        let assets_a = vec![
            AssetSource {
                manifest_asset: super::super::ManifestAsset {
                    path: "assets/b.txt".to_string(),
                    extra: BTreeMap::new(),
                },
                source_path: PathBuf::from("assets/b.txt"),
            },
            AssetSource {
                manifest_asset: super::super::ManifestAsset {
                    path: "assets/a.txt".to_string(),
                    extra: BTreeMap::new(),
                },
                source_path: PathBuf::from("assets/a.txt"),
            },
        ];
        let assets_b = vec![assets_a[1].clone(), assets_a[0].clone()];

        let hash_a = compute_manifest_hash(&root, Path::new("main.wasm"), &assets_a, &manifest)
            .expect("manifest hash should compute");
        let hash_b = compute_manifest_hash(&root, Path::new("main.wasm"), &assets_b, &manifest)
            .expect("manifest hash should compute");
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn materialize_hashed_wasm_reuses_existing_content_and_overwrites_when_mismatched() {
        let root = new_temp_dir("materialize");
        write(&root.join("app.wasm"), b"wasm-v1");

        let relative = materialize_hashed_wasm(&root, Path::new("app.wasm"), "svc")
            .expect("first materialization should succeed");
        let output = root.join(&relative);
        assert!(output.is_file());
        let initial_sha = compute_sha256_hex(&output).expect("sha should compute");

        let relative_second = materialize_hashed_wasm(&root, Path::new("app.wasm"), "svc")
            .expect("second materialization should reuse file");
        assert_eq!(relative_second, relative);
        let reused_sha = compute_sha256_hex(&output).expect("sha should compute");
        assert_eq!(reused_sha, initial_sha);

        write(&output, b"corrupted-output");
        materialize_hashed_wasm(&root, Path::new("app.wasm"), "svc")
            .expect("materialization should repair mismatched output");
        let repaired_sha = compute_sha256_hex(&output).expect("sha should compute");
        let source_sha = compute_sha256_hex(&root.join("app.wasm")).expect("sha should compute");
        assert_eq!(repaired_sha, source_sha);
    }
}
