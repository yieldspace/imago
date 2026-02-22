use std::{fs, path::Path};

use crate::{
    constants::{HW_VERSION_PATH, NANOKVM_ETC, NANOKVM_ROOT},
    types::HardwareKind,
};

pub(crate) fn unsupported_error(message: impl AsRef<str>) -> String {
    format!(
        "unsupported: nanokvm local environment is required ({})",
        message.as_ref()
    )
}

pub(crate) fn ensure_nanokvm_environment() -> Result<(), String> {
    for path in [NANOKVM_ROOT, NANOKVM_ETC, HW_VERSION_PATH] {
        if !Path::new(path).exists() {
            return Err(unsupported_error(format!("missing {path}")));
        }
    }
    Ok(())
}

pub(crate) fn read_file_trimmed(path: &str) -> Result<String, String> {
    let content = fs::read_to_string(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => unsupported_error(format!("missing {path}")),
        _ => format!("failed to read {path}: {err}"),
    })?;
    Ok(content.trim().to_string())
}

pub(crate) fn write_file_string(path: &str, value: &str) -> Result<(), String> {
    fs::write(path, value.as_bytes()).map_err(|err| format!("failed to write {path}: {err}"))
}

pub(crate) fn parse_hardware_kind(raw: &str) -> Result<HardwareKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "alpha" => Ok(HardwareKind::Alpha),
        "beta" => Ok(HardwareKind::Beta),
        "pcie" => Ok(HardwareKind::Pcie),
        other => Err(format!("unknown hardware version value: {other}")),
    }
}

pub(crate) fn read_hardware_kind() -> Result<HardwareKind, String> {
    let raw = read_file_trimmed(HW_VERSION_PATH)?;
    parse_hardware_kind(&raw)
}

pub(crate) fn parse_u8_file(path: &str) -> Result<u8, String> {
    let raw = read_file_trimmed(path)?;
    raw.parse::<u8>()
        .map_err(|err| format!("invalid numeric value in {path}: {err}"))
}

pub(crate) fn parse_u16_file(path: &str) -> Result<u16, String> {
    let raw = read_file_trimmed(path)?;
    raw.parse::<u16>()
        .map_err(|err| format!("invalid numeric value in {path}: {err}"))
}

pub(crate) fn format_host_for_url(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}
