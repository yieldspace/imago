fn main() {
    if std::env::var_os("NIRVASH_DOCGEN_SKIP").is_some() {
        return;
    }
    nirvash_docgen::generate().expect("failed to generate nirvash metamodel docs");
}
