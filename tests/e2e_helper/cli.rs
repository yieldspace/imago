use super::binaries::resolve_imagod_binary;
use anyhow::{Result, anyhow, bail};
use clap::Parser;
use imago_cli::{
    ParsedCli, dispatch_with_runtime,
    runtime::{BufferedOutputSink, CliRuntime, LocalProxyTargetConnector, OutputSink},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

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
        self.plain_command_error_messages().into_iter().last()
    }

    pub fn has_command_error(&self) -> bool {
        !self.plain_command_error_messages().is_empty()
    }

    fn plain_command_error_messages(&self) -> Vec<String> {
        let mut messages = collect_plain_error_messages(&self.stdout);
        messages.extend(collect_plain_error_messages(&self.stderr));
        messages
    }

    pub fn command_error_messages(&self) -> Vec<String> {
        let mut messages = self.plain_command_error_messages();
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
    if is_command_succeeded_line(line) {
        return Some("completed");
    }
    if is_command_failed_line(line) {
        return Some("failed");
    }
    None
}

fn is_command_name(command: &str) -> bool {
    let trimmed = command.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && !trimmed.contains('|')
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_command_succeeded_line(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(command) = trimmed.strip_suffix(" succeeded") else {
        return false;
    };
    is_command_name(command)
}

fn is_command_failed_line(line: &str) -> bool {
    let trimmed = line.trim();

    if let Some(command) = trimmed.strip_suffix(" failed") {
        return is_command_name(command);
    }

    let Some(without_suffix) = trimmed.strip_suffix(')') else {
        return false;
    };
    let Some((command, detail)) = without_suffix.split_once(" failed (") else {
        return false;
    };
    is_command_name(command) && !detail.trim().is_empty()
}

fn is_error_header_line(line: &str) -> bool {
    line.trim().starts_with("error:")
}

fn parse_plain_error_message(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let message = trimmed.strip_prefix("error:")?;
    Some(message.trim().to_string())
}

fn collect_plain_error_messages(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().collect();
    let mut messages = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        if let Some(mut message) = parse_plain_error_message(lines[idx]) {
            idx += 1;
            while idx < lines.len() {
                let continuation = lines[idx];
                if is_error_header_line(continuation)
                    || parse_plain_summary_status(continuation).is_some()
                {
                    break;
                }
                let continuation = continuation.trim();
                if !continuation.is_empty() {
                    message.push('\n');
                    message.push_str(continuation);
                }
                idx += 1;
            }
            messages.push(message);
            continue;
        }
        idx += 1;
    }
    messages
}

fn parse_plain_log_message(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let (name, message) = trimmed.split_once(" | ")?;
    if name.trim().is_empty() {
        return None;
    }
    Some(strip_optional_rfc3339_prefix(message.trim_start()).to_string())
}

fn strip_optional_rfc3339_prefix(message: &str) -> &str {
    let Some((candidate, rest)) = message.split_once(' ') else {
        return message;
    };
    if looks_like_rfc3339_with_offset(candidate) {
        return rest.trim_start();
    }
    message
}

fn looks_like_rfc3339_with_offset(token: &str) -> bool {
    let Some((date, time_with_tz)) = token.split_once('T') else {
        return false;
    };
    if date.len() != 10 {
        return false;
    }
    let mut date_parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) = (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) else {
        return false;
    };
    if !(year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && year.bytes().all(|b| b.is_ascii_digit())
        && month.bytes().all(|b| b.is_ascii_digit())
        && day.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }

    let time_bytes = time_with_tz.as_bytes();
    if time_bytes.len() < 9 || time_bytes[2] != b':' || time_bytes[5] != b':' {
        return false;
    }
    if !time_with_tz[0..2].bytes().all(|b| b.is_ascii_digit())
        || !time_with_tz[3..5].bytes().all(|b| b.is_ascii_digit())
        || !time_with_tz[6..8].bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }

    let mut tz = &time_with_tz[8..];
    if let Some(fraction) = tz.strip_prefix('.') {
        let digits_end = fraction
            .bytes()
            .position(|byte| !byte.is_ascii_digit())
            .unwrap_or(fraction.len());
        if digits_end == 0 {
            return false;
        }
        tz = &fraction[digits_end..];
    }

    if tz == "Z" {
        return true;
    }

    let Some(first) = tz.as_bytes().first() else {
        return false;
    };
    if !matches!(first, b'+' | b'-') {
        return false;
    }
    if tz.len() != 6 {
        return false;
    }
    tz.as_bytes()[3] == b':'
        && tz[1..3].bytes().all(|b| b.is_ascii_digit())
        && tz[4..6].bytes().all(|b| b.is_ascii_digit())
}

static CLI_CWD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct CwdGuard {
    previous: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

impl CwdGuard {
    fn change_to(path: &Path) -> Result<Self> {
        let lock = CLI_CWD_LOCK.get_or_init(|| Mutex::new(()));
        let guard = lock
            .lock()
            .map_err(|_| anyhow!("failed to lock e2e cli cwd guard"))?;
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self {
            previous,
            _lock: guard,
        })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collect_plain_error_messages, looks_like_rfc3339_with_offset, parse_plain_log_message,
        parse_plain_summary_status,
    };

    #[test]
    fn parses_summary_status_from_new_cli_lines() {
        assert_eq!(
            parse_plain_summary_status("deploy succeeded"),
            Some("completed")
        );
        assert_eq!(
            parse_plain_summary_status("deploy failed (build stage failed)"),
            Some("failed")
        );
        assert_eq!(parse_plain_summary_status("deploy failed"), Some("failed"));
        assert_eq!(
            parse_plain_summary_status("warning: deploy: retrying"),
            None
        );
        assert_eq!(parse_plain_summary_status("svc-a | deploy succeeded"), None);
    }

    #[test]
    fn collects_multiline_error_block_with_new_format() {
        let output = "\
deploy failed (build stage failed)\n\
error: service exited before ready\n\
caused by:\n\
  - deployment failed\n\
wasm stdout:\n\
  IMAGO_E2E_DEPLOY_FAIL_STDOUT\n\
wasm stderr:\n\
  IMAGO_E2E_DEPLOY_FAIL_STDERR\n\
";
        let messages = collect_plain_error_messages(output);
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert!(message.starts_with("service exited before ready"));
        assert!(message.contains("wasm stdout:"));
        assert!(message.contains("IMAGO_E2E_DEPLOY_FAIL_STDOUT"));
        assert!(message.contains("wasm stderr:"));
        assert!(message.contains("IMAGO_E2E_DEPLOY_FAIL_STDERR"));
    }

    #[test]
    fn stops_collecting_error_block_at_next_summary_or_error_header() {
        let output = "\
error: deploy failed\n\
  detail line\n\
deploy failed (build stage failed)\n\
error: another failure\n\
  detail two\n";
        let messages = collect_plain_error_messages(output);
        assert_eq!(
            messages,
            vec![
                "deploy failed\ndetail line".to_string(),
                "another failure\ndetail two".to_string()
            ]
        );
    }

    #[test]
    fn parses_log_message_with_or_without_timestamp() {
        assert_eq!(
            parse_plain_log_message("svc-a | hello"),
            Some("hello".to_string())
        );
        assert_eq!(
            parse_plain_log_message("svc-a | 2026-02-26T17:32:10+09:00 hello"),
            Some("hello".to_string())
        );
        assert_eq!(
            parse_plain_log_message("svc-a | 2026-02-26T08:32:10Z hello"),
            Some("hello".to_string())
        );
        assert_eq!(
            parse_plain_log_message("svc-a | 2026-02-26T08:32:10.123456Z hello"),
            Some("hello".to_string())
        );
        assert_eq!(
            parse_plain_log_message("svc-a | 2026/02/26 hello"),
            Some("2026/02/26 hello".to_string())
        );
    }

    #[test]
    fn rfc3339_with_offset_detector_is_strict_enough_for_log_prefix() {
        assert!(looks_like_rfc3339_with_offset("2026-02-26T17:32:10+09:00"));
        assert!(looks_like_rfc3339_with_offset("2026-02-26T08:32:10Z"));
        assert!(looks_like_rfc3339_with_offset("2026-02-26T08:32:10.12Z"));
        assert!(!looks_like_rfc3339_with_offset("2026-02-26 08:32:10"));
        assert!(!looks_like_rfc3339_with_offset("2026-02-26T08:32:10"));
        assert!(!looks_like_rfc3339_with_offset("not-a-timestamp"));
    }
}

pub fn run_imago_cli(
    workspace_root: &Path,
    project_dir: &Path,
    daemon_package: &str,
    args: &[&str],
) -> Result<CmdOutput> {
    let _cwd_guard = CwdGuard::change_to(project_dir)?;
    let imagod_binary = resolve_imagod_binary(workspace_root, daemon_package)?;
    let output_sink = Arc::new(BufferedOutputSink::default());
    let cli_runtime = Arc::new(CliRuntime::plain(
        project_dir,
        Arc::new(LocalProxyTargetConnector::new(imagod_binary)),
        output_sink.clone(),
    ));
    let argv = std::iter::once("imago".to_string())
        .chain(args.iter().map(|arg| (*arg).to_string()))
        .collect::<Vec<_>>();

    let runtime_result = match ParsedCli::try_parse_from(&argv) {
        Ok(cli) => {
            let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let result = tokio_runtime.block_on(dispatch_with_runtime(cli, cli_runtime));
            (result.exit_code, output_sink.snapshot())
        }
        Err(err) => {
            let exit_code = err.exit_code();
            let rendered = err.to_string();
            if err.use_stderr() {
                let _ = output_sink.write_stderr(rendered.as_bytes());
            } else {
                let _ = output_sink.write_stdout(rendered.as_bytes());
            }
            (exit_code, output_sink.snapshot())
        }
    };

    let (exit_code, captured) = runtime_result;
    let stdout = captured.stdout;
    let stderr = captured.stderr;
    let combined = format!("{stdout}{stderr}");

    Ok(CmdOutput {
        status: format!("exit status: {exit_code}"),
        status_code: Some(exit_code),
        success: exit_code == 0,
        stdout,
        stderr,
        combined,
    })
}

#[cfg(test)]
mod run_imago_cli_tests {
    use super::run_imago_cli;
    use anyhow::Result;
    use std::{
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn run_imago_cli_resolves_relative_outputs_from_project_dir() -> Result<()> {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let project_dir = temp_dir("run-imago-cli-cwd");
        let relative_out_dir = "generated-certs";
        let cwd_out_dir = workspace_root.join(relative_out_dir);
        let _cleanup_project = CleanupPath(project_dir.clone());
        let _cleanup_cwd = CleanupPath(cwd_out_dir.clone());

        let output = run_imago_cli(
            workspace_root,
            &project_dir,
            "imagod",
            &[
                "trust",
                "client-key",
                "generate",
                "--out-dir",
                relative_out_dir,
            ],
        )?;

        assert!(
            output.success,
            "command should succeed: stdout={}, stderr={}",
            output.stdout, output.stderr
        );
        assert!(
            project_dir
                .join(relative_out_dir)
                .join("client.key")
                .exists()
        );
        assert!(
            !cwd_out_dir.join("client.key").exists(),
            "relative output should not be created under current test runner cwd"
        );
        Ok(())
    }

    struct CleanupPath(PathBuf);

    impl Drop for CleanupPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{ts}"));
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }
}
