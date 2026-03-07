use imagod_control::OperationManager;
use imagod_model::{
    CommandErrorKind, CommandLifecycleState, CommandProtocolAction, CommandProtocolContext,
    CommandProtocolObservedState, CommandProtocolOutput, CommandProtocolStageId, OperationPhase,
};
use nirvash_core::{
    CodeConformanceSpec, DocGraphPolicy, ExpectedStep, ModelCheckConfig, Signature, StatePredicate,
    StepPredicate, TransitionSystem,
};
use nirvash_macros::{
    Signature as FormalSignature, code_tests, illegal, invariant, subsystem_spec,
};

#[cfg(test)]
use crate::bounds::{SPEC_COMMAND_STATES, SPEC_ERROR_CODES};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(filter(self => {
    let tracked_matches_fields =
        self.tracked == self.lifecycle_state.is_some() && self.tracked == self.phase.is_some();
    let cancel_matches_state = !self.cancel_requested
        || matches!(
            self.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        );

    tracked_matches_fields && cancel_matches_state
}))]
#[signature_invariant(self => {
    let tracked_matches_fields =
        self.tracked == self.lifecycle_state.is_some() && self.tracked == self.phase.is_some();
    let cancel_matches_state = !self.cancel_requested
        || matches!(
            self.lifecycle_state,
            Some(CommandLifecycleState::Accepted | CommandLifecycleState::Running)
        );

    tracked_matches_fields && cancel_matches_state
})]
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

#[illegal(CommandProtocolSpec)]
fn start_while_tracked() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("start_while_tracked", |prev, action, _| {
        matches!(action, CommandProtocolAction::Start(_)) && prev.tracked
    })
}

#[illegal(CommandProtocolSpec)]
fn snapshot_when_absent_or_terminal() -> StepPredicate<CommandProtocolState, CommandProtocolAction>
{
    StepPredicate::new("snapshot_when_absent_or_terminal", |prev, action, _| {
        matches!(action, CommandProtocolAction::SnapshotRunning)
            && (!prev.tracked || prev.is_terminal())
    })
}

#[illegal(CommandProtocolSpec)]
fn cancel_when_absent_or_terminal() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("cancel_when_absent_or_terminal", |prev, action, _| {
        matches!(action, CommandProtocolAction::RequestCancel)
            && (!prev.tracked || prev.is_terminal())
    })
}

#[illegal(CommandProtocolSpec)]
fn remove_when_absent_or_inflight() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("remove_when_absent_or_inflight", |prev, action, _| {
        matches!(action, CommandProtocolAction::Remove) && (!prev.tracked || !prev.is_terminal())
    })
}

fn command_protocol_checker_config() -> ModelCheckConfig {
    ModelCheckConfig {
        check_deadlocks: false,
        ..ModelCheckConfig::default()
    }
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

#[subsystem_spec(
    checker_config(command_protocol_checker_config),
    doc_graph_policy(command_protocol_doc_graph_policy)
)]
impl TransitionSystem for CommandProtocolSpec {
    type State = CommandProtocolState;
    type Action = CommandProtocolAction;

    fn name(&self) -> &'static str {
        "command_protocol"
    }

    fn init(&self, state: &Self::State) -> bool {
        *state == self.initial_state()
    }

    fn next(&self, prev: &Self::State, action: &Self::Action, next: &Self::State) -> bool {
        match <Self as CodeConformanceSpec>::expected_step(self, prev, action) {
            ExpectedStep::Allowed { next: expected, .. } => expected == *next && next.invariant(),
            ExpectedStep::Rejected { .. } => false,
        }
    }
}

impl CodeConformanceSpec for CommandProtocolSpec {
    type Runtime = OperationManager;
    type Context = CommandProtocolContext;
    type ExpectedOutput = CommandProtocolExpectedOutput;
    type ObservedState = CommandProtocolObservedState;
    type ObservedOutput = CommandProtocolOutput;

    async fn fresh_runtime(&self) -> Self::Runtime {
        OperationManager::new()
    }

    fn context(&self) -> Self::Context {
        CommandProtocolContext {
            request_id: uuid::Uuid::from_u128(1),
        }
    }

    fn expected_step(
        &self,
        prev: &Self::State,
        action: &Self::Action,
    ) -> ExpectedStep<Self::State, Self::ExpectedOutput> {
        match action {
            CommandProtocolAction::Start(_) => {
                if prev.tracked {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::Busy,
                            stage: CommandProtocolStageId::CommandStart,
                        },
                    }
                } else {
                    ExpectedStep::Allowed {
                        next: CommandProtocolState {
                            tracked: true,
                            lifecycle_state: Some(CommandLifecycleState::Accepted),
                            cancel_requested: false,
                            phase: Some(OperationPhase::Starting),
                        },
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                }
            }
            CommandProtocolAction::SetRunning => {
                if !prev.tracked {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::NotFound,
                            stage: CommandProtocolStageId::OperationState,
                        },
                    }
                } else {
                    let mut next = *prev;
                    next.lifecycle_state = Some(CommandLifecycleState::Running);
                    ExpectedStep::Allowed {
                        next,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                }
            }
            CommandProtocolAction::RequestCancel => {
                if !prev.tracked || prev.is_terminal() {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::NotFound,
                            stage: CommandProtocolStageId::CommandCancel,
                        },
                    }
                } else if prev.phase == Some(OperationPhase::Spawned) {
                    ExpectedStep::Allowed {
                        next: *prev,
                        output: CommandProtocolExpectedOutput::CancelResponse {
                            cancellable: false,
                            final_state: prev
                                .lifecycle_state
                                .expect("tracked states always carry lifecycle state"),
                        },
                    }
                } else {
                    let mut next = *prev;
                    next.cancel_requested = true;
                    ExpectedStep::Allowed {
                        next,
                        output: CommandProtocolExpectedOutput::CancelResponse {
                            cancellable: true,
                            final_state: CommandLifecycleState::Canceled,
                        },
                    }
                }
            }
            CommandProtocolAction::SnapshotRunning => {
                if !prev.tracked || prev.is_terminal() {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::NotFound,
                            stage: CommandProtocolStageId::StateRequest,
                        },
                    }
                } else {
                    ExpectedStep::Allowed {
                        next: *prev,
                        output: CommandProtocolExpectedOutput::StateSnapshot {
                            state: prev
                                .lifecycle_state
                                .expect("tracked states always carry lifecycle state"),
                            stage_non_empty: true,
                            updated_at_non_zero: true,
                        },
                    }
                }
            }
            CommandProtocolAction::MarkSpawned => {
                if !prev.tracked {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::NotFound,
                            stage: CommandProtocolStageId::OperationState,
                        },
                    }
                } else {
                    let mut next = *prev;
                    next.phase = Some(OperationPhase::Spawned);
                    if prev.cancel_requested {
                        next.cancel_requested = false;
                        ExpectedStep::Allowed {
                            next,
                            output: CommandProtocolExpectedOutput::SpawnResult {
                                spawned: false,
                                canceled: true,
                            },
                        }
                    } else {
                        ExpectedStep::Allowed {
                            next,
                            output: CommandProtocolExpectedOutput::SpawnResult {
                                spawned: true,
                                canceled: false,
                            },
                        }
                    }
                }
            }
            CommandProtocolAction::FinishSucceeded => {
                if !prev.tracked {
                    ExpectedStep::Allowed {
                        next: *prev,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                } else {
                    let mut next = *prev;
                    next.lifecycle_state = Some(CommandLifecycleState::Succeeded);
                    next.cancel_requested = false;
                    next.phase = Some(OperationPhase::Spawned);
                    ExpectedStep::Allowed {
                        next,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                }
            }
            CommandProtocolAction::FinishFailed(_) => {
                if !prev.tracked {
                    ExpectedStep::Allowed {
                        next: *prev,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                } else {
                    let mut next = *prev;
                    next.lifecycle_state = Some(CommandLifecycleState::Failed);
                    next.cancel_requested = false;
                    next.phase = Some(OperationPhase::Spawned);
                    ExpectedStep::Allowed {
                        next,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                }
            }
            CommandProtocolAction::FinishCanceled => {
                if !prev.tracked {
                    ExpectedStep::Allowed {
                        next: *prev,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                } else {
                    let mut next = *prev;
                    next.lifecycle_state = Some(CommandLifecycleState::Canceled);
                    next.cancel_requested = false;
                    next.phase = Some(OperationPhase::Spawned);
                    ExpectedStep::Allowed {
                        next,
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                }
            }
            CommandProtocolAction::Remove => {
                if prev.tracked && prev.is_terminal() {
                    ExpectedStep::Allowed {
                        next: self.initial_state(),
                        output: CommandProtocolExpectedOutput::Ack,
                    }
                } else {
                    ExpectedStep::Rejected {
                        output: CommandProtocolExpectedOutput::Rejected {
                            code: CommandErrorKind::NotFound,
                            stage: CommandProtocolStageId::OperationRemove,
                        },
                    }
                }
            }
        }
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
            CommandProtocolOutput::Ack => CommandProtocolExpectedOutput::Ack,
            CommandProtocolOutput::StateSnapshot {
                state,
                stage,
                updated_at_unix_secs,
            } => CommandProtocolExpectedOutput::StateSnapshot {
                state: *state,
                stage_non_empty: !stage.is_empty(),
                updated_at_non_zero: *updated_at_unix_secs > 0,
            },
            CommandProtocolOutput::CancelResponse {
                cancellable,
                final_state,
            } => CommandProtocolExpectedOutput::CancelResponse {
                cancellable: *cancellable,
                final_state: *final_state,
            },
            CommandProtocolOutput::SpawnResult { spawned, canceled } => {
                CommandProtocolExpectedOutput::SpawnResult {
                    spawned: *spawned,
                    canceled: *canceled,
                }
            }
            CommandProtocolOutput::Rejected { code, stage } => {
                CommandProtocolExpectedOutput::Rejected {
                    code: *code,
                    stage: *stage,
                }
            }
        }
    }
}

#[nirvash_macros::formal_tests(spec = CommandProtocolSpec, init = initial_state)]
const _: () = ();

#[code_tests(
    spec = CommandProtocolSpec,
    init = initial_state
)]
const _: () = ();

pub fn initial_state() -> CommandProtocolState {
    CommandProtocolSpec::new().initial_state()
}

#[cfg(test)]
mod tests {
    use imagod_model::CommandKind;
    use nirvash_core::{ModelChecker, Signature, TemporalSpec, reduce_doc_graph};

    use super::*;

    #[test]
    fn failing_command_does_not_extend_runtime_state_shape() {
        let spec = CommandProtocolSpec::new();
        let prev = CommandProtocolState {
            tracked: true,
            lifecycle_state: Some(CommandLifecycleState::Running),
            cancel_requested: false,
            phase: Some(OperationPhase::Spawned),
        };

        let expected = spec.expected_step(
            &prev,
            &CommandProtocolAction::FinishFailed(CommandErrorKind::Internal),
        );

        match expected {
            ExpectedStep::Allowed { next, .. } => {
                assert_eq!(next.lifecycle_state, Some(CommandLifecycleState::Failed));
                assert!(!next.cancel_requested);
                assert_eq!(next.phase, Some(OperationPhase::Spawned));
            }
            ExpectedStep::Rejected { .. } => panic!("finish_failed should be allowed"),
        }
    }

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
    fn bounded_domain_filters_out_partial_tracking_states() {
        let domain = CommandProtocolState::bounded_domain().into_vec();

        assert!(domain.iter().all(|state| {
            state.tracked == state.lifecycle_state.is_some()
                && state.tracked == state.phase.is_some()
        }));
        assert!(
            domain
                .iter()
                .all(|state| !state.cancel_requested || state.tracked)
        );
    }

    #[test]
    fn project_output_reduces_runtime_snapshot_to_expected_shape() {
        let spec = CommandProtocolSpec::new();
        let projected = spec.project_output(&CommandProtocolOutput::StateSnapshot {
            state: CommandLifecycleState::Running,
            stage: "starting".to_owned(),
            updated_at_unix_secs: 1,
        });
        assert_eq!(
            projected,
            CommandProtocolExpectedOutput::StateSnapshot {
                state: CommandLifecycleState::Running,
                stage_non_empty: true,
                updated_at_non_zero: true,
            }
        );
    }

    #[test]
    fn start_action_is_shared_contract_even_without_state_memory() {
        let spec = CommandProtocolSpec::new();
        let expected = spec.expected_step(
            &spec.initial_state(),
            &CommandProtocolAction::Start(CommandKind::Deploy),
        );
        match expected {
            ExpectedStep::Allowed { next, output } => {
                assert!(next.tracked);
                assert_eq!(output, CommandProtocolExpectedOutput::Ack);
            }
            ExpectedStep::Rejected { .. } => panic!("start should be allowed from init"),
        }
    }

    #[test]
    fn doc_graph_policy_keeps_cancel_and_terminal_edge_cases() {
        let spec = CommandProtocolSpec::new();
        let snapshot = ModelChecker::new(&spec)
            .reachable_graph_snapshot()
            .expect("reachable graph snapshot");
        let original_edge_count = snapshot.edges.iter().map(Vec::len).sum::<usize>();
        let focus_indices = snapshot
            .states
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                spec.doc_graph_policy()
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
                            label: format!("{:?}", edge.action),
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
            reduction: spec.doc_graph_policy().reduction,
            max_edge_actions_in_label: spec.doc_graph_policy().max_edge_actions_in_label,
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
}
