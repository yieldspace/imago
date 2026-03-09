use imagod_control::OperationManager;
use imagod_spec::{CommandProtocolAction as RuntimeCommandProtocolAction, CommandProtocolContext};
use imagod_spec_formal::{CommandProjectionSpec, CommandProtocolAction, SystemState};
use nirvash_core::conformance::{
    ActionApplier, NegativeWitness, PositiveWitness, ProtocolInputWitnessBinding,
    ProtocolRuntimeBinding,
};
use nirvash_macros::{code_witness_test_main, code_witness_tests};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy)]
struct CommandProtocolBinding;

#[derive(Debug, Clone, Copy)]
struct CommandProtocolWitnessSession {
    principal_context: CommandProtocolContext,
    probe_context: CommandProtocolContext,
}

impl ProtocolRuntimeBinding<CommandProjectionSpec> for CommandProtocolBinding {
    type Runtime = OperationManager;
    type Context = CommandProtocolContext;

    async fn fresh_runtime(_spec: &CommandProjectionSpec) -> Self::Runtime {
        OperationManager::new()
    }

    fn context(_spec: &CommandProjectionSpec) -> Self::Context {
        CommandProtocolContext {
            request_id: Uuid::from_u128(1),
        }
    }
}

impl ProtocolInputWitnessBinding<CommandProjectionSpec> for CommandProtocolBinding {
    type Input = RuntimeCommandProtocolAction;
    type Session = CommandProtocolWitnessSession;

    async fn fresh_session(_spec: &CommandProjectionSpec) -> Self::Session {
        let context = CommandProtocolContext {
            request_id: Uuid::from_u128(1),
        };
        CommandProtocolWitnessSession {
            principal_context: context,
            probe_context: context,
        }
    }

    fn positive_witnesses(
        _spec: &CommandProjectionSpec,
        session: &Self::Session,
        _prev: &SystemState,
        action: &CommandProtocolAction,
        _next: &SystemState,
    ) -> Vec<PositiveWitness<Self::Context, Self::Input>> {
        vec![
            PositiveWitness::new(
                "principal",
                session.principal_context,
                action.clone(),
            )
            .with_canonical(true),
        ]
    }

    fn negative_witnesses(
        _spec: &CommandProjectionSpec,
        session: &Self::Session,
        _prev: &SystemState,
        action: &CommandProtocolAction,
    ) -> Vec<NegativeWitness<Self::Context, Self::Input>> {
        vec![NegativeWitness::new(
            "principal",
            session.principal_context,
            action.clone(),
        )]
    }

    async fn execute_input(
        runtime: &Self::Runtime,
        _session: &mut Self::Session,
        context: &Self::Context,
        input: &Self::Input,
    ) -> <CommandProjectionSpec as nirvash_core::conformance::ProtocolConformanceSpec>::ObservedOutput
    {
        runtime.execute_action(context, input).await
    }

    fn probe_context(session: &Self::Session) -> Self::Context {
        session.probe_context
    }
}

#[code_witness_tests(spec = CommandProjectionSpec, binding = CommandProtocolBinding)]
const _: () = ();

code_witness_test_main!();
