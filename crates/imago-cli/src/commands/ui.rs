use std::{collections::BTreeMap, env, time::Duration};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};

use super::CommandResult;
use crate::runtime;

const DOT_SPINNER_TICKS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const UPLOAD_PROGRESS_CHARS: &str = "#>-";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_RESET: &str = "\x1b[0m";

fn spinner_style_template() -> &'static str {
    "{spinner:.cyan} {msg}"
}

fn upload_style_template() -> &'static str {
    "{msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})"
}

fn finished_style_template() -> &'static str {
    "{msg}"
}

fn spinner_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(spinner_style_template())
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_chars(DOT_SPINNER_TICKS)
}

fn upload_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(upload_style_template())
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .with_key(
            "eta",
            |state: &ProgressState, writer: &mut dyn std::fmt::Write| {
                let _ = write!(writer, "{:.1}s", state.eta().as_secs_f64());
            },
        )
        .progress_chars(UPLOAD_PROGRESS_CHARS)
}

fn finished_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(finished_style_template())
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
}

fn rich_start_message(command: &str, detail: &str) -> String {
    format!("{command}: {detail}...")
}

fn plain_start_message(command: &str, detail: &str) -> String {
    format!("{command}: {detail}...")
}

fn rich_stage_message(command: &str, stage: &str, detail: &str) -> String {
    let _ = stage;
    format!("{command}: {detail}...")
}

fn plain_stage_message(command: &str, stage: &str, detail: &str) -> String {
    let _ = stage;
    format!("{command}: {detail}...")
}

fn rich_warn_message(command: &str, message: &str) -> String {
    format!("warning: {command}: {message}")
}

fn compose_build_service_stage_message(service: &str, stage: &str, detail: &str) -> String {
    format!("compose build [{service}] [{stage}] {detail}")
}

fn compose_build_service_waiting_log_message(service: &str) -> String {
    format!("{ANSI_DIM}  > {service}: waiting for build output{ANSI_RESET}")
}

fn compose_build_service_log_message(service: &str, stream: &str, line: &str) -> String {
    format!("{ANSI_DIM}  > {service}: [{stream}] {line}{ANSI_RESET}")
}

fn compose_build_service_failure_message(service: &str, detail: &str) -> String {
    rich_failure_message("compose build", &format!("service={service} {detail}"))
}

fn plain_warn_message(command: &str, message: &str) -> String {
    format!("warning: {command}: {message}")
}

fn plain_upload_start_message(command: &str, total_bytes: u64, detail: &str) -> String {
    format!("{command}: {detail}... ({total_bytes} bytes)")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Rich,
    Plain,
}

#[derive(Debug)]
pub(crate) struct UiState {
    mode: UiMode,
    rich: Option<RichState>,
}

impl UiState {
    pub(crate) fn new(mode: UiMode) -> Self {
        let rich = match mode {
            UiMode::Rich => Some(RichState::new()),
            UiMode::Plain => None,
        };
        Self { mode, rich }
    }

    pub(crate) fn mode(&self) -> UiMode {
        self.mode
    }
}

#[derive(Debug, Clone)]
struct ComposeServiceLines {
    stage: ProgressBar,
    log: ProgressBar,
}

#[derive(Debug)]
struct RichState {
    multi: MultiProgress,
    spinners: BTreeMap<String, ProgressBar>,
    byte_bars: BTreeMap<String, ProgressBar>,
    compose_service_lines: BTreeMap<String, ComposeServiceLines>,
}

impl RichState {
    fn new() -> Self {
        Self {
            multi: MultiProgress::with_draw_target(ProgressDrawTarget::stdout()),
            spinners: BTreeMap::new(),
            byte_bars: BTreeMap::new(),
            compose_service_lines: BTreeMap::new(),
        }
    }

    fn ensure_spinner(&mut self, command: &str, detail: &str) -> ProgressBar {
        if let Some(spinner) = self.spinners.get(command) {
            return spinner.clone();
        }

        let spinner = self.multi.add(ProgressBar::new_spinner());
        spinner.set_style(spinner_progress_style());
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message(rich_start_message(command, detail));
        self.spinners.insert(command.to_string(), spinner.clone());
        spinner
    }

    fn ensure_compose_service_lines(&mut self, service: &str) -> ComposeServiceLines {
        if let Some(lines) = self.compose_service_lines.get(service) {
            return lines.clone();
        }

        let stage = self.multi.add(ProgressBar::new_spinner());
        stage.set_style(spinner_progress_style());
        stage.enable_steady_tick(Duration::from_millis(100));
        stage.set_message(compose_build_service_stage_message(
            service, "waiting", "queued",
        ));

        let log = self.multi.add(ProgressBar::new_spinner());
        log.set_style(finished_progress_style());
        log.set_message(compose_build_service_waiting_log_message(service));

        let lines = ComposeServiceLines { stage, log };
        self.compose_service_lines
            .insert(service.to_string(), lines.clone());
        lines
    }

    fn compose_build_service_stage(&mut self, service: &str, stage: &str, detail: &str) {
        let lines = self.ensure_compose_service_lines(service);
        lines
            .stage
            .set_message(compose_build_service_stage_message(service, stage, detail));
        lines.stage.tick();
    }

    fn compose_build_service_log(&mut self, service: &str, stream: &str, line: &str) {
        let lines = self.ensure_compose_service_lines(service);
        lines
            .log
            .set_message(compose_build_service_log_message(service, stream, line));
        lines.log.tick();
    }

    fn compose_build_service_finish(&mut self, service: &str, succeeded: bool, detail: &str) {
        let Some(lines) = self.compose_service_lines.remove(service) else {
            return;
        };

        if succeeded {
            lines.stage.finish_and_clear();
            lines.log.finish_and_clear();
            return;
        }

        lines.stage.set_style(finished_progress_style());
        lines
            .stage
            .finish_with_message(compose_build_service_failure_message(service, detail));
        lines.log.finish();
    }

    fn clear_command(&mut self, command: &str) {
        if let Some(bar) = self.byte_bars.remove(command) {
            bar.finish_and_clear();
        }
        if let Some(spinner) = self.spinners.remove(command) {
            spinner.finish_and_clear();
        }
    }
}

pub fn initialize() -> UiMode {
    current_mode()
}

pub fn current_mode() -> UiMode {
    if let Some(mode) = runtime::with_ui_state(|state| state.mode()) {
        return mode;
    }
    detect_mode()
}

fn startup_banner_lines_for_mode(_mode: UiMode, version: &str) -> Option<(String, String)> {
    let header = format!("imago {version}");
    let rule = "─".repeat(header.chars().count());
    Some((header, rule))
}

pub fn emit_startup_banner(version: &str) {
    let Some((header, rule)) = startup_banner_lines_for_mode(current_mode(), version) else {
        return;
    };
    let _ = runtime::write_stdout_line(&header);
    let _ = runtime::write_stdout_line(&rule);
}

pub(crate) fn detect_mode() -> UiMode {
    let ci_env = env::var("CI").ok();
    detect_mode_from_ci(ci_env.as_deref())
}

fn detect_mode_from_ci(ci_env: Option<&str>) -> UiMode {
    if ci_env.map(is_ci_value_enabled).unwrap_or(false) {
        return UiMode::Plain;
    }
    UiMode::Rich
}

fn is_ci_value_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized == "true" || normalized == "1"
}

fn with_rich_state<R>(f: impl FnOnce(&mut RichState) -> R) -> Option<R> {
    runtime::with_ui_state(|state| state.rich.as_mut().map(f)).flatten()
}

pub fn command_start(command: &str, detail: &str) {
    match current_mode() {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&plain_start_message(command, detail));
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                state.ensure_spinner(command, detail);
            });
        }
    }
}

pub fn command_stage(command: &str, stage: &str, detail: &str) {
    match current_mode() {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&plain_stage_message(command, stage, detail));
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                let spinner = state.ensure_spinner(command, "started");
                spinner.set_message(rich_stage_message(command, stage, detail));
                spinner.tick();
            });
        }
    }
}

pub fn command_warn(command: &str, message: &str) {
    match current_mode() {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&plain_warn_message(command, message));
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                emit_rich_inline_line(state, command, rich_warn_message(command, message));
            });
        }
    }
}

fn rich_info_message(message: &str) -> String {
    format!("{ANSI_DIM}  > {message}{ANSI_RESET}")
}

fn plain_info_message(_command: &str, message: &str) -> String {
    format!("  {message}")
}

fn info_output_for_mode(mode: UiMode, command: &str, message: &str) -> Option<String> {
    match mode {
        UiMode::Plain => Some(plain_info_message(command, message)),
        UiMode::Rich => Some(rich_info_message(message)),
    }
}

fn rich_success_message(command: &str, detail: &str) -> String {
    let _ = detail;
    format!("{command} succeeded")
}

fn rich_failure_message(command: &str, detail: &str) -> String {
    if detail.trim().is_empty() {
        return format!("{command} failed");
    }
    format!("{command} failed ({detail})")
}

fn plain_finish_message(command: &str, succeeded: bool, detail: &str) -> String {
    if succeeded {
        let _ = detail;
        format!("{command} succeeded")
    } else {
        rich_failure_message(command, detail)
    }
}

fn emit_rich_inline_line(state: &mut RichState, command: &str, line: String) {
    if let Some(spinner) = state.spinners.get(command) {
        spinner.println(line);
    } else {
        let _ = runtime::write_stdout_line(&line);
    }
}

pub fn command_info(command: &str, message: &str) {
    let mode = current_mode();
    let Some(formatted) = info_output_for_mode(mode, command, message) else {
        return;
    };
    match mode {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&formatted);
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                emit_rich_inline_line(state, command, formatted);
            });
        }
    }
}

pub fn command_upload_start(command: &str, total_bytes: u64, detail: &str) {
    match current_mode() {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&plain_upload_start_message(
                command,
                total_bytes,
                detail,
            ));
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                let bar = if let Some(existing) = state.byte_bars.get(command) {
                    existing.clone()
                } else {
                    let created = state.multi.add(ProgressBar::new(total_bytes));
                    state.byte_bars.insert(command.to_string(), created.clone());
                    created
                };
                bar.set_style(upload_progress_style());
                bar.set_length(total_bytes);
                bar.set_position(0);
                bar.set_message(rich_start_message(command, detail));
            });
        }
    }
}

pub fn command_upload_inc(command: &str, bytes: u64) {
    match current_mode() {
        UiMode::Plain => {}
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                if let Some(bar) = state.byte_bars.get(command) {
                    bar.inc(bytes);
                }
            });
        }
    }
}

pub fn command_upload_finish(command: &str) {
    match current_mode() {
        UiMode::Plain => {}
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                if let Some(bar) = state.byte_bars.remove(command) {
                    bar.finish_and_clear();
                }
            });
        }
    }
}

pub fn command_finish(command: &str, succeeded: bool, detail: &str) {
    match current_mode() {
        UiMode::Plain => {
            let _ = runtime::write_stdout_line(&plain_finish_message(command, succeeded, detail));
        }
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                if let Some(bar) = state.byte_bars.remove(command) {
                    bar.finish_and_clear();
                }
                if let Some(spinner) = state.spinners.remove(command) {
                    spinner.set_style(finished_progress_style());
                    if succeeded {
                        spinner.finish_with_message(rich_success_message(command, detail));
                    } else {
                        spinner.finish_with_message(rich_failure_message(command, detail));
                    }
                } else if succeeded {
                    let _ = runtime::write_stdout_line(&rich_success_message(command, detail));
                } else {
                    let _ = runtime::write_stdout_line(&rich_failure_message(command, detail));
                }
            });
        }
    }
}

pub fn command_clear(command: &str) {
    if current_mode() != UiMode::Rich {
        return;
    }
    let _ = with_rich_state(|state| {
        state.clear_command(command);
    });
}

pub(crate) fn ensure_compose_service_lines(service: &str) {
    if current_mode() != UiMode::Rich {
        return;
    }
    let _ = with_rich_state(|state| {
        state.ensure_compose_service_lines(service);
    });
}

pub(crate) fn compose_build_service_stage(service: &str, stage: &str, detail: &str) {
    if current_mode() != UiMode::Rich {
        return;
    }
    let _ = with_rich_state(|state| {
        state.compose_build_service_stage(service, stage, detail);
    });
}

pub(crate) fn compose_build_service_log(service: &str, stream: &str, line: &str) {
    if current_mode() != UiMode::Rich {
        return;
    }
    let _ = with_rich_state(|state| {
        state.compose_build_service_log(service, stream, line);
    });
}

pub(crate) fn compose_build_service_finish(service: &str, succeeded: bool, detail: &str) {
    if current_mode() != UiMode::Rich {
        return;
    }
    let _ = with_rich_state(|state| {
        state.compose_build_service_finish(service, succeeded, detail);
    });
}

fn finalize_error_output_line(result: &CommandResult) -> Option<String> {
    if result.exit_code != 0
        && let Some(message) = result.stderr.as_deref()
    {
        return Some(message.to_string());
    }
    None
}

fn should_suppress_success_meta_output(result: &CommandResult) -> bool {
    result
        .meta
        .get("_suppress_success_meta_output")
        .is_some_and(|value| value == "true")
}

fn success_meta_lines(result: &CommandResult) -> Vec<String> {
    result
        .meta
        .iter()
        .filter(|(key, _)| !key.starts_with('_'))
        .map(|(key, value)| format!("  {key}: {value}"))
        .collect()
}

pub fn finalize_result(result: &CommandResult) {
    if let Some(message) = finalize_error_output_line(result) {
        let _ = runtime::write_stderr_line(&message);
        return;
    }

    if should_suppress_success_meta_output(result) {
        return;
    }

    for line in success_meta_lines(result) {
        let _ = runtime::write_stdout_line(&line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::{path::Path, sync::Arc};

    use crate::runtime::{self, BufferedOutputSink, CliRuntime, OutputSink, SshTargetConnector};

    fn plain_runtime(output_sink: Arc<dyn OutputSink>) -> Arc<CliRuntime> {
        Arc::new(CliRuntime::plain(
            Path::new("."),
            Arc::new(SshTargetConnector),
            output_sink,
        ))
    }

    fn capture_output(action: impl std::future::Future<Output = ()>) -> runtime::BufferedOutput {
        let output_sink = Arc::new(BufferedOutputSink::default());
        let runtime = plain_runtime(output_sink.clone());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(runtime::scope(runtime, action));
        output_sink.snapshot()
    }

    #[test]
    fn detects_plain_when_ci_enabled() {
        assert_eq!(detect_mode_from_ci(Some("true")), UiMode::Plain);
        assert_eq!(detect_mode_from_ci(Some("1")), UiMode::Plain);
    }

    #[test]
    fn detects_rich_when_ci_is_disabled() {
        assert_eq!(detect_mode_from_ci(Some("false")), UiMode::Rich);
        assert_eq!(detect_mode_from_ci(None), UiMode::Rich);
    }

    #[test]
    fn dot_spinner_ticks_are_stable_and_non_empty() {
        assert!(!DOT_SPINNER_TICKS.is_empty());
        assert_eq!(DOT_SPINNER_TICKS, "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");
    }

    #[test]
    fn spinner_style_template_contains_spinner_and_message() {
        let template = spinner_style_template();
        assert!(template.contains("{spinner"));
        assert!(template.contains("{msg}"));
    }

    #[test]
    fn upload_style_template_contains_expected_tokens() {
        let template = upload_style_template();
        assert!(template.contains("{wide_bar"));
        assert!(template.contains("{bytes}"));
        assert!(template.contains("{total_bytes}"));
        assert!(template.contains("{eta}"));
        assert_eq!(UPLOAD_PROGRESS_CHARS, "#>-");
    }

    #[test]
    fn finished_style_template_hides_spinner_placeholder() {
        let template = finished_style_template();
        assert_eq!(template, "{msg}");
        assert!(!template.contains("{spinner"));
    }

    #[test]
    fn rich_success_message_uses_natural_success_line() {
        assert_eq!(
            rich_success_message("deploy", "completed"),
            "deploy succeeded"
        );
    }

    #[test]
    fn info_output_for_mode_uses_expected_format() {
        let message = "cli=0.1.0 project=/tmp/x";
        assert_eq!(
            info_output_for_mode(UiMode::Plain, "deploy", message),
            Some("  cli=0.1.0 project=/tmp/x".to_string())
        );
        assert_eq!(
            info_output_for_mode(UiMode::Rich, "deploy", message),
            Some("\u{1b}[2m  > cli=0.1.0 project=/tmp/x\u{1b}[0m".to_string())
        );
    }

    #[test]
    fn startup_banner_lines_are_generated_for_rich_and_plain() {
        let rich = startup_banner_lines_for_mode(UiMode::Rich, "0.1.0").expect("rich banner");
        assert_eq!(rich.0, "imago 0.1.0");
        assert!(rich.1.chars().all(|ch| ch == '─'));
        assert_eq!(rich.1.chars().count(), rich.0.chars().count());

        let plain = startup_banner_lines_for_mode(UiMode::Plain, "0.1.0").expect("plain banner");
        assert_eq!(plain.0, "imago 0.1.0");
        assert!(plain.1.chars().all(|ch| ch == '─'));
        assert_eq!(plain.1.chars().count(), plain.0.chars().count());
    }

    #[test]
    fn finalize_error_output_line_formats_failure() {
        let result = CommandResult {
            command: "deploy".to_string(),
            exit_code: 2,
            stderr: Some("error: failed".to_string()),
            duration_ms: 0,
            meta: BTreeMap::new(),
        };
        assert_eq!(
            finalize_error_output_line(&result),
            Some("error: failed".to_string())
        );
    }

    #[test]
    fn finalize_error_output_line_is_none_for_success() {
        let result = CommandResult {
            command: "deploy".to_string(),
            exit_code: 0,
            stderr: Some("ignored".to_string()),
            duration_ms: 0,
            meta: BTreeMap::new(),
        };
        assert_eq!(finalize_error_output_line(&result), None);
    }

    #[test]
    fn success_meta_lines_hide_internal_keys() {
        let mut result = CommandResult {
            command: "deploy".to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: 0,
            meta: BTreeMap::new(),
        };
        result.meta.insert(
            "_suppress_success_meta_output".to_string(),
            "false".to_string(),
        );
        result
            .meta
            .insert("service".to_string(), "svc-a".to_string());

        assert_eq!(success_meta_lines(&result), vec!["  service: svc-a"]);
    }

    #[test]
    fn finalize_result_writes_failure_to_runtime_stderr() {
        let result = CommandResult {
            command: "deploy".to_string(),
            exit_code: 2,
            stderr: Some("error: failed".to_string()),
            duration_ms: 0,
            meta: BTreeMap::new(),
        };

        let output = capture_output(async move {
            finalize_result(&result);
        });

        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, "error: failed\n");
    }

    #[test]
    fn finalize_result_writes_success_meta_to_runtime_stdout() {
        let mut result = CommandResult {
            command: "deploy".to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: 0,
            meta: BTreeMap::new(),
        };
        result
            .meta
            .insert("target".to_string(), "default".to_string());

        let output = capture_output(async move {
            finalize_result(&result);
        });

        assert_eq!(output.stdout, "  target: default\n");
        assert_eq!(output.stderr, "");
    }

    #[test]
    fn suppress_success_meta_output_respects_internal_flag() {
        let mut result = CommandResult {
            command: "logs".to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: 0,
            meta: BTreeMap::new(),
        };
        result.meta.insert(
            "_suppress_success_meta_output".to_string(),
            "true".to_string(),
        );

        assert!(should_suppress_success_meta_output(&result));
    }

    #[test]
    fn ensure_compose_service_lines_initializes_waiting_log() {
        let mut state = RichState::new();
        let lines = state.ensure_compose_service_lines("api");

        assert_eq!(
            lines.log.message(),
            compose_build_service_waiting_log_message("api")
        );
    }

    #[test]
    fn compose_build_service_log_overwrites_latest_message() {
        let mut state = RichState::new();
        let lines = state.ensure_compose_service_lines("api");

        state.compose_build_service_log("api", "stdout", "step1");
        state.compose_build_service_log("api", "stderr", "step2");

        assert_eq!(
            lines.log.message(),
            compose_build_service_log_message("api", "stderr", "step2")
        );
    }

    #[test]
    fn compose_build_service_stage_updates_progress_line() {
        let mut state = RichState::new();
        let lines = state.ensure_compose_service_lines("api");

        state.compose_build_service_stage("api", "build", "compiling");

        assert_eq!(
            lines.stage.message(),
            compose_build_service_stage_message("api", "build", "compiling")
        );
    }

    #[test]
    fn compose_build_service_finish_success_removes_service_lines() {
        let mut state = RichState::new();
        state.ensure_compose_service_lines("api");

        state.compose_build_service_finish("api", true, "completed");

        assert!(!state.compose_service_lines.contains_key("api"));
    }

    #[test]
    fn compose_build_service_finish_failure_keeps_latest_log_line() {
        let mut state = RichState::new();
        let lines = state.ensure_compose_service_lines("api");
        state.compose_build_service_log("api", "stderr", "compile failed");

        state.compose_build_service_finish("api", false, "build failed");

        assert!(!state.compose_service_lines.contains_key("api"));
        assert!(lines.stage.is_finished());
        assert_eq!(
            lines.stage.message(),
            compose_build_service_failure_message("api", "build failed")
        );
        assert!(lines.log.is_finished());
        assert_eq!(
            lines.log.message(),
            compose_build_service_log_message("api", "stderr", "compile failed")
        );
    }

    #[test]
    fn clear_command_removes_spinner_and_upload_bar() {
        let mut state = RichState::new();
        let spinner = state.ensure_spinner("logs", "starting");
        let bar = state.multi.add(ProgressBar::new(64));
        state.byte_bars.insert("logs".to_string(), bar.clone());

        state.clear_command("logs");

        assert!(!state.spinners.contains_key("logs"));
        assert!(!state.byte_bars.contains_key("logs"));
        assert!(spinner.is_finished());
        assert!(bar.is_finished());
    }
}
