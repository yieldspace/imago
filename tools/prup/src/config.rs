use anyhow::{Context, Result, anyhow};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub workspace_version: Version,
    pub config: PrupConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct RootCargoToml {
    workspace: RootWorkspace,
}

#[derive(Debug, Clone, Deserialize)]
struct RootWorkspace {
    package: WorkspacePackage,
    #[serde(default)]
    metadata: WorkspaceMetadata,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkspacePackage {
    version: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WorkspaceMetadata {
    #[serde(default)]
    prup: Option<PrupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrupConfig {
    #[serde(default = "default_base_ref")]
    pub base_ref: String,
    #[serde(default = "default_bump_strategy")]
    pub bump_strategy: String,
    #[serde(default = "default_bump")]
    pub default_bump: String,
    #[serde(default = "default_true")]
    pub baseline_tag_required: bool,
    #[serde(default)]
    pub allow_dirty: bool,
    #[serde(default = "default_true")]
    pub github_prerelease: bool,
    #[serde(default = "default_dependency_kinds")]
    pub dependency_kinds: Vec<DependencyKind>,
    #[serde(default)]
    pub shared_line: Option<String>,
    #[serde(default)]
    pub github: GithubConfig,
    #[serde(default)]
    pub lines: Vec<LinePolicy>,
    #[serde(default)]
    pub crates: Vec<CratePolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GithubConfig {
    #[serde(default)]
    pub release_pr: ReleasePrConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleasePrConfig {
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinePolicy {
    pub id: String,
    #[serde(default)]
    pub kind: LineKind,
    #[serde(default)]
    pub propagate_to: Vec<String>,
    #[serde(default)]
    pub tag_pattern: Option<String>,
    #[serde(default)]
    pub github_release: Option<bool>,
    #[serde(default)]
    pub github_release_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LineKind {
    PublicRelease,
    #[default]
    TagOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CratePolicy {
    pub name: String,
    pub line: String,
    #[serde(default)]
    pub version_source: VersionSource,
    #[serde(default)]
    pub emit_tag: bool,
    #[serde(default)]
    pub tag_pattern: Option<String>,
    #[serde(default)]
    pub github_release: Option<bool>,
    #[serde(default)]
    pub github_release_name: Option<String>,
    #[serde(default)]
    pub changelog_update: Option<bool>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VersionSource {
    None,
    #[default]
    Workspace,
    Package,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Normal,
    Build,
    Dev,
}

fn default_base_ref() -> String {
    "origin/main".to_string()
}

fn default_bump_strategy() -> String {
    "conventional_commits".to_string()
}

fn default_bump() -> String {
    "patch".to_string()
}

const fn default_true() -> bool {
    true
}

fn default_dependency_kinds() -> Vec<DependencyKind> {
    vec![
        DependencyKind::Normal,
        DependencyKind::Build,
        DependencyKind::Dev,
    ]
}

pub fn load(repo_root: &Path) -> Result<LoadedConfig> {
    let cargo_toml_path = repo_root.join("Cargo.toml");
    let raw = fs::read_to_string(&cargo_toml_path).with_context(|| {
        format!(
            "failed to read workspace manifest: {}",
            cargo_toml_path.display()
        )
    })?;

    let parsed: RootCargoToml = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", cargo_toml_path.display()))?;

    let config = parsed
        .workspace
        .metadata
        .prup
        .ok_or_else(|| anyhow!("[workspace.metadata.prup] is missing in Cargo.toml"))?;

    let workspace_version =
        Version::parse(&parsed.workspace.package.version).with_context(|| {
            format!(
                "invalid [workspace.package].version in {}",
                cargo_toml_path.display()
            )
        })?;

    validate_config(&config)?;

    Ok(LoadedConfig {
        workspace_version,
        config,
    })
}

fn validate_config(config: &PrupConfig) -> Result<()> {
    if config.lines.is_empty() {
        return Err(anyhow!("[workspace.metadata.prup.lines] must not be empty"));
    }
    if config.crates.is_empty() {
        return Err(anyhow!(
            "[workspace.metadata.prup.crates] must not be empty"
        ));
    }
    if config.dependency_kinds.is_empty() {
        return Err(anyhow!(
            "[workspace.metadata.prup.dependency_kinds] must not be empty"
        ));
    }

    let mut line_ids = BTreeSet::new();
    for line in &config.lines {
        if line.id.trim().is_empty() {
            return Err(anyhow!("line.id must not be empty"));
        }
        if !line_ids.insert(line.id.clone()) {
            return Err(anyhow!("duplicate line.id: {}", line.id));
        }
    }

    if let Some(shared_line) = &config.shared_line
        && !line_ids.contains(shared_line)
    {
        return Err(anyhow!("shared_line references unknown line {shared_line}"));
    }

    let mut kinds = BTreeSet::new();
    for kind in &config.dependency_kinds {
        if !kinds.insert(*kind) {
            return Err(anyhow!("duplicate dependency_kinds entry: {:?}", kind));
        }
    }

    let mut release_pr_labels = BTreeSet::new();
    for label in &config.github.release_pr.labels {
        if label.trim().is_empty() {
            return Err(anyhow!("release_pr label must not be empty"));
        }
        if !release_pr_labels.insert(label.clone()) {
            return Err(anyhow!("duplicate release_pr label: {label}"));
        }
    }

    let line_map = config.line_map();
    let mut crate_names = BTreeSet::new();
    let mut line_emit_tag_counts: BTreeMap<&str, usize> = BTreeMap::new();

    for crate_policy in &config.crates {
        if crate_policy.name.trim().is_empty() {
            return Err(anyhow!("crate name must not be empty"));
        }
        if !crate_names.insert(crate_policy.name.clone()) {
            return Err(anyhow!("duplicate crate config: {}", crate_policy.name));
        }
        if !line_ids.contains(&crate_policy.line) {
            return Err(anyhow!(
                "crate {} references unknown line {}",
                crate_policy.name,
                crate_policy.line
            ));
        }
        if !crate_policy.emit_tag {
            return Err(anyhow!(
                "manual crate config is reserved for top crates; omit internal crate {}",
                crate_policy.name
            ));
        }
        if crate_policy.version_source == VersionSource::None {
            return Err(anyhow!(
                "emit_tag crate {} cannot use version_source=none",
                crate_policy.name
            ));
        }

        let line_policy = line_map
            .get(crate_policy.line.as_str())
            .ok_or_else(|| anyhow!("unknown line {}", crate_policy.line))?;

        if crate_policy.tag_pattern.is_none() && line_policy.tag_pattern.is_none() {
            return Err(anyhow!(
                "emit_tag crate {} must define tag_pattern (crate or line level)",
                crate_policy.name
            ));
        }

        *line_emit_tag_counts
            .entry(crate_policy.line.as_str())
            .or_insert(0) += 1;
    }

    for line in &config.lines {
        for propagate_target in &line.propagate_to {
            if !line_ids.contains(propagate_target) {
                return Err(anyhow!(
                    "line {} propagate_to references unknown line {}",
                    line.id,
                    propagate_target
                ));
            }
        }

        let emit_count = line_emit_tag_counts
            .get(line.id.as_str())
            .copied()
            .unwrap_or(0);

        if emit_count > 1 {
            return Err(anyhow!(
                "line {} has multiple emit_tag crates; exactly one is allowed",
                line.id
            ));
        }

        if line.kind == LineKind::PublicRelease && emit_count != 1 {
            return Err(anyhow!(
                "public-release line {} must have exactly one emit_tag crate",
                line.id
            ));
        }
    }

    if config.bump_strategy != "conventional_commits" {
        return Err(anyhow!(
            "unsupported bump_strategy: {} (expected conventional_commits)",
            config.bump_strategy
        ));
    }

    if !matches!(config.default_bump.as_str(), "patch" | "minor" | "major") {
        return Err(anyhow!(
            "default_bump must be one of patch/minor/major, got {}",
            config.default_bump
        ));
    }

    Ok(())
}

impl PrupConfig {
    pub fn line_map(&self) -> BTreeMap<&str, &LinePolicy> {
        let mut map = BTreeMap::new();
        for line in &self.lines {
            map.insert(line.id.as_str(), line);
        }
        map
    }

    pub fn dependency_kind_set(&self) -> BTreeSet<DependencyKind> {
        self.dependency_kinds.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> PrupConfig {
        PrupConfig {
            base_ref: "origin/main".to_string(),
            bump_strategy: "conventional_commits".to_string(),
            default_bump: "patch".to_string(),
            baseline_tag_required: true,
            allow_dirty: false,
            github_prerelease: true,
            dependency_kinds: default_dependency_kinds(),
            shared_line: Some("imago-shared".to_string()),
            github: GithubConfig {
                release_pr: ReleasePrConfig {
                    labels: vec!["release".to_string()],
                },
            },
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

    #[test]
    fn accepts_top_only_configuration() {
        let config = sample_config();
        validate_config(&config).expect("top-only config should validate");
    }

    #[test]
    fn rejects_manual_internal_crate_entries() {
        let mut config = sample_config();
        config.crates.push(CratePolicy {
            name: "imagod-common".to_string(),
            line: "imagod-daemon".to_string(),
            version_source: VersionSource::Workspace,
            emit_tag: false,
            tag_pattern: None,
            github_release: Some(false),
            github_release_name: None,
            changelog_update: Some(false),
        });

        let error = validate_config(&config).expect_err("internal crate entries should fail");
        assert!(
            error
                .to_string()
                .contains("manual crate config is reserved for top crates")
        );
    }

    #[test]
    fn rejects_duplicate_release_pr_labels() {
        let mut config = sample_config();
        config.github.release_pr.labels = vec!["release".to_string(), "release".to_string()];

        let error = validate_config(&config).expect_err("duplicate labels should fail");
        assert!(error.to_string().contains("duplicate release_pr label"));
    }

    #[test]
    fn rejects_unknown_shared_line() {
        let mut config = sample_config();
        config.shared_line = Some("missing".to_string());

        let error = validate_config(&config).expect_err("unknown shared line should fail");
        assert!(
            error
                .to_string()
                .contains("shared_line references unknown line")
        );
    }
}
