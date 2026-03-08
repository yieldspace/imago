use imago_protocol::CommandProtocolContext;
use imagod_control::OperationManager;
use imagod_spec::command_protocol::CommandProtocolSpec;
use nirvash_core::conformance::ProtocolRuntimeBinding;
use nirvash_macros::code_tests;
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy)]
struct CommandProtocolBinding;

impl ProtocolRuntimeBinding<CommandProtocolSpec> for CommandProtocolBinding {
    type Runtime = OperationManager;
    type Context = CommandProtocolContext;

    async fn fresh_runtime(_spec: &CommandProtocolSpec) -> Self::Runtime {
        OperationManager::new()
    }

    fn context(_spec: &CommandProtocolSpec) -> Self::Context {
        CommandProtocolContext {
            request_id: Uuid::from_u128(1),
        }
    }
}

#[code_tests(spec = CommandProtocolSpec, binding = CommandProtocolBinding)]
const _: () = ();
