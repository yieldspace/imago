pub(crate) const LOCAL_ENDPOINT: &str = "http://127.0.0.1:80";
pub(crate) const DEFAULT_HTTP_PORT: u16 = 80;
pub(crate) const TOKEN_COOKIE_NAME: &str = "nano-kvm-token";
pub(crate) const MAX_MJPEG_HEADER_LINES: usize = 64;
pub(crate) const MAX_MJPEG_HEADER_LINE_BYTES: usize = 8 * 1024;
pub(crate) const MAX_JPEG_BYTES: usize = 10 * 1024 * 1024;

pub(crate) const NANOKVM_ROOT: &str = "/kvmapp";
pub(crate) const NANOKVM_ETC: &str = "/etc/kvm";
pub(crate) const HW_VERSION_PATH: &str = "/etc/kvm/hw";
pub(crate) const SERVER_CONFIG_YAML_PATH: &str = "/etc/kvm/server.yaml";

pub(crate) const STREAM_TYPE_PATH: &str = "/kvmapp/kvm/type";
pub(crate) const STREAM_RESOLUTION_PATH: &str = "/kvmapp/kvm/res";
pub(crate) const STREAM_WIDTH_PATH: &str = "/kvmapp/kvm/width";
pub(crate) const STREAM_HEIGHT_PATH: &str = "/kvmapp/kvm/height";
pub(crate) const STREAM_FPS_PATH: &str = "/kvmapp/kvm/fps";
pub(crate) const STREAM_QUALITY_PATH: &str = "/kvmapp/kvm/qlty";
pub(crate) const STREAM_NOW_FPS_PATH: &str = "/kvmapp/kvm/now_fps";

pub(crate) const FPS_MIN: u8 = 10;
pub(crate) const FPS_MAX: u8 = 60;
pub(crate) const QUALITY_MIN: u16 = 50;
pub(crate) const QUALITY_MAX: u16 = 100;

pub(crate) const USB_MODE_FLAG_PATH: &str = "/sys/kernel/config/usb_gadget/g0/bcdDevice";
pub(crate) const HDMI_STATE_PATH: &str =
    "/sys/devices/platform/soc/30b60000.hdmi/extcon/hdmi/state";
pub(crate) const ETHERNET_OPERSTATE_PATH: &str = "/sys/class/net/eth0/operstate";
pub(crate) const WIFI_OPERSTATE_PATH: &str = "/sys/class/net/wlan0/operstate";
pub(crate) const WIFI_SUPPORTED_FILE: &str = "/etc/kvm/wifi_exist";
pub(crate) const POWER_LED_GPIO_PATH: &str = "/sys/class/gpio/gpio504/value";
pub(crate) const HDD_LED_GPIO_PATH: &str = "/sys/class/gpio/gpio505/value";

pub(crate) const RUNTIME_SCRIPT_PATH: &str = "/etc/init.d/S95nanokvm";
pub(crate) const RUNTIME_SCRIPT_WATCHDOG_ENABLED: &str =
    "/kvmapp/system/init.d/S95nanokvm_watchdog";
pub(crate) const RUNTIME_SCRIPT_WATCHDOG_DISABLED: &str =
    "/kvmapp/system/init.d/S95nanokvm_no_watchdog";
pub(crate) const RUNTIME_SCRIPT_STOP_PING_ENABLED: &str =
    "/kvmapp/system/init.d/S95nanokvm_stop_ping";
pub(crate) const RUNTIME_SCRIPT_STOP_PING_DISABLED: &str =
    "/kvmapp/system/init.d/S95nanokvm_no_stop_ping";

pub(crate) const HID_MODE_NORMAL_SCRIPT: &str = "/kvmapp/system/init.d/S03usbdev";
pub(crate) const HID_MODE_HID_ONLY_SCRIPT: &str = "/kvmapp/system/init.d/S03usbhid";
pub(crate) const HID_MODE_TARGET_SCRIPT: &str = "/etc/init.d/S03usbdev";
pub(crate) const HID_DEVICE_KEYBOARD: &str = "/dev/hidg0";
pub(crate) const HID_DEVICE_RELATIVE_MOUSE: &str = "/dev/hidg1";
pub(crate) const HID_DEVICE_ABSOLUTE_MOUSE: &str = "/dev/hidg2";

pub(crate) const GPIO_POWER_ALPHA_BETA_PCIE: &str = "/sys/class/gpio/gpio503/value";
pub(crate) const GPIO_RESET_ALPHA: &str = "/sys/class/gpio/gpio507/value";
pub(crate) const GPIO_RESET_BETA_PCIE: &str = "/sys/class/gpio/gpio505/value";
pub(crate) const GPIO_PULSE_DEFAULT_MS: u32 = 800;
