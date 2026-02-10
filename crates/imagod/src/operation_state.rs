use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use imago_protocol::{
    CommandCancelResponse, CommandType, ErrorCode, OperationState, StateResponse,
};
use tokio::sync::RwLock;

use crate::error::ImagodError;

#[derive(Debug, Clone)]
struct OperationEntry {
    state: OperationState,
    stage: String,
    updated_at: String,
    cancel_requested: bool,
    phase: OperationPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationPhase {
    Starting,
    Spawned,
}

#[derive(Clone, Default)]
pub struct OperationManager {
    inner: Arc<RwLock<BTreeMap<String, OperationEntry>>>,
}

impl OperationManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start(
        &self,
        request_id: impl Into<String>,
        _command_type: CommandType,
    ) -> Result<(), ImagodError> {
        let request_id = request_id.into();
        let mut inner = self.inner.write().await;

        if inner.contains_key(&request_id) {
            return Err(ImagodError::new(
                ErrorCode::Busy,
                "command.start",
                "request_id is already running",
            ));
        }

        inner.insert(
            request_id,
            OperationEntry {
                state: OperationState::Accepted,
                stage: "accepted".to_string(),
                updated_at: now_unix_secs(),
                cancel_requested: false,
                phase: OperationPhase::Starting,
            },
        );
        Ok(())
    }

    pub async fn set_state(
        &self,
        request_id: &str,
        state: OperationState,
        stage: impl Into<String>,
    ) -> Result<(), ImagodError> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "operation.state",
                "request_id is not running",
            )
        })?;
        entry.state = state;
        entry.stage = stage.into();
        entry.updated_at = now_unix_secs();
        Ok(())
    }

    pub async fn mark_spawned(
        &self,
        request_id: &str,
        stage: impl Into<String>,
    ) -> Result<(), ImagodError> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "operation.state",
                "request_id is not running",
            )
        })?;
        entry.phase = OperationPhase::Spawned;
        entry.stage = stage.into();
        entry.updated_at = now_unix_secs();
        Ok(())
    }

    pub async fn snapshot_running(&self, request_id: &str) -> Result<StateResponse, ImagodError> {
        let inner = self.inner.read().await;
        let entry = inner.get(request_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "state.request",
                "request_id is not running",
            )
        })?;

        if is_terminal(entry.state) {
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                "state.request",
                "request_id is not running",
            ));
        }

        Ok(StateResponse {
            request_id: request_id.to_string(),
            state: entry.state,
            stage: entry.stage.clone(),
            updated_at: entry.updated_at.clone(),
        })
    }

    pub async fn request_cancel(
        &self,
        request_id: &str,
    ) -> Result<CommandCancelResponse, ImagodError> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "command.cancel",
                "request_id is not running",
            )
        })?;

        if is_terminal(entry.state) {
            return Ok(CommandCancelResponse {
                cancellable: false,
                final_state: entry.state,
            });
        }

        if entry.phase == OperationPhase::Spawned {
            return Ok(CommandCancelResponse {
                cancellable: false,
                final_state: entry.state,
            });
        }

        entry.cancel_requested = true;
        entry.updated_at = now_unix_secs();

        Ok(CommandCancelResponse {
            cancellable: true,
            final_state: OperationState::Canceled,
        })
    }

    pub async fn is_cancel_requested(&self, request_id: &str) -> bool {
        let inner = self.inner.read().await;
        inner
            .get(request_id)
            .map(|e| e.cancel_requested)
            .unwrap_or(false)
    }

    pub async fn finish(&self, request_id: &str, state: OperationState, stage: impl Into<String>) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(request_id) {
            entry.state = state;
            entry.stage = stage.into();
            entry.updated_at = now_unix_secs();
            entry.phase = OperationPhase::Spawned;
            entry.cancel_requested = false;
        }
    }

    pub async fn remove(&self, request_id: &str) {
        let mut inner = self.inner.write().await;
        inner.remove(request_id);
    }
}

fn now_unix_secs() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn is_terminal(state: OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded | OperationState::Failed | OperationState::Canceled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_is_allowed_before_spawned() {
        let manager = OperationManager::new();
        manager
            .start("req-1", CommandType::Deploy)
            .await
            .expect("start should succeed");

        let response = manager
            .request_cancel("req-1")
            .await
            .expect("cancel should succeed");
        assert!(response.cancellable);
        assert_eq!(response.final_state, OperationState::Canceled);
    }

    #[tokio::test]
    async fn cancel_is_rejected_after_spawned() {
        let manager = OperationManager::new();
        manager
            .start("req-2", CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .set_state("req-2", OperationState::Running, "running")
            .await
            .expect("state update should succeed");
        manager
            .mark_spawned("req-2", "spawned")
            .await
            .expect("mark spawned should succeed");

        let response = manager
            .request_cancel("req-2")
            .await
            .expect("cancel should return response");
        assert!(!response.cancellable);
        assert_eq!(response.final_state, OperationState::Running);
    }

    #[tokio::test]
    async fn removes_operation_after_finish() {
        let manager = OperationManager::new();
        manager
            .start("req-3", CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .finish("req-3", OperationState::Succeeded, "completed")
            .await;
        manager.remove("req-3").await;

        let err = manager
            .request_cancel("req-3")
            .await
            .expect_err("cancel should fail after removal");
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
