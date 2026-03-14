//! Canonical authorization model shared by the system spec and adapter layers.

use nirvash_macros::{
    ActionVocabulary, FiniteModelDomain as FormalFiniteModelDomain, RelAtom,
    SymbolicEncoding as FormalSymbolicEncoding,
};

use crate::{
    identity::{SessionAuthState, SessionRole, TransportPrincipal},
    operation::ManagerAuthState,
    service::ServiceLifecyclePhase,
};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    ActionVocabulary,
)]
pub enum ExternalMessage {
    HelloNegotiate,
    DeployPrepare,
    ArtifactPush,
    ArtifactCommit,
    CommandStart,
    ServicesList,
    StateRequest,
    CommandCancel,
    LogsRequest,
    RpcInvoke,
    BindingsCertUpload,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum OperationPermission {
    CancelCommand,
    ResolveInvocationTarget,
    LocalRpcInvoke,
    RemoteConnect,
    RemoteInvoke,
    RemoteDisconnect,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum InterfaceId {
    ControlApi,
    LogsApi,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
    RelAtom,
)]
pub enum BindingGrantId {
    Service0ToService1ControlApi,
    Service0ToService1LogsApi,
    Service1ToService0ControlApi,
    Service1ToService0LogsApi,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum AuthorizationDenialReason {
    UnknownPrincipal,
    SessionNotAuthenticated,
    SessionDrained,
    MessageNotAllowed,
    ManagerNotListening,
    ManagerAuthMissing,
    BindingMissing,
    RemoteAuthorityUnknown,
    RemoteConnectionMissing,
    TargetServiceNotRunning,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum AuthorizationDecision {
    NotEvaluated,
    Allow,
    DenyUnknownPrincipal,
    DenySessionNotAuthenticated,
    DenySessionDrained,
    DenyMessageNotAllowed,
    DenyManagerNotListening,
    DenyManagerAuthMissing,
    DenyBindingMissing,
    DenyRemoteAuthorityUnknown,
    DenyRemoteConnectionMissing,
    DenyTargetServiceNotRunning,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    FormalFiniteModelDomain,
    FormalSymbolicEncoding,
)]
pub enum SessionRequestState {
    Idle,
    Pending,
    Completed,
    Denied,
}

impl SessionRequestState {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Pending)
    }
}

impl AuthorizationDecision {
    pub const fn denial_reason(self) -> Option<AuthorizationDenialReason> {
        match self {
            Self::NotEvaluated | Self::Allow => None,
            Self::DenyUnknownPrincipal => Some(AuthorizationDenialReason::UnknownPrincipal),
            Self::DenySessionNotAuthenticated => {
                Some(AuthorizationDenialReason::SessionNotAuthenticated)
            }
            Self::DenySessionDrained => Some(AuthorizationDenialReason::SessionDrained),
            Self::DenyMessageNotAllowed => Some(AuthorizationDenialReason::MessageNotAllowed),
            Self::DenyManagerNotListening => Some(AuthorizationDenialReason::ManagerNotListening),
            Self::DenyManagerAuthMissing => Some(AuthorizationDenialReason::ManagerAuthMissing),
            Self::DenyBindingMissing => Some(AuthorizationDenialReason::BindingMissing),
            Self::DenyRemoteAuthorityUnknown => {
                Some(AuthorizationDenialReason::RemoteAuthorityUnknown)
            }
            Self::DenyRemoteConnectionMissing => {
                Some(AuthorizationDenialReason::RemoteConnectionMissing)
            }
            Self::DenyTargetServiceNotRunning => {
                Some(AuthorizationDenialReason::TargetServiceNotRunning)
            }
        }
    }
}

pub const fn message_authorization_decision(
    principal: TransportPrincipal,
    auth_state: SessionAuthState,
    role: SessionRole,
    manager_accepts_control: bool,
    message: ExternalMessage,
) -> AuthorizationDecision {
    if matches!(auth_state, SessionAuthState::Drained) {
        return AuthorizationDecision::DenySessionDrained;
    }

    if matches!(message, ExternalMessage::HelloNegotiate) {
        return match principal {
            TransportPrincipal::Unknown => AuthorizationDecision::DenyUnknownPrincipal,
            _ => AuthorizationDecision::Allow,
        };
    }

    if !manager_accepts_control {
        return AuthorizationDecision::DenyManagerNotListening;
    }

    if !matches!(auth_state, SessionAuthState::Authenticated) {
        return AuthorizationDecision::DenySessionNotAuthenticated;
    }

    match role {
        SessionRole::Admin => AuthorizationDecision::Allow,
        SessionRole::Client => {
            if matches!(message, ExternalMessage::RpcInvoke) {
                AuthorizationDecision::Allow
            } else {
                AuthorizationDecision::DenyMessageNotAllowed
            }
        }
        SessionRole::ServiceRunner => AuthorizationDecision::DenyMessageNotAllowed,
        SessionRole::Unknown => AuthorizationDecision::DenyUnknownPrincipal,
    }
}

pub const fn operation_authorization_decision(
    permission: OperationPermission,
    manager_auth: ManagerAuthState,
    binding_present: bool,
    authority_trusted: bool,
    connection_present: bool,
    target_lifecycle: ServiceLifecyclePhase,
) -> AuthorizationDecision {
    if !matches!(manager_auth, ManagerAuthState::Verified) {
        return AuthorizationDecision::DenyManagerAuthMissing;
    }

    match permission {
        OperationPermission::CancelCommand => AuthorizationDecision::Allow,
        OperationPermission::ResolveInvocationTarget | OperationPermission::LocalRpcInvoke => {
            if !binding_present {
                return AuthorizationDecision::DenyBindingMissing;
            }
            if !target_lifecycle.is_running() {
                return AuthorizationDecision::DenyTargetServiceNotRunning;
            }
            AuthorizationDecision::Allow
        }
        OperationPermission::RemoteConnect => {
            if authority_trusted {
                AuthorizationDecision::Allow
            } else {
                AuthorizationDecision::DenyRemoteAuthorityUnknown
            }
        }
        OperationPermission::RemoteInvoke => {
            if !binding_present {
                return AuthorizationDecision::DenyBindingMissing;
            }
            if !authority_trusted {
                return AuthorizationDecision::DenyRemoteAuthorityUnknown;
            }
            if !connection_present {
                return AuthorizationDecision::DenyRemoteConnectionMissing;
            }
            if !target_lifecycle.is_running() {
                return AuthorizationDecision::DenyTargetServiceNotRunning;
            }
            AuthorizationDecision::Allow
        }
        OperationPermission::RemoteDisconnect => {
            if connection_present {
                AuthorizationDecision::Allow
            } else {
                AuthorizationDecision::DenyRemoteConnectionMissing
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_only_gets_rpc_after_authentication() {
        assert_eq!(
            message_authorization_decision(
                TransportPrincipal::Client,
                SessionAuthState::Accepted,
                SessionRole::Client,
                true,
                ExternalMessage::RpcInvoke,
            ),
            AuthorizationDecision::DenySessionNotAuthenticated
        );
        assert_eq!(
            message_authorization_decision(
                TransportPrincipal::Client,
                SessionAuthState::Authenticated,
                SessionRole::Client,
                true,
                ExternalMessage::RpcInvoke,
            ),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            message_authorization_decision(
                TransportPrincipal::Client,
                SessionAuthState::Authenticated,
                SessionRole::Client,
                true,
                ExternalMessage::CommandStart,
            ),
            AuthorizationDecision::DenyMessageNotAllowed
        );
    }

    #[test]
    fn remote_invoke_requires_binding_authority_and_running_target() {
        assert_eq!(
            operation_authorization_decision(
                OperationPermission::RemoteInvoke,
                ManagerAuthState::Verified,
                true,
                true,
                true,
                ServiceLifecyclePhase::Running,
            ),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            operation_authorization_decision(
                OperationPermission::RemoteInvoke,
                ManagerAuthState::Verified,
                false,
                true,
                true,
                ServiceLifecyclePhase::Running,
            ),
            AuthorizationDecision::DenyBindingMissing
        );
        assert_eq!(
            operation_authorization_decision(
                OperationPermission::RemoteInvoke,
                ManagerAuthState::Verified,
                true,
                false,
                false,
                ServiceLifecyclePhase::Running,
            ),
            AuthorizationDecision::DenyRemoteAuthorityUnknown
        );
    }
}
