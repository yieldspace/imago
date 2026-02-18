use imago_protocol::{ArtifactStatus, DeployPrepareResponse};

use super::{CleanupPlan, StoreState, UploadSession, commit};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct InMemoryUploadSessionStore;

impl InMemoryUploadSessionStore {
    pub(super) fn build_prepare_response(
        &self,
        session: &UploadSession,
        upload_session_ttl_secs: u64,
        now_epoch_secs: u64,
    ) -> DeployPrepareResponse {
        let artifact_status = if session.committed
            || commit::is_complete(&session.received_ranges, session.artifact_size)
        {
            ArtifactStatus::Complete
        } else if session.received_ranges.is_empty() {
            ArtifactStatus::Missing
        } else {
            ArtifactStatus::Partial
        };

        let missing_ranges = match artifact_status {
            ArtifactStatus::Complete => Vec::new(),
            _ => commit::all_missing_ranges(&session.received_ranges, session.artifact_size),
        };

        DeployPrepareResponse {
            deploy_id: session.deploy_id.clone(),
            artifact_status,
            missing_ranges,
            upload_token: session.upload_token.clone(),
            session_expires_at: now_epoch_secs
                .saturating_add(upload_session_ttl_secs)
                .to_string(),
        }
    }

    pub(super) fn collect_expired_sessions(
        &self,
        state: &mut StoreState,
        now_epoch_secs: u64,
        upload_session_ttl_secs: u64,
    ) -> CleanupPlan {
        let expired_ids = state
            .sessions
            .iter()
            .filter_map(|(deploy_id, session)| {
                if session.committed || session.inflight_writes > 0 || session.commit_in_progress {
                    return None;
                }

                let age = now_epoch_secs.saturating_sub(session.updated_at_epoch_secs);
                if age >= upload_session_ttl_secs {
                    Some(deploy_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        collect_sessions_for_removal_locked(state, expired_ids)
    }

    pub(super) fn collect_old_committed_sessions(
        &self,
        state: &mut StoreState,
        service_name: &str,
        keep_deploy_id: &str,
    ) -> CleanupPlan {
        let old_ids = state
            .sessions
            .iter()
            .filter_map(|(deploy_id, session)| {
                if session.committed
                    && session.service_name == service_name
                    && deploy_id.as_str() != keep_deploy_id
                {
                    Some(deploy_id.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        collect_sessions_for_removal_locked(state, old_ids)
    }

    pub(super) fn cleanup_orphan_idempotency(&self, state: &mut StoreState) {
        let orphan_keys = state
            .idempotency
            .iter()
            .filter_map(|(key, deploy_id)| {
                if state.sessions.contains_key(deploy_id) {
                    None
                } else {
                    Some(key.clone())
                }
            })
            .collect::<Vec<_>>();

        for key in orphan_keys {
            state.idempotency.remove(&key);
        }
    }
}

fn collect_sessions_for_removal_locked(
    state: &mut StoreState,
    deploy_ids: Vec<String>,
) -> CleanupPlan {
    let mut plan = CleanupPlan::default();

    for deploy_id in deploy_ids {
        if let Some(session) = state.sessions.remove(&deploy_id) {
            if state
                .idempotency
                .get(&session.idempotency_key)
                .is_some_and(|mapped| mapped == &deploy_id)
            {
                state.idempotency.remove(&session.idempotency_key);
            }
            plan.files.push(session.file_path);
        }
    }

    plan
}
