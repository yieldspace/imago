//! Runner-side HTTP ingress server and request/response translation utilities.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming as HyperIncomingBody;
use hyper::{Request, Response, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::RunnerBootstrap;
pub use imagod_runtime_internal::{ComponentRuntime, RuntimeHttpRequest, RuntimeHttpResponse};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::{JoinError, JoinHandle},
    time::{self, Duration},
};

pub const STAGE_HTTP_INGRESS: &str = "runner.http_ingress";
const INBOUND_ACCEPT_RETRY_BACKOFF_MS: u64 = 25;
const MAX_INBOUND_CONNECTION_HANDLERS: usize = 32;
#[cfg(not(test))]
const HTTP_INGRESS_CONNECTION_TIMEOUT_SECS: u64 = 30;
#[cfg(test)]
const HTTP_INGRESS_CONNECTION_TIMEOUT_SECS: u64 = 1;
pub const DEFAULT_HTTP_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_HTTP_MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

pub async fn spawn_http_ingress_server<R>(
    runtime: Arc<R>,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<JoinHandle<Result<(), ImagodError>>, ImagodError>
where
    R: ComponentRuntime + 'static,
{
    let port = required_http_port(&bootstrap)?;
    let max_http_body_bytes = required_http_max_body_bytes(&bootstrap)?;
    let bind_addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            format!("failed to bind http ingress {bind_addr}: {e}"),
        )
    })?;

    Ok(tokio::spawn(run_http_ingress_server(
        listener,
        runtime,
        max_http_body_bytes,
        bootstrap,
        shutdown_tx,
        shutdown_rx,
    )))
}

pub fn required_http_port(bootstrap: &RunnerBootstrap) -> Result<u16, ImagodError> {
    match bootstrap.http_port {
        Some(port) if port > 0 => Ok(port),
        _ => Err(ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            "type=http requires http_port in runner bootstrap",
        )),
    }
}

pub fn required_http_max_body_bytes(bootstrap: &RunnerBootstrap) -> Result<usize, ImagodError> {
    let max_body_bytes = bootstrap
        .http_max_body_bytes
        .unwrap_or(DEFAULT_HTTP_MAX_BODY_BYTES as u64);
    if max_body_bytes == 0 || max_body_bytes > MAX_HTTP_MAX_BODY_BYTES as u64 {
        return Err(ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            format!(
                "http_max_body_bytes must be in range 1..={} (got {})",
                MAX_HTTP_MAX_BODY_BYTES, max_body_bytes
            ),
        ));
    }
    usize::try_from(max_body_bytes).map_err(|_| {
        ImagodError::new(
            ErrorCode::Internal,
            STAGE_HTTP_INGRESS,
            format!("http_max_body_bytes is too large for this platform: {max_body_bytes}"),
        )
    })
}

pub async fn run_http_ingress_server<R>(
    listener: TcpListener,
    runtime: Arc<R>,
    max_http_body_bytes: usize,
    bootstrap: RunnerBootstrap,
    shutdown_tx: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ImagodError>
where
    R: ComponentRuntime + 'static,
{
    let concurrency = Arc::new(Semaphore::new(MAX_INBOUND_CONNECTION_HANDLERS));
    let mut connection_tasks = tokio::task::JoinSet::new();
    let service_name = bootstrap.service_name.clone();

    'accept: loop {
        reap_completed_connection_tasks(&mut connection_tasks, &service_name);

        let permit = tokio::select! {
            joined = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                if let Some(joined) = joined {
                    report_connection_task_completion(&service_name, joined);
                }
                continue 'accept;
            }
            acquired = concurrency.clone().acquire_owned() => {
                match acquired {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
        };

        let accepted = loop {
            let accepted = tokio::select! {
                joined = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                    if let Some(joined) = joined {
                        report_connection_task_completion(&service_name, joined);
                    }
                    continue;
                }
                accepted = listener.accept() => accepted,
                changed = shutdown_rx.changed() => {
                    drop(permit);
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break 'accept;
                    }
                    continue 'accept;
                }
            };

            match accepted {
                Ok(v) => break v,
                Err(err) => {
                    drop(permit);
                    if should_retry_accept(&err) {
                        eprintln!("runner http ingress accept transient error: {err}");
                        time::sleep(Duration::from_millis(INBOUND_ACCEPT_RETRY_BACKOFF_MS)).await;
                        continue 'accept;
                    }
                    let _ = shutdown_tx.send(true);
                    return Err(ImagodError::new(
                        ErrorCode::Internal,
                        STAGE_HTTP_INGRESS,
                        format!(
                            "http ingress accept failed for service '{}': {err}",
                            bootstrap.service_name
                        ),
                    ));
                }
            }
        };
        let (stream, _) = accepted;

        let runtime = runtime.clone();
        let service_name = bootstrap.service_name.clone();
        let connection_timeout = Duration::from_secs(HTTP_INGRESS_CONNECTION_TIMEOUT_SECS);
        connection_tasks.spawn(async move {
            let _permit = permit;
            match time::timeout(
                connection_timeout,
                serve_http_connection(stream, runtime, max_http_body_bytes),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!(
                        "runner http ingress connection error service={} error={}",
                        service_name, err
                    );
                }
                Err(_) => {
                    eprintln!(
                        "runner http ingress connection timed out service={} timeout_secs={}",
                        service_name, HTTP_INGRESS_CONNECTION_TIMEOUT_SECS
                    );
                }
            }
        });
    }

    connection_tasks.abort_all();
    while connection_tasks.join_next().await.is_some() {}
    Ok(())
}

pub async fn serve_http_connection<R>(
    stream: tokio::net::TcpStream,
    runtime: Arc<R>,
    max_http_body_bytes: usize,
) -> Result<(), ImagodError>
where
    R: ComponentRuntime,
{
    let service = service_fn(move |request: Request<HyperIncomingBody>| {
        let runtime = runtime.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                handle_http_request(runtime, request, max_http_body_bytes).await,
            )
        }
    });

    http1::Builder::new()
        .keep_alive(false)
        .serve_connection(TokioIo::new(stream), service)
        .await
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_HTTP_INGRESS,
                format!("failed to serve http connection: {e}"),
            )
        })
}

pub async fn handle_http_request<R>(
    runtime: Arc<R>,
    request: Request<HyperIncomingBody>,
    max_http_body_bytes: usize,
) -> Response<Full<Bytes>>
where
    R: ComponentRuntime,
{
    let request = match into_runtime_http_request(request, max_http_body_bytes).await {
        Ok(v) => v,
        Err(err) => return runtime_error_response(err),
    };

    match runtime.handle_http_request(request).await {
        Ok(response) => runtime_http_response_to_hyper(response),
        Err(err) => runtime_error_response(err),
    }
}

pub async fn into_runtime_http_request(
    request: Request<HyperIncomingBody>,
    max_http_body_bytes: usize,
) -> Result<RuntimeHttpRequest, ImagodError> {
    let (parts, mut body) = request.into_parts();
    if let Some(content_length) = parts.headers.get(hyper::header::CONTENT_LENGTH) {
        let content_length = content_length.to_str().map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_HTTP_INGRESS,
                format!("invalid content-length header: {e}"),
            )
        })?;
        let content_length = content_length.parse::<u64>().map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_HTTP_INGRESS,
                format!("invalid content-length value '{content_length}': {e}"),
            )
        })?;
        if content_length > max_http_body_bytes as u64 {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_HTTP_INGRESS,
                format!("http request body exceeds limit of {max_http_body_bytes} bytes"),
            ));
        }
    }

    let mut body_bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_HTTP_INGRESS,
                format!("failed to read request body frame: {e}"),
            )
        })?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if body_bytes.len().saturating_add(data.len()) > max_http_body_bytes {
            return Err(ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_HTTP_INGRESS,
                format!("http request body exceeds limit of {max_http_body_bytes} bytes"),
            ));
        }
        body_bytes.extend_from_slice(&data);
    }

    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();

    Ok(RuntimeHttpRequest {
        method: parts.method.as_str().to_string(),
        uri: parts.uri.to_string(),
        headers,
        body: body_bytes,
    })
}

pub fn runtime_http_response_to_hyper(response: RuntimeHttpResponse) -> Response<Full<Bytes>> {
    let status = hyper::StatusCode::from_u16(response.status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);

    let mut out = Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(response.body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"invalid response"))));
    for (name, value) in response.headers {
        let Ok(name) = hyper::header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = hyper::header::HeaderValue::from_bytes(&value) else {
            continue;
        };
        out.headers_mut().append(name, value);
    }
    out
}

pub fn runtime_error_response(error: ImagodError) -> Response<Full<Bytes>> {
    eprintln!(
        "runner http ingress runtime error: code={:?} stage={} message={}",
        error.code, error.stage, error.message
    );
    let (status, body) = match error.code {
        ErrorCode::BadRequest => (hyper::StatusCode::BAD_REQUEST, "bad request"),
        ErrorCode::Busy => (
            hyper::StatusCode::SERVICE_UNAVAILABLE,
            "service unavailable",
        ),
        _ => (
            hyper::StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error",
        ),
    };
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"runtime error"))))
}

fn reap_completed_connection_tasks(
    connection_tasks: &mut tokio::task::JoinSet<()>,
    service_name: &str,
) {
    while let Some(joined) = connection_tasks.try_join_next() {
        report_connection_task_completion(service_name, joined);
    }
}

fn report_connection_task_completion(service_name: &str, joined: Result<(), JoinError>) {
    if let Err(err) = joined {
        eprintln!(
            "runner http ingress connection task join error service={} error={}",
            service_name, err
        );
    }
}

fn should_retry_accept(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use imagod_ipc::{RunnerAppType, random_secret_hex};
    use imagod_runtime_internal::RuntimeRunRequest;
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::Mutex as StdMutex,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn new_test_root(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        PathBuf::from(format!("/tmp/iss-runner-{prefix}-{ts}"))
    }

    fn new_test_bootstrap(root: &Path) -> RunnerBootstrap {
        let runner_id = "runner-1".to_string();
        RunnerBootstrap {
            runner_id: runner_id.clone(),
            service_name: "svc-test".to_string(),
            release_hash: "release-test".to_string(),
            app_type: RunnerAppType::Cli,
            http_port: None,
            http_max_body_bytes: None,
            http_worker_count: 2,
            http_worker_queue_capacity: 4,
            socket: None,
            component_path: root.join("component.wasm"),
            args: Vec::new(),
            envs: BTreeMap::new(),
            bindings: Vec::new(),
            plugin_dependencies: Vec::new(),
            capabilities: imagod_ipc::CapabilityPolicy::default(),
            manager_control_endpoint: root.join("manager-control.sock"),
            runner_endpoint: root.join("runner.sock"),
            manager_auth_secret: random_secret_hex(),
            invocation_secret: random_secret_hex(),
            epoch_tick_interval_ms: 50,
        }
    }

    fn reserve_test_http_port() -> u16 {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral listener should bind");
        let port = listener
            .local_addr()
            .expect("local_addr should be available")
            .port();
        drop(listener);
        port
    }

    #[derive(Default)]
    struct MockHttpRuntime {
        calls: AtomicUsize,
        last_path: StdMutex<Option<String>>,
    }

    #[async_trait]
    impl ComponentRuntime for MockHttpRuntime {
        fn validate_component(&self, _component_path: &Path) -> Result<(), ImagodError> {
            Ok(())
        }

        async fn run_component(&self, _request: RuntimeRunRequest) -> Result<(), ImagodError> {
            Ok(())
        }

        async fn handle_http_request(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, ImagodError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_path.lock().expect("mutex should lock") = Some(request.uri.clone());
            Ok(RuntimeHttpResponse {
                status: 200,
                headers: vec![("content-type".to_string(), b"text/plain".to_vec())],
                body: b"hello-http".to_vec(),
            })
        }
    }

    struct MockErrorRuntime;

    #[async_trait]
    impl ComponentRuntime for MockErrorRuntime {
        fn validate_component(&self, _component_path: &Path) -> Result<(), ImagodError> {
            Ok(())
        }

        async fn run_component(&self, _request: RuntimeRunRequest) -> Result<(), ImagodError> {
            Ok(())
        }

        async fn handle_http_request(
            &self,
            _request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, ImagodError> {
            Err(ImagodError::new(
                ErrorCode::Internal,
                "runtime.secret-stage",
                "very-secret-internal-detail",
            ))
        }
    }

    #[tokio::test]
    async fn http_ingress_requires_http_port_for_http_type() {
        let root = new_test_root("http-port-required");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        bootstrap.http_port = None;

        let runtime = Arc::new(MockHttpRuntime::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let err = spawn_http_ingress_server(runtime, bootstrap, shutdown_tx, shutdown_rx)
            .await
            .expect_err("http ingress should reject missing http_port");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("requires http_port"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn http_ingress_uses_default_max_body_bytes_when_missing() {
        let root = new_test_root("http-max-body-default");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        bootstrap.http_max_body_bytes = None;
        assert_eq!(
            required_http_max_body_bytes(&bootstrap).expect("default max body should be accepted"),
            DEFAULT_HTTP_MAX_BODY_BYTES
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn http_ingress_rejects_invalid_max_body_bytes() {
        let root = new_test_root("http-max-body-invalid");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        for invalid in [0, (MAX_HTTP_MAX_BODY_BYTES as u64) + 1] {
            bootstrap.http_max_body_bytes = Some(invalid);
            let err = required_http_max_body_bytes(&bootstrap)
                .expect_err("invalid http_max_body_bytes should fail");
            assert_eq!(err.code, ErrorCode::Internal);
            assert!(err.message.contains("http_max_body_bytes"));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn http_ingress_handles_request_and_shutdown() {
        let root = new_test_root("http-ingress-request");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        let port = reserve_test_http_port();
        bootstrap.http_port = Some(port);

        let runtime = Arc::new(MockHttpRuntime::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ingress_task = spawn_http_ingress_server(
            runtime.clone(),
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        )
        .await
        .expect("http ingress should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client should connect");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .expect("request write should succeed");

        let mut response_bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            stream.read_to_end(&mut response_bytes),
        )
        .await
        .expect("response read should complete")
        .expect("response read should succeed");
        let response_text = String::from_utf8_lossy(&response_bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 200"),
            "unexpected status line: {response_text}"
        );
        assert!(response_text.contains("hello-http"));
        assert!(
            response_text
                .to_ascii_lowercase()
                .contains("connection: close"),
            "keep-alive should be disabled: {response_text}"
        );

        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime
                .last_path
                .lock()
                .expect("mutex should lock")
                .as_deref(),
            Some("/health")
        );

        let _ = shutdown_tx.send(true);
        let joined = tokio::time::timeout(Duration::from_secs(2), ingress_task)
            .await
            .expect("ingress task should stop after shutdown")
            .expect("ingress task join should succeed");
        assert!(joined.is_ok(), "ingress should stop cleanly");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn http_ingress_rejects_request_body_over_limit() {
        let root = new_test_root("http-ingress-body-limit");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        let port = reserve_test_http_port();
        bootstrap.http_port = Some(port);
        bootstrap.http_max_body_bytes = Some(16);

        let runtime = Arc::new(MockHttpRuntime::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ingress_task = spawn_http_ingress_server(
            runtime.clone(),
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        )
        .await
        .expect("http ingress should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client should connect");
        stream
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 17\r\nConnection: close\r\n\r\n0123456789ABCDEFG",
            )
            .await
            .expect("request write should succeed");

        let mut response_bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            stream.read_to_end(&mut response_bytes),
        )
        .await
        .expect("response read should complete")
        .expect("response read should succeed");
        let response_text = String::from_utf8_lossy(&response_bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 400"),
            "expected bad request status: {response_text}"
        );
        assert_eq!(
            runtime.calls.load(Ordering::SeqCst),
            0,
            "runtime should not be called for oversized request"
        );

        let _ = shutdown_tx.send(true);
        let joined = tokio::time::timeout(Duration::from_secs(2), ingress_task)
            .await
            .expect("ingress task should stop after shutdown")
            .expect("ingress task join should succeed");
        assert!(joined.is_ok(), "ingress should stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn http_ingress_internal_errors_do_not_leak_details_to_client() {
        let root = new_test_root("http-ingress-error-sanitize");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        let port = reserve_test_http_port();
        bootstrap.http_port = Some(port);

        let runtime = Arc::new(MockErrorRuntime);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ingress_task =
            spawn_http_ingress_server(runtime, bootstrap.clone(), shutdown_tx.clone(), shutdown_rx)
                .await
                .expect("http ingress should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client should connect");
        stream
            .write_all(b"GET /err HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("request write should succeed");

        let mut response_bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(2),
            stream.read_to_end(&mut response_bytes),
        )
        .await
        .expect("response read should complete")
        .expect("response read should succeed");
        let response_text = String::from_utf8_lossy(&response_bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 500"),
            "expected internal status: {response_text}"
        );
        assert!(
            response_text.contains("internal server error"),
            "expected sanitized body: {response_text}"
        );
        assert!(
            !response_text.contains("runtime.secret-stage"),
            "internal stage must not leak: {response_text}"
        );
        assert!(
            !response_text.contains("very-secret-internal-detail"),
            "internal detail must not leak: {response_text}"
        );

        let _ = shutdown_tx.send(true);
        let joined = tokio::time::timeout(Duration::from_secs(2), ingress_task)
            .await
            .expect("ingress task should stop after shutdown")
            .expect("ingress task join should succeed");
        assert!(joined.is_ok(), "ingress should stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn http_ingress_releases_permit_after_idle_connection_timeout() {
        let root = new_test_root("http-ingress-idle-timeout");
        let mut bootstrap = new_test_bootstrap(&root);
        bootstrap.app_type = RunnerAppType::Http;
        let port = reserve_test_http_port();
        bootstrap.http_port = Some(port);

        let runtime = Arc::new(MockHttpRuntime::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ingress_task = spawn_http_ingress_server(
            runtime.clone(),
            bootstrap.clone(),
            shutdown_tx.clone(),
            shutdown_rx,
        )
        .await
        .expect("http ingress should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut idle_streams = Vec::new();
        for _ in 0..MAX_INBOUND_CONNECTION_HANDLERS {
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("idle client should connect");
            idle_streams.push(stream);
        }

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("active client should connect");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("request write should succeed");

        let mut response_bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(4),
            stream.read_to_end(&mut response_bytes),
        )
        .await
        .expect("response read should complete after idle timeout")
        .expect("response read should succeed");
        let response_text = String::from_utf8_lossy(&response_bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 200"),
            "expected successful status: {response_text}"
        );
        assert!(response_text.contains("hello-http"));
        assert_eq!(
            runtime.calls.load(Ordering::SeqCst),
            1,
            "runtime should eventually serve request after idle timeout"
        );

        drop(idle_streams);
        let _ = shutdown_tx.send(true);
        let joined = tokio::time::timeout(Duration::from_secs(2), ingress_task)
            .await
            .expect("ingress task should stop after shutdown")
            .expect("ingress task join should succeed");
        assert!(joined.is_ok(), "ingress should stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn runtime_error_response_maps_busy_to_service_unavailable() {
        let response = runtime_error_response(ImagodError::new(
            ErrorCode::Busy,
            "runtime.http",
            "queue full",
        ));
        assert_eq!(response.status(), hyper::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn ingress_reaps_completed_connection_tasks_while_running() {
        let mut connection_tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            connection_tasks.spawn(async {});
        }
        tokio::task::yield_now().await;

        reap_completed_connection_tasks(&mut connection_tasks, "svc-test");
        assert_eq!(
            connection_tasks.len(),
            0,
            "completed tasks should be collected from join set"
        );
    }
}
