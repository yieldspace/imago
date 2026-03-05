use crate::planner::ReleasePlan;
use crate::workspace::WorkspaceInfo;
use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Value, value};

pub fn apply_plan(
    repo_root: &Path,
    workspace: &WorkspaceInfo,
    plan: &ReleasePlan,
    dry_run: bool,
) -> Result<()> {
    if plan.workspace_version_update.is_some() {
        let cargo_toml_path = repo_root.join("Cargo.toml");
        let mut doc = load_doc(&cargo_toml_path)?;
        if let Some(workspace_update) = &plan.workspace_version_update {
            doc["workspace"]["package"]["version"] = value(workspace_update.after.clone());
        }
        if !dry_run {
            fs::write(&cargo_toml_path, doc.to_string())
                .with_context(|| format!("failed to write {}", cargo_toml_path.display()))?;
        }
    }

    let package_updates: BTreeMap<String, String> = plan
        .package_version_updates
        .iter()
        .map(|update| (update.crate_name.clone(), update.after.clone()))
        .collect();

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

    if !package_updates.is_empty() {
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
            let changed = update_dependency_versions(&mut doc, &package_updates);
            if changed && !dry_run {
                fs::write(&manifest_path, doc.to_string())
                    .with_context(|| format!("failed to write {}", manifest_path.display()))?;
            }
        }
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
