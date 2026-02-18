use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use imago_protocol::ErrorCode;
use imagod_common::ImagodError;
use imagod_ipc::RunnerBootstrap;
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DefaultRunnerSpawner;

impl DefaultRunnerSpawner {
    pub(super) fn spawn_runner_child(
        &self,
        _bootstrap: &RunnerBootstrap,
    ) -> Result<Child, ImagodError> {
        let exe = std::env::current_exe().map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                super::STAGE_START,
                format!("failed to resolve current executable: {e}"),
            )
        })?;
        let mut cmd = Command::new(exe);
        cmd.arg("--runner");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.spawn().map_err(|e| {
            ImagodError::new(
                ErrorCode::Internal,
                super::STAGE_START,
                format!("failed to spawn runner process: {e}"),
            )
        })
    }
}

pub(super) fn build_runner_endpoint(
    storage_root: &Path,
    service_name: &str,
    runner_id: &str,
) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(service_name.as_bytes());
    hasher.update(b":");
    hasher.update(runner_id.as_bytes());
    let digest = hasher.finalize();
    let endpoint_hash = hex::encode(&digest[..super::RUNNER_ENDPOINT_HASH_BYTES]);

    storage_root
        .join("runtime")
        .join("ipc")
        .join("runners")
        .join(format!("runner-{endpoint_hash}.sock"))
}

pub(super) fn validate_unix_socket_path_len(
    path: &Path,
    socket_name: &str,
) -> Result<(), ImagodError> {
    let path_len = path.to_string_lossy().len();
    if path_len <= super::MAX_UNIX_SOCKET_PATH_BYTES {
        return Ok(());
    }

    Err(ImagodError::new(
        ErrorCode::Internal,
        super::STAGE_CONTROL,
        format!(
            "{socket_name} path is too long for AF_UNIX: actual length {path_len}, max {}, path={}",
            super::MAX_UNIX_SOCKET_PATH_BYTES,
            path.display()
        ),
    ))
}
