use std::path::Path;

use crate::commands::{
    build, dependency_cache::DependencyCacheEntry, shared::dependency::DependencyResolver,
};

pub(crate) async fn load_or_refresh_cache_entry<R: DependencyResolver>(
    resolver: &R,
    project_root: &Path,
    dependency: &build::ProjectDependency,
) -> anyhow::Result<DependencyCacheEntry> {
    resolver
        .load_or_refresh_cache_entry(project_root, dependency)
        .await
}
