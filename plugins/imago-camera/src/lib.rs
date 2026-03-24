#[cfg(any(target_arch = "wasm32", test))]
const CAMERA_ID_PREFIX: &str = "v4l2:";

#[cfg(any(target_arch = "wasm32", test))]
fn camera_id_from_path(path: &str) -> String {
    format!("{CAMERA_ID_PREFIX}{path}")
}

#[cfg(any(target_arch = "wasm32", test))]
fn path_sort_key(path: &str) -> (u32, &str) {
    let index = path
        .rsplit_once("video")
        .and_then(|(_, suffix)| suffix.parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    (index, path)
}

#[cfg(any(target_arch = "wasm32", test))]
fn is_camera_device_path(path: &str) -> bool {
    !path.is_empty() && path.starts_with("/dev/video")
}

#[cfg(target_arch = "wasm32")]
mod component {
    use std::cell::{Cell, RefCell};

    use super::camera_id_from_path;
    use super::is_camera_device_path;
    use super::path_sort_key;

    wit_bindgen::generate!({
        path: "wit",
        world: "camera-plugin",
        generate_all
    });

    use self::exports::imago::camera as camera_exports;
    use self::imago::v4l20_2_0 as v4l2;

    type CameraInfo = camera_exports::types::CameraInfo;
    type CameraError = camera_exports::types::CameraError;
    type CaptureProperty = camera_exports::types::CaptureProperty;
    type Frame = camera_exports::types::Frame;
    type PixelFormat = camera_exports::types::PixelFormat;
    type VideoCaptureResource = camera_exports::provider::VideoCapture;
    type V4l2CaptureProperty = v4l2::types::CaptureProperty;
    type V4l2Device = v4l2::device::Device;
    type V4l2Error = v4l2::types::V4l2Error;
    type V4l2Frame = v4l2::types::Frame;
    type V4l2OpenableDevice = v4l2::types::OpenableDevice;
    type V4l2VideoCapture = v4l2::video_capture::VideoCapture;

    struct CameraPlugin;

    struct CameraEntry {
        info: CameraInfo,
        path: String,
    }

    struct CameraVideoCaptureState {
        device: Option<V4l2Device>,
        capture: Option<V4l2VideoCapture>,
    }

    struct CameraVideoCapture {
        state: RefCell<CameraVideoCaptureState>,
        released: Cell<bool>,
    }

    fn camera_info_from_openable_device(index: u32, device: &V4l2OpenableDevice) -> CameraInfo {
        CameraInfo {
            index,
            id: camera_id_from_path(&device.path),
            label: if device.label.is_empty() {
                device.path.clone()
            } else {
                device.label.clone()
            },
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            bus: device.bus,
            address: device.address,
        }
    }

    fn camera_property_from_v4l2(property: CaptureProperty) -> V4l2CaptureProperty {
        match property {
            CaptureProperty::FrameWidth => V4l2CaptureProperty::FrameWidth,
            CaptureProperty::FrameHeight => V4l2CaptureProperty::FrameHeight,
            CaptureProperty::Fps => V4l2CaptureProperty::Fps,
            CaptureProperty::Fourcc => V4l2CaptureProperty::Fourcc,
            CaptureProperty::Brightness => V4l2CaptureProperty::Brightness,
            CaptureProperty::Contrast => V4l2CaptureProperty::Contrast,
            CaptureProperty::Saturation => V4l2CaptureProperty::Saturation,
            CaptureProperty::Gain => V4l2CaptureProperty::Gain,
            CaptureProperty::AutoExposure => V4l2CaptureProperty::AutoExposure,
            CaptureProperty::Exposure => V4l2CaptureProperty::Exposure,
            CaptureProperty::AutoFocus => V4l2CaptureProperty::AutoFocus,
            CaptureProperty::Focus => V4l2CaptureProperty::Focus,
        }
    }

    fn camera_frame_from_v4l2(frame: V4l2Frame) -> Frame {
        Frame {
            bytes: frame.bytes,
            width_px: frame.width_px,
            height_px: frame.height_px,
            stride_bytes: frame.stride_bytes,
            timestamp_ns: frame.timestamp_ns,
            sequence: frame.sequence,
            format: PixelFormat::Rgba8,
        }
    }

    fn map_v4l2_error(error: V4l2Error) -> CameraError {
        match error {
            V4l2Error::NotAllowed => CameraError::NotAllowed,
            V4l2Error::Timeout => CameraError::Timeout,
            V4l2Error::Disconnected => CameraError::Disconnected,
            V4l2Error::Busy => CameraError::Busy,
            V4l2Error::InvalidArgument => CameraError::InvalidArgument,
            V4l2Error::TransportFault => CameraError::TransportFault,
            V4l2Error::OperationNotSupported => CameraError::NotSupported,
            V4l2Error::Other(message) => CameraError::Other(message),
        }
    }

    fn camera_error_other(message: impl Into<String>) -> CameraError {
        CameraError::Other(message.into())
    }

    fn enumerate_camera_entries() -> Result<Vec<CameraEntry>, CameraError> {
        let mut openable_devices =
            v4l2::provider::list_openable_devices().map_err(map_v4l2_error)?;
        openable_devices.retain(|device| is_camera_device_path(&device.path));
        openable_devices
            .sort_by(|left, right| path_sort_key(&left.path).cmp(&path_sort_key(&right.path)));

        Ok(openable_devices
            .into_iter()
            .enumerate()
            .map(|(index, device)| CameraEntry {
                info: camera_info_from_openable_device(
                    u32::try_from(index).unwrap_or(u32::MAX),
                    &device,
                ),
                path: device.path,
            })
            .collect())
    }

    fn lookup_camera_by_index(index: u32) -> Result<CameraEntry, CameraError> {
        enumerate_camera_entries()?
            .into_iter()
            .find(|entry| entry.info.index == index)
            .ok_or(CameraError::NotFound)
    }

    impl CameraVideoCapture {
        fn with_capture<T>(
            &self,
            f: impl FnOnce(&V4l2VideoCapture) -> Result<T, CameraError>,
        ) -> Result<T, CameraError> {
            if self.released.get() {
                return Err(camera_error_other("video capture is released"));
            }

            let state = self.state.borrow();
            let capture = state
                .capture
                .as_ref()
                .ok_or_else(|| camera_error_other("video capture is released"))?;
            f(capture)
        }

        fn release_resources(&self) {
            if self.released.replace(true) {
                return;
            }
            let mut state = self.state.borrow_mut();
            state.capture.take();
            state.device.take();
        }
    }

    impl Drop for CameraVideoCapture {
        fn drop(&mut self) {
            self.release_resources();
        }
    }

    impl camera_exports::provider::Guest for CameraPlugin {
        type VideoCapture = CameraVideoCapture;

        fn list_cameras() -> Result<Vec<CameraInfo>, CameraError> {
            Ok(enumerate_camera_entries()?
                .into_iter()
                .map(|entry| entry.info)
                .collect())
        }

        fn open(index: u32) -> Result<VideoCaptureResource, CameraError> {
            let entry = lookup_camera_by_index(index)?;
            let device = v4l2::provider::open_device(&entry.path).map_err(map_v4l2_error)?;
            let capture = device.open_video_capture().map_err(map_v4l2_error)?;
            Ok(VideoCaptureResource::new(CameraVideoCapture {
                state: RefCell::new(CameraVideoCaptureState {
                    device: Some(device),
                    capture: Some(capture),
                }),
                released: Cell::new(false),
            }))
        }
    }

    impl camera_exports::provider::GuestVideoCapture for CameraVideoCapture {
        fn is_opened(&self) -> bool {
            if self.released.get() {
                return false;
            }
            let state = self.state.borrow();
            let Some(capture) = state.capture.as_ref() else {
                return false;
            };
            capture.is_opened()
        }

        fn get(&self, property: CaptureProperty) -> Result<f64, CameraError> {
            self.with_capture(|capture: &V4l2VideoCapture| {
                capture
                    .get(camera_property_from_v4l2(property))
                    .map_err(map_v4l2_error)
            })
        }

        fn set(&self, property: CaptureProperty, value: f64) -> Result<bool, CameraError> {
            self.with_capture(|capture: &V4l2VideoCapture| {
                capture
                    .set(camera_property_from_v4l2(property), value)
                    .map_err(map_v4l2_error)
            })
        }

        fn read(&self, timeout_ms: u32) -> Result<Frame, CameraError> {
            self.with_capture(|capture: &V4l2VideoCapture| {
                capture
                    .read(timeout_ms)
                    .map(camera_frame_from_v4l2)
                    .map_err(map_v4l2_error)
            })
        }

        fn grab(&self, timeout_ms: u32) -> Result<bool, CameraError> {
            self.with_capture(|capture: &V4l2VideoCapture| {
                capture.grab(timeout_ms).map_err(map_v4l2_error)
            })
        }

        fn retrieve(&self) -> Result<Frame, CameraError> {
            self.with_capture(|capture: &V4l2VideoCapture| {
                capture
                    .retrieve()
                    .map(camera_frame_from_v4l2)
                    .map_err(map_v4l2_error)
            })
        }

        fn release(&self) {
            if self.released.get() {
                return;
            }
            {
                let state = self.state.borrow();
                if let Some(capture) = state.capture.as_ref() {
                    capture.release();
                }
            }
            self.release_resources();
        }
    }

    export!(CameraPlugin);
}

#[cfg(test)]
mod tests {
    use super::camera_id_from_path;
    use super::is_camera_device_path;
    use super::path_sort_key;

    #[test]
    fn camera_id_from_path_prefixes_v4l2_scheme() {
        assert_eq!(camera_id_from_path("/dev/video0"), "v4l2:/dev/video0");
    }

    #[test]
    fn is_camera_device_path_accepts_video_nodes_without_usb_metadata() {
        assert!(is_camera_device_path("/dev/video0"));
        assert!(is_camera_device_path("/dev/video1"));
        assert!(!is_camera_device_path("/dev/null"));
    }

    #[test]
    fn path_sort_key_uses_numeric_video_index() {
        assert!(path_sort_key("/dev/video2") < path_sort_key("/dev/video10"));
    }
}
