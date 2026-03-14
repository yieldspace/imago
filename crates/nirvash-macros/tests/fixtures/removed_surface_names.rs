use nirvash_macros::{
    code_witness_test_main, code_witness_tests, contract_case, nirvash_projection_contract,
    nirvash_runtime_contract,
};

struct LegacyRuntime;
struct LegacyProjection;

#[code_witness_tests]
const _: () = ();

#[nirvash_runtime_contract]
impl LegacyRuntime {}

#[nirvash_projection_contract]
impl LegacyProjection {}

#[contract_case]
fn legacy_case() {}

code_witness_test_main!();

fn main() {}
