use anyhow::{Context, Result, anyhow};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn changed_files_since(repo_root: &Path, base_ref: &str) -> Result<Vec<PathBuf>> {
    let range = format!("{base_ref}..HEAD");
    let output = run_git(repo_root, ["diff", "--name-only", &range])?;

    let files = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();

    Ok(files)
}

pub fn commit_messages_since(repo_root: &Path, base_ref: &str) -> Result<Vec<String>> {
    let range = format!("{base_ref}..HEAD");
    let output = run_git(repo_root, ["log", "--format=%B%x1f", &range])?;

    let messages = output
        .split('\x1f')
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    Ok(messages)
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
