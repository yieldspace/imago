#[test]
fn signature_derive_behaves_for_supported_inputs() {
    let t = trybuild::TestCases::new();
    t.pass("tests/fixtures/derive_signature_ok.rs");
    t.pass("tests/fixtures/subsystem_spec_ok.rs");
    t.compile_fail("tests/fixtures/subsystem_spec_invalid_symmetry.rs");
    t.compile_fail("tests/fixtures/derive_signature_invalid_range.rs");
    t.compile_fail("tests/fixtures/derive_signature_custom_missing_impl.rs");
    t.compile_fail("tests/fixtures/derive_signature_legacy_attrs.rs");
    t.compile_fail("tests/fixtures/derive_signature_custom_with_range.rs");
}
