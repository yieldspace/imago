use wasmtime::component::ResourceTable;

use crate::{
    common::{ensure_nanokvm_environment, unsupported_error},
    constants::{
        RUNTIME_SCRIPT_PATH, RUNTIME_SCRIPT_STOP_PING_DISABLED, RUNTIME_SCRIPT_STOP_PING_ENABLED,
        RUNTIME_SCRIPT_WATCHDOG_DISABLED, RUNTIME_SCRIPT_WATCHDOG_ENABLED,
    },
    types::ToggleState,
};

fn has_line_prefix(content: &str, prefix: &str) -> bool {
    content
        .lines()
        .map(str::trim_start)
        .any(|line| line.starts_with(prefix))
}

pub(crate) fn parse_watchdog_toggle(content: &str) -> Result<ToggleState, String> {
    if has_line_prefix(content, "while true ;") {
        return Ok(ToggleState::Enabled);
    }
    if has_line_prefix(content, "#while true ;") {
        return Ok(ToggleState::Disabled);
    }
    Err("unknown watchdog state in /etc/init.d/S95nanokvm".to_string())
}

pub(crate) fn parse_stop_ping_toggle(content: &str) -> Result<ToggleState, String> {
    if has_line_prefix(content, "(sleep 5;touch /tmp/stop") {
        return Ok(ToggleState::Enabled);
    }
    if has_line_prefix(content, "#(sleep 5;touch /tmp/stop") {
        return Ok(ToggleState::Disabled);
    }
    Err("unknown stop-ping state in /etc/init.d/S95nanokvm".to_string())
}

fn copy_script_file(source: &str, target: &str) -> Result<(), String> {
    let copied =
        std::fs::copy(source, target).map_err(|err| format!("failed to copy {source}: {err}"))?;
    if copied == 0 {
        return Err(format!("failed to copy {source}: copied zero bytes"));
    }
    Ok(())
}

pub(crate) fn watchdog_script_source(state: ToggleState) -> &'static str {
    match state {
        ToggleState::Enabled => RUNTIME_SCRIPT_WATCHDOG_ENABLED,
        ToggleState::Disabled => RUNTIME_SCRIPT_WATCHDOG_DISABLED,
    }
}

pub(crate) fn stop_ping_script_source(state: ToggleState) -> &'static str {
    match state {
        ToggleState::Enabled => RUNTIME_SCRIPT_STOP_PING_ENABLED,
        ToggleState::Disabled => RUNTIME_SCRIPT_STOP_PING_DISABLED,
    }
}

fn read_runtime_script() -> Result<String, String> {
    std::fs::read_to_string(RUNTIME_SCRIPT_PATH).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => unsupported_error(format!("missing {RUNTIME_SCRIPT_PATH}")),
        _ => format!("failed to read {RUNTIME_SCRIPT_PATH}: {err}"),
    })
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::runtime_control::Host for ResourceTable {
    fn get_watchdog(&mut self) -> Result<ToggleState, String> {
        ensure_nanokvm_environment()?;
        parse_watchdog_toggle(&read_runtime_script()?)
    }

    fn set_watchdog(&mut self, state: ToggleState) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        copy_script_file(watchdog_script_source(state), RUNTIME_SCRIPT_PATH)
    }

    fn get_stop_ping(&mut self) -> Result<ToggleState, String> {
        ensure_nanokvm_environment()?;
        parse_stop_ping_toggle(&read_runtime_script()?)
    }

    fn set_stop_ping(&mut self, state: ToggleState) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        copy_script_file(stop_ping_script_source(state), RUNTIME_SCRIPT_PATH)
    }
}
