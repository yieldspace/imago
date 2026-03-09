use imagod_spec::{
    CommandProtocolObservedState as RuntimeCommandProtocolObservedState,
    CommandProtocolOutput as RuntimeCommandProtocolOutput,
};
use nirvash_core::{
    DocGraphPolicy, ModelCase, StatePredicate, TransitionSystem,
    conformance::ProtocolConformanceSpec,
};
use nirvash_macros::{invariant, subsystem_spec};

use crate::{
    CommandErrorKind, CommandLifecycleState, CommandProtocolAction, CommandProtocolStageId,
    OperationPhase,
};

#[cfg(test)]
use crate::{
    CommandKind,
    bounds::{SPEC_COMMAND_STATES, SPEC_ERROR_CODES},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStateClass {
    InFlight,
    Terminal,
}

pub fn classify_command_state(state: CommandLifecycleState) -> CommandStateClass {
    match state {
        CommandLifecycleState::Accepted | CommandLifecycleState::Running => {
            CommandStateClass::InFlight
        }
        CommandLifecycleState::Succeeded
        | CommandLifecycleState::Failed
        | CommandLifecycleState::Canceled => CommandStateClass::Terminal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCodeClass {
    Authentication,
    Validation,
    Operational,
    Storage,
    Internal,
}

pub fn classify_error_code(code: CommandErrorKind) -> ErrorCodeClass {
    match code {
        CommandErrorKind::Unauthorized => ErrorCodeClass::Authentication,
        CommandErrorKind::BadRequest
        | CommandErrorKind::BadManifest
        | CommandErrorKind::RangeInvalid => ErrorCodeClass::Validation,
        CommandErrorKind::Busy
        | CommandErrorKind::NotFound
        | CommandErrorKind::IdempotencyConflict
        | CommandErrorKind::PreconditionFailed
        | CommandErrorKind::OperationTimeout
        | CommandErrorKind::RollbackFailed => ErrorCodeClass::Operational,
        CommandErrorKind::ChunkHashMismatch
        | CommandErrorKind::ArtifactIncomplete
        | CommandErrorKind::StorageQuota => ErrorCodeClass::Storage,
        CommandErrorKind::Internal => ErrorCodeClass::Internal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandProtocolState {
    pub tracked: bool,
    pub lifecycle_state: Option<CommandLifecycleState>,
    pub cancel_requested: bool,
    pub phase: Option<OperationPhase>,
}

impl CommandProtocolState {
    fn is_terminal(self) -> bool {
        self.lifecycle_state
            .is_some_and(CommandLifecycleState::is_terminal)
    }

    fn is_inflight(self) -> bool {
        matches!(
            self.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandProtocolExpectedOutput {
    Ack,
    StateSnapshot {
        state: CommandLifecycleState,
        stage_non_empty: bool,
        updated_at_non_zero: bool,
    },
    CancelResponse {
        cancellable: bool,
        final_state: CommandLifecycleState,
    },
    SpawnResult {
        spawned: bool,
        canceled: bool,
    },
    Rejected {
        code: CommandErrorKind,
        stage: CommandProtocolStageId,
    },
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CommandProtocolSpec;

impl CommandProtocolSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> CommandProtocolState {
        CommandProtocolState {
            tracked: false,
            lifecycle_state: None,
            cancel_requested: false,
            phase: None,
        }
    }

    fn transition_state(
        &self,
        prev: &CommandProtocolState,
        action: &CommandProtocolAction,
    ) -> Option<CommandProtocolState> {
        match action {
            CommandProtocolAction::Start(_) => {
                if prev.tracked {
                    None
                } else {
                    Some(CommandProtocolState {
                        tracked: true,
                        lifecycle_state: Some(CommandLifecycleState::Accepted),
                        cancel_requested: false,
                        phase: Some(OperationPhase::Starting),
                    })
                }
            }
            CommandProtocolAction::SetRunning
                if prev.tracked
                    && matches!(prev.lifecycle_state, Some(CommandLifecycleState::Accepted))
                    && matches!(prev.phase, Some(OperationPhase::Starting)) =>
            {
                let mut next = *prev;
                next.lifecycle_state = Some(CommandLifecycleState::Running);
                Some(next)
            }
            CommandProtocolAction::RequestCancel => {
                if !prev.tracked || !prev.is_inflight() {
                    None
                } else if prev.phase == Some(OperationPhase::Spawned) {
                    Some(*prev)
                } else {
                    let mut next = *prev;
                    next.cancel_requested = true;
                    Some(next)
                }
            }
            CommandProtocolAction::SnapshotRunning => {
                if !prev.tracked || !prev.is_inflight() {
                    None
                } else {
                    Some(*prev)
                }
            }
            CommandProtocolAction::MarkSpawned
                if prev.tracked
                    && matches!(prev.lifecycle_state, Some(CommandLifecycleState::Running))
                    && matches!(prev.phase, Some(OperationPhase::Starting)) =>
            {
                let mut next = *prev;
                next.phase = Some(OperationPhase::Spawned);
                next.cancel_requested = false;
                Some(next)
            }
            CommandProtocolAction::FinishSucceeded
                if prev.tracked
                    && prev.is_inflight()
                    && matches!(prev.phase, Some(OperationPhase::Spawned)) =>
            {
                let mut next = *prev;
                next.lifecycle_state = Some(CommandLifecycleState::Succeeded);
                next.cancel_requested = false;
                next.phase = Some(OperationPhase::Spawned);
                Some(next)
            }
            CommandProtocolAction::FinishFailed(_)
                if prev.tracked
                    && prev.is_inflight()
                    && matches!(prev.phase, Some(OperationPhase::Spawned)) =>
            {
                let mut next = *prev;
                next.lifecycle_state = Some(CommandLifecycleState::Failed);
                next.cancel_requested = false;
                next.phase = Some(OperationPhase::Spawned);
                Some(next)
            }
            CommandProtocolAction::FinishCanceled
                if prev.tracked
                    && prev.is_inflight()
                    && matches!(prev.phase, Some(OperationPhase::Spawned)) =>
            {
                let mut next = *prev;
                next.lifecycle_state = Some(CommandLifecycleState::Canceled);
                next.cancel_requested = false;
                next.phase = Some(OperationPhase::Spawned);
                Some(next)
            }
            CommandProtocolAction::Remove if prev.tracked && prev.is_terminal() => {
                Some(self.initial_state())
            }
            _ => None,
        }
    }

    fn transition_output(
        &self,
        prev: &CommandProtocolState,
        action: &CommandProtocolAction,
        next: Option<&CommandProtocolState>,
    ) -> CommandProtocolExpectedOutput {
        match (action, next) {
            (CommandProtocolAction::Start(_), None) => CommandProtocolExpectedOutput::Rejected {
                code: CommandErrorKind::Busy,
                stage: CommandProtocolStageId::CommandStart,
            },
            (CommandProtocolAction::SetRunning, None) => CommandProtocolExpectedOutput::Rejected {
                code: CommandErrorKind::NotFound,
                stage: CommandProtocolStageId::OperationState,
            },
            (CommandProtocolAction::RequestCancel, None) => {
                CommandProtocolExpectedOutput::Rejected {
                    code: CommandErrorKind::NotFound,
                    stage: CommandProtocolStageId::CommandCancel,
                }
            }
            (CommandProtocolAction::SnapshotRunning, None) => {
                CommandProtocolExpectedOutput::Rejected {
                    code: CommandErrorKind::NotFound,
                    stage: CommandProtocolStageId::StateRequest,
                }
            }
            (CommandProtocolAction::MarkSpawned, None) => CommandProtocolExpectedOutput::Rejected {
                code: CommandErrorKind::NotFound,
                stage: CommandProtocolStageId::OperationState,
            },
            (CommandProtocolAction::Remove, None) => CommandProtocolExpectedOutput::Rejected {
                code: CommandErrorKind::NotFound,
                stage: CommandProtocolStageId::OperationRemove,
            },
            (CommandProtocolAction::FinishSucceeded, None)
            | (CommandProtocolAction::FinishFailed(_), None)
            | (CommandProtocolAction::FinishCanceled, None) => {
                CommandProtocolExpectedOutput::Rejected {
                    code: CommandErrorKind::NotFound,
                    stage: CommandProtocolStageId::OperationState,
                }
            }
            (CommandProtocolAction::Start(_), Some(_))
            | (CommandProtocolAction::SetRunning, Some(_))
            | (CommandProtocolAction::FinishSucceeded, Some(_))
            | (CommandProtocolAction::FinishFailed(_), Some(_))
            | (CommandProtocolAction::FinishCanceled, Some(_))
            | (CommandProtocolAction::Remove, Some(_)) => CommandProtocolExpectedOutput::Ack,
            (CommandProtocolAction::SnapshotRunning, Some(_)) => {
                CommandProtocolExpectedOutput::StateSnapshot {
                    state: prev
                        .lifecycle_state
                        .expect("tracked states always carry lifecycle state"),
                    stage_non_empty: true,
                    updated_at_non_zero: true,
                }
            }
            (CommandProtocolAction::RequestCancel, Some(_)) => {
                if prev.phase == Some(OperationPhase::Spawned) {
                    CommandProtocolExpectedOutput::CancelResponse {
                        cancellable: false,
                        final_state: prev
                            .lifecycle_state
                            .expect("tracked states always carry lifecycle state"),
                    }
                } else {
                    CommandProtocolExpectedOutput::CancelResponse {
                        cancellable: true,
                        final_state: CommandLifecycleState::Canceled,
                    }
                }
            }
            (CommandProtocolAction::MarkSpawned, Some(_)) => {
                CommandProtocolExpectedOutput::SpawnResult {
                    spawned: !prev.cancel_requested,
                    canceled: prev.cancel_requested,
                }
            }
        }
    }
}

#[invariant(CommandProtocolSpec)]
fn tracked_requires_command_fields() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("tracked_requires_command_fields", |state| {
        state.tracked == state.lifecycle_state.is_some() && state.tracked == state.phase.is_some()
    })
}

#[invariant(CommandProtocolSpec)]
fn cancel_only_while_inflight() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("cancel_only_while_inflight", |state| {
        !state.cancel_requested
            || matches!(
                state.lifecycle_state,
                Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
            )
    })
}

#[invariant(CommandProtocolSpec)]
fn spawned_phase_requires_running_or_terminal_lifecycle() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new(
        "spawned_phase_requires_running_or_terminal_lifecycle",
        |state| {
            !matches!(state.phase, Some(OperationPhase::Spawned))
                || matches!(
                    state.lifecycle_state,
                    Some(
                        CommandLifecycleState::Running
                            | CommandLifecycleState::Succeeded
                            | CommandLifecycleState::Failed
                            | CommandLifecycleState::Canceled
                    )
                )
        },
    )
}

#[invariant(CommandProtocolSpec)]
fn accepted_state_stays_in_starting_phase() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("accepted_state_stays_in_starting_phase", |state| {
        !matches!(state.lifecycle_state, Some(CommandLifecycleState::Accepted))
            || matches!(state.phase, Some(OperationPhase::Starting))
    })
}

fn command_protocol_doc_graph_policy() -> DocGraphPolicy<CommandProtocolState> {
    DocGraphPolicy::boundary_paths()
        .with_focus_state(StatePredicate::new(
            "cancel_requested",
            |state: &CommandProtocolState| state.cancel_requested,
        ))
        .with_focus_state(StatePredicate::new(
            "terminal_lifecycle_state",
            |state: &CommandProtocolState| {
                matches!(
                    state.lifecycle_state,
                    Some(
                        CommandLifecycleState::Succeeded
                            | CommandLifecycleState::Failed
                            | CommandLifecycleState::Canceled
                    )
                )
            },
        ))
}

fn command_protocol_model_cases() -> Vec<ModelCase<CommandProtocolState, CommandProtocolAction>> {
    vec![
        ModelCase::default()
            .with_check_deadlocks(false)
            .with_doc_graph_policy(command_protocol_doc_graph_policy()),
    ]
}

#[subsystem_spec(model_cases(command_protocol_model_cases))]
impl TransitionSystem for CommandProtocolSpec {
    type State = CommandProtocolState;
    type Action = CommandProtocolAction;

    fn name(&self) -> &'static str {
        "command_protocol"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash_core::ActionVocabulary>::action_vocabulary()
    }

    fn transition(&self, state: &Self::State, action: &Self::Action) -> Option<Self::State> {
        self.transition_state(state, action)
    }
}

impl ProtocolConformanceSpec for CommandProtocolSpec {
    type ExpectedOutput = CommandProtocolExpectedOutput;
    type ObservedState = RuntimeCommandProtocolObservedState;
    type ObservedOutput = RuntimeCommandProtocolOutput;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        self.transition_output(prev, action, next)
    }

    fn project_state(&self, observed: &Self::ObservedState) -> Self::State {
        CommandProtocolState {
            tracked: observed.tracked,
            lifecycle_state: observed.lifecycle_state,
            cancel_requested: observed.cancel_requested,
            phase: observed.phase,
        }
    }

    fn project_output(&self, observed: &Self::ObservedOutput) -> Self::ExpectedOutput {
        match observed {
            RuntimeCommandProtocolOutput::Ack => CommandProtocolExpectedOutput::Ack,
            RuntimeCommandProtocolOutput::StateSnapshot {
                state,
                stage,
                updated_at_unix_secs,
            } => CommandProtocolExpectedOutput::StateSnapshot {
                state: *state,
                stage_non_empty: !stage.is_empty(),
                updated_at_non_zero: *updated_at_unix_secs > 0,
            },
            RuntimeCommandProtocolOutput::CancelResponse {
                cancellable,
                final_state,
            } => CommandProtocolExpectedOutput::CancelResponse {
                cancellable: *cancellable,
                final_state: *final_state,
            },
            RuntimeCommandProtocolOutput::SpawnResult { spawned, canceled } => {
                CommandProtocolExpectedOutput::SpawnResult {
                    spawned: *spawned,
                    canceled: *canceled,
                }
            }
            RuntimeCommandProtocolOutput::Rejected { code, stage } => {
                CommandProtocolExpectedOutput::Rejected {
                    code: *code,
                    stage: *stage,
                }
            }
        }
    }
}

#[nirvash_macros::formal_tests(spec = CommandProtocolSpec)]
const _: () = ();

pub fn initial_state() -> CommandProtocolState {
    CommandProtocolSpec::new().initial_state()
}

#[cfg(test)]
mod tests {
    use nirvash_core::{ModelCaseSource, ModelChecker, reduce_doc_graph};

    use super::*;

    #[test]
    fn command_contract_classifiers_cover_public_enums() {
        for state in SPEC_COMMAND_STATES {
            let _ = classify_command_state(state);
        }
        for code in SPEC_ERROR_CODES {
            let _ = classify_error_code(code);
        }
    }

    #[test]
    fn derived_action_vocabulary_preserves_representative_subset() {
        assert_eq!(
            <CommandProtocolAction as nirvash_core::ActionVocabulary>::action_vocabulary(),
            vec![
                CommandProtocolAction::Start(CommandKind::Deploy),
                CommandProtocolAction::Start(CommandKind::Run),
                CommandProtocolAction::Start(CommandKind::Stop),
                CommandProtocolAction::SetRunning,
                CommandProtocolAction::RequestCancel,
                CommandProtocolAction::SnapshotRunning,
                CommandProtocolAction::MarkSpawned,
                CommandProtocolAction::FinishSucceeded,
                CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
                CommandProtocolAction::FinishFailed(CommandErrorKind::Busy),
                CommandProtocolAction::FinishCanceled,
                CommandProtocolAction::Remove,
            ]
        );
    }

    #[test]
    fn doc_graph_policy_keeps_cancel_and_terminal_edge_cases() {
        let spec = CommandProtocolSpec::new();
        let model_case = spec.model_cases().into_iter().next().expect("model case");
        let snapshot = ModelChecker::for_case(&spec, model_case.clone())
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot");
        let original_edge_count = snapshot.edges.iter().map(Vec::len).sum::<usize>();
        let focus_indices = snapshot
            .states
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                model_case
                    .doc_graph_policy()
                    .focus_states
                    .iter()
                    .any(|predicate| predicate.eval(state))
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let reduced = reduce_doc_graph(&nirvash_core::DocGraphSnapshot {
            states: snapshot
                .states
                .iter()
                .map(nirvash_core::summarize_doc_graph_state)
                .collect(),
            edges: snapshot
                .edges
                .iter()
                .map(|outgoing| {
                    outgoing
                        .iter()
                        .map(|edge| nirvash_core::DocGraphEdge {
                            label: nirvash_core::format_doc_graph_action(&edge.action),
                            target: edge.target,
                        })
                        .collect()
                })
                .collect(),
            initial_indices: snapshot.initial_indices.clone(),
            deadlocks: snapshot.deadlocks.clone(),
            truncated: snapshot.truncated,
            stutter_omitted: snapshot.stutter_omitted,
            focus_indices,
            reduction: model_case.doc_graph_policy().reduction,
            max_edge_actions_in_label: model_case.doc_graph_policy().max_edge_actions_in_label,
        });

        assert!(
            reduced.states.len() < snapshot.states.len()
                || reduced.edges.len() < original_edge_count
        );
        assert!(
            reduced
                .states
                .iter()
                .any(|state| { state.state.full.contains("cancel_requested: true") })
        );
        assert!(reduced.states.iter().any(|state| {
            state
                .state
                .full
                .contains("lifecycle_state: Some(\n        Succeeded")
                || state
                    .state
                    .full
                    .contains("lifecycle_state: Some(\n        Failed")
                || state
                    .state
                    .full
                    .contains("lifecycle_state: Some(\n        Canceled")
        }));
        assert!(
            reduced.edges.iter().any(|edge| edge.label.contains('|'))
                || reduced
                    .edges
                    .iter()
                    .any(|edge| !edge.collapsed_state_indices.is_empty())
        );
    }

    #[test]
    fn terminal_states_cannot_resume_running_or_spawn() {
        let spec = CommandProtocolSpec::new();
        let terminal = CommandProtocolState {
            tracked: true,
            lifecycle_state: Some(CommandLifecycleState::Succeeded),
            cancel_requested: false,
            phase: Some(OperationPhase::Spawned),
        };

        assert!(
            spec.transition(&terminal, &CommandProtocolAction::SetRunning)
                .is_none()
        );
        assert!(
            spec.transition(&terminal, &CommandProtocolAction::MarkSpawned)
                .is_none()
        );
    }

    #[test]
    fn finish_actions_reject_missing_and_pre_spawn_states() {
        let spec = CommandProtocolSpec::new();
        let missing = spec.initial_state();
        let accepted = CommandProtocolState {
            tracked: true,
            lifecycle_state: Some(CommandLifecycleState::Accepted),
            cancel_requested: false,
            phase: Some(OperationPhase::Starting),
        };

        for state in [missing, accepted] {
            let next = spec.transition(&state, &CommandProtocolAction::FinishSucceeded);
            assert!(next.is_none());
            assert_eq!(
                spec.transition_output(
                    &state,
                    &CommandProtocolAction::FinishSucceeded,
                    next.as_ref(),
                ),
                CommandProtocolExpectedOutput::Rejected {
                    code: CommandErrorKind::NotFound,
                    stage: CommandProtocolStageId::OperationState,
                }
            );
        }
    }

    #[test]
    fn cancel_pending_path_is_preserved_until_finish() {
        let spec = CommandProtocolSpec::new();
        let running_starting = CommandProtocolState {
            tracked: true,
            lifecycle_state: Some(CommandLifecycleState::Running),
            cancel_requested: true,
            phase: Some(OperationPhase::Starting),
        };

        let spawned = spec
            .transition(&running_starting, &CommandProtocolAction::MarkSpawned)
            .expect("cancel-pending running state should still reach spawned");
        assert_eq!(
            spawned,
            CommandProtocolState {
                cancel_requested: false,
                phase: Some(OperationPhase::Spawned),
                ..running_starting
            }
        );
        assert_eq!(
            spec.transition_output(
                &running_starting,
                &CommandProtocolAction::MarkSpawned,
                Some(&spawned),
            ),
            CommandProtocolExpectedOutput::SpawnResult {
                spawned: false,
                canceled: true,
            }
        );

        let canceled = spec
            .transition(&spawned, &CommandProtocolAction::FinishCanceled)
            .expect("spawned state should still allow terminal cancellation");
        assert_eq!(
            canceled.lifecycle_state,
            Some(CommandLifecycleState::Canceled)
        );
        assert_eq!(canceled.phase, Some(OperationPhase::Spawned));
    }
}
