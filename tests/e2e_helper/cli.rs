use super::binaries::resolve_imago_cli_binary;
use anyhow::{Result, bail};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub status: String,
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub combined: String,
}

impl CmdOutput {
    pub fn ensure_success(&self, args: &[&str]) -> Result<()> {
        if self.success {
            return Ok(());
        }

        bail!(
            "imago-cli failed: args={args:?}, status={}, stdout={}, stderr={}",
            self.status,
            self.stdout,
            self.stderr
        )
    }

    pub fn command_summary_status(&self) -> Option<String> {
        self.output_lines()
            .filter_map(parse_plain_summary_status)
            .map(ToOwned::to_owned)
            .last()
    }

    pub fn command_summary_error(&self) -> Option<String> {
        self.output_lines()
            .filter_map(parse_plain_error_message)
            .last()
    }

    pub fn has_command_error(&self) -> bool {
        self.output_lines()
            .any(|line| parse_plain_error_message(line).is_some())
    }

    pub fn command_error_messages(&self) -> Vec<String> {
        let mut messages: Vec<String> = self
            .output_lines()
            .filter_map(parse_plain_error_message)
            .collect();
        if messages.is_empty() {
            let fallback = self.stderr.trim();
            if !fallback.is_empty() {
                messages.push(fallback.to_string());
            }
        }
        messages
    }

    pub fn log_messages(&self) -> Vec<String> {
        self.output_lines()
            .filter_map(parse_plain_log_message)
            .collect()
    }

    fn output_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines().chain(self.stderr.lines())
    }
}

fn parse_plain_summary_status(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.starts_with("[completed] ") {
        return Some("completed");
    }
    if trimmed.starts_with("[failed] ") {
        return Some("failed");
    }
    None
}

fn parse_plain_error_message(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("[error] ")?;
    let (_command, message) = body.split_once(' ')?;
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    Some(message.to_string())
}

fn parse_plain_log_message(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let (prefix, message) = trimmed.split_once(" | ")?;
    let mut parts = prefix.split_whitespace();
    let _service = parts.next()?;
    let stream = parts.next()?;
    if !matches!(stream, "stdout" | "stderr" | "composite") {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(message.to_string())
}

pub fn run_imago_cli(
    workspace_root: &Path,
    project_dir: &Path,
    home_dir: &Path,
    args: &[&str],
) -> Result<CmdOutput> {
    let cli_binary = resolve_imago_cli_binary(workspace_root)?;
    let output = Command::new(cli_binary)
        .args(args)
        .current_dir(project_dir)
        .env("CI", "true")
        .env("HOME", home_dir)
        .env("USERPROFILE", home_dir)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");

    Ok(CmdOutput {
        status: output.status.to_string(),
        status_code: output.status.code(),
        success: output.status.success(),
        stdout,
        stderr,
        combined,
    })
}
