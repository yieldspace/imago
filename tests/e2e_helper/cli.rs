use anyhow::{Result, bail};
use serde_json::Value;
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
        self.output_json_lines()
            .filter(|line| json_line_type(line) == Some("command.summary"))
            .filter_map(|line| json_string_field(&line, "status"))
            .last()
    }

    pub fn command_summary_error(&self) -> Option<String> {
        self.output_json_lines()
            .filter(|line| json_line_type(line) == Some("command.summary"))
            .filter_map(|line| json_string_field(&line, "error"))
            .last()
    }

    pub fn has_command_error(&self) -> bool {
        self.output_json_lines()
            .any(|line| json_line_type(&line) == Some("command.error"))
    }

    pub fn command_error_messages(&self) -> Vec<String> {
        self.output_json_lines()
            .filter(|line| json_line_type(line) == Some("command.error"))
            .filter_map(|line| json_string_field(&line, "message"))
            .collect()
    }

    pub fn log_messages(&self) -> Vec<String> {
        self.output_json_lines()
            .filter(|line| json_line_type(line) == Some("log.line"))
            .filter_map(|line| json_string_field(&line, "log"))
            .collect()
    }

    fn output_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines().chain(self.stderr.lines())
    }

    fn output_json_lines(&self) -> impl Iterator<Item = Value> + '_ {
        self.output_lines().filter_map(parse_json_line)
    }
}

fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line.trim()).ok()
}

fn json_line_type(line: &Value) -> Option<&str> {
    line.get("type")?.as_str()
}

fn json_string_field(line: &Value, key: &str) -> Option<String> {
    line.get(key)?.as_str().map(ToOwned::to_owned)
}

pub fn run_imago_cli(
    workspace_root: &Path,
    project_dir: &Path,
    home_dir: &Path,
    args: &[&str],
) -> Result<CmdOutput> {
    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(workspace_root.join("Cargo.toml"))
        .arg("-p")
        .arg("imago-cli")
        .arg("--")
        .arg("--json")
        .args(args)
        .current_dir(project_dir)
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
