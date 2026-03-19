pub mod uvc;

#[cfg(target_arch = "wasm32")]
mod component {
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use self::exports::imago::camera::provider::Session;
    use self::exports::imago::camera::types;
    use super::uvc::{
        CameraSelector, MjpegFrameAssembler, ParseError, ParsedCamera, ParsedMode, ProbeCommitData,
        TransferKind, build_probe_control, get_probe_control, is_jpeg, parse_probe_control,
        parse_uvc_cameras, probe_control_len, set_commit_control, set_probe_control,
    };

    wit_bindgen::generate!({
        path: "wit",
        world: "camera-plugin",
        generate_all
    });

    struct CameraPlugin;

    struct CameraSession {
        device: RefCell<Option<imago::usb::device::Device>>,
        interface: RefCell<Option<imago::usb::usb_interface::ClaimedInterface>>,
        mode: ParsedMode,
        endpoint_address: u8,
        packet_bytes: u32,
        transfer_kind: TransferKind,
        sequence: Cell<u64>,
        assembler: RefCell<MjpegFrameAssembler>,
        closed: Cell<bool>,
    }

    impl exports::imago::camera::provider::Guest for CameraPlugin {
        type Session = CameraSession;

        fn list_cameras() -> Result<Vec<types::CameraInfo>, types::CameraError> {
            let openable_devices =
                imago::usb::provider::list_openable_devices().map_err(map_usb_error)?;
            let mut cameras = Vec::new();
            for openable in openable_devices {
                let device = match imago::usb::provider::open_device(&openable.path) {
                    Ok(device) => device,
                    Err(error) => return Err(map_usb_error(error)),
                };
                let configs = device.configurations().map_err(map_usb_error)?;
                for config in configs {
                    let raw = device
                        .configuration_descriptor_bytes(config.number)
                        .map_err(map_usb_error)?;
                    let parsed = parse_uvc_cameras(config.number, &raw).map_err(map_parse_error)?;
                    cameras.extend(parsed.into_iter().map(|camera| types::CameraInfo {
                        id: camera.camera_id(&openable.path),
                        label: format!(
                            "{:04x}:{:04x} {}",
                            openable.vendor_id, openable.product_id, openable.path
                        ),
                        vendor_id: openable.vendor_id,
                        product_id: openable.product_id,
                        bus: openable.bus,
                        address: openable.address,
                    }));
                }
            }
            Ok(cameras)
        }

        fn list_modes(camera_id: String) -> Result<Vec<types::CaptureMode>, types::CameraError> {
            let selector = CameraSelector::parse(&camera_id).map_err(map_parse_error)?;
            let (_, camera) = load_camera(&selector)?;
            Ok(camera
                .modes
                .iter()
                .map(parsed_mode_to_capture_mode)
                .collect())
        }

        fn open_session(
            camera_id: String,
            mode: types::CaptureMode,
        ) -> Result<Session, types::CameraError> {
            let selector = CameraSelector::parse(&camera_id).map_err(map_parse_error)?;
            let (device, camera) = load_camera(&selector)?;
            let requested_mode = camera
                .find_mode(mode.width_px, mode.height_px, mode.fps_num, mode.fps_den)
                .cloned()
                .ok_or(types::CameraError::NotFound)?;

            let active_configuration = device.active_configuration().map_err(map_usb_error)?;
            if active_configuration != selector.config_number {
                device
                    .select_configuration(selector.config_number)
                    .map_err(map_usb_error)?;
            }

            let interface = device
                .claim_interface(selector.video_streaming_interface)
                .map_err(map_usb_error)?;
            interface.set_alternate_setting(0).map_err(map_usb_error)?;

            let probe_bytes = negotiate_probe(
                &interface,
                selector.video_streaming_interface,
                camera.uvc_version_bcd,
                &requested_mode,
            )?;
            let probe = parse_probe_control(&probe_bytes).map_err(map_parse_error)?;
            let alt_setting = camera
                .select_alt_setting(probe.max_payload_transfer_size)
                .cloned()
                .ok_or(types::CameraError::NotSupported)?;
            interface
                .set_alternate_setting(alt_setting.alt_setting)
                .map_err(map_usb_error)?;

            let packet_bytes = match alt_setting.transfer_kind {
                TransferKind::Bulk => alt_setting.effective_packet_bytes(),
                TransferKind::Isochronous => probe.max_payload_transfer_size,
            }
            .max(1);

            Ok(Session::new(CameraSession {
                device: RefCell::new(Some(device)),
                interface: RefCell::new(Some(interface)),
                mode: requested_mode,
                endpoint_address: alt_setting.endpoint_address,
                packet_bytes,
                transfer_kind: alt_setting.transfer_kind,
                sequence: Cell::new(0),
                assembler: RefCell::new(MjpegFrameAssembler::default()),
                closed: Cell::new(false),
            }))
        }
    }

    impl exports::imago::camera::provider::GuestSession for CameraSession {
        fn current_mode(&self) -> types::CaptureMode {
            parsed_mode_to_capture_mode(&self.mode)
        }

        fn next_frame(&self, timeout_ms: u32) -> Result<types::EncodedFrame, types::CameraError> {
            if self.closed.get() {
                return Err(types::CameraError::Other(
                    "camera session is closed".to_string(),
                ));
            }
            let deadline = SystemTime::now()
                .checked_add(Duration::from_millis(u64::from(timeout_ms)))
                .ok_or_else(|| types::CameraError::InvalidArgument)?;

            loop {
                let now = SystemTime::now();
                if now >= deadline {
                    self.assembler.borrow_mut().reset();
                    return Err(types::CameraError::Timeout);
                }
                let remaining_ms = deadline
                    .duration_since(now)
                    .unwrap_or_else(|_| Duration::from_millis(0))
                    .as_millis()
                    .clamp(1, u128::from(u32::MAX)) as u32;
                let packet = self.read_packet(remaining_ms)?;
                if let Some(frame_bytes) = self
                    .assembler
                    .borrow_mut()
                    .push_packet(&packet)
                    .map_err(map_parse_error)?
                {
                    if !is_jpeg(&frame_bytes) {
                        self.assembler.borrow_mut().reset();
                        return Err(types::CameraError::TransportFault);
                    }
                    let timestamp_ns = unix_timestamp_ns()?;
                    let sequence = self.sequence.get();
                    let frame = types::EncodedFrame {
                        bytes: frame_bytes,
                        width_px: u32::from(self.mode.width_px),
                        height_px: u32::from(self.mode.height_px),
                        timestamp_ns,
                        sequence,
                        format: types::EncodedFormat::Mjpeg,
                    };
                    self.sequence.set(sequence.saturating_add(1));
                    return Ok(frame);
                }
            }
        }

        fn close(&self) {
            self.closed.set(true);
            self.assembler.borrow_mut().reset();
            self.interface.borrow_mut().take();
            self.device.borrow_mut().take();
        }
    }

    impl CameraSession {
        fn read_packet(&self, timeout_ms: u32) -> Result<Vec<u8>, types::CameraError> {
            let interface_guard = self.interface.borrow();
            let interface = interface_guard
                .as_ref()
                .ok_or_else(|| types::CameraError::Other("camera session is closed".to_string()))?;
            match self.transfer_kind {
                TransferKind::Bulk => interface
                    .bulk_in(self.endpoint_address, self.packet_bytes, timeout_ms)
                    .map_err(map_usb_error),
                TransferKind::Isochronous => interface
                    .isochronous_in(self.endpoint_address, self.packet_bytes, 1, timeout_ms)
                    .map_err(map_usb_error),
            }
        }
    }

    fn load_camera(
        selector: &CameraSelector,
    ) -> Result<(imago::usb::device::Device, ParsedCamera), types::CameraError> {
        let device = imago::usb::provider::open_device(&selector.path).map_err(map_usb_error)?;
        let raw = device
            .configuration_descriptor_bytes(selector.config_number)
            .map_err(map_usb_error)?;
        let parsed = parse_uvc_cameras(selector.config_number, &raw).map_err(map_parse_error)?;
        let camera = parsed
            .into_iter()
            .find(|camera| camera.video_streaming_interface == selector.video_streaming_interface)
            .ok_or(types::CameraError::NotFound)?;
        Ok((device, camera))
    }

    fn negotiate_probe(
        interface: &imago::usb::usb_interface::ClaimedInterface,
        interface_number: u8,
        uvc_version_bcd: u16,
        mode: &ParsedMode,
    ) -> Result<Vec<u8>, types::CameraError> {
        let set_probe = build_probe_control(
            uvc_version_bcd,
            mode.format_index,
            mode.frame_index,
            mode.frame_interval_100ns,
        );
        let (request, value, index) = set_probe_control(interface_number);
        interface
            .control_out(
                imago::usb::types::ControlSetup {
                    control_type: imago::usb::types::ControlType::Class,
                    recipient: imago::usb::types::Recipient::InterfaceTarget,
                    request,
                    value,
                    index,
                },
                &set_probe,
                1_000,
            )
            .map_err(map_usb_error)?;

        let (request, value, index) = get_probe_control(interface_number);
        let probe = interface
            .control_in(
                imago::usb::types::ControlSetup {
                    control_type: imago::usb::types::ControlType::Class,
                    recipient: imago::usb::types::Recipient::InterfaceTarget,
                    request,
                    value,
                    index,
                },
                probe_control_len(uvc_version_bcd) as u32,
                1_000,
            )
            .map_err(map_usb_error)?;
        let ProbeCommitData {
            max_video_frame_size,
            ..
        } = parse_probe_control(&probe).map_err(map_parse_error)?;
        if max_video_frame_size == 0 {
            return Err(types::CameraError::TransportFault);
        }
        let (request, value, index) = set_commit_control(interface_number);
        interface
            .control_out(
                imago::usb::types::ControlSetup {
                    control_type: imago::usb::types::ControlType::Class,
                    recipient: imago::usb::types::Recipient::InterfaceTarget,
                    request,
                    value,
                    index,
                },
                &probe,
                1_000,
            )
            .map_err(map_usb_error)?;
        Ok(probe)
    }

    fn parsed_mode_to_capture_mode(mode: &ParsedMode) -> types::CaptureMode {
        let (fps_num, fps_den) = mode.fps_ratio();
        types::CaptureMode {
            format: types::EncodedFormat::Mjpeg,
            width_px: u32::from(mode.width_px),
            height_px: u32::from(mode.height_px),
            fps_num,
            fps_den,
        }
    }

    fn map_parse_error(error: ParseError) -> types::CameraError {
        types::CameraError::Other(error.to_string())
    }

    fn map_usb_error(error: imago::usb::types::UsbError) -> types::CameraError {
        match error {
            imago::usb::types::UsbError::NotAllowed => types::CameraError::NotAllowed,
            imago::usb::types::UsbError::Timeout => types::CameraError::Timeout,
            imago::usb::types::UsbError::Disconnected => types::CameraError::Disconnected,
            imago::usb::types::UsbError::Busy => types::CameraError::Busy,
            imago::usb::types::UsbError::InvalidArgument => types::CameraError::InvalidArgument,
            imago::usb::types::UsbError::TransferFault => types::CameraError::TransportFault,
            imago::usb::types::UsbError::OperationNotSupported => types::CameraError::NotSupported,
            imago::usb::types::UsbError::Other(message) => types::CameraError::Other(message),
        }
    }

    fn unix_timestamp_ns() -> Result<u64, types::CameraError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| types::CameraError::Other(error.to_string()))?;
        Ok(duration.as_nanos().min(u128::from(u64::MAX)) as u64)
    }

    export!(CameraPlugin);
}
