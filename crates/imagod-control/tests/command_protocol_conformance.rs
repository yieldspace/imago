use imagod_control::OperationManager;
use imagod_spec::{CommandProtocolAction as RuntimeCommandProtocolAction, CommandProtocolContext};
use imagod_spec_formal::CommandProjectionSpec;
use nirvash_macros::{code_witness_test_main, nirvash_runtime_contract};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy)]
struct CommandProtocolBinding;

#[nirvash_runtime_contract(
    spec = CommandProjectionSpec,
    binding = CommandProtocolBinding,
    runtime = OperationManager,
    context = CommandProtocolContext,
    context_expr = CommandProtocolContext {
        request_id: Uuid::from_u128(1),
    },
    fresh_runtime = OperationManager::new(),
    input = RuntimeCommandProtocolAction,
    tests(witness)
)]
impl CommandProtocolBinding {}

code_witness_test_main!();
