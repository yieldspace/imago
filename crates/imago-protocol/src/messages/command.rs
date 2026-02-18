use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::StructuredError;
use crate::validate::{
    Validate, ValidationError, ensure_non_empty, ensure_required_strings, ensure_uuid_not_nil,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandType {
    #[serde(rename = "deploy")]
    Deploy,
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "stop")]
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct StateRequest {
    pub request_id: Uuid,
}

impl Validate for StateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct CommandCancelRequest {
    pub request_id: Uuid,
}

impl Validate for CommandCancelRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        ensure_uuid_not_nil(&self.request_id, "request_id")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCancelResponse {
    pub cancellable: bool,
    pub final_state: CommandState,
}

impl Validate for CommandCancelResponse {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}
