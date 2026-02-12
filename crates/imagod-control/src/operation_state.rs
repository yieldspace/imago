//! In-memory state machine for command start/cancel/status requests.

use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use imago_protocol::{CommandCancelResponse, CommandState, CommandType, ErrorCode, StateResponse};
use tokio::sync::RwLock;
use uuid::Uuid;

use imagod_common::ImagodError;

#[derive(Debug, Clone)]
struct OperationEntry {
    state: CommandState,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of spawn transition after checking cancel intent.
pub enum SpawnTransition {
    /// Spawn should continue.
    Spawned,
    /// Cancel request won the race before spawn completion.
    Canceled,
}

#[derive(Clone, Default)]
/// Manages short-lived command operation state keyed by request id.
pub struct OperationManager {
    inner: Arc<RwLock<BTreeMap<Uuid, OperationEntry>>>,
}

impl OperationManager {
    /// Creates an empty operation manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new operation in `accepted` state.
    pub async fn start(
        &self,
        request_id: Uuid,
        _command_type: CommandType,
    ) -> Result<(), ImagodError> {
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
                state: CommandState::Accepted,
                stage: "accepted".to_string(),
                updated_at: now_unix_secs(),
                cancel_requested: false,
                phase: OperationPhase::Starting,
            },
        );
        Ok(())
    }

    /// Updates operation state and stage for a running request.
    pub async fn set_state(
        &self,
        request_id: &Uuid,
        state: CommandState,
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

    /// Atomically marks operation as spawned unless cancel was requested first.
    pub async fn mark_spawned_if_not_canceled(
        &self,
        request_id: &Uuid,
        stage: impl Into<String>,
    ) -> Result<SpawnTransition, ImagodError> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or_else(|| {
            ImagodError::new(
                ErrorCode::NotFound,
                "operation.state",
                "request_id is not running",
            )
        })?;

        if entry.cancel_requested {
            entry.stage = "cancel-pending".to_string();
            entry.updated_at = now_unix_secs();
            entry.phase = OperationPhase::Spawned;
            entry.cancel_requested = false;
            return Ok(SpawnTransition::Canceled);
        }

        entry.phase = OperationPhase::Spawned;
        entry.stage = stage.into();
        entry.updated_at = now_unix_secs();
        Ok(SpawnTransition::Spawned)
    }

    /// Returns a snapshot for `state.request` when operation is still active.
    pub async fn snapshot_running(&self, request_id: &Uuid) -> Result<StateResponse, ImagodError> {
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
            request_id: *request_id,
            state: entry.state,
            stage: entry.stage.clone(),
            updated_at: entry.updated_at.clone(),
        })
    }

    /// Handles `command.cancel` semantics for a still-tracked operation.
    pub async fn request_cancel(
        &self,
        request_id: &Uuid,
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
            return Err(ImagodError::new(
                ErrorCode::NotFound,
                "command.cancel",
                "request_id is not running",
            ));
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
            final_state: CommandState::Canceled,
        })
    }

    /// Moves the operation to a terminal state while keeping entry for response flow.
    pub async fn finish(&self, request_id: &Uuid, state: CommandState, stage: impl Into<String>) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(request_id) {
            entry.state = state;
            entry.stage = stage.into();
            entry.updated_at = now_unix_secs();
            entry.phase = OperationPhase::Spawned;
            entry.cancel_requested = false;
        }
    }

    /// Removes operation entry after terminal handling is complete.
    pub async fn remove(&self, request_id: &Uuid) {
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

fn is_terminal(state: CommandState) -> bool {
    matches!(
        state,
        CommandState::Succeeded | CommandState::Failed | CommandState::Canceled
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn req(id: u128) -> Uuid {
        Uuid::from_u128(id)
    }

    #[tokio::test]
    async fn cancel_is_allowed_before_spawned() {
        let manager = OperationManager::new();
        manager
            .start(req(1), CommandType::Deploy)
            .await
            .expect("start should succeed");

        let response = manager
            .request_cancel(&req(1))
            .await
            .expect("cancel should succeed");
        assert!(response.cancellable);
        assert_eq!(response.final_state, CommandState::Canceled);
    }

    #[tokio::test]
    async fn cancel_is_rejected_after_spawned() {
        let manager = OperationManager::new();
        manager
            .start(req(2), CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .set_state(&req(2), CommandState::Running, "running")
            .await
            .expect("state update should succeed");
        manager
            .mark_spawned_if_not_canceled(&req(2), "spawned")
            .await
            .expect("mark spawned should succeed");

        let response = manager
            .request_cancel(&req(2))
            .await
            .expect("cancel should return response");
        assert!(!response.cancellable);
        assert_eq!(response.final_state, CommandState::Running);
    }

    #[tokio::test]
    async fn mark_spawned_returns_canceled_when_cancel_was_requested() {
        let manager = OperationManager::new();
        manager
            .start(req(20), CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .set_state(&req(20), CommandState::Running, "running")
            .await
            .expect("state update should succeed");
        let cancel_response = manager
            .request_cancel(&req(20))
            .await
            .expect("cancel request should succeed");
        assert!(cancel_response.cancellable);

        let transition = manager
            .mark_spawned_if_not_canceled(&req(20), "spawned")
            .await
            .expect("mark spawned should succeed");
        assert_eq!(transition, SpawnTransition::Canceled);

        let state = manager
            .snapshot_running(&req(20))
            .await
            .expect("cancel-pending state should remain observable before terminal event");
        assert_eq!(state.state, CommandState::Running);
        assert_eq!(state.stage, "cancel-pending");
    }

    #[tokio::test]
    async fn removes_operation_after_finish() {
        let manager = OperationManager::new();
        manager
            .start(req(3), CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .finish(&req(3), CommandState::Succeeded, "completed")
            .await;
        manager.remove(&req(3)).await;

        let err = manager
            .request_cancel(&req(3))
            .await
            .expect_err("cancel should fail after removal");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn terminal_state_is_not_cancellable() {
        let manager = OperationManager::new();
        manager
            .start(req(4), CommandType::Deploy)
            .await
            .expect("start should succeed");
        manager
            .finish(&req(4), CommandState::Succeeded, "completed")
            .await;

        let err = manager
            .request_cancel(&req(4))
            .await
            .expect_err("cancel should fail for terminal operation");
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
