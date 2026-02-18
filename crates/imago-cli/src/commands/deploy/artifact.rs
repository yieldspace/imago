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
