use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::Manifest;

pub(crate) struct ArtifactBundleRequest<'a> {
    pub(crate) manifest: &'a Manifest,
    pub(crate) manifest_path: &'a Path,
    pub(crate) project_root: &'a Path,
    pub(crate) dependency_component_sources: &'a BTreeMap<String, PathBuf>,
}

pub(crate) trait ArtifactBundler {
    fn bundle(
        &self,
        request: ArtifactBundleRequest<'_>,
    ) -> anyhow::Result<super::TempArtifactBundle>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TarArtifactBundler;

impl ArtifactBundler for TarArtifactBundler {
    fn bundle(
        &self,
        request: ArtifactBundleRequest<'_>,
    ) -> anyhow::Result<super::TempArtifactBundle> {
        super::build_artifact_bundle_file(
            request.manifest,
            request.manifest_path,
            request.project_root,
            request.dependency_component_sources,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::PathBuf};

    use super::{ArtifactBundleRequest, ArtifactBundler, Manifest, TarArtifactBundler};

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-deploy-artifact-tests-{test_name}-{}-{}",
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
    fn tar_artifact_bundler_bundles_valid_manifest() {
        let root = new_temp_dir("success");
        let manifest_path = root.join("build/manifest.json");
        let main_path = root.join("build/main.wasm");
        write(&manifest_path, br#"{"name":"svc"}"#);
        write(&main_path, b"wasm-binary");

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "main.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };
        let bundler = TarArtifactBundler;
        let component_sources = BTreeMap::new();

        let bundle = bundler
            .bundle(ArtifactBundleRequest {
                manifest: &manifest,
                manifest_path: PathBuf::from("build/manifest.json").as_path(),
                project_root: root.as_path(),
                dependency_component_sources: &component_sources,
            })
            .expect("bundle should succeed");

        assert!(bundle.path().is_file(), "bundle file must exist");
    }

    #[test]
    fn tar_artifact_bundler_rejects_unsafe_main_path() {
        let root = new_temp_dir("unsafe-main");
        let manifest_path = root.join("build/manifest.json");
        write(&manifest_path, br#"{"name":"svc"}"#);

        let manifest = Manifest {
            name: "svc".to_string(),
            main: "../evil.wasm".to_string(),
            app_type: "cli".to_string(),
            assets: vec![],
            dependencies: vec![],
        };
        let bundler = TarArtifactBundler;
        let component_sources = BTreeMap::new();

        let err = bundler
            .bundle(ArtifactBundleRequest {
                manifest: &manifest,
                manifest_path: PathBuf::from("build/manifest.json").as_path(),
                project_root: root.as_path(),
                dependency_component_sources: &component_sources,
            })
            .expect_err("unsafe main path must fail");

        assert!(err.to_string().contains("path traversal"));
    }
}
