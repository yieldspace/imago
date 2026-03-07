use async_trait::async_trait;

use crate::commands::build;

#[async_trait]
pub(crate) trait TargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct QuinnTargetConnector;

#[async_trait]
impl TargetConnector for QuinnTargetConnector {
    async fn connect(
        &self,
        target: &build::DeployTargetConfig,
    ) -> anyhow::Result<super::ConnectedTargetSession> {
        super::connect_target(target).await
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{QuinnTargetConnector, TargetConnector};
    use crate::commands::build::DeployTargetConfig;

    fn missing_client_key_path() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "imago-cli-deploy-network-missing-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        if path.exists() {
            fs::remove_file(&path).expect("existing file should be removed");
        }
        path
    }

    #[tokio::test(flavor = "current_thread")]
    async fn quinn_connector_connect_delegates_error_path_to_connect_target() {
        let target = DeployTargetConfig {
            remote: "127.0.0.1:7443".to_string(),
            server_name: None,
            client_key: Some(missing_client_key_path()),
        };

        let direct = super::super::connect_target(&target)
            .await
            .err()
            .expect("direct connect_target should fail with missing key");
        let delegated = QuinnTargetConnector
            .connect(&target)
            .await
            .err()
            .expect("delegated connect should fail with missing key");

        assert_eq!(delegated.to_string(), direct.to_string());
    }
}
