use crate::planner::ReleasePlan;
use crate::workspace::WorkspaceInfo;
use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use toml_edit::{DocumentMut, Item, Value, value};

pub fn apply_plan(
    repo_root: &Path,
    workspace: &WorkspaceInfo,
    plan: &ReleasePlan,
    dry_run: bool,
) -> Result<()> {
    let package_updates: BTreeMap<String, String> = plan
        .package_version_updates
        .iter()
        .map(|update| (update.crate_name.clone(), update.after.clone()))
        .collect();
    let dependency_updates: BTreeMap<String, String> = plan
        .crate_updates
        .iter()
        .map(|update| (update.crate_name.clone(), update.after.clone()))
        .collect();

    let cargo_toml_path = repo_root.join("Cargo.toml");
    let mut root_doc = load_doc(&cargo_toml_path)?;
    let mut root_changed = false;
    if let Some(workspace_update) = &plan.workspace_version_update {
        root_doc["workspace"]["package"]["version"] = value(workspace_update.after.clone());
        root_changed = true;
    }
    root_changed |= update_dependency_versions(&mut root_doc, &dependency_updates);
    if root_changed && !dry_run {
        fs::write(&cargo_toml_path, root_doc.to_string())
            .with_context(|| format!("failed to write {}", cargo_toml_path.display()))?;
    }

    for (crate_name, next_version) in &package_updates {
        let package = workspace
            .package(crate_name)
            .ok_or_else(|| anyhow!("package {crate_name} not found"))?;
        let mut doc = load_doc(&package.manifest_path)?;
        doc["package"]["version"] = value(next_version.clone());
        if !dry_run {
            fs::write(&package.manifest_path, doc.to_string())
                .with_context(|| format!("failed to write {}", package.manifest_path.display()))?;
        }
    }

    if !dependency_updates.is_empty() {
        for manifest_path in workspace.workspace_manifest_paths() {
            if package_updates.keys().any(|crate_name| {
                workspace
                    .package(crate_name)
                    .map(|pkg| pkg.manifest_path == manifest_path)
                    .unwrap_or(false)
            }) {
                continue;
            }

            let mut doc = load_doc(&manifest_path)?;
            let changed = update_dependency_versions(&mut doc, &dependency_updates);
            if changed && !dry_run {
                fs::write(&manifest_path, doc.to_string())
                    .with_context(|| format!("failed to write {}", manifest_path.display()))?;
            }
        }
    }

    if !dry_run {
        sync_lockfile(repo_root)?;
    }

    Ok(())
}

fn sync_lockfile(repo_root: &Path) -> Result<()> {
    let manifest_path = repo_root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("update")
        .arg("--workspace")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(repo_root)
        .output()
        .with_context(|| {
            format!(
                "failed to run cargo update --workspace for {}",
                manifest_path.display()
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(anyhow!(
            "cargo update --workspace failed for {}: {}",
            manifest_path.display(),
            details
        ));
    }

    Ok(())
}

fn load_doc(path: &Path) -> Result<DocumentMut> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    raw.parse::<DocumentMut>()
        .with_context(|| format!("failed to parse TOML: {}", path.display()))
}

fn update_dependency_versions(
    doc: &mut DocumentMut,
    package_updates: &BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    changed |= update_dependency_tables(doc.as_item_mut(), package_updates);
    changed
}

fn update_dependency_tables(item: &mut Item, package_updates: &BTreeMap<String, String>) -> bool {
    let mut changed = false;

    if let Some(table) = item.as_table_like_mut() {
        let keys: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();
        for key in keys {
            if key == "dependencies" || key == "dev-dependencies" || key == "build-dependencies" {
                if let Some(dep_item) = table.get_mut(&key) {
                    changed |= update_single_dependency_table(dep_item, package_updates);
                }
                continue;
            }

            if let Some(child) = table.get_mut(&key) {
                changed |= update_dependency_tables(child, package_updates);
            }
        }
    }

    changed
}

fn update_single_dependency_table(
    item: &mut Item,
    package_updates: &BTreeMap<String, String>,
) -> bool {
    let mut changed = false;

    let Some(table) = item.as_table_like_mut() else {
        return false;
    };

    let dep_names: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();

    for dep_name in dep_names {
        let Some(next_version) = package_updates.get(&dep_name) else {
            continue;
        };

        let Some(dep_item) = table.get_mut(&dep_name) else {
            continue;
        };

        if dep_item.is_str() {
            *dep_item = value(next_version.clone());
            changed = true;
            continue;
        }

        if let Some(inline) = dep_item.as_inline_table_mut() {
            let workspace_ref = inline
                .get("workspace")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if workspace_ref {
                continue;
            }
            inline.insert("version", Value::from(next_version.as_str()));
            changed = true;
            continue;
        }

        if let Some(dep_table) = dep_item.as_table_like_mut() {
            let workspace_ref = dep_table
                .get("workspace")
                .and_then(Item::as_value)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if workspace_ref {
                continue;
            }
            dep_table.insert("version", value(next_version.clone()));
            changed = true;
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{BumpLevel, PackageVersionUpdate, ReleasePlan, VersionUpdate};
    use crate::workspace::{WorkspaceInfo, WorkspacePackage};
    use semver::Version;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn apply_plan_updates_package_versions_and_lockfile() {
        let root = create_package_workspace();
        let workspace = load_workspace_info(&root, &[("imago-cli", "crates/imago-cli")]);
        let plan = ReleasePlan {
            line_base_refs: BTreeMap::new(),
            changed_files: Vec::new(),
            changed_crates: Vec::new(),
            impacted_crates: Vec::new(),
            line_bumps: Vec::new(),
            workspace_version_update: None,
            package_version_updates: vec![PackageVersionUpdate {
                crate_name: "imago-cli".to_string(),
                before: "0.1.0".to_string(),
                after: "0.2.0".to_string(),
                bump: BumpLevel::Minor,
            }],
            crate_updates: vec![crate_update(
                "imago-cli",
                "imago-cli",
                "0.1.0",
                "0.2.0",
                crate::config::VersionSource::Package,
            )],
            tags: Vec::new(),
            release_targets: Vec::new(),
        };

        apply_plan(&root, &workspace, &plan, false).expect("apply should succeed");

        assert!(
            read_to_string(root.join("crates/imago-cli/Cargo.toml"))
                .contains("version = \"0.2.0\"")
        );
        assert_eq!(
            lockfile_package_version(&root, "imago-cli").as_deref(),
            Some("0.2.0")
        );
    }

    #[test]
    fn apply_plan_updates_workspace_versions_and_lockfile() {
        let root = create_workspace_version_workspace();
        let workspace = load_workspace_info(
            &root,
            &[
                ("imagod", "crates/imagod"),
                ("imagod-common", "crates/imagod-common"),
            ],
        );
        let plan = ReleasePlan {
            line_base_refs: BTreeMap::new(),
            changed_files: Vec::new(),
            changed_crates: Vec::new(),
            impacted_crates: Vec::new(),
            line_bumps: Vec::new(),
            workspace_version_update: Some(VersionUpdate {
                before: "0.1.0".to_string(),
                after: "0.2.0".to_string(),
                bump: BumpLevel::Minor,
            }),
            package_version_updates: Vec::new(),
            crate_updates: vec![
                crate_update(
                    "imagod",
                    "imagod-daemon",
                    "0.1.0",
                    "0.2.0",
                    crate::config::VersionSource::Workspace,
                ),
                crate_update(
                    "imagod-common",
                    "imagod-daemon",
                    "0.1.0",
                    "0.2.0",
                    crate::config::VersionSource::Workspace,
                ),
            ],
            tags: Vec::new(),
            release_targets: Vec::new(),
        };

        apply_plan(&root, &workspace, &plan, false).expect("apply should succeed");

        assert!(read_to_string(root.join("Cargo.toml")).contains("version = \"0.2.0\""));
        assert!(
            read_to_string(root.join("Cargo.toml")).contains(
                "imagod-common = { path = \"crates/imagod-common\", version = \"0.2.0\" }"
            )
        );
        assert_eq!(
            lockfile_package_version(&root, "imagod").as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            lockfile_package_version(&root, "imagod-common").as_deref(),
            Some("0.2.0")
        );
    }

    #[test]
    fn apply_plan_dry_run_does_not_touch_manifests_or_lockfile() {
        let root = create_package_workspace();
        let workspace = load_workspace_info(&root, &[("imago-cli", "crates/imago-cli")]);
        let cargo_before = read_to_string(root.join("crates/imago-cli/Cargo.toml"));
        let lock_before = read_to_string(root.join("Cargo.lock"));
        let plan = ReleasePlan {
            line_base_refs: BTreeMap::new(),
            changed_files: Vec::new(),
            changed_crates: Vec::new(),
            impacted_crates: Vec::new(),
            line_bumps: Vec::new(),
            workspace_version_update: None,
            package_version_updates: vec![PackageVersionUpdate {
                crate_name: "imago-cli".to_string(),
                before: "0.1.0".to_string(),
                after: "0.2.0".to_string(),
                bump: BumpLevel::Minor,
            }],
            crate_updates: vec![crate_update(
                "imago-cli",
                "imago-cli",
                "0.1.0",
                "0.2.0",
                crate::config::VersionSource::Package,
            )],
            tags: Vec::new(),
            release_targets: Vec::new(),
        };

        apply_plan(&root, &workspace, &plan, true).expect("dry-run should succeed");

        assert_eq!(
            read_to_string(root.join("crates/imago-cli/Cargo.toml")),
            cargo_before
        );
        assert_eq!(read_to_string(root.join("Cargo.lock")), lock_before);
    }

    fn create_package_workspace() -> PathBuf {
        let root = unique_temp_dir("prup-apply-package");
        write_file(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/imago-cli", "crates/imago-project-config"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
"#,
        );
        write_file(
            root.join("crates/imago-cli/Cargo.toml"),
            r#"[package]
name = "imago-cli"
version = "0.1.0"
edition = "2024"

[dependencies]
imago-project-config = { path = "../imago-project-config", version = "0.1.0" }
"#,
        );
        write_file(
            root.join("crates/imago-cli/src/lib.rs"),
            "pub fn cli() {}\n",
        );
        write_file(
            root.join("crates/imago-project-config/Cargo.toml"),
            r#"[package]
name = "imago-project-config"
version = "0.1.0"
edition = "2024"
"#,
        );
        write_file(
            root.join("crates/imago-project-config/src/lib.rs"),
            "pub fn config() {}\n",
        );
        sync_lockfile(&root).expect("initial cargo update should succeed");
        root
    }

    fn create_workspace_version_workspace() -> PathBuf {
        let root = unique_temp_dir("prup-apply-workspace");
        write_file(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/imagod", "crates/imagod-common"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
imagod-common = { path = "crates/imagod-common", version = "0.1.0" }
"#,
        );
        write_file(
            root.join("crates/imagod/Cargo.toml"),
            r#"[package]
name = "imagod"
version.workspace = true
edition.workspace = true

[dependencies]
imagod-common = { workspace = true }
"#,
        );
        write_file(
            root.join("crates/imagod/src/lib.rs"),
            "pub fn daemon() {}\n",
        );
        write_file(
            root.join("crates/imagod-common/Cargo.toml"),
            r#"[package]
name = "imagod-common"
version.workspace = true
edition.workspace = true
"#,
        );
        write_file(
            root.join("crates/imagod-common/src/lib.rs"),
            "pub fn common() {}\n",
        );
        sync_lockfile(&root).expect("initial cargo update should succeed");
        root
    }

    fn load_workspace_info(root: &Path, packages: &[(&str, &str)]) -> WorkspaceInfo {
        let packages = packages
            .iter()
            .map(|(name, relative_path)| {
                let manifest_path = root.join(relative_path).join("Cargo.toml");
                (
                    (*name).to_string(),
                    WorkspacePackage {
                        manifest_dir: manifest_path
                            .parent()
                            .expect("manifest path should have parent")
                            .to_path_buf(),
                        manifest_path,
                        version: Version::parse("0.1.0").expect("version should parse"),
                    },
                )
            })
            .collect();

        WorkspaceInfo {
            repo_root: root.to_path_buf(),
            packages,
            forward_deps: BTreeMap::new(),
            reverse_deps: BTreeMap::new(),
        }
    }

    fn lockfile_package_version(root: &Path, package_name: &str) -> Option<String> {
        let raw = read_to_string(root.join("Cargo.lock"));
        let mut current_name = None::<String>;

        for line in raw.lines() {
            if let Some(value) = line.strip_prefix("name = \"") {
                current_name = Some(value.trim_end_matches('"').to_string());
                continue;
            }

            if current_name.as_deref() == Some(package_name)
                && let Some(value) = line.strip_prefix("version = \"")
            {
                return Some(value.trim_end_matches('"').to_string());
            }
        }

        None
    }

    fn crate_update(
        crate_name: &str,
        line_id: &str,
        before: &str,
        after: &str,
        version_source: crate::config::VersionSource,
    ) -> crate::planner::CrateVersionUpdate {
        crate::planner::CrateVersionUpdate {
            crate_name: crate_name.to_string(),
            line_id: line_id.to_string(),
            before: before.to_string(),
            after: after.to_string(),
            version_source,
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("temp dir should be creatable");
        root
    }

    fn write_file(path: PathBuf, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir should be creatable");
        }
        fs::write(path, content).expect("file should be writable");
    }

    fn read_to_string(path: PathBuf) -> String {
        fs::read_to_string(path).expect("file should be readable")
    }
}
