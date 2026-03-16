use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::WasiState;
use imagod_runtime_wasmtime::native_plugins::{
    HasSelf, NativePlugin, NativePluginLinker, NativePluginResult, map_native_plugin_linker_error,
    map_native_plugin_resource_validation_error,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::WasiView;
use wasmtime_wasi::p2::{DynPollable, Pollable, subscribe};

pub mod imago_experimental_gpio_plugin_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "host",
        imports: {
            default: async,
        },
    });
}

#[derive(Debug, Default)]
#[imago_native_plugin(
    wit = "wit",
    world = "host",
    descriptor_only = true,
    multi_imports = true,
    allow_non_resource_types = true,
    generate_bindings = false
)]
pub struct ImagoExperimentalGpioPlugin;

impl NativePlugin for ImagoExperimentalGpioPlugin {
    fn package_name(&self) -> &'static str {
        Self::PACKAGE_NAME
    }

    fn supports_import(&self, import_name: &str) -> bool {
        Self::IMPORTS.contains(&import_name)
    }

    fn symbols(&self) -> &'static [&'static str] {
        Self::SYMBOLS
    }

    fn supports_symbol(&self, symbol: &str) -> bool {
        Self::IMPORTS.iter().any(|import_name| {
            symbol
                .strip_prefix(import_name)
                .is_some_and(|tail| tail.starts_with('.'))
        })
    }

    fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()> {
        imago_experimental_gpio_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(
            linker,
            |state| state,
        )
        .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
    }

    fn validate_resources(
        &self,
        resources: &BTreeMap<String, JsonValue>,
    ) -> NativePluginResult<()> {
        parse_digital_pin_catalog(resources)
            .map(|_| ())
            .map_err(|err| {
                let message = match err {
                    GpioError::Other(message) => message,
                    other => format!("{other:?}"),
                };
                map_native_plugin_resource_validation_error(Self::PACKAGE_NAME, message)
            })
    }
}

type GpioError =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::general::GpioError;
type ActiveLevel =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::general::ActiveLevel;
type PinMode = imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::general::PinMode;
type PullResistor =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::general::PullResistor;
type DigitalFlags =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::DigitalFlags;
type AnalogFlags =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::AnalogFlags;
type PinState =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::PinState;
type PollableResource = imago_experimental_gpio_plugin_bindings::wasi::io::poll::Pollable;
type DigitalConfig =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::DigitalConfig;
type AnalogConfig =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::AnalogConfig;
type DigitalInResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::DigitalInPin;
type DigitalOutResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::DigitalOutPin;
type DigitalInOutResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::DigitalInOutPin;
type AnalogInResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::AnalogInPin;
type AnalogOutResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::AnalogOutPin;
type AnalogInOutResource =
    imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::AnalogInOutPin;

const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(20);

const GPIO_RESOURCE_KEY: &str = "gpio";
const GPIO_RESOURCE_DIGITAL_PINS_KEY: &str = "digital_pins";
const GPIO_SYSFS_BASE_PATH: &str = "/sys/class/gpio";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DigitalPinSpec {
    label: String,
    aliases: Vec<String>,
    value_path: String,
    supports_input: bool,
    supports_output: bool,
    default_active_level: ActiveLevel,
    allow_pull_resistor: bool,
}

#[derive(Debug, Clone, Copy)]
struct AnalogPinSpec {
    label: &'static str,
    read_raw_path: Option<&'static str>,
    write_raw_path: Option<&'static str>,
    max_raw: u32,
    default_active_level: ActiveLevel,
}

const ANALOG_PIN_CATALOG: &[AnalogPinSpec] = &[
    AnalogPinSpec {
        label: "ADC0",
        read_raw_path: Some("/sys/bus/iio/devices/iio:device0/in_voltage0_raw"),
        write_raw_path: None,
        max_raw: 4095,
        default_active_level: ActiveLevel::ActiveHigh,
    },
    AnalogPinSpec {
        label: "PWM0",
        read_raw_path: None,
        write_raw_path: Some("/sys/class/pwm/pwmchip0/pwm0/duty_cycle"),
        max_raw: 1_000_000,
        default_active_level: ActiveLevel::ActiveHigh,
    },
    AnalogPinSpec {
        label: "DAC0",
        read_raw_path: Some("/sys/bus/iio/devices/iio:device0/out_voltage0_raw"),
        write_raw_path: Some("/sys/bus/iio/devices/iio:device0/out_voltage0_raw"),
        max_raw: 4095,
        default_active_level: ActiveLevel::ActiveHigh,
    },
];

#[derive(Debug, Clone)]
struct DigitalPinHandle {
    label: String,
    acquire_key: String,
    value_path: String,
    mode: PinMode,
    active_level: ActiveLevel,
    pull_resistor: Option<PullResistor>,
}

#[derive(Debug, Clone)]
struct AnalogPinHandle {
    label: String,
    acquire_key: String,
    read_raw_path: Option<String>,
    write_raw_path: Option<String>,
    mode: PinMode,
    active_level: ActiveLevel,
    max_raw: u32,
}

static ACQUIRED_PIN_MODES: OnceLock<Mutex<BTreeMap<String, PinMode>>> = OnceLock::new();

static NEXT_DIGITAL_IN_REP: AtomicU32 = AtomicU32::new(1);
static NEXT_DIGITAL_OUT_REP: AtomicU32 = AtomicU32::new(1);
static NEXT_DIGITAL_IN_OUT_REP: AtomicU32 = AtomicU32::new(1);
static NEXT_ANALOG_IN_REP: AtomicU32 = AtomicU32::new(1);
static NEXT_ANALOG_OUT_REP: AtomicU32 = AtomicU32::new(1);
static NEXT_ANALOG_IN_OUT_REP: AtomicU32 = AtomicU32::new(1);

static DIGITAL_IN_REGISTRY: OnceLock<Mutex<BTreeMap<u32, DigitalPinHandle>>> = OnceLock::new();
static DIGITAL_OUT_REGISTRY: OnceLock<Mutex<BTreeMap<u32, DigitalPinHandle>>> = OnceLock::new();
static DIGITAL_IN_OUT_REGISTRY: OnceLock<Mutex<BTreeMap<u32, DigitalPinHandle>>> = OnceLock::new();
static ANALOG_IN_REGISTRY: OnceLock<Mutex<BTreeMap<u32, AnalogPinHandle>>> = OnceLock::new();
static ANALOG_OUT_REGISTRY: OnceLock<Mutex<BTreeMap<u32, AnalogPinHandle>>> = OnceLock::new();
static ANALOG_IN_OUT_REGISTRY: OnceLock<Mutex<BTreeMap<u32, AnalogPinHandle>>> = OnceLock::new();
static DIGITAL_PIN_CATALOG_CACHE: OnceLock<Mutex<BTreeMap<String, Vec<DigitalPinSpec>>>> =
    OnceLock::new();

fn acquired_pin_modes() -> &'static Mutex<BTreeMap<String, PinMode>> {
    ACQUIRED_PIN_MODES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn digital_in_registry() -> &'static Mutex<BTreeMap<u32, DigitalPinHandle>> {
    DIGITAL_IN_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn digital_out_registry() -> &'static Mutex<BTreeMap<u32, DigitalPinHandle>> {
    DIGITAL_OUT_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn digital_in_out_registry() -> &'static Mutex<BTreeMap<u32, DigitalPinHandle>> {
    DIGITAL_IN_OUT_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn analog_in_registry() -> &'static Mutex<BTreeMap<u32, AnalogPinHandle>> {
    ANALOG_IN_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn analog_out_registry() -> &'static Mutex<BTreeMap<u32, AnalogPinHandle>> {
    ANALOG_OUT_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn analog_in_out_registry() -> &'static Mutex<BTreeMap<u32, AnalogPinHandle>> {
    ANALOG_IN_OUT_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn digital_pin_catalog_cache() -> &'static Mutex<BTreeMap<String, Vec<DigitalPinSpec>>> {
    DIGITAL_PIN_CATALOG_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn clear_digital_pin_catalog_cache_for_tests() {
    if let Ok(mut guard) = digital_pin_catalog_cache().lock() {
        guard.clear();
    }
}

fn register_handle<T: Clone>(
    registry: &'static Mutex<BTreeMap<u32, T>>,
    next_rep: &AtomicU32,
    handle: T,
) -> u32 {
    loop {
        let rep = next_rep.fetch_add(1, Ordering::Relaxed);
        if rep == 0 {
            continue;
        }

        let mut guard = registry
            .lock()
            .expect("resource registry lock should not be poisoned");
        if guard.insert(rep, handle.clone()).is_none() {
            return rep;
        }
    }
}

fn lookup_handle<T: Clone>(
    registry: &'static Mutex<BTreeMap<u32, T>>,
    rep: u32,
) -> Result<T, GpioError> {
    registry
        .lock()
        .map_err(|_| GpioError::Other("resource registry lock poisoned".to_string()))?
        .get(&rep)
        .cloned()
        .ok_or_else(|| GpioError::Other(format!("resource not found: rep={rep}")))
}

fn remove_handle<T>(registry: &'static Mutex<BTreeMap<u32, T>>, rep: u32) -> Option<T> {
    registry.lock().ok()?.remove(&rep)
}

fn ensure_gpio_supported() -> Result<(), GpioError> {
    #[cfg(target_os = "linux")]
    {
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(GpioError::OperationNotSupported)
    }
}

fn map_join_error(err: tokio::task::JoinError) -> GpioError {
    GpioError::Other(format!("blocking task failed: {err}"))
}

async fn run_blocking_gpio<T, F>(operation: F) -> Result<T, GpioError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, GpioError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(map_join_error)?
}

fn parse_required_string(
    object: &JsonMap<String, JsonValue>,
    field_name: &str,
    field_path: &str,
) -> Result<String, GpioError> {
    let value = object
        .get(field_name)
        .ok_or_else(|| GpioError::Other(format!("{field_path} is required")))?;
    let value = value
        .as_str()
        .ok_or_else(|| GpioError::Other(format!("{field_path} must be a string")))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(GpioError::Other(format!("{field_path} must not be empty")));
    }
    Ok(value.to_string())
}

fn parse_required_bool(
    object: &JsonMap<String, JsonValue>,
    field_name: &str,
    field_path: &str,
) -> Result<bool, GpioError> {
    let value = object
        .get(field_name)
        .ok_or_else(|| GpioError::Other(format!("{field_path} is required")))?;
    value
        .as_bool()
        .ok_or_else(|| GpioError::Other(format!("{field_path} must be a boolean")))
}

fn parse_optional_string_array(
    object: &JsonMap<String, JsonValue>,
    field_name: &str,
    field_path: &str,
) -> Result<Vec<String>, GpioError> {
    let Some(value) = object.get(field_name) else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| GpioError::Other(format!("{field_path} must be an array of strings")))?;
    let mut values = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let item_path = format!("{field_path}[{index}]");
        let value = value
            .as_str()
            .ok_or_else(|| GpioError::Other(format!("{item_path} must be a string")))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(GpioError::Other(format!("{item_path} must not be empty")));
        }
        values.push(value.to_string());
    }
    Ok(values)
}

fn parse_required_active_level(
    object: &JsonMap<String, JsonValue>,
    field_name: &str,
    field_path: &str,
) -> Result<ActiveLevel, GpioError> {
    let raw = parse_required_string(object, field_name, field_path)?;
    match raw.as_str() {
        "active-high" => Ok(ActiveLevel::ActiveHigh),
        "active-low" => Ok(ActiveLevel::ActiveLow),
        other => Err(GpioError::Other(format!(
            "{field_path} must be 'active-high' or 'active-low' (got: {other})"
        ))),
    }
}

fn normalize_digital_value_path(value_path: &str) -> Result<String, GpioError> {
    let mut normalized = PathBuf::new();
    for component in Path::new(value_path).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(GpioError::Other(
            "resources.gpio.digital_pins[].value_path must not normalize to empty path".to_string(),
        ));
    }
    if !normalized.is_absolute() {
        return Err(GpioError::Other(
            "resources.gpio.digital_pins[].value_path must be an absolute path".to_string(),
        ));
    }

    let gpio_base_path = Path::new(GPIO_SYSFS_BASE_PATH);
    if !normalized.starts_with(gpio_base_path) {
        return Err(GpioError::Other(format!(
            "resources.gpio.digital_pins[].value_path must stay under {GPIO_SYSFS_BASE_PATH}"
        )));
    }
    if normalized.file_name() != Some(OsStr::new("value")) {
        return Err(GpioError::Other(
            "resources.gpio.digital_pins[].value_path must target a GPIO value file".to_string(),
        ));
    }

    let normalized = normalized.to_string_lossy().into_owned();
    Ok(normalized)
}

fn parse_digital_pin_catalog(
    resources: &BTreeMap<String, JsonValue>,
) -> Result<Vec<DigitalPinSpec>, GpioError> {
    let Some(gpio_value) = resources.get(GPIO_RESOURCE_KEY) else {
        return Ok(Vec::new());
    };
    let gpio = gpio_value
        .as_object()
        .ok_or_else(|| GpioError::Other("resources.gpio must be a table".to_string()))?;
    let digital_pins_value = gpio
        .get(GPIO_RESOURCE_DIGITAL_PINS_KEY)
        .ok_or_else(|| GpioError::Other("resources.gpio.digital_pins is required".to_string()))?;
    let digital_pins = digital_pins_value.as_array().ok_or_else(|| {
        GpioError::Other("resources.gpio.digital_pins must be an array".to_string())
    })?;

    let mut seen_public_names = BTreeSet::new();
    let mut seen_value_paths = BTreeSet::new();
    let mut specs = Vec::with_capacity(digital_pins.len());
    for (index, pin_value) in digital_pins.iter().enumerate() {
        let pin_path = format!("resources.gpio.digital_pins[{index}]");
        let pin = pin_value
            .as_object()
            .ok_or_else(|| GpioError::Other(format!("{pin_path} must be a table")))?;
        let label = parse_required_string(pin, "label", &format!("{pin_path}.label"))?;
        if !seen_public_names.insert(label.clone()) {
            return Err(GpioError::Other(format!(
                "{pin_path}.label is duplicated: {}",
                label
            )));
        }
        let aliases = parse_optional_string_array(pin, "aliases", &format!("{pin_path}.aliases"))?;
        for (alias_index, alias) in aliases.iter().enumerate() {
            if !seen_public_names.insert(alias.clone()) {
                return Err(GpioError::Other(format!(
                    "{pin_path}.aliases[{alias_index}] is duplicated: {alias}"
                )));
            }
        }
        let value_path_raw =
            parse_required_string(pin, "value_path", &format!("{pin_path}.value_path"))?;
        let value_path = normalize_digital_value_path(&value_path_raw)?;
        if !seen_value_paths.insert(value_path.clone()) {
            return Err(GpioError::Other(format!(
                "{pin_path}.value_path is duplicated: {value_path}"
            )));
        }
        let supports_input =
            parse_required_bool(pin, "supports_input", &format!("{pin_path}.supports_input"))?;
        let supports_output = parse_required_bool(
            pin,
            "supports_output",
            &format!("{pin_path}.supports_output"),
        )?;
        if !supports_input && !supports_output {
            return Err(GpioError::Other(format!(
                "{pin_path} must allow at least one mode (supports_input or supports_output)"
            )));
        }
        let default_active_level = parse_required_active_level(
            pin,
            "default_active_level",
            &format!("{pin_path}.default_active_level"),
        )?;
        let allow_pull_resistor = parse_required_bool(
            pin,
            "allow_pull_resistor",
            &format!("{pin_path}.allow_pull_resistor"),
        )?;

        specs.push(DigitalPinSpec {
            label,
            aliases,
            value_path,
            supports_input,
            supports_output,
            default_active_level,
            allow_pull_resistor,
        });
    }
    Ok(specs)
}

fn lookup_digital_spec_from_catalog(
    catalog: &[DigitalPinSpec],
    pin_label: &str,
) -> Result<DigitalPinSpec, GpioError> {
    catalog
        .iter()
        .find(|spec| spec.label == pin_label || spec.aliases.iter().any(|alias| alias == pin_label))
        .cloned()
        .ok_or(GpioError::UndefinedPinLabel)
}

#[cfg(test)]
fn lookup_digital_spec(
    resources: &BTreeMap<String, JsonValue>,
    pin_label: &str,
) -> Result<DigitalPinSpec, GpioError> {
    let catalog = parse_digital_pin_catalog(resources)?;
    lookup_digital_spec_from_catalog(&catalog, pin_label)
}

fn digital_catalog_cache_key(service_name: &str, release_hash: &str, runner_id: &str) -> String {
    format!("{service_name}\u{1f}{release_hash}\u{1f}{runner_id}")
}

fn lookup_digital_spec_cached(
    cache_key: &str,
    resources: &BTreeMap<String, JsonValue>,
    pin_label: &str,
) -> Result<DigitalPinSpec, GpioError> {
    let mut guard = digital_pin_catalog_cache()
        .lock()
        .map_err(|_| GpioError::Other("digital pin catalog cache lock poisoned".to_string()))?;
    if !guard.contains_key(cache_key) {
        let catalog = parse_digital_pin_catalog(resources)?;
        guard.insert(cache_key.to_string(), catalog);
    }
    let catalog = guard
        .get(cache_key)
        .expect("digital pin catalog cache entry must exist");
    lookup_digital_spec_from_catalog(catalog, pin_label)
}

fn lookup_digital_spec_for_state(
    state: &WasiState,
    pin_label: &str,
) -> Result<DigitalPinSpec, GpioError> {
    let context = state.native_plugin_context();
    let cache_key = digital_catalog_cache_key(
        context.service_name(),
        context.release_hash(),
        context.runner_id(),
    );
    lookup_digital_spec_cached(&cache_key, context.resources(), pin_label)
}

fn lookup_analog_spec(pin_label: &str) -> Result<&'static AnalogPinSpec, GpioError> {
    ANALOG_PIN_CATALOG
        .iter()
        .find(|spec| spec.label == pin_label)
        .ok_or(GpioError::UndefinedPinLabel)
}

fn mode_is_supported_for_digital(spec: &DigitalPinSpec, mode: PinMode) -> bool {
    match mode {
        PinMode::In => spec.supports_input,
        PinMode::Out => spec.supports_output,
        PinMode::InOut => spec.supports_input && spec.supports_output,
    }
}

fn mode_is_supported_for_analog(spec: &AnalogPinSpec, mode: PinMode) -> bool {
    let supports_input = spec.read_raw_path.is_some();
    let supports_output = spec.write_raw_path.is_some();
    match mode {
        PinMode::In => supports_input,
        PinMode::Out => supports_output,
        PinMode::InOut => supports_input && supports_output,
    }
}

fn acquire_pin_key(pin_key: &str, mode: PinMode) -> Result<(), GpioError> {
    let mut guard = acquired_pin_modes()
        .lock()
        .map_err(|_| GpioError::Other("acquired pin registry lock poisoned".to_string()))?;
    if guard.contains_key(pin_key) {
        return Err(GpioError::AlreadyInUse);
    }
    guard.insert(pin_key.to_string(), mode);
    Ok(())
}

fn release_pin_key(pin_key: &str) {
    if let Ok(mut guard) = acquired_pin_modes().lock() {
        guard.remove(pin_key);
    }
}

fn digital_acquire_key(value_path: &str) -> String {
    format!("digital:{value_path}")
}

fn analog_acquire_key(label: &str) -> String {
    format!("analog:{label}")
}

fn digital_backend_requirements(mode: PinMode) -> (bool, bool) {
    match mode {
        PinMode::In => (true, false),
        PinMode::Out => (false, true),
        PinMode::InOut => (true, true),
    }
}

fn resolve_digital_config(
    spec: &DigitalPinSpec,
    flags: &[DigitalFlags],
) -> Result<(ActiveLevel, Option<PullResistor>), GpioError> {
    let mut combined = DigitalFlags::default();
    for flag in flags {
        combined |= *flag;
    }
    let active_high = combined.contains(DigitalFlags::ACTIVE_HIGH);
    let active_low = combined.contains(DigitalFlags::ACTIVE_LOW);
    let pull_up = combined.contains(DigitalFlags::PULL_UP);
    let pull_down = combined.contains(DigitalFlags::PULL_DOWN);

    if active_high && active_low {
        return Err(GpioError::Other(
            "conflicting active-level flags requested".to_string(),
        ));
    }
    if pull_up && pull_down {
        return Err(GpioError::Other(
            "conflicting pull-resistor flags requested".to_string(),
        ));
    }
    if (pull_up || pull_down) && !spec.allow_pull_resistor {
        return Err(GpioError::Other(format!(
            "pin '{}' does not support pull-resistor flags",
            spec.label
        )));
    }

    let active_level = if active_high {
        ActiveLevel::ActiveHigh
    } else if active_low {
        ActiveLevel::ActiveLow
    } else {
        spec.default_active_level
    };
    let pull_resistor = if pull_up {
        Some(PullResistor::PullUp)
    } else if pull_down {
        Some(PullResistor::PullDown)
    } else {
        None
    };

    Ok((active_level, pull_resistor))
}

fn resolve_analog_config(
    spec: &AnalogPinSpec,
    flags: &[AnalogFlags],
) -> Result<ActiveLevel, GpioError> {
    let mut combined = AnalogFlags::default();
    for flag in flags {
        combined |= *flag;
    }
    let active_high = combined.contains(AnalogFlags::ACTIVE_HIGH);
    let active_low = combined.contains(AnalogFlags::ACTIVE_LOW);

    if active_high && active_low {
        return Err(GpioError::Other(
            "conflicting active-level flags requested".to_string(),
        ));
    }

    let active_level = if active_high {
        ActiveLevel::ActiveHigh
    } else if active_low {
        ActiveLevel::ActiveLow
    } else {
        spec.default_active_level
    };

    Ok(active_level)
}

fn validate_digital_backend(
    path: &str,
    need_read: bool,
    need_write: bool,
) -> Result<(), GpioError> {
    if need_read {
        let _ = std::fs::read_to_string(path).map_err(|err| map_io_error(path, "read", err))?;
    }
    if need_write {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|err| map_io_error(path, "write", err))?;
    }
    Ok(())
}

fn validate_analog_backend(
    read_raw_path: Option<&str>,
    write_raw_path: Option<&str>,
    need_read: bool,
    need_write: bool,
) -> Result<(), GpioError> {
    if need_read {
        let Some(path) = read_raw_path else {
            return Err(GpioError::PinModeNotAvailable);
        };
        let _ = std::fs::read_to_string(path).map_err(|err| map_io_error(path, "read", err))?;
    }
    if need_write {
        let Some(path) = write_raw_path else {
            return Err(GpioError::PinModeNotAvailable);
        };
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|err| map_io_error(path, "write", err))?;
    }
    Ok(())
}

fn map_io_error(path: &str, operation: &str, err: std::io::Error) -> GpioError {
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => GpioError::PinNotAllowed,
        std::io::ErrorKind::WouldBlock => GpioError::WouldBlock,
        _ => GpioError::Other(format!("failed to {operation} '{path}': {err}")),
    }
}

fn parse_digital_raw_value(text: &str) -> Result<u8, GpioError> {
    match text.trim() {
        "0" => Ok(0),
        "1" => Ok(1),
        other => Err(GpioError::Other(format!(
            "digital value must be '0' or '1', got '{other}'"
        ))),
    }
}

fn pin_state_from_raw(raw: u8, active_level: ActiveLevel) -> PinState {
    let is_active = match active_level {
        ActiveLevel::ActiveHigh => raw == 1,
        ActiveLevel::ActiveLow => raw == 0,
    };
    if is_active {
        PinState::Active
    } else {
        PinState::Inactive
    }
}

fn raw_from_pin_state(state: PinState, active_level: ActiveLevel) -> u8 {
    match (state, active_level) {
        (PinState::Active, ActiveLevel::ActiveHigh) => 1,
        (PinState::Inactive, ActiveLevel::ActiveHigh) => 0,
        (PinState::Active, ActiveLevel::ActiveLow) => 0,
        (PinState::Inactive, ActiveLevel::ActiveLow) => 1,
    }
}

fn parse_analog_raw_value(text: &str) -> Result<u32, GpioError> {
    text.trim().parse::<u32>().map_err(|err| {
        GpioError::Other(format!(
            "analog value must be an unsigned integer, parse error: {err}"
        ))
    })
}

fn normalize_analog_value(
    raw: u32,
    max_raw: u32,
    active_level: ActiveLevel,
) -> Result<f32, GpioError> {
    if max_raw == 0 {
        return Err(GpioError::Other(
            "analog max_raw must be greater than zero".to_string(),
        ));
    }

    let raw = raw.min(max_raw);
    let mut normalized = raw as f64 / max_raw as f64;
    if matches!(active_level, ActiveLevel::ActiveLow) {
        normalized = 1.0 - normalized;
    }
    Ok(normalized as f32)
}

fn denormalize_analog_value(
    value: f32,
    max_raw: u32,
    active_level: ActiveLevel,
) -> Result<u32, GpioError> {
    if !value.is_finite() {
        return Err(GpioError::Other(
            "normalized analog value must be finite".to_string(),
        ));
    }
    if !(0.0..=1.0).contains(&value) {
        return Err(GpioError::Other(
            "normalized analog value must be in range [0.0, 1.0]".to_string(),
        ));
    }

    let logical = if matches!(active_level, ActiveLevel::ActiveLow) {
        1.0 - value
    } else {
        value
    };

    let scaled = (logical * max_raw as f32).round();
    let bounded = scaled.clamp(0.0, max_raw as f32);
    Ok(bounded as u32)
}

fn read_digital_state_sync(path: &str, active_level: ActiveLevel) -> Result<PinState, GpioError> {
    let raw = std::fs::read_to_string(path).map_err(|err| map_io_error(path, "read", err))?;
    let raw = parse_digital_raw_value(&raw)?;
    Ok(pin_state_from_raw(raw, active_level))
}

fn write_digital_state_sync(
    path: &str,
    state: PinState,
    active_level: ActiveLevel,
) -> Result<(), GpioError> {
    let raw = raw_from_pin_state(state, active_level);
    std::fs::write(path, raw.to_string().as_bytes()).map_err(|err| map_io_error(path, "write", err))
}

fn read_analog_raw_sync(path: &str) -> Result<u32, GpioError> {
    let raw = std::fs::read_to_string(path).map_err(|err| map_io_error(path, "read", err))?;
    parse_analog_raw_value(&raw)
}

fn write_analog_raw_sync(path: &str, value: u32) -> Result<(), GpioError> {
    std::fs::write(path, value.to_string().as_bytes())
        .map_err(|err| map_io_error(path, "write", err))
}

fn digital_config_from_handle(handle: &DigitalPinHandle) -> DigitalConfig {
    DigitalConfig {
        label: handle.label.clone(),
        pin_mode: handle.mode,
        active_level: handle.active_level,
        pull_resistor: handle.pull_resistor,
    }
}

fn analog_config_from_handle(handle: &AnalogPinHandle) -> AnalogConfig {
    AnalogConfig {
        label: handle.label.clone(),
        pin_mode: handle.mode,
        active_level: handle.active_level,
    }
}

fn ensure_digital_readable(handle: &DigitalPinHandle) -> Result<(), GpioError> {
    if matches!(handle.mode, PinMode::In | PinMode::InOut) {
        Ok(())
    } else {
        Err(GpioError::PinModeNotAllowed)
    }
}

fn ensure_digital_writable(handle: &DigitalPinHandle) -> Result<(), GpioError> {
    if matches!(handle.mode, PinMode::Out | PinMode::InOut) {
        Ok(())
    } else {
        Err(GpioError::PinModeNotAllowed)
    }
}

fn ensure_analog_readable(handle: &AnalogPinHandle) -> Result<&str, GpioError> {
    if !matches!(handle.mode, PinMode::In | PinMode::InOut) {
        return Err(GpioError::PinModeNotAllowed);
    }

    handle
        .read_raw_path
        .as_deref()
        .ok_or(GpioError::PinModeNotAvailable)
}

fn ensure_analog_writable(handle: &AnalogPinHandle) -> Result<&str, GpioError> {
    if !matches!(handle.mode, PinMode::Out | PinMode::InOut) {
        return Err(GpioError::PinModeNotAllowed);
    }

    handle
        .write_raw_path
        .as_deref()
        .ok_or(GpioError::PinModeNotAvailable)
}

fn push_pollable_resource<T>(
    table: &mut ResourceTable,
    pollable_impl: T,
) -> Result<Resource<PollableResource>, GpioError>
where
    T: Pollable,
{
    let watcher_resource = table
        .push(pollable_impl)
        .map_err(|err| GpioError::Other(format!("failed to allocate watcher resource: {err}")))?;
    let pollable_resource: Resource<DynPollable> = subscribe(table, watcher_resource)
        .map_err(|err| GpioError::Other(format!("failed to allocate pollable resource: {err}")))?;
    Ok(Resource::new_own(pollable_resource.rep()))
}

fn to_dyn_pollable_resource(pollable: Resource<PollableResource>) -> Resource<DynPollable> {
    if pollable.owned() {
        Resource::new_own(pollable.rep())
    } else {
        Resource::new_borrow(pollable.rep())
    }
}

struct PathReadyPollable {
    paths: Vec<PathBuf>,
    interval: Duration,
}

#[wasmtime_wasi::async_trait]
impl Pollable for PathReadyPollable {
    async fn ready(&mut self) {
        loop {
            let mut all_ready = true;
            for path in &self.paths {
                if tokio::fs::metadata(path).await.is_err() {
                    all_ready = false;
                    break;
                }
            }
            if all_ready {
                return;
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

struct DigitalStatePollable {
    path: PathBuf,
    desired_state: PinState,
    active_level: ActiveLevel,
    interval: Duration,
}

#[wasmtime_wasi::async_trait]
impl Pollable for DigitalStatePollable {
    async fn ready(&mut self) {
        loop {
            if let Some(state) = read_pin_state_async(&self.path, self.active_level).await
                && state == self.desired_state
            {
                return;
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

enum DigitalEdge {
    Rising,
    Falling,
}

struct DigitalEdgePollable {
    path: PathBuf,
    active_level: ActiveLevel,
    previous_state: Option<PinState>,
    edge: DigitalEdge,
    interval: Duration,
}

#[wasmtime_wasi::async_trait]
impl Pollable for DigitalEdgePollable {
    async fn ready(&mut self) {
        loop {
            if let Some(current) = read_pin_state_async(&self.path, self.active_level).await {
                if let Some(previous) = self.previous_state {
                    let matched = match self.edge {
                        DigitalEdge::Rising => {
                            previous == PinState::Inactive && current == PinState::Active
                        }
                        DigitalEdge::Falling => {
                            previous == PinState::Active && current == PinState::Inactive
                        }
                    };
                    if matched {
                        return;
                    }
                }
                self.previous_state = Some(current);
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

enum AnalogThresholdKind {
    AboveRaw(u32),
    BelowRaw(u32),
    Above(f32),
    Below(f32),
}

struct AnalogThresholdPollable {
    read_raw_path: PathBuf,
    active_level: ActiveLevel,
    max_raw: u32,
    threshold: AnalogThresholdKind,
    interval: Duration,
}

#[wasmtime_wasi::async_trait]
impl Pollable for AnalogThresholdPollable {
    async fn ready(&mut self) {
        loop {
            if let Some(raw) = read_analog_raw_async(&self.read_raw_path).await {
                let matched = match self.threshold {
                    AnalogThresholdKind::AboveRaw(limit) => raw >= limit,
                    AnalogThresholdKind::BelowRaw(limit) => raw <= limit,
                    AnalogThresholdKind::Above(limit) => {
                        normalize_analog_value(raw, self.max_raw, self.active_level)
                            .map(|value| value >= limit)
                            .unwrap_or(false)
                    }
                    AnalogThresholdKind::Below(limit) => {
                        normalize_analog_value(raw, self.max_raw, self.active_level)
                            .map(|value| value <= limit)
                            .unwrap_or(false)
                    }
                };
                if matched {
                    return;
                }
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

async fn read_pin_state_async(path: &Path, active_level: ActiveLevel) -> Option<PinState> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let raw = parse_digital_raw_value(&text).ok()?;
    Some(pin_state_from_raw(raw, active_level))
}

async fn read_analog_raw_async(path: &Path) -> Option<u32> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    parse_analog_raw_value(&text).ok()
}

impl imago_experimental_gpio_plugin_bindings::wasi::io::poll::Host for WasiState {
    async fn poll(&mut self, pollables: Vec<Resource<PollableResource>>) -> Vec<u32> {
        let dyn_pollables = pollables
            .into_iter()
            .map(to_dyn_pollable_resource)
            .collect();
        wasmtime_wasi::p2::bindings::sync::io::poll::Host::poll(self.ctx().table, dyn_pollables)
            .expect("wasi:io/poll::poll should not fail")
    }
}

impl imago_experimental_gpio_plugin_bindings::wasi::io::poll::HostPollable for WasiState {
    async fn ready(&mut self, pollable: Resource<PollableResource>) -> bool {
        let dyn_pollable = to_dyn_pollable_resource(pollable);
        wasmtime_wasi::p2::bindings::sync::io::poll::HostPollable::ready(
            self.ctx().table,
            dyn_pollable,
        )
        .expect("wasi:io/poll::pollable.ready should not fail")
    }

    async fn block(&mut self, pollable: Resource<PollableResource>) {
        let dyn_pollable = to_dyn_pollable_resource(pollable);
        wasmtime_wasi::p2::bindings::sync::io::poll::HostPollable::block(
            self.ctx().table,
            dyn_pollable,
        )
        .expect("wasi:io/poll::pollable.block should not fail");
    }

    async fn drop(&mut self, pollable: Resource<PollableResource>) -> wasmtime::Result<()> {
        let dyn_pollable = to_dyn_pollable_resource(pollable);
        wasmtime_wasi::p2::bindings::sync::io::poll::HostPollable::drop(
            self.ctx().table,
            dyn_pollable,
        )
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::general::Host
    for WasiState
{
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::delay::Host for WasiState {
    async fn delay_ns(&mut self, ns: u32) {
        tokio::time::sleep(Duration::from_nanos(u64::from(ns))).await;
    }

    async fn delay_us(&mut self, us: u32) {
        tokio::time::sleep(Duration::from_micros(u64::from(us))).await;
    }

    async fn delay_ms(&mut self, ms: u32) {
        tokio::time::sleep(Duration::from_millis(u64::from(ms))).await;
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::Host
    for WasiState
{
    async fn get_digital_in(
        &mut self,
        pin_label: String,
        flags: Vec<DigitalFlags>,
    ) -> Result<Resource<DigitalInResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_digital_spec_for_state(self, &pin_label)?;
        if !mode_is_supported_for_digital(&spec, PinMode::In) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let (active_level, pull_resistor) = resolve_digital_config(&spec, &flags)?;
        let path = spec.value_path.clone();
        let canonical_label = spec.label.clone();
        let acquire_key = digital_acquire_key(&path);
        let validation_path = path.clone();
        let (need_read, need_write) = digital_backend_requirements(PinMode::In);
        run_blocking_gpio(move || {
            validate_digital_backend(&validation_path, need_read, need_write)
        })
        .await?;

        acquire_pin_key(&acquire_key, PinMode::In)?;

        let handle = DigitalPinHandle {
            label: canonical_label,
            acquire_key,
            value_path: path,
            mode: PinMode::In,
            active_level,
            pull_resistor,
        };
        let rep = register_handle(digital_in_registry(), &NEXT_DIGITAL_IN_REP, handle);
        Ok(Resource::new_own(rep))
    }

    async fn get_digital_out(
        &mut self,
        pin_label: String,
        flags: Vec<DigitalFlags>,
    ) -> Result<Resource<DigitalOutResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_digital_spec_for_state(self, &pin_label)?;
        if !mode_is_supported_for_digital(&spec, PinMode::Out) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let (active_level, pull_resistor) = resolve_digital_config(&spec, &flags)?;
        let path = spec.value_path.clone();
        let canonical_label = spec.label.clone();
        let acquire_key = digital_acquire_key(&path);
        let validation_path = path.clone();
        let (need_read, need_write) = digital_backend_requirements(PinMode::Out);
        run_blocking_gpio(move || {
            validate_digital_backend(&validation_path, need_read, need_write)
        })
        .await?;

        acquire_pin_key(&acquire_key, PinMode::Out)?;

        let handle = DigitalPinHandle {
            label: canonical_label,
            acquire_key,
            value_path: path,
            mode: PinMode::Out,
            active_level,
            pull_resistor,
        };
        let rep = register_handle(digital_out_registry(), &NEXT_DIGITAL_OUT_REP, handle);
        Ok(Resource::new_own(rep))
    }

    async fn get_digital_in_out(
        &mut self,
        pin_label: String,
        flags: Vec<DigitalFlags>,
    ) -> Result<Resource<DigitalInOutResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_digital_spec_for_state(self, &pin_label)?;
        if !mode_is_supported_for_digital(&spec, PinMode::InOut) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let (active_level, pull_resistor) = resolve_digital_config(&spec, &flags)?;
        let path = spec.value_path.clone();
        let canonical_label = spec.label.clone();
        let acquire_key = digital_acquire_key(&path);
        let validation_path = path.clone();
        let (need_read, need_write) = digital_backend_requirements(PinMode::InOut);
        run_blocking_gpio(move || {
            validate_digital_backend(&validation_path, need_read, need_write)
        })
        .await?;

        acquire_pin_key(&acquire_key, PinMode::InOut)?;

        let handle = DigitalPinHandle {
            label: canonical_label,
            acquire_key,
            value_path: path,
            mode: PinMode::InOut,
            active_level,
            pull_resistor,
        };
        let rep = register_handle(digital_in_out_registry(), &NEXT_DIGITAL_IN_OUT_REP, handle);
        Ok(Resource::new_own(rep))
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::HostDigitalOutPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<DigitalOutResource>) -> DigitalConfig {
        let handle = lookup_handle(digital_out_registry(), self_.rep())
            .expect("digital-out resource should exist");
        digital_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<DigitalOutResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(digital_out_registry(), self_.rep())
            .expect("digital-out resource should exist");
        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths: vec![PathBuf::from(handle.value_path)],
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn set_state(
        &mut self,
        self_: Resource<DigitalOutResource>,
        state: PinState,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(digital_out_registry(), self_.rep())?;
        ensure_digital_writable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;
        run_blocking_gpio(move || write_digital_state_sync(&path, state, active_level)).await
    }

    async fn set_active(&mut self, self_: Resource<DigitalOutResource>) -> Result<(), GpioError> {
        self.set_state(self_, PinState::Active).await
    }

    async fn set_inactive(&mut self, self_: Resource<DigitalOutResource>) -> Result<(), GpioError> {
        self.set_state(self_, PinState::Inactive).await
    }

    async fn toggle(&mut self, self_: Resource<DigitalOutResource>) -> Result<(), GpioError> {
        let handle = lookup_handle(digital_out_registry(), self_.rep())?;
        ensure_digital_writable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;

        run_blocking_gpio(move || {
            let current = read_digital_state_sync(&path, active_level)?;
            let next = if current == PinState::Active {
                PinState::Inactive
            } else {
                PinState::Active
            };
            write_digital_state_sync(&path, next, active_level)
        })
        .await
    }

    async fn drop(&mut self, resource: Resource<DigitalOutResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(digital_out_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::HostDigitalInPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<DigitalInResource>) -> DigitalConfig {
        let handle = lookup_handle(digital_in_registry(), self_.rep())
            .expect("digital-in resource should exist");
        digital_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<DigitalInResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(digital_in_registry(), self_.rep())
            .expect("digital-in resource should exist");
        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths: vec![PathBuf::from(handle.value_path)],
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn read(&mut self, self_: Resource<DigitalInResource>) -> Result<PinState, GpioError> {
        let handle = lookup_handle(digital_in_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;
        run_blocking_gpio(move || read_digital_state_sync(&path, active_level)).await
    }

    async fn is_active(&mut self, self_: Resource<DigitalInResource>) -> Result<bool, GpioError> {
        self.read(self_)
            .await
            .map(|state| state == PinState::Active)
    }

    async fn is_inactive(&mut self, self_: Resource<DigitalInResource>) -> Result<bool, GpioError> {
        self.read(self_)
            .await
            .map(|state| state == PinState::Inactive)
    }

    async fn watch_state(
        &mut self,
        self_: Resource<DigitalInResource>,
        state: PinState,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalStatePollable {
                path: PathBuf::from(handle.value_path),
                desired_state: state,
                active_level: handle.active_level,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_active(
        &mut self,
        self_: Resource<DigitalInResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        self.watch_state(self_, PinState::Active).await
    }

    async fn watch_inactive(
        &mut self,
        self_: Resource<DigitalInResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        self.watch_state(self_, PinState::Inactive).await
    }

    async fn watch_falling_edge(
        &mut self,
        self_: Resource<DigitalInResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalEdgePollable {
                path: PathBuf::from(handle.value_path),
                active_level: handle.active_level,
                previous_state: None,
                edge: DigitalEdge::Falling,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_rising_edge(
        &mut self,
        self_: Resource<DigitalInResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalEdgePollable {
                path: PathBuf::from(handle.value_path),
                active_level: handle.active_level,
                previous_state: None,
                edge: DigitalEdge::Rising,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn drop(&mut self, resource: Resource<DigitalInResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(digital_in_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::digital::HostDigitalInOutPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<DigitalInOutResource>) -> DigitalConfig {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())
            .expect("digital-in-out resource should exist");
        digital_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())
            .expect("digital-in-out resource should exist");
        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths: vec![PathBuf::from(handle.value_path)],
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn set_state(
        &mut self,
        self_: Resource<DigitalInOutResource>,
        state: PinState,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_writable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;
        run_blocking_gpio(move || write_digital_state_sync(&path, state, active_level)).await
    }

    async fn set_active(&mut self, self_: Resource<DigitalInOutResource>) -> Result<(), GpioError> {
        self.set_state(self_, PinState::Active).await
    }

    async fn set_inactive(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<(), GpioError> {
        self.set_state(self_, PinState::Inactive).await
    }

    async fn toggle(&mut self, self_: Resource<DigitalInOutResource>) -> Result<(), GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_writable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;

        run_blocking_gpio(move || {
            let current = read_digital_state_sync(&path, active_level)?;
            let next = if current == PinState::Active {
                PinState::Inactive
            } else {
                PinState::Active
            };
            write_digital_state_sync(&path, next, active_level)
        })
        .await
    }

    async fn read(&mut self, self_: Resource<DigitalInOutResource>) -> Result<PinState, GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;
        let path = handle.value_path;
        let active_level = handle.active_level;
        run_blocking_gpio(move || read_digital_state_sync(&path, active_level)).await
    }

    async fn is_active(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<bool, GpioError> {
        self.read(self_)
            .await
            .map(|state| state == PinState::Active)
    }

    async fn is_inactive(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<bool, GpioError> {
        self.read(self_)
            .await
            .map(|state| state == PinState::Inactive)
    }

    async fn watch_state(
        &mut self,
        self_: Resource<DigitalInOutResource>,
        state: PinState,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalStatePollable {
                path: PathBuf::from(handle.value_path),
                desired_state: state,
                active_level: handle.active_level,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_active(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        self.watch_state(self_, PinState::Active).await
    }

    async fn watch_inactive(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        self.watch_state(self_, PinState::Inactive).await
    }

    async fn watch_falling_edge(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalEdgePollable {
                path: PathBuf::from(handle.value_path),
                active_level: handle.active_level,
                previous_state: None,
                edge: DigitalEdge::Falling,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_rising_edge(
        &mut self,
        self_: Resource<DigitalInOutResource>,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(digital_in_out_registry(), self_.rep())?;
        ensure_digital_readable(&handle)?;

        push_pollable_resource(
            self.ctx().table,
            DigitalEdgePollable {
                path: PathBuf::from(handle.value_path),
                active_level: handle.active_level,
                previous_state: None,
                edge: DigitalEdge::Rising,
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn drop(&mut self, resource: Resource<DigitalInOutResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(digital_in_out_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::Host for WasiState {
    async fn get_analog_in(
        &mut self,
        pin_label: String,
        flags: Vec<AnalogFlags>,
    ) -> Result<Resource<AnalogInResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_analog_spec(&pin_label)?;
        if !mode_is_supported_for_analog(spec, PinMode::In) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let active_level = resolve_analog_config(spec, &flags)?;
        run_blocking_gpio(move || {
            validate_analog_backend(spec.read_raw_path, spec.write_raw_path, true, false)
        })
        .await?;

        let acquire_key = analog_acquire_key(spec.label);
        acquire_pin_key(&acquire_key, PinMode::In)?;

        let handle = AnalogPinHandle {
            label: spec.label.to_string(),
            acquire_key,
            read_raw_path: spec.read_raw_path.map(ToString::to_string),
            write_raw_path: spec.write_raw_path.map(ToString::to_string),
            mode: PinMode::In,
            active_level,
            max_raw: spec.max_raw,
        };
        let rep = register_handle(analog_in_registry(), &NEXT_ANALOG_IN_REP, handle);
        Ok(Resource::new_own(rep))
    }

    async fn get_analog_out(
        &mut self,
        pin_label: String,
        flags: Vec<AnalogFlags>,
    ) -> Result<Resource<AnalogOutResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_analog_spec(&pin_label)?;
        if !mode_is_supported_for_analog(spec, PinMode::Out) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let active_level = resolve_analog_config(spec, &flags)?;
        run_blocking_gpio(move || {
            validate_analog_backend(spec.read_raw_path, spec.write_raw_path, false, true)
        })
        .await?;

        let acquire_key = analog_acquire_key(spec.label);
        acquire_pin_key(&acquire_key, PinMode::Out)?;

        let handle = AnalogPinHandle {
            label: spec.label.to_string(),
            acquire_key,
            read_raw_path: spec.read_raw_path.map(ToString::to_string),
            write_raw_path: spec.write_raw_path.map(ToString::to_string),
            mode: PinMode::Out,
            active_level,
            max_raw: spec.max_raw,
        };
        let rep = register_handle(analog_out_registry(), &NEXT_ANALOG_OUT_REP, handle);
        Ok(Resource::new_own(rep))
    }

    async fn get_analog_in_out(
        &mut self,
        pin_label: String,
        flags: Vec<AnalogFlags>,
    ) -> Result<Resource<AnalogInOutResource>, GpioError> {
        ensure_gpio_supported()?;

        let spec = lookup_analog_spec(&pin_label)?;
        if !mode_is_supported_for_analog(spec, PinMode::InOut) {
            return Err(GpioError::PinModeNotAvailable);
        }

        let active_level = resolve_analog_config(spec, &flags)?;
        run_blocking_gpio(move || {
            validate_analog_backend(spec.read_raw_path, spec.write_raw_path, true, true)
        })
        .await?;

        let acquire_key = analog_acquire_key(spec.label);
        acquire_pin_key(&acquire_key, PinMode::InOut)?;

        let handle = AnalogPinHandle {
            label: spec.label.to_string(),
            acquire_key,
            read_raw_path: spec.read_raw_path.map(ToString::to_string),
            write_raw_path: spec.write_raw_path.map(ToString::to_string),
            mode: PinMode::InOut,
            active_level,
            max_raw: spec.max_raw,
        };
        let rep = register_handle(analog_in_out_registry(), &NEXT_ANALOG_IN_OUT_REP, handle);
        Ok(Resource::new_own(rep))
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::HostAnalogOutPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<AnalogOutResource>) -> AnalogConfig {
        let handle = lookup_handle(analog_out_registry(), self_.rep())
            .expect("analog-out resource should exist");
        analog_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<AnalogOutResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(analog_out_registry(), self_.rep())
            .expect("analog-out resource should exist");
        let mut paths = Vec::new();
        if let Some(path) = handle.write_raw_path {
            paths.push(PathBuf::from(path));
        }

        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths,
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn set_value_raw(
        &mut self,
        self_: Resource<AnalogOutResource>,
        value: u32,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(analog_out_registry(), self_.rep())?;
        let path = ensure_analog_writable(&handle)?.to_string();
        let value = value.min(handle.max_raw);
        run_blocking_gpio(move || write_analog_raw_sync(&path, value)).await
    }

    async fn set_value(
        &mut self,
        self_: Resource<AnalogOutResource>,
        value: f32,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(analog_out_registry(), self_.rep())?;
        let path = ensure_analog_writable(&handle)?.to_string();
        let raw = denormalize_analog_value(value, handle.max_raw, handle.active_level)?;
        run_blocking_gpio(move || write_analog_raw_sync(&path, raw)).await
    }

    async fn drop(&mut self, resource: Resource<AnalogOutResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(analog_out_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::HostAnalogInPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<AnalogInResource>) -> AnalogConfig {
        let handle = lookup_handle(analog_in_registry(), self_.rep())
            .expect("analog-in resource should exist");
        analog_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<AnalogInResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(analog_in_registry(), self_.rep())
            .expect("analog-in resource should exist");
        let mut paths = Vec::new();
        if let Some(path) = handle.read_raw_path {
            paths.push(PathBuf::from(path));
        }

        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths,
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn read_raw(&mut self, self_: Resource<AnalogInResource>) -> Result<u32, GpioError> {
        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();
        let max_raw = handle.max_raw;
        run_blocking_gpio(move || read_analog_raw_sync(&path).map(|raw| raw.min(max_raw))).await
    }

    async fn read(&mut self, self_: Resource<AnalogInResource>) -> Result<f32, GpioError> {
        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();
        let max_raw = handle.max_raw;
        let active_level = handle.active_level;

        run_blocking_gpio(move || {
            let raw = read_analog_raw_sync(&path)?;
            normalize_analog_value(raw, max_raw, active_level)
        })
        .await
    }

    async fn watch_above_raw(
        &mut self,
        self_: Resource<AnalogInResource>,
        value: u32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::AboveRaw(value.min(handle.max_raw)),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_above(
        &mut self,
        self_: Resource<AnalogInResource>,
        value: f32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(GpioError::Other(
                "normalized analog threshold must be in range [0.0, 1.0]".to_string(),
            ));
        }

        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::Above(value),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_below_raw(
        &mut self,
        self_: Resource<AnalogInResource>,
        value: u32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::BelowRaw(value.min(handle.max_raw)),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_below(
        &mut self,
        self_: Resource<AnalogInResource>,
        value: f32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(GpioError::Other(
                "normalized analog threshold must be in range [0.0, 1.0]".to_string(),
            ));
        }

        let handle = lookup_handle(analog_in_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::Below(value),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn drop(&mut self, resource: Resource<AnalogInResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(analog_in_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

impl imago_experimental_gpio_plugin_bindings::imago::experimental_gpio::analog::HostAnalogInOutPin
    for WasiState
{
    async fn get_config(&mut self, self_: Resource<AnalogInOutResource>) -> AnalogConfig {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())
            .expect("analog-in-out resource should exist");
        analog_config_from_handle(&handle)
    }

    async fn watch_for_ready(
        &mut self,
        self_: Resource<AnalogInOutResource>,
    ) -> Resource<PollableResource> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())
            .expect("analog-in-out resource should exist");
        let mut paths = Vec::new();
        if let Some(path) = handle.read_raw_path {
            paths.push(PathBuf::from(path));
        }
        if let Some(path) = handle.write_raw_path {
            paths.push(PathBuf::from(path));
        }

        push_pollable_resource(
            self.ctx().table,
            PathReadyPollable {
                paths,
                interval: WATCH_POLL_INTERVAL,
            },
        )
        .expect("watch-for-ready pollable allocation should succeed")
    }

    async fn set_value_raw(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: u32,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_writable(&handle)?.to_string();
        let value = value.min(handle.max_raw);
        run_blocking_gpio(move || write_analog_raw_sync(&path, value)).await
    }

    async fn set_value(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: f32,
    ) -> Result<(), GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_writable(&handle)?.to_string();
        let raw = denormalize_analog_value(value, handle.max_raw, handle.active_level)?;
        run_blocking_gpio(move || write_analog_raw_sync(&path, raw)).await
    }

    async fn read_raw(&mut self, self_: Resource<AnalogInOutResource>) -> Result<u32, GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();
        let max_raw = handle.max_raw;
        run_blocking_gpio(move || read_analog_raw_sync(&path).map(|raw| raw.min(max_raw))).await
    }

    async fn read(&mut self, self_: Resource<AnalogInOutResource>) -> Result<f32, GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();
        let max_raw = handle.max_raw;
        let active_level = handle.active_level;

        run_blocking_gpio(move || {
            let raw = read_analog_raw_sync(&path)?;
            normalize_analog_value(raw, max_raw, active_level)
        })
        .await
    }

    async fn watch_above_raw(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: u32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::AboveRaw(value.min(handle.max_raw)),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_above(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: f32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(GpioError::Other(
                "normalized analog threshold must be in range [0.0, 1.0]".to_string(),
            ));
        }

        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::Above(value),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_below_raw(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: u32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::BelowRaw(value.min(handle.max_raw)),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn watch_below(
        &mut self,
        self_: Resource<AnalogInOutResource>,
        value: f32,
    ) -> Result<Resource<PollableResource>, GpioError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(GpioError::Other(
                "normalized analog threshold must be in range [0.0, 1.0]".to_string(),
            ));
        }

        let handle = lookup_handle(analog_in_out_registry(), self_.rep())?;
        let path = ensure_analog_readable(&handle)?.to_string();

        push_pollable_resource(
            self.ctx().table,
            AnalogThresholdPollable {
                read_raw_path: PathBuf::from(path),
                active_level: handle.active_level,
                max_raw: handle.max_raw,
                threshold: AnalogThresholdKind::Below(value),
                interval: WATCH_POLL_INTERVAL,
            },
        )
    }

    async fn drop(&mut self, resource: Resource<AnalogInOutResource>) -> wasmtime::Result<()> {
        if let Some(handle) = remove_handle(analog_in_out_registry(), resource.rep()) {
            release_pin_key(&handle.acquire_key);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digital_spec_fixture(allow_pull_resistor: bool) -> DigitalPinSpec {
        DigitalPinSpec {
            label: "GPIO17".to_string(),
            aliases: vec![],
            value_path: "/sys/class/gpio/gpio17/value".to_string(),
            supports_input: true,
            supports_output: true,
            default_active_level: ActiveLevel::ActiveHigh,
            allow_pull_resistor,
        }
    }

    fn resources_with_valid_digital_pins() -> BTreeMap<String, JsonValue> {
        BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO22",
                        "aliases": ["blue-led"],
                        "value_path": "/sys/class/gpio/gpio22/value",
                        "supports_input": false,
                        "supports_output": true,
                        "default_active_level": "active-low",
                        "allow_pull_resistor": false
                    }
                ]
            }),
        )])
    }

    #[test]
    fn lookup_digital_spec_reports_undefined_label_when_resources_gpio_is_missing() {
        let resources = BTreeMap::new();
        let err = lookup_digital_spec(&resources, "DOES_NOT_EXIST")
            .expect_err("unknown label should fail");
        assert!(matches!(err, GpioError::UndefinedPinLabel));
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_non_array_digital_pins() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": "not-an-array"
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources).expect_err("non-array must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("resources.gpio.digital_pins must be an array"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_missing_required_field() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources).expect_err("missing field must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("resources.gpio.digital_pins[0].supports_output is required"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_invalid_default_active_level() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "invalid",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("invalid active level must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("default_active_level"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_duplicate_labels() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio22/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources).expect_err("duplicate labels must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains(".label is duplicated"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_duplicate_labels_before_other_field_validation() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO17"
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("duplicate labels should fail first");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains(".label is duplicated"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("supports_output"),
            "duplicate label should fail before other field validation: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_duplicate_aliases_before_other_field_validation() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "aliases": ["blue-led"],
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO22",
                        "aliases": ["blue-led"]
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("duplicate aliases should fail first");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains(".aliases[0] is duplicated"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("supports_output"),
            "duplicate alias should fail before other field validation: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_duplicate_value_paths() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO22",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("duplicate value_path must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("value_path is duplicated"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_duplicate_value_paths_after_normalization() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO22",
                        "value_path": "/sys/class/gpio/../gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources)
            .expect_err("normalized duplicate value_path should fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("value_path is duplicated"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_relative_value_path() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("relative value_path should fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("must be an absolute path"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_value_path_outside_gpio_sysfs() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/etc/hosts",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources)
            .expect_err("value_path outside sysfs should fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("must stay under /sys/class/gpio"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_non_value_gpio_file() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/direction",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("non-value file path should fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("must target a GPIO value file"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_pin_without_supported_mode() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": false,
                        "supports_output": false,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err =
            parse_digital_pin_catalog(&resources).expect_err("unsupported mode pin must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains("must allow at least one mode"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_and_lookup_accepts_valid_entries() {
        let resources = resources_with_valid_digital_pins();
        let parsed = parse_digital_pin_catalog(&resources).expect("catalog should parse");
        assert_eq!(parsed.len(), 2);
        let spec = lookup_digital_spec(&resources, "GPIO17").expect("known label should resolve");
        assert_eq!(spec.label, "GPIO17");
        assert_eq!(spec.value_path, "/sys/class/gpio/gpio17/value");
        assert!(spec.supports_input);
        assert!(spec.supports_output);
    }

    #[test]
    fn parse_digital_pin_catalog_lookup_accepts_alias() {
        let resources = resources_with_valid_digital_pins();
        let spec = lookup_digital_spec(&resources, "blue-led").expect("alias should resolve");
        assert_eq!(spec.label, "GPIO22");
        assert_eq!(spec.aliases, vec!["blue-led".to_string()]);
    }

    #[test]
    fn parse_digital_pin_catalog_rejects_alias_that_matches_primary_label() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    },
                    {
                        "label": "GPIO22",
                        "aliases": ["GPIO17"],
                        "value_path": "/sys/class/gpio/gpio22/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let err = parse_digital_pin_catalog(&resources)
            .expect_err("alias matching a primary label must fail");
        let message = match err {
            GpioError::Other(message) => message,
            _ => panic!("expected other error"),
        };
        assert!(
            message.contains(".aliases[0] is duplicated"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn parse_digital_pin_catalog_normalizes_value_path_in_lookup_result() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "GPIO17",
                        "value_path": "/sys/class/gpio/../gpio/gpio17/value",
                        "supports_input": true,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": true
                    }
                ]
            }),
        )]);
        let spec = lookup_digital_spec(&resources, "GPIO17").expect("known label should resolve");
        assert_eq!(spec.value_path, "/sys/class/gpio/gpio17/value");
    }

    #[test]
    fn lookup_digital_spec_cached_reuses_catalog_for_same_cache_key() {
        clear_digital_pin_catalog_cache_for_tests();
        let cache_key =
            digital_catalog_cache_key("service-cache-test", "release-cache-test", "runner-cache");

        let resources = resources_with_valid_digital_pins();
        let first = lookup_digital_spec_cached(&cache_key, &resources, "GPIO17")
            .expect("first lookup should parse and resolve");
        assert_eq!(first.label, "GPIO17");

        let second = lookup_digital_spec_cached(&cache_key, &BTreeMap::new(), "GPIO17")
            .expect("second lookup should use cached catalog");
        assert_eq!(second.label, "GPIO17");

        clear_digital_pin_catalog_cache_for_tests();
    }

    #[test]
    fn pin_acquire_detects_duplicate_use() {
        let pin_key = digital_acquire_key("/sys/class/gpio/gpio17/value");
        acquire_pin_key(&pin_key, PinMode::Out).expect("first acquire should pass");
        let err = acquire_pin_key(&pin_key, PinMode::In).expect_err("duplicate must fail");
        assert!(matches!(err, GpioError::AlreadyInUse));
        release_pin_key(&pin_key);
    }

    #[test]
    fn alias_acquire_uses_same_physical_pin_key() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "A27",
                        "aliases": ["blue-led"],
                        "value_path": "/sys/class/gpio/gpio507/value",
                        "supports_input": false,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": false
                    }
                ]
            }),
        )]);
        let primary = lookup_digital_spec(&resources, "A27").expect("primary label should resolve");
        let alias = lookup_digital_spec(&resources, "blue-led").expect("alias should resolve");
        let primary_key = digital_acquire_key(&primary.value_path);
        let alias_key = digital_acquire_key(&alias.value_path);
        acquire_pin_key(&primary_key, PinMode::Out).expect("primary acquire should pass");
        let err = acquire_pin_key(&alias_key, PinMode::Out)
            .expect_err("alias acquire should fail for the same physical pin");
        assert!(matches!(err, GpioError::AlreadyInUse));
        release_pin_key(&primary_key);
    }

    #[test]
    fn digital_config_uses_canonical_label_for_alias_lookup() {
        let resources = BTreeMap::from([(
            "gpio".to_string(),
            json!({
                "digital_pins": [
                    {
                        "label": "A27",
                        "aliases": ["blue-led"],
                        "value_path": "/sys/class/gpio/gpio507/value",
                        "supports_input": false,
                        "supports_output": true,
                        "default_active_level": "active-high",
                        "allow_pull_resistor": false
                    }
                ]
            }),
        )]);
        let spec = lookup_digital_spec(&resources, "blue-led").expect("alias should resolve");
        let handle = DigitalPinHandle {
            label: spec.label,
            acquire_key: digital_acquire_key(&spec.value_path),
            value_path: spec.value_path,
            mode: PinMode::Out,
            active_level: ActiveLevel::ActiveHigh,
            pull_resistor: None,
        };
        let config = digital_config_from_handle(&handle);
        assert_eq!(config.label, "A27");
    }

    #[test]
    fn digital_raw_roundtrip_observes_active_level() {
        assert_eq!(
            raw_from_pin_state(PinState::Active, ActiveLevel::ActiveHigh),
            1
        );
        assert_eq!(
            raw_from_pin_state(PinState::Inactive, ActiveLevel::ActiveLow),
            1
        );

        assert_eq!(
            pin_state_from_raw(1, ActiveLevel::ActiveHigh),
            PinState::Active
        );
        assert_eq!(
            pin_state_from_raw(1, ActiveLevel::ActiveLow),
            PinState::Inactive
        );
    }

    #[test]
    fn normalize_and_denormalize_analog_values() {
        let normalized = normalize_analog_value(2048, 4095, ActiveLevel::ActiveHigh)
            .expect("normalization should work");
        assert!(normalized > 0.49 && normalized < 0.51);

        let raw = denormalize_analog_value(0.25, 4095, ActiveLevel::ActiveHigh)
            .expect("denormalization should work");
        assert!(raw > 1000 && raw < 1100);

        let low_raw = denormalize_analog_value(1.0, 4095, ActiveLevel::ActiveLow)
            .expect("active-low denormalization should work");
        assert_eq!(low_raw, 0);
    }

    #[test]
    fn parse_digital_raw_value_rejects_invalid_text() {
        let err = parse_digital_raw_value("2").expect_err("invalid value should fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_digital_config_applies_active_low_and_pull_up() {
        let spec = digital_spec_fixture(true);
        let (active_level, pull_resistor) =
            resolve_digital_config(&spec, &[DigitalFlags::ACTIVE_LOW, DigitalFlags::PULL_UP])
                .expect("flags should be accepted");
        assert_eq!(active_level, ActiveLevel::ActiveLow);
        assert_eq!(pull_resistor, Some(PullResistor::PullUp));
    }

    #[test]
    fn resolve_digital_config_rejects_conflicting_active_level_flags() {
        let spec = digital_spec_fixture(true);
        let err = resolve_digital_config(
            &spec,
            &[DigitalFlags::ACTIVE_HIGH, DigitalFlags::ACTIVE_LOW],
        )
        .expect_err("conflicting active-level flags must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_digital_config_rejects_conflicting_pull_flags() {
        let spec = digital_spec_fixture(true);
        let err = resolve_digital_config(&spec, &[DigitalFlags::PULL_UP, DigitalFlags::PULL_DOWN])
            .expect_err("conflicting pull flags must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_digital_config_rejects_pull_flags_when_pin_disallows_pull_resistor() {
        let spec = digital_spec_fixture(false);
        let err = resolve_digital_config(&spec, &[DigitalFlags::PULL_UP])
            .expect_err("pull flag must fail when pin disallows pull resistor");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_analog_config_applies_active_low_flag() {
        let spec = lookup_analog_spec("ADC0").expect("known pin");
        let active_level = resolve_analog_config(spec, &[AnalogFlags::ACTIVE_LOW])
            .expect("active-low should be accepted");
        assert_eq!(active_level, ActiveLevel::ActiveLow);
    }

    #[test]
    fn resolve_analog_config_rejects_conflicting_active_level_flags() {
        let spec = lookup_analog_spec("ADC0").expect("known pin");
        let err = resolve_analog_config(spec, &[AnalogFlags::ACTIVE_HIGH, AnalogFlags::ACTIVE_LOW])
            .expect_err("conflicting active-level flags must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_digital_config_allows_duplicate_same_active_level_flag() {
        let spec = digital_spec_fixture(true);
        let (active_level, pull_resistor) =
            resolve_digital_config(&spec, &[DigitalFlags::ACTIVE_LOW, DigitalFlags::ACTIVE_LOW])
                .expect("duplicate same flag should be accepted");
        assert_eq!(active_level, ActiveLevel::ActiveLow);
        assert_eq!(pull_resistor, None);
    }

    #[test]
    fn resolve_digital_config_accepts_combined_bitset_in_single_element() {
        let spec = digital_spec_fixture(true);
        let combined = DigitalFlags::ACTIVE_LOW | DigitalFlags::PULL_UP;
        let (active_level, pull_resistor) =
            resolve_digital_config(&spec, &[combined]).expect("combined bitset should be accepted");
        assert_eq!(active_level, ActiveLevel::ActiveLow);
        assert_eq!(pull_resistor, Some(PullResistor::PullUp));
    }

    #[test]
    fn resolve_digital_config_rejects_conflicting_active_level_bits_in_single_element() {
        let spec = digital_spec_fixture(true);
        let combined = DigitalFlags::ACTIVE_HIGH | DigitalFlags::ACTIVE_LOW;
        let err = resolve_digital_config(&spec, &[combined])
            .expect_err("single-element conflicting active-level bits must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_digital_config_rejects_conflicting_pull_bits_in_single_element() {
        let spec = digital_spec_fixture(true);
        let combined = DigitalFlags::PULL_UP | DigitalFlags::PULL_DOWN;
        let err = resolve_digital_config(&spec, &[combined])
            .expect_err("single-element conflicting pull bits must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn resolve_analog_config_accepts_combined_bitset_in_single_element() {
        let spec = lookup_analog_spec("ADC0").expect("known pin");
        let active_level = resolve_analog_config(spec, &[AnalogFlags::ACTIVE_LOW])
            .expect("combined bitset with single flag should be accepted");
        assert_eq!(active_level, ActiveLevel::ActiveLow);
    }

    #[test]
    fn resolve_analog_config_rejects_conflicting_bits_in_single_element() {
        let spec = lookup_analog_spec("ADC0").expect("known pin");
        let combined = AnalogFlags::ACTIVE_HIGH | AnalogFlags::ACTIVE_LOW;
        let err = resolve_analog_config(spec, &[combined])
            .expect_err("single-element conflicting active-level bits must fail");
        assert!(matches!(err, GpioError::Other(_)));
    }

    #[test]
    fn digital_out_backend_validation_does_not_require_read_access() {
        assert_eq!(digital_backend_requirements(PinMode::Out), (false, true));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_is_operation_not_supported() {
        let err = ensure_gpio_supported().expect_err("non-linux should be unsupported");
        assert!(matches!(err, GpioError::OperationNotSupported));
    }
}
