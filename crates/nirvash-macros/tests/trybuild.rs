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
    t.pass("tests/fixtures/code_tests_ok.rs");
    t.pass("tests/fixtures/code_witness_tests_ok.rs");
    t.compile_fail("tests/fixtures/attribute_missing_target.rs");
    t.compile_fail("tests/fixtures/attribute_wrong_type.rs");
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
    t.compile_fail("tests/fixtures/derive_relational_state_invalid.rs");
    unsafe {
        std::env::remove_var("NIRVASH_DOC_FRAGMENT_SPEC");
    }
}
