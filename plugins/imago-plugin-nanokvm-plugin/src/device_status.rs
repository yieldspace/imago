use wasmtime::component::ResourceTable;

use crate::{
    common::{ensure_nanokvm_environment, read_file_trimmed, read_hardware_kind},
    constants::{
        ETHERNET_OPERSTATE_PATH, HDD_LED_GPIO_PATH, HDMI_STATE_PATH, POWER_LED_GPIO_PATH,
        USB_MODE_FLAG_PATH, WIFI_OPERSTATE_PATH, WIFI_SUPPORTED_FILE,
    },
    types::{FeatureStatus, HardwareKind, HdmiStatus, LedStates, LedStatus, LinkStatus, UsbMode},
};

pub(crate) fn parse_usb_mode(raw: &str) -> Result<UsbMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "0x0510" => Ok(UsbMode::Normal),
        "0x0623" => Ok(UsbMode::HidOnly),
        other => Err(format!("unknown usb mode value: {other}")),
    }
}

pub(crate) fn parse_hdmi_status(raw: &str) -> Result<HdmiStatus, String> {
    match raw.trim() {
        "1" => Ok(HdmiStatus::Normal),
        "0" => Ok(HdmiStatus::Abnormal),
        other => Err(format!("unknown hdmi status value: {other}")),
    }
}

pub(crate) fn parse_link_status(raw: &str, label: &str) -> Result<LinkStatus, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "up" => Ok(LinkStatus::Connected),
        "down" => Ok(LinkStatus::Disconnected),
        other => Err(format!("unknown {label} link value: {other}")),
    }
}

fn parse_led_status(raw: &str, label: &str) -> Result<LedStatus, String> {
    match raw.trim() {
        "0" => Ok(LedStatus::On),
        "1" => Ok(LedStatus::Off),
        other => Err(format!("unknown {label} led value: {other}")),
    }
}

fn read_usb_mode() -> Result<UsbMode, String> {
    parse_usb_mode(&read_file_trimmed(USB_MODE_FLAG_PATH)?)
}

fn read_hdmi_status() -> Result<HdmiStatus, String> {
    parse_hdmi_status(&read_file_trimmed(HDMI_STATE_PATH)?)
}

fn read_network_link_status(path: &str, label: &str) -> Result<LinkStatus, String> {
    parse_link_status(&read_file_trimmed(path)?, label)
}

fn read_wifi_supported() -> Result<FeatureStatus, String> {
    match std::fs::metadata(WIFI_SUPPORTED_FILE) {
        Ok(_) => Ok(FeatureStatus::Enabled),
        Err(err) => match err.kind() {
            std::io::ErrorKind::NotFound => Ok(FeatureStatus::Disabled),
            _ => Err(format!("failed to read wifi supported marker: {err}")),
        },
    }
}

fn read_led_states() -> Result<LedStates, String> {
    let hardware_kind = read_hardware_kind()?;
    let power = parse_led_status(&read_file_trimmed(POWER_LED_GPIO_PATH)?, "power")?;
    let hdd = match hardware_kind {
        HardwareKind::Alpha => Some(parse_led_status(
            &read_file_trimmed(HDD_LED_GPIO_PATH)?,
            "hdd",
        )?),
        HardwareKind::Beta | HardwareKind::Pcie => None,
    };

    Ok(LedStates { power, hdd })
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::Host for ResourceTable {
    fn get_usb_mode(&mut self) -> Result<UsbMode, String> {
        ensure_nanokvm_environment()?;
        read_usb_mode()
    }

    fn get_hdmi_status(&mut self) -> Result<HdmiStatus, String> {
        ensure_nanokvm_environment()?;
        read_hdmi_status()
    }

    fn get_ethernet_status(&mut self) -> Result<LinkStatus, String> {
        ensure_nanokvm_environment()?;
        read_network_link_status(ETHERNET_OPERSTATE_PATH, "ethernet")
    }

    fn get_wifi_status(&mut self) -> Result<LinkStatus, String> {
        ensure_nanokvm_environment()?;
        read_network_link_status(WIFI_OPERSTATE_PATH, "wifi")
    }

    fn get_wifi_supported(&mut self) -> Result<FeatureStatus, String> {
        ensure_nanokvm_environment()?;
        read_wifi_supported()
    }

    fn get_led_states(&mut self) -> Result<LedStates, String> {
        ensure_nanokvm_environment()?;
        read_led_states()
    }
}
