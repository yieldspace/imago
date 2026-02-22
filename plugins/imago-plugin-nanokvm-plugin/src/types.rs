use crate::imago_nanokvm_plugin_bindings;

pub(crate) type CaptureSession = imago_nanokvm_plugin_bindings::imago::nanokvm::capture::Session;
pub(crate) type CaptureAuth = imago_nanokvm_plugin_bindings::imago::nanokvm::capture::Auth;
pub(crate) type InputStream = imago_nanokvm_plugin_bindings::wasi::io::streams::InputStream;

pub(crate) type StreamType =
    imago_nanokvm_plugin_bindings::imago::nanokvm::stream_config::StreamType;
pub(crate) type Resolution =
    imago_nanokvm_plugin_bindings::imago::nanokvm::stream_config::Resolution;
pub(crate) type StreamHardwareVersion =
    imago_nanokvm_plugin_bindings::imago::nanokvm::stream_config::HardwareVersion;
pub(crate) type StreamSettings =
    imago_nanokvm_plugin_bindings::imago::nanokvm::stream_config::StreamSettings;

pub(crate) type UsbMode = imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::UsbMode;
pub(crate) type HdmiStatus =
    imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::HdmiStatus;
pub(crate) type LinkStatus =
    imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::LinkStatus;
pub(crate) type FeatureStatus =
    imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::FeatureStatus;
pub(crate) type LedStatus = imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::LedStatus;
pub(crate) type LedStates = imago_nanokvm_plugin_bindings::imago::nanokvm::device_status::LedStates;

pub(crate) type ToggleState =
    imago_nanokvm_plugin_bindings::imago::nanokvm::runtime_control::ToggleState;

pub(crate) type HidMode = imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::HidMode;
pub(crate) type KeyboardLayout =
    imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::KeyboardLayout;
pub(crate) type KeyboardEvent =
    imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::KeyboardEvent;
pub(crate) type RelativeMouseEvent =
    imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::RelativeMouseEvent;
pub(crate) type AbsoluteMouseEvent =
    imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::AbsoluteMouseEvent;
pub(crate) type TouchEvent = imago_nanokvm_plugin_bindings::imago::nanokvm::hid_control::TouchEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardwareKind {
    Alpha,
    Beta,
    Pcie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpioPulseKind {
    Power,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PasteKey {
    pub(crate) modifiers: u8,
    pub(crate) code: u8,
}
