use imagod_spec::{
    CommandOutputSummary as RuntimeCommandOutputSummary,
    CommandStateSummary as RuntimeCommandStateSummary,
};
use nirvash::{BoolExpr, DocGraphPolicy};
use nirvash_conformance::ProtocolConformanceSpec;
use nirvash_lower::{FrontendSpec, ModelInstance};
use nirvash_macros::{
    FiniteModelDomain as FormalFiniteModelDomain, SymbolicEncoding as FormalSymbolicEncoding,
    invariant, nirvash_expr, nirvash_transition_program, subsystem_spec,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalFiniteModelDomain, FormalSymbolicEncoding)]
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

    #[allow(dead_code)]
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
fn tracked_requires_command_fields() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { tracked_requires_command_fields(state) =>
        state.tracked == state.lifecycle_state.is_some() && state.tracked == state.phase.is_some()
    }
}

#[invariant(CommandProtocolSpec)]
fn cancel_only_while_inflight() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { cancel_only_while_inflight(state) =>
        !state.cancel_requested
            || matches!(
                state.lifecycle_state,
                Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
            )
    }
}

#[invariant(CommandProtocolSpec)]
fn spawned_phase_requires_running_or_terminal_lifecycle() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { spawned_phase_requires_running_or_terminal_lifecycle(state) =>
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
    }
}

#[invariant(CommandProtocolSpec)]
fn accepted_state_stays_in_starting_phase() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { accepted_state_stays_in_starting_phase(state) =>
        !matches!(state.lifecycle_state, Some(CommandLifecycleState::Accepted))
            || matches!(state.phase, Some(OperationPhase::Starting))
    }
}

fn command_protocol_doc_graph_policy() -> DocGraphPolicy<CommandProtocolState> {
    DocGraphPolicy::boundary_paths()
        .with_focus_state(cancel_requested_focus_state())
        .with_focus_state(terminal_lifecycle_focus_state())
}

fn cancel_requested_focus_state() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { cancel_requested(state) => state.cancel_requested }
}

fn terminal_lifecycle_focus_state() -> BoolExpr<CommandProtocolState> {
    nirvash_expr! { terminal_lifecycle_state(state) =>
        matches!(
            state.lifecycle_state,
            Some(
                CommandLifecycleState::Succeeded
                    | CommandLifecycleState::Failed
                    | CommandLifecycleState::Canceled
            )
        )
    }
}

fn command_protocol_model_cases() -> Vec<ModelInstance<CommandProtocolState, CommandProtocolAction>>
{
    vec![
        ModelInstance::default()
            .with_check_deadlocks(false)
            .with_doc_graph_policy(command_protocol_doc_graph_policy()),
    ]
}

#[subsystem_spec(model_cases(command_protocol_model_cases))]
impl FrontendSpec for CommandProtocolSpec {
    type State = CommandProtocolState;
    type Action = CommandProtocolAction;

    fn frontend_name(&self) -> &'static str {
        "command_protocol"
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![self.initial_state()]
    }

    fn actions(&self) -> Vec<Self::Action> {
        <Self::Action as nirvash::ActionVocabulary>::action_vocabulary()
    }

    fn transition_program(
        &self,
    ) -> Option<::nirvash::TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule start when matches!(action, CommandProtocolAction::Start(_)) && !prev.tracked => {
                set tracked <= true;
                set lifecycle_state <= Some(CommandLifecycleState::Accepted);
                set cancel_requested <= false;
                set phase <= Some(OperationPhase::Starting);
            }

            rule set_running when matches!(action, CommandProtocolAction::SetRunning)
                && prev.tracked
                && matches!(prev.lifecycle_state, Some(CommandLifecycleState::Accepted))
                && matches!(prev.phase, Some(OperationPhase::Starting)) => {
                set lifecycle_state <= Some(CommandLifecycleState::Running);
            }

            rule request_cancel_spawned when matches!(action, CommandProtocolAction::RequestCancel)
                && prev.tracked
                && prev.is_inflight()
                && prev.phase == Some(OperationPhase::Spawned) => {
            }

            rule request_cancel_pending when matches!(action, CommandProtocolAction::RequestCancel)
                && prev.tracked
                && prev.is_inflight()
                && prev.phase != Some(OperationPhase::Spawned) => {
                set cancel_requested <= true;
            }

            rule snapshot_running when matches!(action, CommandProtocolAction::SnapshotRunning)
                && prev.tracked
                && prev.is_inflight() => {
            }

            rule mark_spawned when matches!(action, CommandProtocolAction::MarkSpawned)
                && prev.tracked
                && matches!(prev.lifecycle_state, Some(CommandLifecycleState::Running))
                && matches!(prev.phase, Some(OperationPhase::Starting)) => {
                set phase <= Some(OperationPhase::Spawned);
                set cancel_requested <= false;
            }

            rule finish_succeeded when matches!(action, CommandProtocolAction::FinishSucceeded)
                && prev.tracked
                && prev.is_inflight()
                && matches!(prev.phase, Some(OperationPhase::Spawned)) => {
                set lifecycle_state <= Some(CommandLifecycleState::Succeeded);
                set cancel_requested <= false;
                set phase <= Some(OperationPhase::Spawned);
            }

            rule finish_failed when matches!(action, CommandProtocolAction::FinishFailed(_))
                && prev.tracked
                && prev.is_inflight()
                && matches!(prev.phase, Some(OperationPhase::Spawned)) => {
                set lifecycle_state <= Some(CommandLifecycleState::Failed);
                set cancel_requested <= false;
                set phase <= Some(OperationPhase::Spawned);
            }

            rule finish_canceled when matches!(action, CommandProtocolAction::FinishCanceled)
                && prev.tracked
                && prev.is_inflight()
                && matches!(prev.phase, Some(OperationPhase::Spawned)) => {
                set lifecycle_state <= Some(CommandLifecycleState::Canceled);
                set cancel_requested <= false;
                set phase <= Some(OperationPhase::Spawned);
            }

            rule remove when matches!(action, CommandProtocolAction::Remove)
                && prev.tracked
                && prev.is_terminal() => {
                set tracked <= false;
                set lifecycle_state <= None;
                set cancel_requested <= false;
                set phase <= None;
            }
        })
    }
}

#[allow(dead_code)]
fn command_protocol_transition(
    prev: &CommandProtocolState,
    action: &CommandProtocolAction,
) -> Option<CommandProtocolState> {
    CommandProtocolSpec::new().transition_state(prev, action)
}

impl ProtocolConformanceSpec for CommandProtocolSpec {
    type ExpectedOutput = CommandProtocolExpectedOutput;
    type ProbeState = RuntimeCommandStateSummary;
    type ProbeOutput = RuntimeCommandOutputSummary;
    type SummaryState = RuntimeCommandStateSummary;
    type SummaryOutput = RuntimeCommandOutputSummary;

    fn expected_output(
        &self,
        prev: &Self::State,
        action: &Self::Action,
        next: Option<&Self::State>,
    ) -> Self::ExpectedOutput {
        self.transition_output(prev, action, next)
    }

    fn summarize_state(&self, probe: &Self::ProbeState) -> Self::SummaryState {
        *probe
    }

    fn summarize_output(&self, probe: &Self::ProbeOutput) -> Self::SummaryOutput {
        probe.clone()
    }

    fn abstract_state(&self, observed: &Self::SummaryState) -> Self::State {
        CommandProtocolState {
            tracked: observed.tracked,
            lifecycle_state: observed.lifecycle_state,
            cancel_requested: observed.cancel_requested,
            phase: observed.phase,
        }
    }

    fn abstract_output(&self, observed: &Self::SummaryOutput) -> Self::ExpectedOutput {
        match observed {
            RuntimeCommandOutputSummary::Ack => CommandProtocolExpectedOutput::Ack,
            RuntimeCommandOutputSummary::StateSnapshot {
                state,
                stage,
                updated_at_unix_secs,
            } => CommandProtocolExpectedOutput::StateSnapshot {
                state: *state,
                stage_non_empty: !stage.is_empty(),
                updated_at_non_zero: *updated_at_unix_secs > 0,
            },
            RuntimeCommandOutputSummary::CancelResponse {
                cancellable,
                final_state,
            } => CommandProtocolExpectedOutput::CancelResponse {
                cancellable: *cancellable,
                final_state: *final_state,
            },
            RuntimeCommandOutputSummary::SpawnResult { spawned, canceled } => {
                CommandProtocolExpectedOutput::SpawnResult {
                    spawned: *spawned,
                    canceled: *canceled,
                }
            }
            RuntimeCommandOutputSummary::Rejected { code, stage } => {
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
    use nirvash::reduce_doc_graph;
    use nirvash_check::ModelChecker;

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
            <CommandProtocolAction as nirvash::ActionVocabulary>::action_vocabulary(),
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
        let mut lowering_cx = nirvash_lower::LoweringCx;
        let lowered =
            <CommandProtocolSpec as nirvash_lower::FrontendSpec>::lower(&spec, &mut lowering_cx)
                .expect("spec should lower");
        let model_case = lowered
            .model_instances()
            .into_iter()
            .next()
            .expect("model case");
        let snapshot = ModelChecker::for_case(&lowered, model_case.clone())
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
        let reduced = reduce_doc_graph(&nirvash::DocGraphSnapshot {
            states: snapshot
                .states
                .iter()
                .map(nirvash::summarize_doc_graph_state)
                .collect(),
            edges: snapshot
                .edges
                .iter()
                .map(|outgoing| {
                    outgoing
                        .iter()
                        .map(|edge| {
                            let presentation = nirvash::describe_doc_graph_action(&edge.action);
                            nirvash::DocGraphEdge {
                                label: presentation.label,
                                compact_label: presentation.compact_label,
                                scenario_priority: presentation.scenario_priority,
                                interaction_steps: presentation.interaction_steps,
                                process_steps: presentation.process_steps,
                                target: edge.target,
                            }
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
