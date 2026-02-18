use bytes::Bytes;
use async_trait::async_trait;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_runtime_internal::{
    HttpComponentSupervisor, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpWorkItem,
};
use tokio::sync::{mpsc, oneshot};
use wasmtime::Store;
use wasmtime_wasi_http::{WasiHttpView, bindings::Proxy, bindings::http::types::Scheme};

use crate::{STAGE_RUNTIME, WasiState, map_runtime_error};

#[derive(Default)]
pub(crate) struct DefaultHttpComponentSupervisor {
    request_tx: tokio::sync::Mutex<Option<mpsc::Sender<RuntimeHttpWorkItem>>>,
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
        let mut guard = self.request_tx.lock().await;
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
        let guard = self.request_tx.lock().await;
        guard.as_ref().cloned().ok_or_else(|| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNTIME,
                "http component is not running",
            )
        })
    }

    async fn unregister_http_component(&self) -> Option<mpsc::Sender<RuntimeHttpWorkItem>> {
        let mut guard = self.request_tx.lock().await;
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
        .new_incoming_request(Scheme::Http, request)
        .map_err(|e| map_runtime_error(format!("failed to map incoming HTTP request: {e}")))?;

    let (sender, receiver) = oneshot::channel();
    let out = store
        .data_mut()
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
    response: hyper::Response<wasmtime_wasi_http::body::HyperOutgoingBody>,
) -> Result<RuntimeHttpResponse, ImagodError> {
    let (parts, body) = response.into_parts();
    let collected = BodyExt::collect(body)
        .await
        .map_err(|e| map_runtime_error(format!("failed to collect outgoing response body: {e}")))?;
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();

    Ok(RuntimeHttpResponse {
        status: parts.status.as_u16(),
        headers,
        body: collected.to_bytes().to_vec(),
    })
}
