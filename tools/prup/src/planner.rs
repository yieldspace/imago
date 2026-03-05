use crate::config::{CratePolicy, LinePolicy, VersionSource};
use crate::resolver::ResolvedPolicy;
use crate::workspace::WorkspaceInfo;
use anyhow::{Result, anyhow};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BumpLevel {
    Patch,
    Minor,
    Major,
}

impl BumpLevel {
    pub fn from_config(value: &str) -> Result<Self> {
        match value {
            "patch" => Ok(Self::Patch),
            "minor" => Ok(Self::Minor),
            "major" => Ok(Self::Major),
            _ => Err(anyhow!("unsupported bump level: {value}")),
        }
    }

    pub fn apply(self, version: &Version) -> Version {
        let mut next = version.clone();
        match self {
            Self::Patch => {
                next.patch += 1;
                next.pre = semver::Prerelease::EMPTY;
                next.build = semver::BuildMetadata::EMPTY;
            }
            Self::Minor => {
                next.minor += 1;
                next.patch = 0;
                next.pre = semver::Prerelease::EMPTY;
                next.build = semver::BuildMetadata::EMPTY;
            }
            Self::Major => {
                next.major += 1;
                next.minor = 0;
                next.patch = 0;
                next.pre = semver::Prerelease::EMPTY;
                next.build = semver::BuildMetadata::EMPTY;
            }
        }
        next
    }
}

#[derive(Debug, Clone)]
pub struct LineScopeInput {
    pub line_id: String,
    pub base_ref: String,
    pub changed_files: Vec<PathBuf>,
    pub commit_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePlan {
    pub line_base_refs: BTreeMap<String, String>,
    pub changed_files: Vec<String>,
    pub changed_crates: Vec<String>,
    pub impacted_crates: Vec<String>,
    pub line_bumps: Vec<LineBumpPlan>,
    pub workspace_version_update: Option<VersionUpdate>,
    pub package_version_updates: Vec<PackageVersionUpdate>,
    pub crate_updates: Vec<CrateVersionUpdate>,
    pub tags: Vec<TagPlan>,
    pub release_targets: Vec<ReleaseTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineBumpPlan {
    pub line_id: String,
    pub bump: BumpLevel,
    pub triggered_by: Vec<String>,
    pub propagated_from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionUpdate {
    pub before: String,
    pub after: String,
    pub bump: BumpLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersionUpdate {
    pub crate_name: String,
    pub before: String,
    pub after: String,
    pub bump: BumpLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateVersionUpdate {
    pub crate_name: String,
    pub line_id: String,
    pub before: String,
    pub after: String,
    pub version_source: VersionSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagPlan {
    pub crate_name: String,
    pub tag: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseTarget {
    pub crate_name: String,
    pub tag: String,
    pub release_name: String,
    pub github_release: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentReleaseTarget {
    pub crate_name: String,
    pub version: String,
    pub tag: String,
    pub github_release: bool,
    pub release_name: String,
}

pub fn build_plan_from_line_scopes(
    config: &ResolvedPolicy,
    workspace: &WorkspaceInfo,
    workspace_version: &Version,
    line_scopes: &[LineScopeInput],
) -> Result<ReleasePlan> {
    let default_bump = BumpLevel::from_config(&config.default_bump)?;
    let crate_map = config.crate_map();

    let mut line_base_refs: BTreeMap<String, String> = BTreeMap::new();
    let mut changed_files_union: BTreeSet<String> = BTreeSet::new();
    let mut changed_crates_union: BTreeSet<String> = BTreeSet::new();
    let mut impacted_crates_union: BTreeSet<String> = BTreeSet::new();

    let mut line_bumps: BTreeMap<String, BumpLevel> = BTreeMap::new();
    let mut line_triggered_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut line_propagated_from: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for line_scope in line_scopes {
        line_base_refs.insert(line_scope.line_id.clone(), line_scope.base_ref.clone());

        let mut changed_crates: BTreeSet<String> = BTreeSet::new();
        for path in &line_scope.changed_files {
            changed_files_union.insert(path.to_string_lossy().to_string());
            if let Some(owner) = workspace.owner_of_file(path) {
                changed_crates.insert(owner);
            }
        }

        changed_crates_union.extend(changed_crates.iter().cloned());

        let impacted_crates = workspace.reverse_closure(&changed_crates, config.dependency_kinds());
        impacted_crates_union.extend(impacted_crates.iter().cloned());

        let source_lines = impacted_source_lines(&impacted_crates, &crate_map);
        let mut contributing_lines = BTreeSet::new();

        for source_line in source_lines {
            if can_reach_line(config, &source_line, &line_scope.line_id)? {
                contributing_lines.insert(source_line);
            }
        }

        if contributing_lines.is_empty() {
            continue;
        }

        let bump = detect_bump_from_commits(&line_scope.commit_messages).unwrap_or(default_bump);
        line_bumps
            .entry(line_scope.line_id.clone())
            .and_modify(|current| *current = (*current).max(bump))
            .or_insert(bump);

        for impacted_crate in &impacted_crates {
            let Some(crate_policy) = crate_map.get(impacted_crate.as_str()) else {
                continue;
            };
            if contributing_lines.contains(&crate_policy.line) {
                line_triggered_by
                    .entry(line_scope.line_id.clone())
                    .or_default()
                    .insert(impacted_crate.clone());
            }
        }

        for source_line in contributing_lines {
            if source_line != line_scope.line_id {
                line_propagated_from
                    .entry(line_scope.line_id.clone())
                    .or_default()
                    .insert(source_line);
            }
        }
    }

    let mut line_bump_plans = Vec::new();
    for (line_id, bump) in &line_bumps {
        line_bump_plans.push(LineBumpPlan {
            line_id: line_id.clone(),
            bump: *bump,
            triggered_by: line_triggered_by
                .get(line_id)
                .map(|items| items.iter().cloned().collect())
                .unwrap_or_default(),
            propagated_from: line_propagated_from
                .get(line_id)
                .map(|items| items.iter().cloned().collect())
                .unwrap_or_default(),
        });
    }

    let workspace_bump = if line_bumps.iter().any(|(line_id, _)| {
        config
            .crates_for_line(line_id)
            .iter()
            .any(|crate_policy| crate_policy.version_source == VersionSource::Workspace)
    }) {
        Some(
            line_bumps
                .values()
                .copied()
                .max()
                .ok_or_else(|| anyhow!("line bump calculation failed"))?,
        )
    } else {
        None
    };

    let workspace_version_update = workspace_bump.map(|bump| VersionUpdate {
        before: workspace_version.to_string(),
        after: bump.apply(workspace_version).to_string(),
        bump,
    });

    let mut package_version_updates = Vec::new();
    let mut package_version_map: BTreeMap<String, Version> = BTreeMap::new();

    for crate_policy in &config.crates {
        if crate_policy.version_source != VersionSource::Package {
            continue;
        }

        let Some(line_bump) = line_bumps.get(&crate_policy.line) else {
            continue;
        };

        let package = workspace
            .package(&crate_policy.name)
            .ok_or_else(|| anyhow!("package {} not found", crate_policy.name))?;
        let before = package.version.clone();
        let after = line_bump.apply(&before);
        package_version_map.insert(crate_policy.name.clone(), after.clone());

        package_version_updates.push(PackageVersionUpdate {
            crate_name: crate_policy.name.clone(),
            before: before.to_string(),
            after: after.to_string(),
            bump: *line_bump,
        });
    }

    let workspace_after_version = workspace_version_update
        .as_ref()
        .map(|update| Version::parse(&update.after))
        .transpose()?;

    let mut crate_updates = Vec::new();
    for crate_policy in &config.crates {
        match crate_policy.version_source {
            VersionSource::None => {}
            VersionSource::Workspace => {
                let Some(after_workspace) = &workspace_after_version else {
                    continue;
                };
                crate_updates.push(CrateVersionUpdate {
                    crate_name: crate_policy.name.clone(),
                    line_id: crate_policy.line.clone(),
                    before: workspace_version.to_string(),
                    after: after_workspace.to_string(),
                    version_source: VersionSource::Workspace,
                });
            }
            VersionSource::Package => {
                let Some(after) = package_version_map.get(&crate_policy.name) else {
                    continue;
                };
                let package = workspace
                    .package(&crate_policy.name)
                    .ok_or_else(|| anyhow!("package {} not found", crate_policy.name))?;
                crate_updates.push(CrateVersionUpdate {
                    crate_name: crate_policy.name.clone(),
                    line_id: crate_policy.line.clone(),
                    before: package.version.to_string(),
                    after: after.to_string(),
                    version_source: VersionSource::Package,
                });
            }
        }
    }

    crate_updates.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    package_version_updates.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

    let line_map = config.line_map();
    let crate_policy_map = config.crate_map();

    let mut tags = Vec::new();
    let mut release_targets = Vec::new();
    let mut seen_tags = BTreeSet::new();

    for crate_update in &crate_updates {
        let crate_policy = crate_policy_map
            .get(crate_update.crate_name.as_str())
            .ok_or_else(|| anyhow!("missing crate policy: {}", crate_update.crate_name))?;

        if !crate_policy.emit_tag {
            continue;
        }

        let line_policy = line_map
            .get(crate_update.line_id.as_str())
            .ok_or_else(|| anyhow!("missing line policy: {}", crate_update.line_id))?;

        let tag_template = crate_policy
            .tag_pattern
            .as_deref()
            .or(line_policy.tag_pattern.as_deref())
            .unwrap_or("{{crate}}-v{{version}}");
        let tag = render_template(tag_template, &crate_update.crate_name, &crate_update.after);

        if seen_tags.insert(tag.clone()) {
            tags.push(TagPlan {
                crate_name: crate_update.crate_name.clone(),
                tag: tag.clone(),
                version: crate_update.after.clone(),
            });
        }

        let create_release = crate_policy
            .github_release
            .or(line_policy.github_release)
            .unwrap_or(false);
        let release_name_template = crate_policy
            .github_release_name
            .as_deref()
            .or(line_policy.github_release_name.as_deref())
            .unwrap_or(tag_template);

        release_targets.push(ReleaseTarget {
            crate_name: crate_update.crate_name.clone(),
            tag,
            release_name: render_template(
                release_name_template,
                &crate_update.crate_name,
                &crate_update.after,
            ),
            github_release: create_release,
        });
    }

    tags.sort_by(|a, b| a.tag.cmp(&b.tag));
    release_targets.sort_by(|a, b| a.tag.cmp(&b.tag));

    Ok(ReleasePlan {
        line_base_refs,
        changed_files: changed_files_union.into_iter().collect(),
        changed_crates: changed_crates_union.into_iter().collect(),
        impacted_crates: impacted_crates_union.into_iter().collect(),
        line_bumps: line_bump_plans,
        workspace_version_update,
        package_version_updates,
        crate_updates,
        tags,
        release_targets,
    })
}

pub fn build_current_release_targets(
    config: &ResolvedPolicy,
    workspace: &WorkspaceInfo,
    workspace_version: &Version,
) -> Result<Vec<CurrentReleaseTarget>> {
    let line_map = config.line_map();
    let mut targets = Vec::new();

    for crate_policy in config.emit_tag_crates() {
        let line_policy = line_map
            .get(crate_policy.line.as_str())
            .ok_or_else(|| anyhow!("unknown line {}", crate_policy.line))?;

        let version = match crate_policy.version_source {
            VersionSource::Workspace => workspace_version.clone(),
            VersionSource::Package => workspace
                .package(&crate_policy.name)
                .ok_or_else(|| anyhow!("package {} not found", crate_policy.name))?
                .version
                .clone(),
            VersionSource::None => {
                return Err(anyhow!(
                    "emit_tag crate {} cannot use version_source=none",
                    crate_policy.name
                ));
            }
        };

        let tag_template = crate_policy
            .tag_pattern
            .as_deref()
            .or(line_policy.tag_pattern.as_deref())
            .unwrap_or("{{crate}}-v{{version}}");
        let tag = render_template(tag_template, &crate_policy.name, &version.to_string());

        let create_release = crate_policy
            .github_release
            .or(line_policy.github_release)
            .unwrap_or(false);

        let release_name_template = crate_policy
            .github_release_name
            .as_deref()
            .or(line_policy.github_release_name.as_deref())
            .unwrap_or(tag_template);

        targets.push(CurrentReleaseTarget {
            crate_name: crate_policy.name.clone(),
            version: version.to_string(),
            tag,
            github_release: create_release,
            release_name: render_template(
                release_name_template,
                &crate_policy.name,
                &version.to_string(),
            ),
        });
    }

    targets.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(targets)
}

pub fn baseline_tag_glob(crate_policy: &CratePolicy, line_policy: &LinePolicy) -> String {
    let template = crate_policy
        .tag_pattern
        .as_deref()
        .or(line_policy.tag_pattern.as_deref())
        .unwrap_or("{{crate}}-v{{version}}");

    let with_crate = template.replace("{{crate}}", &crate_policy.name);
    if with_crate.contains("{{version}}") {
        with_crate.replace("{{version}}", "*")
    } else {
        format!("{with_crate}*")
    }
}

pub fn detect_bump_from_commits(messages: &[String]) -> Option<BumpLevel> {
    let mut detected: Option<BumpLevel> = None;

    for message in messages {
        let first_line = message.lines().next().unwrap_or_default().trim();

        if message.contains("BREAKING CHANGE") || is_breaking_subject(first_line) {
            detected = Some(BumpLevel::Major);
            continue;
        }

        if is_feat_subject(first_line) {
            detected =
                Some(detected.map_or(BumpLevel::Minor, |current| current.max(BumpLevel::Minor)));
            continue;
        }

        if is_fix_subject(first_line) {
            detected =
                Some(detected.map_or(BumpLevel::Patch, |current| current.max(BumpLevel::Patch)));
        }
    }

    detected
}

fn impacted_source_lines(
    impacted_crates: &BTreeSet<String>,
    crate_map: &BTreeMap<&str, &CratePolicy>,
) -> BTreeSet<String> {
    let mut lines = BTreeSet::new();
    for crate_name in impacted_crates {
        if let Some(crate_policy) = crate_map.get(crate_name.as_str()) {
            lines.insert(crate_policy.line.clone());
        }
    }
    lines
}

fn can_reach_line(config: &ResolvedPolicy, source_line: &str, target_line: &str) -> Result<bool> {
    if source_line == target_line {
        return Ok(true);
    }

    let line_map = config.line_map();
    let mut visited = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::from([source_line.to_string()]);

    while let Some(current) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }

        let line_policy = line_map
            .get(current.as_str())
            .ok_or_else(|| anyhow!("unknown line {current}"))?;

        for next in &line_policy.propagate_to {
            if next == target_line {
                return Ok(true);
            }
            queue.push_back(next.clone());
        }
    }

    Ok(false)
}

fn is_breaking_subject(subject: &str) -> bool {
    let Some(prefix) = subject.split(':').next() else {
        return false;
    };
    prefix.contains('!')
}

fn is_feat_subject(subject: &str) -> bool {
    subject.starts_with("feat:") || subject.starts_with("feat(")
}

fn is_fix_subject(subject: &str) -> bool {
    subject.starts_with("fix:") || subject.starts_with("fix(")
}

fn render_template(template: &str, crate_name: &str, version: &str) -> String {
    template
        .replace("{{crate}}", crate_name)
        .replace("{{version}}", version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DependencyKind, LineKind};
    use crate::resolver::ResolvedPolicy;
    use crate::workspace::{WorkspaceInfo, WorkspacePackage};
    use std::path::PathBuf;

    fn sample_policy() -> ResolvedPolicy {
        ResolvedPolicy {
            base_ref: "origin/main".to_string(),
            default_bump: "patch".to_string(),
            baseline_tag_required: true,
            allow_dirty: false,
            github_prerelease: true,
            dependency_kinds: BTreeSet::from([
                DependencyKind::Normal,
                DependencyKind::Build,
                DependencyKind::Dev,
            ]),
            lines: vec![
                LinePolicy {
                    id: "imago-cli".to_string(),
                    kind: LineKind::PublicRelease,
                    propagate_to: vec![],
                    tag_pattern: None,
                    github_release: Some(false),
                    github_release_name: None,
                },
                LinePolicy {
                    id: "imagod-daemon".to_string(),
                    kind: LineKind::PublicRelease,
                    propagate_to: vec![],
                    tag_pattern: None,
                    github_release: Some(false),
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
                    github_release_name: None,
                    changelog_update: Some(true),
                },
                CratePolicy {
                    name: "imagod".to_string(),
                    line: "imagod-daemon".to_string(),
                    version_source: VersionSource::Workspace,
                    emit_tag: true,
                    tag_pattern: Some("imagod-v{{version}}".to_string()),
                    github_release: Some(true),
                    github_release_name: None,
                    changelog_update: Some(true),
                },
                CratePolicy {
                    name: "imago-protocol".to_string(),
                    line: "imago-shared".to_string(),
                    version_source: VersionSource::Workspace,
                    emit_tag: false,
                    tag_pattern: None,
                    github_release: Some(false),
                    github_release_name: None,
                    changelog_update: Some(false),
                },
                CratePolicy {
                    name: "imagod-common".to_string(),
                    line: "imagod-daemon".to_string(),
                    version_source: VersionSource::Workspace,
                    emit_tag: false,
                    tag_pattern: None,
                    github_release: Some(false),
                    github_release_name: None,
                    changelog_update: Some(false),
                },
                CratePolicy {
                    name: "imago-project-config".to_string(),
                    line: "imago-cli".to_string(),
                    version_source: VersionSource::None,
                    emit_tag: false,
                    tag_pattern: None,
                    github_release: Some(false),
                    github_release_name: None,
                    changelog_update: Some(false),
                },
            ],
        }
    }

    fn sample_workspace() -> WorkspaceInfo {
        let mut packages = BTreeMap::new();
        for (name, version) in [
            ("imago-cli", "0.2.0"),
            ("imagod", "0.1.0"),
            ("imago-protocol", "0.1.0"),
            ("imagod-common", "0.1.0"),
            ("imago-project-config", "0.1.0"),
            ("out-of-scope", "0.1.0"),
        ] {
            packages.insert(
                name.to_string(),
                WorkspacePackage {
                    manifest_path: PathBuf::from(format!("/tmp/{name}/Cargo.toml")),
                    manifest_dir: PathBuf::from(format!("/tmp/{name}")),
                    version: Version::parse(version).expect("version should parse"),
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
            .entry("imagod-common".to_string())
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

        WorkspaceInfo {
            repo_root: PathBuf::from("/tmp"),
            packages,
            forward_deps,
            reverse_deps,
        }
    }

    #[test]
    fn shared_change_propagates_to_cli_and_daemon() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let plan = build_plan_from_line_scopes(
            &config,
            &workspace,
            &workspace_version,
            &[
                LineScopeInput {
                    line_id: "imago-cli".to_string(),
                    base_ref: "imago-v0.2.0".to_string(),
                    changed_files: vec![PathBuf::from("imago-protocol/src/lib.rs")],
                    commit_messages: vec!["feat: protocol change".to_string()],
                },
                LineScopeInput {
                    line_id: "imagod-daemon".to_string(),
                    base_ref: "imagod-v0.1.0".to_string(),
                    changed_files: vec![PathBuf::from("imago-protocol/src/lib.rs")],
                    commit_messages: vec!["feat: protocol change".to_string()],
                },
            ],
        )
        .expect("plan should build");

        let bumped_lines: BTreeSet<String> = plan
            .line_bumps
            .into_iter()
            .map(|item| item.line_id)
            .collect();
        assert!(bumped_lines.contains("imago-cli"));
        assert!(bumped_lines.contains("imagod-daemon"));
        assert_eq!(
            plan.workspace_version_update
                .expect("workspace bump should exist")
                .after,
            "0.2.0"
        );
    }

    #[test]
    fn cli_only_change_does_not_bump_daemon_or_workspace_version() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let plan = build_plan_from_line_scopes(
            &config,
            &workspace,
            &workspace_version,
            &[
                LineScopeInput {
                    line_id: "imago-cli".to_string(),
                    base_ref: "imago-v0.2.0".to_string(),
                    changed_files: vec![PathBuf::from("imago-project-config/src/lib.rs")],
                    commit_messages: vec!["fix: adjust cli config".to_string()],
                },
                LineScopeInput {
                    line_id: "imagod-daemon".to_string(),
                    base_ref: "imagod-v0.1.0".to_string(),
                    changed_files: vec![],
                    commit_messages: vec![],
                },
            ],
        )
        .expect("plan should build");

        let bumped_lines: BTreeSet<String> = plan
            .line_bumps
            .iter()
            .map(|item| item.line_id.clone())
            .collect();
        assert!(bumped_lines.contains("imago-cli"));
        assert!(!bumped_lines.contains("imagod-daemon"));
        assert!(plan.workspace_version_update.is_none());

        let updated_crates: BTreeSet<String> = plan
            .crate_updates
            .into_iter()
            .map(|item| item.crate_name)
            .collect();
        assert!(updated_crates.contains("imago-cli"));
        assert!(!updated_crates.contains("imagod"));
        assert!(!updated_crates.contains("imago-project-config"));
    }

    #[test]
    fn release_targets_only_include_emit_tag_crates() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let targets = build_current_release_targets(&config, &workspace, &workspace_version)
            .expect("targets should build");

        let names: BTreeSet<String> = targets
            .into_iter()
            .map(|target| target.crate_name)
            .collect();
        assert!(names.contains("imago-cli"));
        assert!(names.contains("imagod"));
        assert!(!names.contains("imago-protocol"));
        assert!(!names.contains("imagod-common"));
    }

    #[test]
    fn breaking_commit_promotes_major() {
        let detected = detect_bump_from_commits(&[
            "fix: minor fix".to_string(),
            "feat!: change api".to_string(),
        ]);
        assert_eq!(detected, Some(BumpLevel::Major));
    }
}
