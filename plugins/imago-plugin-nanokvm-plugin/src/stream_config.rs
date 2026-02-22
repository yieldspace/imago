use wasmtime::component::ResourceTable;

use crate::{
    common::{
        ensure_nanokvm_environment, parse_u8_file, parse_u16_file, read_file_trimmed,
        read_hardware_kind, write_file_string,
    },
    constants::{
        FPS_MAX, FPS_MIN, QUALITY_MAX, QUALITY_MIN, SERVER_CONFIG_YAML_PATH, STREAM_FPS_PATH,
        STREAM_HEIGHT_PATH, STREAM_NOW_FPS_PATH, STREAM_QUALITY_PATH, STREAM_RESOLUTION_PATH,
        STREAM_TYPE_PATH, STREAM_WIDTH_PATH,
    },
    types::{HardwareKind, Resolution, StreamHardwareVersion, StreamSettings, StreamType},
};

fn to_stream_hardware_version(kind: HardwareKind) -> StreamHardwareVersion {
    match kind {
        HardwareKind::Alpha => StreamHardwareVersion::Alpha,
        HardwareKind::Beta => StreamHardwareVersion::Beta,
        HardwareKind::Pcie => StreamHardwareVersion::Pcie,
    }
}

pub(crate) fn parse_stream_type(raw: &str) -> Result<StreamType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "mjpeg" | "mjpg" => Ok(StreamType::Mjpeg),
        "h264" => Ok(StreamType::H264),
        other => Err(format!("unknown stream type value: {other}")),
    }
}

fn stream_type_file_value(stream_type: StreamType) -> &'static str {
    match stream_type {
        StreamType::Mjpeg => "mjpeg",
        StreamType::H264 => "h264",
    }
}

fn parse_resolution(raw: &str) -> Result<Resolution, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1080" | "1080p" | "r1080p" => Ok(Resolution::R1080p),
        "720" | "720p" | "r720p" => Ok(Resolution::R720p),
        "600" | "600p" | "r600p" => Ok(Resolution::R600p),
        "480" | "480p" | "r480p" => Ok(Resolution::R480p),
        other => Err(format!("unknown resolution value: {other}")),
    }
}

fn resolution_file_value(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::R1080p => "1080",
        Resolution::R720p => "720",
        Resolution::R600p => "600",
        Resolution::R480p => "480",
    }
}

pub(crate) fn validate_fps(fps: u8) -> Result<(), String> {
    if (FPS_MIN..=FPS_MAX).contains(&fps) {
        Ok(())
    } else {
        Err(format!(
            "nanokvm fps must be in range {FPS_MIN}..={FPS_MAX}"
        ))
    }
}

pub(crate) fn validate_quality(quality: u16) -> Result<(), String> {
    if (QUALITY_MIN..=QUALITY_MAX).contains(&quality) {
        Ok(())
    } else {
        Err(format!(
            "nanokvm quality must be in range {QUALITY_MIN}..={QUALITY_MAX}"
        ))
    }
}

fn get_stream_settings() -> Result<StreamSettings, String> {
    let stream_type = parse_stream_type(&read_file_trimmed(STREAM_TYPE_PATH)?)?;
    let resolution = parse_resolution(&read_file_trimmed(STREAM_RESOLUTION_PATH)?)?;
    let width = parse_u16_file(STREAM_WIDTH_PATH)?;
    let height = parse_u16_file(STREAM_HEIGHT_PATH)?;
    let fps = parse_u8_file(STREAM_FPS_PATH)?;
    let quality = parse_u16_file(STREAM_QUALITY_PATH)?;
    let now_fps = parse_u8_file(STREAM_NOW_FPS_PATH)?;
    let hardware_version = to_stream_hardware_version(read_hardware_kind()?);

    Ok(StreamSettings {
        stream_type,
        resolution,
        width,
        height,
        fps,
        quality,
        now_fps,
        hardware_version,
    })
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::stream_config::Host for ResourceTable {
    fn get_settings(&mut self) -> Result<StreamSettings, String> {
        ensure_nanokvm_environment()?;
        get_stream_settings()
    }

    fn set_stream_type(&mut self, stream_type: StreamType) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        write_file_string(STREAM_TYPE_PATH, stream_type_file_value(stream_type))
    }

    fn set_resolution(&mut self, resolution: Resolution) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        write_file_string(STREAM_RESOLUTION_PATH, resolution_file_value(resolution))
    }

    fn set_fps(&mut self, fps: u8) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        validate_fps(fps)?;
        write_file_string(STREAM_FPS_PATH, &fps.to_string())
    }

    fn set_quality(&mut self, quality: u16) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        validate_quality(quality)?;
        write_file_string(STREAM_QUALITY_PATH, &quality.to_string())
    }

    fn get_server_config_yaml(&mut self) -> Result<String, String> {
        ensure_nanokvm_environment()?;
        std::fs::read_to_string(SERVER_CONFIG_YAML_PATH).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                crate::common::unsupported_error(format!("missing {SERVER_CONFIG_YAML_PATH}"))
            }
            _ => format!("failed to read {SERVER_CONFIG_YAML_PATH}: {err}"),
        })
    }
}
