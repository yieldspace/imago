use crate::config::{
    CratePolicy, DependencyKind, GithubConfig, LinePolicy, PrupConfig, VersionSource,
};
use crate::workspace::WorkspaceInfo;
use anyhow::{Result, anyhow};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone)]
pub struct ResolvedPolicy {
    pub base_ref: String,
    pub default_bump: String,
    pub baseline_tag_required: bool,
    pub allow_dirty: bool,
    pub github_prerelease: bool,
    pub github: GithubConfig,
    pub dependency_kinds: BTreeSet<DependencyKind>,
    pub lines: Vec<LinePolicy>,
    pub crates: Vec<CratePolicy>,
}

pub fn resolve(config: &PrupConfig, workspace: &WorkspaceInfo) -> Result<ResolvedPolicy> {
    let dependency_kinds = config.dependency_kind_set();
    let line_top_map = top_crates_by_line(config)?;
    let mut resolved_crates = Vec::new();
    let mut crate_to_lines: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for top_crate in &config.crates {
        if workspace.package(&top_crate.name).is_none() {
            return Err(anyhow!(
                "crate {} is configured in [workspace.metadata.prup] but not found in workspace",
                top_crate.name
            ));
        }

        let closure = workspace
            .dependency_closure(&BTreeSet::from([top_crate.name.clone()]), &dependency_kinds);
        for crate_name in closure {
            if workspace.package(&crate_name).is_none() {
                continue;
            }
            crate_to_lines
                .entry(crate_name)
                .or_default()
                .insert(top_crate.line.clone());
        }
    }

    let configured_map: BTreeMap<&str, &CratePolicy> = config
        .crates
        .iter()
        .map(|crate_policy| (crate_policy.name.as_str(), crate_policy))
        .collect();

    for (crate_name, lines) in crate_to_lines {
        if let Some(configured) = configured_map.get(crate_name.as_str()) {
            resolved_crates.push((*configured).clone());
            continue;
        }

        let line = resolve_line_assignment(&crate_name, &lines, config)?;
        let version_source = default_version_source_for_line(&line, config, &line_top_map)?;

        resolved_crates.push(CratePolicy {
            name: crate_name,
            line,
            version_source,
            emit_tag: false,
            tag_pattern: None,
            github_release: Some(false),
            github_release_name: None,
            changelog_update: Some(false),
        });
    }

    resolved_crates.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(ResolvedPolicy {
        base_ref: config.base_ref.clone(),
        default_bump: config.default_bump.clone(),
        baseline_tag_required: config.baseline_tag_required,
        allow_dirty: config.allow_dirty,
        github_prerelease: config.github_prerelease,
        github: config.github.clone(),
        dependency_kinds,
        lines: config.lines.clone(),
        crates: resolved_crates,
    })
}

impl ResolvedPolicy {
    pub fn line_map(&self) -> BTreeMap<&str, &LinePolicy> {
        let mut map = BTreeMap::new();
        for line in &self.lines {
            map.insert(line.id.as_str(), line);
        }
        map
    }

    pub fn crate_map(&self) -> BTreeMap<&str, &CratePolicy> {
        let mut map = BTreeMap::new();
        for crate_policy in &self.crates {
            map.insert(crate_policy.name.as_str(), crate_policy);
        }
        map
    }

    pub fn crates_for_line<'a>(&'a self, line_id: &str) -> Vec<&'a CratePolicy> {
        self.crates
            .iter()
            .filter(|crate_policy| crate_policy.line == line_id)
            .collect()
    }

    pub fn emit_tag_crates(&self) -> Vec<&CratePolicy> {
        self.crates
            .iter()
            .filter(|crate_policy| crate_policy.emit_tag)
            .collect()
    }

    pub fn dependency_kinds(&self) -> &BTreeSet<DependencyKind> {
        &self.dependency_kinds
    }
}

fn resolve_line_assignment(
    crate_name: &str,
    lines: &BTreeSet<String>,
    config: &PrupConfig,
) -> Result<String> {
    if lines.len() == 1 {
        return lines
            .iter()
            .next()
            .cloned()
            .ok_or_else(|| anyhow!("line assignment is unexpectedly empty for {crate_name}"));
    }

    match &config.shared_line {
        Some(shared_line) => Ok(shared_line.clone()),
        None => Err(anyhow!(
            "crate {crate_name} reaches multiple lines ({}) but shared_line is not configured",
            lines.iter().cloned().collect::<Vec<_>>().join(", ")
        )),
    }
}

fn default_version_source_for_line(
    line_id: &str,
    config: &PrupConfig,
    line_top_map: &BTreeMap<&str, &CratePolicy>,
) -> Result<VersionSource> {
    if let Some(top_crate) = line_top_map.get(line_id) {
        return Ok(match top_crate.version_source {
            VersionSource::Workspace => VersionSource::Workspace,
            VersionSource::Package | VersionSource::None => VersionSource::None,
        });
    }

    let line_map = config.line_map();
    let mut visited = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::from([line_id.to_string()]);

    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }

        if let Some(top_crate) = line_top_map.get(current.as_str()) {
            if top_crate.version_source == VersionSource::Workspace {
                return Ok(VersionSource::Workspace);
            }
            continue;
        }

        let line_policy = line_map
            .get(current.as_str())
            .ok_or_else(|| anyhow!("unknown line {current}"))?;
        for next in &line_policy.propagate_to {
            queue.push_back(next.clone());
        }
    }

    Ok(VersionSource::None)
}

fn top_crates_by_line(config: &PrupConfig) -> Result<BTreeMap<&str, &CratePolicy>> {
    let mut map = BTreeMap::new();
    for crate_policy in &config.crates {
        if !crate_policy.emit_tag {
            continue;
        }
        if map
            .insert(crate_policy.line.as_str(), crate_policy)
            .is_some()
        {
            return Err(anyhow!(
                "line {} has multiple emit_tag crates; exactly one is allowed",
                crate_policy.line
            ));
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DependencyKind, LineKind, PrupConfig};
    use crate::workspace::{WorkspaceInfo, WorkspacePackage};
    use semver::Version;
    use std::path::PathBuf;

    fn sample_config() -> PrupConfig {
        PrupConfig {
            base_ref: "origin/main".to_string(),
            bump_strategy: "conventional_commits".to_string(),
            default_bump: "patch".to_string(),
            baseline_tag_required: true,
            allow_dirty: false,
            github_prerelease: true,
            github: GithubConfig {
                release_pr: ReleasePrConfig {
                    labels: vec!["release".to_string()],
                },
            },
            dependency_kinds: vec![
                DependencyKind::Normal,
                DependencyKind::Build,
                DependencyKind::Dev,
            ],
            shared_line: Some("imago-shared".to_string()),
            lines: vec![
                LinePolicy {
                    id: "imago-cli".to_string(),
                    kind: LineKind::PublicRelease,
                    propagate_to: vec![],
                    tag_pattern: None,
                    github_release: Some(true),
                    github_release_name: None,
                },
                LinePolicy {
                    id: "imagod-daemon".to_string(),
                    kind: LineKind::PublicRelease,
                    propagate_to: vec![],
                    tag_pattern: None,
                    github_release: Some(true),
                    github_release_name: None,
                },
                LinePolicy {
                    id: "imago-shared".to_string(),
                    kind: LineKind::TagOnly,
                    propagate_to: vec!["imago-cli".to_string(), "imagod-daemon".to_string()],
                    tag_pattern: None,
                    github_release: Some(false),
                    github_release_name: None,
                },
            ],
            crates: vec![
                CratePolicy {
                    name: "imago-cli".to_string(),
                    line: "imago-cli".to_string(),
                    version_source: VersionSource::Package,
                    emit_tag: true,
                    tag_pattern: Some("imago-v{{version}}".to_string()),
                    github_release: Some(true),
                    github_release_name: Some("imago-v{{version}}".to_string()),
                    changelog_update: Some(true),
                },
                CratePolicy {
                    name: "imagod".to_string(),
                    line: "imagod-daemon".to_string(),
                    version_source: VersionSource::Workspace,
                    emit_tag: true,
                    tag_pattern: Some("imagod-v{{version}}".to_string()),
                    github_release: Some(true),
                    github_release_name: Some("imagod-v{{version}}".to_string()),
                    changelog_update: Some(true),
                },
            ],
        }
    }

    fn sample_workspace() -> WorkspaceInfo {
        let mut packages = BTreeMap::new();
        for name in [
            "imago-cli",
            "imagod",
            "imago-protocol",
            "imago-project-config",
            "imagod-common",
        ] {
            packages.insert(
                name.to_string(),
                WorkspacePackage {
                    manifest_path: PathBuf::from(format!("/tmp/{name}/Cargo.toml")),
                    manifest_dir: PathBuf::from(format!("/tmp/{name}")),
                    version: Version::parse("0.1.0").expect("version should parse"),
                },
            );
        }

        let mut forward_deps: BTreeMap<String, BTreeMap<String, BTreeSet<DependencyKind>>> =
            BTreeMap::new();
        forward_deps
            .entry("imago-cli".to_string())
            .or_default()
            .insert(
                "imago-protocol".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        forward_deps
            .entry("imago-cli".to_string())
            .or_default()
            .insert(
                "imago-project-config".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        forward_deps
            .entry("imagod".to_string())
            .or_default()
            .insert(
                "imago-protocol".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        forward_deps
            .entry("imagod".to_string())
            .or_default()
            .insert(
                "imagod-common".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );

        let mut reverse_deps: BTreeMap<String, BTreeMap<String, BTreeSet<DependencyKind>>> =
            BTreeMap::new();
        reverse_deps
            .entry("imago-protocol".to_string())
            .or_default()
            .insert(
                "imago-cli".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        reverse_deps
            .entry("imago-protocol".to_string())
            .or_default()
            .insert(
                "imagod".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        reverse_deps
            .entry("imago-project-config".to_string())
            .or_default()
            .insert(
                "imago-cli".to_string(),
                BTreeSet::from([DependencyKind::Normal]),
            );
        reverse_deps
            .entry("imagod-common".to_string())
            .or_default()
            .insert(
                "imagod".to_string(),
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
    fn overlap_crate_is_assigned_to_shared_line() {
        let resolved = resolve(&sample_config(), &sample_workspace()).expect("resolve should work");
        let crate_map = resolved.crate_map();

        assert_eq!(
            crate_map
                .get("imago-protocol")
                .expect("shared crate should exist")
                .line,
            "imago-shared"
        );
        assert_eq!(
            crate_map
                .get("imago-project-config")
                .expect("cli crate should exist")
                .version_source,
            VersionSource::None
        );
        assert_eq!(
            crate_map
                .get("imagod-common")
                .expect("daemon crate should exist")
                .version_source,
            VersionSource::Workspace
        );
    }

    #[test]
    fn overlap_without_shared_line_fails_closed() {
        let mut config = sample_config();
        config.shared_line = None;

        let error = resolve(&config, &sample_workspace()).expect_err("overlap should fail");
        assert!(error.to_string().contains("shared_line is not configured"));
    }
}
