use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use imago_schema_gen::generate_all;

fn main() {
    println!("cargo:rerun-if-changed=crates/imago-project-config/src/lib.rs");
    println!("cargo:rerun-if-changed=crates/imago-schema-gen/src/lib.rs");
    println!("cargo:rerun-if-changed=crates/imagod-config/src/lib.rs");
    println!("cargo:rerun-if-changed=crates/imago-project-config/Cargo.toml");
    println!("cargo:rerun-if-changed=crates/imago-schema-gen/Cargo.toml");
    println!("cargo:rerun-if-changed=crates/imagod-config/Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is missing"));
    generate_all(&manifest_dir).expect("failed to generate JSON schema files");

    println!("cargo:rerun-if-changed=e2e/Cargo.toml");
    println!("cargo:rerun-if-changed=e2e/src/bin");
    println!("cargo:rerun-if-changed=e2e/wit");
    println!("cargo:rerun-if-changed=e2e/imago.lock");

    if env::var_os("CARGO_FEATURE_E2E").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is missing"));
    let target_dir = out_dir.join("e2e-target");
    fs::create_dir_all(&target_dir).expect("failed to create build script target dir");

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .arg("build")
        .arg("-p")
        .arg("e2e")
        .arg("--target")
        .arg("wasm32-wasip2")
        .arg("--bins")
        .arg("-F")
        .arg("e2e")
        .arg("--target-dir")
        .arg(&target_dir)
        .current_dir(&manifest_dir)
        .output()
        .expect("failed to run cargo build for e2e artifacts");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "failed to build e2e wasm artifacts: status={}\nstdout:\n{}\nstderr:\n{}",
            output.status, stdout, stderr
        );
    }

    let wasm_dir = target_dir.join("wasm32-wasip2").join("debug");
    export_artifact(
        "IMAGO_E2E_WASM_CLI_BASE",
        &wasm_dir.join("e2e_cli_base.wasm"),
    );
    export_artifact(
        "IMAGO_E2E_WASM_CLI_FAIL",
        &wasm_dir.join("e2e_cli_fail.wasm"),
    );
    export_artifact("IMAGO_E2E_WASM_HTTP", &wasm_dir.join("e2e_http.wasm"));
    export_artifact(
        "IMAGO_E2E_WASM_HTTP_SLOW",
        &wasm_dir.join("e2e_http_slow.wasm"),
    );
    export_artifact(
        "IMAGO_E2E_WASM_RPC_CALLER",
        &wasm_dir.join("e2e_rpc_caller.wasm"),
    );
    export_artifact(
        "IMAGO_E2E_WASM_RPC_CALLEE",
        &wasm_dir.join("e2e_rpc_callee.wasm"),
    );
    export_artifact(
        "IMAGO_E2E_WASM_HTTP_OUTBOUND_CLI",
        &wasm_dir.join("e2e_http_outbound_cli.wasm"),
    );
}

fn export_artifact(key: &str, path: &Path) {
    if !path.is_file() {
        panic!("missing e2e wasm artifact for {key}: {}", path.display());
    }
    println!("cargo:rustc-env={key}={}", path.display());
}
