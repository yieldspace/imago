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
