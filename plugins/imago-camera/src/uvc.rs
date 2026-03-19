use std::{collections::BTreeMap, fmt};

const DESC_TYPE_CONFIGURATION: u8 = 0x02;
const DESC_TYPE_INTERFACE: u8 = 0x04;
const DESC_TYPE_ENDPOINT: u8 = 0x05;
const DESC_TYPE_CS_INTERFACE: u8 = 0x24;

const USB_CLASS_VIDEO: u8 = 0x0e;
const USB_SUBCLASS_VIDEOCONTROL: u8 = 0x01;
const USB_SUBCLASS_VIDEOSTREAMING: u8 = 0x02;

const VS_INPUT_HEADER: u8 = 0x01;
const VS_FORMAT_MJPEG: u8 = 0x06;
const VS_FRAME_MJPEG: u8 = 0x07;
const VC_HEADER: u8 = 0x01;

const UVC_SET_CUR: u8 = 0x01;
const UVC_GET_CUR: u8 = 0x81;
const VS_PROBE_CONTROL: u8 = 0x01;
const VS_COMMIT_CONTROL: u8 = 0x02;

const UVC_HEADER_EOF: u8 = 0x02;
const UVC_HEADER_ERR: u8 = 0x40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Bulk,
    Isochronous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingAltSetting {
    pub alt_setting: u8,
    pub endpoint_address: u8,
    pub transfer_kind: TransferKind,
    pub max_packet_size: u16,
}

impl StreamingAltSetting {
    pub fn effective_packet_bytes(&self) -> u32 {
        match self.transfer_kind {
            TransferKind::Bulk => u32::from(self.max_packet_size),
            TransferKind::Isochronous => {
                let payload = u32::from(self.max_packet_size & 0x07ff);
                let transactions = 1 + u32::from((self.max_packet_size >> 11) & 0x3);
                payload * transactions
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMode {
    pub format_index: u8,
    pub frame_index: u8,
    pub width_px: u16,
    pub height_px: u16,
    pub frame_interval_100ns: u32,
    pub max_video_frame_size: u32,
}

impl ParsedMode {
    pub fn fps_ratio(&self) -> (u32, u32) {
        (10_000_000, self.frame_interval_100ns)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCamera {
    pub config_number: u8,
    pub video_streaming_interface: u8,
    pub uvc_version_bcd: u16,
    pub alt_settings: Vec<StreamingAltSetting>,
    pub modes: Vec<ParsedMode>,
}

impl ParsedCamera {
    pub fn camera_id(&self, path: &str) -> String {
        format!(
            "usb:{path}#cfg={}&vs={}",
            self.config_number, self.video_streaming_interface
        )
    }

    pub fn find_mode(
        &self,
        width_px: u32,
        height_px: u32,
        fps_num: u32,
        fps_den: u32,
    ) -> Option<&ParsedMode> {
        self.modes.iter().find(|mode| {
            let (mode_num, mode_den) = mode.fps_ratio();
            u32::from(mode.width_px) == width_px
                && u32::from(mode.height_px) == height_px
                && mode_num == fps_num
                && mode_den == fps_den
        })
    }

    pub fn select_alt_setting(&self, payload_bytes: u32) -> Option<&StreamingAltSetting> {
        let mut iso = self
            .alt_settings
            .iter()
            .filter(|alt| alt.transfer_kind == TransferKind::Isochronous)
            .collect::<Vec<_>>();
        iso.sort_by_key(|alt| alt.effective_packet_bytes());
        if let Some(best) = iso
            .iter()
            .find(|alt| alt.effective_packet_bytes() >= payload_bytes)
        {
            return Some(*best);
        }
        if let Some(best) = iso.last().copied() {
            return Some(best);
        }

        let mut bulk = self
            .alt_settings
            .iter()
            .filter(|alt| alt.transfer_kind == TransferKind::Bulk)
            .collect::<Vec<_>>();
        bulk.sort_by_key(|alt| alt.effective_packet_bytes());
        bulk.into_iter().next()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraSelector {
    pub path: String,
    pub config_number: u8,
    pub video_streaming_interface: u8,
}

impl CameraSelector {
    pub fn parse(camera_id: &str) -> Result<Self, ParseError> {
        let rest = camera_id
            .strip_prefix("usb:")
            .ok_or_else(|| ParseError::new("camera id must start with 'usb:'"))?;
        let (path, query) = rest
            .split_once("#cfg=")
            .ok_or_else(|| ParseError::new("camera id is missing '#cfg='"))?;
        let (cfg, vs) = query
            .split_once("&vs=")
            .ok_or_else(|| ParseError::new("camera id is missing '&vs='"))?;
        let config_number = cfg
            .parse::<u8>()
            .map_err(|_| ParseError::new("camera id has invalid config number"))?;
        let video_streaming_interface = vs
            .parse::<u8>()
            .map_err(|_| ParseError::new("camera id has invalid VS interface"))?;
        Ok(Self {
            path: path.to_string(),
            config_number,
            video_streaming_interface,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeCommitData {
    pub max_video_frame_size: u32,
    pub max_payload_transfer_size: u32,
}

pub fn parse_uvc_cameras(
    configuration_number: u8,
    bytes: &[u8],
) -> Result<Vec<ParsedCamera>, ParseError> {
    if bytes.len() < 9 {
        return Err(ParseError::new(
            "configuration descriptor is shorter than 9 bytes",
        ));
    }
    if bytes[1] != DESC_TYPE_CONFIGURATION {
        return Err(ParseError::new(
            "buffer does not start with a configuration descriptor",
        ));
    }

    #[derive(Clone, Copy)]
    struct InterfaceContext {
        number: u8,
        alternate_setting: u8,
        class_code: u8,
        subclass_code: u8,
    }

    #[derive(Default)]
    struct StreamBuilder {
        alt_settings: Vec<StreamingAltSetting>,
        modes: Vec<ParsedMode>,
    }

    let mut streams = BTreeMap::<u8, StreamBuilder>::new();
    let mut current_context = None::<InterfaceContext>;
    let mut current_mjpeg_format = BTreeMap::<u8, u8>::new();
    let mut uvc_version_bcd = 0x0110;

    let mut offset = 0usize;
    while offset + 2 <= bytes.len() {
        let len = usize::from(bytes[offset]);
        if len < 2 || offset + len > bytes.len() {
            return Err(ParseError::new("malformed USB descriptor length"));
        }
        let descriptor = &bytes[offset..offset + len];
        match descriptor[1] {
            DESC_TYPE_INTERFACE => {
                if len < 9 {
                    return Err(ParseError::new("short interface descriptor"));
                }
                current_context = Some(InterfaceContext {
                    number: descriptor[2],
                    alternate_setting: descriptor[3],
                    class_code: descriptor[5],
                    subclass_code: descriptor[6],
                });
            }
            DESC_TYPE_ENDPOINT => {
                if len < 7 {
                    return Err(ParseError::new("short endpoint descriptor"));
                }
                if let Some(context) = current_context
                    && context.class_code == USB_CLASS_VIDEO
                    && context.subclass_code == USB_SUBCLASS_VIDEOSTREAMING
                    && descriptor[2] & 0x80 != 0
                {
                    let transfer_kind = match descriptor[3] & 0x03 {
                        0x01 => TransferKind::Isochronous,
                        0x02 => TransferKind::Bulk,
                        _ => {
                            offset += len;
                            continue;
                        }
                    };
                    streams
                        .entry(context.number)
                        .or_default()
                        .alt_settings
                        .push(StreamingAltSetting {
                            alt_setting: context.alternate_setting,
                            endpoint_address: descriptor[2],
                            transfer_kind,
                            max_packet_size: le_u16(&descriptor[4..6]),
                        });
                }
            }
            DESC_TYPE_CS_INTERFACE => {
                if len < 3 {
                    return Err(ParseError::new("short class-specific interface descriptor"));
                }
                let Some(context) = current_context else {
                    offset += len;
                    continue;
                };
                if context.class_code != USB_CLASS_VIDEO {
                    offset += len;
                    continue;
                }
                match context.subclass_code {
                    USB_SUBCLASS_VIDEOCONTROL => {
                        if descriptor[2] == VC_HEADER && len >= 5 {
                            uvc_version_bcd = le_u16(&descriptor[3..5]);
                        }
                    }
                    USB_SUBCLASS_VIDEOSTREAMING => {
                        match descriptor[2] {
                            VS_INPUT_HEADER => {
                                streams.entry(context.number).or_default();
                            }
                            VS_FORMAT_MJPEG => {
                                if len < 4 {
                                    return Err(ParseError::new("short MJPEG format descriptor"));
                                }
                                current_mjpeg_format.insert(context.number, descriptor[3]);
                            }
                            VS_FRAME_MJPEG => {
                                let format_index = *current_mjpeg_format
                                .get(&context.number)
                                .ok_or_else(|| ParseError::new("frame descriptor appeared before MJPEG format descriptor"))?;
                                let frame = parse_mjpeg_frame_descriptor(descriptor, format_index)?;
                                streams
                                    .entry(context.number)
                                    .or_default()
                                    .modes
                                    .extend(frame);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        offset += len;
    }

    Ok(streams
        .into_iter()
        .filter_map(|(video_streaming_interface, mut stream)| {
            if stream.alt_settings.is_empty() || stream.modes.is_empty() {
                return None;
            }
            stream
                .alt_settings
                .sort_by_key(|alt| (alt.alt_setting, alt.endpoint_address));
            stream.modes.sort_by_key(|mode| {
                (
                    mode.width_px,
                    mode.height_px,
                    mode.frame_interval_100ns,
                    mode.format_index,
                    mode.frame_index,
                )
            });
            Some(ParsedCamera {
                config_number: configuration_number,
                video_streaming_interface,
                uvc_version_bcd,
                alt_settings: stream.alt_settings,
                modes: stream.modes,
            })
        })
        .collect())
}

fn parse_mjpeg_frame_descriptor(
    descriptor: &[u8],
    format_index: u8,
) -> Result<Vec<ParsedMode>, ParseError> {
    if descriptor.len() < 26 {
        return Err(ParseError::new("short MJPEG frame descriptor"));
    }
    let frame_index = descriptor[3];
    let width_px = le_u16(&descriptor[5..7]);
    let height_px = le_u16(&descriptor[7..9]);
    let max_video_frame_size = le_u32(&descriptor[17..21]);
    let default_interval = le_u32(&descriptor[21..25]);
    let interval_type = descriptor[25];
    let intervals = if interval_type == 0 {
        if descriptor.len() < 38 {
            return Err(ParseError::new(
                "continuous MJPEG frame descriptor is shorter than 38 bytes",
            ));
        }
        expand_continuous_intervals(
            le_u32(&descriptor[26..30]),
            le_u32(&descriptor[30..34]),
            le_u32(&descriptor[34..38]),
            default_interval,
        )
    } else {
        let needed = 26usize + usize::from(interval_type) * 4;
        if descriptor.len() < needed {
            return Err(ParseError::new(
                "discrete MJPEG frame descriptor is truncated",
            ));
        }
        let mut values = (0..usize::from(interval_type))
            .map(|index| {
                let start = 26 + index * 4;
                le_u32(&descriptor[start..start + 4])
            })
            .collect::<Vec<_>>();
        if !values.contains(&default_interval) {
            values.push(default_interval);
        }
        values.sort_unstable();
        values.dedup();
        values
    };

    Ok(intervals
        .into_iter()
        .map(|frame_interval_100ns| ParsedMode {
            format_index,
            frame_index,
            width_px,
            height_px,
            frame_interval_100ns,
            max_video_frame_size,
        })
        .collect())
}

fn expand_continuous_intervals(
    min_interval: u32,
    max_interval: u32,
    step: u32,
    default_interval: u32,
) -> Vec<u32> {
    if min_interval == 0 || max_interval == 0 || min_interval > max_interval {
        return vec![default_interval];
    }
    if step == 0 {
        return vec![default_interval];
    }
    let count = ((max_interval - min_interval) / step).saturating_add(1);
    if count > 120 {
        return vec![default_interval];
    }
    let mut values = Vec::with_capacity(count as usize + 1);
    let mut current = min_interval;
    while current <= max_interval {
        values.push(current);
        match current.checked_add(step) {
            Some(next) if next > current => current = next,
            _ => break,
        }
    }
    if !values.contains(&default_interval) {
        values.push(default_interval);
        values.sort_unstable();
    }
    values
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub fn probe_control_len(uvc_version_bcd: u16) -> usize {
    if uvc_version_bcd >= 0x0150 { 34 } else { 26 }
}

pub fn build_probe_control(
    uvc_version_bcd: u16,
    format_index: u8,
    frame_index: u8,
    frame_interval_100ns: u32,
) -> Vec<u8> {
    let mut bytes = vec![0u8; probe_control_len(uvc_version_bcd)];
    bytes[0..2].copy_from_slice(&1u16.to_le_bytes());
    bytes[2] = format_index;
    bytes[3] = frame_index;
    bytes[4..8].copy_from_slice(&frame_interval_100ns.to_le_bytes());
    bytes
}

pub fn parse_probe_control(bytes: &[u8]) -> Result<ProbeCommitData, ParseError> {
    if bytes.len() < 26 {
        return Err(ParseError::new(
            "probe/commit control payload is shorter than 26 bytes",
        ));
    }
    Ok(ProbeCommitData {
        max_video_frame_size: le_u32(&bytes[18..22]),
        max_payload_transfer_size: le_u32(&bytes[22..26]),
    })
}

pub fn probe_control_setup(interface_number: u8, request: u8, selector: u8) -> (u8, u16, u16) {
    (
        request,
        u16::from(selector) << 8,
        u16::from(interface_number),
    )
}

pub fn set_probe_control(interface_number: u8) -> (u8, u16, u16) {
    probe_control_setup(interface_number, UVC_SET_CUR, VS_PROBE_CONTROL)
}

pub fn get_probe_control(interface_number: u8) -> (u8, u16, u16) {
    probe_control_setup(interface_number, UVC_GET_CUR, VS_PROBE_CONTROL)
}

pub fn set_commit_control(interface_number: u8) -> (u8, u16, u16) {
    probe_control_setup(interface_number, UVC_SET_CUR, VS_COMMIT_CONTROL)
}

#[derive(Debug, Default)]
pub struct MjpegFrameAssembler {
    current_fid: Option<u8>,
    current_frame: Vec<u8>,
}

impl MjpegFrameAssembler {
    pub fn push_packet(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, ParseError> {
        if packet.len() < 2 {
            return Err(ParseError::new("UVC packet is shorter than 2 bytes"));
        }
        let header_len = usize::from(packet[0]);
        if header_len < 2 || header_len > packet.len() {
            return Err(ParseError::new("UVC packet header length is invalid"));
        }
        let flags = packet[1];
        let fid = flags & 0x01;
        if let Some(current_fid) = self.current_fid
            && current_fid != fid
            && !self.current_frame.is_empty()
        {
            self.current_frame.clear();
        }
        self.current_fid = Some(fid);

        if flags & UVC_HEADER_ERR != 0 {
            self.current_frame.clear();
            return Ok(None);
        }

        let payload = &packet[header_len..];
        if !payload.is_empty() {
            self.current_frame.extend_from_slice(payload);
        }

        if flags & UVC_HEADER_EOF == 0 {
            return Ok(None);
        }
        if self.current_frame.is_empty() {
            return Err(ParseError::new("received EOF packet without JPEG payload"));
        }

        let frame = std::mem::take(&mut self.current_frame);
        self.current_fid = Some(fid ^ 0x01);
        Ok(Some(frame))
    }

    pub fn reset(&mut self) {
        self.current_fid = None;
        self.current_frame.clear();
    }
}

pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uvc_camera_extracts_mjpeg_modes_and_alt_settings() {
        let bytes = sample_uvc_configuration();
        let cameras = parse_uvc_cameras(1, &bytes).expect("configuration should parse");
        assert_eq!(cameras.len(), 1);
        let camera = &cameras[0];
        assert_eq!(camera.uvc_version_bcd, 0x0110);
        assert_eq!(camera.video_streaming_interface, 1);
        assert_eq!(camera.alt_settings.len(), 1);
        assert_eq!(
            camera.alt_settings[0].transfer_kind,
            TransferKind::Isochronous
        );
        assert_eq!(camera.alt_settings[0].effective_packet_bytes(), 3072);
        assert_eq!(camera.modes.len(), 1);
        assert_eq!(camera.modes[0].width_px, 640);
        assert_eq!(camera.modes[0].height_px, 480);
        assert_eq!(camera.modes[0].frame_interval_100ns, 333_333);
        assert!(camera.find_mode(640, 480, 10_000_000, 333_333).is_some());
    }

    #[test]
    fn parse_uvc_cameras_ignores_non_video_configuration() {
        let cameras = parse_uvc_cameras(1, &sample_non_video_configuration())
            .expect("non-video configuration should parse");
        assert!(cameras.is_empty());
    }

    #[test]
    fn parse_uvc_cameras_ignores_stream_without_mjpeg_modes() {
        let cameras = parse_uvc_cameras(1, &sample_video_configuration_without_mjpeg_modes())
            .expect("video configuration without MJPEG modes should parse");
        assert!(cameras.is_empty());
    }

    #[test]
    fn camera_id_round_trip() {
        let selector = CameraSelector::parse("usb:/dev/bus/usb/001/002#cfg=1&vs=3")
            .expect("camera id should parse");
        assert_eq!(selector.path, "/dev/bus/usb/001/002");
        assert_eq!(selector.config_number, 1);
        assert_eq!(selector.video_streaming_interface, 3);
    }

    #[test]
    fn build_and_parse_probe_control_round_trip() {
        let control = build_probe_control(0x0110, 1, 2, 333_333);
        assert_eq!(control.len(), 26);
        let mut response = control.clone();
        response[18..22].copy_from_slice(&614_400u32.to_le_bytes());
        response[22..26].copy_from_slice(&3_072u32.to_le_bytes());
        let parsed = parse_probe_control(&response).expect("probe data should parse");
        assert_eq!(parsed.max_video_frame_size, 614_400);
        assert_eq!(parsed.max_payload_transfer_size, 3_072);
        assert_eq!(set_probe_control(3), (0x01, 0x0100, 3));
        assert_eq!(get_probe_control(3), (0x81, 0x0100, 3));
        assert_eq!(set_commit_control(3), (0x01, 0x0200, 3));
    }

    #[test]
    fn parse_probe_control_rejects_short_payload() {
        let err = parse_probe_control(&[0u8; 25]).expect_err("short payload must fail");
        assert!(
            err.to_string()
                .contains("probe/commit control payload is shorter"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mjpeg_frame_assembler_reassembles_split_frame() {
        let mut assembler = MjpegFrameAssembler::default();
        assert!(
            assembler
                .push_packet(&[2, 0x00, 0xff, 0xd8, 0x11, 0x22])
                .expect("first packet should parse")
                .is_none()
        );
        let frame = assembler
            .push_packet(&[2, 0x02, 0x33, 0x44, 0xff, 0xd9])
            .expect("second packet should parse")
            .expect("frame should complete");
        assert_eq!(frame, vec![0xff, 0xd8, 0x11, 0x22, 0x33, 0x44, 0xff, 0xd9]);
        assert!(is_jpeg(&frame));
    }

    #[test]
    fn mjpeg_frame_assembler_drops_incomplete_frame_on_fid_toggle() {
        let mut assembler = MjpegFrameAssembler::default();
        assert!(
            assembler
                .push_packet(&[2, 0x00, 0xff, 0xd8])
                .expect("first packet should parse")
                .is_none()
        );
        assert!(
            assembler
                .push_packet(&[2, 0x01, 0xaa, 0xbb])
                .expect("fid toggle should parse")
                .is_none()
        );
        let frame = assembler
            .push_packet(&[2, 0x03, 0xff, 0xd8, 0xff, 0xd9])
            .expect("final packet should parse")
            .expect("new frame should complete");
        assert_eq!(frame, vec![0xaa, 0xbb, 0xff, 0xd8, 0xff, 0xd9]);
    }

    #[test]
    fn mjpeg_frame_assembler_rejects_invalid_header_length() {
        let err = MjpegFrameAssembler::default()
            .push_packet(&[5, 0x00, 0xff])
            .expect_err("invalid header length must fail");
        assert!(
            err.to_string().contains("header length is invalid"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mjpeg_frame_assembler_rejects_eof_without_payload() {
        let err = MjpegFrameAssembler::default()
            .push_packet(&[2, 0x02])
            .expect_err("EOF without payload must fail");
        assert!(
            err.to_string().contains("EOF packet without JPEG payload"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mjpeg_frame_assembler_discards_packet_with_error_flag() {
        let mut assembler = MjpegFrameAssembler::default();
        assert!(
            assembler
                .push_packet(&[2, 0x40, 0xff, 0xd8, 0x11, 0x22])
                .expect("error packet should parse")
                .is_none()
        );
        let frame = assembler
            .push_packet(&[2, 0x02, 0xff, 0xd8, 0xff, 0xd9])
            .expect("next packet should parse")
            .expect("next clean frame should complete");
        assert_eq!(frame, vec![0xff, 0xd8, 0xff, 0xd9]);
    }

    fn sample_uvc_configuration() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[9, 0x02, 96, 0x00, 2, 1, 0, 0x80, 50]);
        bytes.extend_from_slice(&[9, 0x04, 0, 0, 0, 0x0e, 0x01, 0x00, 0]);
        bytes.extend_from_slice(&[12, 0x24, 0x01, 0x10, 0x01, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[9, 0x04, 1, 0, 0, 0x0e, 0x02, 0x00, 0]);
        bytes.extend_from_slice(&[13, 0x24, 0x01, 1, 0x81, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[11, 0x24, 0x06, 1, 1, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[
            30, 0x24, 0x07, 1, 0, 0x80, 0x02, 0xe0, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x60, 0x09,
            0x00, 0x15, 0x16, 0x05, 0x00, 1, 0x15, 0x16, 0x05, 0x00,
        ]);
        bytes.extend_from_slice(&[9, 0x04, 1, 1, 1, 0x0e, 0x02, 0x00, 0]);
        bytes.extend_from_slice(&[7, 0x05, 0x81, 0x01, 0x00, 0x14, 1]);
        bytes
    }

    fn sample_non_video_configuration() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[9, 0x02, 18, 0x00, 1, 1, 0, 0x80, 50]);
        bytes.extend_from_slice(&[9, 0x04, 0, 0, 0, 0xff, 0x00, 0x00, 0]);
        bytes
    }

    fn sample_video_configuration_without_mjpeg_modes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[9, 0x02, 59, 0x00, 2, 1, 0, 0x80, 50]);
        bytes.extend_from_slice(&[9, 0x04, 0, 0, 0, 0x0e, 0x01, 0x00, 0]);
        bytes.extend_from_slice(&[12, 0x24, 0x01, 0x10, 0x01, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[9, 0x04, 1, 0, 0, 0x0e, 0x02, 0x00, 0]);
        bytes.extend_from_slice(&[13, 0x24, 0x01, 1, 0x81, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[9, 0x04, 1, 1, 1, 0x0e, 0x02, 0x00, 0]);
        bytes.extend_from_slice(&[7, 0x05, 0x81, 0x01, 0x00, 0x14, 1]);
        bytes
    }
}
