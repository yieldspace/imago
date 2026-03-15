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
    t.pass("tests/fixtures/system_spec_type_paths_ok.rs");
    t.pass("tests/fixtures/case_scoped_constraints_ok.rs");
    t.pass("tests/fixtures/code_tests_import_first_ok.rs");
    t.pass("tests/fixtures/code_tests_import_block_ok.rs");
    t.pass("tests/fixtures/code_tests_import_nested_module_ok.rs");
    t.pass("tests/fixtures/code_tests_import_profiles_ok.rs");
    t.pass("tests/fixtures/code_tests_export_alias_ok.rs");
    t.pass("tests/fixtures/code_tests_payload_finite_domain_ok.rs");
    t.pass("tests/fixtures/code_tests_projection_override_ok.rs");
    t.pass("tests/fixtures/code_tests_seed_override_ok.rs");
    t.pass("tests/fixtures/code_tests_strategy_only_seed_ok.rs");
    t.pass("tests/fixtures/code_tests_trace_binding_ok.rs");
    t.pass("tests/fixtures/code_tests_concurrency_marker_ok.rs");
    t.pass("tests/fixtures/code_tests_nonclone_multi_binding_ok.rs");
    t.pass("tests/fixtures/code_tests_custom_fixture_nonserializable_ok.rs");
    t.pass("tests/fixtures/code_tests_low_level_nested_installer_ok.rs");
    t.pass("tests/fixtures/code_tests_same_tail_specs_ok.rs");
    t.pass("tests/fixtures/code_tests_kani_harness_ok.rs");
    t.pass("tests/fixtures/derive_protocol_input_witness_ok.rs");
    t.pass("tests/fixtures/function_like_bool_macros_ok.rs");
    t.pass("tests/fixtures/function_like_transition_program_ok.rs");
    t.compile_fail("tests/fixtures/attribute_missing_target.rs");
    t.compile_fail("tests/fixtures/attribute_wrong_type.rs");
    t.compile_fail("tests/fixtures/case_scoped_constraints_invalid_option.rs");
    t.compile_fail("tests/fixtures/case_scoped_constraints_duplicate_labels.rs");
    t.compile_fail("tests/fixtures/removed_surface_names.rs");
    t.compile_fail("tests/fixtures/code_tests_legacy_args.rs");
    t.compile_fail("tests/fixtures/code_tests_missing_oracle.rs");
    t.compile_fail("tests/fixtures/code_tests_missing_projection.rs");
    t.compile_fail("tests/fixtures/code_tests_missing_trace_binding.rs");
    t.compile_fail("tests/fixtures/code_tests_missing_concurrency_marker.rs");
    t.compile_fail("tests/fixtures/old_macro_names.rs");
    t.compile_fail("tests/fixtures/subsystem_spec_invalid_symmetry.rs");
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
    t.compile_fail("tests/fixtures/function_like_bool_macros_invalid.rs");
    t.compile_fail("tests/fixtures/function_like_transition_program_invalid.rs");
    t.compile_fail("tests/fixtures/registered_invariant_closure_reject.rs");
    t.compile_fail("tests/fixtures/registered_transition_program_closure_reject.rs");
    t.compile_fail("tests/fixtures/registered_property_closure_reject.rs");
    unsafe {
        std::env::remove_var("NIRVASH_DOC_FRAGMENT_SPEC");
    }
}
