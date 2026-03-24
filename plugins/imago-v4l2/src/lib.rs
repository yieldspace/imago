#[cfg(any(test, target_os = "linux"))]
use std::cmp::Ordering as CmpOrdering;
#[cfg(any(test, target_os = "linux"))]
use std::fs;
#[cfg(target_os = "linux")]
use std::io::{self, Cursor};
#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(any(test, target_os = "linux"))]
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    thread,
};

use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::WasiState;
use imagod_runtime_wasmtime::native_plugins::{
    HasSelf, NativePlugin, NativePluginLinker, NativePluginResult, map_native_plugin_linker_error,
    map_native_plugin_resource_validation_error,
};
#[cfg(target_os = "linux")]
use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat as JpegPixelFormat};
#[cfg(target_os = "linux")]
use nix::errno::Errno;
use serde_json::{Map as JsonMap, Value as JsonValue};
use tokio::sync::{mpsc, oneshot};
#[cfg(target_os = "linux")]
use v4l::{
    buffer::{Flags as V4lBufferFlags, Type as V4lBufferType},
    capability::{Capabilities as V4lCapabilities, Flags as V4lCapabilityFlags},
    control::{
        Control as V4lControl, Description as V4lControlDescription, Flags as V4lControlFlags,
        Type as V4lControlType, Value as V4lControlValue,
    },
    device::Device as V4lDevice,
    format::{Format as V4lFormat, FourCC as V4lFourCc},
    fraction::Fraction as V4lFraction,
    frameinterval::FrameIntervalEnum as V4lFrameIntervalEnum,
    framesize::FrameSizeEnum as V4lFrameSizeEnum,
    io::{mmap::Stream as V4lMmapStream, traits::CaptureStream as V4lCaptureStream},
    timestamp::Timestamp as V4lTimestamp,
    v4l_sys as v4l_bindings,
    video::{Capture as V4lCaptureDevice, capture::Parameters as V4lCaptureParameters},
};
use wasmtime::component::Resource;

pub mod imago_v4l2_plugin_bindings {
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
pub struct ImagoV4l2Plugin;

impl NativePlugin for ImagoV4l2Plugin {
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
        imago_v4l2_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
            .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
    }

    fn validate_resources(
        &self,
        resources: &BTreeMap<String, JsonValue>,
    ) -> NativePluginResult<()> {
        parse_v4l2_resources_config(resources)
            .map(|_| ())
            .map_err(|message| {
                map_native_plugin_resource_validation_error(Self::PACKAGE_NAME, message)
            })
    }
}

type V4l2Error = imago_v4l2_plugin_bindings::imago::v4l2::types::V4l2Error;
type EncodedFormat = imago_v4l2_plugin_bindings::imago::v4l2::types::EncodedFormat;
#[cfg(target_os = "linux")]
type FramePixelFormat = imago_v4l2_plugin_bindings::imago::v4l2::types::PixelFormat;
type Limits = imago_v4l2_plugin_bindings::imago::v4l2::types::Limits;
type OpenableDevice = imago_v4l2_plugin_bindings::imago::v4l2::types::OpenableDevice;
type CaptureMode = imago_v4l2_plugin_bindings::imago::v4l2::types::CaptureMode;
type CaptureProperty = imago_v4l2_plugin_bindings::imago::v4l2::types::CaptureProperty;
type EncodedFrame = imago_v4l2_plugin_bindings::imago::v4l2::types::EncodedFrame;
type Frame = imago_v4l2_plugin_bindings::imago::v4l2::types::Frame;
type DeviceResource = imago_v4l2_plugin_bindings::imago::v4l2::device::Device;
type StreamResource = imago_v4l2_plugin_bindings::imago::v4l2::capture_stream::CaptureStream;
type VideoCaptureResource = imago_v4l2_plugin_bindings::imago::v4l2::video_capture::VideoCapture;

const V4L2_RESOURCE_KEY: &str = "v4l2";
const V4L2_RESOURCE_PATHS_KEY: &str = "paths";
const V4L2_RESOURCE_MAX_FRAME_BYTES_KEY: &str = "max_frame_bytes";
const V4L2_RESOURCE_MAX_TIMEOUT_MS_KEY: &str = "max_timeout_ms";
const V4L2_RESOURCE_MAX_PATHS_KEY: &str = "max_paths";
const V4L2_RESOURCE_BUFFER_COUNT_KEY: &str = "buffer_count";

const DEFAULT_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_TIMEOUT_MS: u32 = 30_000;
const DEFAULT_MAX_PATHS: usize = 128;
const DEFAULT_BUFFER_COUNT: usize = 4;

const MAX_MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_MAX_TIMEOUT_MS: u32 = 120_000;
const MAX_MAX_PATHS: usize = 256;
const MAX_BUFFER_COUNT: usize = 32;

const DEVICE_COMMAND_CHANNEL_CAPACITY: usize = 32;
const DEFAULT_THREAD_STACK_BYTES: usize = 256 * 1024;
#[cfg(any(test, target_os = "linux"))]
const MAX_EXPANDED_CAPTURE_MODES: usize = 4_096;
#[cfg(target_os = "linux")]
const MJPG_FOURCC_VALUE: u32 = u32::from_le_bytes(*b"MJPG");
#[cfg(target_os = "linux")]
const V4L2_EXPOSURE_AUTO_CTRL_VALUE: i32 = 0;
#[cfg(target_os = "linux")]
const V4L2_EXPOSURE_MANUAL_CTRL_VALUE: i32 = 1;

#[cfg(target_os = "linux")]
fn mjpeg_fourcc() -> V4lFourCc {
    V4lFourCc::new(b"MJPG")
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameInterval {
    numerator: u32,
    denominator: u32,
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RationalValue {
    numerator: u128,
    denominator: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct V4l2LimitsConfig {
    max_frame_bytes: usize,
    max_timeout_ms: u32,
    max_paths: usize,
    buffer_count: usize,
}

impl Default for V4l2LimitsConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_timeout_ms: DEFAULT_MAX_TIMEOUT_MS,
            max_paths: DEFAULT_MAX_PATHS,
            buffer_count: DEFAULT_BUFFER_COUNT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct V4l2ResourcesConfig {
    paths: Vec<String>,
    allowlist: BTreeSet<String>,
    limits: V4l2LimitsConfig,
}

#[derive(Clone)]
struct DeviceRuntimeHandle {
    sender: mpsc::Sender<DeviceCommand>,
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

#[derive(Clone)]
struct StreamHandle {
    sender: mpsc::Sender<DeviceCommand>,
}

#[derive(Clone)]
struct VideoCaptureHandle {
    sender: mpsc::Sender<DeviceCommand>,
}

#[cfg(target_os = "linux")]
type CaptureIoStream = V4lMmapStream<'static>;

#[cfg(target_os = "linux")]
struct StreamState {
    mode: CaptureMode,
    stream: CaptureIoStream,
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone)]
struct VideoCaptureSelection {
    width_px: Option<u32>,
    height_px: Option<u32>,
    fps: Option<u32>,
}

#[cfg(target_os = "linux")]
struct VideoCaptureState {
    selected_mode: CaptureMode,
    selection: VideoCaptureSelection,
    stream: Option<StreamState>,
    last_frame: Option<Frame>,
}

#[cfg(target_os = "linux")]
struct DeviceThreadState {
    device: Arc<V4lDevice>,
    info: OpenableDevice,
    modes: Vec<CaptureMode>,
    limits: V4l2LimitsConfig,
    active_stream: Option<StreamState>,
    active_video_capture: Option<VideoCaptureState>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
enum DeviceCommand {
    Info {
        reply: oneshot::Sender<OpenableDevice>,
    },
    ListModes {
        reply: oneshot::Sender<Vec<CaptureMode>>,
    },
    OpenStream {
        mode: CaptureMode,
        reply: oneshot::Sender<Result<(), V4l2Error>>,
    },
    CurrentMode {
        reply: oneshot::Sender<Result<CaptureMode, V4l2Error>>,
    },
    NextFrame {
        timeout_ms: u32,
        reply: oneshot::Sender<Result<EncodedFrame, V4l2Error>>,
    },
    OpenVideoCapture {
        reply: oneshot::Sender<Result<(), V4l2Error>>,
    },
    VideoCaptureIsOpened {
        reply: oneshot::Sender<bool>,
    },
    VideoCaptureGet {
        property: CaptureProperty,
        reply: oneshot::Sender<Result<f64, V4l2Error>>,
    },
    VideoCaptureSet {
        property: CaptureProperty,
        value: f64,
        reply: oneshot::Sender<Result<bool, V4l2Error>>,
    },
    VideoCaptureRead {
        timeout_ms: u32,
        reply: oneshot::Sender<Result<Frame, V4l2Error>>,
    },
    VideoCaptureGrab {
        timeout_ms: u32,
        reply: oneshot::Sender<Result<bool, V4l2Error>>,
    },
    VideoCaptureRetrieve {
        reply: oneshot::Sender<Result<Frame, V4l2Error>>,
    },
    CloseStreamNoReply,
    CloseVideoCaptureNoReply,
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

static NEXT_DEVICE_REP: AtomicU32 = AtomicU32::new(1);
static DEVICE_REGISTRY: OnceLock<Mutex<BTreeMap<u32, DeviceRuntimeHandle>>> = OnceLock::new();

static NEXT_STREAM_REP: AtomicU32 = AtomicU32::new(1);
static STREAM_REGISTRY: OnceLock<Mutex<BTreeMap<u32, StreamHandle>>> = OnceLock::new();

static NEXT_VIDEO_CAPTURE_REP: AtomicU32 = AtomicU32::new(1);
static VIDEO_CAPTURE_REGISTRY: OnceLock<Mutex<BTreeMap<u32, VideoCaptureHandle>>> = OnceLock::new();

static V4L2_RESOURCES_CACHE: OnceLock<Mutex<BTreeMap<String, V4l2ResourcesConfig>>> =
    OnceLock::new();

fn device_registry() -> &'static Mutex<BTreeMap<u32, DeviceRuntimeHandle>> {
    DEVICE_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn stream_registry() -> &'static Mutex<BTreeMap<u32, StreamHandle>> {
    STREAM_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn video_capture_registry() -> &'static Mutex<BTreeMap<u32, VideoCaptureHandle>> {
    VIDEO_CAPTURE_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn v4l2_resources_cache() -> &'static Mutex<BTreeMap<String, V4l2ResourcesConfig>> {
    V4L2_RESOURCES_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
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

fn register_stream_handle(handle: StreamHandle) -> Result<u32, String> {
    register_rep(&NEXT_STREAM_REP, stream_registry(), handle)
}

fn lookup_stream_handle(rep: u32) -> Result<StreamHandle, String> {
    lookup_rep(stream_registry(), rep, "stream handle")
}

fn remove_stream_handle(rep: u32) {
    remove_rep(stream_registry(), rep);
}

fn register_video_capture_handle(handle: VideoCaptureHandle) -> Result<u32, String> {
    register_rep(&NEXT_VIDEO_CAPTURE_REP, video_capture_registry(), handle)
}

fn lookup_video_capture_handle(rep: u32) -> Result<VideoCaptureHandle, String> {
    lookup_rep(video_capture_registry(), rep, "video capture handle")
}

fn remove_video_capture_handle(rep: u32) {
    remove_rep(video_capture_registry(), rep);
}

fn retain_handles_for_other_senders<T>(
    registry: &mut BTreeMap<u32, T>,
    sender: &mpsc::Sender<DeviceCommand>,
) -> usize
where
    T: SenderBackedHandle,
{
    let before = registry.len();
    registry.retain(|_, handle| !handle.sender().same_channel(sender));
    before.saturating_sub(registry.len())
}

fn remove_stream_handles_for_sender(sender: &mpsc::Sender<DeviceCommand>) -> usize {
    let mut removed = 0;
    if let Ok(mut guard) = stream_registry().lock() {
        removed = retain_handles_for_other_senders(&mut guard, sender);
    }
    removed
}

fn remove_video_capture_handles_for_sender(sender: &mpsc::Sender<DeviceCommand>) -> usize {
    let mut removed = 0;
    if let Ok(mut guard) = video_capture_registry().lock() {
        removed = retain_handles_for_other_senders(&mut guard, sender);
    }
    removed
}

trait SenderBackedHandle {
    fn sender(&self) -> &mpsc::Sender<DeviceCommand>;
}

impl SenderBackedHandle for StreamHandle {
    fn sender(&self) -> &mpsc::Sender<DeviceCommand> {
        &self.sender
    }
}

impl SenderBackedHandle for VideoCaptureHandle {
    fn sender(&self) -> &mpsc::Sender<DeviceCommand> {
        &self.sender
    }
}

fn map_lookup_error(err: String) -> V4l2Error {
    V4l2Error::Other(err)
}

fn default_openable_device() -> OpenableDevice {
    OpenableDevice {
        path: String::new(),
        label: String::new(),
        vendor_id: 0,
        product_id: 0,
        bus: 0,
        address: 0,
    }
}

fn default_capture_mode() -> CaptureMode {
    CaptureMode {
        format: EncodedFormat::Mjpeg,
        width_px: 0,
        height_px: 0,
        fps_num: 0,
        fps_den: 1,
    }
}

#[cfg(target_os = "linux")]
fn default_video_capture_selection() -> VideoCaptureSelection {
    VideoCaptureSelection {
        width_px: None,
        height_px: None,
        fps: None,
    }
}

#[cfg(target_os = "linux")]
fn video_capture_state_from_modes(modes: &[CaptureMode]) -> Result<VideoCaptureState, V4l2Error> {
    let Some(selected_mode) = modes.first().cloned() else {
        return Err(V4l2Error::OperationNotSupported);
    };

    Ok(VideoCaptureState {
        selected_mode,
        selection: default_video_capture_selection(),
        stream: None,
        last_frame: None,
    })
}

#[cfg(any(test, target_os = "linux"))]
fn mode_matches_selection(mode: &CaptureMode, selection: &VideoCaptureSelection) -> bool {
    if selection
        .width_px
        .is_some_and(|width_px| mode.width_px != width_px)
    {
        return false;
    }
    if selection
        .height_px
        .is_some_and(|height_px| mode.height_px != height_px)
    {
        return false;
    }
    if selection
        .fps
        .is_some_and(|fps| mode.fps_den != 1 || mode.fps_num != fps)
    {
        return false;
    }
    true
}

#[cfg(target_os = "linux")]
fn capture_modes_equal(left: &CaptureMode, right: &CaptureMode) -> bool {
    left.format == right.format
        && left.width_px == right.width_px
        && left.height_px == right.height_px
        && left.fps_num == right.fps_num
        && left.fps_den == right.fps_den
}

#[cfg(any(test, target_os = "linux"))]
fn select_best_mode(
    modes: &[CaptureMode],
    selection: &VideoCaptureSelection,
    baseline: &CaptureMode,
) -> Option<CaptureMode> {
    let mut candidates: Vec<&CaptureMode> = modes
        .iter()
        .filter(|mode| mode_matches_selection(mode, selection))
        .collect();
    candidates.sort_by(|left, right| {
        let left_width_diff = left.width_px.abs_diff(baseline.width_px);
        let right_width_diff = right.width_px.abs_diff(baseline.width_px);
        left_width_diff
            .cmp(&right_width_diff)
            .then_with(|| {
                left.height_px
                    .abs_diff(baseline.height_px)
                    .cmp(&right.height_px.abs_diff(baseline.height_px))
            })
            .then_with(|| right.fps_num.cmp(&left.fps_num))
            .then_with(|| left.fps_den.cmp(&right.fps_den))
            .then_with(|| right.width_px.cmp(&left.width_px))
            .then_with(|| right.height_px.cmp(&left.height_px))
    });
    candidates.into_iter().next().cloned()
}

#[cfg(target_os = "linux")]
fn rounded_integer_fps(value: f64) -> Result<u32, V4l2Error> {
    if !value.is_finite() || value <= 0.0 {
        return Err(V4l2Error::InvalidArgument);
    }
    let rounded = value.round();
    if !(1.0..=f64::from(u32::MAX)).contains(&rounded) {
        return Err(V4l2Error::InvalidArgument);
    }
    Ok(rounded as u32)
}

#[cfg(target_os = "linux")]
fn rounded_u32(value: f64) -> Result<u32, V4l2Error> {
    if !value.is_finite() || value < 0.0 {
        return Err(V4l2Error::InvalidArgument);
    }
    let rounded = value.round();
    if !(0.0..=f64::from(u32::MAX)).contains(&rounded) {
        return Err(V4l2Error::InvalidArgument);
    }
    Ok(rounded as u32)
}

#[cfg(target_os = "linux")]
fn rounded_i32(value: f64) -> Result<i32, V4l2Error> {
    if !value.is_finite() {
        return Err(V4l2Error::InvalidArgument);
    }
    let rounded = value.round();
    if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&rounded) {
        return Err(V4l2Error::InvalidArgument);
    }
    Ok(rounded as i32)
}

#[cfg(target_os = "linux")]
fn ctrl_id_from_property(property: CaptureProperty) -> Option<u32> {
    match property {
        CaptureProperty::Brightness => Some(v4l_bindings::V4L2_CID_BRIGHTNESS),
        CaptureProperty::Contrast => Some(v4l_bindings::V4L2_CID_CONTRAST),
        CaptureProperty::Saturation => Some(v4l_bindings::V4L2_CID_SATURATION),
        CaptureProperty::Gain => Some(v4l_bindings::V4L2_CID_GAIN),
        CaptureProperty::AutoExposure => Some(v4l_bindings::V4L2_CID_EXPOSURE_AUTO),
        CaptureProperty::Exposure => Some(v4l_bindings::V4L2_CID_EXPOSURE_ABSOLUTE),
        CaptureProperty::AutoFocus => Some(v4l_bindings::V4L2_CID_FOCUS_AUTO),
        CaptureProperty::Focus => Some(v4l_bindings::V4L2_CID_FOCUS_ABSOLUTE),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn query_control_description(
    device: &V4lDevice,
    id: u32,
) -> Result<V4lControlDescription, V4l2Error> {
    device
        .query_controls()
        .map_err(map_io_error)?
        .into_iter()
        .find(|control| control.id == id)
        .ok_or(V4l2Error::OperationNotSupported)
}

#[cfg(target_os = "linux")]
fn ensure_control_get_allowed(control: &V4lControlDescription) -> Result<(), V4l2Error> {
    if control
        .flags
        .intersects(V4lControlFlags::DISABLED | V4lControlFlags::WRITE_ONLY)
    {
        return Err(V4l2Error::OperationNotSupported);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn ensure_control_set_allowed(control: &V4lControlDescription) -> Result<(), V4l2Error> {
    if control
        .flags
        .intersects(V4lControlFlags::DISABLED | V4lControlFlags::READ_ONLY)
    {
        return Err(V4l2Error::OperationNotSupported);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn numeric_control_value(value: V4lControlValue) -> Result<i64, V4l2Error> {
    match value {
        V4lControlValue::Integer(value) => Ok(value),
        V4lControlValue::Boolean(value) => Ok(i64::from(value)),
        _ => Err(V4l2Error::OperationNotSupported),
    }
}

#[cfg(target_os = "linux")]
fn map_boolish_to_ctrl(value: f64) -> Result<i32, V4l2Error> {
    let rounded = rounded_i32(value)?;
    match rounded {
        0 | 1 => Ok(rounded),
        _ => Err(V4l2Error::InvalidArgument),
    }
}

#[cfg(target_os = "linux")]
fn get_control_value(device: &V4lDevice, property: CaptureProperty) -> Result<f64, V4l2Error> {
    let id = ctrl_id_from_property(property).ok_or(V4l2Error::OperationNotSupported)?;
    let control = query_control_description(device, id)?;
    ensure_control_get_allowed(&control)?;
    let value = device.control(id).map_err(map_io_error)?;
    let value = numeric_control_value(value.value)?;
    match property {
        CaptureProperty::AutoExposure => {
            Ok(if value == i64::from(V4L2_EXPOSURE_MANUAL_CTRL_VALUE) {
                0.0
            } else if value == i64::from(V4L2_EXPOSURE_AUTO_CTRL_VALUE) {
                1.0
            } else {
                return Err(V4l2Error::OperationNotSupported);
            })
        }
        CaptureProperty::AutoFocus => Ok(f64::from((value != 0) as u8)),
        _ => Ok(value as f64),
    }
}

#[cfg(target_os = "linux")]
fn set_control_value(
    device: &V4lDevice,
    property: CaptureProperty,
    value: f64,
) -> Result<bool, V4l2Error> {
    let id = ctrl_id_from_property(property).ok_or(V4l2Error::OperationNotSupported)?;
    let control = query_control_description(device, id)?;
    ensure_control_set_allowed(&control)?;
    let ctrl_value = match property {
        CaptureProperty::AutoExposure => match map_boolish_to_ctrl(value)? {
            0 => V4L2_EXPOSURE_MANUAL_CTRL_VALUE,
            1 => V4L2_EXPOSURE_AUTO_CTRL_VALUE,
            _ => unreachable!(),
        },
        CaptureProperty::AutoFocus => map_boolish_to_ctrl(value)?,
        _ => rounded_i32(value)?,
    };
    let value = match control.typ {
        V4lControlType::Boolean => V4lControlValue::Boolean(ctrl_value != 0),
        V4lControlType::Integer
        | V4lControlType::Integer64
        | V4lControlType::Menu
        | V4lControlType::IntegerMenu => V4lControlValue::Integer(i64::from(ctrl_value)),
        _ => return Err(V4l2Error::OperationNotSupported),
    };
    let expected = match property {
        CaptureProperty::AutoExposure => {
            f64::from((ctrl_value == V4L2_EXPOSURE_AUTO_CTRL_VALUE) as u8)
        }
        CaptureProperty::AutoFocus => f64::from((ctrl_value != 0) as u8),
        _ => f64::from(ctrl_value),
    };
    let can_read_back = !control.flags.contains(V4lControlFlags::WRITE_ONLY);

    device
        .set_control(V4lControl { id, value })
        .map_err(map_io_error)?;
    if !can_read_back {
        return Ok(true);
    }

    Ok(get_control_value(device, property)? == expected)
}

#[cfg(target_os = "linux")]
fn frame_from_decoded_bytes(
    bytes: Vec<u8>,
    width_px: u32,
    height_px: u32,
    sequence: u64,
    timestamp_ns: u64,
) -> Result<Frame, V4l2Error> {
    let stride_bytes = width_px.checked_mul(4).ok_or(V4l2Error::TransportFault)?;
    let expected_len = usize::try_from(
        u64::from(stride_bytes)
            .checked_mul(u64::from(height_px))
            .ok_or(V4l2Error::TransportFault)?,
    )
    .map_err(|_| V4l2Error::TransportFault)?;
    if bytes.len() != expected_len {
        return Err(V4l2Error::TransportFault);
    }
    Ok(Frame {
        bytes,
        width_px,
        height_px,
        stride_bytes,
        timestamp_ns,
        sequence,
        format: FramePixelFormat::Rgba8,
    })
}

#[cfg(target_os = "linux")]
fn rgb_to_rgba(bytes: &[u8]) -> Result<Vec<u8>, V4l2Error> {
    if !bytes.len().is_multiple_of(3) {
        return Err(V4l2Error::TransportFault);
    }
    let pixel_count = bytes.len() / 3;
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or(V4l2Error::TransportFault)?;
    let mut rgba = Vec::with_capacity(output_len);
    for chunk in bytes.chunks_exact(3) {
        rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]);
    }
    Ok(rgba)
}

#[cfg(target_os = "linux")]
fn l8_to_rgba(bytes: &[u8]) -> Result<Vec<u8>, V4l2Error> {
    let output_len = bytes
        .len()
        .checked_mul(4)
        .ok_or(V4l2Error::TransportFault)?;
    let mut rgba = Vec::with_capacity(output_len);
    for value in bytes {
        rgba.extend_from_slice(&[*value, *value, *value, 0xff]);
    }
    Ok(rgba)
}

#[cfg(target_os = "linux")]
fn l16_to_rgba(bytes: &[u8]) -> Result<Vec<u8>, V4l2Error> {
    if !bytes.len().is_multiple_of(2) {
        return Err(V4l2Error::TransportFault);
    }
    let pixel_count = bytes.len() / 2;
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or(V4l2Error::TransportFault)?;
    let mut rgba = Vec::with_capacity(output_len);
    for chunk in bytes.chunks_exact(2) {
        let value = u16::from_be_bytes([chunk[0], chunk[1]]);
        let v8 = (value >> 8) as u8;
        rgba.extend_from_slice(&[v8, v8, v8, 0xff]);
    }
    Ok(rgba)
}

#[cfg(target_os = "linux")]
fn cmyk32_to_rgba(bytes: &[u8]) -> Result<Vec<u8>, V4l2Error> {
    if !bytes.len().is_multiple_of(4) {
        return Err(V4l2Error::TransportFault);
    }
    let pixel_count = bytes.len() / 4;
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or(V4l2Error::TransportFault)?;
    let mut rgba = Vec::with_capacity(output_len);
    for chunk in bytes.chunks_exact(4) {
        let k = u16::from(255 - chunk[3]);
        let r = ((u16::from(255 - chunk[0]) * k) / 255) as u8;
        let g = ((u16::from(255 - chunk[1]) * k) / 255) as u8;
        let b = ((u16::from(255 - chunk[2]) * k) / 255) as u8;
        rgba.extend_from_slice(&[r, g, b, 0xff]);
    }
    Ok(rgba)
}

#[cfg(target_os = "linux")]
fn frame_payload_from_mapping(
    mapping: &[u8],
    bytes_used: usize,
    data_offset: usize,
) -> Result<Vec<u8>, V4l2Error> {
    let payload_len = bytes_used
        .checked_sub(data_offset)
        .ok_or(V4l2Error::TransportFault)?;
    let payload = mapping
        .get(..payload_len)
        .ok_or(V4l2Error::TransportFault)?;
    if payload.is_empty() {
        return Err(V4l2Error::TransportFault);
    }
    Ok(payload.to_vec())
}

#[cfg(target_os = "linux")]
fn decode_mjpeg_frame(
    jpeg_bytes: &[u8],
    limits: &V4l2LimitsConfig,
    sequence: u64,
    timestamp_ns: u64,
) -> Result<Frame, V4l2Error> {
    let mut decoder = JpegDecoder::new(Cursor::new(jpeg_bytes));
    let max_decoded_bytes = limits
        .max_frame_bytes
        .checked_mul(4)
        .ok_or(V4l2Error::OperationNotSupported)?;
    decoder.set_max_decoding_buffer_size(max_decoded_bytes);
    let decoded = decoder
        .decode()
        .map_err(|err| V4l2Error::Other(format!("jpeg decode failed: {err}")))?;
    let info = decoder.info().ok_or(V4l2Error::TransportFault)?;
    let rgba = match info.pixel_format {
        JpegPixelFormat::RGB24 => rgb_to_rgba(&decoded)?,
        JpegPixelFormat::L8 => l8_to_rgba(&decoded)?,
        JpegPixelFormat::L16 => l16_to_rgba(&decoded)?,
        JpegPixelFormat::CMYK32 => cmyk32_to_rgba(&decoded)?,
    };
    frame_from_decoded_bytes(
        rgba,
        u32::from(info.width),
        u32::from(info.height),
        sequence,
        timestamp_ns,
    )
}

fn normalize_video_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("resources.v4l2.paths[] must not be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("resources.v4l2.paths[] must not contain NUL".to_string());
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(format!(
            "resources.v4l2.paths[] must be an absolute path (got: {trimmed})"
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
                    "resources.v4l2.paths[] must not use platform prefixes (got: {trimmed})"
                ));
            }
        }
    }

    let normalized = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };

    let Some(file_name) = Path::new(&normalized)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return Err(format!(
            "resources.v4l2.paths[] must match /dev/video<index> (got: {normalized})"
        ));
    };
    let suffix = file_name.strip_prefix("video").ok_or_else(|| {
        format!("resources.v4l2.paths[] must match /dev/video<index> (got: {normalized})")
    })?;
    if !normalized.starts_with("/dev/video")
        || suffix.is_empty()
        || !suffix.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(format!(
            "resources.v4l2.paths[] must match /dev/video<index> (got: {normalized})"
        ));
    }

    Ok(normalized)
}

fn parse_u64_field(table: &JsonMap<String, JsonValue>, key: &str) -> Result<Option<u64>, String> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };

    let number = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .ok_or_else(|| format!("resources.v4l2.{key} must be a non-negative integer"))?;
    Ok(Some(number))
}

fn parse_v4l2_resources_config(
    resources: &BTreeMap<String, JsonValue>,
) -> Result<V4l2ResourcesConfig, String> {
    let v4l2_value = resources
        .get(V4L2_RESOURCE_KEY)
        .ok_or_else(|| "resources.v4l2 is required".to_string())?;
    let v4l2_table = v4l2_value
        .as_object()
        .ok_or_else(|| "resources.v4l2 must be a table".to_string())?;

    let paths_value = v4l2_table
        .get(V4L2_RESOURCE_PATHS_KEY)
        .ok_or_else(|| "resources.v4l2.paths is required".to_string())?;
    let paths_array = paths_value
        .as_array()
        .ok_or_else(|| "resources.v4l2.paths must be an array".to_string())?;

    let mut paths = Vec::with_capacity(paths_array.len());
    let mut allowlist = BTreeSet::new();
    for (index, path_value) in paths_array.iter().enumerate() {
        let raw = path_value
            .as_str()
            .ok_or_else(|| format!("resources.v4l2.paths[{index}] must be a string"))?;
        let normalized = normalize_video_path(raw)?;
        if !allowlist.insert(normalized.clone()) {
            return Err(format!(
                "resources.v4l2.paths[{index}] duplicates normalized path: {normalized}"
            ));
        }
        paths.push(normalized);
    }

    let max_frame_bytes = parse_u64_field(v4l2_table, V4L2_RESOURCE_MAX_FRAME_BYTES_KEY)?
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                format!(
                    "resources.v4l2.{V4L2_RESOURCE_MAX_FRAME_BYTES_KEY} is too large for this platform"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_FRAME_BYTES);
    if max_frame_bytes == 0 || max_frame_bytes > MAX_MAX_FRAME_BYTES {
        return Err(format!(
            "resources.v4l2.{V4L2_RESOURCE_MAX_FRAME_BYTES_KEY} must be within 1..={MAX_MAX_FRAME_BYTES}"
        ));
    }

    let max_timeout_ms = parse_u64_field(v4l2_table, V4L2_RESOURCE_MAX_TIMEOUT_MS_KEY)?
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                format!(
                    "resources.v4l2.{V4L2_RESOURCE_MAX_TIMEOUT_MS_KEY} is too large for this runtime"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_TIMEOUT_MS);
    if max_timeout_ms == 0 || max_timeout_ms > MAX_MAX_TIMEOUT_MS {
        return Err(format!(
            "resources.v4l2.{V4L2_RESOURCE_MAX_TIMEOUT_MS_KEY} must be within 1..={MAX_MAX_TIMEOUT_MS}"
        ));
    }

    let max_paths = parse_u64_field(v4l2_table, V4L2_RESOURCE_MAX_PATHS_KEY)?
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                format!(
                    "resources.v4l2.{V4L2_RESOURCE_MAX_PATHS_KEY} is too large for this platform"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_PATHS);
    if max_paths > MAX_MAX_PATHS {
        return Err(format!(
            "resources.v4l2.{V4L2_RESOURCE_MAX_PATHS_KEY} must be within 0..={MAX_MAX_PATHS}"
        ));
    }

    if paths.len() > max_paths {
        return Err(format!(
            "resources.v4l2.paths contains {} entries which exceeds max_paths={max_paths}",
            paths.len()
        ));
    }

    let buffer_count = parse_u64_field(v4l2_table, V4L2_RESOURCE_BUFFER_COUNT_KEY)?
        .map(|value| {
            usize::try_from(value).map_err(|_| {
                format!(
                    "resources.v4l2.{V4L2_RESOURCE_BUFFER_COUNT_KEY} is too large for this platform"
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_BUFFER_COUNT);
    if buffer_count == 0 || buffer_count > MAX_BUFFER_COUNT {
        return Err(format!(
            "resources.v4l2.{V4L2_RESOURCE_BUFFER_COUNT_KEY} must be within 1..={MAX_BUFFER_COUNT}"
        ));
    }

    Ok(V4l2ResourcesConfig {
        paths,
        allowlist,
        limits: V4l2LimitsConfig {
            max_frame_bytes,
            max_timeout_ms,
            max_paths,
            buffer_count,
        },
    })
}

fn v4l2_resources_cache_key(service_name: &str, release_hash: &str, runner_id: &str) -> String {
    format!("{service_name}\u{1f}{release_hash}\u{1f}{runner_id}")
}

fn load_v4l2_resources_for_state(state: &WasiState) -> Result<V4l2ResourcesConfig, V4l2Error> {
    let context = state.native_plugin_context();
    let cache_key = v4l2_resources_cache_key(
        context.service_name(),
        context.release_hash(),
        context.runner_id(),
    );

    let mut guard = v4l2_resources_cache()
        .lock()
        .map_err(|_| V4l2Error::Other("v4l2 resource cache lock poisoned".to_string()))?;

    if !guard.contains_key(&cache_key) {
        let parsed = parse_v4l2_resources_config(context.resources()).map_err(V4l2Error::Other)?;
        guard.insert(cache_key.clone(), parsed);
    }

    guard
        .get(&cache_key)
        .cloned()
        .ok_or_else(|| V4l2Error::Other("v4l2 resource cache entry missing".to_string()))
}

fn load_v4l2_resources_for_state_or_default(state: &WasiState) -> V4l2ResourcesConfig {
    load_v4l2_resources_for_state(state).unwrap_or_default()
}

fn to_limits_record(limits: &V4l2LimitsConfig) -> Limits {
    Limits {
        max_frame_bytes: u32::try_from(limits.max_frame_bytes)
            .expect("max_frame_bytes should fit in u32"),
        max_timeout_ms: limits.max_timeout_ms,
        max_paths: u32::try_from(limits.max_paths).expect("max_paths should fit in u32"),
        buffer_count: u32::try_from(limits.buffer_count).expect("buffer_count should fit in u32"),
    }
}

fn ensure_v4l2_supported() -> Result<(), V4l2Error> {
    #[cfg(target_os = "linux")]
    {
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(V4l2Error::OperationNotSupported)
    }
}

#[cfg(target_os = "linux")]
fn validate_timeout(timeout_ms: u32, limits: &V4l2LimitsConfig) -> Result<Duration, V4l2Error> {
    if timeout_ms == 0 || timeout_ms > limits.max_timeout_ms {
        return Err(V4l2Error::InvalidArgument);
    }
    Ok(Duration::from_millis(u64::from(timeout_ms)))
}

#[cfg(target_os = "linux")]
fn map_errno(errno: Errno) -> V4l2Error {
    match errno {
        Errno::EACCES | Errno::EPERM => V4l2Error::NotAllowed,
        Errno::EAGAIN | Errno::EBUSY => V4l2Error::Busy,
        Errno::EINVAL => V4l2Error::InvalidArgument,
        Errno::ETIMEDOUT => V4l2Error::Timeout,
        Errno::ENODEV | Errno::ENOENT | Errno::ENXIO | Errno::EPIPE => V4l2Error::Disconnected,
        Errno::ENOTTY | Errno::ENOSYS => V4l2Error::OperationNotSupported,
        _ => V4l2Error::TransportFault,
    }
}

#[cfg(target_os = "linux")]
fn map_io_error(err: io::Error) -> V4l2Error {
    if let Some(errno) = err.raw_os_error() {
        return map_errno(Errno::from_raw(errno));
    }

    match err.kind() {
        io::ErrorKind::TimedOut => V4l2Error::Timeout,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => V4l2Error::InvalidArgument,
        io::ErrorKind::WouldBlock => V4l2Error::Busy,
        io::ErrorKind::NotFound
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::UnexpectedEof
        | io::ErrorKind::NotConnected => V4l2Error::Disconnected,
        io::ErrorKind::PermissionDenied => V4l2Error::NotAllowed,
        io::ErrorKind::Unsupported => V4l2Error::OperationNotSupported,
        _ => V4l2Error::TransportFault,
    }
}

#[cfg(target_os = "linux")]
fn openable_device_label(path: &str, capability: &V4lCapabilities) -> String {
    let card = capability.card.trim();
    if card.is_empty() {
        path.to_string()
    } else {
        format!("{card} {path}")
    }
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsbMetadata {
    vendor_id: u16,
    product_id: u16,
    bus: u8,
    address: u8,
}

#[cfg(any(test, target_os = "linux"))]
fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

#[cfg(any(test, target_os = "linux"))]
fn read_hex_u16(path: &Path) -> Option<u16> {
    let raw = read_trimmed(path)?;
    u16::from_str_radix(raw.trim_start_matches("0x"), 16).ok()
}

#[cfg(any(test, target_os = "linux"))]
fn read_u8(path: &Path) -> Option<u8> {
    read_trimmed(path)?.parse::<u8>().ok()
}

#[cfg(any(test, target_os = "linux"))]
fn resolve_usb_metadata_from_sys_root(sys_root: &Path, video_path: &str) -> Option<UsbMetadata> {
    let video_name = Path::new(video_path).file_name()?.to_str()?;
    let class_device = sys_root
        .join("class")
        .join("video4linux")
        .join(video_name)
        .join("device");
    let canonical = fs::canonicalize(class_device).ok()?;

    for candidate in canonical.ancestors() {
        let vendor_id = read_hex_u16(&candidate.join("idVendor"));
        let product_id = read_hex_u16(&candidate.join("idProduct"));
        let bus = read_u8(&candidate.join("busnum"));
        let address = read_u8(&candidate.join("devnum"));
        if let (Some(vendor_id), Some(product_id), Some(bus), Some(address)) =
            (vendor_id, product_id, bus, address)
        {
            return Some(UsbMetadata {
                vendor_id,
                product_id,
                bus,
                address,
            });
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn resolve_usb_metadata(video_path: &str) -> Option<UsbMetadata> {
    resolve_usb_metadata_from_sys_root(Path::new("/sys"), video_path)
}

#[cfg(target_os = "linux")]
fn build_openable_device(path: &str, capability: &V4lCapabilities) -> OpenableDevice {
    let usb = resolve_usb_metadata(path);
    OpenableDevice {
        path: path.to_string(),
        label: openable_device_label(path, capability),
        vendor_id: usb.map_or(0, |usb| usb.vendor_id),
        product_id: usb.map_or(0, |usb| usb.product_id),
        bus: usb.map_or(0, |usb| usb.bus),
        address: usb.map_or(0, |usb| usb.address),
    }
}

#[cfg(any(test, target_os = "linux"))]
fn gcd_u128(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(any(test, target_os = "linux"))]
impl RationalValue {
    fn new(numerator: u32, denominator: u32) -> Result<Self, V4l2Error> {
        if numerator == 0 || denominator == 0 {
            return Err(V4l2Error::OperationNotSupported);
        }
        Ok(Self::reduced(
            u128::from(numerator),
            u128::from(denominator),
        ))
    }

    fn reduced(numerator: u128, denominator: u128) -> Self {
        let gcd = gcd_u128(numerator, denominator);
        Self {
            numerator: numerator / gcd,
            denominator: denominator / gcd,
        }
    }

    fn checked_add(self, other: Self) -> Result<Self, V4l2Error> {
        let gcd = gcd_u128(self.denominator, other.denominator);
        let left_factor = other.denominator / gcd;
        let right_factor = self.denominator / gcd;
        let denominator = self
            .denominator
            .checked_mul(left_factor)
            .ok_or(V4l2Error::OperationNotSupported)?;
        let numerator = self
            .numerator
            .checked_mul(left_factor)
            .and_then(|value| {
                other
                    .numerator
                    .checked_mul(right_factor)
                    .and_then(|other_value| value.checked_add(other_value))
            })
            .ok_or(V4l2Error::OperationNotSupported)?;
        Ok(Self::reduced(numerator, denominator))
    }

    fn checked_cmp(self, other: Self) -> Result<CmpOrdering, V4l2Error> {
        let left = self
            .numerator
            .checked_mul(other.denominator)
            .ok_or(V4l2Error::OperationNotSupported)?;
        let right = other
            .numerator
            .checked_mul(self.denominator)
            .ok_or(V4l2Error::OperationNotSupported)?;
        Ok(left.cmp(&right))
    }

    fn try_into_frame_interval(self) -> Result<FrameInterval, V4l2Error> {
        Ok(FrameInterval {
            numerator: u32::try_from(self.numerator)
                .map_err(|_| V4l2Error::OperationNotSupported)?,
            denominator: u32::try_from(self.denominator)
                .map_err(|_| V4l2Error::OperationNotSupported)?,
        })
    }
}

#[cfg(any(test, target_os = "linux"))]
fn expand_stepwise_u32_values(
    min: u32,
    max: u32,
    step: u32,
    limit: usize,
) -> Result<Vec<u32>, V4l2Error> {
    if min > max || step == 0 {
        return Err(V4l2Error::OperationNotSupported);
    }

    let mut values = Vec::new();
    let mut current = min;
    loop {
        if values.len() >= limit {
            return Err(V4l2Error::OperationNotSupported);
        }
        values.push(current);
        if current == max {
            break;
        }
        current = current
            .checked_add(step)
            .ok_or(V4l2Error::OperationNotSupported)?;
        if current > max {
            break;
        }
    }

    Ok(values)
}

#[cfg(any(test, target_os = "linux"))]
fn expand_stepwise_frame_intervals(
    min: FrameInterval,
    max: FrameInterval,
    step: FrameInterval,
    limit: usize,
) -> Result<Vec<FrameInterval>, V4l2Error> {
    let min = RationalValue::new(min.numerator, min.denominator)?;
    let max = RationalValue::new(max.numerator, max.denominator)?;
    let step = RationalValue::new(step.numerator, step.denominator)?;
    if min.checked_cmp(max)? == CmpOrdering::Greater {
        return Err(V4l2Error::OperationNotSupported);
    }

    let mut values = Vec::new();
    let mut current = min;
    loop {
        if values.len() >= limit {
            return Err(V4l2Error::OperationNotSupported);
        }
        values.push(current.try_into_frame_interval()?);
        let next = current.checked_add(step)?;
        if next.checked_cmp(max)? == CmpOrdering::Greater {
            break;
        }
        current = next;
    }

    Ok(values)
}

#[cfg(target_os = "linux")]
fn insert_capture_mode(
    unique: &mut BTreeSet<(u32, u32, u32, u32)>,
    width_px: u32,
    height_px: u32,
    fps_num: u32,
    fps_den: u32,
) -> Result<(), V4l2Error> {
    let mode = (width_px, height_px, fps_num, fps_den);
    if unique.contains(&mode) {
        return Ok(());
    }
    if unique.len() >= MAX_EXPANDED_CAPTURE_MODES {
        return Err(V4l2Error::OperationNotSupported);
    }
    unique.insert(mode);
    Ok(())
}

#[cfg(target_os = "linux")]
fn ensure_capture_capabilities(caps: &V4lCapabilities) -> Result<(), V4l2Error> {
    if !caps.capabilities.contains(V4lCapabilityFlags::STREAMING) {
        return Err(V4l2Error::OperationNotSupported);
    }
    if !caps
        .capabilities
        .contains(V4lCapabilityFlags::VIDEO_CAPTURE)
    {
        return Err(V4l2Error::OperationNotSupported);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn enumerate_mjpeg_modes(device: &V4lDevice) -> Result<Vec<CaptureMode>, V4l2Error> {
    let caps = device.query_caps().map_err(map_io_error)?;
    ensure_capture_capabilities(&caps)?;

    let mut unique = BTreeSet::new();
    for fmt in device.enum_formats().map_err(map_io_error)? {
        if fmt.fourcc != mjpeg_fourcc() {
            continue;
        }
        enumerate_modes_for_format(device, fmt.fourcc, &mut unique)?;
    }

    Ok(unique
        .into_iter()
        .map(|(width_px, height_px, fps_num, fps_den)| CaptureMode {
            format: EncodedFormat::Mjpeg,
            width_px,
            height_px,
            fps_num,
            fps_den,
        })
        .collect())
}

#[cfg(target_os = "linux")]
fn enumerate_modes_for_format(
    device: &V4lDevice,
    pixel_format: V4lFourCc,
    unique: &mut BTreeSet<(u32, u32, u32, u32)>,
) -> Result<(), V4l2Error> {
    for frame_size in device.enum_framesizes(pixel_format).map_err(map_io_error)? {
        match frame_size.size {
            V4lFrameSizeEnum::Discrete(size) => {
                enumerate_intervals_for_size(
                    device,
                    pixel_format,
                    size.width,
                    size.height,
                    unique,
                )?;
            }
            V4lFrameSizeEnum::Stepwise(size) => {
                let widths = expand_stepwise_u32_values(
                    size.min_width,
                    size.max_width,
                    size.step_width,
                    MAX_EXPANDED_CAPTURE_MODES,
                )?;
                let heights = expand_stepwise_u32_values(
                    size.min_height,
                    size.max_height,
                    size.step_height,
                    MAX_EXPANDED_CAPTURE_MODES,
                )?;
                let size_pair_count = widths
                    .len()
                    .checked_mul(heights.len())
                    .ok_or(V4l2Error::OperationNotSupported)?;
                if size_pair_count > MAX_EXPANDED_CAPTURE_MODES {
                    return Err(V4l2Error::OperationNotSupported);
                }
                for width in widths {
                    for height in &heights {
                        enumerate_intervals_for_size(device, pixel_format, width, *height, unique)?;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn enumerate_intervals_for_size(
    device: &V4lDevice,
    pixel_format: V4lFourCc,
    width: u32,
    height: u32,
    unique: &mut BTreeSet<(u32, u32, u32, u32)>,
) -> Result<(), V4l2Error> {
    for interval in device
        .enum_frameintervals(pixel_format, width, height)
        .map_err(map_io_error)?
    {
        match interval.interval {
            V4lFrameIntervalEnum::Discrete(discrete) => {
                if let Some((fps_num, fps_den)) =
                    fps_ratio_from_time_per_frame(discrete.numerator, discrete.denominator)
                {
                    insert_capture_mode(unique, width, height, fps_num, fps_den)?;
                }
            }
            V4lFrameIntervalEnum::Stepwise(stepwise) => {
                let intervals = expand_stepwise_frame_intervals(
                    FrameInterval {
                        numerator: stepwise.min.numerator,
                        denominator: stepwise.min.denominator,
                    },
                    FrameInterval {
                        numerator: stepwise.max.numerator,
                        denominator: stepwise.max.denominator,
                    },
                    FrameInterval {
                        numerator: stepwise.step.numerator,
                        denominator: stepwise.step.denominator,
                    },
                    MAX_EXPANDED_CAPTURE_MODES,
                )?;
                for interval in intervals {
                    if let Some((fps_num, fps_den)) =
                        fps_ratio_from_time_per_frame(interval.numerator, interval.denominator)
                    {
                        insert_capture_mode(unique, width, height, fps_num, fps_den)?;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(any(test, target_os = "linux"))]
fn fps_ratio_from_time_per_frame(numerator: u32, denominator: u32) -> Option<(u32, u32)> {
    if numerator == 0 || denominator == 0 {
        return None;
    }
    Some((denominator, numerator))
}

#[cfg(target_os = "linux")]
fn inspect_device(
    path: &str,
) -> Result<(Arc<V4lDevice>, OpenableDevice, Vec<CaptureMode>), V4l2Error> {
    let device = Arc::new(V4lDevice::with_path(Path::new(path)).map_err(map_io_error)?);
    let capability = device.query_caps().map_err(map_io_error)?;
    ensure_capture_capabilities(&capability)?;
    let info = build_openable_device(path, &capability);
    let modes = enumerate_mjpeg_modes(device.as_ref())?;
    Ok((device, info, modes))
}

#[cfg(target_os = "linux")]
fn enumerate_openable_devices(allowlist: &BTreeSet<String>) -> Vec<OpenableDevice> {
    let mut devices = Vec::new();
    for path in allowlist {
        if let Ok((_, info, modes)) = inspect_device(path)
            && !modes.is_empty()
        {
            devices.push(info);
        }
    }
    devices
}

#[cfg(not(target_os = "linux"))]
fn enumerate_openable_devices(_allowlist: &BTreeSet<String>) -> Vec<OpenableDevice> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn verify_exact_mode(
    format: &V4lFormat,
    mode: &CaptureMode,
    limits: &V4l2LimitsConfig,
) -> Result<(), V4l2Error> {
    if format.width != mode.width_px
        || format.height != mode.height_px
        || format.fourcc != mjpeg_fourcc()
    {
        return Err(V4l2Error::OperationNotSupported);
    }
    if format.size == 0 {
        return Err(V4l2Error::TransportFault);
    }
    if usize::try_from(format.size)
        .ok()
        .is_some_and(|sizeimage| sizeimage > limits.max_frame_bytes)
    {
        return Err(V4l2Error::OperationNotSupported);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_exact_frame_interval(device: &V4lDevice, mode: &CaptureMode) -> Result<(), V4l2Error> {
    let mut params: V4lCaptureParameters = device.params().map_err(map_io_error)?;
    params.interval = V4lFraction::new(mode.fps_den, mode.fps_num);
    let applied = device.set_params(&params).map_err(map_io_error)?;
    if applied.interval.numerator != mode.fps_den || applied.interval.denominator != mode.fps_num {
        return Err(V4l2Error::OperationNotSupported);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn create_capture_stream(
    device: &V4lDevice,
    buffer_count: usize,
) -> Result<CaptureIoStream, V4l2Error> {
    let buffer_count = u32::try_from(buffer_count).map_err(|_| V4l2Error::InvalidArgument)?;
    let stream = V4lMmapStream::with_buffers(device, V4lBufferType::VideoCapture, buffer_count)
        .map_err(map_io_error)?;

    // SAFETY: `v4l::io::mmap::Stream` owns the mmap arena and device handle internally.
    // We only keep the stream inside `StreamState` and copy frame bytes out before returning.
    Ok(unsafe { std::mem::transmute::<V4lMmapStream<'_>, CaptureIoStream>(stream) })
}

#[cfg(target_os = "linux")]
fn open_stream_state(
    device: &Arc<V4lDevice>,
    mode: &CaptureMode,
    limits: &V4l2LimitsConfig,
) -> Result<StreamState, V4l2Error> {
    let desired = V4lFormat::new(mode.width_px, mode.height_px, mjpeg_fourcc());
    let applied = device.set_format(&desired).map_err(map_io_error)?;
    verify_exact_mode(&applied, mode, limits)?;
    set_exact_frame_interval(device.as_ref(), mode)?;
    let stream = create_capture_stream(device.as_ref(), limits.buffer_count)?;

    Ok(StreamState {
        mode: *mode,
        stream,
    })
}

#[cfg(target_os = "linux")]
fn close_stream_state(stream: StreamState) -> Result<(), V4l2Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(stream)))
        .map_err(|_| V4l2Error::TransportFault)?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn timestamp_to_ns(timestamp: V4lTimestamp) -> Result<u64, V4l2Error> {
    let seconds = u64::try_from(timestamp.sec).map_err(|_| V4l2Error::TransportFault)?;
    let micros = u64::try_from(timestamp.usec).map_err(|_| V4l2Error::TransportFault)?;
    if micros >= 1_000_000 {
        return Err(V4l2Error::TransportFault);
    }

    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(micros.saturating_mul(1_000)))
        .ok_or(V4l2Error::TransportFault)
}

#[cfg(target_os = "linux")]
fn read_next_frame(
    stream: &mut StreamState,
    limits: &V4l2LimitsConfig,
    timeout_ms: u32,
) -> Result<EncodedFrame, V4l2Error> {
    let timeout = validate_timeout(timeout_ms, limits)?;
    stream.stream.set_timeout(timeout);

    let (mapping, metadata) = V4lCaptureStream::next(&mut stream.stream).map_err(map_io_error)?;
    if metadata.flags.contains(V4lBufferFlags::ERROR) {
        return Err(V4l2Error::TransportFault);
    }

    let bytes_used = usize::try_from(metadata.bytesused).map_err(|_| V4l2Error::TransportFault)?;
    let frame_bytes = frame_payload_from_mapping(mapping, bytes_used, 0)?;
    if frame_bytes.len() > limits.max_frame_bytes {
        return Err(V4l2Error::TransportFault);
    }

    Ok(EncodedFrame {
        bytes: frame_bytes,
        width_px: stream.mode.width_px,
        height_px: stream.mode.height_px,
        timestamp_ns: timestamp_to_ns(metadata.timestamp)?,
        sequence: u64::from(metadata.sequence),
        format: EncodedFormat::Mjpeg,
    })
}

#[cfg(target_os = "linux")]
fn close_video_capture_state(mut state: VideoCaptureState) -> Result<(), V4l2Error> {
    if let Some(stream) = state.stream.take() {
        close_stream_state(stream)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn ensure_video_capture_stream<'a>(
    state: &'a mut VideoCaptureState,
    device: &Arc<V4lDevice>,
    limits: &V4l2LimitsConfig,
) -> Result<&'a mut StreamState, V4l2Error> {
    if state.stream.is_none() {
        let stream = open_stream_state(device, &state.selected_mode, limits)?;
        state.stream = Some(stream);
    }
    state
        .stream
        .as_mut()
        .ok_or_else(|| V4l2Error::Other("video capture stream is not open".to_string()))
}

#[cfg(target_os = "linux")]
fn video_capture_property_value(
    state: &VideoCaptureState,
    device: &V4lDevice,
    property: CaptureProperty,
) -> Result<f64, V4l2Error> {
    match property {
        CaptureProperty::FrameWidth => Ok(f64::from(
            state
                .selection
                .width_px
                .unwrap_or(state.selected_mode.width_px),
        )),
        CaptureProperty::FrameHeight => Ok(f64::from(
            state
                .selection
                .height_px
                .unwrap_or(state.selected_mode.height_px),
        )),
        CaptureProperty::Fps => Ok(f64::from(
            state.selection.fps.unwrap_or(state.selected_mode.fps_num),
        )),
        CaptureProperty::Fourcc => Ok(f64::from(MJPG_FOURCC_VALUE)),
        _ => get_control_value(device, property),
    }
}

#[cfg(target_os = "linux")]
fn validate_updated_selection(
    state: &VideoCaptureState,
    modes: &[CaptureMode],
    update: impl FnOnce(&mut VideoCaptureSelection) -> Result<(), V4l2Error>,
) -> Result<Option<CaptureMode>, V4l2Error> {
    let mut selection = state.selection.clone();
    update(&mut selection)?;
    Ok(select_best_mode(modes, &selection, &state.selected_mode))
}

#[cfg(target_os = "linux")]
fn set_video_capture_property(
    state: &mut VideoCaptureState,
    device: &V4lDevice,
    modes: &[CaptureMode],
    property: CaptureProperty,
    value: f64,
) -> Result<bool, V4l2Error> {
    match property {
        CaptureProperty::FrameWidth => {
            let rounded = rounded_u32(value)?;
            if rounded == 0 {
                return Err(V4l2Error::InvalidArgument);
            }
            let Some(selected_mode) = validate_updated_selection(state, modes, |selection| {
                selection.width_px = Some(rounded);
                Ok(())
            })?
            else {
                return Ok(false);
            };
            state.selection.width_px = Some(rounded);
            state.selected_mode = selected_mode;
        }
        CaptureProperty::FrameHeight => {
            let rounded = rounded_u32(value)?;
            if rounded == 0 {
                return Err(V4l2Error::InvalidArgument);
            }
            let Some(selected_mode) = validate_updated_selection(state, modes, |selection| {
                selection.height_px = Some(rounded);
                Ok(())
            })?
            else {
                return Ok(false);
            };
            state.selection.height_px = Some(rounded);
            state.selected_mode = selected_mode;
        }
        CaptureProperty::Fps => {
            let rounded = rounded_integer_fps(value)?;
            let Some(selected_mode) = validate_updated_selection(state, modes, |selection| {
                selection.fps = Some(rounded);
                Ok(())
            })?
            else {
                return Ok(false);
            };
            state.selection.fps = Some(rounded);
            state.selected_mode = selected_mode;
        }
        CaptureProperty::Fourcc => {
            let rounded = rounded_u32(value)?;
            if rounded != MJPG_FOURCC_VALUE {
                return Ok(false);
            }
        }
        _ => {
            return set_control_value(device, property, value);
        }
    }

    if let Some(stream) = state.stream.take() {
        close_stream_state(stream)?;
    }
    state.last_frame = None;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn grab_video_capture_frame(
    state: &mut VideoCaptureState,
    device: &Arc<V4lDevice>,
    limits: &V4l2LimitsConfig,
    timeout_ms: u32,
) -> Result<bool, V4l2Error> {
    let stream = ensure_video_capture_stream(state, device, limits)?;
    let encoded = read_next_frame(stream, limits, timeout_ms)?;
    let frame = decode_mjpeg_frame(
        &encoded.bytes,
        limits,
        encoded.sequence,
        encoded.timestamp_ns,
    )?;
    state.last_frame = Some(frame);
    Ok(true)
}

#[cfg(target_os = "linux")]
fn retrieve_video_capture_frame(state: &VideoCaptureState) -> Result<Frame, V4l2Error> {
    state
        .last_frame
        .clone()
        .ok_or_else(|| V4l2Error::Other("no grabbed frame available".to_string()))
}

#[cfg(target_os = "linux")]
fn read_video_capture_frame(
    state: &mut VideoCaptureState,
    device: &Arc<V4lDevice>,
    limits: &V4l2LimitsConfig,
    timeout_ms: u32,
) -> Result<Frame, V4l2Error> {
    grab_video_capture_frame(state, device, limits, timeout_ms)?;
    retrieve_video_capture_frame(state)
}

#[cfg(target_os = "linux")]
fn run_device_thread(
    path: String,
    limits: V4l2LimitsConfig,
    mut receiver: mpsc::Receiver<DeviceCommand>,
    ready: oneshot::Sender<Result<(), V4l2Error>>,
) {
    let (device, info, modes) = match inspect_device(&path) {
        Ok(result) => result,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };

    let mut state = DeviceThreadState {
        device,
        info,
        modes,
        limits,
        active_stream: None,
        active_video_capture: None,
    };
    let _ = ready.send(Ok(()));

    while let Some(command) = receiver.blocking_recv() {
        match command {
            DeviceCommand::Info { reply } => {
                let _ = reply.send(state.info.clone());
            }
            DeviceCommand::ListModes { reply } => {
                let _ = reply.send(state.modes.clone());
            }
            DeviceCommand::OpenStream { mode, reply } => {
                let result =
                    if state.active_stream.is_some() || state.active_video_capture.is_some() {
                        Err(V4l2Error::Busy)
                    } else if !state
                        .modes
                        .iter()
                        .any(|candidate| capture_modes_equal(candidate, &mode))
                    {
                        Err(V4l2Error::InvalidArgument)
                    } else {
                        open_stream_state(&state.device, &mode, &state.limits).map(|stream| {
                            state.active_stream = Some(stream);
                        })
                    };
                let _ = reply.send(result);
            }
            DeviceCommand::CurrentMode { reply } => {
                let result = state
                    .active_stream
                    .as_ref()
                    .map(|stream| stream.mode)
                    .ok_or_else(|| V4l2Error::Other("stream is not open".to_string()));
                let _ = reply.send(result);
            }
            DeviceCommand::NextFrame { timeout_ms, reply } => {
                let result = if let Some(stream) = state.active_stream.as_mut() {
                    read_next_frame(stream, &state.limits, timeout_ms)
                } else {
                    Err(V4l2Error::Other("stream is not open".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::OpenVideoCapture { reply } => {
                let result =
                    if state.active_stream.is_some() || state.active_video_capture.is_some() {
                        Err(V4l2Error::Busy)
                    } else {
                        video_capture_state_from_modes(&state.modes).map(|video_capture| {
                            state.active_video_capture = Some(video_capture);
                        })
                    };
                let _ = reply.send(result);
            }
            DeviceCommand::VideoCaptureIsOpened { reply } => {
                let _ = reply.send(state.active_video_capture.is_some());
            }
            DeviceCommand::VideoCaptureGet { property, reply } => {
                let result = if let Some(video_capture) = state.active_video_capture.as_ref() {
                    video_capture_property_value(video_capture, state.device.as_ref(), property)
                } else {
                    Err(V4l2Error::Other("video capture is released".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::VideoCaptureSet {
                property,
                value,
                reply,
            } => {
                let result = if let Some(video_capture) = state.active_video_capture.as_mut() {
                    set_video_capture_property(
                        video_capture,
                        state.device.as_ref(),
                        &state.modes,
                        property,
                        value,
                    )
                } else {
                    Err(V4l2Error::Other("video capture is released".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::VideoCaptureRead { timeout_ms, reply } => {
                let result = if let Some(video_capture) = state.active_video_capture.as_mut() {
                    read_video_capture_frame(
                        video_capture,
                        &state.device,
                        &state.limits,
                        timeout_ms,
                    )
                } else {
                    Err(V4l2Error::Other("video capture is released".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::VideoCaptureGrab { timeout_ms, reply } => {
                let result = if let Some(video_capture) = state.active_video_capture.as_mut() {
                    grab_video_capture_frame(
                        video_capture,
                        &state.device,
                        &state.limits,
                        timeout_ms,
                    )
                } else {
                    Err(V4l2Error::Other("video capture is released".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::VideoCaptureRetrieve { reply } => {
                let result = if let Some(video_capture) = state.active_video_capture.as_ref() {
                    retrieve_video_capture_frame(video_capture)
                } else {
                    Err(V4l2Error::Other("video capture is released".to_string()))
                };
                let _ = reply.send(result);
            }
            DeviceCommand::CloseStreamNoReply => {
                if let Some(stream) = state.active_stream.take() {
                    let _ = close_stream_state(stream);
                }
            }
            DeviceCommand::CloseVideoCaptureNoReply => {
                if let Some(video_capture) = state.active_video_capture.take() {
                    let _ = close_video_capture_state(video_capture);
                }
            }
            DeviceCommand::Shutdown { reply } => {
                if let Some(stream) = state.active_stream.take() {
                    let _ = close_stream_state(stream);
                }
                if let Some(video_capture) = state.active_video_capture.take() {
                    let _ = close_video_capture_state(video_capture);
                }
                let _ = reply.send(());
                break;
            }
        }
    }

    if let Some(stream) = state.active_stream.take() {
        let _ = close_stream_state(stream);
    }
    if let Some(video_capture) = state.active_video_capture.take() {
        let _ = close_video_capture_state(video_capture);
    }
}

#[cfg(not(target_os = "linux"))]
fn run_device_thread(
    _path: String,
    _limits: V4l2LimitsConfig,
    _receiver: mpsc::Receiver<DeviceCommand>,
    ready: oneshot::Sender<Result<(), V4l2Error>>,
) {
    let _ = ready.send(Err(V4l2Error::OperationNotSupported));
}

async fn start_device_runtime(
    path: String,
    limits: V4l2LimitsConfig,
) -> Result<DeviceRuntimeHandle, V4l2Error> {
    let (sender, receiver) = mpsc::channel::<DeviceCommand>(DEVICE_COMMAND_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = oneshot::channel::<Result<(), V4l2Error>>();
    let thread_limits = limits.clone();

    let file_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("video");
    let thread_name = format!("imago-v4l2-{file_name}");
    let thread_path = path.clone();
    let thread_handle = thread::Builder::new()
        .name(thread_name)
        .stack_size(DEFAULT_THREAD_STACK_BYTES)
        .spawn(move || run_device_thread(thread_path, thread_limits, receiver, ready_tx))
        .map_err(|err| V4l2Error::Other(format!("failed to spawn v4l2 thread: {err}")))?;

    match ready_rx.await {
        Ok(Ok(())) => Ok(DeviceRuntimeHandle {
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
            Err(V4l2Error::Disconnected)
        }
    }
}

async fn request_device<T>(
    sender: &mpsc::Sender<DeviceCommand>,
    build: impl FnOnce(oneshot::Sender<Result<T, V4l2Error>>) -> DeviceCommand,
) -> Result<T, V4l2Error> {
    let (reply_tx, reply_rx) = oneshot::channel();
    sender
        .try_send(build(reply_tx))
        .map_err(map_channel_send_error)?;

    reply_rx.await.unwrap_or(Err(V4l2Error::Disconnected))
}

async fn request_device_value<T>(
    sender: &mpsc::Sender<DeviceCommand>,
    build: impl FnOnce(oneshot::Sender<T>) -> DeviceCommand,
    fallback: T,
) -> T {
    let (reply_tx, reply_rx) = oneshot::channel();
    if sender.try_send(build(reply_tx)).is_err() {
        return fallback;
    }
    reply_rx.await.unwrap_or(fallback)
}

fn map_channel_send_error<T>(err: mpsc::error::TrySendError<T>) -> V4l2Error {
    match err {
        mpsc::error::TrySendError::Full(_) => V4l2Error::Busy,
        mpsc::error::TrySendError::Closed(_) => V4l2Error::Disconnected,
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

async fn send_close_stream_no_reply_command(sender: &mpsc::Sender<DeviceCommand>) {
    match sender.try_send(DeviceCommand::CloseStreamNoReply) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => {}
        Err(mpsc::error::TrySendError::Full(command)) => {
            let _ = sender.send(command).await;
        }
    }
}

async fn send_close_video_capture_no_reply_command(sender: &mpsc::Sender<DeviceCommand>) {
    match sender.try_send(DeviceCommand::CloseVideoCaptureNoReply) {
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

impl imago_v4l2_plugin_bindings::imago::v4l2::provider::Host for WasiState {
    async fn list_openable_paths(&mut self) -> Vec<String> {
        load_v4l2_resources_for_state_or_default(self).paths
    }

    async fn list_openable_devices(&mut self) -> Result<Vec<OpenableDevice>, V4l2Error> {
        ensure_v4l2_supported()?;
        let resources = load_v4l2_resources_for_state(self)?;
        let allowlist = resources.allowlist;

        tokio::task::spawn_blocking(move || enumerate_openable_devices(&allowlist))
            .await
            .map_err(|err| V4l2Error::Other(format!("v4l2 enumeration task failed: {err}")))
    }

    async fn get_limits(&mut self) -> Limits {
        let resources = load_v4l2_resources_for_state_or_default(self);
        to_limits_record(&resources.limits)
    }

    async fn open_device(&mut self, path: String) -> Result<Resource<DeviceResource>, V4l2Error> {
        ensure_v4l2_supported()?;
        let resources = load_v4l2_resources_for_state(self)?;
        let normalized = normalize_video_path(&path).map_err(|_| V4l2Error::InvalidArgument)?;
        if !resources.allowlist.contains(&normalized) {
            return Err(V4l2Error::NotAllowed);
        }

        let runtime_handle = start_device_runtime(normalized.clone(), resources.limits).await?;
        let rep = register_device_handle(runtime_handle).map_err(map_lookup_error)?;
        Ok(Resource::new_own(rep))
    }
}

impl imago_v4l2_plugin_bindings::imago::v4l2::types::Host for WasiState {}

impl imago_v4l2_plugin_bindings::imago::v4l2::device::Host for WasiState {}

impl imago_v4l2_plugin_bindings::imago::v4l2::device::HostDevice for WasiState {
    async fn info(&mut self, self_: Resource<DeviceResource>) -> OpenableDevice {
        let Ok(handle) = lookup_device_handle(self_.rep()) else {
            return default_openable_device();
        };
        request_device_value(
            &handle.sender,
            |reply| DeviceCommand::Info { reply },
            default_openable_device(),
        )
        .await
    }

    async fn list_modes(&mut self, self_: Resource<DeviceResource>) -> Vec<CaptureMode> {
        let Ok(handle) = lookup_device_handle(self_.rep()) else {
            return Vec::new();
        };
        request_device_value(
            &handle.sender,
            |reply| DeviceCommand::ListModes { reply },
            Vec::new(),
        )
        .await
    }

    async fn open_stream(
        &mut self,
        self_: Resource<DeviceResource>,
        mode: CaptureMode,
    ) -> Result<Resource<StreamResource>, V4l2Error> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::OpenStream {
            mode,
            reply,
        })
        .await?;
        let rep = register_stream_handle(StreamHandle {
            sender: handle.sender,
        })
        .map_err(map_lookup_error)?;
        Ok(Resource::new_own(rep))
    }

    async fn open_video_capture(
        &mut self,
        self_: Resource<DeviceResource>,
    ) -> Result<Resource<VideoCaptureResource>, V4l2Error> {
        let handle = lookup_device_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::OpenVideoCapture {
            reply,
        })
        .await?;
        let rep = register_video_capture_handle(VideoCaptureHandle {
            sender: handle.sender,
        })
        .map_err(map_lookup_error)?;
        Ok(Resource::new_own(rep))
    }

    async fn drop(&mut self, resource: Resource<DeviceResource>) -> wasmtime::Result<()> {
        if let Ok(handle) = lookup_device_handle(resource.rep()) {
            let _ = remove_stream_handles_for_sender(&handle.sender);
            let _ = remove_video_capture_handles_for_sender(&handle.sender);
            shutdown_device_runtime(&handle).await;
        }
        remove_device_handle(resource.rep());
        Ok(())
    }
}

impl imago_v4l2_plugin_bindings::imago::v4l2::capture_stream::Host for WasiState {}

impl imago_v4l2_plugin_bindings::imago::v4l2::capture_stream::HostCaptureStream for WasiState {
    async fn current_mode(&mut self, self_: Resource<StreamResource>) -> CaptureMode {
        let Ok(handle) = lookup_stream_handle(self_.rep()) else {
            return default_capture_mode();
        };
        request_device(&handle.sender, |reply| DeviceCommand::CurrentMode { reply })
            .await
            .unwrap_or_else(|_| default_capture_mode())
    }

    async fn next_frame(
        &mut self,
        self_: Resource<StreamResource>,
        timeout_ms: u32,
    ) -> Result<EncodedFrame, V4l2Error> {
        let handle = lookup_stream_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::NextFrame {
            timeout_ms,
            reply,
        })
        .await
    }

    async fn drop(&mut self, resource: Resource<StreamResource>) -> wasmtime::Result<()> {
        if let Ok(handle) = lookup_stream_handle(resource.rep()) {
            send_close_stream_no_reply_command(&handle.sender).await;
        }
        remove_stream_handle(resource.rep());
        Ok(())
    }
}

impl imago_v4l2_plugin_bindings::imago::v4l2::video_capture::Host for WasiState {}

impl imago_v4l2_plugin_bindings::imago::v4l2::video_capture::HostVideoCapture for WasiState {
    async fn is_opened(&mut self, self_: Resource<VideoCaptureResource>) -> bool {
        let Ok(handle) = lookup_video_capture_handle(self_.rep()) else {
            return false;
        };
        request_device_value(
            &handle.sender,
            |reply| DeviceCommand::VideoCaptureIsOpened { reply },
            false,
        )
        .await
    }

    async fn get(
        &mut self,
        self_: Resource<VideoCaptureResource>,
        property: CaptureProperty,
    ) -> Result<f64, V4l2Error> {
        let handle = lookup_video_capture_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::VideoCaptureGet {
            property,
            reply,
        })
        .await
    }

    async fn set(
        &mut self,
        self_: Resource<VideoCaptureResource>,
        property: CaptureProperty,
        value: f64,
    ) -> Result<bool, V4l2Error> {
        let handle = lookup_video_capture_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::VideoCaptureSet {
            property,
            value,
            reply,
        })
        .await
    }

    async fn read(
        &mut self,
        self_: Resource<VideoCaptureResource>,
        timeout_ms: u32,
    ) -> Result<Frame, V4l2Error> {
        let handle = lookup_video_capture_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::VideoCaptureRead {
            timeout_ms,
            reply,
        })
        .await
    }

    async fn grab(
        &mut self,
        self_: Resource<VideoCaptureResource>,
        timeout_ms: u32,
    ) -> Result<bool, V4l2Error> {
        let handle = lookup_video_capture_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| DeviceCommand::VideoCaptureGrab {
            timeout_ms,
            reply,
        })
        .await
    }

    async fn retrieve(
        &mut self,
        self_: Resource<VideoCaptureResource>,
    ) -> Result<Frame, V4l2Error> {
        let handle = lookup_video_capture_handle(self_.rep()).map_err(map_lookup_error)?;
        request_device(&handle.sender, |reply| {
            DeviceCommand::VideoCaptureRetrieve { reply }
        })
        .await
    }

    async fn release(&mut self, self_: Resource<VideoCaptureResource>) {
        if let Ok(handle) = lookup_video_capture_handle(self_.rep()) {
            send_close_video_capture_no_reply_command(&handle.sender).await;
        }
    }

    async fn drop(&mut self, resource: Resource<VideoCaptureResource>) -> wasmtime::Result<()> {
        if let Ok(handle) = lookup_video_capture_handle(resource.rep()) {
            send_close_video_capture_no_reply_command(&handle.sender).await;
        }
        remove_video_capture_handle(resource.rep());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resources_with_v4l2(value: JsonValue) -> BTreeMap<String, JsonValue> {
        BTreeMap::from([(V4L2_RESOURCE_KEY.to_string(), value)])
    }

    #[test]
    fn normalize_video_path_requires_absolute_video_node() {
        let err = normalize_video_path("dev/video0").expect_err("relative path must fail");
        assert!(err.contains("absolute"), "unexpected error: {err}");

        let err = normalize_video_path("/dev/media0").expect_err("non-video path must fail");
        assert!(err.contains("/dev/video"), "unexpected error: {err}");
    }

    #[test]
    fn normalize_video_path_rejects_empty_nul_and_non_numeric_suffix() {
        let err = normalize_video_path(" ").expect_err("empty path must fail");
        assert!(err.contains("must not be empty"), "unexpected error: {err}");

        let err = normalize_video_path("/dev/\0video0").expect_err("NUL path must fail");
        assert!(err.contains("NUL"), "unexpected error: {err}");

        let err = normalize_video_path("/dev/videoX").expect_err("non numeric suffix must fail");
        assert!(err.contains("/dev/video"), "unexpected error: {err}");
    }

    #[test]
    fn parse_v4l2_resources_requires_table_and_paths() {
        let err = parse_v4l2_resources_config(&BTreeMap::new())
            .expect_err("missing v4l2 resource should fail");
        assert!(
            err.contains("resources.v4l2 is required"),
            "unexpected error: {err}"
        );

        let err = parse_v4l2_resources_config(&resources_with_v4l2(json!({})))
            .expect_err("missing paths should fail");
        assert!(err.contains("paths is required"), "unexpected error: {err}");
    }

    #[test]
    fn parse_v4l2_resources_applies_defaults_and_rejects_duplicates() {
        let config = parse_v4l2_resources_config(&resources_with_v4l2(json!({
            "paths": ["/dev/video0"]
        })))
        .expect("default limits should parse");
        assert_eq!(config.paths, vec!["/dev/video0".to_string()]);
        assert_eq!(config.limits, V4l2LimitsConfig::default());

        let err = parse_v4l2_resources_config(&resources_with_v4l2(json!({
            "paths": ["/dev/video0", "/dev/./video0"]
        })))
        .expect_err("duplicate normalized path must fail");
        assert!(
            err.contains("duplicates normalized path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_v4l2_resources_rejects_out_of_range_limits() {
        let err = parse_v4l2_resources_config(&resources_with_v4l2(json!({
            "paths": ["/dev/video0"],
            "max_frame_bytes": 0
        })))
        .expect_err("zero max_frame_bytes must fail");
        assert!(err.contains("max_frame_bytes"), "unexpected error: {err}");

        let err = parse_v4l2_resources_config(&resources_with_v4l2(json!({
            "paths": ["/dev/video0"],
            "buffer_count": 0
        })))
        .expect_err("zero buffer_count must fail");
        assert!(err.contains("buffer_count"), "unexpected error: {err}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rgb_to_rgba_requires_complete_pixels() {
        let err = rgb_to_rgba(&[0x01, 0x02]).expect_err("incomplete RGB pixels should fail");
        assert!(matches!(err, V4l2Error::TransportFault));
        assert_eq!(
            rgb_to_rgba(&[1, 2, 3, 4, 5, 6]).expect("RGB conversion should succeed"),
            vec![1, 2, 3, 0xff, 4, 5, 6, 0xff]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn l8_to_rgba_converts_grayscale_with_alpha() {
        assert_eq!(
            l8_to_rgba(&[0x00, 0x7f, 0xff]).expect("L8 conversion should succeed"),
            vec![
                0x00, 0x00, 0x00, 0xff, 0x7f, 0x7f, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn l16_to_rgba_converts_u16_grayscale_to_rgba() {
        assert_eq!(
            l16_to_rgba(&[0x12, 0x12, 0x34, 0x34]).expect("L16 conversion should succeed"),
            vec![0x12, 0x12, 0x12, 0xff, 0x34, 0x34, 0x34, 0xff]
        );
        let err = l16_to_rgba(&[0x12]).expect_err("incomplete L16 pixels should fail");
        assert!(matches!(err, V4l2Error::TransportFault));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cmyk32_to_rgba_converts_cmyk_to_rgba() {
        assert_eq!(
            cmyk32_to_rgba(&[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff,
            ])
            .expect("CMYK conversion should succeed"),
            vec![
                255, 255, 255, 0xff, 127, 127, 127, 0xff, 0x00, 0x00, 0x00, 0xff,
            ]
        );
        let err =
            cmyk32_to_rgba(&[0x00, 0x00, 0x00]).expect_err("incomplete CMYK pixels should fail");
        assert!(matches!(err, V4l2Error::TransportFault));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn frame_payload_from_mapping_trims_trailing_padding() {
        assert_eq!(
            frame_payload_from_mapping(&[1, 2, 3, 4, 5, 6], 5, 1)
                .expect("payload extraction should succeed"),
            vec![1, 2, 3, 4]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn frame_payload_from_mapping_rejects_invalid_lengths() {
        let err = frame_payload_from_mapping(&[1, 2], 1, 2)
            .expect_err("bytes_used smaller than data_offset should fail");
        assert!(matches!(err, V4l2Error::TransportFault));

        let err = frame_payload_from_mapping(&[1, 2], 4, 1).expect_err("short mapping should fail");
        assert!(matches!(err, V4l2Error::TransportFault));
    }

    #[test]
    fn fps_ratio_inverts_time_per_frame() {
        assert_eq!(fps_ratio_from_time_per_frame(1, 30), Some((30, 1)));
        assert_eq!(fps_ratio_from_time_per_frame(0, 30), None);
        assert_eq!(fps_ratio_from_time_per_frame(1, 0), None);
    }

    #[test]
    fn expand_stepwise_u32_values_enumerates_exact_sizes() {
        assert_eq!(
            expand_stepwise_u32_values(640, 1280, 320, MAX_EXPANDED_CAPTURE_MODES)
                .expect("stepwise sizes should expand"),
            vec![640, 960, 1280]
        );
    }

    #[test]
    fn expand_stepwise_u32_values_rejects_zero_step_and_limit_excess() {
        assert!(matches!(
            expand_stepwise_u32_values(640, 1280, 0, MAX_EXPANDED_CAPTURE_MODES)
                .expect_err("zero step must fail"),
            V4l2Error::OperationNotSupported
        ));
        assert!(matches!(
            expand_stepwise_u32_values(0, 4_096, 1, MAX_EXPANDED_CAPTURE_MODES)
                .expect_err("more than 4096 entries must fail"),
            V4l2Error::OperationNotSupported
        ));
    }

    #[test]
    fn expand_stepwise_frame_intervals_enumerates_exact_rational_steps() {
        assert_eq!(
            expand_stepwise_frame_intervals(
                FrameInterval {
                    numerator: 1,
                    denominator: 30,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 15,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 60,
                },
                MAX_EXPANDED_CAPTURE_MODES,
            )
            .expect("stepwise intervals should expand"),
            vec![
                FrameInterval {
                    numerator: 1,
                    denominator: 30,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 20,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 15,
                },
            ]
        );
    }

    #[test]
    fn expand_stepwise_frame_intervals_fail_closes_on_invalid_or_overflowing_ranges() {
        assert!(matches!(
            expand_stepwise_frame_intervals(
                FrameInterval {
                    numerator: 1,
                    denominator: 30,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 15,
                },
                FrameInterval {
                    numerator: 0,
                    denominator: 1,
                },
                MAX_EXPANDED_CAPTURE_MODES,
            )
            .expect_err("zero step must fail"),
            V4l2Error::OperationNotSupported
        ));
        assert!(matches!(
            expand_stepwise_frame_intervals(
                FrameInterval {
                    numerator: 1,
                    denominator: 65_537,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 32_768,
                },
                FrameInterval {
                    numerator: 1,
                    denominator: 65_539,
                },
                MAX_EXPANDED_CAPTURE_MODES,
            )
            .expect_err("overflowing reduced fraction must fail"),
            V4l2Error::OperationNotSupported
        ));
    }

    #[test]
    fn select_best_mode_prefers_matching_candidates_with_smallest_delta() {
        let modes = vec![
            CaptureMode {
                format: EncodedFormat::Mjpeg,
                width_px: 640,
                height_px: 480,
                fps_num: 30,
                fps_den: 1,
            },
            CaptureMode {
                format: EncodedFormat::Mjpeg,
                width_px: 1280,
                height_px: 720,
                fps_num: 30,
                fps_den: 1,
            },
            CaptureMode {
                format: EncodedFormat::Mjpeg,
                width_px: 1280,
                height_px: 720,
                fps_num: 60,
                fps_den: 1,
            },
        ];
        let selection = VideoCaptureSelection {
            width_px: Some(1280),
            height_px: Some(720),
            fps: None,
        };
        let baseline = modes[1];
        let selected =
            select_best_mode(&modes, &selection, &baseline).expect("matching mode should exist");
        assert_eq!(selected.width_px, 1280);
        assert_eq!(selected.height_px, 720);
        assert_eq!(selected.fps_num, 60);
        assert_eq!(selected.fps_den, 1);
    }

    #[test]
    fn select_best_mode_returns_none_when_integer_fps_has_no_exact_match() {
        let modes = vec![
            CaptureMode {
                format: EncodedFormat::Mjpeg,
                width_px: 640,
                height_px: 480,
                fps_num: 30,
                fps_den: 1,
            },
            CaptureMode {
                format: EncodedFormat::Mjpeg,
                width_px: 640,
                height_px: 480,
                fps_num: 15,
                fps_den: 1,
            },
        ];
        let selection = VideoCaptureSelection {
            width_px: Some(640),
            height_px: Some(480),
            fps: Some(24),
        };
        assert!(select_best_mode(&modes, &selection, &modes[0]).is_none());
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn resolve_usb_metadata_walks_up_sysfs_ancestors() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "imago-v4l2-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let usb_interface = root.join("devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.0");
        let usb_device = usb_interface
            .parent()
            .expect("usb interface should have device parent")
            .to_path_buf();
        fs::create_dir_all(&usb_interface).expect("usb interface dir should exist");
        fs::create_dir_all(root.join("class/video4linux/video0"))
            .expect("video class dir should exist");
        fs::write(usb_device.join("idVendor"), "0c45\n").expect("idVendor should be written");
        fs::write(usb_device.join("idProduct"), "6366\n").expect("idProduct should be written");
        fs::write(usb_device.join("busnum"), "1\n").expect("busnum should be written");
        fs::write(usb_device.join("devnum"), "7\n").expect("devnum should be written");
        symlink(&usb_interface, root.join("class/video4linux/video0/device"))
            .expect("device symlink should be created");

        let metadata = resolve_usb_metadata_from_sys_root(&root, "/dev/video0")
            .expect("usb metadata should resolve");
        assert_eq!(metadata.vendor_id, 0x0c45);
        assert_eq!(metadata.product_id, 0x6366);
        assert_eq!(metadata.bus, 1);
        assert_eq!(metadata.address, 7);

        let _ = fs::remove_dir_all(root);
    }
}
