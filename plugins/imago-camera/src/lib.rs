pub mod uvc;

#[cfg(any(target_arch = "wasm32", test))]
use uvc::{ProbeCommitData, TransferKind};

#[cfg(any(target_arch = "wasm32", test))]
const MAX_NEGOTIATED_FRAME_BYTES: u32 = 8 * 1024 * 1024;
#[cfg(any(target_arch = "wasm32", test))]
const DEFAULT_CONTROL_TIMEOUT_MS: u32 = 1_000;

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameLimitError {
    DescriptorZero,
    NegotiatedZero,
    NegotiatedExceedsLimit,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeCommitError {
    ZeroFrameSize,
    ZeroPayloadSize,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlTimeoutError {
    LimitZero,
}

#[cfg(any(target_arch = "wasm32", test))]
fn packet_bytes_for_transfer(
    transfer_kind: TransferKind,
    negotiated_payload_bytes: u32,
    endpoint_packet_bytes: u32,
) -> u32 {
    match transfer_kind {
        // Bulk reads may contain multiple USB packets that together form one UVC payload.
        TransferKind::Bulk => negotiated_payload_bytes.max(endpoint_packet_bytes),
        TransferKind::Isochronous => negotiated_payload_bytes,
    }
    .max(1)
}

#[cfg(any(target_arch = "wasm32", test))]
fn negotiated_frame_limit_bytes(
    descriptor_max_video_frame_size: u32,
    negotiated_max_video_frame_size: u32,
) -> Result<usize, FrameLimitError> {
    if descriptor_max_video_frame_size == 0 {
        return Err(FrameLimitError::DescriptorZero);
    }
    if negotiated_max_video_frame_size == 0 {
        return Err(FrameLimitError::NegotiatedZero);
    }
    let max_frame_bytes = descriptor_max_video_frame_size.min(MAX_NEGOTIATED_FRAME_BYTES);
    if negotiated_max_video_frame_size > max_frame_bytes {
        return Err(FrameLimitError::NegotiatedExceedsLimit);
    }
    Ok(negotiated_max_video_frame_size as usize)
}

#[cfg(any(target_arch = "wasm32", test))]
fn validate_probe_commit_data(probe: ProbeCommitData) -> Result<ProbeCommitData, ProbeCommitError> {
    if probe.max_video_frame_size == 0 {
        return Err(ProbeCommitError::ZeroFrameSize);
    }
    if probe.max_payload_transfer_size == 0 {
        return Err(ProbeCommitError::ZeroPayloadSize);
    }
    Ok(probe)
}

#[cfg(any(target_arch = "wasm32", test))]
fn control_timeout_ms(max_timeout_ms: u32) -> Result<u32, ControlTimeoutError> {
    if max_timeout_ms == 0 {
        return Err(ControlTimeoutError::LimitZero);
    }
    Ok(max_timeout_ms.min(DEFAULT_CONTROL_TIMEOUT_MS))
}

#[cfg(any(target_arch = "wasm32", test))]
fn remaining_timeout_ms(deadline: std::time::Instant, now: std::time::Instant) -> u32 {
    deadline
        .saturating_duration_since(now)
        .as_millis()
        .clamp(1, u128::from(u32::MAX)) as u32
}

#[cfg(target_arch = "wasm32")]
mod component {
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use self::exports::imago::camera::provider::Session;
    use self::exports::imago::camera::types;
    use super::control_timeout_ms;
    use super::negotiated_frame_limit_bytes;
    use super::packet_bytes_for_transfer;
    use super::remaining_timeout_ms;
    use super::uvc::{
        CameraSelector, MjpegFrameAssembler, ParseError, ParsedCamera, ParsedMode, TransferKind,
        build_probe_control, get_probe_control, is_jpeg, parse_probe_control, parse_uvc_cameras,
        probe_control_len, set_commit_control, set_probe_control,
    };
    use super::validate_probe_commit_data;

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
                let device_cameras = match list_device_cameras(&openable) {
                    Ok(device_cameras) => device_cameras,
                    Err(_) => continue,
                };
                cameras.extend(device_cameras);
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
                control_timeout_ms(imago::usb::provider::get_limits().max_timeout_ms)
                    .map_err(|_| types::CameraError::TransportFault)?,
            )?;
            let probe = parse_probe_control(&probe_bytes).map_err(map_parse_error)?;
            let alt_setting = camera
                .select_alt_setting(probe.max_payload_transfer_size)
                .cloned()
                .ok_or(types::CameraError::NotSupported)?;
            interface
                .set_alternate_setting(alt_setting.alt_setting)
                .map_err(map_usb_error)?;

            let packet_bytes = packet_bytes_for_transfer(
                alt_setting.transfer_kind,
                probe.max_payload_transfer_size,
                alt_setting.effective_packet_bytes(),
            );
            let frame_limit_bytes = negotiated_frame_limit_bytes(
                requested_mode.max_video_frame_size,
                probe.max_video_frame_size,
            )
            .map_err(|_| types::CameraError::TransportFault)?;

            Ok(Session::new(CameraSession {
                device: RefCell::new(Some(device)),
                interface: RefCell::new(Some(interface)),
                mode: requested_mode,
                endpoint_address: alt_setting.endpoint_address,
                packet_bytes,
                transfer_kind: alt_setting.transfer_kind,
                sequence: Cell::new(0),
                assembler: RefCell::new(MjpegFrameAssembler::new(frame_limit_bytes)),
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
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(u64::from(timeout_ms)))
                .ok_or_else(|| types::CameraError::InvalidArgument)?;

            loop {
                let now = Instant::now();
                if now >= deadline {
                    self.assembler.borrow_mut().reset();
                    return Err(types::CameraError::Timeout);
                }
                let remaining_ms = remaining_timeout_ms(deadline, now);
                let packet = match self.read_packet(remaining_ms) {
                    Ok(packet) => packet,
                    Err(error) => {
                        self.assembler.borrow_mut().reset();
                        return Err(error);
                    }
                };
                let maybe_frame = {
                    let mut assembler = self.assembler.borrow_mut();
                    match assembler.push_packet(&packet) {
                        Ok(frame) => frame,
                        Err(_) => {
                            assembler.reset();
                            return Err(types::CameraError::TransportFault);
                        }
                    }
                };
                if let Some(frame_bytes) = maybe_frame {
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

    fn list_device_cameras(
        openable: &imago::usb::types::OpenableDevice,
    ) -> Result<Vec<types::CameraInfo>, types::CameraError> {
        let device = imago::usb::provider::open_device(&openable.path).map_err(map_usb_error)?;
        let configs = device.configurations().map_err(map_usb_error)?;
        let mut cameras = Vec::new();
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
        Ok(cameras)
    }

    fn negotiate_probe(
        interface: &imago::usb::usb_interface::ClaimedInterface,
        interface_number: u8,
        uvc_version_bcd: u16,
        mode: &ParsedMode,
        timeout_ms: u32,
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
                timeout_ms,
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
                timeout_ms,
            )
            .map_err(map_usb_error)?;
        validate_probe_commit_data(parse_probe_control(&probe).map_err(map_parse_error)?)
            .map_err(|_| types::CameraError::TransportFault)?;
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
                timeout_ms,
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

#[cfg(test)]
mod tests {
    use super::uvc::{ProbeCommitData, TransferKind};
    use super::{
        ControlTimeoutError, DEFAULT_CONTROL_TIMEOUT_MS, FrameLimitError,
        MAX_NEGOTIATED_FRAME_BYTES, ProbeCommitError, control_timeout_ms,
        negotiated_frame_limit_bytes, packet_bytes_for_transfer, remaining_timeout_ms,
        validate_probe_commit_data,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn packet_bytes_for_bulk_uses_negotiated_payload_size() {
        assert_eq!(
            packet_bytes_for_transfer(TransferKind::Bulk, 3_072, 1_024),
            3_072
        );
    }

    #[test]
    fn packet_bytes_for_bulk_keeps_endpoint_size_when_probe_is_smaller() {
        assert_eq!(
            packet_bytes_for_transfer(TransferKind::Bulk, 512, 1_024),
            1_024
        );
    }

    #[test]
    fn packet_bytes_for_isochronous_uses_negotiated_payload_size() {
        assert_eq!(
            packet_bytes_for_transfer(TransferKind::Isochronous, 3_072, 1_024),
            3_072
        );
    }

    #[test]
    fn remaining_timeout_ms_uses_monotonic_duration() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(250);
        assert_eq!(remaining_timeout_ms(deadline, now), 250);
    }

    #[test]
    fn remaining_timeout_ms_clamps_elapsed_deadlines_to_one() {
        let now = Instant::now();
        let deadline = now - Duration::from_millis(1);
        assert_eq!(remaining_timeout_ms(deadline, now), 1);
    }

    #[test]
    fn negotiated_frame_limit_bytes_uses_probe_size_within_bounds() {
        assert_eq!(negotiated_frame_limit_bytes(614_400, 512_000), Ok(512_000));
    }

    #[test]
    fn negotiated_frame_limit_bytes_rejects_probe_larger_than_descriptor() {
        assert_eq!(
            negotiated_frame_limit_bytes(614_400, 700_000),
            Err(FrameLimitError::NegotiatedExceedsLimit)
        );
    }

    #[test]
    fn negotiated_frame_limit_bytes_rejects_probe_larger_than_local_cap() {
        assert_eq!(
            negotiated_frame_limit_bytes(
                MAX_NEGOTIATED_FRAME_BYTES.saturating_mul(2),
                MAX_NEGOTIATED_FRAME_BYTES.saturating_add(1)
            ),
            Err(FrameLimitError::NegotiatedExceedsLimit)
        );
    }

    #[test]
    fn negotiated_frame_limit_bytes_rejects_zero_descriptor_size() {
        assert_eq!(
            negotiated_frame_limit_bytes(0, 1),
            Err(FrameLimitError::DescriptorZero)
        );
    }

    #[test]
    fn negotiated_frame_limit_bytes_rejects_zero_probe_size() {
        assert_eq!(
            negotiated_frame_limit_bytes(1, 0),
            Err(FrameLimitError::NegotiatedZero)
        );
    }

    #[test]
    fn validate_probe_commit_data_accepts_non_zero_sizes() {
        assert_eq!(
            validate_probe_commit_data(ProbeCommitData {
                max_video_frame_size: 614_400,
                max_payload_transfer_size: 3_072,
            }),
            Ok(ProbeCommitData {
                max_video_frame_size: 614_400,
                max_payload_transfer_size: 3_072,
            })
        );
    }

    #[test]
    fn validate_probe_commit_data_rejects_zero_frame_size() {
        assert_eq!(
            validate_probe_commit_data(ProbeCommitData {
                max_video_frame_size: 0,
                max_payload_transfer_size: 3_072,
            }),
            Err(ProbeCommitError::ZeroFrameSize)
        );
    }

    #[test]
    fn validate_probe_commit_data_rejects_zero_payload_size() {
        assert_eq!(
            validate_probe_commit_data(ProbeCommitData {
                max_video_frame_size: 614_400,
                max_payload_transfer_size: 0,
            }),
            Err(ProbeCommitError::ZeroPayloadSize)
        );
    }

    #[test]
    fn control_timeout_ms_clamps_to_default_timeout() {
        assert_eq!(control_timeout_ms(5_000), Ok(DEFAULT_CONTROL_TIMEOUT_MS));
    }

    #[test]
    fn control_timeout_ms_uses_stricter_usb_limit() {
        assert_eq!(control_timeout_ms(250), Ok(250));
    }

    #[test]
    fn control_timeout_ms_rejects_zero_limit() {
        assert_eq!(control_timeout_ms(0), Err(ControlTimeoutError::LimitZero));
    }
}
