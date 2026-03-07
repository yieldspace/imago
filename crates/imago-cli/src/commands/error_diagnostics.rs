use anyhow::Error;
use std::borrow::Cow;

const HINT_UNAUTHORIZED: &str = "For direct targets, verify target.client_key, ~/.imago/known_hosts, and server_name/remote. For ssh targets, verify remote and SSH access, then retry.";
const HINT_BUILD_FAILED: &str =
    "Run `imago artifact build` first and fix build.command errors before retrying service deploy.";
const HINT_TARGET_CONFIG: &str = "Check `imago.toml` target settings. Direct targets use remote/server_name/client_key; ssh targets use only remote.";
const HINT_REMOTE_PARSE: &str = "Fix the target remote format. Use host:port for direct targets or ssh://user@host[?socket=/path] for SSH targets.";
const HINT_TRANSPORT_CONNECT: &str =
    "Check target reachability/TLS settings, then retry the QUIC/WebTransport connection.";
const HINT_BUSY: &str = "The target is busy. Wait for in-flight operations to finish and retry.";
const HINT_COMMAND_START_STREAM_INTERRUPTED: &str = "The command.start stream was interrupted. The command may still be running on target; inspect target state/logs before retrying service deploy/service start/service stop.";
const HINT_STORAGE_QUOTA: &str =
    "The target reported storage quota exhaustion. Free disk space or increase quota.";
const HINT_PRECONDITION_FAILED: &str =
    "A precondition failed on the target. Refresh state and retry with up-to-date inputs.";

pub fn format_command_error(command: &str, err: &Error) -> String {
    let summary = normalize_error_summary(command, &err.to_string());

    let mut causes: Vec<String> = err.chain().map(|cause| cause.to_string()).collect();
    if causes.first().is_some_and(|head| head == &summary) {
        causes.remove(0);
    }
    if causes.is_empty() {
        causes.push(summary.clone());
    }

    let mut hints = Vec::new();
    append_hints(err, &mut hints);
    if hints.is_empty() {
        let retry_command = retry_command_hint(command);
        hints.push(format!(
            "Inspect the causes above and retry `{retry_command}` after fixing the root issue."
        ));
    }

    let mut formatted = String::new();
    formatted.push_str("error: ");
    formatted.push_str(&summary);
    formatted.push_str("\ncaused by:");
    for cause in causes {
        formatted.push_str("\n  - ");
        formatted.push_str(&cause);
    }
    formatted.push_str("\nhint:");
    for hint in hints {
        formatted.push_str("\n  - ");
        formatted.push_str(&hint);
    }
    formatted
}

fn retry_command_hint(command: &str) -> Cow<'_, str> {
    match command {
        "project.init" => Cow::Borrowed("imago project init"),
        "artifact.build" => Cow::Borrowed("imago artifact build"),
        "deps.sync" => Cow::Borrowed("imago deps sync"),
        "service.deploy" => Cow::Borrowed("imago service deploy"),
        "service.start" => Cow::Borrowed("imago service start"),
        "service.stop" => Cow::Borrowed("imago service stop"),
        "service.ls" => Cow::Borrowed("imago service ls"),
        "service.logs" => Cow::Borrowed("imago service logs"),
        "stack" => Cow::Borrowed("imago stack <subcommand>"),
        "trust.cert.upload" => Cow::Borrowed("imago trust cert upload"),
        "trust.cert.replicate" => Cow::Borrowed("imago trust cert replicate"),
        "trust.client-key.generate" => Cow::Borrowed("imago trust client-key generate"),
        _ => Cow::Borrowed(command),
    }
}

pub fn summarize_command_failure(_command: &str, err: &Error) -> String {
    let chain_messages: Vec<String> = err.chain().map(|cause| cause.to_string()).collect();
    let combined = chain_messages.join("\n");
    let combined_lower = combined.to_ascii_lowercase();

    if combined_lower.contains("build stage failed")
        || combined_lower.contains("failed to run build before deploy")
        || combined_lower.contains("build.command failed")
    {
        return "build stage failed".to_string();
    }

    if combined_lower.contains("failed to load target configuration")
        || combined_lower.contains("target settings are invalid")
    {
        return "load-config stage failed".to_string();
    }

    if combined_lower.contains("failed to establish quic")
        || combined_lower.contains("failed to start quic")
        || combined_lower.contains("failed to establish webtransport")
        || combined_lower.contains("connect failed")
    {
        return "connect stage failed".to_string();
    }

    if combined_lower.contains("hello.negotiate")
        || (combined_lower.contains("hello") && combined_lower.contains("negotiate"))
    {
        return "hello stage failed".to_string();
    }

    if combined_lower.contains("command.start") {
        return "command.start stage failed".to_string();
    }

    if combined_lower.contains("logs.request") {
        return "logs.request stage failed".to_string();
    }

    if combined_lower.contains("services.list") {
        return "services.list stage failed".to_string();
    }

    "operation failed".to_string()
}

fn append_hints(err: &Error, hints: &mut Vec<String>) {
    let chain_messages: Vec<String> = err.chain().map(|cause| cause.to_string()).collect();
    let combined = chain_messages.join("\n");
    let combined_lower = combined.to_ascii_lowercase();

    if combined.contains("E_UNAUTHORIZED")
        || combined_lower.contains("unauthorized")
        || combined_lower.contains("public key authentication failed")
    {
        push_unique(hints, HINT_UNAUTHORIZED);
    }

    if combined_lower.contains("failed to run build before deploy")
        || combined_lower.contains("build.command failed")
    {
        push_unique(hints, HINT_BUILD_FAILED);
    }

    if combined_lower.contains("failed to load target configuration")
        || combined_lower.contains("target settings are invalid")
    {
        push_unique(hints, HINT_TARGET_CONFIG);
    }

    let looks_like_remote_parse_error = (combined_lower.contains("remote")
        && (combined_lower.contains("parse")
            || combined_lower.contains("invalid")
            || combined_lower.contains("socket")))
        || combined_lower.contains("invalid socket address")
        || combined_lower.contains("relative url without a base")
        || combined_lower.contains("empty host");
    if looks_like_remote_parse_error {
        push_unique(hints, HINT_REMOTE_PARSE);
    }

    if combined_lower.contains("failed to establish quic")
        || combined_lower.contains("failed to start quic")
        || combined_lower.contains("failed to establish webtransport")
    {
        push_unique(hints, HINT_TRANSPORT_CONNECT);
    }

    if combined.contains("E_BUSY") || combined.contains("Busy") {
        push_unique(hints, HINT_BUSY);
    }

    let looks_like_command_start_stream_interrupted = combined_lower
        .contains("command.start request stream failed")
        || combined_lower.contains("command may still be running")
        || (combined_lower.contains("command.start")
            && combined_lower.contains("request stream")
            && (combined_lower.contains("timed out")
                || combined_lower.contains("connection")
                || combined_lower.contains("closed")
                || combined_lower.contains("reset")));
    if looks_like_command_start_stream_interrupted {
        push_unique(hints, HINT_COMMAND_START_STREAM_INTERRUPTED);
    }

    if combined.contains("E_STORAGE_QUOTA") || combined.contains("StorageQuota") {
        push_unique(hints, HINT_STORAGE_QUOTA);
    }

    if combined.contains("E_PRECONDITION_FAILED") || combined.contains("PreconditionFailed") {
        push_unique(hints, HINT_PRECONDITION_FAILED);
    }
}

fn normalize_error_summary(command: &str, summary: &str) -> String {
    let prefix_with_colon = format!("{command} failed: ");
    if let Some(stripped) = summary.strip_prefix(&prefix_with_colon) {
        return stripped.trim().to_string();
    }

    let prefix_without_colon = format!("{command} failed ");
    if let Some(stripped) = summary.strip_prefix(&prefix_without_colon) {
        return stripped.trim().to_string();
    }

    summary.to_string()
}

fn push_unique(hints: &mut Vec<String>, hint: &str) {
    if hints.iter().any(|existing| existing == hint) {
        return;
    }
    hints.push(hint.to_string());
}

#[cfg(test)]
mod tests {
    use super::{format_command_error, summarize_command_failure};
    use anyhow::anyhow;

    #[test]
    fn formats_summary_causes_and_hints_sections() {
        let err = anyhow!("simple failure");
        let formatted = format_command_error("artifact.build", &err);

        assert!(formatted.starts_with("error: simple failure\ncaused by:\n  - "));
        assert!(formatted.contains("\nhint:\n  - "));
    }

    #[test]
    fn includes_multiple_causes_from_error_chain() {
        let err = anyhow!("root cause")
            .context("middle cause")
            .context("top summary");

        let formatted = format_command_error("service.deploy", &err);
        assert!(formatted.starts_with("error: top summary"));
        assert!(formatted.contains("  - middle cause"));
        assert!(formatted.contains("  - root cause"));
    }

    #[test]
    fn includes_unauthorized_hint_when_error_code_exists() {
        let err = anyhow!("server error: auth failed (E_UNAUTHORIZED) at transport.connect");

        let formatted = format_command_error("service.start", &err);
        assert!(formatted.contains("target.client_key"));
        assert!(formatted.contains("known_hosts"));
    }

    #[test]
    fn includes_unauthorized_hint_for_plain_unauthorized_text() {
        let err = anyhow!("request rejected: Unauthorized at transport.connect");

        let formatted = format_command_error("service.start", &err);
        assert!(formatted.contains("target.client_key"));
        assert!(formatted.contains("known_hosts"));
    }

    #[test]
    fn includes_fallback_hint_when_no_rule_matches() {
        let err = anyhow!("unexpected checksum mismatch in local cache");

        let formatted = format_command_error("service.stop", &err);
        assert!(formatted.contains(
            "Inspect the causes above and retry `imago service stop` after fixing the root issue.",
        ),);
    }

    #[test]
    fn keeps_unknown_command_in_fallback_hint() {
        let err = anyhow!("unexpected failure");

        let formatted = format_command_error("custom.command", &err);
        assert!(formatted.contains(
            "Inspect the causes above and retry `custom.command` after fixing the root issue.",
        ));
    }

    #[test]
    fn includes_command_start_stream_interrupted_hint() {
        let err = anyhow!("request stream read timed out after 15000 ms")
            .context("command.start request stream failed; command may still be running on target");

        let formatted = format_command_error("service.deploy", &err);
        assert!(formatted.contains("command.start stream was interrupted"));
        assert!(formatted.contains("may still be running on target"));
    }

    #[test]
    fn strips_redundant_command_prefix_from_summary() {
        let err = anyhow!("root").context("service.deploy failed: root");
        let formatted = format_command_error("service.deploy", &err);
        assert!(formatted.starts_with("error: root"));
    }

    #[test]
    fn summarize_command_failure_reports_build_stage() {
        let err = anyhow!("build.command failed with exit code 1");
        assert_eq!(
            summarize_command_failure("service.deploy", &err),
            "build stage failed"
        );
    }

    #[test]
    fn summarize_command_failure_reports_hello_stage() {
        let err = anyhow!("hello.negotiate was rejected by server");
        assert_eq!(
            summarize_command_failure("service.start", &err),
            "hello stage failed"
        );
    }

    #[test]
    fn summarize_command_failure_uses_fallback() {
        let err = anyhow!("unexpected failure");
        assert_eq!(
            summarize_command_failure("service.start", &err),
            "operation failed"
        );
    }
}
