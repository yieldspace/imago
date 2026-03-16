use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_SDK_URL: &str = "https://codeload.github.com/milkv-duo/tpu-sdk-sg200x/tar.gz/6fa0d80a635db13b6b9dc061d68b8da0593b79f3";
const DEFAULT_SDK_SHA256: &str = "08fa6715fdd48db370b6b945c58410c608101292deee710200b85501085bde8b";
const DEFAULT_MUSL_GXX: &str = "riscv64-unknown-linux-musl-g++";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkMode {
    Static,
    Dynamic,
}

fn main() {
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=CVI_TPU_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_LIB_DIR");
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_LINK_MODE");
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_MUSL_GXX");
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_SDK_URL");
    println!("cargo:rerun-if-env-changed=IMAGO_CVITEK_SDK_SHA256");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_arch != "riscv64" || target_os != "linux" {
        return;
    }

    let sdk_root = resolve_sdk_root().unwrap_or_else(|message| panic!("{}", message));
    let lib_dir = env::var("IMAGO_CVITEK_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| sdk_root.join("lib"));
    let link_mode = resolve_link_mode(&target_env).unwrap_or_else(|message| panic!("{}", message));
    ensure_required_libraries(&lib_dir, link_mode).unwrap_or_else(|message| panic!("{}", message));
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    if link_mode == LinkMode::Static && target_env == "musl" {
        add_musl_cpp_runtime_search_paths().unwrap_or_else(|message| panic!("{}", message));
    }

    match link_mode {
        LinkMode::Dynamic => {
            for lib in ["cviruntime", "cvikernel", "cvimath", "z"] {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
        LinkMode::Static => {
            for lib in [
                "cviruntime-static",
                "cvikernel-static",
                "cvimath-static",
                "z",
            ] {
                println!("cargo:rustc-link-lib=static={lib}");
            }
            if target_env == "musl" {
                for lib in ["stdc++", "gcc_eh", "atomic"] {
                    println!("cargo:rustc-link-lib=static={lib}");
                }
            } else {
                println!("cargo:rustc-link-lib=dylib=stdc++");
                println!("cargo:rustc-link-lib=dylib=atomic");
            }
            for lib in ["m", "pthread", "dl"] {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
    }
}

fn resolve_link_mode(target_env: &str) -> Result<LinkMode, String> {
    match env::var("IMAGO_CVITEK_LINK_MODE") {
        Ok(value) => match value.as_str() {
            "static" => Ok(LinkMode::Static),
            "dynamic" => Ok(LinkMode::Dynamic),
            other => Err(format!(
                "unsupported IMAGO_CVITEK_LINK_MODE '{other}'; expected 'static' or 'dynamic'"
            )),
        },
        Err(_) => {
            if target_env == "musl" && resolve_program_path(&musl_gxx_program()).is_none() {
                println!(
                    "cargo:warning=falling back to IMAGO_CVITEK_LINK_MODE=dynamic because {} was not found in PATH",
                    musl_gxx_program()
                );
                Ok(LinkMode::Dynamic)
            } else {
                Ok(LinkMode::Static)
            }
        }
    }
}

fn resolve_sdk_root() -> Result<PathBuf, String> {
    if let Ok(value) = env::var("IMAGO_CVITEK_SDK_ROOT") {
        return Ok(PathBuf::from(value));
    }
    if let Ok(value) = env::var("CVI_TPU_SDK_ROOT") {
        return Ok(PathBuf::from(value));
    }
    download_sdk()
}

fn ensure_required_libraries(lib_dir: &Path, link_mode: LinkMode) -> Result<(), String> {
    if !lib_dir.is_dir() {
        return Err(format!(
            "wasi-nn-cvitek library directory does not exist: {}",
            lib_dir.display()
        ));
    }

    match link_mode {
        LinkMode::Dynamic => {
            for lib in ["libcviruntime.so", "libcvikernel.so", "libcvimath.so"] {
                let candidate = lib_dir.join(lib);
                if !candidate.is_file() {
                    return Err(format!(
                        "wasi-nn-cvitek dynamic library is missing: {}",
                        candidate.display()
                    ));
                }
            }
        }
        LinkMode::Static => {
            for lib in [
                "libcviruntime-static.a",
                "libcvikernel-static.a",
                "libcvimath-static.a",
            ] {
                let candidate = lib_dir.join(lib);
                if !candidate.is_file() {
                    return Err(format!(
                        "wasi-nn-cvitek static archive is missing: {}",
                        candidate.display()
                    ));
                }
            }
        }
    }

    Ok(())
}

fn download_sdk() -> Result<PathBuf, String> {
    let sdk_url = env::var("IMAGO_CVITEK_SDK_URL").unwrap_or_else(|_| DEFAULT_SDK_URL.to_owned());
    let expected_sha =
        env::var("IMAGO_CVITEK_SDK_SHA256").unwrap_or_else(|_| DEFAULT_SDK_SHA256.to_owned());
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|err| err.to_string())?);
    let cache_dir = out_dir.join("cvitek-sdk").join(&expected_sha);
    let tarball_path = cache_dir.join("sdk.tar.gz");

    if let Some(existing_root) = find_sdk_root(&cache_dir)? {
        return Ok(existing_root);
    }

    fs::create_dir_all(&cache_dir).map_err(|err| {
        format!(
            "failed to create wasi-nn-cvitek SDK cache directory {}: {err}",
            cache_dir.display()
        )
    })?;

    ensure_downloaded_sdk_tarball(&sdk_url, &expected_sha, &tarball_path)?;

    remove_extracted_sdk_roots(&cache_dir)?;
    extract_archive(&tarball_path, &cache_dir)?;
    find_sdk_root(&cache_dir)?.ok_or_else(|| {
        format!(
            "failed to locate extracted CVITEK TPU SDK under {} after unpacking {sdk_url}",
            cache_dir.display()
        )
    })
}

fn find_sdk_root(search_dir: &Path) -> Result<Option<PathBuf>, String> {
    if !search_dir.is_dir() {
        return Ok(None);
    }

    let entries = fs::read_dir(search_dir).map_err(|err| {
        format!(
            "failed to enumerate wasi-nn-cvitek SDK cache {}: {err}",
            search_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to inspect wasi-nn-cvitek SDK cache {}: {err}",
                search_dir.display()
            )
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("lib").is_dir() {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

fn remove_extracted_sdk_roots(cache_dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(cache_dir).map_err(|err| {
        format!(
            "failed to enumerate wasi-nn-cvitek SDK cache {}: {err}",
            cache_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to inspect wasi-nn-cvitek SDK cache {}: {err}",
                cache_dir.display()
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| {
                format!(
                    "failed to clear previous CVITEK TPU SDK extraction {}: {err}",
                    path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn ensure_downloaded_sdk_tarball(
    sdk_url: &str,
    expected_sha: &str,
    tarball_path: &Path,
) -> Result<(), String> {
    if !tarball_path.is_file() {
        println!("cargo:warning=downloading CVITEK TPU SDK from {sdk_url}");
        download_file(sdk_url, tarball_path)?;
    }

    let actual_sha = compute_sha256(tarball_path)?;
    if actual_sha == expected_sha {
        return Ok(());
    }

    fs::remove_file(tarball_path).map_err(|err| {
        format!(
            "downloaded CVITEK TPU SDK checksum mismatch and failed to remove stale tarball {}: {err}",
            tarball_path.display()
        )
    })?;
    println!(
        "cargo:warning=retrying CVITEK TPU SDK download after checksum mismatch for {}",
        tarball_path.display()
    );
    println!("cargo:warning=downloading CVITEK TPU SDK from {sdk_url}");
    download_file(sdk_url, tarball_path)?;

    let retried_sha = compute_sha256(tarball_path)?;
    if retried_sha == expected_sha {
        return Ok(());
    }

    Err(format!(
        "downloaded CVITEK TPU SDK checksum mismatch: expected {expected_sha}, got {retried_sha}"
    ))
}

fn add_musl_cpp_runtime_search_paths() -> Result<(), String> {
    let musl_gxx = musl_gxx_program();
    let musl_gxx_path = resolve_program_path(&musl_gxx).ok_or_else(|| {
        format!(
            "wasi-nn-cvitek static musl linking requires {musl_gxx} in PATH or IMAGO_CVITEK_LINK_MODE=dynamic"
        )
    })?;

    let mut search_dirs = Vec::new();
    for lib_name in ["libstdc++.a", "libgcc_eh.a", "libatomic.a"] {
        let lib_path = toolchain_library_path(&musl_gxx_path, lib_name)?;
        let parent = lib_path.parent().ok_or_else(|| {
            format!(
                "failed to determine parent directory for musl runtime library {}",
                lib_path.display()
            )
        })?;
        if !search_dirs.iter().any(|dir: &PathBuf| dir == parent) {
            search_dirs.push(parent.to_path_buf());
        }
    }

    for search_dir in search_dirs {
        println!("cargo:rustc-link-search=native={}", search_dir.display());
    }

    Ok(())
}

fn toolchain_library_path(toolchain: &Path, lib_name: &str) -> Result<PathBuf, String> {
    let output = Command::new(toolchain)
        .arg(format!("-print-file-name={lib_name}"))
        .output()
        .map_err(|err| format!("failed to query {toolchain:?} for {lib_name}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "{toolchain:?} exited with status {:?} while resolving {lib_name}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if resolved.is_empty() || resolved == lib_name {
        return Err(format!(
            "{toolchain:?} did not report a usable path for {lib_name}; install the musl C++ toolchain or set IMAGO_CVITEK_LINK_MODE=dynamic"
        ));
    }

    let path = PathBuf::from(resolved);
    if !path.is_file() {
        return Err(format!(
            "{toolchain:?} reported {lib_name} at {}, but the file does not exist",
            path.display()
        ));
    }

    Ok(path)
}

fn musl_gxx_program() -> String {
    env::var("IMAGO_CVITEK_MUSL_GXX").unwrap_or_else(|_| DEFAULT_MUSL_GXX.to_owned())
}

fn download_file(url: &str, destination: &Path) -> Result<(), String> {
    let destination_str = destination.display().to_string();
    if command_exists("curl") {
        run_command("curl", &["-fsSL", "-o", &destination_str, url])?;
        return Ok(());
    }
    if command_exists("wget") {
        run_command("wget", &["-q", "-O", &destination_str, url])?;
        return Ok(());
    }
    Err("wasi-nn-cvitek auto-download requires curl or wget in PATH".to_owned())
}

fn extract_archive(archive: &Path, destination_dir: &Path) -> Result<(), String> {
    if !command_exists("tar") {
        return Err("wasi-nn-cvitek auto-download requires tar in PATH".to_owned());
    }
    let archive_str = archive.display().to_string();
    let destination_str = destination_dir.display().to_string();
    run_command("tar", &["-xzf", &archive_str, "-C", &destination_str])
}

fn compute_sha256(path: &Path) -> Result<String, String> {
    let path_str = path.display().to_string();
    if command_exists("sha256sum") {
        return compute_sha256_with_command("sha256sum", &[&path_str]);
    }
    if command_exists("shasum") {
        return compute_sha256_with_command("shasum", &["-a", "256", &path_str]);
    }
    Err("wasi-nn-cvitek auto-download requires sha256sum or shasum in PATH".to_owned())
}

fn compute_sha256_with_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| format!("failed to run {program}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} exited with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "failed to parse {program} output for {}",
                args.last().unwrap_or(&"")
            )
        })
}

fn run_command(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| format!("failed to run {program}: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(format!(
        "{program} exited with status {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    ))
}

fn command_exists(program: &str) -> bool {
    resolve_program_path(program).is_some()
}

fn resolve_program_path(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }

    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}
