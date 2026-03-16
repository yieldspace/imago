fn main() {
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_LINK_MODE");

    if std::env::var_os("CARGO_FEATURE_WASI_NN_CVITEK").is_none() {
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    if std::env::var("IMAGO_CVITEK_LINK_MODE").as_deref() == Ok("dynamic") {
        println!("cargo:rustc-link-arg-bin=imagod=-Wl,-rpath,$ORIGIN/lib");
    }
}
