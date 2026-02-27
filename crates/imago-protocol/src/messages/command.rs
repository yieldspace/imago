//! Command lifecycle payloads (`start`, `event`, `state`, `cancel`).
//!
//! Validation in this module enforces key runtime invariants:
//! - command payload shape MUST match `command_type`
//! - state polling responses MUST remain non-terminal
//! - progress/failed events MUST include required context

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StructuredError;
use crate::validate::{
    Validate, ValidationError, ensure_non_empty, ensure_required_strings, ensure_uuid_not_nil,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Top-level command operation kind.
pub enum CommandType {
    #[serde(rename = "deploy")]
    Deploy,
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "stop")]
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Emitted lifecycle transitions for one command request.
pub enum CommandEventType {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "progress")]
    Progress,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "canceled")]
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Snapshot state used by `state.response` and cancel outcomes.
pub enum CommandState {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "succeeded")]
    Succeeded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "canceled")]
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Request payload for `command.start`.
///
/// # Examples
/// ```rust
/// use uuid::Uuid;
/// use imago_protocol::{messages::{CommandPayload, CommandStartRequest, CommandType, RunCommandPayload}, Validate};
///
/// let request = CommandStartRequest {
///     request_id: Uuid::new_v4(),
///     command_type: CommandType::Run,
///     payload: CommandPayload::Run(RunCommandPayload {
///         name: "svc-a".to_string(),
///     }),
/// };
/// request.validate().expect("payload must match command_type");
/// ```
pub struct CommandStartRequest {
    pub request_id: Uuid,
    pub command_type: CommandType,
    pub payload: CommandPayload,
}

impl Validate for CommandStartRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;

        match (&self.command_type, &self.payload) {
            (CommandType::Deploy, CommandPayload::Deploy(payload)) => payload.validate(),
            (CommandType::Run, CommandPayload::Run(payload)) => payload.validate(),
            (CommandType::Stop, CommandPayload::Stop(payload)) => payload.validate(),
            _ => Err(ValidationError::invalid(
                "payload",
                "payload does not match command_type",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Response payload for `command.start`.
pub struct CommandStartResponse {
    pub accepted: bool,
}

impl Validate for CommandStartResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// Command payload union keyed by `command_type`.
pub enum CommandPayload {
    Deploy(DeployCommandPayload),
    Stop(StopCommandPayload),
    Run(RunCommandPayload),
}

impl Validate for CommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Deploy(payload) => payload.validate(),
            Self::Run(payload) => payload.validate(),
            Self::Stop(payload) => payload.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Deploy-specific command payload.
pub struct DeployCommandPayload {
    pub deploy_id: String,
    pub expected_current_release: String,
    pub restart_policy: String,
    #[serde(default = "default_true")]
    pub auto_rollback: bool,
}

impl Validate for DeployCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_required_strings(&[
            (&self.deploy_id, "deploy_id"),
            (&self.expected_current_release, "expected_current_release"),
            (&self.restart_policy, "restart_policy"),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Run-specific command payload.
pub struct RunCommandPayload {
    pub name: String,
}

impl Validate for RunCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "name")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Stop-specific command payload.
pub struct StopCommandPayload {
    pub name: String,
    pub force: bool,
}

impl Validate for StopCommandPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_non_empty(&self.name, "name")
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Event payload emitted during command execution.
///
/// # Examples
/// ```rust
/// use uuid::Uuid;
/// use imago_protocol::{messages::{CommandEvent, CommandEventType, CommandType}, Validate};
///
/// let event = CommandEvent {
///     event_type: CommandEventType::Progress,
///     request_id: Uuid::new_v4(),
///     command_type: CommandType::Deploy,
///     timestamp: "1735689600".to_string(),
///     stage: Some("starting".to_string()),
///     error: None,
/// };
/// event.validate().expect("progress event requires non-empty stage");
/// ```
pub struct CommandEvent {
    pub event_type: CommandEventType,
    pub request_id: Uuid,
    pub command_type: CommandType,
    pub timestamp: String,
    pub stage: Option<String>,
    pub error: Option<StructuredError>,
}

impl Validate for CommandEvent {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        ensure_non_empty(&self.timestamp, "timestamp")?;

        if self.event_type == CommandEventType::Progress {
            let stage = self
                .stage
                .as_deref()
                .ok_or(ValidationError::missing("stage"))?;
            ensure_non_empty(stage, "stage")?;
        }

        if self.event_type == CommandEventType::Failed {
            let err = self
                .error
                .as_ref()
                .ok_or(ValidationError::missing("error"))?;
            err.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Poll request payload for in-flight command state.
pub struct StateRequest {
    pub request_id: Uuid,
}

impl Validate for StateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Poll response payload for in-flight command state.
///
/// # Examples
/// ```rust
/// use uuid::Uuid;
/// use imago_protocol::{messages::{CommandState, StateResponse}, Validate};
///
/// let response = StateResponse {
///     request_id: Uuid::new_v4(),
///     state: CommandState::Running,
///     stage: "running".to_string(),
///     updated_at: "1735689600".to_string(),
/// };
/// response.validate().expect("state.response must stay non-terminal");
/// ```
pub struct StateResponse {
    pub request_id: Uuid,
    pub state: CommandState,
    pub stage: String,
    pub updated_at: String,
}

impl Validate for StateResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")?;
        match self.state {
            CommandState::Accepted | CommandState::Running => {}
            CommandState::Succeeded | CommandState::Failed | CommandState::Canceled => {
                return Err(ValidationError::invalid(
                    "state",
                    "terminal states are not allowed for state.response",
                ));
            }
        }

        ensure_required_strings(&[(&self.stage, "stage"), (&self.updated_at, "updated_at")])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Cancellation request payload keyed by command request id.
pub struct CommandCancelRequest {
    pub request_id: Uuid,
}

impl Validate for CommandCancelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Cancellation response payload.
pub struct CommandCancelResponse {
    pub cancellable: bool,
    pub final_state: CommandState,
}

impl Validate for CommandCancelResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(dead_code)]
    use uuid::Uuid;

    use super::{
        CommandCancelRequest, CommandEvent, CommandEventType, CommandPayload, CommandStartRequest,
        CommandState, CommandType, DeployCommandPayload, RunCommandPayload, StateResponse,
        StopCommandPayload,
    };
    use crate::Validate;

    #[test]
    fn given_command_start_cases__when_validate__then_payload_must_match_command_type() {
        let valid = CommandStartRequest {
            request_id: Uuid::new_v4(),
            command_type: CommandType::Run,
            payload: CommandPayload::Run(RunCommandPayload {
                name: "svc-a".to_string(),
            }),
        };
        valid.validate().expect("run payload should match run type");

        let mismatch = CommandStartRequest {
            request_id: Uuid::new_v4(),
            command_type: CommandType::Run,
            payload: CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "deploy-1".to_string(),
                expected_current_release: "rel-1".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        };
        let err = mismatch
            .validate()
            .expect_err("payload mismatch should fail");
        assert!(
            err.to_string()
                .contains("payload does not match command_type")
        );
    }

    #[test]
    fn given_command_payload_variants__when_validate__then_required_names_are_checked() {
        let invalid_run = RunCommandPayload {
            name: "".to_string(),
        };
        assert!(invalid_run.validate().is_err());

        let invalid_stop = StopCommandPayload {
            name: "".to_string(),
            force: false,
        };
        assert!(invalid_stop.validate().is_err());
    }

    #[test]
    fn given_command_event_cases__when_validate__then_progress_and_failed_require_required_context()
    {
        let progress_missing_stage = CommandEvent {
            event_type: CommandEventType::Progress,
            request_id: Uuid::new_v4(),
            command_type: CommandType::Deploy,
            timestamp: "1735689600".to_string(),
            stage: None,
            error: None,
        };
        assert!(progress_missing_stage.validate().is_err());

        let failed_missing_error = CommandEvent {
            event_type: CommandEventType::Failed,
            request_id: Uuid::new_v4(),
            command_type: CommandType::Deploy,
            timestamp: "1735689600".to_string(),
            stage: Some("failed".to_string()),
            error: None,
        };
        assert!(failed_missing_error.validate().is_err());

        let valid_progress = CommandEvent {
            event_type: CommandEventType::Progress,
            request_id: Uuid::new_v4(),
            command_type: CommandType::Deploy,
            timestamp: "1735689600".to_string(),
            stage: Some("running".to_string()),
            error: None,
        };
        valid_progress
            .validate()
            .expect("progress with stage should pass");
    }

    #[test]
    fn given_state_response_cases__when_validate__then_terminal_states_are_rejected() {
        let terminal_states = [
            CommandState::Succeeded,
            CommandState::Failed,
            CommandState::Canceled,
        ];
        for state in terminal_states {
            let response = StateResponse {
                request_id: Uuid::new_v4(),
                state,
                stage: "done".to_string(),
                updated_at: "1735689600".to_string(),
            };
            assert!(
                response.validate().is_err(),
                "terminal state {state:?} should be rejected"
            );
        }

        let non_terminal = StateResponse {
            request_id: Uuid::new_v4(),
            state: CommandState::Running,
            stage: "running".to_string(),
            updated_at: "1735689600".to_string(),
        };
        non_terminal
            .validate()
            .expect("running state response should pass");
    }

    #[test]
    fn given_cancel_request__when_validate__then_request_id_must_be_non_nil() {
        let invalid = CommandCancelRequest {
            request_id: Uuid::nil(),
        };
        assert!(invalid.validate().is_err());

        let valid = CommandCancelRequest {
            request_id: Uuid::new_v4(),
        };
        valid.validate().expect("non-nil request_id should pass");
    }
}
