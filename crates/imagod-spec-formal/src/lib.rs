pub mod artifact_deploy;
pub mod atoms;
pub mod bounds;
pub mod command_projection;
pub mod command_protocol;
pub mod deploy;
pub mod logs_projection;
pub mod manager_runtime;
pub mod manager_runtime_projection;
pub mod plugin_platform;
pub mod router_projection;
pub mod rpc;
pub mod runner_bootstrap;
pub mod runner_runtime;
pub mod runtime_projection;
pub mod service_supervision;
pub mod session_auth;
pub mod session_auth_projection;
pub mod session_transport;
pub mod shutdown_flow;
mod state_domain;
mod summary_mapping;
pub mod supervision;
mod symbolic_registration;
pub mod system;
pub mod wire_protocol;

#[cfg(test)]
mod toy_model_controls;

#[cfg(test)]
mod symbolic_registration_tests {
    use std::collections::BTreeSet;

    use nirvash::{
        TransitionSystem,
        registry::{registered_symbolic_effect_keys, registered_symbolic_pure_helper_keys},
    };

    use super::{
        PluginPlatformSpec, RpcSpec, SupervisionSpec, SystemSpec,
        artifact_deploy::ArtifactDeploySpec, command_protocol::CommandProtocolSpec,
        deploy::DeploySpec, manager_runtime::ManagerRuntimeSpec,
        runner_bootstrap::RunnerBootstrapSpec, runner_runtime::RunnerRuntimeSpec,
        service_supervision::ServiceSupervisionSpec, session_auth::SessionAuthSpec,
        session_transport::SessionTransportSpec, shutdown_flow::ShutdownFlowSpec,
        wire_protocol::WireProtocolSpec,
    };

    macro_rules! collect_program_keys {
        ($used_pure:expr, $used_effects:expr, $missing_pure:expr, $missing_effects:expr, $spec:expr) => {{
            let program = $spec
                .transition_program()
                .expect("production formal spec should expose transition_program()");
            $used_pure.extend(program.symbolic_pure_helper_keys());
            $used_effects.extend(program.symbolic_effect_keys());
            $missing_pure.extend(program.unregistered_symbolic_pure_helper_keys());
            $missing_effects.extend(program.unregistered_symbolic_effect_keys());
        }};
    }

    #[test]
    fn shared_symbolic_registration_matches_transition_program_usage() {
        let mut used_pure = BTreeSet::new();
        let mut used_effects = BTreeSet::new();
        let mut missing_pure = BTreeSet::new();
        let mut missing_effects = BTreeSet::new();

        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            ArtifactDeploySpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            CommandProtocolSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            DeploySpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            ManagerRuntimeSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            PluginPlatformSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            RpcSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            RunnerBootstrapSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            RunnerRuntimeSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            ServiceSupervisionSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            SessionAuthSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            SessionTransportSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            ShutdownFlowSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            SupervisionSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            SystemSpec::new()
        );
        collect_program_keys!(
            used_pure,
            used_effects,
            missing_pure,
            missing_effects,
            WireProtocolSpec::new()
        );

        assert!(
            missing_pure.is_empty(),
            "missing pure helper registrations: {missing_pure:?}"
        );
        assert!(
            missing_effects.is_empty(),
            "missing effect registrations: {missing_effects:?}"
        );
        assert_eq!(
            used_pure.into_iter().collect::<Vec<_>>(),
            registered_symbolic_pure_helper_keys()
        );
        assert_eq!(
            used_effects.into_iter().collect::<Vec<_>>(),
            registered_symbolic_effect_keys()
        );
    }
}

pub use command_projection::CommandProjectionSpec;
pub use deploy::{DeployAction, DeploySpec, DeployState};
pub use imagod_spec::{
    CommandErrorKind, CommandKind, CommandLifecycleState, CommandProtocolAction,
    CommandProtocolStageId, LogsStateSummary, ManagerRuntimeStateSummary, OperationPhase,
    PluginKind, RouterStateSummary, RunnerAppType, RuntimeStateSummary, SessionAuthStateSummary,
};
pub use logs_projection::{LogsProjectionAction, LogsProjectionSpec};
pub use manager_runtime::{
    ManagerRuntimeAction, ManagerRuntimePhase, ManagerRuntimeSpec, ManagerRuntimeState,
};
pub use manager_runtime_projection::{
    ManagerRuntimeProjectionAction, ManagerRuntimeProjectionSpec,
};
pub use plugin_platform::{PluginPlatformAction, PluginPlatformSpec, PluginPlatformState};
pub use router_projection::{RouterProjectionAction, RouterProjectionSpec};
pub use rpc::{RpcAction, RpcSpec, RpcState};
pub use runtime_projection::{RuntimeProjectionAction, RuntimeProjectionSpec};
pub use session_auth_projection::{SessionAuthProjectionAction, SessionAuthProjectionSpec};
pub use supervision::{SupervisionAction, SupervisionSpec, SupervisionState};
pub use system::{
    SystemAction, SystemEffect, SystemMessageBinding, SystemSpec, SystemState,
    system_message_binding,
};
