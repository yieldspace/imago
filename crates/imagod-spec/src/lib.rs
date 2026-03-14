pub mod authorization;
pub mod command_contract;
pub mod envelope;
pub mod error;
pub mod identity;
pub mod ipc;
pub mod manager;
pub mod messages;
pub mod operation;
pub mod rpc;
pub mod service;
pub mod system;
pub mod validate;
pub mod wire;

pub use authorization::{
    AuthorizationDecision, AuthorizationDenialReason, BindingGrantId, ExternalMessage, InterfaceId,
    OperationPermission, SessionRequestState,
};
pub use error::*;
pub use identity::{
    RemoteAuthorityId, ServiceId, SessionAuthState, SessionId, SessionRole, TransportPrincipal,
};
pub use ipc::*;
pub use manager::{MaintenancePhase, ManagerPhase, ManagerShutdownPhase};
pub use messages::MessageType;
pub use operation::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandTerminalState, ManagerAuthState,
};
pub use rpc::RpcOutcome;
pub use service::ServiceLifecyclePhase;
pub use system::{SystemEvent, SystemStateFragment};
pub use validate::*;
pub use wire::*;

#[cfg(test)]
pub(crate) type CborError = String;

#[cfg(test)]
pub(crate) fn to_cbor<T>(value: &T) -> Result<Vec<u8>, CborError>
where
    T: serde::Serialize,
{
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).map_err(|err| err.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) fn from_cbor<T>(bytes: &[u8]) -> Result<T, CborError>
where
    T: serde::de::DeserializeOwned,
{
    ciborium::de::from_reader(bytes).map_err(|err| err.to_string())
}
