//! Bootstrap helpers shared by runner startup flow.

use std::path::{Path, PathBuf};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::RunnerBootstrap;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Stage label used by runner bootstrap decode/validation errors.
pub const STAGE_RUNNER_BOOTSTRAP: &str = "runner.process";
/// Backward-compatible alias for existing call sites.
pub const STAGE_RUNNER: &str = STAGE_RUNNER_BOOTSTRAP;

/// Maximum accepted runner bootstrap payload size in bytes.
pub const MAX_RUNNER_BOOTSTRAP_BYTES: usize = 64 * 1024;

/// Ensures runner endpoint socket path is removed when function scope exits.
#[derive(Debug)]
pub struct SocketCleanupGuard {
    path: PathBuf,
}

impl SocketCleanupGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                eprintln!(
                    "failed to remove runner endpoint {}: {err}",
                    self.path.display()
                );
            }
            Err(_) => {}
        }
    }
}

pub async fn read_runner_bootstrap<R>(reader: R) -> Result<RunnerBootstrap, ImagodError>
where
    R: AsyncRead + Unpin,
{
    let mut limited_reader = reader.take((MAX_RUNNER_BOOTSTRAP_BYTES + 1) as u64);
    let mut bootstrap_bytes = Vec::new();
    limited_reader
        .read_to_end(&mut bootstrap_bytes)
        .await
        .map_err(|e| {
            ImagodError::new(
                ErrorCode::BadRequest,
                STAGE_RUNNER_BOOTSTRAP,
                format!("failed to read runner bootstrap from stdin: {e}"),
            )
        })?;

    decode_runner_bootstrap(&bootstrap_bytes)
}

pub fn decode_runner_bootstrap(bootstrap_bytes: &[u8]) -> Result<RunnerBootstrap, ImagodError> {
    validate_runner_bootstrap_size(bootstrap_bytes.len())?;
    imago_protocol::from_cbor::<RunnerBootstrap>(bootstrap_bytes).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNNER_BOOTSTRAP,
            format!("failed to decode runner bootstrap: {e}"),
        )
    })
}

pub fn validate_runner_bootstrap_size(bootstrap_len: usize) -> Result<(), ImagodError> {
    if bootstrap_len > MAX_RUNNER_BOOTSTRAP_BYTES {
        return Err(ImagodError::new(
            ErrorCode::BadRequest,
            STAGE_RUNNER_BOOTSTRAP,
            format!(
                "runner bootstrap is too large: {bootstrap_len} bytes (max {MAX_RUNNER_BOOTSTRAP_BYTES})"
            ),
        ));
    }

    Ok(())
}

/// Ensures runner socket parent exists and removes stale socket files before bind.
pub fn prepare_socket_path(path: &Path) -> Result<(), ImagodError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER_BOOTSTRAP,
                format!(
                    "failed to create runner socket parent {}: {e}",
                    parent.display()
                ),
            )
        })?;
    }

    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                STAGE_RUNNER_BOOTSTRAP,
                format!("failed to remove existing socket {}: {e}", path.display()),
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imago_protocol::ErrorCode;
    use std::{
        io::Cursor,
        os::unix::net::UnixListener as StdUnixListener,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn run_async_test<F>(future: F)
    where
        F: std::future::Future<Output = ()>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(future);
    }

    fn new_test_socket_path(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let root = PathBuf::from(format!(
            "/tmp/imago-runtime-bootstrap-test-{prefix}-{}-{ts}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("test root should be created");
        root.join("runner.sock")
    }

    #[test]
    fn validate_runner_bootstrap_size_accepts_exact_limit() {
        assert!(validate_runner_bootstrap_size(MAX_RUNNER_BOOTSTRAP_BYTES).is_ok());
    }

    #[test]
    fn validate_runner_bootstrap_size_rejects_over_limit() {
        let err = validate_runner_bootstrap_size(MAX_RUNNER_BOOTSTRAP_BYTES + 1)
            .expect_err("oversized bootstrap should be rejected");
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("too large"));
    }

    #[test]
    fn read_runner_bootstrap_rejects_oversized_input_before_decode() {
        run_async_test(async {
            let oversized = vec![0_u8; MAX_RUNNER_BOOTSTRAP_BYTES + 1];
            let err = read_runner_bootstrap(Cursor::new(oversized))
                .await
                .expect_err("oversized bootstrap should fail before decode");
            assert_eq!(err.code, ErrorCode::BadRequest);
            assert!(err.message.contains("too large"));
        });
    }

    #[test]
    fn socket_cleanup_guard_removes_endpoint_on_drop() {
        let socket_path = new_test_socket_path("cleanup");
        let parent = socket_path
            .parent()
            .expect("socket parent should exist")
            .to_path_buf();
        prepare_socket_path(&socket_path).expect("socket parent preparation should succeed");

        let listener = StdUnixListener::bind(&socket_path).expect("socket bind should succeed");
        assert!(socket_path.exists());
        {
            let _cleanup_guard = SocketCleanupGuard::new(socket_path.clone());
        }
        assert!(
            !socket_path.exists(),
            "socket path should be removed by cleanup guard"
        );

        drop(listener);
        let _ = std::fs::remove_dir_all(parent);
    }
}
