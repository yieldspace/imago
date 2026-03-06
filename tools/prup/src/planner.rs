use crate::config::{CratePolicy, LinePolicy, VersionSource};
use crate::git::CommitInfo;
use crate::resolver::ResolvedPolicy;
use crate::workspace::WorkspaceInfo;
use anyhow::{Result, anyhow};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
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
    pub commits: Vec<CommitInfo>,
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
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePrTarget {
    pub line_id: String,
    pub crate_name: String,
    pub before_version: String,
    pub after_version: String,
    pub bump: BumpLevel,
    pub base_ref: String,
    pub current_tag: String,
    pub next_tag: String,
    pub branch: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

pub fn build_plan_from_line_scopes(
    config: &ResolvedPolicy,
    workspace: &WorkspaceInfo,
    workspace_version: &Version,
    line_scopes: &[LineScopeInput],
) -> Result<ReleasePlan> {
    let default_bump = BumpLevel::from_config(&config.default_bump)?;
    let crate_map = config.crate_map();
    let emit_tag_crates_by_line: BTreeMap<&str, &CratePolicy> = config
        .emit_tag_crates()
        .into_iter()
        .map(|crate_policy| (crate_policy.line.as_str(), crate_policy))
        .collect();

    let mut line_base_refs: BTreeMap<String, String> = BTreeMap::new();
    let mut changed_files_union: BTreeSet<String> = BTreeSet::new();
    let mut changed_crates_union: BTreeSet<String> = BTreeSet::new();
    let mut impacted_crates_union: BTreeSet<String> = BTreeSet::new();

    let mut line_bumps: BTreeMap<String, BumpLevel> = BTreeMap::new();
    let mut line_triggered_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut line_propagated_from: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for line_scope in line_scopes {
        line_base_refs.insert(line_scope.line_id.clone(), line_scope.base_ref.clone());

        let relevant_commits = collect_relevant_commits(
            config,
            workspace,
            &crate_map,
            &line_scope.line_id,
            &line_scope.commits,
        )?;

        if relevant_commits.is_empty() {
            continue;
        }

        let mut changed_crates = BTreeSet::new();
        let mut impacted_crates = BTreeSet::new();
        let mut contributing_lines = BTreeSet::new();

        for commit in &relevant_commits {
            for path in &commit.files {
                changed_files_union.insert(path.to_string_lossy().to_string());
            }

            let commit_changed_crates = changed_crates_for_files(workspace, &commit.files);
            changed_crates.extend(commit_changed_crates.iter().cloned());

            let commit_impacted_crates =
                workspace.reverse_closure(&commit_changed_crates, config.dependency_kinds());
            impacted_crates.extend(commit_impacted_crates.iter().cloned());

            let source_lines = impacted_source_lines(&commit_impacted_crates, &crate_map);
            for source_line in source_lines {
                if can_reach_line(config, &source_line, &line_scope.line_id)? {
                    contributing_lines.insert(source_line);
                }
            }
        }

        changed_crates_union.extend(changed_crates.iter().cloned());
        impacted_crates_union.extend(impacted_crates.iter().cloned());

        let relevant_messages: Vec<String> = relevant_commits.iter().map(commit_message).collect();
        let bump = match detect_bump_from_commits(&relevant_messages) {
            Some(detected) => normalize_bump_for_line_version(
                config,
                workspace,
                workspace_version,
                &emit_tag_crates_by_line,
                &line_scope.line_id,
                detected,
            )?,
            None => default_bump,
        };
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
    line_scopes: &[LineScopeInput],
    github_repo_name_with_owner: Option<&str>,
) -> Result<Vec<CurrentReleaseTarget>> {
    let line_map = config.line_map();
    let crate_map = config.crate_map();
    let mut targets = Vec::new();

    for crate_policy in config.emit_tag_crates() {
        let line_policy = line_map
            .get(crate_policy.line.as_str())
            .ok_or_else(|| anyhow!("unknown line {}", crate_policy.line))?;
        let line_scope = line_scopes
            .iter()
            .find(|scope| scope.line_id == crate_policy.line)
            .ok_or_else(|| anyhow!("missing line scope for {}", crate_policy.line))?;

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

        let relevant_commits = collect_relevant_commits(
            config,
            workspace,
            &crate_map,
            &crate_policy.line,
            &line_scope.commits,
        )?;

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
            body: render_release_body(&relevant_commits, github_repo_name_with_owner),
        });
    }

    targets.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(targets)
}

pub fn build_release_pr_target(
    config: &ResolvedPolicy,
    plan: &ReleasePlan,
    line_id: &str,
) -> Result<ReleasePrTarget> {
    let line_bump = plan
        .line_bumps
        .iter()
        .find(|line| line.line_id == line_id)
        .ok_or_else(|| anyhow!("line {} is not bumped in the provided plan", line_id))?;

    let crate_policy = config
        .emit_tag_crates()
        .into_iter()
        .find(|crate_policy| crate_policy.line == line_id)
        .ok_or_else(|| anyhow!("line {} does not have an emit_tag crate", line_id))?;

    let crate_update = plan
        .crate_updates
        .iter()
        .find(|update| update.crate_name == crate_policy.name)
        .ok_or_else(|| anyhow!("top crate update missing for line {}", line_id))?;

    let line_map = config.line_map();
    let line_policy = line_map
        .get(line_id)
        .ok_or_else(|| anyhow!("missing line policy for {}", line_id))?;

    let tag_template = crate_policy
        .tag_pattern
        .as_deref()
        .or(line_policy.tag_pattern.as_deref())
        .unwrap_or("{{crate}}-v{{version}}");

    let current_tag = render_template(tag_template, &crate_policy.name, &crate_update.before);
    let next_tag = render_template(tag_template, &crate_policy.name, &crate_update.after);
    let title = format!(
        "ci(release): {} {} -> {}",
        crate_policy.name, crate_update.before, crate_update.after
    );

    Ok(ReleasePrTarget {
        line_id: line_id.to_string(),
        crate_name: crate_policy.name.clone(),
        before_version: crate_update.before.clone(),
        after_version: crate_update.after.clone(),
        bump: line_bump.bump,
        base_ref: plan
            .line_base_refs
            .get(line_id)
            .cloned()
            .unwrap_or_else(|| current_tag.clone()),
        current_tag: current_tag.clone(),
        next_tag: next_tag.clone(),
        branch: release_pr_branch_name(line_id),
        title,
        body: render_release_pr_body(
            crate_policy,
            line_bump,
            crate_update,
            plan,
            &current_tag,
            &next_tag,
        ),
        labels: config.github.release_pr.labels.clone(),
    })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ReleaseNoteSection {
    Added,
    Fixed,
    Other,
}

impl ReleaseNoteSection {
    fn heading(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Fixed => "Fixed",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedReleaseCommit {
    section: ReleaseNoteSection,
    scope: Option<String>,
    summary: String,
    breaking: bool,
    pr_number: Option<String>,
}

fn collect_relevant_commits(
    config: &ResolvedPolicy,
    workspace: &WorkspaceInfo,
    crate_map: &BTreeMap<&str, &CratePolicy>,
    line_id: &str,
    commits: &[CommitInfo],
) -> Result<Vec<CommitInfo>> {
    let mut relevant = Vec::new();

    for commit in commits {
        if is_release_bookkeeping_subject(&commit.subject) {
            continue;
        }

        let changed_crates = changed_crates_for_files(workspace, &commit.files);
        if changed_crates.is_empty() {
            continue;
        }

        let impacted_crates = workspace.reverse_closure(&changed_crates, config.dependency_kinds());
        let source_lines = impacted_source_lines(&impacted_crates, crate_map);

        let mut reaches_line = false;
        for source_line in &source_lines {
            if can_reach_line(config, source_line, line_id)? {
                reaches_line = true;
                break;
            }
        }

        if reaches_line {
            relevant.push(commit.clone());
        }
    }

    Ok(relevant)
}

fn changed_crates_for_files(workspace: &WorkspaceInfo, files: &[PathBuf]) -> BTreeSet<String> {
    files
        .iter()
        .filter_map(|path| workspace.owner_of_file(path))
        .collect()
}

fn commit_message(commit: &CommitInfo) -> String {
    if commit.body.is_empty() {
        commit.subject.clone()
    } else {
        format!("{}\n\n{}", commit.subject, commit.body)
    }
}

fn render_release_body(
    commits: &[CommitInfo],
    github_repo_name_with_owner: Option<&str>,
) -> String {
    let mut added = Vec::new();
    let mut fixed = Vec::new();
    let mut other = Vec::new();

    for commit in commits {
        let parsed = parse_release_commit(commit);
        match parsed.section {
            ReleaseNoteSection::Added => added.push(parsed),
            ReleaseNoteSection::Fixed => fixed.push(parsed),
            ReleaseNoteSection::Other => other.push(parsed),
        }
    }

    let mut body = String::new();
    for (section, entries) in [
        (ReleaseNoteSection::Added, added),
        (ReleaseNoteSection::Fixed, fixed),
        (ReleaseNoteSection::Other, other),
    ] {
        if entries.is_empty() {
            continue;
        }

        if !body.is_empty() {
            body.push('\n');
        }

        let _ = writeln!(body, "### {}", section.heading());
        let _ = writeln!(body);

        for entry in entries {
            let _ = writeln!(
                body,
                "{}",
                render_release_note_entry(&entry, github_repo_name_with_owner)
            );
        }
    }

    if body.trim().is_empty() {
        "No notable changes.".to_string()
    } else {
        body.trim_end().to_string()
    }
}

fn render_release_note_entry(
    entry: &ParsedReleaseCommit,
    github_repo_name_with_owner: Option<&str>,
) -> String {
    let mut line = String::from("- ");

    if let Some(scope) = &entry.scope {
        let _ = write!(line, "*({scope})* ");
    }

    if entry.breaking {
        line.push_str("[breaking] ");
    }

    line.push_str(&entry.summary);

    if let Some(pr_number) = &entry.pr_number {
        if let Some(repo) = github_repo_name_with_owner {
            let _ = write!(
                line,
                " ([#{}](https://github.com/{repo}/pull/{}))",
                pr_number, pr_number
            );
        } else {
            let _ = write!(line, " (#{})", pr_number);
        }
    }

    line
}

fn parse_release_commit(commit: &CommitInfo) -> ParsedReleaseCommit {
    let (subject_without_pr, pr_number) = split_subject_pr_number(&commit.subject);

    if let Some(parsed) = parse_conventional_subject(&subject_without_pr) {
        let section = match parsed.kind.as_str() {
            "feat" => ReleaseNoteSection::Added,
            "fix" => ReleaseNoteSection::Fixed,
            _ => ReleaseNoteSection::Other,
        };

        return ParsedReleaseCommit {
            section,
            scope: parsed.scope,
            summary: parsed.summary,
            breaking: parsed.breaking || commit.body.contains("BREAKING CHANGE"),
            pr_number,
        };
    }

    ParsedReleaseCommit {
        section: ReleaseNoteSection::Other,
        scope: None,
        summary: subject_without_pr,
        breaking: commit.body.contains("BREAKING CHANGE"),
        pr_number,
    }
}

#[derive(Debug, Clone)]
struct ParsedConventionalSubject {
    kind: String,
    scope: Option<String>,
    summary: String,
    breaking: bool,
}

fn parse_conventional_subject(subject: &str) -> Option<ParsedConventionalSubject> {
    let (prefix, summary) = subject.split_once(':')?;
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }

    let prefix = prefix.trim();
    if prefix.is_empty() {
        return None;
    }

    let breaking = prefix.contains('!');
    let prefix = prefix.trim_end_matches('!');

    if let Some((kind, scope)) = prefix.split_once('(') {
        let scope = scope.strip_suffix(')')?.trim();
        let kind = kind.trim();
        if kind.is_empty() {
            return None;
        }
        return Some(ParsedConventionalSubject {
            kind: kind.to_string(),
            scope: if scope.is_empty() {
                None
            } else {
                Some(scope.to_string())
            },
            summary: summary.to_string(),
            breaking,
        });
    }

    Some(ParsedConventionalSubject {
        kind: prefix.to_string(),
        scope: None,
        summary: summary.to_string(),
        breaking,
    })
}

fn split_subject_pr_number(subject: &str) -> (String, Option<String>) {
    let subject = subject.trim();
    let Some((prefix, suffix)) = subject.rsplit_once(" (#") else {
        return (subject.to_string(), None);
    };
    let Some(pr_number) = suffix.strip_suffix(')') else {
        return (subject.to_string(), None);
    };
    if pr_number.is_empty() || !pr_number.chars().all(|ch| ch.is_ascii_digit()) {
        return (subject.to_string(), None);
    }
    (prefix.trim().to_string(), Some(pr_number.to_string()))
}

fn is_release_bookkeeping_subject(subject: &str) -> bool {
    let subject = subject.trim();
    if subject.starts_with("ci(release):") {
        return true;
    }

    if subject.starts_with("chore(") || subject.starts_with("chore:") {
        let lower = subject.to_ascii_lowercase();
        return lower.contains(": release ") || lower.ends_with(": release");
    }

    false
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

fn normalize_bump_for_line_version(
    config: &ResolvedPolicy,
    workspace: &WorkspaceInfo,
    workspace_version: &Version,
    emit_tag_crates_by_line: &BTreeMap<&str, &CratePolicy>,
    line_id: &str,
    detected: BumpLevel,
) -> Result<BumpLevel> {
    let current_version = current_line_version(
        workspace,
        workspace_version,
        emit_tag_crates_by_line,
        line_id,
    )?;
    Ok(normalize_bump_for_version(
        detected,
        &current_version,
        &config.pre_1_0_breaking_bump,
    ))
}

fn current_line_version(
    workspace: &WorkspaceInfo,
    workspace_version: &Version,
    emit_tag_crates_by_line: &BTreeMap<&str, &CratePolicy>,
    line_id: &str,
) -> Result<Version> {
    let crate_policy = emit_tag_crates_by_line
        .get(line_id)
        .ok_or_else(|| anyhow!("line {} does not have an emit_tag crate", line_id))?;

    match crate_policy.version_source {
        VersionSource::Workspace => Ok(workspace_version.clone()),
        VersionSource::Package => workspace
            .package(&crate_policy.name)
            .map(|package| package.version.clone())
            .ok_or_else(|| anyhow!("package {} not found", crate_policy.name)),
        VersionSource::None => Err(anyhow!(
            "emit_tag crate {} cannot use version_source=none",
            crate_policy.name
        )),
    }
}

fn normalize_bump_for_version(
    detected: BumpLevel,
    current_version: &Version,
    pre_1_0_breaking_bump: &str,
) -> BumpLevel {
    if detected == BumpLevel::Major
        && current_version.major == 0
        && pre_1_0_breaking_bump == "minor"
    {
        BumpLevel::Minor
    } else {
        detected
    }
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

fn render_release_pr_body(
    crate_policy: &CratePolicy,
    line_bump: &LineBumpPlan,
    crate_update: &CrateVersionUpdate,
    plan: &ReleasePlan,
    current_tag: &str,
    next_tag: &str,
) -> String {
    let mut body = String::new();
    let _ = writeln!(body, "## Release");
    let _ = writeln!(body, "- Line: `{}`", line_bump.line_id);
    let _ = writeln!(body, "- Top crate: `{}`", crate_policy.name);
    let _ = writeln!(body, "- Bump: `{}`", bump_name(line_bump.bump));
    let _ = writeln!(
        body,
        "- Version: `{}` -> `{}`",
        crate_update.before, crate_update.after
    );
    let _ = writeln!(body, "- Tag: `{}` -> `{}`", current_tag, next_tag);

    if !line_bump.triggered_by.is_empty() {
        let _ = writeln!(body, "\n## Triggered By");
        for crate_name in &line_bump.triggered_by {
            let _ = writeln!(body, "- `{crate_name}`");
        }
    }

    if !line_bump.propagated_from.is_empty() {
        let _ = writeln!(body, "\n## Propagated From");
        for line_name in &line_bump.propagated_from {
            let _ = writeln!(body, "- `{line_name}`");
        }
    }

    if !plan.crate_updates.is_empty() {
        let _ = writeln!(body, "\n## Updated Crates");
        for update in &plan.crate_updates {
            let _ = writeln!(
                body,
                "- `{}`: `{}` -> `{}`",
                update.crate_name, update.before, update.after
            );
        }
    }

    body
}

fn release_pr_branch_name(line_id: &str) -> String {
    format!("codex/prup-release-{}", sanitize_branch_component(line_id))
}

fn sanitize_branch_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '-',
        })
        .collect()
}

fn bump_name(bump: BumpLevel) -> &'static str {
    match bump {
        BumpLevel::Patch => "patch",
        BumpLevel::Minor => "minor",
        BumpLevel::Major => "major",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DependencyKind, GithubConfig, LineKind, ReleasePrConfig};
    use crate::resolver::ResolvedPolicy;
    use crate::workspace::{WorkspaceInfo, WorkspacePackage};
    use std::path::PathBuf;

    fn sample_policy() -> ResolvedPolicy {
        ResolvedPolicy {
            base_ref: "origin/main".to_string(),
            default_bump: "patch".to_string(),
            pre_1_0_breaking_bump: "minor".to_string(),
            baseline_tag_required: true,
            allow_dirty: false,
            github_prerelease: true,
            github: GithubConfig {
                release_pr: ReleasePrConfig {
                    labels: vec!["release".to_string()],
                },
            },
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

    fn commit(subject: &str, files: &[&str]) -> CommitInfo {
        CommitInfo {
            sha: format!("sha-{}", subject.replace(' ', "-")),
            subject: subject.to_string(),
            body: String::new(),
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    fn commit_with_body(subject: &str, body: &str, files: &[&str]) -> CommitInfo {
        CommitInfo {
            sha: format!("sha-{}", subject.replace(' ', "-")),
            subject: subject.to_string(),
            body: body.to_string(),
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    fn line_scope(line_id: &str, base_ref: &str, commits: Vec<CommitInfo>) -> LineScopeInput {
        LineScopeInput {
            line_id: line_id.to_string(),
            base_ref: base_ref.to_string(),
            commits,
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
                line_scope(
                    "imago-cli",
                    "imago-v0.2.0",
                    vec![commit(
                        "feat: protocol change",
                        &["imago-protocol/src/lib.rs"],
                    )],
                ),
                line_scope(
                    "imagod-daemon",
                    "imagod-v0.1.0",
                    vec![commit(
                        "feat: protocol change",
                        &["imago-protocol/src/lib.rs"],
                    )],
                ),
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
                line_scope(
                    "imago-cli",
                    "imago-v0.2.0",
                    vec![commit(
                        "fix: adjust cli config",
                        &["imago-project-config/src/lib.rs"],
                    )],
                ),
                line_scope("imagod-daemon", "imagod-v0.1.0", vec![]),
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

        let targets = build_current_release_targets(
            &config,
            &workspace,
            &workspace_version,
            &[
                line_scope("imago-cli", "imago-v0.2.0", vec![]),
                line_scope("imagod-daemon", "imagod-v0.1.0", vec![]),
            ],
            Some("yieldspace/imago"),
        )
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

    #[test]
    fn pre_1_0_minor_policy_normalizes_major_bump() {
        let normalized = normalize_bump_for_version(
            BumpLevel::Major,
            &Version::parse("0.1.1").expect("version should parse"),
            "minor",
        );
        assert_eq!(normalized, BumpLevel::Minor);
    }

    #[test]
    fn pre_1_0_major_policy_keeps_major_bump() {
        let normalized = normalize_bump_for_version(
            BumpLevel::Major,
            &Version::parse("0.1.1").expect("version should parse"),
            "major",
        );
        assert_eq!(normalized, BumpLevel::Major);
    }

    #[test]
    fn release_body_groups_commits_and_links_pull_requests() {
        let body = render_release_body(
            &[
                commit(
                    "feat(cli)!: add deploy mode (#101)",
                    &["imago-project-config/src/lib.rs"],
                ),
                commit(
                    "fix(prup): keep lock in sync (#102)",
                    &["imago-project-config/src/lib.rs"],
                ),
                commit(
                    "docs: document release flow (#103)",
                    &["imago-project-config/src/lib.rs"],
                ),
            ],
            Some("yieldspace/imago"),
        );

        assert!(body.contains("### Added"));
        assert!(body.contains("### Fixed"));
        assert!(body.contains("### Other"));
        assert!(body.contains("*("));
        assert!(body.contains("[breaking] add deploy mode"));
        assert!(body.contains("[#101](https://github.com/yieldspace/imago/pull/101)"));
        assert!(body.contains("keep lock in sync"));
        assert!(body.contains("document release flow"));
    }

    #[test]
    fn release_bookkeeping_commits_do_not_create_bumps_or_notes() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");
        let scopes = vec![
            line_scope(
                "imago-cli",
                "imago-v0.2.0",
                vec![commit(
                    "ci(release): imago-cli 0.1.1 -> 0.2.0 (#293)",
                    &["imago-cli/src/main.rs"],
                )],
            ),
            line_scope("imagod-daemon", "imagod-v0.1.0", vec![]),
        ];

        let plan = build_plan_from_line_scopes(&config, &workspace, &workspace_version, &scopes)
            .expect("plan should build");
        assert!(plan.line_bumps.is_empty());

        let targets = build_current_release_targets(
            &config,
            &workspace,
            &workspace_version,
            &scopes,
            Some("yieldspace/imago"),
        )
        .expect("targets should build");

        let cli_target = targets
            .iter()
            .find(|target| target.crate_name == "imago-cli")
            .expect("cli target should exist");
        assert_eq!(cli_target.body, "No notable changes.");
    }

    #[test]
    fn post_1_0_breaking_stays_major() {
        let normalized = normalize_bump_for_version(
            BumpLevel::Major,
            &Version::parse("1.2.3").expect("version should parse"),
            "minor",
        );
        assert_eq!(normalized, BumpLevel::Major);
    }

    #[test]
    fn breaking_commit_uses_minor_for_pre_1_0_lines_when_configured() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let plan = build_plan_from_line_scopes(
            &config,
            &workspace,
            &workspace_version,
            &[
                line_scope(
                    "imago-cli",
                    "imago-v0.2.0",
                    vec![commit(
                        "feat!: change cli api",
                        &["imago-project-config/src/lib.rs"],
                    )],
                ),
                line_scope(
                    "imagod-daemon",
                    "imagod-v0.1.0",
                    vec![commit_with_body(
                        "fix: daemon api",
                        "BREAKING CHANGE: daemon api",
                        &["imagod-common/src/lib.rs"],
                    )],
                ),
            ],
        )
        .expect("plan should build");

        let line_bumps: BTreeMap<String, BumpLevel> = plan
            .line_bumps
            .iter()
            .map(|item| (item.line_id.clone(), item.bump))
            .collect();
        assert_eq!(line_bumps.get("imago-cli"), Some(&BumpLevel::Minor));
        assert_eq!(line_bumps.get("imagod-daemon"), Some(&BumpLevel::Minor));

        let cli_update = plan
            .package_version_updates
            .iter()
            .find(|update| update.crate_name == "imago-cli")
            .expect("cli update should exist");
        assert_eq!(cli_update.after, "0.3.0");
        assert_eq!(cli_update.bump, BumpLevel::Minor);

        let workspace_update = plan
            .workspace_version_update
            .as_ref()
            .expect("workspace update should exist");
        assert_eq!(workspace_update.after, "0.2.0");
        assert_eq!(workspace_update.bump, BumpLevel::Minor);

        let target =
            build_release_pr_target(&config, &plan, "imagod-daemon").expect("target should build");
        assert_eq!(target.bump, BumpLevel::Minor);
        assert!(target.body.contains("- Bump: `minor`"));
        assert_eq!(target.after_version, "0.2.0");
    }

    #[test]
    fn default_major_policy_preserves_pre_1_0_major_bump() {
        let mut config = sample_policy();
        config.pre_1_0_breaking_bump = "major".to_string();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let plan = build_plan_from_line_scopes(
            &config,
            &workspace,
            &workspace_version,
            &[line_scope(
                "imagod-daemon",
                "imagod-v0.1.0",
                vec![commit(
                    "feat!: change daemon api",
                    &["imagod-common/src/lib.rs"],
                )],
            )],
        )
        .expect("plan should build");

        let workspace_update = plan
            .workspace_version_update
            .as_ref()
            .expect("workspace update should exist");
        assert_eq!(workspace_update.bump, BumpLevel::Major);
        assert_eq!(workspace_update.after, "1.0.0");
    }

    #[test]
    fn release_pr_target_uses_line_specific_metadata() {
        let config = sample_policy();
        let workspace = sample_workspace();
        let workspace_version = Version::parse("0.1.0").expect("workspace version should parse");

        let plan = build_plan_from_line_scopes(
            &config,
            &workspace,
            &workspace_version,
            &[line_scope(
                "imago-cli",
                "imago-v0.2.0",
                vec![commit(
                    "fix: adjust cli config",
                    &["imago-project-config/src/lib.rs"],
                )],
            )],
        )
        .expect("plan should build");

        let target =
            build_release_pr_target(&config, &plan, "imago-cli").expect("target should build");

        assert_eq!(target.branch, "codex/prup-release-imago-cli");
        assert_eq!(target.title, "ci(release): imago-cli 0.2.0 -> 0.2.1");
        assert_eq!(target.current_tag, "imago-v0.2.0");
        assert_eq!(target.next_tag, "imago-v0.2.1");
        assert_eq!(target.labels, vec!["release".to_string()]);
    }
}
