use std::{thread, time::Duration};

use wasmtime::component::ResourceTable;

use crate::{
    common::{ensure_nanokvm_environment, read_hardware_kind, write_file_string},
    constants::{
        GPIO_POWER_ALPHA_BETA_PCIE, GPIO_PULSE_DEFAULT_MS, GPIO_RESET_ALPHA, GPIO_RESET_BETA_PCIE,
    },
    types::{GpioPulseKind, HardwareKind},
};

pub(crate) fn gpio_path_for(kind: HardwareKind, pulse_kind: GpioPulseKind) -> &'static str {
    match (kind, pulse_kind) {
        (HardwareKind::Alpha, GpioPulseKind::Power) => GPIO_POWER_ALPHA_BETA_PCIE,
        (HardwareKind::Alpha, GpioPulseKind::Reset) => GPIO_RESET_ALPHA,
        (HardwareKind::Beta, GpioPulseKind::Power) => GPIO_POWER_ALPHA_BETA_PCIE,
        (HardwareKind::Beta, GpioPulseKind::Reset) => GPIO_RESET_BETA_PCIE,
        (HardwareKind::Pcie, GpioPulseKind::Power) => GPIO_POWER_ALPHA_BETA_PCIE,
        (HardwareKind::Pcie, GpioPulseKind::Reset) => GPIO_RESET_BETA_PCIE,
    }
}

pub(crate) fn pulse_duration_ms(duration_ms: Option<u32>) -> u32 {
    duration_ms
        .filter(|duration| *duration > 0)
        .unwrap_or(GPIO_PULSE_DEFAULT_MS)
}

fn pulse_gpio(path: &str, duration_ms: Option<u32>) -> Result<(), String> {
    write_file_string(path, "1")?;
    thread::sleep(Duration::from_millis(pulse_duration_ms(duration_ms).into()));
    write_file_string(path, "0")
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::io_control::Host for ResourceTable {
    fn power_pulse(&mut self, duration_ms: Option<u32>) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let path = gpio_path_for(read_hardware_kind()?, GpioPulseKind::Power);
        pulse_gpio(path, duration_ms)
    }

    fn reset_pulse(&mut self, duration_ms: Option<u32>) -> Result<(), String> {
        ensure_nanokvm_environment()?;
        let path = gpio_path_for(read_hardware_kind()?, GpioPulseKind::Reset);
        pulse_gpio(path, duration_ms)
    }
}
