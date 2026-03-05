use crate::config::DependencyKind;
use anyhow::{Context, Result, anyhow};
use semver::Version;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

type DependencyGraph = BTreeMap<String, BTreeMap<String, BTreeSet<DependencyKind>>>;

#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub repo_root: PathBuf,
    pub packages: BTreeMap<String, WorkspacePackage>,
    pub forward_deps: DependencyGraph,
    pub reverse_deps: DependencyGraph,
}

#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub version: Version,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
    resolve: Option<MetadataResolve>,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: String,
}

#[derive(Debug, Deserialize)]
struct MetadataResolve {
    nodes: Vec<MetadataNode>,
}

#[derive(Debug, Deserialize)]
struct MetadataNode {
    id: String,
    deps: Vec<MetadataDep>,
}

#[derive(Debug, Deserialize)]
struct MetadataDep {
    pkg: String,
    #[serde(default)]
    dep_kinds: Vec<MetadataDepKind>,
}

#[derive(Debug, Deserialize)]
struct MetadataDepKind {
    kind: Option<String>,
}

pub fn load(repo_root: &Path) -> Result<WorkspaceInfo> {
    let manifest_path = repo_root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(repo_root)
        .output()
        .with_context(|| "failed to run cargo metadata")?;

    if !output.status.success() {
        return Err(anyhow!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let parsed: CargoMetadata = serde_json::from_slice(&output.stdout)
        .with_context(|| "failed to parse cargo metadata output")?;

    let workspace_member_set: BTreeSet<String> = parsed.workspace_members.into_iter().collect();

    let mut id_to_name = BTreeMap::new();
    let mut packages = BTreeMap::new();

    for package in parsed.packages {
        if !workspace_member_set.contains(&package.id) {
            continue;
        }
        let manifest_path = PathBuf::from(package.manifest_path);
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("invalid manifest path for {}", package.name))?
            .to_path_buf();
        let version = Version::parse(&package.version)
            .with_context(|| format!("invalid package version for {}", package.name))?;

        id_to_name.insert(package.id.clone(), package.name.clone());
        packages.insert(
            package.name.clone(),
            WorkspacePackage {
                manifest_path,
                manifest_dir,
                version,
            },
        );
    }

    let mut forward_deps: DependencyGraph = BTreeMap::new();
    let mut reverse_deps: DependencyGraph = BTreeMap::new();

    if let Some(resolve) = parsed.resolve {
        for node in resolve.nodes {
            let Some(node_name) = id_to_name.get(&node.id) else {
                continue;
            };

            for dep in node.deps {
                let Some(dep_name) = id_to_name.get(&dep.pkg) else {
                    continue;
                };

                let dep_kinds = parse_dependency_kinds(&dep.dep_kinds)?;
                forward_deps
                    .entry(node_name.clone())
                    .or_default()
                    .entry(dep_name.clone())
                    .or_default()
                    .extend(dep_kinds.iter().copied());
                reverse_deps
                    .entry(dep_name.clone())
                    .or_default()
                    .entry(node_name.clone())
                    .or_default()
                    .extend(dep_kinds.iter().copied());
            }
        }
    }

    Ok(WorkspaceInfo {
        repo_root: repo_root.to_path_buf(),
        packages,
        forward_deps,
        reverse_deps,
    })
}

impl WorkspaceInfo {
    pub fn owner_of_file(&self, relative_path: &Path) -> Option<String> {
        let absolute = self.repo_root.join(relative_path);
        let mut best_match: Option<(&str, usize)> = None;

        for (crate_name, package) in &self.packages {
            if absolute.starts_with(&package.manifest_dir) {
                let depth = package.manifest_dir.components().count();
                if best_match
                    .map(|(_, best_depth)| depth > best_depth)
                    .unwrap_or(true)
                {
                    best_match = Some((crate_name.as_str(), depth));
                }
            }
        }

        best_match.map(|(name, _)| name.to_string())
    }

    pub fn dependency_closure(
        &self,
        seed_crates: &BTreeSet<String>,
        kinds: &BTreeSet<DependencyKind>,
    ) -> BTreeSet<String> {
        traverse_graph(&self.forward_deps, seed_crates, kinds)
    }

    pub fn reverse_closure(
        &self,
        seed_crates: &BTreeSet<String>,
        kinds: &BTreeSet<DependencyKind>,
    ) -> BTreeSet<String> {
        traverse_graph(&self.reverse_deps, seed_crates, kinds)
    }

    pub fn package(&self, name: &str) -> Option<&WorkspacePackage> {
        self.packages.get(name)
    }
}

fn parse_dependency_kinds(items: &[MetadataDepKind]) -> Result<BTreeSet<DependencyKind>> {
    let mut kinds = BTreeSet::new();

    if items.is_empty() {
        kinds.insert(DependencyKind::Normal);
        return Ok(kinds);
    }

    for item in items {
        let kind = match item.kind.as_deref() {
            None | Some("normal") => DependencyKind::Normal,
            Some("build") => DependencyKind::Build,
            Some("dev") => DependencyKind::Dev,
            Some(other) => return Err(anyhow!("unsupported cargo dependency kind: {other}")),
        };
        kinds.insert(kind);
    }

    Ok(kinds)
}

fn traverse_graph(
    graph: &DependencyGraph,
    seed_crates: &BTreeSet<String>,
    allowed_kinds: &BTreeSet<DependencyKind>,
) -> BTreeSet<String> {
    let mut visited = seed_crates.clone();
    let mut queue: VecDeque<String> = seed_crates.iter().cloned().collect();

    while let Some(crate_name) = queue.pop_front() {
        let Some(edges) = graph.get(&crate_name) else {
            continue;
        };

        for (next, edge_kinds) in edges {
            if edge_kinds.is_disjoint(allowed_kinds) {
                continue;
            }
            if visited.insert(next.clone()) {
                queue.push_back(next.clone());
            }
        }
    }

    visited
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_workspace() -> WorkspaceInfo {
        let mut packages = BTreeMap::new();
        for name in ["root", "normal-dep", "build-dep", "dev-dep", "consumer"] {
            packages.insert(
                name.to_string(),
                WorkspacePackage {
                    manifest_path: PathBuf::from(format!("/tmp/{name}/Cargo.toml")),
                    manifest_dir: PathBuf::from(format!("/tmp/{name}")),
                    version: Version::parse("0.1.0").expect("version should parse"),
                },
            );
        }

        let mut forward_deps: DependencyGraph = BTreeMap::new();
        forward_deps.entry("root".to_string()).or_default().insert(
            "normal-dep".to_string(),
            BTreeSet::from([DependencyKind::Normal]),
        );
        forward_deps.entry("root".to_string()).or_default().insert(
            "build-dep".to_string(),
            BTreeSet::from([DependencyKind::Build]),
        );
        forward_deps
            .entry("root".to_string())
            .or_default()
            .insert("dev-dep".to_string(), BTreeSet::from([DependencyKind::Dev]));
        forward_deps
            .entry("consumer".to_string())
            .or_default()
            .insert("root".to_string(), BTreeSet::from([DependencyKind::Normal]));

        let mut reverse_deps: DependencyGraph = BTreeMap::new();
        reverse_deps
            .entry("normal-dep".to_string())
            .or_default()
            .insert("root".to_string(), BTreeSet::from([DependencyKind::Normal]));
        reverse_deps
            .entry("build-dep".to_string())
            .or_default()
            .insert("root".to_string(), BTreeSet::from([DependencyKind::Build]));
        reverse_deps
            .entry("dev-dep".to_string())
            .or_default()
            .insert("root".to_string(), BTreeSet::from([DependencyKind::Dev]));
        reverse_deps.entry("root".to_string()).or_default().insert(
            "consumer".to_string(),
            BTreeSet::from([DependencyKind::Normal]),
        );

        WorkspaceInfo {
            repo_root: PathBuf::from("/tmp"),
            packages,
            forward_deps,
            reverse_deps,
        }
    }

    #[test]
    fn dependency_closure_respects_kind_filter() {
        let workspace = sample_workspace();
        let seed = BTreeSet::from(["root".to_string()]);

        let closure = workspace.dependency_closure(
            &seed,
            &BTreeSet::from([DependencyKind::Normal, DependencyKind::Build]),
        );

        assert!(closure.contains("root"));
        assert!(closure.contains("normal-dep"));
        assert!(closure.contains("build-dep"));
        assert!(!closure.contains("dev-dep"));
    }

    #[test]
    fn reverse_closure_respects_kind_filter() {
        let workspace = sample_workspace();
        let seed = BTreeSet::from(["dev-dep".to_string()]);

        let closure = workspace.reverse_closure(&seed, &BTreeSet::from([DependencyKind::Dev]));

        assert!(closure.contains("dev-dep"));
        assert!(closure.contains("root"));
        assert!(!closure.contains("consumer"));
    }
}
