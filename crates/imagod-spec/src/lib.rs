pub mod command_contract;
pub mod envelope;
pub mod error;
pub mod ipc;
pub mod messages;
pub mod validate;
pub mod wire;

pub use command_contract::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction,
    CommandProtocolContext, CommandProtocolObservedState, CommandProtocolOutput,
    CommandProtocolStageId, OperationPhase,
};
pub use ipc::*;
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
