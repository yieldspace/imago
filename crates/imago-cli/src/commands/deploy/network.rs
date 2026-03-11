use async_trait::async_trait;
use std::{path::PathBuf, time::Duration};

use crate::commands::build;

#[async_trait]
#[doc(hidden)]
pub trait AdminTransport: Send + Sync {
    fn close(&self);

    async fn request_response_bytes(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<u8>>;

    async fn stream_response_frames(
        &self,
        framed: &[u8],
        open_write_timeout: Duration,
        read_idle_timeout: Option<Duration>,
        follow: bool,
        on_frame: &mut (dyn FnMut(Vec<u8>) -> anyhow::Result<bool> + Send),
    ) -> anyhow::Result<super::StreamRequestTermination>;
}

#[async_trait]
#[doc(hidden)]
pub trait TargetConnector: Send + Sync {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession>;
}

#[derive(Debug, Default, Clone, Copy)]
#[doc(hidden)]
pub struct SshTargetConnector;

#[async_trait]
impl TargetConnector for SshTargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession> {
        super::connect_target(target).await
    }
}

#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct LocalProxyTargetConnector {
    imagod_binary: PathBuf,
}

impl LocalProxyTargetConnector {
    pub fn new(imagod_binary: PathBuf) -> Self {
        Self { imagod_binary }
    }
}

#[async_trait]
impl TargetConnector for LocalProxyTargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession> {
        super::connect_local_proxy_target(target, &self.imagod_binary)
    }
}

#[cfg(test)]
mod tests {
    use super::LocalProxyTargetConnector;
    use crate::commands::build;
    use std::path::PathBuf;

    #[test]
    fn ssh_proxy_command_args_match_proxy_stdio_contract() {
        let remote = build::SshTargetRemote {
            user: Some("root".to_string()),
            host: "edge.example.com".to_string(),
            port: Some(2222),
            socket_path: Some("/tmp/imagod.sock".to_string()),
        };

        let args = super::super::ssh_proxy_command_args(&remote);

        assert_eq!(
            args,
            vec![
                "-T",
                "-o",
                "BatchMode=yes",
                "-p",
                "2222",
                "root@edge.example.com",
                "imagod",
                "proxy-stdio",
                "--socket",
                "/tmp/imagod.sock",
            ]
        );
    }

    #[test]
    fn local_proxy_connector_keeps_binary_path() {
        let connector = LocalProxyTargetConnector::new(PathBuf::from("/tmp/imagod"));
        assert_eq!(connector.imagod_binary, PathBuf::from("/tmp/imagod"));
    }
}
