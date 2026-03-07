use nirvash_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, fairness, illegal, invariant, property, subsystem_spec,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
pub struct ArtifactDeployState {
    pub upload: UploadStage,
    pub release: ReleaseStage,
    pub precondition_ok: bool,
    pub auto_rollback: bool,
    pub chunks: ArtifactChunks,
}

impl ArtifactDeployStateSignatureSpec for ArtifactDeployState {
    fn representatives() -> BoundedDomain<Self> {
        let mut states = vec![ArtifactDeploySpec::new().initial_state()];

        for chunks in [1_u8, 2_u8] {
            let chunks = ArtifactChunks::new(chunks).expect("within bounds");
            states.push(Self {
                upload: UploadStage::Partial,
                release: ReleaseStage::None,
                precondition_ok: false,
                auto_rollback: true,
                chunks,
            });
            states.push(Self {
                upload: UploadStage::Complete,
                release: ReleaseStage::None,
                precondition_ok: false,
                auto_rollback: true,
                chunks,
            });
            states.push(Self {
                upload: UploadStage::Committed,
                release: ReleaseStage::None,
                precondition_ok: false,
                auto_rollback: true,
                chunks,
            });

            for release in [
                ReleaseStage::Prepared,
                ReleaseStage::Promoted,
                ReleaseStage::RollbackPending,
                ReleaseStage::RolledBack,
            ] {
                states.push(Self {
                    upload: UploadStage::Committed,
                    release,
                    precondition_ok: true,
                    auto_rollback: true,
                    chunks,
                });
            }
        }

        BoundedDomain::new(states)
    }

    fn signature_invariant(&self) -> bool {
        let promoted_requires_commit = matches!(
            self.release,
            ReleaseStage::Prepared
                | ReleaseStage::Promoted
                | ReleaseStage::RollbackPending
                | ReleaseStage::RolledBack
        )
        .then_some(matches!(self.upload, UploadStage::Committed))
        .unwrap_or(true);
        let precondition_matches_release =
            self.precondition_ok || matches!(self.release, ReleaseStage::None);
        let rollback_requires_flag =
            !matches!(self.release, ReleaseStage::RollbackPending) || self.auto_rollback;

        promoted_requires_commit && precondition_matches_release && rollback_requires_flag
    }
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

#[illegal(ArtifactDeploySpec)]
fn commit_before_complete() -> StepPredicate<ArtifactDeployState, ArtifactDeployAction> {
    StepPredicate::new("commit_before_complete", |prev, action, _| {
        matches!(action, ArtifactDeployAction::CommitUpload)
            && !matches!(prev.upload, UploadStage::Complete)
    })
}

#[illegal(ArtifactDeploySpec)]
fn promote_without_prepare() -> StepPredicate<ArtifactDeployState, ArtifactDeployAction> {
    StepPredicate::new("promote_without_prepare", |prev, action, _| {
        matches!(action, ArtifactDeployAction::PromoteRelease)
            && !matches!(prev.release, ReleaseStage::Prepared)
    })
}

#[illegal(ArtifactDeploySpec)]
fn deploy_on_mismatched_precondition() -> StepPredicate<ArtifactDeployState, ArtifactDeployAction> {
    StepPredicate::new("deploy_on_mismatched_precondition", |_, action, _| {
        matches!(action, ArtifactDeployAction::StartDeployMismatched)
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

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        let mut candidate = *prev;
        match action {
            ArtifactDeployAction::ReceiveChunk
                if !matches!(prev.upload, UploadStage::Committed) && !prev.chunks.is_max() =>
            {
                candidate.upload = UploadStage::Partial;
                candidate.chunks = prev.chunks.saturating_inc();
            }
            ArtifactDeployAction::CompleteUpload
                if matches!(prev.upload, UploadStage::Missing | UploadStage::Partial)
                    && !prev.chunks.is_zero() =>
            {
                candidate.upload = UploadStage::Complete;
            }
            ArtifactDeployAction::CommitUpload if matches!(prev.upload, UploadStage::Complete) => {
                candidate.upload = UploadStage::Committed;
            }
            ArtifactDeployAction::StartDeployMatched
                if matches!(prev.upload, UploadStage::Committed)
                    && matches!(prev.release, ReleaseStage::None | ReleaseStage::RolledBack) =>
            {
                candidate.release = ReleaseStage::Prepared;
                candidate.precondition_ok = true;
            }
            ArtifactDeployAction::PromoteRelease
                if matches!(prev.release, ReleaseStage::Prepared) && prev.precondition_ok =>
            {
                candidate.release = ReleaseStage::Promoted;
            }
            ArtifactDeployAction::TriggerRollback
                if matches!(prev.release, ReleaseStage::Promoted) && prev.auto_rollback =>
            {
                candidate.release = ReleaseStage::RollbackPending;
            }
            ArtifactDeployAction::FinishRollback
                if matches!(prev.release, ReleaseStage::RollbackPending) =>
            {
                candidate.release = ReleaseStage::RolledBack;
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[nirvash_macros::formal_tests(spec = ArtifactDeploySpec, init = initial_state)]
const _: () = ();

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
        assert!(spec.next(&prev, &ArtifactDeployAction::PromoteRelease, &next));
    }
}
