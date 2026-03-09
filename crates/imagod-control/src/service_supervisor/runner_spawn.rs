use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use imagod_common::ImagodError;
use imagod_spec::{ErrorCode, RunnerBootstrap};
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DefaultRunnerSpawner;

impl DefaultRunnerSpawner {
    pub(super) fn spawn_runner_child(
        &self,
        bootstrap: &RunnerBootstrap,
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
        for (key, value) in runner_env_overrides(bootstrap.wasm_parallel_compilation) {
            cmd.env(key, value);
        }
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

fn runner_env_overrides(wasm_parallel_compilation: bool) -> Vec<(&'static str, &'static str)> {
    let mut envs = vec![("TOKIO_WORKER_THREADS", "1")];
    if !wasm_parallel_compilation {
        envs.push(("RAYON_NUM_THREADS", "1"));
    }
    envs
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

#[cfg(test)]
mod tests {
    use super::runner_env_overrides;
    use std::collections::BTreeMap;

    fn env_map(wasm_parallel_compilation: bool) -> BTreeMap<&'static str, &'static str> {
        runner_env_overrides(wasm_parallel_compilation)
            .into_iter()
            .collect::<BTreeMap<_, _>>()
    }

    #[test]
    fn runner_env_overrides_force_single_worker_and_single_rayon_when_parallel_disabled() {
        let envs = env_map(false);
        assert_eq!(envs.get("TOKIO_WORKER_THREADS"), Some(&"1"));
        assert_eq!(envs.get("RAYON_NUM_THREADS"), Some(&"1"));
    }

    #[test]
    fn runner_env_overrides_keep_rayon_unset_when_parallel_enabled() {
        let envs = env_map(true);
        assert_eq!(envs.get("TOKIO_WORKER_THREADS"), Some(&"1"));
        assert!(!envs.contains_key("RAYON_NUM_THREADS"));
    }
}
