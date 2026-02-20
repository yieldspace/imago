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
