use anyhow::{Context, Result, anyhow};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    #[allow(dead_code)]
    pub sha: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<PathBuf>,
}

fn run_git<I, S>(repo_root: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .with_context(|| "failed to run git command")?;

    if !output.status.success() {
        return Err(anyhow!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn ensure_clean(repo_root: &Path, allow_dirty: bool) -> Result<()> {
    if allow_dirty {
        return Ok(());
    }

    let status = run_git(repo_root, ["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "worktree is dirty; rerun with --allow-dirty if intentional"
    ))
}

pub fn commits_since(repo_root: &Path, base_ref: &str) -> Result<Vec<CommitInfo>> {
    let range = format!("{base_ref}..HEAD");
    let output = run_git(
        repo_root,
        [
            "log",
            "--format=%H%x1f%s%x1f%b%x1f%x1e",
            "--name-only",
            "--no-renames",
            &range,
        ],
    )?;

    let mut commits = Vec::new();
    for record in output.split('\x1e') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }

        let mut parts = record.splitn(4, '\x1f');
        let sha = parts.next().unwrap_or_default().trim();
        let subject = parts.next().unwrap_or_default().trim();
        let body = parts.next().unwrap_or_default().trim();
        let files = parts.next().unwrap_or_default();

        if sha.is_empty() || subject.is_empty() {
            continue;
        }

        commits.push(CommitInfo {
            sha: sha.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
            files: files
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect(),
        });
    }

    Ok(commits)
}

pub fn list_tags(repo_root: &Path, pattern: &str) -> Result<Vec<String>> {
    let output = run_git(repo_root, ["tag", "--list", pattern, "--sort=-creatordate"])?;
    let tags = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    Ok(tags)
}

pub fn latest_tag(repo_root: &Path, pattern: &str) -> Result<Option<String>> {
    Ok(list_tags(repo_root, pattern)?.into_iter().next())
}

pub fn github_repo_name_with_owner(repo_root: &Path) -> Result<Option<String>> {
    let remote_url = run_git(repo_root, ["config", "--get", "remote.origin.url"])?;
    Ok(parse_github_repo_name_with_owner(remote_url.trim()))
}

fn parse_github_repo_name_with_owner(remote_url: &str) -> Option<String> {
    let remote_url = remote_url.trim().trim_end_matches(".git");

    if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        return Some(path.to_string());
    }

    if let Some(path) = remote_url.strip_prefix("https://github.com/") {
        return Some(path.to_string());
    }

    if let Some(path) = remote_url.strip_prefix("http://github.com/") {
        return Some(path.to_string());
    }

    if let Some(path) = remote_url.strip_prefix("ssh://git@github.com/") {
        return Some(path.to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_repo_name_from_ssh_remote() {
        assert_eq!(
            parse_github_repo_name_with_owner("git@github.com:yieldspace/imago.git"),
            Some("yieldspace/imago".to_string())
        );
    }

    #[test]
    fn parses_github_repo_name_from_https_remote() {
        assert_eq!(
            parse_github_repo_name_with_owner("https://github.com/yieldspace/imago.git"),
            Some("yieldspace/imago".to_string())
        );
    }
}
