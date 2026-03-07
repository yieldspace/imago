use imago_formal_core::{
    BoundedDomain, Fairness, Ltl, Signature, StatePredicate, StepPredicate, TransitionSystem,
};
use imago_formal_macros::{
    Signature as FormalSignature, imago_fairness, imago_illegal, imago_invariant, imago_property,
    imago_subsystem_spec,
};
use imago_protocol::{CommandState, CommandType, ErrorCode};

use crate::bounds::{SPEC_COMMAND_TYPES, SPEC_ERROR_CODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStateClass {
    InFlight,
    Terminal,
}

pub fn classify_command_state(state: CommandState) -> CommandStateClass {
    match state {
        CommandState::Accepted | CommandState::Running => CommandStateClass::InFlight,
        CommandState::Succeeded | CommandState::Failed | CommandState::Canceled => {
            CommandStateClass::Terminal
        }
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

pub fn classify_error_code(code: ErrorCode) -> ErrorCodeClass {
    match code {
        ErrorCode::Unauthorized => ErrorCodeClass::Authentication,
        ErrorCode::BadRequest | ErrorCode::BadManifest | ErrorCode::RangeInvalid => {
            ErrorCodeClass::Validation
        }
        ErrorCode::Busy
        | ErrorCode::NotFound
        | ErrorCode::IdempotencyConflict
        | ErrorCode::PreconditionFailed
        | ErrorCode::OperationTimeout
        | ErrorCode::RollbackFailed => ErrorCodeClass::Operational,
        ErrorCode::ChunkHashMismatch | ErrorCode::ArtifactIncomplete | ErrorCode::StorageQuota => {
            ErrorCodeClass::Storage
        }
        ErrorCode::Internal => ErrorCodeClass::Internal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FormalSignature)]
#[signature(custom)]
pub struct CommandProtocolState {
    pub command_type: Option<CommandType>,
    pub command_state: Option<CommandState>,
    pub cancel_requested: bool,
    pub last_error: Option<ErrorCode>,
    pub state_poll_allowed: bool,
}

impl CommandProtocolStateSignatureSpec for CommandProtocolState {
    fn representatives() -> BoundedDomain<Self> {
        BoundedDomain::new(vec![
            CommandProtocolSpec::new().initial_state(),
            Self {
                command_type: Some(CommandType::Deploy),
                command_state: Some(CommandState::Accepted),
                cancel_requested: false,
                last_error: None,
                state_poll_allowed: true,
            },
            Self {
                command_type: Some(CommandType::Run),
                command_state: Some(CommandState::Running),
                cancel_requested: true,
                last_error: None,
                state_poll_allowed: true,
            },
            Self {
                command_type: Some(CommandType::Stop),
                command_state: Some(CommandState::Failed),
                cancel_requested: false,
                last_error: Some(ErrorCode::Internal),
                state_poll_allowed: false,
            },
            Self {
                command_type: Some(CommandType::Deploy),
                command_state: Some(CommandState::Failed),
                cancel_requested: false,
                last_error: Some(ErrorCode::Internal),
                state_poll_allowed: false,
            },
            Self {
                command_type: Some(CommandType::Run),
                command_state: Some(CommandState::Succeeded),
                cancel_requested: false,
                last_error: None,
                state_poll_allowed: false,
            },
            Self {
                command_type: Some(CommandType::Run),
                command_state: Some(CommandState::Canceled),
                cancel_requested: false,
                last_error: None,
                state_poll_allowed: false,
            },
        ])
    }

    fn signature_invariant(&self) -> bool {
        let state_matches_type = self.command_state.is_some() == self.command_type.is_some();
        let error_matches_state =
            self.last_error.is_some() == matches!(self.command_state, Some(CommandState::Failed));
        let cancel_matches_state = !self.cancel_requested
            || matches!(
                self.command_state,
                Some(CommandState::Accepted | CommandState::Running)
            );
        let poll_matches_state = !self.state_poll_allowed
            || matches!(
                self.command_state,
                Some(CommandState::Accepted | CommandState::Running)
            );

        state_matches_type && error_matches_state && cancel_matches_state && poll_matches_state
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandProtocolAction {
    Start(CommandType),
    EmitProgress,
    RequestCancel,
    PollState,
    EmitSucceeded,
    EmitFailed(ErrorCode),
    EmitCanceled,
    ClearTerminal,
}

impl Signature for CommandProtocolAction {
    fn bounded_domain() -> BoundedDomain<Self> {
        let mut values = SPEC_COMMAND_TYPES
            .into_iter()
            .map(Self::Start)
            .collect::<Vec<_>>();
        values.extend([
            Self::EmitProgress,
            Self::RequestCancel,
            Self::PollState,
            Self::EmitSucceeded,
            Self::EmitCanceled,
            Self::ClearTerminal,
        ]);
        values.extend(SPEC_ERROR_CODES.into_iter().map(Self::EmitFailed));
        BoundedDomain::new(values)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CommandProtocolSpec;

impl CommandProtocolSpec {
    pub const fn new() -> Self {
        Self
    }

    pub const fn initial_state(&self) -> CommandProtocolState {
        CommandProtocolState {
            command_type: None,
            command_state: None,
            cancel_requested: false,
            last_error: None,
            state_poll_allowed: false,
        }
    }
}

#[imago_invariant]
fn command_state_requires_type() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("command_state_requires_type", |state| {
        state.command_state.is_some() == state.command_type.is_some()
    })
}

#[imago_invariant]
fn failed_requires_error() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("failed_requires_error", |state| {
        state.last_error.is_some() == matches!(state.command_state, Some(CommandState::Failed))
    })
}

#[imago_invariant]
fn cancel_only_when_inflight() -> StatePredicate<CommandProtocolState> {
    StatePredicate::new("cancel_only_when_inflight", |state| {
        !state.cancel_requested
            || matches!(
                state.command_state,
                Some(CommandState::Accepted | CommandState::Running)
            )
    })
}

#[imago_illegal]
fn poll_idle_command() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("poll_idle_command", |prev, action, _| {
        matches!(action, CommandProtocolAction::PollState) && prev.command_state.is_none()
    })
}

#[imago_illegal]
fn cancel_without_request() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("cancel_without_request", |prev, action, _| {
        matches!(action, CommandProtocolAction::EmitCanceled) && !prev.cancel_requested
    })
}

#[imago_illegal]
fn start_while_busy() -> StepPredicate<CommandProtocolState, CommandProtocolAction> {
    StepPredicate::new("start_while_busy", |prev, action, _| {
        matches!(action, CommandProtocolAction::Start(_)) && prev.command_state.is_some()
    })
}

#[imago_property]
fn inflight_leads_to_terminal() -> Ltl<CommandProtocolState, CommandProtocolAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("inflight", |state| {
            matches!(
                state.command_state,
                Some(CommandState::Accepted | CommandState::Running)
            )
        })),
        Ltl::pred(StatePredicate::new("terminal", |state| {
            matches!(
                state.command_state,
                Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
            )
        })),
    )
}

#[imago_property]
fn cancel_request_leads_to_terminal() -> Ltl<CommandProtocolState, CommandProtocolAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("cancel_requested", |state| {
            state.cancel_requested
        })),
        Ltl::pred(StatePredicate::new("terminal_after_cancel", |state| {
            matches!(
                state.command_state,
                Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
            )
        })),
    )
}

#[imago_property]
fn terminal_leads_to_cleared() -> Ltl<CommandProtocolState, CommandProtocolAction> {
    Ltl::leads_to(
        Ltl::pred(StatePredicate::new("terminal_pending_clear", |state| {
            matches!(
                state.command_state,
                Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
            )
        })),
        Ltl::pred(StatePredicate::new("cleared", |state| {
            state.command_state.is_none()
        })),
    )
}

#[imago_fairness]
fn terminal_emission_fairness() -> Fairness<CommandProtocolState, CommandProtocolAction> {
    Fairness::weak(StepPredicate::new("emit_terminal", |_, action, next| {
        matches!(
            action,
            CommandProtocolAction::EmitSucceeded
                | CommandProtocolAction::EmitFailed(_)
                | CommandProtocolAction::EmitCanceled
        ) && matches!(
            next.command_state,
            Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
        )
    }))
}

#[imago_fairness]
fn clear_terminal_fairness() -> Fairness<CommandProtocolState, CommandProtocolAction> {
    Fairness::weak(StepPredicate::new(
        "clear_terminal",
        |prev, action, next| {
            matches!(action, CommandProtocolAction::ClearTerminal)
                && matches!(
                    prev.command_state,
                    Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
                )
                && next.command_state.is_none()
        },
    ))
}

#[imago_subsystem_spec(
    invariants(
        command_state_requires_type,
        failed_requires_error,
        cancel_only_when_inflight
    ),
    illegal(poll_idle_command, cancel_without_request, start_while_busy),
    properties(
        inflight_leads_to_terminal,
        cancel_request_leads_to_terminal,
        terminal_leads_to_cleared
    ),
    fairness(terminal_emission_fairness, clear_terminal_fairness)
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
        let mut candidate = *prev;
        match action {
            CommandProtocolAction::Start(command_type) if prev.command_state.is_none() => {
                candidate.command_type = Some(*command_type);
                candidate.command_state = Some(CommandState::Accepted);
                candidate.cancel_requested = false;
                candidate.last_error = None;
                candidate.state_poll_allowed = true;
            }
            CommandProtocolAction::EmitProgress
                if matches!(
                    prev.command_state,
                    Some(CommandState::Accepted | CommandState::Running)
                ) =>
            {
                candidate.command_state = Some(CommandState::Running);
                candidate.state_poll_allowed = true;
            }
            CommandProtocolAction::RequestCancel
                if matches!(
                    prev.command_state,
                    Some(CommandState::Accepted | CommandState::Running)
                ) =>
            {
                candidate.cancel_requested = true;
            }
            CommandProtocolAction::PollState
                if matches!(
                    prev.command_state,
                    Some(CommandState::Accepted | CommandState::Running)
                ) =>
            {
                candidate.state_poll_allowed = true;
            }
            CommandProtocolAction::EmitSucceeded
                if matches!(prev.command_state, Some(CommandState::Running)) =>
            {
                candidate.command_state = Some(CommandState::Succeeded);
                candidate.cancel_requested = false;
                candidate.state_poll_allowed = false;
            }
            CommandProtocolAction::EmitFailed(code)
                if matches!(
                    prev.command_state,
                    Some(CommandState::Accepted | CommandState::Running)
                ) =>
            {
                candidate.command_state = Some(CommandState::Failed);
                candidate.cancel_requested = false;
                candidate.last_error = Some(*code);
                candidate.state_poll_allowed = false;
            }
            CommandProtocolAction::EmitCanceled
                if prev.cancel_requested
                    && matches!(
                        prev.command_state,
                        Some(CommandState::Accepted | CommandState::Running)
                    ) =>
            {
                candidate.command_state = Some(CommandState::Canceled);
                candidate.cancel_requested = false;
                candidate.last_error = None;
                candidate.state_poll_allowed = false;
            }
            CommandProtocolAction::ClearTerminal
                if matches!(
                    prev.command_state,
                    Some(CommandState::Succeeded | CommandState::Failed | CommandState::Canceled)
                ) =>
            {
                candidate = self.initial_state();
            }
            _ => return false,
        }

        candidate == *next && candidate.invariant()
    }
}

#[cfg(test)]
#[imago_formal_macros::imago_formal_tests(spec = CommandProtocolSpec, init = initial_state)]
const _: () = ();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::SPEC_COMMAND_STATES;

    #[test]
    fn failing_command_captures_error_code() {
        let spec = CommandProtocolSpec::new();
        let prev = CommandProtocolState {
            command_type: Some(CommandType::Run),
            command_state: Some(CommandState::Running),
            cancel_requested: false,
            last_error: None,
            state_poll_allowed: true,
        };
        let next = CommandProtocolState {
            command_type: Some(CommandType::Run),
            command_state: Some(CommandState::Failed),
            cancel_requested: false,
            last_error: Some(ErrorCode::Internal),
            state_poll_allowed: false,
        };
        assert!(spec.next(
            &prev,
            &CommandProtocolAction::EmitFailed(ErrorCode::Internal),
            &next
        ));
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
}
