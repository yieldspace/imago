use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    os::raw::c_void,
    path::{Component, Path},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::WasiState;
use imagod_runtime_wasmtime::native_plugins::{
    HasSelf, NativePlugin, NativePluginLinker, NativePluginResult, map_native_plugin_linker_error,
    map_native_plugin_resource_validation_error,
};
use rusb::{Hotplug, HotplugBuilder, UsbContext};
use serde_json::{Map as JsonMap, Value as JsonValue};
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::Resource;

pub mod imago_usb_plugin_bindings {
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
pub struct ImagoUsbPlugin;

impl NativePlugin for ImagoUsbPlugin {
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
        imago_usb_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
            .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
    }

    fn validate_resources(
        &self,
        resources: &BTreeMap<String, JsonValue>,
    ) -> NativePluginResult<()> {
        parse_usb_resources_config(resources)
            .map(|_| ())
            .map_err(|message| {
                map_native_plugin_resource_validation_error(Self::PACKAGE_NAME, message)
            })
    }
}

type UsbError = imago_usb_plugin_bindings::imago::usb::types::UsbError;
type ControlSetup = imago_usb_plugin_bindings::imago::usb::types::ControlSetup;
type ControlType = imago_usb_plugin_bindings::imago::usb::types::ControlType;
type Recipient = imago_usb_plugin_bindings::imago::usb::types::Recipient;
type Limits = imago_usb_plugin_bindings::imago::usb::types::Limits;
type VersionRecord = imago_usb_plugin_bindings::imago::usb::types::Version;
type DirectionRecord = imago_usb_plugin_bindings::imago::usb::types::Direction;
type UsageTypeRecord = imago_usb_plugin_bindings::imago::usb::types::UsageType;
type SyncTypeRecord = imago_usb_plugin_bindings::imago::usb::types::SyncType;
type TransferTypeRecord = imago_usb_plugin_bindings::imago::usb::types::TransferType;
type EndpointDescriptorRecord = imago_usb_plugin_bindings::imago::usb::types::EndpointDescriptor;
type InterfaceDescriptorRecord = imago_usb_plugin_bindings::imago::usb::types::InterfaceDescriptor;
type ConfigurationDescriptorRecord =
    imago_usb_plugin_bindings::imago::usb::types::ConfigurationDescriptor;
type DeviceDescriptorRecord = imago_usb_plugin_bindings::imago::usb::types::DeviceDescriptor;
type OpenableDevice = imago_usb_plugin_bindings::imago::usb::types::OpenableDevice;
type DeviceConnectionEvent = imago_usb_plugin_bindings::imago::usb::types::DeviceConnectionEvent;
type DeviceResource = imago_usb_plugin_bindings::imago::usb::device::Device;
type ClaimedInterfaceResource =
    imago_usb_plugin_bindings::imago::usb::usb_interface::ClaimedInterface;

const USB_RESOURCE_KEY: &str = "usb";
const USB_RESOURCE_PATHS_KEY: &str = "paths";
const USB_RESOURCE_MAX_TRANSFER_BYTES_KEY: &str = "max_transfer_bytes";
const USB_RESOURCE_MAX_TIMEOUT_MS_KEY: &str = "max_timeout_ms";
const USB_RESOURCE_MAX_PATHS_KEY: &str = "max_paths";

const DEFAULT_MAX_TRANSFER_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_TIMEOUT_MS: u32 = 30_000;
const DEFAULT_MAX_PATHS: usize = 128;

const MAX_MAX_TRANSFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_MAX_TIMEOUT_MS: u32 = 120_000;
const MAX_MAX_PATHS: usize = 256;

const DEVICE_COMMAND_CHANNEL_CAPACITY: usize = 64;
const DEFAULT_THREAD_STACK_BYTES: usize = 256 * 1024;
const MAX_HOTPLUG_QUEUE_LEN: usize = 256;
const MAX_ISO_PACKETS: u16 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsbLimitsConfig {
    max_transfer_bytes: usize,
    max_timeout_ms: u32,
    max_paths: usize,
}

impl Default for UsbLimitsConfig {
    fn default() -> Self {
        Self {
            max_transfer_bytes: DEFAULT_MAX_TRANSFER_BYTES,
            max_timeout_ms: DEFAULT_MAX_TIMEOUT_MS,
            max_paths: DEFAULT_MAX_PATHS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct UsbResourcesConfig {
    paths: Vec<String>,
    allowlist: BTreeSet<String>,
    limits: UsbLimitsConfig,
}

#[derive(Clone)]
struct DeviceRuntimeHandle {
    path: String,
    sender: mpsc::Sender<DeviceCommand>,
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

#[derive(Clone)]
struct ClaimedInterfaceHandle {
    number: u8,
    sender: mpsc::Sender<DeviceCommand>,
}

struct HotplugManager {
    queue: Arc<(Mutex<VecDeque<DeviceConnectionEvent>>, Condvar)>,
    stop: Arc<AtomicBool>,
    init_error_message: Arc<Mutex<Option<String>>>,
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl Drop for HotplugManager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut guard) = self.thread_handle.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

#[derive(Clone)]
struct HotplugCallback {
    allowlist: BTreeSet<String>,
    queue: Arc<(Mutex<VecDeque<DeviceConnectionEvent>>, Condvar)>,
}

impl HotplugCallback {
    fn push_event(&self, event: DeviceConnectionEvent) {
        let (lock, condvar) = &*self.queue;
        if let Ok(mut guard) = lock.lock() {
            if guard.len() >= MAX_HOTPLUG_QUEUE_LEN {
                let _ = guard.pop_front();
            }
            guard.push_back(event);
            condvar.notify_one();
        }
    }

    fn to_openable_device(&self, device: &rusb::Device<rusb::Context>) -> Option<OpenableDevice> {
        let bus = device.bus_number();
        let address = device.address();
        let path = usbfs_path(bus, address);
        if !self.allowlist.contains(&path) {
            return None;
        }

        let descriptor = device.device_descriptor().ok();
        let vendor_id = descriptor.as_ref().map_or(0, |d| d.vendor_id());
        let product_id = descriptor.as_ref().map_or(0, |d| d.product_id());

        Some(OpenableDevice {
            path,
            bus,
            address,
            vendor_id,
            product_id,
        })
    }
}

impl Hotplug<rusb::Context> for HotplugCallback {
    fn device_arrived(&mut self, device: rusb::Device<rusb::Context>) {
        if let Some(openable) = self.to_openable_device(&device) {
            self.push_event(DeviceConnectionEvent::Connected(openable));
        }
    }

    fn device_left(&mut self, device: rusb::Device<rusb::Context>) {
        if let Some(openable) = self.to_openable_device(&device) {
            self.push_event(DeviceConnectionEvent::Disconnected(openable));
        }
    }
}

struct DeviceThreadState {
    handle: rusb::DeviceHandle<rusb::Context>,
    claimed_interfaces: BTreeSet<u8>,
    alt_settings: BTreeMap<u8, u8>,
    limits: UsbLimitsConfig,
}

enum DeviceCommand {
    ClaimInterface {
        number: u8,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    ReleaseInterface {
        number: u8,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    ReleaseInterfaceNoReply {
        number: u8,
    },
    DeviceDescriptor {
        reply: oneshot::Sender<Result<DeviceDescriptorRecord, UsbError>>,
    },
    Configurations {
        reply: oneshot::Sender<Result<Vec<ConfigurationDescriptorRecord>, UsbError>>,
    },
    Reset {
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    ActiveConfiguration {
        reply: oneshot::Sender<Result<u8, UsbError>>,
    },
    SelectConfiguration {
        configuration: u8,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    AlternateSetting {
        interface: u8,
        reply: oneshot::Sender<Result<u8, UsbError>>,
    },
    SetAlternateSetting {
        interface: u8,
        setting: u8,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    ControlIn {
        interface: u8,
        setup: ControlSetup,
        length: u32,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<Vec<u8>, UsbError>>,
    },
    ControlOut {
        interface: u8,
        setup: ControlSetup,
        data: Vec<u8>,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    BulkIn {
        interface: u8,
        endpoint: u8,
        length: u32,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<Vec<u8>, UsbError>>,
    },
    BulkOut {
        interface: u8,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    InterruptIn {
        interface: u8,
        endpoint: u8,
        length: u32,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<Vec<u8>, UsbError>>,
    },
    InterruptOut {
        interface: u8,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<(), UsbError>>,
    },
    IsochronousIn {
        interface: u8,
        endpoint: u8,
        length: u32,
        packets: u16,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<Vec<u8>, UsbError>>,
    },
    IsochronousOut {
        interface: u8,
        endpoint: u8,
        data: Vec<u8>,
        packets: u16,
        timeout_ms: u32,
        reply: oneshot::Sender<Result<u32, UsbError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

static NEXT_DEVICE_REP: AtomicU32 = AtomicU32::new(1);
static DEVICE_REGISTRY: OnceLock<Mutex<BTreeMap<u32, DeviceRuntimeHandle>>> = OnceLock::new();

static NEXT_CLAIMED_INTERFACE_REP: AtomicU32 = AtomicU32::new(1);
static CLAIMED_INTERFACE_REGISTRY: OnceLock<Mutex<BTreeMap<u32, ClaimedInterfaceHandle>>> =
    OnceLock::new();

static USB_RESOURCES_CACHE: OnceLock<Mutex<BTreeMap<String, UsbResourcesConfig>>> = OnceLock::new();
static HOTPLUG_MANAGER_CACHE: OnceLock<Mutex<BTreeMap<String, Arc<HotplugManager>>>> =
    OnceLock::new();

fn device_registry() -> &'static Mutex<BTreeMap<u32, DeviceRuntimeHandle>> {
    DEVICE_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn claimed_interface_registry() -> &'static Mutex<BTreeMap<u32, ClaimedInterfaceHandle>> {
    CLAIMED_INTERFACE_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn usb_resources_cache() -> &'static Mutex<BTreeMap<String, UsbResourcesConfig>> {
    USB_RESOURCES_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn hotplug_manager_cache() -> &'static Mutex<BTreeMap<String, Arc<HotplugManager>>> {
    HOTPLUG_MANAGER_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn register_rep<T: Clone>(
    next_rep: &AtomicU32,
    registry: &Mutex<BTreeMap<u32, T>>,
    value: T,
) -> Result<u32, String> {
    loop {
        let rep = next_rep.fetch_add(1, Ordering::Relaxed);
        if rep == 0 {
            continue;
        }
        let mut guard = registry
            .lock()
            .map_err(|_| "resource registry lock poisoned".to_string())?;
        if guard.insert(rep, value.clone()).is_none() {
            return Ok(rep);
        }
    }
}

fn lookup_rep<T: Clone>(
    registry: &Mutex<BTreeMap<u32, T>>,
    rep: u32,
    kind: &str,
) -> Result<T, String> {
    let guard = registry
        .lock()
        .map_err(|_| "resource registry lock poisoned".to_string())?;
    guard
        .get(&rep)
        .cloned()
        .ok_or_else(|| format!("{kind} not found: rep={rep}"))
}

fn remove_rep<T>(registry: &Mutex<BTreeMap<u32, T>>, rep: u32) {
    if let Ok(mut guard) = registry.lock() {
        guard.remove(&rep);
    }
}

fn register_device_handle(handle: DeviceRuntimeHandle) -> Result<u32, String> {
    register_rep(&NEXT_DEVICE_REP, device_registry(), handle)
}

fn lookup_device_handle(rep: u32) -> Result<DeviceRuntimeHandle, String> {
    lookup_rep(device_registry(), rep, "device handle")
}

fn remove_device_handle(rep: u32) {
    remove_rep(device_registry(), rep);
}

fn register_claimed_interface_handle(handle: ClaimedInterfaceHandle) -> Result<u32, String> {
    register_rep(
        &NEXT_CLAIMED_INTERFACE_REP,
        claimed_interface_registry(),
        handle,
    )
}

fn lookup_claimed_interface_handle(rep: u32) -> Result<ClaimedInterfaceHandle, String> {
    lookup_rep(
        claimed_interface_registry(),
        rep,
        "claimed interface handle",
    )
}

fn remove_claimed_interface_handle(rep: u32) {
    remove_rep(claimed_interface_registry(), rep);
}

fn retain_claimed_interfaces_for_other_senders(
    registry: &mut BTreeMap<u32, ClaimedInterfaceHandle>,
    sender: &mpsc::Sender<DeviceCommand>,
) -> usize {
    let before = registry.len();
    registry.retain(|_, handle| !handle.sender.same_channel(sender));
    before.saturating_sub(registry.len())
}

fn remove_claimed_interface_handles_for_sender(sender: &mpsc::Sender<DeviceCommand>) -> usize {
    let mut removed = 0;
    if let Ok(mut guard) = claimed_interface_registry().lock() {
        removed = retain_claimed_interfaces_for_other_senders(&mut guard, sender);
    }
    removed
}

fn map_lookup_error(err: String) -> UsbError {
    UsbError::Other(err)
}

fn normalize_usb_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("resources.usb.paths[] must not be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("resources.usb.paths[] must not contain NUL".to_string());
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(format!(
            "resources.usb.paths[] must be an absolute path (got: {trimmed})"
        ));
    }

    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                let _ = segments.pop();
            }
            Component::Normal(segment) => segments.push(segment.to_string_lossy().into_owned()),
            Component::Prefix(_) => {
                return Err(format!(
                    "resources.usb.paths[] must not use platform prefixes (got: {trimmed})"
                ));
            }
        }
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

fn parse_u64_field(table: &JsonMap<String, JsonValue>, key: &str) -> Result<Option<u64>, String> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };

    let number = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .ok_or_else(|| format!("resources.usb.{key} must be a non-negative integer"))?;
    Ok(Some(number))
}

fn parse_usb_resources_config(
    resources: &BTreeMap<String, JsonValue>,
) -> Result<UsbResourcesConfig, String> {
    let usb_value = resources
        .get(USB_RESOURCE_KEY)
        .ok_or_else(|| "resources.usb is required".to_string())?;
    let usb_table = usb_value
        .as_object()
        .ok_or_else(|| "resources.usb must be a table".to_string())?;

    let paths_value = usb_table
        .get(USB_RESOURCE_PATHS_KEY)
        .ok_or_else(|| "resources.usb.paths is required".to_string())?;
    let paths_array = paths_value
        .as_array()
        .ok_or_else(|| "resources.usb.paths must be an array".to_string())?;

    let mut paths = Vec::with_capacity(paths_array.len());
    let mut allowlist = BTreeSet::new();
    for (index, path_value) in paths_array.iter().enumerate() {
        let raw = path_value
            .as_str()
            .ok_or_else(|| format!("resources.usb.paths[{index}] must be a string"))?;
        let normalized = normalize_usb_path(raw)?;
        if !allowlist.insert(normalized.clone()) {
            return Err(format!(
                "resources.usb.paths[{index}] duplicates normalized path: {normalized}"
            ));
        }
        paths.push(normalized);
    }

    let max_transfer_bytes = parse_u64_field(usb_table, USB_RESOURCE_MAX_TRANSFER_BYTES_KEY)?
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                format!(
                    "resources.usb.{USB_RESOURCE_MAX_TRANSFER_BYTES_KEY} is too large for this platform"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_TRANSFER_BYTES);
    if max_transfer_bytes == 0 || max_transfer_bytes > MAX_MAX_TRANSFER_BYTES {
        return Err(format!(
            "resources.usb.{USB_RESOURCE_MAX_TRANSFER_BYTES_KEY} must be within 1..={MAX_MAX_TRANSFER_BYTES}"
        ));
    }

    let max_timeout_ms = parse_u64_field(usb_table, USB_RESOURCE_MAX_TIMEOUT_MS_KEY)?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                format!(
                    "resources.usb.{USB_RESOURCE_MAX_TIMEOUT_MS_KEY} is too large for this runtime"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_TIMEOUT_MS);
    if max_timeout_ms == 0 || max_timeout_ms > MAX_MAX_TIMEOUT_MS {
        return Err(format!(
            "resources.usb.{USB_RESOURCE_MAX_TIMEOUT_MS_KEY} must be within 1..={MAX_MAX_TIMEOUT_MS}"
        ));
    }

    let max_paths = parse_u64_field(usb_table, USB_RESOURCE_MAX_PATHS_KEY)?
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                format!("resources.usb.{USB_RESOURCE_MAX_PATHS_KEY} is too large for this platform")
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_PATHS);
    if max_paths > MAX_MAX_PATHS {
        return Err(format!(
            "resources.usb.{USB_RESOURCE_MAX_PATHS_KEY} must be within 0..={MAX_MAX_PATHS}"
        ));
    }

    if paths.len() > max_paths {
        return Err(format!(
            "resources.usb.paths contains {} entries which exceeds max_paths={max_paths}",
            paths.len()
        ));
    }

    Ok(UsbResourcesConfig {
        paths,
        allowlist,
        limits: UsbLimitsConfig {
            max_transfer_bytes,
            max_timeout_ms,
            max_paths,
        },
    })
}

fn usb_resources_cache_key(service_name: &str, release_hash: &str, runner_id: &str) -> String {
    format!("{service_name}\u{1f}{release_hash}\u{1f}{runner_id}")
}

fn is_hotplug_cache_key_in_scope(cache_key: &str, service_name: &str, runner_id: &str) -> bool {
    let mut parts = cache_key.split('\u{1f}');
    let Some(service) = parts.next() else {
        return false;
    };
    let Some(_release_hash) = parts.next() else {
        return false;
    };
    let Some(runner) = parts.next() else {
        return false;
    };
    parts.next().is_none() && service == service_name && runner == runner_id
}

fn load_usb_resources_for_state(state: &WasiState) -> Result<UsbResourcesConfig, UsbError> {
    let context = state.native_plugin_context();
    let cache_key = usb_resources_cache_key(
        context.service_name(),
        context.release_hash(),
        context.runner_id(),
    );

    let mut guard = usb_resources_cache()
        .lock()
        .map_err(|_| UsbError::Other("usb resource cache lock poisoned".to_string()))?;

    if !guard.contains_key(&cache_key) {
        let parsed = parse_usb_resources_config(context.resources()).map_err(UsbError::Other)?;
        guard.insert(cache_key.clone(), parsed);
    }

    guard
        .get(&cache_key)
        .cloned()
        .ok_or_else(|| UsbError::Other("usb resource cache entry missing".to_string()))
}

fn load_usb_resources_for_state_or_default(state: &WasiState) -> UsbResourcesConfig {
    load_usb_resources_for_state(state).unwrap_or_default()
}

fn to_limits_record(limits: &UsbLimitsConfig) -> Limits {
    Limits {
        max_transfer_bytes: u32::try_from(limits.max_transfer_bytes)
            .expect("max_transfer_bytes should fit in u32"),
        max_timeout_ms: limits.max_timeout_ms,
        max_paths: u32::try_from(limits.max_paths).expect("max_paths should fit in u32"),
    }
}

fn ensure_usb_supported() -> Result<(), UsbError> {
    #[cfg(target_os = "linux")]
    {
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(UsbError::OperationNotSupported)
    }
}

fn validate_timeout(timeout_ms: u32, limits: &UsbLimitsConfig) -> Result<Duration, UsbError> {
    if timeout_ms == 0 || timeout_ms > limits.max_timeout_ms {
        return Err(UsbError::InvalidArgument);
    }
    Ok(Duration::from_millis(u64::from(timeout_ms)))
}

fn validate_poll_timeout(timeout_ms: u32, limits: &UsbLimitsConfig) -> Result<Duration, UsbError> {
    if timeout_ms > limits.max_timeout_ms {
        return Err(UsbError::InvalidArgument);
    }
    Ok(Duration::from_millis(u64::from(timeout_ms)))
}

fn duration_to_libusb_timeout_ms(timeout: Duration) -> Result<u32, UsbError> {
    u32::try_from(timeout.as_millis()).map_err(|_| UsbError::InvalidArgument)
}

fn compute_iso_packet_lengths(total_len: usize, packets: u16) -> Result<Vec<u32>, UsbError> {
    validate_iso_packets(packets)?;

    let packet_count = usize::from(packets);
    let base_len = total_len / packet_count;
    let remainder = total_len % packet_count;
    let mut lengths = Vec::with_capacity(packet_count);

    for index in 0..packet_count {
        let len = base_len + usize::from(index < remainder);
        lengths.push(u32::try_from(len).map_err(|_| UsbError::InvalidArgument)?);
    }
    Ok(lengths)
}

fn validate_transfer_len(length: u32, limits: &UsbLimitsConfig) -> Result<usize, UsbError> {
    let length = usize::try_from(length).map_err(|_| UsbError::InvalidArgument)?;
    if length > limits.max_transfer_bytes {
        return Err(UsbError::InvalidArgument);
    }
    Ok(length)
}

fn validate_transfer_data_len(len: usize, limits: &UsbLimitsConfig) -> Result<(), UsbError> {
    if len > limits.max_transfer_bytes {
        return Err(UsbError::InvalidArgument);
    }
    Ok(())
}

fn validate_endpoint_in_address(endpoint: u8) -> Result<(), UsbError> {
    // This validator is used by bulk/interrupt/iso transfers only.
    // Endpoint 0 is reserved for control transfers and intentionally rejected here.
    if endpoint & 0x80 == 0 || (endpoint & 0x0f) == 0 {
        return Err(UsbError::InvalidArgument);
    }
    Ok(())
}

fn validate_endpoint_out_address(endpoint: u8) -> Result<(), UsbError> {
    // This validator is used by bulk/interrupt/iso transfers only.
    // Endpoint 0 is reserved for control transfers and intentionally rejected here.
    if endpoint & 0x80 != 0 || (endpoint & 0x0f) == 0 {
        return Err(UsbError::InvalidArgument);
    }
    Ok(())
}

fn validate_iso_packets(packets: u16) -> Result<(), UsbError> {
    if packets == 0 || packets > MAX_ISO_PACKETS {
        return Err(UsbError::InvalidArgument);
    }
    Ok(())
}

fn round_up_in_request_len(requested: usize, max_packet_size: usize) -> Result<usize, UsbError> {
    if requested == 0 {
        return Ok(0);
    }
    if max_packet_size == 0 {
        return Err(UsbError::TransferFault);
    }

    let remainder = requested % max_packet_size;
    if remainder == 0 {
        return Ok(requested);
    }

    requested
        .checked_add(max_packet_size - remainder)
        .ok_or(UsbError::InvalidArgument)
}

fn map_control_type(value: ControlType) -> rusb::RequestType {
    match value {
        ControlType::Standard => rusb::RequestType::Standard,
        ControlType::Class => rusb::RequestType::Class,
        ControlType::Vendor => rusb::RequestType::Vendor,
    }
}

fn map_recipient(value: Recipient) -> rusb::Recipient {
    match value {
        Recipient::Device => rusb::Recipient::Device,
        Recipient::InterfaceTarget => rusb::Recipient::Interface,
        Recipient::Endpoint => rusb::Recipient::Endpoint,
        Recipient::Other => rusb::Recipient::Other,
    }
}

fn build_request_type(setup: &ControlSetup, direction: rusb::Direction) -> u8 {
    rusb::request_type(
        direction,
        map_control_type(setup.control_type),
        map_recipient(setup.recipient),
    )
}

fn map_rusb_error(err: rusb::Error) -> UsbError {
    match err {
        rusb::Error::Io => UsbError::TransferFault,
        rusb::Error::InvalidParam => UsbError::InvalidArgument,
        rusb::Error::Access => UsbError::NotAllowed,
        rusb::Error::NoDevice => UsbError::Disconnected,
        rusb::Error::NotFound => UsbError::NotAllowed,
        rusb::Error::Busy => UsbError::Busy,
        rusb::Error::Timeout => UsbError::Timeout,
        rusb::Error::Overflow => UsbError::TransferFault,
        rusb::Error::Pipe => UsbError::TransferFault,
        rusb::Error::Interrupted => UsbError::Timeout,
        rusb::Error::NoMem => UsbError::Other("usb backend out of memory".to_string()),
        rusb::Error::NotSupported => UsbError::OperationNotSupported,
        rusb::Error::BadDescriptor => UsbError::TransferFault,
        rusb::Error::Other => UsbError::Other("usb backend error".to_string()),
    }
}

fn map_libusb_error_code(code: i32) -> UsbError {
    use rusb::ffi::constants::*;

    match code {
        LIBUSB_ERROR_IO => UsbError::TransferFault,
        LIBUSB_ERROR_INVALID_PARAM => UsbError::InvalidArgument,
        LIBUSB_ERROR_ACCESS => UsbError::NotAllowed,
        LIBUSB_ERROR_NO_DEVICE => UsbError::Disconnected,
        LIBUSB_ERROR_NOT_FOUND => UsbError::NotAllowed,
        LIBUSB_ERROR_BUSY => UsbError::Busy,
        LIBUSB_ERROR_TIMEOUT => UsbError::Timeout,
        LIBUSB_ERROR_OVERFLOW => UsbError::TransferFault,
        LIBUSB_ERROR_PIPE => UsbError::TransferFault,
        LIBUSB_ERROR_INTERRUPTED => UsbError::Timeout,
        LIBUSB_ERROR_NO_MEM => UsbError::Other("usb backend out of memory".to_string()),
        LIBUSB_ERROR_NOT_SUPPORTED => UsbError::OperationNotSupported,
        LIBUSB_ERROR_OTHER => UsbError::Other(format!("libusb error code: {code}")),
        _ => UsbError::Other(format!("libusb error code: {code}")),
    }
}

fn map_version(version: rusb::Version) -> VersionRecord {
    VersionRecord {
        major: version.major(),
        minor: version.minor(),
        subminor: version.sub_minor(),
    }
}

fn map_direction(direction: rusb::Direction) -> DirectionRecord {
    match direction {
        rusb::Direction::In => DirectionRecord::In,
        rusb::Direction::Out => DirectionRecord::Out,
    }
}

fn map_usage_type(usage_type: rusb::UsageType) -> UsageTypeRecord {
    match usage_type {
        rusb::UsageType::Data => UsageTypeRecord::Data,
        rusb::UsageType::Feedback => UsageTypeRecord::Feedback,
        rusb::UsageType::FeedbackData => UsageTypeRecord::FeedbackData,
        rusb::UsageType::Reserved => UsageTypeRecord::Reserved,
    }
}

fn map_sync_type(sync_type: rusb::SyncType) -> SyncTypeRecord {
    match sync_type {
        rusb::SyncType::NoSync => SyncTypeRecord::NoSync,
        rusb::SyncType::Asynchronous => SyncTypeRecord::Asynchronous,
        rusb::SyncType::Adaptive => SyncTypeRecord::Adaptive,
        rusb::SyncType::Synchronous => SyncTypeRecord::Synchronous,
    }
}

fn map_transfer_type(transfer_type: rusb::TransferType) -> TransferTypeRecord {
    match transfer_type {
        rusb::TransferType::Control => TransferTypeRecord::Control,
        rusb::TransferType::Isochronous => TransferTypeRecord::Isochronous,
        rusb::TransferType::Bulk => TransferTypeRecord::Bulk,
        rusb::TransferType::Interrupt => TransferTypeRecord::Interrupt,
    }
}

fn map_endpoint_descriptor(endpoint: &rusb::EndpointDescriptor<'_>) -> EndpointDescriptorRecord {
    EndpointDescriptorRecord {
        address: endpoint.address(),
        direction: map_direction(endpoint.direction()),
        interval: endpoint.interval(),
        max_packet_size: endpoint.max_packet_size(),
        number: endpoint.number(),
        refresh: endpoint.refresh(),
        sync_type: map_sync_type(endpoint.sync_type()),
        synch_address: endpoint.synch_address(),
        transfer_type: map_transfer_type(endpoint.transfer_type()),
        usage_type: map_usage_type(endpoint.usage_type()),
    }
}

fn map_interface_descriptor(
    descriptor: &rusb::InterfaceDescriptor<'_>,
) -> InterfaceDescriptorRecord {
    let endpoint_descriptors = descriptor
        .endpoint_descriptors()
        .map(|endpoint| map_endpoint_descriptor(&endpoint))
        .collect();

    InterfaceDescriptorRecord {
        number: descriptor.interface_number(),
        alternate_setting: descriptor.setting_number(),
        class_code: descriptor.class_code(),
        subclass_code: descriptor.sub_class_code(),
        protocol: descriptor.protocol_code(),
        interface_string_index: descriptor.description_string_index(),
        endpoint_descriptors,
    }
}

fn map_configuration_descriptor(
    descriptor: &rusb::ConfigDescriptor,
) -> ConfigurationDescriptorRecord {
    let interfaces = descriptor
        .interfaces()
        .flat_map(|interface| interface.descriptors())
        .map(|interface_descriptor| map_interface_descriptor(&interface_descriptor))
        .collect();

    ConfigurationDescriptorRecord {
        max_power: descriptor.max_power(),
        number: descriptor.number(),
        interfaces,
    }
}

fn map_device_descriptor(descriptor: &rusb::DeviceDescriptor) -> DeviceDescriptorRecord {
    DeviceDescriptorRecord {
        device_class: descriptor.class_code(),
        device_protocol: descriptor.protocol_code(),
        device_subclass: descriptor.sub_class_code(),
        device_version: map_version(descriptor.device_version()),
        product_id: descriptor.product_id(),
        usb_version: map_version(descriptor.usb_version()),
        vendor_id: descriptor.vendor_id(),
        max_packet_size: descriptor.max_packet_size(),
        manufacturer_string_index: descriptor.manufacturer_string_index(),
        product_string_index: descriptor.product_string_index(),
        serial_number_string_index: descriptor.serial_number_string_index(),
        num_configurations: descriptor.num_configurations(),
    }
}

fn usbfs_path(bus: u8, address: u8) -> String {
    format!("/dev/bus/usb/{bus:03}/{address:03}")
}

fn parse_usbfs_bus_and_address(path: &str) -> Result<(u8, u8), String> {
    let normalized = normalize_usb_path(path)?;
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());

    let Some(first) = parts.next() else {
        return Err("usb path must not be root".to_string());
    };
    let Some(second) = parts.next() else {
        return Err("usb path must contain /dev/bus/usb/<bus>/<address>".to_string());
    };
    let Some(third) = parts.next() else {
        return Err("usb path must contain /dev/bus/usb/<bus>/<address>".to_string());
    };
    let Some(bus_str) = parts.next() else {
        return Err("usb path must contain /dev/bus/usb/<bus>/<address>".to_string());
    };
    let Some(address_str) = parts.next() else {
        return Err("usb path must contain /dev/bus/usb/<bus>/<address>".to_string());
    };

    if parts.next().is_some() || first != "dev" || second != "bus" || third != "usb" {
        return Err("usb path must match /dev/bus/usb/<bus>/<address>".to_string());
    }

    let bus = bus_str
        .parse::<u16>()
        .map_err(|_| format!("invalid usb bus number in path: {path}"))?;
    let address = address_str
        .parse::<u16>()
        .map_err(|_| format!("invalid usb address in path: {path}"))?;

    let bus =
        u8::try_from(bus).map_err(|_| format!("usb bus number out of range in path: {path}"))?;
    let address =
        u8::try_from(address).map_err(|_| format!("usb address out of range in path: {path}"))?;

    if bus == 0 || address == 0 {
        return Err("usb bus and address must be non-zero".to_string());
    }

    Ok((bus, address))
}

fn enumerate_openable_devices(
    allowlist: &BTreeSet<String>,
) -> Result<Vec<OpenableDevice>, UsbError> {
    let context = rusb::Context::new().map_err(map_rusb_error)?;
    let device_list = context.devices().map_err(map_rusb_error)?;

    let mut devices = Vec::new();
    for device in device_list.iter() {
        let path = usbfs_path(device.bus_number(), device.address());
        if !allowlist.contains(&path) {
            continue;
        }

        let descriptor = device.device_descriptor().map_err(map_rusb_error)?;
        devices.push(OpenableDevice {
            path,
            bus: device.bus_number(),
            address: device.address(),
            vendor_id: descriptor.vendor_id(),
            product_id: descriptor.product_id(),
        });
    }

    Ok(devices)
}

fn max_packet_size_for_endpoint(
    state: &DeviceThreadState,
    interface: u8,
    endpoint: u8,
) -> Option<usize> {
    let current_alt = state.alt_settings.get(&interface).copied().unwrap_or(0);
    let active = state.handle.active_configuration().ok()?;
    let device = state.handle.device();

    let device_descriptor = device.device_descriptor().ok()?;
    for config_index in 0..device_descriptor.num_configurations() {
        let config = device.config_descriptor(config_index).ok()?;
        if config.number() != active {
            continue;
        }

        for usb_interface in config.interfaces() {
            if usb_interface.number() != interface {
                continue;
            }

            for descriptor in usb_interface.descriptors() {
                if descriptor.setting_number() != current_alt {
                    continue;
                }
                for endpoint_descriptor in descriptor.endpoint_descriptors() {
                    if endpoint_descriptor.address() == endpoint {
                        return Some(usize::from(endpoint_descriptor.max_packet_size()));
                    }
                }
            }
        }
    }

    None
}

fn open_handle_for_bus_address(
    bus: u8,
    address: u8,
) -> Result<rusb::DeviceHandle<rusb::Context>, UsbError> {
    let context = rusb::Context::new().map_err(map_rusb_error)?;
    let device_list = context.devices().map_err(map_rusb_error)?;

    for device in device_list.iter() {
        if device.bus_number() == bus && device.address() == address {
            return device.open().map_err(map_rusb_error);
        }
    }

    Err(UsbError::Disconnected)
}

fn ensure_interface_claimed(state: &DeviceThreadState, interface: u8) -> Result<(), UsbError> {
    if !state.claimed_interfaces.contains(&interface) {
        return Err(UsbError::InvalidArgument);
    }
    Ok(())
}

fn perform_iso_transfer(
    state: &DeviceThreadState,
    endpoint: u8,
    mut buffer: Vec<u8>,
    packets: u16,
    timeout: Duration,
) -> Result<(usize, Vec<u8>), UsbError> {
    use rusb::ffi;
    use rusb::ffi::constants::*;

    validate_iso_packets(packets)?;
    let packet_lengths = compute_iso_packet_lengths(buffer.len(), packets)?;

    let packet_count = i32::from(packets);
    let total_len = i32::try_from(buffer.len()).map_err(|_| UsbError::InvalidArgument)?;
    let timeout_ms = duration_to_libusb_timeout_ms(timeout)?;

    let transfer = unsafe { ffi::libusb_alloc_transfer(packet_count) };
    if transfer.is_null() {
        return Err(UsbError::Other(
            "libusb_alloc_transfer returned null".to_string(),
        ));
    }

    let completed = Box::new(AtomicBool::new(false));
    let completed_ptr = Box::into_raw(completed);

    extern "system" fn transfer_cb(transfer: *mut ffi::libusb_transfer) {
        if transfer.is_null() {
            return;
        }
        let completed = unsafe { (*transfer).user_data.cast::<AtomicBool>() };
        if !completed.is_null() {
            unsafe { (*completed).store(true, Ordering::SeqCst) };
        }
    }

    unsafe {
        ffi::libusb_fill_iso_transfer(
            transfer,
            state.handle.as_raw(),
            endpoint,
            buffer.as_mut_ptr(),
            total_len,
            packet_count,
            transfer_cb,
            completed_ptr.cast::<c_void>(),
            timeout_ms,
        );

        // Distribute remainder bytes across packet descriptors so scheduled packet
        // lengths always sum to the requested transfer length.
        let descriptor_ptr = std::ptr::addr_of_mut!((*transfer).iso_packet_desc)
            .cast::<ffi::libusb_iso_packet_descriptor>();
        for (index, length) in packet_lengths.iter().copied().enumerate() {
            (*descriptor_ptr.add(index)).length = length;
        }
    }

    let submit_result = unsafe { ffi::libusb_submit_transfer(transfer) };
    if submit_result != 0 {
        unsafe {
            drop(Box::from_raw(completed_ptr));
            ffi::libusb_free_transfer(transfer);
        }
        return Err(map_libusb_error_code(submit_result));
    }

    let deadline = Instant::now() + timeout;
    let context = state.handle.context().clone();
    let mut timed_out = false;
    let mut event_error_code = None;

    while !unsafe { (*completed_ptr).load(Ordering::Acquire) } {
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            break;
        }

        let remaining = deadline.saturating_duration_since(now);
        let wait = remaining.min(Duration::from_millis(20));
        let tv = libc::timeval {
            tv_sec: wait.as_secs() as libc::time_t,
            tv_usec: wait.subsec_micros() as libc::suseconds_t,
        };

        let rc = unsafe {
            ffi::libusb_handle_events_timeout_completed(context.as_raw(), &tv, std::ptr::null_mut())
        };
        if rc < 0 && rc != LIBUSB_ERROR_INTERRUPTED {
            event_error_code = Some(rc);
            break;
        }
    }

    if timed_out || event_error_code.is_some() {
        let _ = unsafe { ffi::libusb_cancel_transfer(transfer) };

        // libusb_cancel_transfer is asynchronous. Keep pumping events until callback completion
        // before freeing transfer-owned buffers to avoid use-after-free.
        while !unsafe { (*completed_ptr).load(Ordering::Acquire) } {
            let tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 10_000,
            };
            let rc = unsafe {
                ffi::libusb_handle_events_timeout_completed(
                    context.as_raw(),
                    &tv,
                    std::ptr::null_mut(),
                )
            };
            if rc < 0 && rc != LIBUSB_ERROR_INTERRUPTED {
                event_error_code.get_or_insert(rc);
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    let (status, actual_len) = unsafe {
        let status = (*transfer).status;
        let actual_len = usize::try_from((*transfer).actual_length.max(0)).unwrap_or(0);
        (status, actual_len)
    };

    unsafe {
        drop(Box::from_raw(completed_ptr));
        ffi::libusb_free_transfer(transfer);
    }

    if timed_out {
        return Err(UsbError::Timeout);
    }
    if let Some(code) = event_error_code {
        return Err(map_libusb_error_code(code));
    }

    let result = match status {
        LIBUSB_TRANSFER_COMPLETED => Ok(actual_len),
        LIBUSB_TRANSFER_CANCELLED | LIBUSB_TRANSFER_TIMED_OUT => Err(UsbError::Timeout),
        LIBUSB_TRANSFER_NO_DEVICE => Err(UsbError::Disconnected),
        LIBUSB_TRANSFER_STALL | LIBUSB_TRANSFER_ERROR | LIBUSB_TRANSFER_OVERFLOW => {
            Err(UsbError::TransferFault)
        }
        _ => Err(UsbError::Other(format!(
            "iso transfer status code: {status}"
        ))),
    }?;

    if endpoint & 0x80 != 0 {
        buffer.truncate(result);
    }

    Ok((result, buffer))
}

fn run_device_thread(
    bus: u8,
    address: u8,
    limits: UsbLimitsConfig,
    mut receiver: mpsc::Receiver<DeviceCommand>,
    ready: oneshot::Sender<Result<(), UsbError>>,
) {
    let handle = match open_handle_for_bus_address(bus, address) {
        Ok(handle) => handle,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };

    let mut state = DeviceThreadState {
        handle,
        claimed_interfaces: BTreeSet::new(),
        alt_settings: BTreeMap::new(),
        limits,
    };

    let _ = ready.send(Ok(()));

    while let Some(command) = receiver.blocking_recv() {
        match command {
            DeviceCommand::ClaimInterface { number, reply } => {
                let result = state
                    .handle
                    .set_auto_detach_kernel_driver(true)
                    .or_else(|err| {
                        if err == rusb::Error::NotSupported {
                            Ok(())
                        } else {
                            Err(err)
                        }
                    })
                    .and_then(|_| state.handle.claim_interface(number))
                    .map(|_| {
                        state.claimed_interfaces.insert(number);
                        state.alt_settings.entry(number).or_insert(0);
                    })
                    .map_err(map_rusb_error);
                let _ = reply.send(result);
            }
            DeviceCommand::ReleaseInterface { number, reply } => {
                let result = if state.claimed_interfaces.contains(&number) {
                    state
                        .handle
                        .release_interface(number)
                        .map(|_| {
                            state.claimed_interfaces.remove(&number);
                            state.alt_settings.remove(&number);
                        })
                        .map_err(map_rusb_error)
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
            }
            DeviceCommand::ReleaseInterfaceNoReply { number } => {
                if state.claimed_interfaces.contains(&number) {
                    let _ = state.handle.release_interface(number);
                    state.claimed_interfaces.remove(&number);
                    state.alt_settings.remove(&number);
                }
            }
            DeviceCommand::DeviceDescriptor { reply } => {
                let result = state
                    .handle
                    .device()
                    .device_descriptor()
                    .map(|descriptor| map_device_descriptor(&descriptor))
                    .map_err(map_rusb_error);
                let _ = reply.send(result);
            }
            DeviceCommand::Configurations { reply } => {
                let result = (|| {
                    let device = state.handle.device();
                    let descriptor = device.device_descriptor().map_err(map_rusb_error)?;
                    let mut configs = Vec::new();
                    for index in 0..descriptor.num_configurations() {
                        let config = device.config_descriptor(index).map_err(map_rusb_error)?;
                        configs.push(map_configuration_descriptor(&config));
                    }
                    Ok(configs)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::Reset { reply } => {
                let result = state.handle.reset().map_err(map_rusb_error);
                let _ = reply.send(result);
            }
            DeviceCommand::ActiveConfiguration { reply } => {
                let result = state.handle.active_configuration().map_err(map_rusb_error);
                let _ = reply.send(result);
            }
            DeviceCommand::SelectConfiguration {
                configuration,
                reply,
            } => {
                let result = state
                    .handle
                    .set_active_configuration(configuration)
                    .map_err(map_rusb_error);
                let _ = reply.send(result);
            }
            DeviceCommand::AlternateSetting { interface, reply } => {
                let result = ensure_interface_claimed(&state, interface)
                    .map(|_| state.alt_settings.get(&interface).copied().unwrap_or(0));
                let _ = reply.send(result);
            }
            DeviceCommand::SetAlternateSetting {
                interface,
                setting,
                reply,
            } => {
                let result = ensure_interface_claimed(&state, interface)
                    .and_then(|_| {
                        state
                            .handle
                            .set_alternate_setting(interface, setting)
                            .map_err(map_rusb_error)
                    })
                    .map(|_| {
                        state.alt_settings.insert(interface, setting);
                    });
                let _ = reply.send(result);
            }
            DeviceCommand::ControlIn {
                interface,
                setup,
                length,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    let request_len = validate_transfer_len(length, &state.limits)?;
                    if request_len > usize::from(u16::MAX) {
                        return Err(UsbError::InvalidArgument);
                    }

                    let mut data = vec![0u8; request_len];
                    let request_type = build_request_type(&setup, rusb::Direction::In);
                    let read = state
                        .handle
                        .read_control(
                            request_type,
                            setup.request,
                            setup.value,
                            setup.index,
                            &mut data,
                            timeout,
                        )
                        .map_err(map_rusb_error)?;
                    data.truncate(read);
                    Ok(data)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::ControlOut {
                interface,
                setup,
                data,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    validate_transfer_data_len(data.len(), &state.limits)?;
                    if data.len() > usize::from(u16::MAX) {
                        return Err(UsbError::InvalidArgument);
                    }

                    let request_type = build_request_type(&setup, rusb::Direction::Out);
                    let _ = state
                        .handle
                        .write_control(
                            request_type,
                            setup.request,
                            setup.value,
                            setup.index,
                            &data,
                            timeout,
                        )
                        .map_err(map_rusb_error)?;
                    Ok(())
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::BulkIn {
                interface,
                endpoint,
                length,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_in_address(endpoint)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    let requested_len = validate_transfer_len(length, &state.limits)?;
                    if requested_len == 0 {
                        return Ok(Vec::new());
                    }

                    let max_packet_size = max_packet_size_for_endpoint(&state, interface, endpoint)
                        .unwrap_or(requested_len);
                    let padded_len = round_up_in_request_len(requested_len, max_packet_size)?;
                    let mut buffer = vec![0u8; padded_len];
                    let read = state
                        .handle
                        .read_bulk(endpoint, &mut buffer, timeout)
                        .map_err(map_rusb_error)?;
                    buffer.truncate(read.min(requested_len));
                    Ok(buffer)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::BulkOut {
                interface,
                endpoint,
                data,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_out_address(endpoint)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    validate_transfer_data_len(data.len(), &state.limits)?;
                    let _ = state
                        .handle
                        .write_bulk(endpoint, &data, timeout)
                        .map_err(map_rusb_error)?;
                    Ok(())
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::InterruptIn {
                interface,
                endpoint,
                length,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_in_address(endpoint)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    let requested_len = validate_transfer_len(length, &state.limits)?;
                    if requested_len == 0 {
                        return Ok(Vec::new());
                    }

                    let max_packet_size = max_packet_size_for_endpoint(&state, interface, endpoint)
                        .unwrap_or(requested_len);
                    let padded_len = round_up_in_request_len(requested_len, max_packet_size)?;
                    let mut buffer = vec![0u8; padded_len];
                    let read = state
                        .handle
                        .read_interrupt(endpoint, &mut buffer, timeout)
                        .map_err(map_rusb_error)?;
                    buffer.truncate(read.min(requested_len));
                    Ok(buffer)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::InterruptOut {
                interface,
                endpoint,
                data,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_out_address(endpoint)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    validate_transfer_data_len(data.len(), &state.limits)?;
                    let _ = state
                        .handle
                        .write_interrupt(endpoint, &data, timeout)
                        .map_err(map_rusb_error)?;
                    Ok(())
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::IsochronousIn {
                interface,
                endpoint,
                length,
                packets,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_in_address(endpoint)?;
                    validate_iso_packets(packets)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    let request_len = validate_transfer_len(length, &state.limits)?;
                    if request_len == 0 {
                        return Ok(Vec::new());
                    }

                    let buffer = vec![0u8; request_len];
                    let (actual_len, mut data) =
                        perform_iso_transfer(&state, endpoint, buffer, packets, timeout)?;
                    data.truncate(actual_len.min(request_len));
                    Ok(data)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::IsochronousOut {
                interface,
                endpoint,
                data,
                packets,
                timeout_ms,
                reply,
            } => {
                let result = (|| {
                    ensure_interface_claimed(&state, interface)?;
                    validate_endpoint_out_address(endpoint)?;
                    validate_iso_packets(packets)?;
                    let timeout = validate_timeout(timeout_ms, &state.limits)?;
                    validate_transfer_data_len(data.len(), &state.limits)?;
                    let (actual_len, _) =
                        perform_iso_transfer(&state, endpoint, data, packets, timeout)?;
                    let actual_len =
                        u32::try_from(actual_len).map_err(|_| UsbError::InvalidArgument)?;
                    Ok(actual_len)
                })();
                let _ = reply.send(result);
            }
            DeviceCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

async fn start_device_runtime(
    path: String,
    bus: u8,
    address: u8,
    limits: UsbLimitsConfig,
) -> Result<DeviceRuntimeHandle, UsbError> {
    let (sender, receiver) = mpsc::channel::<DeviceCommand>(DEVICE_COMMAND_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = oneshot::channel::<Result<(), UsbError>>();
    let thread_limits = limits.clone();

    let thread_name = format!("imago-usb-dev-{bus:03}-{address:03}");
    let thread_handle = thread::Builder::new()
        .name(thread_name)
        .stack_size(DEFAULT_THREAD_STACK_BYTES)
        .spawn(move || run_device_thread(bus, address, thread_limits, receiver, ready_tx))
        .map_err(|err| UsbError::Other(format!("failed to spawn usb thread: {err}")))?;

    match ready_rx.await {
        Ok(Ok(())) => Ok(DeviceRuntimeHandle {
            path,
            sender,
            thread_handle: Arc::new(Mutex::new(Some(thread_handle))),
        }),
        Ok(Err(err)) => {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = thread_handle.join();
            })
            .await;
            Err(err)
        }
        Err(_) => {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = thread_handle.join();
            })
            .await;
            Err(UsbError::Disconnected)
        }
    }
}

async fn request_device<T>(
    sender: &mpsc::Sender<DeviceCommand>,
    build: impl FnOnce(oneshot::Sender<Result<T, UsbError>>) -> DeviceCommand,
) -> Result<T, UsbError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    sender
        .try_send(build(reply_tx))
        .map_err(map_channel_send_error)?;

    reply_rx.await.unwrap_or(Err(UsbError::Disconnected))
}

fn map_channel_send_error<T>(err: mpsc::error::TrySendError<T>) -> UsbError {
    match err {
        mpsc::error::TrySendError::Full(_) => UsbError::Busy,
        mpsc::error::TrySendError::Closed(_) => UsbError::Disconnected,
    }
}

async fn send_shutdown_command(
    sender: &mpsc::Sender<DeviceCommand>,
    reply: oneshot::Sender<()>,
) -> bool {
    match sender.try_send(DeviceCommand::Shutdown { reply }) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
        Err(mpsc::error::TrySendError::Full(command)) => sender.send(command).await.is_ok(),
    }
}

async fn send_release_interface_no_reply_command(sender: &mpsc::Sender<DeviceCommand>, number: u8) {
    match sender.try_send(DeviceCommand::ReleaseInterfaceNoReply { number }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => {}
        Err(mpsc::error::TrySendError::Full(command)) => {
            let _ = sender.send(command).await;
        }
    }
}

async fn shutdown_device_runtime(handle: &DeviceRuntimeHandle) {
    let (reply_tx, reply_rx) = oneshot::channel();
    let shutdown_sent = send_shutdown_command(&handle.sender, reply_tx).await;
    if shutdown_sent {
        let _ = reply_rx.await;
    }

    let join_handle = handle
        .thread_handle
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());

    if let Some(join_handle) = join_handle {
        let _ = tokio::task::spawn_blocking(move || {
            let _ = join_handle.join();
        })
        .await;
    }
}

fn set_hotplug_init_error_message(
    init_error_message: &Arc<Mutex<Option<String>>>,
    message: String,
) {
    if let Ok(mut guard) = init_error_message.lock()
        && guard.is_none()
    {
        *guard = Some(message);
    }
}

fn read_hotplug_init_error_message(
    init_error_message: &Arc<Mutex<Option<String>>>,
) -> Result<Option<UsbError>, UsbError> {
    let guard = init_error_message
        .lock()
        .map_err(|_| UsbError::Other("hotplug init state lock poisoned".to_string()))?;
    Ok(guard
        .as_ref()
        .map(|message| UsbError::Other(message.clone())))
}

fn create_hotplug_manager(
    cache_key: &str,
    allowlist: BTreeSet<String>,
) -> Result<Arc<HotplugManager>, UsbError> {
    let queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let init_error_message = Arc::new(Mutex::new(None));

    let thread_name = format!("imago-usb-hotplug-{}", cache_key.replace('\u{1f}', "_"));
    let thread_queue = queue.clone();
    let thread_stop = stop.clone();
    let thread_init_error_message = init_error_message.clone();

    let thread_handle = thread::Builder::new()
        .name(thread_name)
        .stack_size(DEFAULT_THREAD_STACK_BYTES)
        .spawn(move || {
            let context = match rusb::Context::new() {
                Ok(context) => context,
                Err(err) => {
                    set_hotplug_init_error_message(
                        &thread_init_error_message,
                        format!("failed to initialize usb hotplug context: {err}"),
                    );
                    return;
                }
            };

            let callback = HotplugCallback {
                allowlist,
                queue: thread_queue,
            };
            let registration = match HotplugBuilder::new()
                .enumerate(false)
                .register(&context, Box::new(callback))
            {
                Ok(registration) => registration,
                Err(err) => {
                    set_hotplug_init_error_message(
                        &thread_init_error_message,
                        format!("failed to register usb hotplug callback: {err}"),
                    );
                    return;
                }
            };

            while !thread_stop.load(Ordering::Acquire) {
                let _ = context.handle_events(Some(Duration::from_millis(100)));
            }

            drop(registration);
        })
        .map_err(|err| UsbError::Other(format!("failed to spawn hotplug thread: {err}")))?;

    Ok(Arc::new(HotplugManager {
        queue,
        stop,
        init_error_message,
        thread_handle: Arc::new(Mutex::new(Some(thread_handle))),
    }))
}

fn get_hotplug_manager_for_state(
    state: &WasiState,
    resources: &UsbResourcesConfig,
) -> Result<Arc<HotplugManager>, UsbError> {
    let context = state.native_plugin_context();
    let cache_key = usb_resources_cache_key(
        context.service_name(),
        context.release_hash(),
        context.runner_id(),
    );

    let mut cache = hotplug_manager_cache()
        .lock()
        .map_err(|_| UsbError::Other("hotplug manager cache lock poisoned".to_string()))?;

    let stale_keys: Vec<_> = cache
        .keys()
        .filter(|key| {
            *key != &cache_key
                && is_hotplug_cache_key_in_scope(key, context.service_name(), context.runner_id())
        })
        .cloned()
        .collect();
    let mut stale_managers = Vec::new();
    for key in stale_keys {
        if let Some(manager) = cache.remove(&key) {
            stale_managers.push(manager);
        }
    }

    if let Some(manager) = cache.get(&cache_key) {
        let manager = manager.clone();
        drop(cache);
        drop(stale_managers);
        return Ok(manager);
    }

    let manager = create_hotplug_manager(&cache_key, resources.allowlist.clone())?;
    cache.insert(cache_key, manager.clone());
    drop(cache);
    drop(stale_managers);
    Ok(manager)
}

fn poll_hotplug_event(
    queue: &Arc<(Mutex<VecDeque<DeviceConnectionEvent>>, Condvar)>,
    timeout: Duration,
) -> Option<DeviceConnectionEvent> {
    let (lock, condvar) = &**queue;
    let mut guard = lock.lock().ok()?;

    if let Some(event) = guard.pop_front() {
        return Some(event);
    }

    if timeout.is_zero() {
        return None;
    }

    let (mut guard, _) = condvar.wait_timeout(guard, timeout).ok()?;
    guard.pop_front()
}

impl imago_usb_plugin_bindings::imago::usb::provider::Host for WasiState {
    async fn list_openable_paths(&mut self) -> Vec<String> {
        load_usb_resources_for_state_or_default(self).paths
    }

    async fn list_openable_devices(&mut self) -> Result<Vec<OpenableDevice>, UsbError> {
        ensure_usb_supported()?;
        let resources = load_usb_resources_for_state(self)?;
        let allowlist = resources.allowlist;

        tokio::task::spawn_blocking(move || enumerate_openable_devices(&allowlist))
            .await
            .map_err(|err| UsbError::Other(format!("usb enumeration task failed: {err}")))?
    }

    async fn poll_device_event(
        &mut self,
        timeout_ms: u32,
    ) -> Result<DeviceConnectionEvent, UsbError> {
        ensure_usb_supported()?;
        let resources = load_usb_resources_for_state(self)?;
        let timeout = validate_poll_timeout(timeout_ms, &resources.limits)?;
        let manager = get_hotplug_manager_for_state(self, &resources)?;
        if let Some(err) = read_hotplug_init_error_message(&manager.init_error_message)? {
            return Err(err);
        }
        let queue = manager.queue.clone();

        let event = tokio::task::spawn_blocking(move || poll_hotplug_event(&queue, timeout))
            .await
            .map_err(|err| UsbError::Other(format!("hotplug polling task failed: {err}")))?;

        if let Some(event) = event {
            return Ok(event);
        }
        if let Some(err) = read_hotplug_init_error_message(&manager.init_error_message)? {
            return Err(err);
        }

        Ok(DeviceConnectionEvent::Pending)
    }

    async fn get_limits(&mut self) -> Limits {
        let resources = load_usb_resources_for_state_or_default(self);
        to_limits_record(&resources.limits)
    }

    async fn open_device(&mut self, path: String) -> Result<Resource<DeviceResource>, UsbError> {
        ensure_usb_supported()?;

        let resources = load_usb_resources_for_state(self)?;
        let normalized = normalize_usb_path(&path).map_err(|_| UsbError::InvalidArgument)?;
        if !resources.allowlist.contains(&normalized) {
            return Err(UsbError::NotAllowed);
        }

        let (bus, address) =
            parse_usbfs_bus_and_address(&normalized).map_err(|_| UsbError::InvalidArgument)?;
        let runtime_handle =
            start_device_runtime(normalized.clone(), bus, address, resources.limits).await?;

        let rep = register_device_handle(runtime_handle).map_err(map_lookup_error)?;
        Ok(Resource::new_own(rep))
    }
}

impl imago_usb_plugin_bindings::imago::usb::types::Host for WasiState {}

impl imago_usb_plugin_bindings::imago::usb::device::Host for WasiState {}

impl imago_usb_plugin_bindings::imago::usb::device::HostDevice for WasiState {
    async fn path(&mut self, self_: Resource<DeviceResource>) -> String {
        lookup_device_handle(self_.rep())
            .map(|handle| handle.path)
            .unwrap_or_default()
    }

    async fn device_descriptor(
        &mut self,
        self_: Resource<DeviceResource>,
    ) -> Result<DeviceDescriptorRecord, UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::DeviceDescriptor {
            reply,
        })
        .await
    }

    async fn configurations(
        &mut self,
        self_: Resource<DeviceResource>,
    ) -> Result<Vec<ConfigurationDescriptorRecord>, UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::Configurations {
            reply,
        })
        .await
    }

    async fn reset(&mut self, self_: Resource<DeviceResource>) -> Result<(), UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::Reset { reply }).await
    }

    async fn active_configuration(
        &mut self,
        self_: Resource<DeviceResource>,
    ) -> Result<u8, UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::ActiveConfiguration {
            reply,
        })
        .await
    }

    async fn select_configuration(
        &mut self,
        self_: Resource<DeviceResource>,
        configuration: u8,
    ) -> Result<(), UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::SelectConfiguration {
            configuration,
            reply,
        })
        .await
    }

    async fn claim_interface(
        &mut self,
        self_: Resource<DeviceResource>,
        number: u8,
    ) -> Result<Resource<ClaimedInterfaceResource>, UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::ClaimInterface {
            number,
            reply,
        })
        .await?;

        let rep = register_claimed_interface_handle(ClaimedInterfaceHandle {
            number,
            sender: handle.sender,
        })
        .map_err(map_lookup_error)?;
        Ok(Resource::new_own(rep))
    }

    async fn release_interface(
        &mut self,
        self_: Resource<DeviceResource>,
        number: u8,
    ) -> Result<(), UsbError> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::ReleaseInterface {
            number,
            reply,
        })
        .await
    }

    async fn drop(&mut self, resource: Resource<DeviceResource>) -> wasmtime::Result<()> {
        if let Ok(handle) = lookup_device_handle(resource.rep()) {
            let _ = remove_claimed_interface_handles_for_sender(&handle.sender);
            shutdown_device_runtime(&handle).await;
        }
        remove_device_handle(resource.rep());
        Ok(())
    }
}

impl imago_usb_plugin_bindings::imago::usb::usb_interface::Host for WasiState {}

impl imago_usb_plugin_bindings::imago::usb::usb_interface::HostClaimedInterface for WasiState {
    async fn number(&mut self, self_: Resource<ClaimedInterfaceResource>) -> u8 {
        lookup_claimed_interface_handle(self_.rep())
            .map(|handle| handle.number)
            .unwrap_or(0)
    }

    async fn alternate_setting(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
    ) -> Result<u8, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::AlternateSetting {
            interface: handle.number,
            reply,
        })
        .await
    }

    async fn set_alternate_setting(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        setting: u8,
    ) -> Result<(), UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::SetAlternateSetting {
            interface: handle.number,
            setting,
            reply,
        })
        .await
    }

    async fn control_in(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        setup: ControlSetup,
        length: u32,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::ControlIn {
            interface: handle.number,
            setup,
            length,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn control_out(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        setup: ControlSetup,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<(), UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::ControlOut {
            interface: handle.number,
            setup,
            data,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn bulk_in(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        length: u32,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::BulkIn {
            interface: handle.number,
            endpoint,
            length,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn bulk_out(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<(), UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::BulkOut {
            interface: handle.number,
            endpoint,
            data,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn interrupt_in(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        length: u32,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::InterruptIn {
            interface: handle.number,
            endpoint,
            length,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn interrupt_out(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<(), UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::InterruptOut {
            interface: handle.number,
            endpoint,
            data,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn isochronous_in(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        length: u32,
        packets: u16,
        timeout_ms: u32,
    ) -> Result<Vec<u8>, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::IsochronousIn {
            interface: handle.number,
            endpoint,
            length,
            packets,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn isochronous_out(
        &mut self,
        self_: Resource<ClaimedInterfaceResource>,
        endpoint: u8,
        data: Vec<u8>,
        packets: u16,
        timeout_ms: u32,
    ) -> Result<u32, UsbError> {
        let handle = lookup_claimed_interface_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::IsochronousOut {
            interface: handle.number,
            endpoint,
            data,
            packets,
            timeout_ms,
            reply,
        })
        .await
    }

    async fn drop(&mut self, resource: Resource<ClaimedInterfaceResource>) -> wasmtime::Result<()> {
        if let Ok(handle) = lookup_claimed_interface_handle(resource.rep()) {
            send_release_interface_no_reply_command(&handle.sender, handle.number).await;
        }
        remove_claimed_interface_handle(resource.rep());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resources_with_usb(value: JsonValue) -> BTreeMap<String, JsonValue> {
        BTreeMap::from([(USB_RESOURCE_KEY.to_string(), value)])
    }

    #[test]
    fn normalize_usb_path_requires_absolute_path() {
        let err = normalize_usb_path("dev/bus/usb/001/001").expect_err("relative path must fail");
        assert!(err.contains("absolute"), "unexpected error: {err}");
    }

    #[test]
    fn normalize_usb_path_rejects_empty_or_nul() {
        let err = normalize_usb_path(" ").expect_err("empty path must fail");
        assert!(err.contains("must not be empty"), "unexpected error: {err}");

        let err = normalize_usb_path("/dev/\0usb").expect_err("NUL path must fail");
        assert!(err.contains("NUL"), "unexpected error: {err}");
    }

    #[test]
    fn parse_usbfs_bus_and_address_accepts_valid_path() {
        let parsed =
            parse_usbfs_bus_and_address("/dev/bus/usb/001/042").expect("valid path should parse");
        assert_eq!(parsed, (1, 42));
    }

    #[test]
    fn parse_usbfs_bus_and_address_rejects_invalid_path() {
        let err =
            parse_usbfs_bus_and_address("/dev/bus/usb/001").expect_err("invalid path must fail");
        assert!(err.contains("/dev/bus/usb"), "unexpected error: {err}");
    }

    #[test]
    fn parse_usb_resources_requires_usb_table() {
        let err = parse_usb_resources_config(&BTreeMap::new())
            .expect_err("missing usb resource should fail");
        assert!(
            err.contains("resources.usb is required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_usb_resources_requires_paths_array() {
        let err = parse_usb_resources_config(&resources_with_usb(json!({})))
            .expect_err("missing paths should fail");
        assert!(err.contains("paths is required"), "unexpected error: {err}");

        let err = parse_usb_resources_config(&resources_with_usb(json!({ "paths": "x" })))
            .expect_err("non-array paths should fail");
        assert!(err.contains("must be an array"), "unexpected error: {err}");
    }

    #[test]
    fn parse_usb_resources_rejects_duplicate_paths_after_normalization() {
        let err = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": [
                "/dev/bus/usb/001/001",
                "/dev/bus/usb/001/./001"
            ]
        })))
        .expect_err("normalized duplicates should fail");
        assert!(
            err.contains("duplicates normalized path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_usb_resources_accepts_empty_allowlist() {
        let config = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": []
        })))
        .expect("empty allowlist should be valid");
        assert!(config.paths.is_empty());
        assert!(config.allowlist.is_empty());
        assert_eq!(config.limits, UsbLimitsConfig::default());
    }

    #[test]
    fn parse_usb_resources_applies_limit_defaults() {
        let config = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": ["/dev/bus/usb/001/001"]
        })))
        .expect("default limits should apply");

        assert_eq!(config.limits.max_transfer_bytes, DEFAULT_MAX_TRANSFER_BYTES);
        assert_eq!(config.limits.max_timeout_ms, DEFAULT_MAX_TIMEOUT_MS);
        assert_eq!(config.limits.max_paths, DEFAULT_MAX_PATHS);
    }

    #[test]
    fn parse_usb_resources_applies_custom_limits() {
        let config = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": ["/dev/bus/usb/001/001"],
            "max_transfer_bytes": 65536,
            "max_timeout_ms": 5000,
            "max_paths": 8
        })))
        .expect("custom limits should parse");

        assert_eq!(config.limits.max_transfer_bytes, 65536);
        assert_eq!(config.limits.max_timeout_ms, 5000);
        assert_eq!(config.limits.max_paths, 8);
    }

    #[test]
    fn parse_usb_resources_rejects_out_of_range_limits() {
        let err = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": ["/dev/bus/usb/001/001"],
            "max_transfer_bytes": 0
        })))
        .expect_err("zero transfer limit must fail");
        assert!(
            err.contains("max_transfer_bytes"),
            "unexpected error: {err}"
        );

        let err = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": ["/dev/bus/usb/001/001"],
            "max_timeout_ms": 0
        })))
        .expect_err("zero timeout must fail");
        assert!(err.contains("max_timeout_ms"), "unexpected error: {err}");

        let err = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": ["/dev/bus/usb/001/001"],
            "max_paths": 999
        })))
        .expect_err("oversized max_paths must fail");
        assert!(err.contains("max_paths"), "unexpected error: {err}");
    }

    #[test]
    fn parse_usb_resources_rejects_path_count_exceeding_limit() {
        let err = parse_usb_resources_config(&resources_with_usb(json!({
            "paths": [
                "/dev/bus/usb/001/001",
                "/dev/bus/usb/001/002"
            ],
            "max_paths": 1
        })))
        .expect_err("exceeding max_paths must fail");
        assert!(err.contains("exceeds max_paths"), "unexpected error: {err}");
    }

    #[test]
    fn validate_timeout_enforces_bounds() {
        let limits = UsbLimitsConfig::default();
        assert!(validate_timeout(1, &limits).is_ok());
        assert!(validate_timeout(0, &limits).is_err());
        assert!(validate_timeout(limits.max_timeout_ms + 1, &limits).is_err());
    }

    #[test]
    fn duration_to_libusb_timeout_ms_rejects_overflow() {
        assert_eq!(
            duration_to_libusb_timeout_ms(Duration::from_millis(1_000))
                .expect("timeout should fit"),
            1_000
        );

        let overflow = Duration::from_millis(u64::from(u32::MAX) + 1);
        assert!(duration_to_libusb_timeout_ms(overflow).is_err());
    }

    #[test]
    fn compute_iso_packet_lengths_distributes_remainder_bytes() {
        let lengths = compute_iso_packet_lengths(1025, 8).expect("packet lengths should compute");
        assert_eq!(lengths.len(), 8);
        assert_eq!(lengths[0], 129);
        assert!(lengths.iter().skip(1).all(|len| *len == 128));
        assert_eq!(lengths.iter().map(|len| u64::from(*len)).sum::<u64>(), 1025);
    }

    #[test]
    fn validate_transfer_len_enforces_bounds() {
        let limits = UsbLimitsConfig::default();
        assert_eq!(
            validate_transfer_len(0, &limits).expect("zero should pass"),
            0
        );

        let too_large = u32::try_from(limits.max_transfer_bytes + 1)
            .expect("max transfer bytes should fit in u32");
        assert!(validate_transfer_len(too_large, &limits).is_err());
    }

    #[test]
    fn validate_iso_packets_enforces_bounds() {
        assert!(validate_iso_packets(1).is_ok());
        assert!(validate_iso_packets(MAX_ISO_PACKETS).is_ok());
        assert!(validate_iso_packets(0).is_err());
        assert!(validate_iso_packets(MAX_ISO_PACKETS + 1).is_err());
    }

    #[test]
    fn round_up_in_request_len_matches_packet_boundary() {
        assert_eq!(round_up_in_request_len(64, 64).expect("aligned"), 64);
        assert_eq!(round_up_in_request_len(65, 64).expect("round up"), 128);
        assert_eq!(round_up_in_request_len(0, 64).expect("zero"), 0);
        assert!(round_up_in_request_len(1, 0).is_err());
    }

    #[test]
    fn endpoint_direction_validation_rejects_control_endpoint_zero_for_non_control_transfers() {
        assert!(validate_endpoint_in_address(0x81).is_ok());
        assert!(validate_endpoint_out_address(0x01).is_ok());

        assert!(validate_endpoint_in_address(0x01).is_err());
        assert!(validate_endpoint_out_address(0x81).is_err());
        assert!(validate_endpoint_in_address(0x80).is_err());
        assert!(validate_endpoint_out_address(0x00).is_err());
    }

    #[test]
    fn map_rusb_error_maps_timeout_to_usb_timeout() {
        assert!(matches!(
            map_rusb_error(rusb::Error::Timeout),
            UsbError::Timeout
        ));
    }

    #[test]
    fn channel_send_error_maps_full_to_busy() {
        let (sender, _receiver) = mpsc::channel::<u8>(1);
        sender.try_send(1).expect("first send should pass");
        let err = sender.try_send(2).expect_err("second send should fail");
        assert!(matches!(map_channel_send_error(err), UsbError::Busy));
    }

    #[test]
    fn send_shutdown_command_falls_back_when_queue_is_full() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime must build");

        runtime.block_on(async {
            let (sender, mut receiver) = mpsc::channel::<DeviceCommand>(1);
            sender
                .try_send(DeviceCommand::ReleaseInterfaceNoReply { number: 1 })
                .expect("channel should accept first command");

            let recv_task = tokio::spawn(async move {
                let _ = receiver.recv().await;
                let Some(DeviceCommand::Shutdown { reply }) = receiver.recv().await else {
                    return false;
                };
                let _ = reply.send(());
                true
            });

            let (reply_tx, reply_rx) = oneshot::channel();
            assert!(send_shutdown_command(&sender, reply_tx).await);
            assert!(reply_rx.await.is_ok());
            assert!(recv_task.await.expect("receiver task must complete"));
        });
    }

    #[test]
    fn send_release_interface_no_reply_command_falls_back_when_queue_is_full() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime must build");

        runtime.block_on(async {
            let (sender, mut receiver) = mpsc::channel::<DeviceCommand>(1);
            sender
                .try_send(DeviceCommand::Shutdown {
                    reply: oneshot::channel().0,
                })
                .expect("channel should accept first command");

            let recv_task = tokio::spawn(async move {
                let _ = receiver.recv().await;
                let Some(DeviceCommand::ReleaseInterfaceNoReply { number }) = receiver.recv().await
                else {
                    return None;
                };
                Some(number)
            });

            send_release_interface_no_reply_command(&sender, 7).await;
            assert_eq!(
                recv_task.await.expect("receiver task must complete"),
                Some(7)
            );
        });
    }

    #[test]
    fn cache_key_is_runner_scoped() {
        let key_a = usb_resources_cache_key("svc", "release-a", "runner-1");
        let key_b = usb_resources_cache_key("svc", "release-b", "runner-1");
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn hotplug_cache_scope_matches_service_and_runner() {
        let key = usb_resources_cache_key("svc", "release-a", "runner-1");
        assert!(is_hotplug_cache_key_in_scope(&key, "svc", "runner-1"));
        assert!(!is_hotplug_cache_key_in_scope(&key, "svc", "runner-2"));
        assert!(!is_hotplug_cache_key_in_scope(&key, "other", "runner-1"));
        assert!(!is_hotplug_cache_key_in_scope(
            "invalid-cache-key",
            "svc",
            "runner-1"
        ));
    }

    #[test]
    fn retain_claimed_interfaces_removes_matching_sender() {
        let (sender_a, _receiver_a) = mpsc::channel::<DeviceCommand>(2);
        let (sender_b, _receiver_b) = mpsc::channel::<DeviceCommand>(2);

        let mut registry = BTreeMap::new();
        registry.insert(
            1,
            ClaimedInterfaceHandle {
                number: 1,
                sender: sender_a.clone(),
            },
        );
        registry.insert(
            2,
            ClaimedInterfaceHandle {
                number: 2,
                sender: sender_b.clone(),
            },
        );
        registry.insert(
            3,
            ClaimedInterfaceHandle {
                number: 3,
                sender: sender_a.clone(),
            },
        );

        let removed = retain_claimed_interfaces_for_other_senders(&mut registry, &sender_a);
        assert_eq!(removed, 2);
        assert_eq!(registry.len(), 1);
        assert!(
            registry
                .values()
                .all(|handle| handle.sender.same_channel(&sender_b))
        );
    }

    #[test]
    fn hotplug_manager_drop_sets_stop_and_joins_thread() {
        let queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));

        let thread_stop = stop.clone();
        let thread_exited = exited.clone();
        let thread_handle = thread::Builder::new()
            .name("hotplug-drop-test".to_string())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                thread_exited.store(true, Ordering::Release);
            })
            .expect("thread should spawn");

        let manager = HotplugManager {
            queue,
            stop: stop.clone(),
            init_error_message: Arc::new(Mutex::new(None)),
            thread_handle: Arc::new(Mutex::new(Some(thread_handle))),
        };
        drop(manager);

        assert!(stop.load(Ordering::Acquire));
        assert!(exited.load(Ordering::Acquire));
    }

    #[test]
    fn read_hotplug_init_error_message_returns_error_when_set() {
        let init_error_message = Arc::new(Mutex::new(Some("init failed".to_string())));
        let err = read_hotplug_init_error_message(&init_error_message)
            .expect("lock should succeed")
            .expect("error should exist");
        assert!(matches!(err, UsbError::Other(message) if message == "init failed"));
    }

    #[test]
    fn read_hotplug_init_error_message_returns_none_when_unset() {
        let init_error_message = Arc::new(Mutex::new(None));
        assert!(
            read_hotplug_init_error_message(&init_error_message)
                .expect("lock should succeed")
                .is_none()
        );
    }

    #[test]
    fn hotplug_queue_discards_oldest_entry_when_full() {
        let queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));

        {
            let (lock, _) = &*queue;
            let mut guard = lock.lock().expect("queue lock should work");
            for _ in 0..MAX_HOTPLUG_QUEUE_LEN {
                guard.push_back(DeviceConnectionEvent::Pending);
            }
            if guard.len() >= MAX_HOTPLUG_QUEUE_LEN {
                let _ = guard.pop_front();
            }
            guard.push_back(DeviceConnectionEvent::Pending);
            assert_eq!(guard.len(), MAX_HOTPLUG_QUEUE_LEN);
        }
    }
}
