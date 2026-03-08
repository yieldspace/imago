use nirvash_core::{Fairness, Ltl, StatePredicate, StepPredicate, TransitionSystem};
use nirvash_macros::{Signature as FormalSignature, fairness, invariant, property, subsystem_spec};

use crate::bounds::ArtifactChunks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum UploadStage {
    Missing,
    Partial,
    Complete,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum ReleaseStage {
    None,
    Prepared,
    Promoted,
    RollbackPending,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactDeployState {
    pub upload: UploadStage,
    pub release: ReleaseStage,
    pub precondition_ok: bool,
    pub auto_rollback: bool,
    pub chunks: ArtifactChunks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
pub enum ArtifactDeployAction {
    ReceiveChunk,
    CompleteUpload,
    CommitUpload,
    StartDeployMatched,
    StartDeployMismatched,
    PromoteRelease,
    TriggerRollback,
    FinishRollback,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArtifactDeploySpec;

impl ArtifactDeploySpec {
    pub const fn new() -> Self {
        Self
    }

    pub fn initial_state(&self) -> ArtifactDeployState {
        ArtifactDeployState {
            upload: UploadStage::Missing,
            release: ReleaseStage::None,
            precondition_ok: false,
            auto_rollback: true,
            chunks: ArtifactChunks::new(0).expect("within bounds"),
        }
    }
}

fn artifact_deploy_state_valid(state: &ArtifactDeployState) -> bool {
    let promoted_requires_commit = matches!(
        state.release,
        ReleaseStage::Prepared
            | ReleaseStage::Promoted
            | ReleaseStage::RollbackPending
            | ReleaseStage::RolledBack
    )
    .then_some(matches!(state.upload, UploadStage::Committed))
    .unwrap_or(true);
    let precondition_matches_release =
        state.precondition_ok || matches!(state.release, ReleaseStage::None);
    let rollback_requires_flag =
        !matches!(state.release, ReleaseStage::RollbackPending) || state.auto_rollback;

    promoted_requires_commit && precondition_matches_release && rollback_requires_flag
}

#[invariant(ArtifactDeploySpec)]
fn prepared_release_requires_committed_upload() -> StatePredicate<ArtifactDeployState> {
    StatePredicate::new("prepared_release_requires_committed_upload", |state| {
        !matches!(
            state.release,
            ReleaseStage::Prepared
                | ReleaseStage::Promoted
                | ReleaseStage::RollbackPending
                | ReleaseStage::RolledBack
        ) || matches!(state.upload, UploadStage::Committed)
    })
}

#[invariant(ArtifactDeploySpec)]
fn prepared_release_requires_precondition() -> StatePredicate<ArtifactDeployState> {
    StatePredicate::new("prepared_release_requires_precondition", |state| {
        matches!(state.release, ReleaseStage::None) || state.precondition_ok
    })
}

#[invariant(ArtifactDeploySpec)]
fn rollback_requires_auto_rollback_flag() -> StatePredicate<ArtifactDeployState> {
    StatePredicate::new("rollback_requires_auto_rollback_flag", |state| {
        !matches!(state.release, ReleaseStage::RollbackPending) || state.auto_rollback
    })
}

#[property(ArtifactDeploySpec)]
fn partial_upload_leads_to_complete() -> Ltl<ArtifactDeployState, ArtifactDeployAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("partial_upload", |state| {
            matches!(state.upload, UploadStage::Partial)
        })),
        Ltl::pred(StatePredicate::new("upload_complete", |state| {
            matches!(state.upload, UploadStage::Complete | UploadStage::Committed)
        })),
    )
}

#[property(ArtifactDeploySpec)]
fn prepared_release_leads_to_promoted_or_rolled_back()
-> Ltl<ArtifactDeployState, ArtifactDeployAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("prepared_release", |state| {
            matches!(state.release, ReleaseStage::Prepared)
        })),
        Ltl::pred(StatePredicate::new("promoted_or_rolled_back", |state| {
            matches!(
                state.release,
                ReleaseStage::Promoted | ReleaseStage::RolledBack
            )
        })),
    )
}

#[property(ArtifactDeploySpec)]
fn rollback_pending_leads_to_rolled_back() -> Ltl<ArtifactDeployState, ArtifactDeployAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("rollback_pending", |state| {
            matches!(state.release, ReleaseStage::RollbackPending)
        })),
        Ltl::pred(StatePredicate::new("rolled_back", |state| {
            matches!(state.release, ReleaseStage::RolledBack)
        })),
    )
}

#[fairness(ArtifactDeploySpec)]
fn upload_progress_fairness() -> Fairness<ArtifactDeployState, ArtifactDeployAction> {
    Fairness::weak(StepPredicate::new(
        "complete_upload",
        |prev, action, next| {
            matches!(action, ArtifactDeployAction::CompleteUpload)
                && matches!(prev.upload, UploadStage::Missing | UploadStage::Partial)
                && matches!(next.upload, UploadStage::Complete)
        },
    ))
}

#[fairness(ArtifactDeploySpec)]
fn promote_release_fairness() -> Fairness<ArtifactDeployState, ArtifactDeployAction> {
    Fairness::weak(StepPredicate::new(
        "promote_release",
        |prev, action, next| {
            matches!(action, ArtifactDeployAction::PromoteRelease)
                && matches!(prev.release, ReleaseStage::Prepared)
                && matches!(next.release, ReleaseStage::Promoted)
        },
    ))
}

#[fairness(ArtifactDeploySpec)]
fn rollback_finish_fairness() -> Fairness<ArtifactDeployState, ArtifactDeployAction> {
    Fairness::weak(StepPredicate::new(
        "finish_rollback",
        |prev, action, next| {
            matches!(action, ArtifactDeployAction::FinishRollback)
                && matches!(prev.release, ReleaseStage::RollbackPending)
                && matches!(next.release, ReleaseStage::RolledBack)
        },
    ))
}

#[subsystem_spec]
impl TransitionSystem for ArtifactDeploySpec {
    type State = ArtifactDeployState;
    type Action = ArtifactDeployAction;

    fn name(&self) -> &'static str {
        "artifact_deploy"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        action_vocabulary()
    }

    fn transition(&self, prev: &Self::State, action: &Self::Action) -> Option<Self::State> {
        transition_state(prev, action)
    }
}

#[nirvash_macros::formal_tests(spec = ArtifactDeploySpec)]
const _: () = ();

fn action_vocabulary() -> Vec<ArtifactDeployAction> {
    vec![
        ArtifactDeployAction::ReceiveChunk,
        ArtifactDeployAction::CompleteUpload,
        ArtifactDeployAction::CommitUpload,
        ArtifactDeployAction::StartDeployMatched,
        ArtifactDeployAction::StartDeployMismatched,
        ArtifactDeployAction::PromoteRelease,
        ArtifactDeployAction::TriggerRollback,
        ArtifactDeployAction::FinishRollback,
    ]
}

fn transition_state(
    prev: &ArtifactDeployState,
    action: &ArtifactDeployAction,
) -> Option<ArtifactDeployState> {
    let mut candidate = *prev;
    let allowed = match action {
        ArtifactDeployAction::ReceiveChunk
            if !matches!(prev.upload, UploadStage::Committed) && !prev.chunks.is_max() =>
        {
            candidate.upload = UploadStage::Partial;
            candidate.chunks = prev.chunks.saturating_inc();
            true
        }
        ArtifactDeployAction::CompleteUpload
            if matches!(prev.upload, UploadStage::Missing | UploadStage::Partial)
                && !prev.chunks.is_zero() =>
        {
            candidate.upload = UploadStage::Complete;
            true
        }
        ArtifactDeployAction::CommitUpload if matches!(prev.upload, UploadStage::Complete) => {
            candidate.upload = UploadStage::Committed;
            true
        }
        ArtifactDeployAction::StartDeployMatched
            if matches!(prev.upload, UploadStage::Committed)
                && matches!(prev.release, ReleaseStage::None | ReleaseStage::RolledBack) =>
        {
            candidate.release = ReleaseStage::Prepared;
            candidate.precondition_ok = true;
            true
        }
        ArtifactDeployAction::PromoteRelease
            if matches!(prev.release, ReleaseStage::Prepared) && prev.precondition_ok =>
        {
            candidate.release = ReleaseStage::Promoted;
            true
        }
        ArtifactDeployAction::TriggerRollback
            if matches!(prev.release, ReleaseStage::Promoted) && prev.auto_rollback =>
        {
            candidate.release = ReleaseStage::RollbackPending;
            true
        }
        ArtifactDeployAction::FinishRollback
            if matches!(prev.release, ReleaseStage::RollbackPending) =>
        {
            candidate.release = ReleaseStage::RolledBack;
            true
        }
        _ => false,
    };

    allowed
        .then_some(candidate)
        .filter(artifact_deploy_state_valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_release_can_promote() {
        let spec = ArtifactDeploySpec::new();
        let prev = ArtifactDeployState {
            upload: UploadStage::Committed,
            release: ReleaseStage::Prepared,
            precondition_ok: true,
            auto_rollback: true,
            chunks: ArtifactChunks::new(2).expect("within bounds"),
        };
        let next = ArtifactDeployState {
            release: ReleaseStage::Promoted,
            ..prev
        };
        assert!(spec.contains_transition(&prev, &ArtifactDeployAction::PromoteRelease, &next,));
    }
}
