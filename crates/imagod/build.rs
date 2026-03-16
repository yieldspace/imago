fn main() {
    if std::env::var_os("CARGO_FEATURE_WASI_NN_CVITEK").is_none() {
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    println!("cargo:rustc-link-arg-bin=imagod=-Wl,-rpath,$ORIGIN/lib");
}
