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

    pub async fn finish_and_remove(
        &self,
        request_id: &str,
        state: OperationState,
        stage: impl Into<String>,
    ) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(request_id) {
            entry.state = state;
            entry.stage = stage.into();
            entry.updated_at = now_unix_secs();
        }
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
