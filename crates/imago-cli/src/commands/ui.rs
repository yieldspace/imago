use std::{
    collections::BTreeMap,
    env,
    io::{self, Write},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};
use serde::Serialize;
use time::OffsetDateTime;

use super::CommandResult;

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
    format!("{command} {detail}")
}

fn plain_start_message(command: &str, detail: &str) -> String {
    format!("[start] {command} {detail}")
}

fn rich_stage_message(command: &str, stage: &str, detail: &str) -> String {
    format!("{command} [{stage}] {detail}")
}

fn plain_stage_message(command: &str, stage: &str, detail: &str) -> String {
    format!("[progress] {command} stage={stage} {detail}")
}

fn rich_warn_message(command: &str, message: &str) -> String {
    format!("[warn] {command} {message}")
}

fn plain_warn_message(command: &str, message: &str) -> String {
    format!("[warn] {command} {message}")
}

fn plain_upload_start_message(command: &str, total_bytes: u64, detail: &str) -> String {
    format!("[progress] {command} stage=upload {detail} total_bytes={total_bytes}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Rich,
    Plain,
    Json,
}

#[derive(Debug)]
struct UiRuntime {
    mode: UiMode,
    rich: Option<Mutex<RichState>>,
}

impl UiRuntime {
    fn new(mode: UiMode) -> Self {
        let rich = match mode {
            UiMode::Rich => Some(Mutex::new(RichState::new())),
            UiMode::Plain | UiMode::Json => None,
        };
        Self { mode, rich }
    }
}

#[derive(Debug)]
struct RichState {
    multi: MultiProgress,
    spinners: BTreeMap<String, ProgressBar>,
    byte_bars: BTreeMap<String, ProgressBar>,
}

impl RichState {
    fn new() -> Self {
        Self {
            multi: MultiProgress::with_draw_target(ProgressDrawTarget::stdout()),
            spinners: BTreeMap::new(),
            byte_bars: BTreeMap::new(),
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
}

static UI_RUNTIME: OnceLock<Mutex<Option<UiRuntime>>> = OnceLock::new();

fn runtime_cell() -> &'static Mutex<Option<UiRuntime>> {
    UI_RUNTIME.get_or_init(|| Mutex::new(None))
}

pub fn initialize(global_json: bool) -> UiMode {
    let mode = detect_mode(global_json);
    if let Ok(mut guard) = runtime_cell().lock() {
        *guard = Some(UiRuntime::new(mode));
    }
    mode
}

pub fn current_mode() -> UiMode {
    if let Ok(guard) = runtime_cell().lock()
        && let Some(runtime) = guard.as_ref()
    {
        return runtime.mode;
    }
    detect_mode(false)
}

fn startup_banner_lines_for_mode(mode: UiMode, version: &str) -> Option<(String, String)> {
    if mode == UiMode::Json {
        return None;
    }
    let header = format!("imago {version}");
    let rule = "─".repeat(header.chars().count());
    Some((header, rule))
}

pub fn emit_startup_banner(version: &str) {
    let Some((header, rule)) = startup_banner_lines_for_mode(current_mode(), version) else {
        return;
    };
    println!("{header}");
    println!("{rule}");
}

fn detect_mode(global_json: bool) -> UiMode {
    let ci_env = env::var("CI").ok();
    detect_mode_from_ci(global_json, ci_env.as_deref())
}

fn detect_mode_from_ci(global_json: bool, ci_env: Option<&str>) -> UiMode {
    if global_json {
        return UiMode::Json;
    }
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
    let guard = runtime_cell().lock().ok()?;
    let runtime = guard.as_ref()?;
    let rich_mutex = runtime.rich.as_ref()?;
    let mut rich = rich_mutex.lock().ok()?;
    Some(f(&mut rich))
}

pub fn command_start(command: &str, detail: &str) {
    match current_mode() {
        UiMode::Json => {}
        UiMode::Plain => println!("{}", plain_start_message(command, detail)),
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                state.ensure_spinner(command, detail);
            });
        }
    }
}

pub fn command_stage(command: &str, stage: &str, detail: &str) {
    match current_mode() {
        UiMode::Json => {}
        UiMode::Plain => println!("{}", plain_stage_message(command, stage, detail)),
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
        UiMode::Json => {}
        UiMode::Plain => println!("{}", plain_warn_message(command, message)),
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

fn plain_info_message(command: &str, message: &str) -> String {
    format!("[info] {command} {message}")
}

fn info_output_for_mode(mode: UiMode, command: &str, message: &str) -> Option<String> {
    match mode {
        UiMode::Json => None,
        UiMode::Plain => Some(plain_info_message(command, message)),
        UiMode::Rich => Some(rich_info_message(message)),
    }
}

fn rich_success_message(command: &str, detail: &str) -> String {
    format!("✔ completed {command} {detail}")
}

fn rich_failure_message(command: &str, detail: &str) -> String {
    format!("[failed] {command} {detail}")
}

fn plain_finish_message(command: &str, succeeded: bool, detail: &str) -> String {
    if succeeded {
        format!("[completed] {command} {detail}")
    } else {
        format!("[failed] {command} {detail}")
    }
}

fn emit_rich_inline_line(state: &mut RichState, command: &str, line: String) {
    if let Some(spinner) = state.spinners.get(command) {
        spinner.println(line);
    } else {
        println!("{line}");
    }
}

pub fn command_info(command: &str, message: &str) {
    let mode = current_mode();
    let Some(formatted) = info_output_for_mode(mode, command, message) else {
        return;
    };
    match mode {
        UiMode::Json => {}
        UiMode::Plain => println!("{formatted}"),
        UiMode::Rich => {
            let _ = with_rich_state(|state| {
                emit_rich_inline_line(state, command, formatted);
            });
        }
    }
}

pub fn command_upload_start(command: &str, total_bytes: u64, detail: &str) {
    match current_mode() {
        UiMode::Json => {}
        UiMode::Plain => println!(
            "{}",
            plain_upload_start_message(command, total_bytes, detail)
        ),
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
        UiMode::Json | UiMode::Plain => {}
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
        UiMode::Json | UiMode::Plain => {}
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
        UiMode::Json => {}
        UiMode::Plain => println!("{}", plain_finish_message(command, succeeded, detail)),
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
                    println!("{}", rich_success_message(command, detail));
                } else {
                    println!("{}", rich_failure_message(command, detail));
                }
            });
        }
    }
}

#[derive(Debug, Serialize)]
struct JsonCommandSummary<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    command: &'a str,
    status: &'a str,
    duration_ms: u128,
    timestamp: String,
    meta: &'a BTreeMap<String, String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonCommandError<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    command: &'a str,
    message: &'a str,
    stage: &'a str,
    code: &'a str,
}

fn now_timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

fn write_json_line<T: Serialize>(payload: &T) {
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, payload).is_ok() {
        let _ = stdout.write_all(b"\n");
    }
}

pub fn emit_command_error_json(command: &str, message: &str, stage: &str, code: &str) {
    let payload = JsonCommandError {
        line_type: "command.error",
        command,
        message,
        stage,
        code,
    };
    write_json_line(&payload);
}

pub fn finalize_result(result: &CommandResult) {
    match current_mode() {
        UiMode::Json => {
            if result.skip_json_summary {
                return;
            }
            let status = if result.exit_code == 0 {
                "completed"
            } else {
                "failed"
            };
            let payload = JsonCommandSummary {
                line_type: "command.summary",
                command: &result.command,
                status,
                duration_ms: result.duration_ms,
                timestamp: now_timestamp(),
                meta: &result.meta,
                error: result.stderr.clone(),
            };
            write_json_line(&payload);
        }
        UiMode::Plain | UiMode::Rich => {
            if result.exit_code != 0
                && let Some(message) = result.stderr.as_deref()
            {
                println!("[error] {} {}", result.command, message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_json_flag_first() {
        assert_eq!(detect_mode_from_ci(true, Some("true")), UiMode::Json);
    }

    #[test]
    fn detects_plain_when_ci_enabled() {
        assert_eq!(detect_mode_from_ci(false, Some("true")), UiMode::Plain);
        assert_eq!(detect_mode_from_ci(false, Some("1")), UiMode::Plain);
    }

    #[test]
    fn detects_rich_when_ci_is_disabled() {
        assert_eq!(detect_mode_from_ci(false, Some("false")), UiMode::Rich);
        assert_eq!(detect_mode_from_ci(false, None), UiMode::Rich);
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
    fn rich_success_message_uses_check_mark() {
        assert_eq!(
            rich_success_message("deploy", "completed"),
            "✔ completed deploy completed"
        );
    }

    #[test]
    fn info_output_for_mode_uses_expected_format() {
        let message = "cli=0.1.0 project=/tmp/x";
        assert_eq!(
            info_output_for_mode(UiMode::Plain, "deploy", message),
            Some("[info] deploy cli=0.1.0 project=/tmp/x".to_string())
        );
        assert_eq!(
            info_output_for_mode(UiMode::Rich, "deploy", message),
            Some("\u{1b}[2m  > cli=0.1.0 project=/tmp/x\u{1b}[0m".to_string())
        );
        assert_eq!(info_output_for_mode(UiMode::Json, "deploy", message), None);
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
    fn startup_banner_lines_are_not_generated_for_json() {
        assert_eq!(startup_banner_lines_for_mode(UiMode::Json, "0.1.0"), None);
    }
}
