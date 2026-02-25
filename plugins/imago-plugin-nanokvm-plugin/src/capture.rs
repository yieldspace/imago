use std::io::{BufRead, BufReader, Read};

use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::p2::{DynInputStream, pipe::MemoryInputPipe};

use crate::{
    constants::{MAX_JPEG_BYTES, MAX_MJPEG_HEADER_LINE_BYTES, MAX_MJPEG_HEADER_LINES},
    session::{
        NanoKvmSession, create_local_session, create_session, lookup_session, map_http_error,
        parse_session_auth, register_session, remove_session,
    },
    types::{CaptureAuth, CaptureSession, InputStream},
};

pub(crate) fn read_first_jpeg_frame_from_session(
    session: &NanoKvmSession,
) -> Result<Vec<u8>, String> {
    let url = format!("{}/api/stream/mjpeg", session.endpoint);

    let request = crate::session::build_http_agent()
        .get(&url)
        .header("Accept", "multipart/x-mixed-replace");
    let request = if let Some(cookie_header) = &session.cookie_header {
        request.header("Cookie", cookie_header)
    } else {
        request
    };

    let mut response = request
        .call()
        .map_err(|err| map_http_error("nanokvm mjpeg request failed", err))?;
    let status_code = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        if body.trim().is_empty() {
            return Err(format!(
                "nanokvm mjpeg request failed: http status {status_code}"
            ));
        }
        return Err(format!(
            "nanokvm mjpeg request failed: http status {status_code}: {}",
            body.trim()
        ));
    }

    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok());
    let boundary = parse_mjpeg_boundary(content_type)?;
    let reader = response.into_body().into_reader();
    read_first_mjpeg_frame(reader, &boundary)
}

pub(crate) fn parse_mjpeg_boundary(content_type: Option<&str>) -> Result<String, String> {
    let Some(content_type) = content_type else {
        return Ok("frame".to_string());
    };

    let lower = content_type.to_ascii_lowercase();
    if !lower.contains("multipart/x-mixed-replace") {
        return Err(format!(
            "nanokvm stream content-type is not multipart/x-mixed-replace: {content_type}"
        ));
    }

    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some(value) = part.strip_prefix("boundary=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let value = value.strip_prefix("--").unwrap_or(value);
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }

    Ok("frame".to_string())
}

pub(crate) fn read_first_mjpeg_frame<R>(reader: R, boundary: &str) -> Result<Vec<u8>, String>
where
    R: Read,
{
    let mut reader = BufReader::new(reader);
    let boundary_line = format!("--{boundary}");

    fn read_bounded_line<R: BufRead>(
        reader: &mut R,
        line_kind: &str,
    ) -> Result<Option<String>, String> {
        let mut bytes = Vec::new();
        let bytes_read = reader
            .take((MAX_MJPEG_HEADER_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|err| format!("failed to read {line_kind}: {err}"))?;

        if bytes_read == 0 {
            return Ok(None);
        }
        if bytes.len() > MAX_MJPEG_HEADER_LINE_BYTES {
            return Err(format!("{line_kind} exceeds maximum size"));
        }

        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| format!("{line_kind} is not valid utf-8: {err}"))
    }

    loop {
        let line = read_bounded_line(&mut reader, "mjpeg boundary line")?
            .ok_or_else(|| "mjpeg stream ended before first frame".to_string())?;

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == boundary_line || trimmed == format!("{boundary_line}--") {
            break;
        }
    }

    let mut content_length: Option<usize> = None;
    let mut header_terminated = false;
    for _ in 0..MAX_MJPEG_HEADER_LINES {
        let line = read_bounded_line(&mut reader, "mjpeg frame header line")?
            .ok_or_else(|| "mjpeg stream ended while reading frame header".to_string())?;

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            header_terminated = true;
            break;
        }

        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };

        if name.trim().eq_ignore_ascii_case("content-length") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|err| format!("invalid mjpeg content-length: {err}"))?;
            if parsed == 0 {
                return Err("mjpeg content-length must be greater than zero".to_string());
            }
            if parsed > MAX_JPEG_BYTES {
                return Err(format!("mjpeg frame exceeds max bytes: {parsed}"));
            }
            content_length = Some(parsed);
        }
    }
    if !header_terminated {
        return Err(format!(
            "mjpeg frame header terminator not found within {MAX_MJPEG_HEADER_LINES} lines"
        ));
    }

    let content_length =
        content_length.ok_or_else(|| "mjpeg frame header missing content-length".to_string())?;

    let mut frame = vec![0u8; content_length];
    reader
        .read_exact(&mut frame)
        .map_err(|err| format!("failed to read mjpeg frame bytes: {err}"))?;

    Ok(frame)
}

pub(crate) fn push_input_stream_resource(
    table: &mut ResourceTable,
    jpeg_frame: Vec<u8>,
) -> Result<Resource<InputStream>, String> {
    let stream: DynInputStream = Box::new(MemoryInputPipe::new(jpeg_frame));
    let resource = table
        .push(stream)
        .map_err(|err| format!("failed to allocate wasi input-stream resource: {err}"))?;
    Ok(Resource::new_own(resource.rep()))
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::capture::HostSession for ResourceTable {
    fn capture_jpeg(
        &mut self,
        self_: Resource<CaptureSession>,
    ) -> Result<Resource<InputStream>, String> {
        let session = lookup_session(self_.rep())?;
        let frame = read_first_jpeg_frame_from_session(&session)?;
        push_input_stream_resource(self, frame)
    }

    fn disconnect(&mut self, self_: Resource<CaptureSession>) {
        remove_session(self_.rep());
    }

    fn drop(&mut self, resource: Resource<CaptureSession>) -> wasmtime::Result<()> {
        remove_session(resource.rep());
        Ok(())
    }
}

impl crate::imago_nanokvm_plugin_bindings::imago::nanokvm::capture::Host for ResourceTable {
    fn connect(
        &mut self,
        endpoint: String,
        auth: CaptureAuth,
    ) -> Result<Resource<CaptureSession>, String> {
        let endpoint = crate::session::normalize_endpoint(&endpoint)?;
        let auth = parse_session_auth(auth)?;
        let session = create_session(endpoint, auth)?;
        Ok(Resource::new_own(register_session(session)))
    }

    fn local(&mut self, auth: CaptureAuth) -> Result<Resource<CaptureSession>, String> {
        let auth = parse_session_auth(auth)?;
        let session = create_local_session(auth)?;
        Ok(Resource::new_own(register_session(session)))
    }
}
