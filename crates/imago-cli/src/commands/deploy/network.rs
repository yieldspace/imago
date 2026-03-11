use async_trait::async_trait;
use std::time::Duration;

use crate::commands::build;

#[async_trait]
pub(crate) trait AdminTransport: Send + Sync {
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
pub(crate) trait TargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SshTargetConnector;

#[async_trait]
impl TargetConnector for SshTargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession> {
        super::connect_target(target).await
    }
}

#[cfg(test)]
mod tests {
    use super::{SshTargetConnector, TargetConnector};
    use crate::commands::build::{self, DeployTargetConfig};

    #[tokio::test(flavor = "current_thread")]
    async fn ssh_connector_connect_delegates_error_path_to_connect_target() {
        let target = DeployTargetConfig {
            remote: "ssh://localhost?socket=/run/imago/imagod.sock".to_string(),
            ssh_remote: build::SshTargetRemote {
                user: None,
                host: "localhost".to_string(),
                port: None,
                socket_path: Some("/run/imago/imagod.sock".to_string()),
            },
        };

        let direct = super::super::connect_target(&target)
            .await
            .expect("direct connect_target should establish ssh transport session");
        let delegated = SshTargetConnector
            .connect(&target)
            .await
            .expect("delegated connect should establish ssh transport session");

        assert_eq!(delegated.authority, direct.authority);
        assert_eq!(delegated.resolved_addr, direct.resolved_addr);
        assert_eq!(delegated.remote_input, direct.remote_input);
        direct.close(0, b"test complete");
        delegated.close(0, b"test complete");
    }
}
