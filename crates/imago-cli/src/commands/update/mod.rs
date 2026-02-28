//! Dependency and binding resolution pipeline for `imago deps sync`.
//!
//! The update flow resolves WIT/component sources, rewrites `wit/deps`,
//! and persists lock metadata consumed by build/deploy operations.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use imago_lockfile::{
    IMAGO_LOCK_VERSION, ImagoLock, ImagoLockBindingWit, ImagoLockDependency,
    TransitivePackageRecord, collect_wit_packages, save_to_project_root,
};
use sha2::{Digest, Sha256};
use wit_component::WitPrinter;
use wit_parser::{
    FunctionKind, InterfaceId, PackageId, Resolve, Type, TypeDefKind, TypeId,
    UnresolvedPackageGroup,
};

use crate::{
    cli::UpdateArgs,
    commands::{
        CommandResult,
        build::{self},
        dependency_cache::{self},
        error_diagnostics::{format_command_error, summarize_command_failure},
        plugin_sources,
        shared::dependency::StandardDependencyResolver,
        ui,
    },
};

mod cache;

const IMAGO_NODE_CONNECTION_USE: &str = "use imago:node/rpc@0.1.0.{connection};";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedBindingWit {
    name: String,
    wit_source_kind: plugin_sources::SourceKind,
    wit_source: String,
    wit_registry: Option<String>,
    wit_version: String,
    package_name: String,
    package_version: Option<String>,
    wit_path: String,
    interface_names: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UpdateSummary {
    dependencies: usize,
    binding_wits: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentWorldNonWasiInterfaceRequirement {
    dependency_name: String,
    package_name: String,
    version: String,
    interfaces: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ComponentWorldReferenceSummary {
    referenced_package_versions: BTreeMap<String, String>,
    non_wasi_interface_requirements: Vec<ComponentWorldNonWasiInterfaceRequirement>,
}

pub async fn run(args: UpdateArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(_args: UpdateArgs, project_root: &Path) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start("deps.sync", "starting");
    match run_inner_async(project_root).await {
        Ok(summary) => {
            ui::command_finish("deps.sync", true, "");
            let mut result = CommandResult::success("deps.sync", started_at);
            result
                .meta
                .insert("dependencies".to_string(), summary.dependencies.to_string());
            result
                .meta
                .insert("binding_wits".to_string(), summary.binding_wits.to_string());
            result
        }
        Err(err) => {
            let summary_message = summarize_command_failure("deps.sync", &err);
            let diagnostic_message = format_command_error("deps.sync", &err);
            ui::command_finish("deps.sync", false, &summary_message);
            CommandResult::failure("deps.sync", started_at, diagnostic_message)
        }
    }
}

fn normalize_path_for_compare(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn normalize_absolute_path_for_compare(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory for update path validation")?
            .join(path)
    };
    Ok(normalize_path_for_compare(&absolute))
}

fn validate_wit_sources_outside_wit_deps(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
    bindings: &[build::ProjectBindingSource],
) -> anyhow::Result<()> {
    let project_root_abs = normalize_absolute_path_for_compare(project_root)?;
    let mut wit_deps_roots = vec![normalize_path_for_compare(
        &project_root_abs.join("wit").join("deps"),
    )];
    if let Ok(canonical_project_root) = fs::canonicalize(&project_root_abs) {
        let canonical_wit_deps_root =
            normalize_path_for_compare(&canonical_project_root.join("wit").join("deps"));
        if !wit_deps_roots
            .iter()
            .any(|existing| existing == &canonical_wit_deps_root)
        {
            wit_deps_roots.push(canonical_wit_deps_root);
        }
    }

    let validate_path_source = |subject_name: &str,
                                source_label: &str,
                                source_kind: plugin_sources::SourceKind,
                                source: &str|
     -> anyhow::Result<()> {
        if source_kind != plugin_sources::SourceKind::Path {
            return Ok(());
        }
        if source.starts_with("http://") || source.starts_with("https://") {
            return Ok(());
        }
        let raw_path = source.strip_prefix("file://").unwrap_or(source);
        let source_path = if Path::new(raw_path).is_absolute() {
            PathBuf::from(raw_path)
        } else {
            project_root_abs.join(raw_path)
        };
        let mut source_candidates = vec![normalize_absolute_path_for_compare(&source_path)?];
        if let Ok(canonical_source) = fs::canonicalize(&source_path) {
            let canonical_source = normalize_path_for_compare(&canonical_source);
            if !source_candidates
                .iter()
                .any(|existing| existing == &canonical_source)
            {
                source_candidates.push(canonical_source);
            }
        }

        if source_candidates.iter().any(|candidate| {
            wit_deps_roots
                .iter()
                .any(|wit_deps_root| candidate.starts_with(wit_deps_root))
        }) {
            return Err(anyhow!(
                "{} {} '{}' points under wit/deps, which `imago deps sync` resets; move the source outside wit/deps",
                subject_name,
                source_label,
                source
            ));
        }
        Ok(())
    };

    for dependency in dependencies {
        validate_path_source(
            &dependency.name,
            "wit source",
            dependency.wit.source_kind,
            &dependency.wit.source,
        )?;
        if let Some(component) = dependency.component.as_ref() {
            validate_path_source(
                &dependency.name,
                "component source",
                component.source_kind,
                &component.source,
            )?;
        }
    }
    for binding in bindings {
        validate_path_source(
            &binding.name,
            "binding wit source",
            binding.wit_source_kind,
            &binding.wit_source,
        )?;
    }
    Ok(())
}

fn validate_wit_output_path_collisions(
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<()> {
    let mut targets: Vec<(PathBuf, &str)> = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let target_rel =
            dependency_cache::dependency_wit_target_rel(&dependency.name, &dependency.version);
        for (existing_target, existing_dependency) in &targets {
            if existing_target == &target_rel {
                return Err(anyhow!(
                    "dependencies '{}' and '{}' both resolve to '{}'; dependency WIT output paths must be unique",
                    existing_dependency,
                    dependency.name,
                    plugin_sources::path_to_manifest_string(&target_rel)
                ));
            }
            if target_rel.starts_with(existing_target) || existing_target.starts_with(&target_rel) {
                return Err(anyhow!(
                    "dependencies '{}' and '{}' have overlapping WIT output paths ('{}' and '{}'); dependency WIT output paths must be disjoint",
                    existing_dependency,
                    dependency.name,
                    plugin_sources::path_to_manifest_string(existing_target),
                    plugin_sources::path_to_manifest_string(&target_rel)
                ));
            }
        }
        targets.push((target_rel, dependency.name.as_str()));
    }
    Ok(())
}

fn package_name_from_resolve(resolve: &Resolve, package_id: PackageId) -> String {
    let package = &resolve.packages[package_id].name;
    format!("{}:{}", package.namespace, package.name)
}

fn package_version_from_resolve(resolve: &Resolve, package_id: PackageId) -> Option<String> {
    resolve.packages[package_id]
        .name
        .version
        .as_ref()
        .map(ToString::to_string)
}

fn package_interface_names(resolve: &Resolve, package_id: PackageId) -> BTreeSet<String> {
    resolve.packages[package_id]
        .interfaces
        .keys()
        .cloned()
        .collect()
}

fn package_interface_ids(package_name: &str, interface_names: &BTreeSet<String>) -> Vec<String> {
    interface_names
        .iter()
        .map(|interface_name| format!("{package_name}/{interface_name}"))
        .collect()
}

fn validate_binding_dependency_collision(
    binding: &build::ProjectBindingSource,
    package_name: &str,
    package_version: Option<&str>,
    dependency: &build::ProjectDependency,
) -> anyhow::Result<()> {
    if dependency.wit.source_kind != binding.wit_source_kind
        || dependency.wit.source != binding.wit_source
        || dependency.wit.registry != binding.wit_registry
    {
        return Err(anyhow!(
            "binding '{}' points to package '{}' but dependency '{}' has different wit source kind/source/registry; align bindings.wit with dependencies.wit or remove one side",
            binding.name,
            package_name,
            dependency.name
        ));
    }
    if let Some(package_version) = package_version
        && dependency.version != package_version
    {
        return Err(anyhow!(
            "binding '{}' package '{}' version '{}' does not match dependency '{}' version '{}'",
            binding.name,
            package_name,
            package_version,
            dependency.name,
            dependency.version
        ));
    }
    Ok(())
}

async fn materialize_binding_wit_source(
    project_root: &Path,
    binding: &build::ProjectBindingSource,
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
    destination_root: &Path,
) -> anyhow::Result<()> {
    fs::create_dir_all(destination_root).with_context(|| {
        format!(
            "failed to create binding wit destination {}",
            destination_root.display()
        )
    })?;
    plugin_sources::materialize_wit_source(
        project_root,
        binding.wit_source_kind,
        &binding.wit_source,
        Some(&binding.wit_version),
        binding.wit_registry.as_deref(),
        namespace_registries,
        None,
        binding.wit_sha256.as_deref(),
        destination_root,
    )
    .await
    .with_context(|| {
        format!(
            "failed to resolve binding '{}' wit source '{}'",
            binding.name, binding.wit_source
        )
    })?;
    Ok(())
}

fn source_kind_sort_key(kind: plugin_sources::SourceKind) -> u8 {
    match kind {
        plugin_sources::SourceKind::Wit => 0,
        plugin_sources::SourceKind::Oci => 1,
        plugin_sources::SourceKind::Path => 2,
    }
}

async fn resolve_binding_wits(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
    lock_entries: &[ImagoLockDependency],
    bindings: &[build::ProjectBindingSource],
    namespace_registries: Option<&plugin_sources::NamespaceRegistries>,
) -> anyhow::Result<Vec<ResolvedBindingWit>> {
    if bindings.is_empty() {
        return Ok(Vec::new());
    }

    let deps_root = project_root.join("wit").join("deps");
    let dependency_by_name = dependencies
        .iter()
        .zip(lock_entries.iter())
        .map(|(dependency, lock_entry)| (lock_entry.name.as_str(), dependency))
        .collect::<BTreeMap<_, _>>();
    let lock_by_name = lock_entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut package_resolutions = BTreeMap::<
        String,
        (
            plugin_sources::SourceKind,
            String,
            Option<String>,
            String,
            Option<String>,
            String,
            BTreeSet<String>,
        ),
    >::new();
    let mut path_to_package = BTreeMap::<String, String>::new();
    for lock_entry in lock_entries {
        path_to_package.insert(lock_entry.wit_path.clone(), lock_entry.name.clone());
    }

    let mut resolved_bindings = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let temp_root =
            std::env::temp_dir().join(format!("imago-update-binding-{}", uuid::Uuid::new_v4()));
        let temp_top = temp_root.join("top");
        fs::create_dir_all(&temp_root)
            .with_context(|| format!("failed to create temp dir {}", temp_root.display()))?;
        let resolve_result = async {
            materialize_binding_wit_source(
                project_root,
                binding,
                namespace_registries,
                &temp_top,
            )
            .await?;
            let (temp_resolve, temp_package_id) = parse_dependency_wit_with_deps(&temp_top, &temp_root)
                .with_context(|| {
                    format!(
                        "failed to parse binding '{}' WIT source '{}'",
                        binding.name, binding.wit_source
                    )
                })?;
            let package_name = package_name_from_resolve(&temp_resolve, temp_package_id);
            let package_version = package_version_from_resolve(&temp_resolve, temp_package_id);
            let interface_names = package_interface_names(&temp_resolve, temp_package_id);
            if interface_names.is_empty() {
                return Err(anyhow!(
                    "binding '{}' source '{}' resolved package '{}' has no interfaces",
                    binding.name,
                    binding.wit_source,
                    package_name
                ));
            }

            if let Some((
                existing_source_kind,
                existing_source,
                existing_registry,
                existing_wit_version,
                existing_version,
                existing_path,
                existing_interfaces,
            )) = package_resolutions.get(&package_name)
            {
                if existing_source_kind != &binding.wit_source_kind
                    || existing_source != &binding.wit_source
                    || existing_registry != &binding.wit_registry
                    || existing_wit_version != &binding.wit_version
                    || existing_version != &package_version
                {
                    return Err(anyhow!(
                        "binding '{}' package '{}' conflicts with another binding source; same package must use identical source/version/registry",
                        binding.name,
                        package_name
                    ));
                }
                return Ok(ResolvedBindingWit {
                    name: binding.name.clone(),
                    wit_source_kind: binding.wit_source_kind,
                    wit_source: binding.wit_source.clone(),
                    wit_registry: binding.wit_registry.clone(),
                    wit_version: binding.wit_version.clone(),
                    package_name: package_name.clone(),
                    package_version,
                    wit_path: existing_path.clone(),
                    interface_names: existing_interfaces.clone(),
                });
            }

            let wit_path = if let Some(dependency) = dependency_by_name.get(package_name.as_str()) {
                validate_binding_dependency_collision(
                    binding,
                    &package_name,
                    package_version.as_deref(),
                    dependency,
                )?;
                let lock_entry = lock_by_name.get(package_name.as_str()).ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                        package_name
                    )
                })?;
                let wit_path = lock_entry.wit_path.clone();
                let dependency_wit_root = project_root.join(&wit_path);
                let (resolve, package_id) =
                    parse_dependency_wit_with_deps(&dependency_wit_root, &deps_root).with_context(
                        || {
                            format!(
                                "failed to parse hydrated dependency WIT for binding package '{}'",
                                package_name
                            )
                        },
                    )?;
                let hydrated_interface_names = package_interface_names(&resolve, package_id);
                if hydrated_interface_names.is_empty() {
                    return Err(anyhow!(
                        "binding '{}' target package '{}' has no interfaces in hydrated dependency WIT",
                        binding.name,
                        package_name
                    ));
                }
                package_resolutions.insert(
                    package_name.clone(),
                    (
                        binding.wit_source_kind,
                        binding.wit_source.clone(),
                        binding.wit_registry.clone(),
                        binding.wit_version.clone(),
                        package_version.clone(),
                        wit_path.clone(),
                        hydrated_interface_names.clone(),
                    ),
                );
                return Ok(ResolvedBindingWit {
                    name: binding.name.clone(),
                    wit_source_kind: binding.wit_source_kind,
                    wit_source: binding.wit_source.clone(),
                    wit_registry: binding.wit_registry.clone(),
                    wit_version: binding.wit_version.clone(),
                    package_name: package_name.clone(),
                    package_version,
                    wit_path,
                    interface_names: hydrated_interface_names,
                });
            } else {
                plugin_sources::wit_deps_path(&package_name, package_version.as_deref())
            };

            if let Some(existing_package) = path_to_package.get(&wit_path)
                && existing_package != &package_name
            {
                return Err(anyhow!(
                    "binding '{}' package '{}' resolves to '{}' which is already used by package '{}'",
                    binding.name,
                    package_name,
                    wit_path,
                    existing_package
                ));
            }
            path_to_package.insert(wit_path.clone(), package_name.clone());

            let project_wit_root = project_root.join(&wit_path);
            materialize_binding_wit_source(
                project_root,
                binding,
                namespace_registries,
                &project_wit_root,
            )
            .await?;
            let (project_resolve, project_package_id) =
                parse_dependency_wit_with_deps(&project_wit_root, &deps_root).with_context(|| {
                    format!(
                        "failed to parse hydrated binding package '{}' at {}",
                        package_name,
                        project_wit_root.display()
                    )
                })?;
            let project_package_name = package_name_from_resolve(&project_resolve, project_package_id);
            if project_package_name != package_name {
                return Err(anyhow!(
                    "binding '{}' hydrated package mismatch: expected '{}', actual '{}'",
                    binding.name,
                    package_name,
                    project_package_name
                ));
            }
            let hydrated_interface_names = package_interface_names(&project_resolve, project_package_id);
            if hydrated_interface_names.is_empty() {
                return Err(anyhow!(
                    "binding '{}' package '{}' has no interfaces after hydration",
                    binding.name,
                    package_name
                ));
            }

            package_resolutions.insert(
                package_name.clone(),
                (
                    binding.wit_source_kind,
                    binding.wit_source.clone(),
                    binding.wit_registry.clone(),
                    binding.wit_version.clone(),
                    package_version.clone(),
                    wit_path.clone(),
                    hydrated_interface_names.clone(),
                ),
            );
            Ok(ResolvedBindingWit {
                name: binding.name.clone(),
                wit_source_kind: binding.wit_source_kind,
                wit_source: binding.wit_source.clone(),
                wit_registry: binding.wit_registry.clone(),
                wit_version: binding.wit_version.clone(),
                package_name,
                package_version,
                wit_path,
                interface_names: hydrated_interface_names,
            })
        }
        .await;
        let _ = fs::remove_dir_all(&temp_root);
        resolved_bindings.push(resolve_result?);
    }

    let mut deduped =
        BTreeMap::<(String, u8, String, Option<String>, String, String), ResolvedBindingWit>::new();
    for binding in resolved_bindings {
        let key = (
            binding.name.clone(),
            source_kind_sort_key(binding.wit_source_kind),
            binding.wit_source.clone(),
            binding.wit_registry.clone(),
            binding.wit_version.clone(),
            binding.package_name.clone(),
        );
        deduped
            .entry(key)
            .and_modify(|existing| {
                existing
                    .interface_names
                    .extend(binding.interface_names.iter().cloned());
            })
            .or_insert(binding);
    }
    Ok(deduped.into_values().collect())
}

fn rewrite_bound_wit_packages(
    project_root: &Path,
    bindings: &[ResolvedBindingWit],
) -> anyhow::Result<()> {
    if bindings.is_empty() {
        return Ok(());
    }

    let mut rewrite_targets = BTreeMap::<String, (String, BTreeSet<String>)>::new();
    for binding in bindings {
        if let Some((package_name, interfaces)) = rewrite_targets.get_mut(&binding.wit_path) {
            if package_name != &binding.package_name {
                return Err(anyhow!(
                    "bindings package '{}' conflicts with '{}' on shared wit path '{}'",
                    binding.package_name,
                    package_name,
                    binding.wit_path
                ));
            }
            interfaces.extend(binding.interface_names.iter().cloned());
            continue;
        }
        rewrite_targets.insert(
            binding.wit_path.clone(),
            (
                binding.package_name.clone(),
                binding.interface_names.clone(),
            ),
        );
    }

    let deps_root = project_root.join("wit").join("deps");
    for (wit_path, (package_name, interface_names)) in rewrite_targets {
        let dependency_wit_root = project_root.join(&wit_path);
        rewrite_dependency_wit_interfaces(
            &package_name,
            &dependency_wit_root,
            &deps_root,
            &interface_names,
        )?;
    }

    Ok(())
}

fn rewrite_dependency_wit_interfaces(
    dependency_name: &str,
    dependency_wit_root: &Path,
    deps_root: &Path,
    interface_names: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let (resolve, package_id) = parse_dependency_wit_with_deps(dependency_wit_root, deps_root)
        .with_context(|| {
            format!(
                "failed to parse hydrated WIT package for dependency '{}' at {}",
                dependency_name,
                dependency_wit_root.display()
            )
        })?;

    for interface_name in interface_names {
        let interface_id =
            find_interface_id(&resolve, package_id, interface_name).ok_or_else(|| {
                anyhow!(
                    "binding interface '{} / {}' was not found in dependency WIT at {}",
                    dependency_name,
                    interface_name,
                    dependency_wit_root.display()
                )
            })?;
        ensure_interface_contains_no_resource(
            &resolve,
            interface_id,
            dependency_name,
            interface_name,
        )?;
    }

    let canonical = render_wit_package_text(&resolve, package_id).with_context(|| {
        format!(
            "failed to render canonical WIT package for dependency '{}' from {}",
            dependency_name,
            dependency_wit_root.display()
        )
    })?;
    let rewritten = rewrite_interfaces_in_wit_text(canonical, interface_names)?;
    persist_rewritten_dependency_wit(dependency_wit_root, &rewritten)?;
    Ok(())
}

fn parse_dependency_wit_with_deps(
    dependency_wit_root: &Path,
    deps_root: &Path,
) -> anyhow::Result<(Resolve, PackageId)> {
    let temp_root =
        std::env::temp_dir().join(format!("imago-update-rewrite-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_root)
        .with_context(|| format!("failed to create temp dir {}", temp_root.display()))?;

    let parse_result = (|| -> anyhow::Result<(Resolve, PackageId)> {
        copy_tree_recursive(dependency_wit_root, &temp_root).with_context(|| {
            format!(
                "failed to stage dependency WIT files from {}",
                dependency_wit_root.display()
            )
        })?;

        let deps_dst = temp_root.join("deps");
        fs::create_dir_all(&deps_dst)
            .with_context(|| format!("failed to create deps dir {}", deps_dst.display()))?;

        let self_name = dependency_wit_root.file_name().ok_or_else(|| {
            anyhow!(
                "failed to resolve dependency dir name for {}",
                dependency_wit_root.display()
            )
        })?;
        if deps_root.is_dir() {
            for entry in fs::read_dir(deps_root)
                .with_context(|| format!("failed to read {}", deps_root.display()))?
            {
                let entry = entry.with_context(|| {
                    format!(
                        "failed to read dependency entry under {}",
                        deps_root.display()
                    )
                })?;
                if entry.file_name() == self_name {
                    continue;
                }
                copy_tree_recursive(&entry.path(), &deps_dst.join(entry.file_name()))
                    .with_context(|| {
                        format!(
                            "failed to stage transitive dependency {}",
                            entry.path().display()
                        )
                    })?;
            }
        }

        let mut resolve = Resolve::default();
        let (package_id, _) = resolve.push_dir(&temp_root).with_context(|| {
            format!(
                "failed to parse staged dependency package {}",
                temp_root.display()
            )
        })?;
        Ok((resolve, package_id))
    })();

    let _ = fs::remove_dir_all(&temp_root);
    parse_result
}

fn copy_tree_recursive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to inspect source path {}", source.display()))?;
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed to copy file {} -> {}",
                source.display(),
                destination.display()
            )
        })?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(anyhow!(
            "source path is not file or dir: {}",
            source.display()
        ));
    }

    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create directory {}", destination.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read directory {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry under {}", source.display()))?;
        let entry_path = entry.path();
        let file_name = entry.file_name();
        copy_tree_recursive(&entry_path, &destination.join(file_name))?;
    }
    Ok(())
}

fn find_interface_id(
    resolve: &Resolve,
    package_id: PackageId,
    interface_name: &str,
) -> Option<InterfaceId> {
    resolve.packages[package_id]
        .interfaces
        .get(interface_name)
        .copied()
}

fn ensure_interface_contains_no_resource(
    resolve: &Resolve,
    interface_id: InterfaceId,
    dependency_name: &str,
    interface_name: &str,
) -> anyhow::Result<()> {
    let interface = &resolve.interfaces[interface_id];
    for function in interface.functions.values() {
        if !matches!(
            function.kind,
            FunctionKind::Freestanding | FunctionKind::AsyncFreestanding
        ) {
            return Err(anyhow!(
                "binding WIT '{}/{}' contains resource methods; `imago deps sync` does not support resources",
                dependency_name,
                interface_name
            ));
        }
        for param in &function.params {
            if type_contains_resource(resolve, &param.ty, &mut BTreeSet::new()) {
                return Err(anyhow!(
                    "binding WIT '{}/{}' contains resource types; `imago deps sync` does not support resources",
                    dependency_name,
                    interface_name
                ));
            }
        }
        if let Some(result) = &function.result
            && type_contains_resource(resolve, result, &mut BTreeSet::new())
        {
            return Err(anyhow!(
                "binding WIT '{}/{}' contains resource types; `imago deps sync` does not support resources",
                dependency_name,
                interface_name
            ));
        }
    }
    for type_id in interface.types.values() {
        if type_id_contains_resource(resolve, *type_id, &mut BTreeSet::new()) {
            return Err(anyhow!(
                "binding WIT '{}/{}' contains resource definitions; `imago deps sync` does not support resources",
                dependency_name,
                interface_name
            ));
        }
    }
    Ok(())
}

fn type_contains_resource(resolve: &Resolve, ty: &Type, seen: &mut BTreeSet<TypeId>) -> bool {
    match ty {
        Type::Id(type_id) => type_id_contains_resource(resolve, *type_id, seen),
        _ => false,
    }
}

fn type_id_contains_resource(
    resolve: &Resolve,
    type_id: TypeId,
    seen: &mut BTreeSet<TypeId>,
) -> bool {
    if !seen.insert(type_id) {
        return false;
    }
    let typedef = &resolve.types[type_id];
    match &typedef.kind {
        TypeDefKind::Resource | TypeDefKind::Handle(_) => true,
        TypeDefKind::Record(record) => record
            .fields
            .iter()
            .any(|field| type_contains_resource(resolve, &field.ty, seen)),
        TypeDefKind::Tuple(tuple) => tuple
            .types
            .iter()
            .any(|item| type_contains_resource(resolve, item, seen)),
        TypeDefKind::Variant(variant) => variant.cases.iter().any(|case| {
            case.ty
                .as_ref()
                .is_some_and(|ty| type_contains_resource(resolve, ty, seen))
        }),
        TypeDefKind::Option(ty) => type_contains_resource(resolve, ty, seen),
        TypeDefKind::Result(result) => {
            result
                .ok
                .as_ref()
                .is_some_and(|ty| type_contains_resource(resolve, ty, seen))
                || result
                    .err
                    .as_ref()
                    .is_some_and(|ty| type_contains_resource(resolve, ty, seen))
        }
        TypeDefKind::List(ty) => type_contains_resource(resolve, ty, seen),
        TypeDefKind::Map(key, value) => {
            type_contains_resource(resolve, key, seen)
                || type_contains_resource(resolve, value, seen)
        }
        TypeDefKind::FixedLengthList(ty, _) => type_contains_resource(resolve, ty, seen),
        TypeDefKind::Future(ty) | TypeDefKind::Stream(ty) => ty
            .as_ref()
            .is_some_and(|item| type_contains_resource(resolve, item, seen)),
        TypeDefKind::Type(ty) => type_contains_resource(resolve, ty, seen),
        TypeDefKind::Flags(_) | TypeDefKind::Enum(_) | TypeDefKind::Unknown => false,
    }
}

fn render_wit_package_text(resolve: &Resolve, package_id: PackageId) -> anyhow::Result<String> {
    let mut printer = WitPrinter::default();
    printer
        .print(resolve, package_id, &[])
        .context("failed to print WIT package")?;
    Ok(printer.output.to_string())
}

fn rewrite_interfaces_in_wit_text(
    package_text: String,
    interface_names: &BTreeSet<String>,
) -> anyhow::Result<String> {
    let mut lines = package_text.lines().map(str::to_string).collect::<Vec<_>>();
    for interface_name in interface_names {
        rewrite_one_interface_block(&mut lines, interface_name)?;
    }
    Ok(lines.join("\n") + "\n")
}

fn rewrite_one_interface_block(
    lines: &mut Vec<String>,
    interface_name: &str,
) -> anyhow::Result<()> {
    let open_index = lines
        .iter()
        .position(|line| {
            let trimmed = line.trim();
            if !(trimmed.starts_with("interface ") && trimmed.ends_with('{')) {
                return false;
            }
            let name = trimmed
                .trim_start_matches("interface ")
                .trim_end_matches('{')
                .trim();
            name == interface_name
        })
        .ok_or_else(|| {
            anyhow!(
                "interface '{}' was not found in rendered WIT package",
                interface_name
            )
        })?;

    let close_index = (open_index + 1..lines.len())
        .find(|idx| lines[*idx].trim() == "}")
        .ok_or_else(|| anyhow!("interface '{}' block is not closed", interface_name))?;

    let has_use_line = lines[open_index + 1..close_index]
        .iter()
        .any(|line| line.trim() == IMAGO_NODE_CONNECTION_USE);
    if !has_use_line {
        lines.insert(open_index + 1, format!("    {IMAGO_NODE_CONNECTION_USE}"));
    }

    let close_index = (open_index + 1..lines.len())
        .find(|idx| lines[*idx].trim() == "}")
        .ok_or_else(|| anyhow!("interface '{}' block is not closed", interface_name))?;
    for line in lines.iter_mut().take(close_index).skip(open_index + 1) {
        if let Some(rewritten) = rewrite_function_signature_line(line)? {
            *line = rewritten;
        }
    }
    Ok(())
}

fn rewrite_function_signature_line(line: &str) -> anyhow::Result<Option<String>> {
    let indent_len = line.chars().take_while(|ch| ch.is_whitespace()).count();
    let indent = &line[..indent_len];
    let trimmed = line.trim();
    if !trimmed.ends_with(';') {
        return Ok(None);
    }
    let Some((name, after_name)) = trimmed.split_once(':') else {
        return Ok(None);
    };
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let mut rest = after_name.trim_start();
    let async_prefix = if rest.starts_with("async ") {
        rest = rest["async ".len()..].trim_start();
        "async "
    } else {
        ""
    };
    if !rest.starts_with("func(") {
        return Ok(None);
    }
    let after_open = &rest["func(".len()..];
    let Some(params_end) = after_open.find(')') else {
        return Err(anyhow!("failed to parse function signature '{}'", trimmed));
    };
    let params = after_open[..params_end].trim();
    let after_params = after_open[params_end + 1..].trim_start();
    let after_params = after_params
        .strip_suffix(';')
        .ok_or_else(|| anyhow!("failed to parse function signature '{}'", trimmed))?
        .trim();
    let return_type = if after_params.is_empty() {
        None
    } else if let Some(raw) = after_params.strip_prefix("->") {
        Some(raw.trim())
    } else {
        return Ok(None);
    };

    if params.starts_with("connection:") {
        return Ok(None);
    }

    let params = if params.is_empty() {
        "connection: borrow<connection>".to_string()
    } else {
        format!("connection: borrow<connection>, {params}")
    };
    let wrapped_result = match return_type {
        Some(result_type) => format!("result<{result_type}, string>"),
        None => "result<_, string>".to_string(),
    };
    Ok(Some(format!(
        "{indent}{name}: {async_prefix}func({params}) -> {wrapped_result};"
    )))
}

fn persist_rewritten_dependency_wit(
    dependency_wit_root: &Path,
    package_text: &str,
) -> anyhow::Result<()> {
    fs::create_dir_all(dependency_wit_root)
        .with_context(|| format!("failed to create {}", dependency_wit_root.display()))?;
    let package_wit_path = dependency_wit_root.join("package.wit");
    fs::write(&package_wit_path, package_text)
        .with_context(|| format!("failed to write {}", package_wit_path.display()))?;
    remove_other_wit_files(dependency_wit_root, &package_wit_path)?;
    Ok(())
}

fn remove_other_wit_files(root: &Path, keep: &Path) -> anyhow::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", root.display()))?;
        let path = entry.path();
        if path == keep {
            continue;
        }
        let metadata =
            fs::metadata(&path).with_context(|| format!("failed to inspect {}", path.display()))?;
        if metadata.is_dir() {
            remove_other_wit_files(&path, keep)?;
            continue;
        }
        if metadata.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wit"))
        {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn has_top_level_wit_files(wit_dir: &Path) -> anyhow::Result<bool> {
    if !wit_dir.is_dir() {
        return Ok(false);
    }
    for entry in
        fs::read_dir(wit_dir).with_context(|| format!("failed to read {}", wit_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", wit_dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if file_type.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        if Path::new(&file_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wit"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect_foreign_package_versions_from_unresolved(
    unresolved: &UnresolvedPackageGroup,
    source_label: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut package_versions = BTreeMap::<String, String>::new();
    for package in std::iter::once(&unresolved.main).chain(unresolved.nested.iter()) {
        for foreign_package in package.foreign_deps.keys() {
            let package_name = format!("{}:{}", foreign_package.namespace, foreign_package.name);
            let version = foreign_package
                .version
                .as_ref()
                .map(ToString::to_string)
                .ok_or_else(|| {
                    anyhow!(
                        "{} references package '{}' without explicit version; add '@<version>' to import/export/include",
                        source_label,
                        package_name
                    )
                })?;
            merge_foreign_package_version(
                &mut package_versions,
                &package_name,
                &version,
                source_label,
            )?;
        }
    }
    Ok(package_versions)
}

fn collect_foreign_package_versions_from_wit_dir(
    wit_dir: &Path,
) -> anyhow::Result<BTreeMap<String, String>> {
    let unresolved = UnresolvedPackageGroup::parse_dir(wit_dir).with_context(|| {
        format!(
            "failed to parse WIT package directory {}",
            wit_dir.display()
        )
    })?;
    collect_foreign_package_versions_from_unresolved(
        &unresolved,
        &format!("WIT package directory '{}'", wit_dir.display()),
    )
}

fn merge_foreign_package_version(
    versions: &mut BTreeMap<String, String>,
    package: &str,
    version: &str,
    source_label: &str,
) -> anyhow::Result<()> {
    if let Some(existing) = versions.get(package) {
        if existing != version {
            return Err(anyhow!(
                "{} references wasi package '{}' with conflicting versions '{}' and '{}'",
                source_label,
                package,
                existing,
                version
            ));
        }
        return Ok(());
    }
    versions.insert(package.to_string(), version.to_string());
    Ok(())
}

fn collect_component_world_wasi_packages_and_validate_non_wasi_references(
    dependencies: &[build::ProjectDependency],
    cache_entries_by_dependency: &BTreeMap<String, dependency_cache::DependencyCacheEntry>,
    dependency_versions_by_effective_name: &BTreeMap<String, String>,
) -> anyhow::Result<ComponentWorldReferenceSummary> {
    let mut referenced_package_versions = BTreeMap::<String, String>::new();
    let mut non_wasi_interfaces_by_key =
        BTreeMap::<(String, String, String), BTreeSet<String>>::new();

    for dependency in dependencies {
        let cache_entry = cache_entries_by_dependency
            .get(&dependency.name)
            .ok_or_else(|| {
                anyhow!(
                    "dependency '{}' cache entry is missing during component world validation",
                    dependency.name
                )
            })?;
        for package in &cache_entry.component_world_foreign_packages {
            let version = package.version.as_deref().ok_or_else(|| {
                anyhow!(
                    "component world package in dependency '{}' references package '{}' without explicit version; add '@<version>' to import/export/include",
                    dependency.name,
                    package.name
                )
            })?;
            merge_foreign_package_version(
                &mut referenced_package_versions,
                &package.name,
                version,
                &format!(
                    "component world package in dependency '{}'",
                    dependency.name
                ),
            )?;
            if package
                .name
                .split_once(':')
                .is_some_and(|(namespace, _)| namespace == "wasi")
            {
                continue;
            }

            let Some(expected_version) = dependency_versions_by_effective_name.get(&package.name)
            else {
                return Err(anyhow!(
                    "component world package in dependency '{}' references non-wasi package '{}' which is not declared in [[dependencies]]",
                    dependency.name,
                    package.name
                ));
            };
            if expected_version != version {
                return Err(anyhow!(
                    "component world package in dependency '{}' references non-wasi package '{}@{}' but [[dependencies]] declares '{}@{}'",
                    dependency.name,
                    package.name,
                    version,
                    package.name,
                    expected_version
                ));
            }
            if package.interfaces.is_empty() {
                return Err(anyhow!(
                    "component world package in dependency '{}' references non-wasi package '{}@{}' but no interface names were recorded",
                    dependency.name,
                    package.name,
                    version
                ));
            }
            non_wasi_interfaces_by_key
                .entry((
                    dependency.name.clone(),
                    package.name.clone(),
                    version.to_string(),
                ))
                .or_default()
                .extend(package.interfaces.iter().cloned());
        }
    }

    Ok(ComponentWorldReferenceSummary {
        referenced_package_versions,
        non_wasi_interface_requirements: non_wasi_interfaces_by_key
            .into_iter()
            .map(|((dependency_name, package_name, version), interfaces)| {
                ComponentWorldNonWasiInterfaceRequirement {
                    dependency_name,
                    package_name,
                    version,
                    interfaces,
                }
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HydratedWitPackageMetadata {
    package_name: String,
    package_version: Option<String>,
    interface_names: BTreeSet<String>,
    foreign_packages: BTreeMap<String, String>,
}

fn read_hydrated_wit_package_metadata(
    project_root: &Path,
    package_name: &str,
    package_version: &str,
) -> anyhow::Result<HydratedWitPackageMetadata> {
    let package_root =
        project_root
            .join("wit")
            .join("deps")
            .join(plugin_sources::wit_deps_dir_name(
                package_name,
                Some(package_version),
            ));
    if !has_top_level_wit_files(&package_root)? {
        return Err(anyhow!(
            "hydrated dependency WIT for package '{}@{}' is missing at {}",
            package_name,
            package_version,
            package_root.display()
        ));
    }
    let unresolved = UnresolvedPackageGroup::parse_dir(&package_root).with_context(|| {
        format!(
            "failed to parse hydrated WIT package '{}@{}' in {}",
            package_name,
            package_version,
            package_root.display()
        )
    })?;
    let foreign_packages = collect_foreign_package_versions_from_unresolved(
        &unresolved,
        &format!(
            "hydrated WIT package '{}@{}' in {}",
            package_name,
            package_version,
            package_root.display()
        ),
    )?;
    Ok(HydratedWitPackageMetadata {
        package_name: format!(
            "{}:{}",
            unresolved.main.name.namespace, unresolved.main.name.name
        ),
        package_version: unresolved
            .main
            .name
            .version
            .as_ref()
            .map(ToString::to_string),
        interface_names: unresolved
            .main
            .interfaces
            .iter()
            .filter_map(|(_, interface)| interface.name.clone())
            .collect(),
        foreign_packages,
    })
}

fn validate_component_world_non_wasi_interface_requirements(
    project_root: &Path,
    requirements: &[ComponentWorldNonWasiInterfaceRequirement],
) -> anyhow::Result<()> {
    if requirements.is_empty() {
        return Ok(());
    }
    let mut metadata_by_package = BTreeMap::<(String, String), HydratedWitPackageMetadata>::new();
    for requirement in requirements {
        let metadata_key = (
            requirement.package_name.clone(),
            requirement.version.clone(),
        );
        let metadata = if let Some(metadata) = metadata_by_package.get(&metadata_key) {
            metadata
        } else {
            let metadata = read_hydrated_wit_package_metadata(
                project_root,
                &requirement.package_name,
                &requirement.version,
            )?;
            metadata_by_package.insert(metadata_key.clone(), metadata);
            metadata_by_package
                .get(&metadata_key)
                .expect("just inserted")
        };
        if metadata.package_name != requirement.package_name {
            return Err(anyhow!(
                "component world package in dependency '{}' references non-wasi package '{}@{}' but hydrated package is '{}'",
                requirement.dependency_name,
                requirement.package_name,
                requirement.version,
                metadata.package_name
            ));
        }
        if metadata.package_version.as_deref() != Some(requirement.version.as_str()) {
            return Err(anyhow!(
                "component world package in dependency '{}' references non-wasi package '{}@{}' but hydrated package version is '{}'",
                requirement.dependency_name,
                requirement.package_name,
                requirement.version,
                metadata
                    .package_version
                    .as_deref()
                    .unwrap_or("<unspecified>")
            ));
        }
        for interface in &requirement.interfaces {
            if !metadata.interface_names.contains(interface) {
                return Err(anyhow!(
                    "component world package in dependency '{}' references non-wasi interface '{}/{}@{}' but hydrated WIT package in '{}' does not define that interface",
                    requirement.dependency_name,
                    requirement.package_name,
                    interface,
                    requirement.version,
                    plugin_sources::path_to_manifest_string(
                        &PathBuf::from("wit")
                            .join("deps")
                            .join(plugin_sources::wit_deps_dir_name(
                                &requirement.package_name,
                                Some(requirement.version.as_str())
                            ))
                    ),
                ));
            }
        }
    }
    Ok(())
}

async fn hydrate_wasi_packages_from_component_and_wit_package_dir(
    project_root: &Path,
    mut referenced_package_versions: BTreeMap<String, String>,
    dependency_versions_by_effective_name: &BTreeMap<String, String>,
) -> anyhow::Result<Vec<TransitivePackageRecord>> {
    let wit_dir = project_root.join("wit");
    if has_top_level_wit_files(&wit_dir)? {
        let wit_dir_versions = collect_foreign_package_versions_from_wit_dir(&wit_dir)?;
        for (package, version) in wit_dir_versions {
            merge_foreign_package_version(
                &mut referenced_package_versions,
                &package,
                &version,
                &format!("WIT package directory '{}'", wit_dir.display()),
            )?;
        }
    }

    if referenced_package_versions.is_empty() {
        return Ok(Vec::new());
    }

    let deps_root = wit_dir.join("deps");
    fs::create_dir_all(&deps_root)
        .with_context(|| format!("failed to create {}", deps_root.display()))?;
    let mut records = Vec::new();

    let mut queue: VecDeque<(String, String, String)> = referenced_package_versions
        .iter()
        .map(|(package, version)| {
            (
                package.clone(),
                version.clone(),
                "seed package reference".to_string(),
            )
        })
        .collect();
    let mut visited_wasi = BTreeSet::<(String, String)>::new();
    let mut visited_non_wasi = BTreeSet::<(String, String)>::new();

    while let Some((package, version, source_label)) = queue.pop_front() {
        let namespace = package.split_once(':').map(|(ns, _)| ns).ok_or_else(|| {
            anyhow!(
                "{} references package '{}' with invalid package name format",
                source_label,
                package
            )
        })?;
        if namespace != "wasi" {
            let Some(expected_version) = dependency_versions_by_effective_name.get(&package) else {
                return Err(anyhow!(
                    "{} references non-wasi package '{}@{}' which is not declared in [[dependencies]]",
                    source_label,
                    package,
                    version
                ));
            };
            if expected_version != &version {
                return Err(anyhow!(
                    "{} references non-wasi package '{}@{}' but [[dependencies]] declares '{}@{}'",
                    source_label,
                    package,
                    version,
                    package,
                    expected_version
                ));
            }
            if !visited_non_wasi.insert((package.clone(), version.clone())) {
                continue;
            }
            let metadata =
                read_hydrated_wit_package_metadata(project_root, &package, version.as_str())?;
            if metadata.package_name != package {
                return Err(anyhow!(
                    "{} references non-wasi package '{}@{}' but hydrated package is '{}'",
                    source_label,
                    package,
                    version,
                    metadata.package_name
                ));
            }
            if metadata.package_version.as_deref() != Some(version.as_str()) {
                return Err(anyhow!(
                    "{} references non-wasi package '{}@{}' but hydrated package version is '{}'",
                    source_label,
                    package,
                    version,
                    metadata
                        .package_version
                        .as_deref()
                        .unwrap_or("<unspecified>")
                ));
            }
            for (foreign_package, foreign_version) in metadata.foreign_packages {
                merge_foreign_package_version(
                    &mut referenced_package_versions,
                    &foreign_package,
                    &foreign_version,
                    &format!(
                        "transitive dependency of non-wasi package '{}@{}'",
                        package, version
                    ),
                )?;
                queue.push_back((
                    foreign_package,
                    foreign_version,
                    format!(
                        "transitive dependency of non-wasi package '{}@{}'",
                        package, version
                    ),
                ));
            }
            continue;
        }
        if !visited_wasi.insert((package.clone(), version.clone())) {
            continue;
        }

        let source = package.to_string();
        let destination = deps_root.join(plugin_sources::wit_deps_dir_name(
            &package,
            Some(version.as_str()),
        ));
        let temp_root =
            std::env::temp_dir().join(format!("imago-update-wasi-{}", uuid::Uuid::new_v4()));
        let temp_destination = temp_root.join("package");
        fs::create_dir_all(&temp_destination).with_context(|| {
            format!(
                "failed to create temporary dir {}",
                temp_destination.display()
            )
        })?;
        let materialized = plugin_sources::materialize_wit_source(
            project_root,
            plugin_sources::SourceKind::Wit,
            &source,
            Some(&version),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY),
            None,
            Some(package.as_str()),
            None,
            &temp_destination,
        )
        .await
        .with_context(|| {
            format!(
                "failed to materialize wasi package '{}@{}' ({})",
                package, version, source_label
            )
        })?;
        let temp_package_wit_path = temp_destination.join("package.wit");
        if !temp_package_wit_path.is_file() {
            let _ = fs::remove_dir_all(&temp_root);
            return Err(anyhow!(
                "materialized wasi package '{}' is missing package.wit at {}",
                package,
                temp_package_wit_path.display()
            ));
        }
        let package_wit_bytes = fs::read(&temp_package_wit_path).with_context(|| {
            format!(
                "failed to read package.wit for materialized wasi package '{}'",
                package
            )
        })?;
        let _ = fs::remove_dir_all(&temp_root);

        fs::create_dir_all(&destination)
            .with_context(|| format!("failed to create {}", destination.display()))?;
        let package_wit_path = destination.join("package.wit");
        if package_wit_path.is_file() {
            let existing = fs::read(&package_wit_path).with_context(|| {
                format!("failed to read existing {}", package_wit_path.display())
            })?;
            if existing != package_wit_bytes {
                return Err(anyhow!(
                    "conflicting transitive WIT package detected at {}",
                    package_wit_path.display()
                ));
            }
        } else {
            fs::write(&package_wit_path, &package_wit_bytes)
                .with_context(|| format!("failed to write {}", package_wit_path.display()))?;
        }
        if !package_wit_path.is_file() {
            return Err(anyhow!(
                "materialized wasi package '{}' is missing package.wit at {}",
                package,
                package_wit_path.display()
            ));
        }

        for transitive in materialized.transitive_packages {
            let transitive_version = transitive.version.ok_or_else(|| {
                anyhow!(
                    "wasi package '{}@{}' references transitive package '{}' without explicit version",
                    package,
                    version,
                    transitive.name
                )
            })?;
            merge_foreign_package_version(
                &mut referenced_package_versions,
                &transitive.name,
                &transitive_version,
                &format!(
                    "transitive dependency of wasi package '{}@{}'",
                    package, version
                ),
            )?;
            queue.push_back((
                transitive.name.clone(),
                transitive_version.clone(),
                format!(
                    "transitive dependency of wasi package '{}@{}'",
                    package, version
                ),
            ));
        }

        let digest = format!("sha256:{}", hex::encode(Sha256::digest(package_wit_bytes)));
        records.push(TransitivePackageRecord {
            name: package.clone(),
            registry: Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY.to_string()),
            requirement: format!("={version}"),
            version: Some(version.clone()),
            digest,
            source: Some(source),
            path: plugin_sources::wit_deps_path(&package, Some(version.as_str())),
            via: String::new(),
        });
    }
    Ok(records)
}

async fn run_inner_async(project_root: &Path) -> anyhow::Result<UpdateSummary> {
    ui::command_stage(
        "deps.sync",
        "load-input",
        "loading dependencies and bindings",
    );
    let namespace_registries = build::load_namespace_registries(project_root)?;
    let dependency_resolver = StandardDependencyResolver;
    let dependencies = build::load_project_dependencies_with_namespace_registries(
        project_root,
        &namespace_registries,
    )?;
    let bindings = build::load_project_binding_sources_with_namespace_registries(
        project_root,
        &namespace_registries,
    )?;
    validate_wit_sources_outside_wit_deps(project_root, &dependencies, &bindings)?;
    validate_wit_output_path_collisions(&dependencies)?;

    let mut lock_entries = Vec::with_capacity(dependencies.len());
    let mut transitive_records = Vec::new();
    let mut cache_entries_by_dependency = BTreeMap::new();
    let mut dependency_versions_by_effective_name = BTreeMap::<String, String>::new();
    let mut dependency_by_effective_name = BTreeMap::<String, String>::new();

    ui::command_stage("deps.sync", "refresh-cache", "refreshing dependency cache");
    for dependency in &dependencies {
        let cache_entry = cache::load_or_refresh_cache_entry(
            &dependency_resolver,
            project_root,
            dependency,
            Some(&namespace_registries),
        )
        .await?;
        cache_entries_by_dependency.insert(dependency.name.clone(), cache_entry.clone());
        let effective_dependency_name = cache_entry
            .resolved_package_name
            .clone()
            .unwrap_or_else(|| dependency.name.clone());
        if let Some(existing_dependency) = dependency_by_effective_name
            .insert(effective_dependency_name.clone(), dependency.name.clone())
        {
            return Err(anyhow!(
                "dependencies '{}' and '{}' both resolve to package '{}'; dependency package names must be unique",
                existing_dependency,
                dependency.name,
                effective_dependency_name
            ));
        }
        dependency_versions_by_effective_name.insert(
            effective_dependency_name.clone(),
            dependency.version.clone(),
        );
        transitive_records.extend(cache_entry.transitive_packages.iter().map(|transitive| {
            TransitivePackageRecord {
                name: transitive.name.clone(),
                registry: transitive.registry.clone(),
                requirement: transitive.requirement.clone(),
                version: transitive.version.clone(),
                digest: transitive.digest.clone(),
                source: transitive.source.clone(),
                path: transitive.path.clone(),
                via: effective_dependency_name.clone(),
            }
        }));
        lock_entries.push(ImagoLockDependency {
            name: effective_dependency_name,
            version: dependency.version.clone(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_digest: cache_entry.wit_digest,
            wit_path: dependency_cache::dependency_wit_path(
                &cache_entry
                    .resolved_package_name
                    .clone()
                    .unwrap_or_else(|| dependency.name.clone()),
                &dependency.version,
            ),
            component_source: cache_entry.component_source,
            component_registry: cache_entry.component_registry,
            component_sha256: cache_entry.component_sha256,
        });
    }
    let component_world_reference_summary =
        collect_component_world_wasi_packages_and_validate_non_wasi_references(
            &dependencies,
            &cache_entries_by_dependency,
            &dependency_versions_by_effective_name,
        )?;

    ui::command_stage("deps.sync", "hydrate-wit", "hydrating wit/deps");
    dependency_cache::hydrate_project_wit_deps(
        project_root,
        &dependencies,
        Some(&namespace_registries),
    )?;
    ui::command_stage(
        "deps.sync",
        "hydrate-wasi-from-component-and-wit-dir",
        "hydrating wasi packages from component and wit package dir",
    );
    let mut auto_wasi_records = hydrate_wasi_packages_from_component_and_wit_package_dir(
        project_root,
        component_world_reference_summary
            .referenced_package_versions
            .clone(),
        &dependency_versions_by_effective_name,
    )
    .await?;
    transitive_records.append(&mut auto_wasi_records);
    ui::command_stage(
        "deps.sync",
        "validate-component-world-foreign-interfaces",
        "validating component world foreign interfaces",
    );
    validate_component_world_non_wasi_interface_requirements(
        project_root,
        &component_world_reference_summary.non_wasi_interface_requirements,
    )?;
    let resolved_binding_wits = resolve_binding_wits(
        project_root,
        &dependencies,
        &lock_entries,
        &bindings,
        Some(&namespace_registries),
    )
    .await?;
    rewrite_bound_wit_packages(project_root, &resolved_binding_wits)?;
    for entry in &mut lock_entries {
        let hydrated_path = project_root.join(&entry.wit_path);
        entry.wit_digest = build::compute_path_digest_hex(&hydrated_path).with_context(|| {
            format!(
                "failed to compute hydrated wit digest for dependency '{}' at {}",
                entry.name,
                hydrated_path.display()
            )
        })?;
    }

    let mut binding_wits = Vec::new();
    let mut seen_binding_keys = BTreeSet::<(String, String, Option<String>, String, String)>::new();
    for binding in resolved_binding_wits {
        let key = (
            binding.name.clone(),
            binding.wit_source.clone(),
            binding.wit_registry.clone(),
            binding.wit_version.clone(),
            binding.wit_path.clone(),
        );
        if !seen_binding_keys.insert(key) {
            continue;
        }
        let hydrated_path = project_root.join(&binding.wit_path);
        let wit_digest = build::compute_path_digest_hex(&hydrated_path).with_context(|| {
            format!(
                "failed to compute hydrated binding wit digest for '{}' at {}",
                binding.name,
                hydrated_path.display()
            )
        })?;
        binding_wits.push(ImagoLockBindingWit {
            name: binding.name,
            wit_source: binding.wit_source,
            wit_registry: binding.wit_registry,
            wit_version: binding.wit_version,
            wit_digest,
            wit_path: binding.wit_path,
            interfaces: package_interface_ids(&binding.package_name, &binding.interface_names),
        });
    }
    binding_wits.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.wit_source.cmp(&b.wit_source))
            .then(a.wit_path.cmp(&b.wit_path))
    });
    lock_entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    let dependency_count = lock_entries.len();
    let binding_wits_count = binding_wits.len();
    let lock = ImagoLock {
        version: IMAGO_LOCK_VERSION,
        dependencies: lock_entries,
        wit_packages: collect_wit_packages(transitive_records),
        binding_wits,
    };
    ui::command_stage("deps.sync", "write-lock", "writing imago.lock");
    save_to_project_root(project_root, &lock)?;

    Ok(UpdateSummary {
        dependencies: dependency_count,
        binding_wits: binding_wits_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_parser::Resolve;

    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn change_to(path: &Path) -> Self {
            let previous = std::env::current_dir().expect("current dir should be readable");
            std::env::set_current_dir(path).expect("current dir should be changeable");
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-cli-update-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn local_warg_package_root(root: &Path, package: &str, version: &str) -> PathBuf {
        root.join(".imago")
            .join("warg")
            .join(plugin_sources::warg_local_package_key(package))
            .join(version)
    }

    fn local_warg_file_path(root: &Path, package: &str, version: &str, file_name: &str) -> PathBuf {
        local_warg_package_root(root, package, version).join(file_name)
    }

    fn local_oci_package_key(registry: &str, package: &str) -> String {
        format!(
            "pkg-{}",
            hex::encode(format!("{registry}/{package}").as_bytes())
        )
    }

    fn local_oci_package_root(
        root: &Path,
        registry: &str,
        package: &str,
        version: &str,
    ) -> PathBuf {
        root.join(".imago")
            .join("oci")
            .join(local_oci_package_key(registry, package))
            .join(version)
    }

    fn local_oci_file_path(
        root: &Path,
        registry: &str,
        package: &str,
        version: &str,
        file_name: &str,
    ) -> PathBuf {
        local_oci_package_root(root, registry, package, version).join(file_name)
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(sha2::Sha256::digest(bytes))
    }

    fn encode_wit_package(root: &Path) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        wit_component::encode(&resolve, pkg).expect("fixture WIT package should encode")
    }

    fn encode_wit_component(root: &Path, world: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        let (pkg, _) = resolve
            .push_dir(root)
            .expect("fixture WIT directory should parse");
        let world_id = resolve
            .select_world(&[pkg], Some(world))
            .expect("fixture world should exist");
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world_id,
            wit_component::StringEncoding::UTF8,
        )
        .expect("component metadata embedding should succeed");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("component encoder should accept module")
            .encode()
            .expect("component encoding should succeed")
    }

    #[tokio::test]
    async fn update_resolves_file_source_into_wit_and_lock() {
        let root = new_temp_dir("file-source");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.version, 1);
        assert_eq!(lock.dependencies.len(), 1);
        assert!(lock.wit_packages.is_empty());
        let entry = &lock.dependencies[0];
        assert_eq!(entry.name, "test:example");
        assert_eq!(entry.wit_source, "registry/example");
        assert_eq!(entry.wit_registry, None);
        assert_eq!(entry.wit_path, "wit/deps/test-example-0.1.0");
        assert!(root.join(&entry.wit_path).exists());
        assert!(entry.component_source.is_none());
        assert!(entry.component_sha256.is_none());
        assert!(!entry.wit_digest.is_empty());

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(second.exit_code, 0);
        let lock_raw_2 =
            fs::read_to_string(root.join("imago.lock")).expect("lock should exist after rerun");
        let lock_2: ImagoLock = toml::from_str(&lock_raw_2).expect("lock should parse");
        assert_eq!(lock_2.dependencies[0].wit_digest, entry.wit_digest);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_uses_default_warg_source_when_wit_is_omitted() {
        let root = new_temp_dir("warg-default");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "native"
wit = "yieldspace:example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "yieldspace:example", "1.2.3", "wit.wit"),
            b"package yieldspace:example@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        assert_eq!(lock.dependencies[0].wit_source, "yieldspace:example");
        assert_eq!(
            lock.dependencies[0].wit_registry,
            Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string())
        );
        assert_eq!(
            lock.dependencies[0].wit_path,
            "wit/deps/yieldspace-example-1.2.3"
        );
        assert!(root.join(&lock.dependencies[0].wit_path).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_uses_wasi_default_registry_when_wit_is_omitted() {
        let root = new_temp_dir("warg-default-wasi-namespace");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.6"
kind = "native"
wit = "wasi:cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:cli", "0.2.6", "wit.wit"),
            b"package wasi:cli@0.2.6;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        assert_eq!(lock.dependencies[0].wit_source, "wasi:cli");
        assert_eq!(
            lock.dependencies[0].wit_registry.as_deref(),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_namespace_registry_overrides_wasi_default_when_wit_is_omitted() {
        let root = new_temp_dir("warg-default-wasi-overridden");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[namespace_registries]
wasi = "custom-wasi.example"

[[dependencies]]
version = "0.2.6"
kind = "native"
wit = "wasi:io"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:io", "0.2.6", "wit.wit"),
            b"package wasi:io@0.2.6;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        assert_eq!(lock.dependencies[0].wit_source, "wasi:io");
        assert_eq!(
            lock.dependencies[0].wit_registry.as_deref(),
            Some("custom-wasi.example")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_resolves_oci_wit_source_from_local_cache() {
        let root = new_temp_dir("oci-wit-local-cache");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.0"
kind = "native"
oci = "ghcr.io/chikoski/advent-of-spin"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_oci_file_path(
                &root,
                "ghcr.io",
                "chikoski:advent-of-spin",
                "0.2.0",
                "wit.wit",
            ),
            b"package chikoski:advent-of-spin@0.2.0;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        let entry = &lock.dependencies[0];
        assert_eq!(entry.wit_source, "ghcr.io/chikoski/advent-of-spin");
        assert_eq!(entry.wit_registry, None);
        assert_eq!(entry.wit_path, "wit/deps/chikoski-advent-of-spin-0.2.0");
        assert!(root.join(&entry.wit_path).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_accepts_warg_dependency_when_resolved_package_matches_dependency_name() {
        let root = new_temp_dir("warg-dependency-name-matches-resolved-package");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "native"
wit = "yieldspace:nanokvm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "yieldspace:nanokvm", "1.2.3", "wit.wit"),
            b"package yieldspace:nanokvm@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        let entry = &lock.dependencies[0];
        assert_eq!(entry.wit_source, "yieldspace:nanokvm".to_string());
        assert_eq!(entry.wit_path, "wit/deps/yieldspace-nanokvm-1.2.3");
        assert!(root.join(&entry.wit_path).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_accepts_oci_dependency_when_resolved_package_matches_dependency_name() {
        let root = new_temp_dir("oci-dependency-name-matches-resolved-package");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "native"
oci = "ghcr.io/yieldspace/nanokvm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_oci_file_path(&root, "ghcr.io", "yieldspace:nanokvm", "1.2.3", "wit.wit"),
            b"package yieldspace:nanokvm@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        let entry = &lock.dependencies[0];
        assert_eq!(entry.wit_source, "ghcr.io/yieldspace/nanokvm".to_string());
        assert_eq!(entry.wit_path, "wit/deps/yieldspace-nanokvm-1.2.3");
        assert!(root.join(&entry.wit_path).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_warg_dependency_when_resolved_package_mismatches_dependency_name() {
        let root = new_temp_dir("warg-dependency-name-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "native"
wit = "yieldspace:nanokvm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "yieldspace:nanokvm", "1.2.3", "wit.wit"),
            b"package yieldspace:other@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("top-level WIT package mismatch"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("yieldspace:nanokvm"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("yieldspace:other"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_oci_dependency_when_resolved_package_mismatches_dependency_name() {
        let root = new_temp_dir("oci-dependency-name-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "native"
oci = "ghcr.io/yieldspace/nanokvm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_oci_file_path(&root, "ghcr.io", "yieldspace:nanokvm", "1.2.3", "wit.wit"),
            b"package yieldspace:other@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("top-level WIT package mismatch"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("yieldspace:nanokvm"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("yieldspace:other"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_allows_binding_remote_source_with_source_package_mismatch() {
        let root = new_temp_dir("binding-remote-source-package-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-target"
version = "0.1.0"
wit = "yieldspace:nanokvm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "yieldspace:nanokvm", "0.1.0", "wit.wit"),
            br#"
package yieldspace:other@0.1.0;

interface greet {
  hello: func() -> string;
}
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.binding_wits.len(), 1);
        assert_eq!(lock.binding_wits[0].name, "svc-target");
        assert_eq!(
            lock.binding_wits[0].interfaces,
            vec!["yieldspace:other/greet".to_string()]
        );
        assert!(
            root.join("wit/deps/yieldspace-other-0.1.0/package.wit")
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_oci_wit_source_with_registry_key() {
        let root = new_temp_dir("oci-wit-registry-rejected");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.0"
kind = "native"
oci = "ghcr.io/chikoski/advent-of-spin"
registry = "wa.dev"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("dependencies[0].registry is not allowed when source kind is `oci`"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_oci_component_source_with_registry_key() {
        let root = new_temp_dir("oci-component-registry-rejected");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.0"
kind = "wasm"
path = "registry/example"

[dependencies.component]
oci = "ghcr.io/chikoski/advent-of-spin"
registry = "wa.dev"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains(
                "dependencies[0].component.registry is not allowed when source kind is `oci`"
            ),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_records_component_source_and_sha_and_materializes_dependency_component_cache() {
        let root = new_temp_dir("component-sha");
        let component_bytes = b"\0asmfake-component";
        let component_sha = sha256_hex(component_bytes);
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "1.2.3"
kind = "wasm"
path = "registry/example"

[dependencies.component]
path = "registry/example-component.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@1.2.3;\n",
        );
        write(
            &root.join("registry/example-component.wasm"),
            component_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        let entry = &lock.dependencies[0];
        assert_eq!(
            entry.component_source.as_deref(),
            Some("registry/example-component.wasm")
        );
        assert_eq!(entry.component_registry, None);
        assert_eq!(
            entry.component_sha256.as_deref(),
            Some(component_sha.as_str())
        );
        assert!(
            root.join(".imago/deps/path-source-0/meta.toml").exists(),
            "dependency cache metadata must be written"
        );
        assert!(
            root.join(".imago/deps/path-source-0/components")
                .join(format!("{component_sha}.wasm"))
                .exists(),
            "dependency component cache must be materialized"
        );
        assert!(
            !root
                .join(".imago/components")
                .join(format!("{component_sha}.wasm"))
                .exists(),
            "update must not materialize deploy-time shared component cache"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_derives_component_info_from_wit_component_source() {
        let root = new_temp_dir("wit-component-derived");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
wit = "root:component"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-component");
        write(
            &fixture_wit_root.join("package.wit"),
            br#"
package root:component@0.1.0;

world plugin {
}
"#,
        );
        let component_bytes = encode_wit_component(&fixture_wit_root, "plugin");
        let expected_sha = sha256_hex(&component_bytes);
        write(
            &local_warg_file_path(&root, "root:component", "0.1.0", "wit.wasm"),
            &component_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        let entry = lock
            .dependencies
            .iter()
            .find(|entry| entry.name == "root:component")
            .expect("dependency lock entry should exist");
        assert_eq!(entry.component_source.as_deref(), Some("root:component"));
        assert_eq!(
            entry.component_registry.as_deref(),
            Some(plugin_sources::DEFAULT_WARG_REGISTRY)
        );
        assert_eq!(
            entry.component_sha256.as_deref(),
            Some(expected_sha.as_str())
        );
        assert!(
            root.join(".imago/deps/root-component/components")
                .join(format!("{expected_sha}.wasm"))
                .exists(),
            "derived component bytes must be stored in dependency cache"
        );
        assert!(
            root.join("wit/deps/root-component-0.1.0/package.wit")
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_root_component_with_export_interfaces_only() {
        let root = new_temp_dir("root-component-export-only");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
wit = "root:component"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:name"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let root_component_fixture = root.join("fixture-root-component");
        write(
            &root_component_fixture.join("package.wit"),
            br#"
package root:component@0.1.0;

interface imported {
  ping: func();
}

interface exported {
}

world plugin {
  import chikoski:name/name-provider@0.1.0;
  import imported;
  export exported;
}
"#,
        );
        write(
            &root_component_fixture.join("deps/chikoski-name/package.wit"),
            br#"
package chikoski:name@0.1.0;

interface name-provider {
  get-name: func() -> string;
}
"#,
        );
        write(
            &local_warg_file_path(&root, "root:component", "0.1.0", "wit.wasm"),
            &encode_wit_component(&root_component_fixture, "plugin"),
        );

        let chikoski_name_fixture = root.join("fixture-chikoski-name");
        write(
            &chikoski_name_fixture.join("package.wit"),
            br#"
package chikoski:name@0.1.0;

interface name-provider {
  get-name: func() -> string;
}
"#,
        );
        write(
            &local_warg_file_path(&root, "chikoski:name", "0.1.0", "wit.wasm"),
            &encode_wit_package(&chikoski_name_fixture),
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let package_text =
            fs::read_to_string(root.join("wit/deps/root-component-0.1.0/package.wit"))
                .expect("root:component package.wit should exist");
        assert!(
            package_text.contains("interface exported"),
            "exported interface must remain: {package_text}"
        );
        assert!(
            package_text.contains("export exported;"),
            "world export must remain: {package_text}"
        );
        assert!(
            !package_text.contains("interface imported"),
            "import-only local interface must be removed: {package_text}"
        );
        assert!(
            !package_text.contains("import chikoski:name/name-provider"),
            "world import must be removed: {package_text}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validate_component_world_non_wasi_interface_requirements_rejects_missing_interface() {
        let root = new_temp_dir("component-world-non-wasi-interface-missing");
        write(
            &root.join("wit/deps/chikoski-name-0.1.0/package.wit"),
            br#"
package chikoski:name@0.1.0;

interface other-provider {
  get-name: func() -> string;
}
"#,
        );
        let requirements = vec![ComponentWorldNonWasiInterfaceRequirement {
            dependency_name: "root:component".to_string(),
            package_name: "chikoski:name".to_string(),
            version: "0.1.0".to_string(),
            interfaces: BTreeSet::from(["name-provider".to_string()]),
        }];
        let err = validate_component_world_non_wasi_interface_requirements(&root, &requirements)
            .expect_err("missing non-wasi interface must fail");
        assert!(
            err.to_string().contains("does not define that interface"),
            "unexpected error: {err:#}"
        );
        assert!(
            err.to_string()
                .contains("chikoski:name/name-provider@0.1.0"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_wasi_packages_from_component_versions_and_wit_dir_merge() {
        let root = new_temp_dir("component-wasi-wit-dir-merge");
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world plugin {
  import wasi:random/random@0.2.6;
}
"#,
        );
        let wasi_random_fixture = root.join("fixture-wasi-random-merge");
        write(
            &wasi_random_fixture.join("package.wit"),
            br#"
package wasi:random@0.2.6;

interface random {
  get-random-bytes: func(len: u64) -> list<u8>;
}
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:random", "0.2.6", "wit.wasm"),
            &encode_wit_package(&wasi_random_fixture),
        );

        let records = hydrate_wasi_packages_from_component_and_wit_package_dir(
            &root,
            BTreeMap::from([("wasi:random".to_string(), "0.2.6".to_string())]),
            &BTreeMap::new(),
        )
        .await
        .expect("wasi hydration should succeed");

        assert!(
            root.join("wit/deps/wasi-random-0.2.6/package.wit")
                .is_file()
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "wasi:random");
        assert!(records[0].via.is_empty());
        assert_eq!(
            records[0].registry.as_deref(),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_rejects_component_world_non_wasi_package_missing_from_dependencies() {
        let dependencies = vec![build::ProjectDependency {
            name: "root:component".to_string(),
            version: "0.1.0".to_string(),
            kind: build::ManifestDependencyKind::Wasm,
            wit: build::ProjectDependencySource {
                source: "warg://root:component@0.1.0".to_string(),
                source_kind: plugin_sources::SourceKind::Wit,
                registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                sha256: None,
            },
            requires: vec![],
            component: None,
            capabilities: build::ManifestCapabilityPolicy::default(),
        }];
        let cache_entries = BTreeMap::from([(
            "root:component".to_string(),
            dependency_cache::DependencyCacheEntry {
                name: "root:component".to_string(),
                resolved_package_name: None,
                version: "0.1.0".to_string(),
                kind: "wasm".to_string(),
                wit_source: "warg://root:component@0.1.0".to_string(),
                wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                wit_sha256: None,
                wit_path: "wit/deps/root-component-0.1.0".to_string(),
                wit_digest: "deadbeef".to_string(),
                wit_source_fingerprint: None,
                component_source: Some("warg://root:component@0.1.0".to_string()),
                component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                component_sha256: Some("0".repeat(64)),
                component_source_fingerprint: None,
                component_world_foreign_packages: vec![
                    dependency_cache::DependencyCacheComponentWorldForeignPackage {
                        name: "chikoski:name".to_string(),
                        version: Some("0.1.0".to_string()),
                        interfaces: vec!["name-provider".to_string()],
                        interfaces_recorded: true,
                    },
                ],
                component_world_foreign_packages_recorded: true,
                transitive_packages: vec![],
            },
        )]);

        let err = collect_component_world_wasi_packages_and_validate_non_wasi_references(
            &dependencies,
            &cache_entries,
            &BTreeMap::from([("root:component".to_string(), "0.1.0".to_string())]),
        )
        .expect_err("missing non-wasi dependency must fail");
        assert!(
            err.to_string()
                .contains("non-wasi package 'chikoski:name' which is not declared"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn update_rejects_component_world_non_wasi_package_version_mismatch() {
        let dependencies = vec![
            build::ProjectDependency {
                name: "root:component".to_string(),
                version: "0.1.0".to_string(),
                kind: build::ManifestDependencyKind::Wasm,
                wit: build::ProjectDependencySource {
                    source: "warg://root:component@0.1.0".to_string(),
                    source_kind: plugin_sources::SourceKind::Wit,
                    registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    sha256: None,
                },
                requires: vec![],
                component: None,
                capabilities: build::ManifestCapabilityPolicy::default(),
            },
            build::ProjectDependency {
                name: "chikoski:name".to_string(),
                version: "0.2.0".to_string(),
                kind: build::ManifestDependencyKind::Native,
                wit: build::ProjectDependencySource {
                    source: "warg://chikoski:name@0.2.0".to_string(),
                    source_kind: plugin_sources::SourceKind::Wit,
                    registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                    sha256: None,
                },
                requires: vec![],
                component: None,
                capabilities: build::ManifestCapabilityPolicy::default(),
            },
        ];
        let cache_entries = BTreeMap::from([(
            "root:component".to_string(),
            dependency_cache::DependencyCacheEntry {
                name: "root:component".to_string(),
                resolved_package_name: None,
                version: "0.1.0".to_string(),
                kind: "wasm".to_string(),
                wit_source: "warg://root:component@0.1.0".to_string(),
                wit_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                wit_sha256: None,
                wit_path: "wit/deps/root-component-0.1.0".to_string(),
                wit_digest: "deadbeef".to_string(),
                wit_source_fingerprint: None,
                component_source: Some("warg://root:component@0.1.0".to_string()),
                component_registry: Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string()),
                component_sha256: Some("0".repeat(64)),
                component_source_fingerprint: None,
                component_world_foreign_packages: vec![
                    dependency_cache::DependencyCacheComponentWorldForeignPackage {
                        name: "chikoski:name".to_string(),
                        version: Some("0.1.0".to_string()),
                        interfaces: vec!["name-provider".to_string()],
                        interfaces_recorded: true,
                    },
                ],
                component_world_foreign_packages_recorded: true,
                transitive_packages: vec![],
            },
        )]);

        let err = collect_component_world_wasi_packages_and_validate_non_wasi_references(
            &dependencies,
            &cache_entries,
            &BTreeMap::from([
                ("root:component".to_string(), "0.1.0".to_string()),
                ("chikoski:name".to_string(), "0.2.0".to_string()),
            ]),
        )
        .expect_err("non-wasi version mismatch must fail");
        assert!(
            err.to_string()
                .contains("references non-wasi package 'chikoski:name@0.1.0'"),
            "unexpected error: {err:#}"
        );
        assert!(
            err.to_string()
                .contains("[[dependencies]] declares 'chikoski:name@0.2.0'"),
            "unexpected error: {err:#}"
        );
    }

    #[tokio::test]
    async fn update_rejects_wasi_version_conflict_between_component_world_and_wit_dir() {
        let root = new_temp_dir("component-world-wasi-wit-dir-conflict");
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world plugin {
  import wasi:random/random@0.2.7;
}
"#,
        );

        let err = hydrate_wasi_packages_from_component_and_wit_package_dir(
            &root,
            BTreeMap::from([("wasi:random".to_string(), "0.2.6".to_string())]),
            &BTreeMap::new(),
        )
        .await
        .expect_err("conflicting component/wit-dir wasi versions must fail");
        assert!(
            err.to_string()
                .contains("conflicting versions '0.2.6' and '0.2.7'"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_component_dependency_when_expected_package_is_missing_in_resolve() {
        let root = new_temp_dir("wit-component-expected-package-missing");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:missing"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-component-expected-package-missing");
        write(
            &fixture_wit_root.join("package.wit"),
            br#"
package root:component@0.1.0;

world plugin {
  import chikoski:name/name-provider@0.1.0;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/chikoski-name/package.wit"),
            br#"
package chikoski:name@0.1.0;

interface name-provider {
  get-name: func() -> string;
}
"#,
        );
        let component_bytes = encode_wit_component(&fixture_wit_root, "plugin");
        write(
            &local_warg_file_path(&root, "chikoski:missing", "0.1.0", "wit.wasm"),
            &component_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("top-level WIT package mismatch"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("chikoski:missing"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("root:component"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_wasm_dependency_without_component_when_wit_is_not_component() {
        let root = new_temp_dir("wit-not-component-for-wasm");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-package");
        write(
            &fixture_wit_root.join("package.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 2,
            "update must fail for non-component WIT"
        );
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("did not decode as a component"),
            "unexpected stderr: {stderr}"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_wa_dev_wit_shorthand() {
        let root = new_temp_dir("wa-dev-shorthand");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "https://wa.dev/chikoski:hello/greet"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("must not use URL scheme; use plain package name"),
            "unexpected stderr: {stderr}"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_sanitized_wit_output_path_collisions() {
        let root = new_temp_dir("wit-output-collision");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "foo-bar:baz"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "foo:bar-baz"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("wit/deps/stale/dependency.wit"),
            b"package stale:dep;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("both resolve to 'wit/deps/foo-bar-baz-0.1.0'"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/stale/dependency.wit").exists(),
            "wit/deps must not be reset when collision is detected"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_overlapping_wit_output_paths_before_reset() {
        let root = new_temp_dir("wit-output-overlap");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "foo-bar:baz"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "foo:bar-baz"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("wit/deps/stale/dependency.wit"),
            b"package stale:dep;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("both resolve to 'wit/deps/foo-bar-baz-0.1.0'"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/stale/dependency.wit").exists(),
            "wit/deps must not be reset when overlap is detected"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_overlapping_wit_output_paths_from_resolved_path_dependencies() {
        let root = new_temp_dir("wit-output-overlap-resolved-path-source");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/a"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/b"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        let cache_wit_root_a = dependency_cache::cache_entry_root(&root, "path-source-0").join(
            dependency_cache::dependency_wit_path("path-source-0", "0.1.0"),
        );
        write(
            &cache_wit_root_a.join("package.wit"),
            b"package foo-bar:baz@0.1.0;\ninterface api { ping: func(); }\n",
        );
        let cache_wit_root_b = dependency_cache::cache_entry_root(&root, "path-source-1").join(
            dependency_cache::dependency_wit_path("path-source-1", "0.1.0"),
        );
        write(
            &cache_wit_root_b.join("package.wit"),
            b"package placeholder:pkg@0.1.0;\ninterface api { pong: func(); }\n",
        );
        let entry_a = dependency_cache::DependencyCacheEntry {
            name: "path-source-0".to_string(),
            resolved_package_name: Some("foo-bar:baz".to_string()),
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "registry/a".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: dependency_cache::dependency_wit_path("path-source-0", "0.1.0"),
            wit_digest: build::compute_path_digest_hex(&cache_wit_root_a).expect("digest"),
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        dependency_cache::save_entry(&root, &entry_a).expect("cache entry A should be saved");
        let entry_b = dependency_cache::DependencyCacheEntry {
            name: "path-source-1".to_string(),
            resolved_package_name: Some("foo:bar-baz".to_string()),
            version: "0.1.0".to_string(),
            kind: "native".to_string(),
            wit_source: "registry/b".to_string(),
            wit_registry: None,
            wit_sha256: None,
            wit_path: dependency_cache::dependency_wit_path("path-source-1", "0.1.0"),
            wit_digest: build::compute_path_digest_hex(&cache_wit_root_b).expect("digest"),
            wit_source_fingerprint: None,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            component_source_fingerprint: None,
            component_world_foreign_packages: vec![],
            component_world_foreign_packages_recorded: true,
            transitive_packages: vec![],
        };
        dependency_cache::save_entry(&root, &entry_b).expect("cache entry B should be saved");
        write(
            &root.join("wit/deps/stale/dependency.wit"),
            b"package stale:dep;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("both resolve to 'wit/deps/foo-bar-baz-0.1.0'"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/stale/dependency.wit").exists(),
            "wit/deps must not be reset when overlap is detected"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_file_source_under_wit_deps_before_reset() {
        let root = new_temp_dir("file-source-under-wit-deps");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "wit/deps/vendor/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("wit/deps/vendor/example.wit"),
            b"package test:example@0.1.0;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("points under wit/deps"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/vendor/example.wit").exists(),
            "source under wit/deps must not be deleted"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_absolute_file_source_under_wit_deps_when_project_root_is_dot() {
        let root = new_temp_dir("file-source-under-wit-deps-absolute");
        let absolute_source = root.join("wit/deps/vendor/example.wit");
        write(
            &root.join("imago.toml"),
            format!(
                r#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "{}"

[target.default]
remote = "127.0.0.1:4443"
"#,
                absolute_source.display()
            )
            .as_bytes(),
        );
        write(&absolute_source, b"package test:example@0.1.0;\n");

        let _cwd_guard = CwdGuard::change_to(&root);
        let result = run(UpdateArgs {}).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("points under wit/deps"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            absolute_source.exists(),
            "source under wit/deps must not be deleted"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_component_file_source_under_wit_deps_before_reset() {
        let root = new_temp_dir("component-file-source-under-wit-deps");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "wasm"
path = "registry/example"

[dependencies.component]
path = "wit/deps/vendor/example-component.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );
        write(
            &root.join("wit/deps/vendor/example-component.wasm"),
            b"\0asmfake-component",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("component source"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("points under wit/deps"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/vendor/example-component.wasm").exists(),
            "component source under wit/deps must not be deleted"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_dependency_name_with_absolute_path_before_reset() {
        let root = new_temp_dir("dependency-name-absolute-path");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "/tmp/pwn"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("wit/deps/stale/dependency.wit"),
            b"package stale:dep;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("failed to parse dependencies[0] source configuration"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("warg source package contains invalid path components"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            root.join("wit/deps/stale/dependency.wit").exists(),
            "wit/deps must not be reset when dependency name validation fails"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_warg_transitive_wit_packages() {
        let root = new_temp_dir("warg-transitive");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("wit/deps/stale/dependency.wit"),
            b"package stale:dep;\n",
        );

        let fixture_wit_root = root.join("fixture-wit");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import chikoski:name/name-provider@0.1.0;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/chikoski-name/package.wit"),
            br#"
package chikoski:name@0.1.0;

interface name-provider {
  get-name: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        assert!(
            !root.join("wit/deps/stale").exists(),
            "wit/deps must be reset before resolving"
        );
        assert!(
            root.join("wit/deps/chikoski-hello-0.1.0/package.wit")
                .exists(),
            "top-level package should be materialized"
        );
        assert!(
            root.join("wit/deps/chikoski-name-0.1.0/package.wit")
                .exists(),
            "transitive package should be materialized"
        );
        assert!(
            !root
                .join("wit/deps/chikoski-hello-0.1.0/.imago_transitive")
                .exists()
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.version, 1);
        assert_eq!(lock.wit_packages.len(), 1);
        assert_eq!(lock.wit_packages[0].name, "chikoski:name");
        assert_eq!(
            lock.wit_packages[0].registry.as_deref(),
            Some(plugin_sources::DEFAULT_WARG_REGISTRY)
        );
        assert_eq!(lock.wit_packages[0].versions.len(), 1);
        let version = &lock.wit_packages[0].versions[0];
        assert_eq!(version.requirement, "=0.1.0");
        assert_eq!(version.version.as_deref(), Some("0.1.0"));
        assert_eq!(version.source.as_deref(), Some("chikoski:name"));
        assert_eq!(version.path, "wit/deps/chikoski-name-0.1.0");
        assert_eq!(version.via, vec!["chikoski:hello".to_string()]);
        assert!(version.digest.starts_with("sha256:"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_warg_transitive_wasi_packages_with_wasi_default_registry() {
        let root = new_temp_dir("warg-transitive-wasi-default");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-wasi");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import wasi:io/streams@0.2.6;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/wasi-io/package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  read: func();
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        let wasi_io = lock
            .wit_packages
            .iter()
            .find(|pkg| pkg.name == "wasi:io")
            .expect("wasi:io transitive package should be materialized");
        assert_eq!(
            wasi_io.registry.as_deref(),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY)
        );
        let version = &wasi_io.versions[0];
        assert_eq!(version.source.as_deref(), Some("wasi:io"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_warg_transitive_wasi_packages_with_namespace_override() {
        let root = new_temp_dir("warg-transitive-wasi-namespace-override");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[namespace_registries]
wasi = "custom-wasi.example"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-wasi-override");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import wasi:io/streams@0.2.6;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/wasi-io/package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  read: func();
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        let wasi_io = lock
            .wit_packages
            .iter()
            .find(|pkg| pkg.name == "wasi:io")
            .expect("wasi:io transitive package should be materialized");
        assert_eq!(wasi_io.registry.as_deref(), Some("custom-wasi.example"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_non_wasi_transitive_with_parent_registry_fallback() {
        let root = new_temp_dir("warg-transitive-parent-registry-fallback");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"
registry = "custom-root.example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-parent-registry");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import chikoski:name/name-provider@0.1.0;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/chikoski-name/package.wit"),
            br#"
package chikoski:name@0.1.0;

interface name-provider {
  get-name: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        let transitive = lock
            .wit_packages
            .iter()
            .find(|pkg| pkg.name == "chikoski:name")
            .expect("chikoski:name transitive package should be materialized");
        assert_eq!(transitive.registry.as_deref(), Some("custom-root.example"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_allows_warg_top_package_without_version() {
        let root = new_temp_dir("warg-top-without-version");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-top-no-version");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello;

interface greet {
  hello: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(
            root.join("wit/deps/chikoski-hello-0.1.0/package.wit")
                .exists()
        );
        assert!(root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_warg_top_package_version_mismatch() {
        let root = new_temp_dir("warg-top-version-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-top-version-mismatch");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.2.0;

interface greet {
  hello: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("top-level WIT package 'chikoski:hello' version mismatch"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_allows_warg_transitive_package_without_version() {
        let root = new_temp_dir("warg-transitive-without-version");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );

        let fixture_wit_root = root.join("fixture-wit-transitive-no-version");
        write(
            &fixture_wit_root.join("greet.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import chikoski:name/name-provider;
}
"#,
        );
        write(
            &fixture_wit_root.join("deps/chikoski-name/package.wit"),
            br#"
package chikoski:name;

interface name-provider {
  name: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/chikoski-name/package.wit").exists());
        assert!(
            !root
                .join("wit/deps/chikoski-hello-0.1.0/.imago_transitive")
                .exists()
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.wit_packages.len(), 1);
        assert_eq!(lock.wit_packages[0].name, "chikoski:name");
        let version = &lock.wit_packages[0].versions[0];
        assert_eq!(version.requirement, "*");
        assert!(version.version.is_none());
        assert!(version.source.is_none());
        assert_eq!(version.path, "wit/deps/chikoski-name");
        assert_eq!(version.via, vec!["chikoski:hello".to_string()]);
        assert!(version.digest.starts_with("sha256:"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_allows_file_source_package_without_version() {
        let root = new_temp_dir("file-source-without-version");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(
            root.join("wit/deps/test-example-0.1.0/package.wit")
                .exists()
        );
        assert!(root.join("imago.lock").exists());
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.dependencies.len(), 1);
        assert_eq!(lock.dependencies[0].name, "test:example");

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rehydrates_wit_from_cache_when_file_source_disappears() {
        let root = new_temp_dir("file-source-missing-cache-hit");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );

        let first = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(first.exit_code, 0, "first update should succeed: {first:?}");

        fs::remove_file(root.join("registry/example/package.wit"))
            .expect("source should be removable");
        fs::remove_dir_all(root.join("wit/deps")).expect("wit/deps should be removable");

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should succeed from cache: {:?}",
            second.stderr
        );
        assert!(
            root.join("wit/deps/test-example-0.1.0/package.wit")
                .exists(),
            "wit/deps should be hydrated from dependency cache"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_refreshes_dependency_cache_when_file_source_changes() {
        let root = new_temp_dir("file-source-refresh");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/example"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\n",
        );

        let first = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(first.exit_code, 0, "first update should succeed: {first:?}");
        let lock_v1: ImagoLock = toml::from_str(
            &fs::read_to_string(root.join("imago.lock")).expect("lock should exist"),
        )
        .expect("lock should parse");
        let digest_v1 = lock_v1.dependencies[0].wit_digest.clone();

        write(
            &root.join("registry/example/package.wit"),
            b"package test:example@0.1.0;\ninterface changed { ping: func(); }\n",
        );

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should succeed: {second:?}"
        );
        let lock_v2: ImagoLock = toml::from_str(
            &fs::read_to_string(root.join("imago.lock")).expect("lock should exist"),
        )
        .expect("lock should parse");
        let digest_v2 = lock_v2.dependencies[0].wit_digest.clone();
        assert_ne!(
            digest_v1, digest_v2,
            "wit digest must change after source update"
        );
        let cached_wit = fs::read_to_string(
            root.join(".imago/deps/path-source-0/wit/deps/path-source-0-0.1.0/package.wit"),
        )
        .expect("cached wit should exist");
        assert!(
            cached_wit.contains("interface changed"),
            "cached wit should include the refreshed interface: {cached_wit}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_uses_dependency_cache_when_local_warg_fixture_is_removed() {
        let root = new_temp_dir("warg-missing-after-cache");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        let fixture_wit_root = root.join("fixture-wit-warg");
        write(
            &fixture_wit_root.join("package.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}
"#,
        );
        let wit_package_bytes = encode_wit_package(&fixture_wit_root);
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wasm"),
            &wit_package_bytes,
        );

        let first = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(first.exit_code, 0, "first update should succeed: {first:?}");

        fs::remove_dir_all(
            root.join(".imago/warg")
                .join(plugin_sources::warg_local_package_key("chikoski:hello")),
        )
        .expect("local warg fixture should be removable");
        fs::remove_dir_all(root.join("wit/deps")).expect("wit/deps should be removable");

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should succeed from dependency cache: {:?}",
            second.stderr
        );
        assert!(
            root.join("wit/deps/chikoski-hello-0.1.0/package.wit")
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_plain_wit_with_foreign_imports_from_warg_source() {
        let root = new_temp_dir("warg-plain-wit-foreign-import");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
wit = "chikoski:hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "chikoski:hello", "0.1.0", "wit.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}

world example {
  import chikoski:name/name-provider;
}
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("contains foreign imports in plain .wit form"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rewrites_binding_interfaces_with_connection_and_outer_result() {
        let root = new_temp_dir("bindings-rewrite");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/hello"

[[bindings]]
name = "svc-target"
version = "0.1.0"
path = "registry/hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello/package.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
  ping: func(a: u32) -> u32;
}

interface untouched {
  pass-through: func() -> string;
}
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let rewritten = fs::read_to_string(root.join("wit/deps/chikoski-hello-0.1.0/package.wit"))
            .expect("rewritten package.wit should exist");
        assert!(
            rewritten.contains("use imago:node/rpc@0.1.0.{connection};"),
            "connection use must be injected: {rewritten}"
        );
        assert!(
            rewritten
                .contains("hello: func(connection: borrow<connection>) -> result<string, string>;"),
            "return must be wrapped: {rewritten}"
        );
        assert!(
            rewritten.contains(
                "ping: func(connection: borrow<connection>, a: u32) -> result<u32, string>;"
            ),
            "connection arg must be injected: {rewritten}"
        );
        assert!(
            rewritten.contains(
                "pass-through: func(connection: borrow<connection>) -> result<string, string>;"
            ),
            "all interfaces in package must be rewritten: {rewritten}"
        );

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should keep rewrite idempotent: {:?}",
            second.stderr
        );
        let rewritten_second =
            fs::read_to_string(root.join("wit/deps/chikoski-hello-0.1.0/package.wit"))
                .expect("rewritten package.wit should exist");
        let use_count = rewritten_second
            .matches("use imago:node/rpc@0.1.0.{connection};")
            .count();
        assert_eq!(
            use_count, 2,
            "connection use should exist once per rewritten interface"
        );
        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert_eq!(lock.binding_wits.len(), 1);
        assert_eq!(lock.binding_wits[0].name, "svc-target");
        assert_eq!(
            lock.binding_wits[0].wit_source,
            "registry/hello".to_string()
        );
        assert_eq!(
            lock.binding_wits[0].interfaces,
            vec![
                "chikoski:hello/greet".to_string(),
                "chikoski:hello/untouched".to_string()
            ]
        );
        build::build_project("default", &root)
            .expect("build should succeed after rewrite by using synchronized dependency cache");

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_generates_wit_deps_for_bindings_without_dependencies() {
        let root = new_temp_dir("bindings-only");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[bindings]]
name = "svc-target"
version = "0.1.0"
path = "registry/hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello/package.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  hello: func() -> string;
}
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );

        let rewritten = fs::read_to_string(root.join("wit/deps/chikoski-hello-0.1.0/package.wit"))
            .expect("binding-only package.wit should exist");
        assert!(
            rewritten
                .contains("hello: func(connection: borrow<connection>) -> result<string, string>;"),
            "binding-only WIT should be rewritten: {rewritten}"
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert!(lock.dependencies.is_empty());
        assert_eq!(lock.binding_wits.len(), 1);
        assert_eq!(lock.binding_wits[0].name, "svc-target");
        assert_eq!(
            lock.binding_wits[0].interfaces,
            vec!["chikoski:hello/greet".to_string()]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_dependency_binding_package_collision_on_source_mismatch() {
        let root = new_temp_dir("binding-dependency-source-mismatch");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/hello-a"

[[bindings]]
name = "svc-target"
version = "0.1.0"
path = "registry/hello-b"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello-a/package.wit"),
            br#"
package chikoski:hello@0.1.0;
interface greet { hello: func() -> string; }
"#,
        );
        write(
            &root.join("registry/hello-b/package.wit"),
            br#"
package chikoski:hello@0.1.0;
interface greet { hello2: func() -> string; }
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("different wit source kind/source/registry"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_binding_wit_with_resource_definition() {
        let root = new_temp_dir("bindings-resource-reject");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/hello"

[[bindings]]
name = "svc-target"
version = "0.1.0"
path = "registry/hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello/package.wit"),
            br#"
package chikoski:hello@0.1.0;

interface greet {
  resource connection {
    close: func();
  }
  hello: func(connection: borrow<connection>) -> string;
}
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("does not support resources"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_same_wit_mapped_to_multiple_binding_names() {
        let root = new_temp_dir("bindings-ambiguous");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.1.0"
kind = "native"
path = "registry/hello"

[[bindings]]
name = "svc-target-a"
version = "0.1.0"
path = "registry/hello"

[[bindings]]
name = "svc-target-b"
version = "0.1.0"
path = "registry/hello"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello/package.wit"),
            b"package chikoski:hello@0.1.0;\ninterface greet { hello: func() -> string; }\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("maps to multiple services"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_materializes_wasi_packages_from_wit_package_dir() {
        let root = new_temp_dir("wit-package-dir-wasi-materialize");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(
            &root.join("wit/common.wit"),
            br#"
package example:svc@0.1.0;

world common {
  import wasi:clocks/wall-clock@0.2.6;
}
"#,
        );
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world host {
  include common;
  import wasi:io/streams@0.2.6;
  export wasi:clocks/wall-clock@0.2.6;
}
"#,
        );

        let wasi_io_fixture = root.join("fixture-wasi-io");
        write(
            &wasi_io_fixture.join("package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  read: func();
}
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:io", "0.2.6", "wit.wasm"),
            &encode_wit_package(&wasi_io_fixture),
        );

        let wasi_clocks_fixture = root.join("fixture-wasi-clocks");
        write(
            &wasi_clocks_fixture.join("package.wit"),
            br#"
package wasi:clocks@0.2.6;

interface wall-clock {
  now: func() -> u64;
}
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:clocks", "0.2.6", "wit.wasm"),
            &encode_wit_package(&wasi_clocks_fixture),
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/wasi-io-0.2.6/package.wit").is_file());
        assert!(
            root.join("wit/deps/wasi-clocks-0.2.6/package.wit")
                .is_file()
        );

        let lock_raw = fs::read_to_string(root.join("imago.lock")).expect("lock should exist");
        let lock: ImagoLock = toml::from_str(&lock_raw).expect("lock should parse");
        assert!(lock.dependencies.is_empty());
        let wasi_io = lock
            .wit_packages
            .iter()
            .find(|package| package.name == "wasi:io")
            .expect("wasi:io lock entry must exist");
        assert_eq!(
            wasi_io.registry.as_deref(),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY)
        );
        assert_eq!(wasi_io.versions.len(), 1);
        assert!(
            wasi_io.versions[0].via.is_empty(),
            "wit-dir origin should keep empty via"
        );

        let wasi_clocks = lock
            .wit_packages
            .iter()
            .find(|package| package.name == "wasi:clocks")
            .expect("wasi:clocks lock entry must exist");
        assert_eq!(
            wasi_clocks.registry.as_deref(),
            Some(plugin_sources::DEFAULT_WASI_WARG_REGISTRY)
        );
        assert_eq!(wasi_clocks.versions.len(), 1);
        assert!(
            wasi_clocks.versions[0].via.is_empty(),
            "wit-dir origin should keep empty via"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_wasi_reference_without_explicit_version_in_wit_package_dir() {
        let root = new_temp_dir("wit-package-dir-wasi-unversioned");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world host {
  import wasi:io/streams;
}
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("without explicit version"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_conflicting_wasi_versions_in_wit_package_dir() {
        let root = new_temp_dir("wit-package-dir-wasi-version-conflict");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(
            &root.join("wit/a.wit"),
            br#"
package example:svc@0.1.0;

world host-a {
  import wasi:io/streams@0.2.6;
}
"#,
        );
        write(
            &root.join("wit/b.wit"),
            br#"
package example:svc@0.1.0;

world host-b {
  import wasi:io/streams@0.2.7;
}
"#,
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("conflicting versions"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_ignores_wit_dir_without_top_level_wit_files() {
        let root = new_temp_dir("wit-package-dir-no-top-level-wit");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(&root.join("wit/notes.txt"), b"not a wit file");

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("imago.lock").is_file());
        assert!(
            !root.join("wit/deps/wasi-io-0.2.6").exists(),
            "wasi packages should not be materialized without top-level .wit files"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_allows_wit_dir_wasi_materialization_when_dependency_output_matches() {
        let root = new_temp_dir("wit-package-dir-wasi-overlap-same");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.6"
kind = "native"
wit = "wasi:io"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world host {
  import wasi:io/streams@0.2.6;
}
"#,
        );

        let wasi_io_fixture = root.join("fixture-wasi-io-overlap-same");
        write(
            &wasi_io_fixture.join("package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  read: func();
}
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:io", "0.2.6", "wit.wasm"),
            &encode_wit_package(&wasi_io_fixture),
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            result.exit_code, 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/wasi-io-0.2.6/package.wit").is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn update_rejects_wit_dir_wasi_materialization_when_dependency_output_conflicts() {
        let root = new_temp_dir("wit-package-dir-wasi-overlap-conflict");
        write(
            &root.join("imago.toml"),
            br#"
name = "svc"
main = "build/app.wasm"
type = "cli"

[[dependencies]]
version = "0.2.6"
kind = "native"
path = "registry/wasi-io"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(&root.join("build/app.wasm"), b"\0asm");
        write(
            &root.join("wit/world.wit"),
            br#"
package example:svc@0.1.0;

world host {
  import wasi:io/streams@0.2.6;
}
"#,
        );
        write(
            &root.join("registry/wasi-io/package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  read: func();
}
"#,
        );

        let wasi_io_fixture = root.join("fixture-wasi-io-overlap-conflict");
        write(
            &wasi_io_fixture.join("package.wit"),
            br#"
package wasi:io@0.2.6;

interface streams {
  write: func();
}
"#,
        );
        write(
            &local_warg_file_path(&root, "wasi:io", "0.2.6", "wit.wasm"),
            &encode_wit_package(&wasi_io_fixture),
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("conflicting transitive WIT package detected"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
