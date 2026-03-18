fn main() {
    let target = std::env::var("TARGET").expect("TARGET should be set by cargo");
    println!("cargo:rustc-env=IMAGOD_BUILD_TARGET={target}");

    let mut enabled_features = Vec::new();
    for (feature_env, feature_name) in [
        ("CARGO_FEATURE_WASI_NN_CVITEK", "wasi-nn-cvitek"),
        ("CARGO_FEATURE_WASI_NN_ONNX", "wasi-nn-onnx"),
        ("CARGO_FEATURE_WASI_NN_OPENVINO", "wasi-nn-openvino"),
    ] {
        if std::env::var_os(feature_env).is_some() {
            enabled_features.push(feature_name);
        }
    }
    enabled_features.sort_unstable();
    println!(
        "cargo:rustc-env=IMAGOD_BUILD_FEATURES={}",
        enabled_features.join(",")
    );

    if std::env::var_os("CARGO_FEATURE_WASI_NN_CVITEK").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
    {
        println!("cargo:rustc-link-arg-bin=imagod=-Wl,-rpath,$ORIGIN/lib");
    }
}
