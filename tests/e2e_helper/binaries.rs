use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

const E2E_IMAGOD_BIN_ENV: &str = "IMAGO_E2E_IMAGOD_BIN";
const E2E_IMAGO_CLI_BIN_ENV: &str = "IMAGO_E2E_IMAGO_CLI_BIN";

static BUILT_PACKAGES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub fn resolve_imago_cli_binary(workspace_root: &Path) -> Result<PathBuf> {
    resolve_binary_for_package(
        workspace_root,
        "imago-cli",
        "imago",
        Some(E2E_IMAGO_CLI_BIN_ENV),
    )
}

pub fn resolve_imagod_binary(workspace_root: &Path, daemon_package: &str) -> Result<PathBuf> {
    let env_override = if daemon_package == "imagod" {
        Some(E2E_IMAGOD_BIN_ENV)
    } else {
        None
    };
    resolve_binary_for_package(workspace_root, daemon_package, daemon_package, env_override)
}

fn resolve_binary_for_package(
    workspace_root: &Path,
    package_name: &str,
    binary_name: &str,
    env_override: Option<&str>,
) -> Result<PathBuf> {
    if let Some(env_key) = env_override
        && let Some(value) = std::env::var_os(env_key)
    {
        let path = PathBuf::from(value);
        if !path.is_file() {
            bail!("{env_key} points to missing binary: {}", path.display());
        }
        return Ok(path);
    }

    let binary_path = resolve_target_debug_dir(workspace_root).join(binary_name);
    ensure_package_built_once(workspace_root, package_name, &binary_path)?;
    if !binary_path.is_file() {
        bail!(
            "failed to resolve binary for package '{package_name}': {}",
            binary_path.display()
        );
    }
    Ok(binary_path)
}

fn resolve_target_debug_dir(workspace_root: &Path) -> PathBuf {
    resolve_target_debug_dir_from_env(
        workspace_root,
        std::env::var_os("CARGO_TARGET_DIR").as_deref(),
    )
}

fn resolve_target_debug_dir_from_env(
    workspace_root: &Path,
    cargo_target_dir: Option<&OsStr>,
) -> PathBuf {
    match cargo_target_dir {
        Some(path) if !path.is_empty() => {
            let target_dir = PathBuf::from(path);
            if target_dir.is_absolute() {
                target_dir.join("debug")
            } else {
                workspace_root.join(target_dir).join("debug")
            }
        }
        _ => workspace_root.join("target").join("debug"),
    }
}

/// Builds `package_name` at most once per process for concurrent e2e tests.
///
/// `BUILT_PACKAGES` tracks package names in a `OnceLock<Mutex<HashSet<_>>>`.
/// If the package is already marked as built and `binary_path` exists, this
/// returns immediately without invoking `cargo build`.
fn ensure_package_built_once(
    workspace_root: &Path,
    package_name: &str,
    binary_path: &Path,
) -> Result<()> {
    let lock = BUILT_PACKAGES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut built = lock
        .lock()
        .map_err(|_| anyhow!("failed to lock e2e binary build tracker"))?;
    if built.contains(package_name) && binary_path.is_file() {
        return Ok(());
    }
    if !binary_path.is_file() {
        let output = Command::new("cargo")
            .arg("build")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(workspace_root.join("Cargo.toml"))
            .arg("-p")
            .arg(package_name)
            .current_dir(workspace_root)
            .output()
            .with_context(|| format!("failed to build package '{package_name}'"))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "failed to build package '{package_name}': status={}, stdout={}, stderr={}",
                output.status,
                stdout,
                stderr
            );
        }
    }
    built.insert(package_name.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_target_debug_dir_defaults_to_workspace_target() {
        let workspace_root = Path::new("/workspace/imago");
        let resolved = resolve_target_debug_dir_from_env(workspace_root, None);
        assert_eq!(resolved, workspace_root.join("target").join("debug"));
    }

    #[test]
    fn resolve_target_debug_dir_uses_absolute_env_path() {
        let workspace_root = Path::new("/workspace/imago");
        let resolved = resolve_target_debug_dir_from_env(
            workspace_root,
            Some(OsStr::new("/tmp/custom-target")),
        );
        assert_eq!(resolved, Path::new("/tmp/custom-target").join("debug"));
    }

    #[test]
    fn resolve_target_debug_dir_resolves_relative_env_path_from_workspace_root() {
        let workspace_root = Path::new("/workspace/imago");
        let resolved =
            resolve_target_debug_dir_from_env(workspace_root, Some(OsStr::new("custom-target")));
        assert_eq!(resolved, workspace_root.join("custom-target").join("debug"));
    }
}
