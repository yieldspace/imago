//! In-memory state machine for command start/cancel/status requests.

use std::{collections::BTreeMap, sync::Arc, time::UNIX_EPOCH};

use imagod_spec::{
    CommandErrorKind, CommandLifecycleState, CommandProtocolAction, CommandProtocolContext,
    CommandProtocolObservedState, CommandProtocolOutput, CommandProtocolStageId, OperationPhase,
};
use nirvash::conformance::{ActionApplier, StateObserver};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct OperationEntry {
    state: CommandLifecycleState,
    stage: String,
    updated_at_unix_secs: u64,
    cancel_requested: bool,
    phase: OperationPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OperationReject {
    code: CommandErrorKind,
    stage: CommandProtocolStageId,
}

impl OperationReject {
    const fn new(code: CommandErrorKind, stage: CommandProtocolStageId) -> Self {
        Self { code, stage }
    }

    const fn into_output(self) -> CommandProtocolOutput {
        CommandProtocolOutput::Rejected {
            code: self.code,
            stage: self.stage,
        }
    }
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

    async fn start_entry(&self, request_id: Uuid) -> Result<(), OperationReject> {
        let mut inner = self.inner.write().await;

        if inner.contains_key(&request_id) {
            return Err(OperationReject::new(
                CommandErrorKind::Busy,
                CommandProtocolStageId::CommandStart,
            ));
        }

        inner.insert(
            request_id,
            OperationEntry {
                state: CommandLifecycleState::Accepted,
                stage: "accepted".to_owned(),
                updated_at_unix_secs: now_unix_secs(),
                cancel_requested: false,
                phase: OperationPhase::Starting,
            },
        );
        Ok(())
    }

    async fn set_state_entry(
        &self,
        request_id: &Uuid,
        state: CommandLifecycleState,
        stage: &str,
    ) -> Result<(), OperationReject> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or(OperationReject::new(
            CommandErrorKind::NotFound,
            CommandProtocolStageId::OperationState,
        ))?;
        if entry.state != CommandLifecycleState::Accepted || entry.phase != OperationPhase::Starting
        {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::OperationState,
            ));
        }
        entry.state = state;
        entry.stage = stage.to_owned();
        entry.updated_at_unix_secs = now_unix_secs();
        Ok(())
    }

    async fn request_cancel_entry(
        &self,
        request_id: &Uuid,
    ) -> Result<(bool, CommandLifecycleState), OperationReject> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or(OperationReject::new(
            CommandErrorKind::NotFound,
            CommandProtocolStageId::CommandCancel,
        ))?;

        if is_terminal(entry.state) {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::CommandCancel,
            ));
        }

        if entry.phase == OperationPhase::Spawned {
            return Ok((false, entry.state));
        }

        entry.cancel_requested = true;
        entry.updated_at_unix_secs = now_unix_secs();
        Ok((true, CommandLifecycleState::Canceled))
    }

    async fn snapshot_running_entry(
        &self,
        request_id: &Uuid,
    ) -> Result<(CommandLifecycleState, String, u64), OperationReject> {
        let inner = self.inner.read().await;
        let entry = inner.get(request_id).ok_or(OperationReject::new(
            CommandErrorKind::NotFound,
            CommandProtocolStageId::StateRequest,
        ))?;

        if is_terminal(entry.state) {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::StateRequest,
            ));
        }

        Ok((entry.state, entry.stage.clone(), entry.updated_at_unix_secs))
    }

    async fn mark_spawned_entry(
        &self,
        request_id: &Uuid,
        stage: &str,
    ) -> Result<(bool, bool), OperationReject> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or(OperationReject::new(
            CommandErrorKind::NotFound,
            CommandProtocolStageId::OperationState,
        ))?;
        if entry.state != CommandLifecycleState::Running || entry.phase != OperationPhase::Starting
        {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::OperationState,
            ));
        }

        if entry.cancel_requested {
            entry.stage = "cancel-pending".to_owned();
            entry.updated_at_unix_secs = now_unix_secs();
            entry.phase = OperationPhase::Spawned;
            entry.cancel_requested = false;
            return Ok((false, true));
        }

        entry.phase = OperationPhase::Spawned;
        entry.stage = stage.to_owned();
        entry.updated_at_unix_secs = now_unix_secs();
        Ok((true, false))
    }

    async fn finish_entry(
        &self,
        request_id: &Uuid,
        state: CommandLifecycleState,
        stage: &str,
    ) -> Result<(), OperationReject> {
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(request_id).ok_or(OperationReject::new(
            CommandErrorKind::NotFound,
            CommandProtocolStageId::OperationState,
        ))?;
        if is_terminal(entry.state) || entry.phase != OperationPhase::Spawned {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::OperationState,
            ));
        }
        entry.state = state;
        entry.stage = stage.to_owned();
        entry.updated_at_unix_secs = now_unix_secs();
        entry.phase = OperationPhase::Spawned;
        entry.cancel_requested = false;
        Ok(())
    }

    async fn remove_entry(&self, request_id: &Uuid) -> Result<(), OperationReject> {
        let mut inner = self.inner.write().await;
        let can_remove = inner
            .get(request_id)
            .is_some_and(|entry| is_terminal(entry.state));
        if !can_remove {
            return Err(OperationReject::new(
                CommandErrorKind::NotFound,
                CommandProtocolStageId::OperationRemove,
            ));
        }
        inner.remove(request_id);
        Ok(())
    }
}

impl ActionApplier for OperationManager {
    type Action = CommandProtocolAction;
    type Output = CommandProtocolOutput;
    type Context = CommandProtocolContext;

    async fn execute_action(&self, context: &Self::Context, action: &Self::Action) -> Self::Output {
        match action {
            CommandProtocolAction::Start(_) => self
                .start_entry(context.request_id)
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::SetRunning => self
                .set_state_entry(
                    &context.request_id,
                    CommandLifecycleState::Running,
                    "starting",
                )
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::RequestCancel => self
                .request_cancel_entry(&context.request_id)
                .await
                .map(
                    |(cancellable, final_state)| CommandProtocolOutput::CancelResponse {
                        cancellable,
                        final_state,
                    },
                )
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::SnapshotRunning => self
                .snapshot_running_entry(&context.request_id)
                .await
                .map(
                    |(state, stage, updated_at_unix_secs)| CommandProtocolOutput::StateSnapshot {
                        state,
                        stage,
                        updated_at_unix_secs,
                    },
                )
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::MarkSpawned => self
                .mark_spawned_entry(&context.request_id, "spawned")
                .await
                .map(|(spawned, canceled)| CommandProtocolOutput::SpawnResult { spawned, canceled })
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::FinishSucceeded => self
                .finish_entry(
                    &context.request_id,
                    CommandLifecycleState::Succeeded,
                    "succeeded",
                )
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::FinishFailed(_) => self
                .finish_entry(&context.request_id, CommandLifecycleState::Failed, "failed")
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::FinishCanceled => self
                .finish_entry(
                    &context.request_id,
                    CommandLifecycleState::Canceled,
                    "canceled",
                )
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
            CommandProtocolAction::Remove => self
                .remove_entry(&context.request_id)
                .await
                .map(|_| CommandProtocolOutput::Ack)
                .unwrap_or_else(OperationReject::into_output),
        }
    }
}

impl StateObserver for OperationManager {
    type SummaryState = CommandProtocolObservedState;
    type Context = CommandProtocolContext;

    async fn observe_state(&self, context: &Self::Context) -> Self::SummaryState {
        let inner = self.inner.read().await;
        match inner.get(&context.request_id) {
            Some(entry) => CommandProtocolObservedState {
                tracked: true,
                lifecycle_state: Some(entry.state),
                cancel_requested: entry.cancel_requested,
                phase: Some(entry.phase),
            },
            None => CommandProtocolObservedState::missing(),
        }
    }
}

fn now_unix_secs() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs()
}

fn is_terminal(state: CommandLifecycleState) -> bool {
    state.is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use imagod_spec::CommandKind;

    fn req(id: u128) -> Uuid {
        Uuid::from_u128(id)
    }

    fn context(id: u128) -> CommandProtocolContext {
        CommandProtocolContext {
            request_id: req(id),
        }
    }

    #[tokio::test]
    async fn cancel_is_allowed_before_spawned() {
        let manager = OperationManager::new();
        let context = context(1);
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Deploy))
                .await,
            CommandProtocolOutput::Ack
        );

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::RequestCancel)
                .await,
            CommandProtocolOutput::CancelResponse {
                cancellable: true,
                final_state: CommandLifecycleState::Canceled,
            }
        );
    }

    #[tokio::test]
    async fn cancel_is_rejected_after_spawned() {
        let manager = OperationManager::new();
        let context = context(2);
        manager
            .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Deploy))
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::SetRunning)
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::MarkSpawned)
            .await;

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::RequestCancel)
                .await,
            CommandProtocolOutput::CancelResponse {
                cancellable: false,
                final_state: CommandLifecycleState::Running,
            }
        );
    }

    #[tokio::test]
    async fn mark_spawned_returns_canceled_when_cancel_was_requested() {
        let manager = OperationManager::new();
        let context = context(20);
        manager
            .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Deploy))
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::SetRunning)
            .await;
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::RequestCancel)
                .await,
            CommandProtocolOutput::CancelResponse {
                cancellable: true,
                final_state: CommandLifecycleState::Canceled,
            }
        );

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::MarkSpawned)
                .await,
            CommandProtocolOutput::SpawnResult {
                spawned: false,
                canceled: true,
            }
        );

        match manager
            .execute_action(&context, &CommandProtocolAction::SnapshotRunning)
            .await
        {
            CommandProtocolOutput::StateSnapshot { state, stage, .. } => {
                assert_eq!(state, CommandLifecycleState::Running);
                assert_eq!(stage, "cancel-pending");
            }
            other => panic!("unexpected snapshot output: {other:?}"),
        }
    }

    #[tokio::test]
    async fn removes_operation_after_finish() {
        let manager = OperationManager::new();
        let context = context(3);
        manager
            .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Deploy))
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::SetRunning)
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::MarkSpawned)
            .await;
        manager
            .execute_action(&context, &CommandProtocolAction::FinishSucceeded)
            .await;
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::Remove)
                .await,
            CommandProtocolOutput::Ack
        );

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::RequestCancel)
                .await,
            CommandProtocolOutput::Rejected {
                code: CommandErrorKind::NotFound,
                stage: CommandProtocolStageId::CommandCancel,
            }
        );
    }

    #[tokio::test]
    async fn execute_action_normalizes_runtime_outputs() {
        let manager = OperationManager::new();
        let context = context(42);

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Run))
                .await,
            CommandProtocolOutput::Ack
        );

        match manager
            .execute_action(&context, &CommandProtocolAction::SnapshotRunning)
            .await
        {
            CommandProtocolOutput::StateSnapshot {
                state,
                stage,
                updated_at_unix_secs,
            } => {
                assert_eq!(state, CommandLifecycleState::Accepted);
                assert_eq!(stage, "accepted");
                assert!(updated_at_unix_secs > 0);
            }
            other => panic!("unexpected snapshot output: {other:?}"),
        }

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::RequestCancel)
                .await,
            CommandProtocolOutput::CancelResponse {
                cancellable: true,
                final_state: CommandLifecycleState::Canceled,
            }
        );
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::SetRunning)
                .await,
            CommandProtocolOutput::Ack
        );
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::MarkSpawned)
                .await,
            CommandProtocolOutput::SpawnResult {
                spawned: false,
                canceled: true,
            }
        );
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::FinishCanceled)
                .await,
            CommandProtocolOutput::Ack
        );
        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::Remove)
                .await,
            CommandProtocolOutput::Ack
        );
    }

    #[tokio::test]
    async fn finish_is_rejected_before_spawned() {
        let manager = OperationManager::new();
        let context = context(88);
        manager
            .execute_action(&context, &CommandProtocolAction::Start(CommandKind::Deploy))
            .await;

        assert_eq!(
            manager
                .execute_action(&context, &CommandProtocolAction::FinishSucceeded)
                .await,
            CommandProtocolOutput::Rejected {
                code: CommandErrorKind::NotFound,
                stage: CommandProtocolStageId::OperationState,
            }
        );
    }

    #[tokio::test]
    async fn action_applier_and_state_observer_follow_runtime_contract() {
        let manager = OperationManager::new();
        let context = context(77);

        assert_eq!(
            <OperationManager as ActionApplier>::execute_action(
                &manager,
                &context,
                &CommandProtocolAction::Start(CommandKind::Deploy),
            )
            .await,
            CommandProtocolOutput::Ack
        );
        assert_eq!(
            <OperationManager as StateObserver>::observe_state(&manager, &context).await,
            CommandProtocolObservedState {
                tracked: true,
                lifecycle_state: Some(CommandLifecycleState::Accepted),
                cancel_requested: false,
                phase: Some(OperationPhase::Starting),
            }
        );
    }
}
