use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_runtime_internal::{
    HttpComponentSupervisor, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpWorkItem,
};
use tokio::sync::{mpsc, oneshot};
use wasmtime::Store;
use wasmtime_wasi_http::p2::{WasiHttpView, bindings::Proxy, bindings::http::types::Scheme};

use crate::{STAGE_RUNTIME, WasiState, map_runtime_error};

type HyperOutgoingBody = wasmtime_wasi_http::p2::body::HyperOutgoingBody;

#[derive(Default)]
pub(crate) struct DefaultHttpComponentSupervisor {
    request_tx: std::sync::RwLock<Option<mpsc::Sender<RuntimeHttpWorkItem>>>,
}

impl DefaultHttpComponentSupervisor {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HttpComponentSupervisor for DefaultHttpComponentSupervisor {
    async fn register_http_component(
        &self,
        request_tx: mpsc::Sender<RuntimeHttpWorkItem>,
        mut http_ready_tx: Option<oneshot::Sender<()>>,
    ) -> Result<(), ImagodError> {
        let mut guard = self.request_tx.write().map_err(|_| {
            map_runtime_error("http component supervisor state is poisoned".to_string())
        })?;
        if guard.is_some() {
            return Err(map_runtime_error(
                "http component is already running in this runtime instance".to_string(),
            ));
        }
        *guard = Some(request_tx);
        if let Some(ready_tx) = http_ready_tx.take() {
            let _ = ready_tx.send(());
        }
        Ok(())
    }

    async fn request_sender(&self) -> Result<mpsc::Sender<RuntimeHttpWorkItem>, ImagodError> {
        let guard = self.request_tx.read().map_err(|_| {
            map_runtime_error("http component supervisor state is poisoned".to_string())
        })?;
        guard.as_ref().cloned().ok_or_else(|| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNTIME,
                "http component is not running",
            )
        })
    }

    async fn unregister_http_component(&self) -> Option<mpsc::Sender<RuntimeHttpWorkItem>> {
        let mut guard = match self.request_tx.write() {
            Ok(guard) => guard,
            Err(_) => return None,
        };
        guard.take()
    }
}

pub(crate) async fn run_http_worker(
    mut store: Store<WasiState>,
    proxy: Proxy,
    mut request_rx: mpsc::Receiver<RuntimeHttpWorkItem>,
) {
    while let Some(work_item) = request_rx.recv().await {
        let result = handle_http_request_in_store(&mut store, &proxy, work_item.request).await;
        let _ = work_item.response_tx.send(result);
    }
}

async fn handle_http_request_in_store(
    store: &mut Store<WasiState>,
    proxy: &Proxy,
    request: RuntimeHttpRequest,
) -> Result<RuntimeHttpResponse, ImagodError> {
    let request = runtime_request_to_hyper_request(request)?;
    let req = store
        .data_mut()
        .http()
        .new_incoming_request(Scheme::Http, request)
        .map_err(|e| map_runtime_error(format!("failed to map incoming HTTP request: {e}")))?;

    let (sender, receiver) = oneshot::channel();
    let out = store
        .data_mut()
        .http()
        .new_response_outparam(sender)
        .map_err(|e| map_runtime_error(format!("failed to allocate response outparam: {e}")))?;

    proxy
        .wasi_http_incoming_handler()
        .call_handle(store, req, out)
        .await
        .map_err(|e| map_runtime_error(format!("incoming-handler trap: {e}")))?;

    let response = receiver.await.map_err(|_| {
        map_runtime_error("incoming-handler did not set response outparam".to_string())
    })?;
    let response = response.map_err(|code| {
        map_runtime_error(format!(
            "incoming-handler returned wasi:http error: {code:?}"
        ))
    })?;

    runtime_response_from_hyper(response).await
}

fn runtime_request_to_hyper_request(
    request: RuntimeHttpRequest,
) -> Result<hyper::Request<BoxBody<Bytes, hyper::Error>>, ImagodError> {
    let method = hyper::Method::from_bytes(request.method.as_bytes()).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNTIME,
            format!("invalid http method '{}': {e}", request.method),
        )
    })?;

    let uri_text = if request.uri.is_empty() {
        "/".to_string()
    } else {
        request.uri
    };
    let uri = uri_text.parse::<hyper::Uri>().map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNTIME,
            format!("invalid http uri '{uri_text}': {e}"),
        )
    })?;

    let mut builder = hyper::Request::builder().method(method).uri(uri);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in request.headers {
            let name = hyper::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_RUNTIME,
                    format!("invalid header name '{name}': {e}"),
                )
            })?;
            let value = hyper::header::HeaderValue::from_bytes(&value).map_err(|e| {
                ImagodError::new(
                    ErrorCode::BadRequest,
                    STAGE_RUNTIME,
                    format!("invalid header value for '{name}': {e}"),
                )
            })?;
            headers.append(name, value);
        }
    }

    let body = BoxBody::new(
        Full::new(Bytes::from(request.body))
            .map_err(|never| match never {})
            .boxed(),
    );
    builder.body(body).map_err(|e| {
        map_runtime_error(format!(
            "failed to build hyper request for incoming-handler: {e}"
        ))
    })
}

async fn runtime_response_from_hyper(
    response: hyper::Response<HyperOutgoingBody>,
) -> Result<RuntimeHttpResponse, ImagodError> {
    let (parts, body) = response.into_parts();
    let body = collect_response_body_optimized(body).await?;
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();

    Ok(RuntimeHttpResponse {
        status: parts.status.as_u16(),
        headers,
        body,
    })
}

#[derive(Default, Debug)]
enum ResponseBodyAccumulator {
    #[default]
    Empty,
    Single(Bytes),
    Multi(BytesMut),
}

impl ResponseBodyAccumulator {
    fn push_data(&mut self, data: Bytes) {
        if data.is_empty() {
            return;
        }
        match self {
            Self::Empty => {
                *self = Self::Single(data);
            }
            Self::Single(single) => {
                let first = std::mem::take(single);
                let mut combined = BytesMut::with_capacity(first.len().saturating_add(data.len()));
                combined.extend_from_slice(first.as_ref());
                combined.extend_from_slice(data.as_ref());
                *self = Self::Multi(combined);
            }
            Self::Multi(combined) => {
                combined.extend_from_slice(data.as_ref());
            }
        }
    }

    fn finish(self) -> Bytes {
        match self {
            Self::Empty => Bytes::new(),
            Self::Single(single) => single,
            Self::Multi(combined) => combined.freeze(),
        }
    }
}

fn consume_response_frame(
    accumulator: &mut ResponseBodyAccumulator,
    frame: hyper::body::Frame<Bytes>,
) {
    if let Ok(data) = frame.into_data() {
        accumulator.push_data(data);
    }
}

async fn collect_response_body_optimized(
    mut body: HyperOutgoingBody,
) -> Result<Bytes, ImagodError> {
    let mut accumulator = ResponseBodyAccumulator::default();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            map_runtime_error(format!("failed to read outgoing response body frame: {e}"))
        })?;
        consume_response_frame(&mut accumulator, frame);
    }
    Ok(accumulator.finish())
}

#[cfg(test)]
async fn collect_response_body_legacy(body: HyperOutgoingBody) -> Result<Bytes, ImagodError> {
    let collected = BodyExt::collect(body)
        .await
        .map_err(|e| map_runtime_error(format!("failed to collect outgoing response body: {e}")))?;
    Ok(Bytes::from(collected.to_bytes().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn single_frame_outgoing_body(body: Bytes) -> HyperOutgoingBody {
        Full::new(body)
            .map_err(|never| match never {})
            .boxed_unsync()
    }

    fn p99_micros(samples: &mut [u128]) -> u128 {
        samples.sort_unstable();
        let index = samples
            .len()
            .saturating_mul(99)
            .div_ceil(100)
            .saturating_sub(1);
        samples[index]
    }

    #[cfg(unix)]
    fn peak_rss_bytes() -> Option<u64> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
        if result != 0 {
            return None;
        }

        let usage = unsafe { usage.assume_init() };
        let max_rss = usage.ru_maxrss;
        if max_rss < 0 {
            return None;
        }

        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            Some(max_rss as u64)
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        )))]
        {
            Some((max_rss as u64).saturating_mul(1024))
        }
    }

    #[cfg(not(unix))]
    fn peak_rss_bytes() -> Option<u64> {
        None
    }

    #[test]
    fn response_body_accumulator_uses_single_state_for_one_chunk() {
        let body = Bytes::from_static(b"single");
        let body_ptr = body.as_ptr();
        let mut accumulator = ResponseBodyAccumulator::default();
        accumulator.push_data(body.clone());
        assert!(matches!(accumulator, ResponseBodyAccumulator::Single(_)));

        let assembled = accumulator.finish();
        assert_eq!(assembled, body);
        assert_eq!(assembled.as_ptr(), body_ptr);
    }

    #[test]
    fn response_body_accumulator_concatenates_multiple_chunks() {
        let mut accumulator = ResponseBodyAccumulator::default();
        accumulator.push_data(Bytes::from_static(b"hello"));
        accumulator.push_data(Bytes::from_static(b"-"));
        accumulator.push_data(Bytes::from_static(b"world"));

        assert!(matches!(accumulator, ResponseBodyAccumulator::Multi(_)));
        assert_eq!(accumulator.finish(), Bytes::from_static(b"hello-world"));
    }

    #[test]
    fn consume_response_frame_ignores_trailers() {
        let mut accumulator = ResponseBodyAccumulator::default();
        consume_response_frame(
            &mut accumulator,
            hyper::body::Frame::data(Bytes::from_static(b"chunk")),
        );
        let mut trailers = hyper::HeaderMap::new();
        trailers.insert("x-test", hyper::header::HeaderValue::from_static("1"));
        consume_response_frame(&mut accumulator, hyper::body::Frame::trailers(trailers));

        assert_eq!(accumulator.finish(), Bytes::from_static(b"chunk"));
    }

    #[tokio::test]
    #[ignore]
    async fn http_response_perf_compare() {
        const BODY_SIZE_BYTES: usize = 32 * 1024 * 1024;
        const ITERATIONS: usize = 64;
        let payload = Bytes::from(vec![b'x'; BODY_SIZE_BYTES]);

        // `ru_maxrss` is process-global peak memory; run legacy first as the baseline.
        let _ = collect_response_body_legacy(single_frame_outgoing_body(payload.clone()))
            .await
            .expect("legacy warmup should succeed");
        let mut legacy_samples = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let started = Instant::now();
            let out = collect_response_body_legacy(single_frame_outgoing_body(payload.clone()))
                .await
                .expect("legacy collection should succeed");
            legacy_samples.push(started.elapsed().as_micros());
            assert_eq!(out.len(), BODY_SIZE_BYTES);
        }
        let legacy_rss_peak = peak_rss_bytes();

        let _ = collect_response_body_optimized(single_frame_outgoing_body(payload.clone()))
            .await
            .expect("optimized warmup should succeed");
        let mut optimized_samples = Vec::with_capacity(ITERATIONS);
        for _ in 0..ITERATIONS {
            let started = Instant::now();
            let out = collect_response_body_optimized(single_frame_outgoing_body(payload.clone()))
                .await
                .expect("optimized collection should succeed");
            optimized_samples.push(started.elapsed().as_micros());
            assert_eq!(out.len(), BODY_SIZE_BYTES);
        }
        let optimized_rss_peak = peak_rss_bytes();

        let optimized_p99 = p99_micros(&mut optimized_samples);
        let legacy_p99 = p99_micros(&mut legacy_samples);

        eprintln!(
            "http_response_perf_compare optimized_p99_us={} legacy_p99_us={} optimized_peak_rss_bytes={:?} legacy_peak_rss_bytes={:?}",
            optimized_p99, legacy_p99, optimized_rss_peak, legacy_rss_peak
        );

        assert!(
            optimized_p99 <= legacy_p99,
            "optimized p99 {}us is slower than legacy {}us",
            optimized_p99,
            legacy_p99
        );

        match (optimized_rss_peak, legacy_rss_peak) {
            (Some(optimized), Some(legacy)) => {
                assert!(
                    optimized <= legacy,
                    "optimized peak RSS {} bytes exceeds legacy {} bytes",
                    optimized,
                    legacy
                );
            }
            _ => {
                eprintln!(
                    "http_response_perf_compare peak RSS measurement unavailable on this platform; reporting N/A"
                );
            }
        }
    }
}
