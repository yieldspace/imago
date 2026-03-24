#[cfg(any(target_arch = "wasm32", test))]
const CAMERA_ID_PREFIX: &str = "v4l2:";

#[cfg(any(target_arch = "wasm32", test))]
fn camera_id_from_path(path: &str) -> String {
    format!("{CAMERA_ID_PREFIX}{path}")
}

#[cfg(any(target_arch = "wasm32", test))]
fn camera_path_from_id(camera_id: &str) -> Option<&str> {
    let path = camera_id.strip_prefix(CAMERA_ID_PREFIX)?;
    if path.is_empty() || !path.starts_with("/dev/video") {
        return None;
    }
    Some(path)
}

#[cfg(any(target_arch = "wasm32", test))]
fn is_camera_device_path(path: &str) -> bool {
    !path.is_empty() && path.starts_with("/dev/video")
}

#[cfg(target_arch = "wasm32")]
mod component {
    use std::cell::{Cell, RefCell};

    use super::camera_id_from_path;
    use super::camera_path_from_id;
    use super::is_camera_device_path;

    wit_bindgen::generate!({
        path: "wit",
        world: "camera-plugin",
        generate_all
    });

    type CameraInfo = exports::imago::camera::types::CameraInfo;
    type CameraError = exports::imago::camera::types::CameraError;
    type CaptureMode = exports::imago::camera::types::CaptureMode;
    type EncodedFrame = exports::imago::camera::types::EncodedFrame;
    type EncodedFormat = exports::imago::camera::types::EncodedFormat;
    type Session = exports::imago::camera::provider::Session;
    type V4l2Device = imago::v4l2::device::Device;
    type V4l2Stream = imago::v4l2::capture_stream::CaptureStream;
    type V4l2CaptureMode = imago::v4l2::types::CaptureMode;
    type V4l2EncodedFrame = imago::v4l2::types::EncodedFrame;
    type V4l2OpenableDevice = imago::v4l2::types::OpenableDevice;

    struct CameraPlugin;

    struct CameraSession {
        mode: CaptureMode,
        device: RefCell<Option<V4l2Device>>,
        stream: RefCell<Option<V4l2Stream>>,
        closed: Cell<bool>,
    }

    fn camera_info_from_openable_device(device: &V4l2OpenableDevice) -> CameraInfo {
        CameraInfo {
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

    fn camera_capture_mode_from_v4l2(mode: &V4l2CaptureMode) -> CaptureMode {
        CaptureMode {
            format: EncodedFormat::Mjpeg,
            width_px: mode.width_px,
            height_px: mode.height_px,
            fps_num: mode.fps_num,
            fps_den: mode.fps_den,
        }
    }

    fn v4l2_capture_mode_from_camera(mode: &CaptureMode) -> V4l2CaptureMode {
        V4l2CaptureMode {
            format: imago::v4l2::types::EncodedFormat::Mjpeg,
            width_px: mode.width_px,
            height_px: mode.height_px,
            fps_num: mode.fps_num,
            fps_den: mode.fps_den,
        }
    }

    fn camera_encoded_frame_from_v4l2(frame: V4l2EncodedFrame) -> EncodedFrame {
        EncodedFrame {
            bytes: frame.bytes,
            width_px: frame.width_px,
            height_px: frame.height_px,
            timestamp_ns: frame.timestamp_ns,
            sequence: frame.sequence,
            format: EncodedFormat::Mjpeg,
        }
    }

    fn map_v4l2_error(error: imago::v4l2::types::V4l2Error) -> CameraError {
        match error {
            imago::v4l2::types::V4l2Error::NotAllowed => CameraError::NotAllowed,
            imago::v4l2::types::V4l2Error::Timeout => CameraError::Timeout,
            imago::v4l2::types::V4l2Error::Disconnected => CameraError::Disconnected,
            imago::v4l2::types::V4l2Error::Busy => CameraError::Busy,
            imago::v4l2::types::V4l2Error::InvalidArgument => CameraError::InvalidArgument,
            imago::v4l2::types::V4l2Error::TransportFault => CameraError::TransportFault,
            imago::v4l2::types::V4l2Error::OperationNotSupported => CameraError::NotSupported,
            imago::v4l2::types::V4l2Error::Other(message) => CameraError::Other(message),
        }
    }

    fn camera_error_other(message: impl Into<String>) -> CameraError {
        CameraError::Other(message.into())
    }

    fn enumerate_cameras() -> Result<Vec<CameraInfo>, CameraError> {
        let openable_devices =
            imago::v4l2::provider::list_openable_devices().map_err(map_v4l2_error)?;
        Ok(openable_devices
            .into_iter()
            .filter(|device| is_camera_device_path(&device.path))
            .map(|device| camera_info_from_openable_device(&device))
            .collect())
    }

    impl CameraSession {
        fn close_resources(&self) {
            if self.closed.replace(true) {
                return;
            }
            self.stream.borrow_mut().take();
            self.device.borrow_mut().take();
        }
    }

    impl Drop for CameraSession {
        fn drop(&mut self) {
            self.close_resources();
        }
    }

    impl exports::imago::camera::provider::Guest for CameraPlugin {
        type Session = CameraSession;

        fn list_cameras() -> Result<Vec<CameraInfo>, CameraError> {
            enumerate_cameras()
        }

        fn list_modes(camera_id: String) -> Result<Vec<CaptureMode>, CameraError> {
            let path = resolve_camera_path(&camera_id)?;
            let device = imago::v4l2::provider::open_device(path).map_err(map_v4l2_error)?;
            let modes = device.list_modes();
            Ok(modes
                .into_iter()
                .map(|mode| camera_capture_mode_from_v4l2(&mode))
                .collect())
        }

        fn open_session(camera_id: String, mode: CaptureMode) -> Result<Session, CameraError> {
            let path = resolve_camera_path(&camera_id)?;
            let device = imago::v4l2::provider::open_device(path).map_err(map_v4l2_error)?;
            let v4l2_mode = v4l2_capture_mode_from_camera(&mode);
            let stream = device.open_stream(v4l2_mode).map_err(map_v4l2_error)?;

            Ok(Session::new(CameraSession {
                mode,
                device: RefCell::new(Some(device)),
                stream: RefCell::new(Some(stream)),
                closed: Cell::new(false),
            }))
        }
    }

    impl exports::imago::camera::provider::GuestSession for CameraSession {
        fn current_mode(&self) -> CaptureMode {
            if self.closed.get() {
                return self.mode.clone();
            }

            let stream = self.stream.borrow();
            stream
                .as_ref()
                .map(|stream| {
                    let mode = stream.current_mode();
                    camera_capture_mode_from_v4l2(&mode)
                })
                .unwrap_or_else(|| self.mode.clone())
        }

        fn next_frame(&self, timeout_ms: u32) -> Result<EncodedFrame, CameraError> {
            if self.closed.get() {
                return Err(camera_error_other("camera session is closed"));
            }

            let stream = self.stream.borrow();
            let stream = stream
                .as_ref()
                .ok_or_else(|| camera_error_other("camera session is closed"))?;
            let frame: V4l2EncodedFrame = stream.next_frame(timeout_ms).map_err(map_v4l2_error)?;
            Ok(camera_encoded_frame_from_v4l2(frame))
        }

        fn close(&self) {
            self.close_resources();
        }
    }

    fn resolve_camera_path(camera_id: &str) -> Result<&str, CameraError> {
        let path = camera_path_from_id(camera_id).ok_or(CameraError::InvalidArgument)?;
        let cameras = enumerate_cameras()?;
        if cameras.iter().any(|camera| camera.id == camera_id) {
            Ok(path)
        } else {
            Err(CameraError::NotFound)
        }
    }

    export!(CameraPlugin);
}

#[cfg(test)]
mod tests {
    use super::camera_id_from_path;
    use super::camera_path_from_id;
    use super::is_camera_device_path;

    #[test]
    fn camera_id_from_path_prefixes_v4l2_scheme() {
        assert_eq!(camera_id_from_path("/dev/video0"), "v4l2:/dev/video0");
    }

    #[test]
    fn camera_path_from_id_requires_v4l2_prefix() {
        assert_eq!(camera_path_from_id("v4l2:/dev/video0"), Some("/dev/video0"));
        assert_eq!(camera_path_from_id("usb:/dev/video0"), None);
        assert_eq!(camera_path_from_id("v4l2:"), None);
    }

    #[test]
    fn is_camera_device_path_accepts_video_nodes_without_usb_metadata() {
        assert!(is_camera_device_path("/dev/video0"));
        assert!(is_camera_device_path("/dev/video1"));
        assert!(!is_camera_device_path("/dev/null"));
    }
}
