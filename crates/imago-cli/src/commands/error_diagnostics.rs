use anyhow::Error;

const HINT_UNAUTHORIZED: &str =
    "Verify target.client_key, ~/.imago/known_hosts, and server_name/remote settings, then retry.";
const HINT_BUILD_FAILED: &str =
    "Run `imago build` first and fix build.command errors before retrying deploy.";
const HINT_TARGET_CONFIG: &str =
    "Check `imago.toml` target settings (remote, server_name, client_key) and fix invalid values.";
const HINT_REMOTE_PARSE: &str =
    "Fix the target remote format. Use a valid host:port or URL accepted by the command.";
const HINT_TRANSPORT_CONNECT: &str =
    "Check target reachability/TLS settings, then retry the QUIC/WebTransport connection.";
const HINT_BUSY: &str = "The target is busy. Wait for in-flight operations to finish and retry.";
const HINT_STORAGE_QUOTA: &str =
    "The target reported storage quota exhaustion. Free disk space or increase quota.";
const HINT_PRECONDITION_FAILED: &str =
    "A precondition failed on the target. Refresh state and retry with up-to-date inputs.";

pub fn format_command_error(command: &str, err: &Error) -> String {
    let summary = err.to_string();

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
        hints.push(format!(
            "Inspect the causes above and retry `{command}` after fixing the root issue."
        ));
    }

    let mut formatted = String::new();
    formatted.push_str(&summary);
    formatted.push_str("\ncauses:");
    for cause in causes {
        formatted.push_str("\n- ");
        formatted.push_str(&cause);
    }
    formatted.push_str("\nhints:");
    for hint in hints {
        formatted.push_str("\n- ");
        formatted.push_str(&hint);
    }
    formatted
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

    if combined.contains("E_STORAGE_QUOTA") || combined.contains("StorageQuota") {
        push_unique(hints, HINT_STORAGE_QUOTA);
    }

    if combined.contains("E_PRECONDITION_FAILED") || combined.contains("PreconditionFailed") {
        push_unique(hints, HINT_PRECONDITION_FAILED);
    }
}

fn push_unique(hints: &mut Vec<String>, hint: &str) {
    if hints.iter().any(|existing| existing == hint) {
        return;
    }
    hints.push(hint.to_string());
}

#[cfg(test)]
mod tests {
    use super::format_command_error;
    use anyhow::anyhow;

    #[test]
    fn formats_summary_causes_and_hints_sections() {
        let err = anyhow!("simple failure");
        let formatted = format_command_error("build", &err);

        assert!(formatted.starts_with("simple failure\ncauses:\n- "));
        assert!(formatted.contains("\nhints:\n- "));
    }

    #[test]
    fn includes_multiple_causes_from_error_chain() {
        let err = anyhow!("root cause")
            .context("middle cause")
            .context("top summary");

        let formatted = format_command_error("deploy", &err);
        assert!(formatted.starts_with("top summary"));
        assert!(formatted.contains("- middle cause"));
        assert!(formatted.contains("- root cause"));
    }

    #[test]
    fn includes_unauthorized_hint_when_error_code_exists() {
        let err = anyhow!("server error: auth failed (E_UNAUTHORIZED) at transport.connect");

        let formatted = format_command_error("run", &err);
        assert!(formatted.contains("target.client_key"));
        assert!(formatted.contains("known_hosts"));
    }

    #[test]
    fn includes_unauthorized_hint_for_plain_unauthorized_text() {
        let err = anyhow!("request rejected: Unauthorized at transport.connect");

        let formatted = format_command_error("run", &err);
        assert!(formatted.contains("target.client_key"));
        assert!(formatted.contains("known_hosts"));
    }

    #[test]
    fn includes_fallback_hint_when_no_rule_matches() {
        let err = anyhow!("unexpected checksum mismatch in local cache");

        let formatted = format_command_error("stop", &err);
        assert!(
            formatted
                .contains("Inspect the causes above and retry `stop` after fixing the root issue."),
        );
    }
}
