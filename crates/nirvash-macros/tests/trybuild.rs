#[test]
fn signature_derive_behaves_for_supported_inputs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fragment_path = temp.path().join("Spec.md");
    std::fs::write(
        &fragment_path,
        "## Meta Model\n\n<svg xmlns=\"http://www.w3.org/2000/svg\"><text>doc-enabled</text></svg>\n",
    )
    .expect("write fragment");
    unsafe {
        std::env::set_var("NIRVASH_DOC_FRAGMENT_SPEC", &fragment_path);
    }
    let t = trybuild::TestCases::new();
    t.pass("tests/fixtures/derive_signature_ok.rs");
    t.pass("tests/fixtures/derive_action_vocabulary_ok.rs");
    t.pass("tests/fixtures/derive_rel_atom_ok.rs");
    t.pass("tests/fixtures/derive_relational_state_ok.rs");
    t.pass("tests/fixtures/subsystem_spec_ok.rs");
    t.pass("tests/fixtures/case_scoped_constraints_ok.rs");
    t.pass("tests/fixtures/code_tests_ok.rs");
    t.pass("tests/fixtures/code_witness_tests_ok.rs");
    t.pass("tests/fixtures/runtime_contract_grouped_ok.rs");
    t.pass("tests/fixtures/runtime_contract_binding_witness_ok.rs");
    t.pass("tests/fixtures/runtime_contract_runtime_witness_ok.rs");
    t.pass("tests/fixtures/runtime_contract_input_codec_ok.rs");
    t.pass("tests/fixtures/projection_contract_ok.rs");
    t.pass("tests/fixtures/projection_model_ok.rs");
    t.pass("tests/fixtures/projection_model_exhaustive_ok.rs");
    t.pass("tests/fixtures/derive_protocol_input_witness_ok.rs");
    t.compile_fail("tests/fixtures/attribute_missing_target.rs");
    t.compile_fail("tests/fixtures/attribute_wrong_type.rs");
    t.compile_fail("tests/fixtures/case_scoped_constraints_invalid_option.rs");
    t.compile_fail("tests/fixtures/case_scoped_constraints_duplicate_labels.rs");
    t.compile_fail("tests/fixtures/runtime_contract_duplicate_action.rs");
    t.compile_fail("tests/fixtures/runtime_contract_legacy_summary_args.rs");
    t.compile_fail("tests/fixtures/runtime_contract_witness_missing_dispatch.rs");
    t.compile_fail("tests/fixtures/runtime_contract_input_codec_invalid.rs");
    t.compile_fail("tests/fixtures/old_macro_names.rs");
    t.compile_fail("tests/fixtures/subsystem_spec_invalid_symmetry.rs");
    t.compile_fail("tests/fixtures/code_witness_tests_missing_main.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_action.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_driver.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_fresh.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_context.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_harness.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_probe.rs");
    t.compile_fail("tests/fixtures/derive_signature_invalid_range.rs");
    t.compile_fail("tests/fixtures/derive_signature_custom_missing_impl.rs");
    t.compile_fail("tests/fixtures/derive_signature_legacy_attrs.rs");
    t.compile_fail("tests/fixtures/derive_signature_custom_with_range.rs");
    t.compile_fail("tests/fixtures/derive_signature_custom_with_bounds.rs");
    t.compile_fail("tests/fixtures/derive_signature_invalid_len.rs");
    t.compile_fail("tests/fixtures/derive_signature_invalid_filter.rs");
    t.compile_fail("tests/fixtures/derive_action_vocabulary_invalid.rs");
    t.compile_fail("tests/fixtures/derive_protocol_input_witness_invalid.rs");
    t.compile_fail("tests/fixtures/derive_relational_state_invalid.rs");
    t.compile_fail("tests/fixtures/projection_model_invalid_pattern.rs");
    unsafe {
        std::env::remove_var("NIRVASH_DOC_FRAGMENT_SPEC");
    }
}
