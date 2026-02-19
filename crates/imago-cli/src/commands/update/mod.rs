use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use imago_lockfile::{
    IMAGO_LOCK_VERSION, ImagoLock, ImagoLockDependency, TransitivePackageRecord,
    collect_wit_packages, save_to_project_root,
};
use toml::Value as TomlValue;
use wit_component::WitPrinter;
use wit_parser::{FunctionKind, InterfaceId, PackageId, Resolve, Type, TypeDefKind, TypeId};

use crate::{
    cli::UpdateArgs,
    commands::{
        CommandResult,
        build::{self},
        dependency_cache::{self},
        plugin_sources,
        shared::dependency::StandardDependencyResolver,
    },
};

mod cache;

const IMAGO_NODE_CONNECTION_USE: &str = "use imago:node/rpc@0.1.0.{connection};";

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingTarget {
    name: String,
    wit: String,
}

pub async fn run(args: UpdateArgs) -> CommandResult {
    run_with_project_root(args, Path::new(".")).await
}

pub(crate) async fn run_with_project_root(_args: UpdateArgs, project_root: &Path) -> CommandResult {
    match run_inner_async(project_root).await {
        Ok(()) => CommandResult {
            exit_code: 0,
            stderr: None,
        },
        Err(err) => CommandResult {
            exit_code: 2,
            stderr: Some(format!("{err:#}")),
        },
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

    let validate_file_source = |dependency_name: &str,
                                source_label: &str,
                                source: &str|
     -> anyhow::Result<()> {
        let Some(raw_path) = source.strip_prefix("file://") else {
            return Ok(());
        };
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
                "dependency '{}' {} '{}' points under wit/deps, which `imago update` resets; move the source outside wit/deps",
                dependency_name,
                source_label,
                source
            ));
        }
        Ok(())
    };

    for dependency in dependencies {
        validate_file_source(&dependency.name, "wit source", &dependency.wit.source)?;
        if let Some(component) = dependency.component.as_ref() {
            validate_file_source(&dependency.name, "component source", &component.source)?;
        }
    }
    Ok(())
}

fn validate_wit_output_path_collisions(
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<()> {
    let mut targets: Vec<(PathBuf, &str)> = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let target_rel = dependency_cache::dependency_wit_target_rel(&dependency.name);
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

fn load_binding_targets(project_root: &Path) -> anyhow::Result<Vec<BindingTarget>> {
    let imago_toml_path = project_root.join("imago.toml");
    let raw = fs::read_to_string(&imago_toml_path)
        .with_context(|| format!("failed to read {}", imago_toml_path.display()))?;
    let root: toml::Table = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", imago_toml_path.display()))?;
    let Some(value) = root.get("bindings") else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("bindings must be an array"))?;
    let mut bindings = Vec::with_capacity(array.len());
    let mut wit_to_name = BTreeMap::<String, String>::new();
    for (index, entry) in array.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| anyhow!("bindings[{index}] must be a table"))?;
        for key in table.keys() {
            if key == "target" {
                return Err(anyhow!(
                    "bindings[{index}].target is no longer supported; use bindings[{index}].name"
                ));
            }
            if !matches!(key.as_str(), "name" | "wit") {
                return Err(anyhow!("bindings[{index}].{key} is not supported"));
            }
        }

        let name = table
            .get("name")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].name must be a string"))?
            .trim()
            .to_string();
        let wit = table
            .get("wit")
            .and_then(TomlValue::as_str)
            .ok_or_else(|| anyhow!("bindings[{index}].wit must be a string"))?
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(anyhow!("bindings[{index}].name must not be empty"));
        }
        if wit.is_empty() {
            return Err(anyhow!("bindings[{index}].wit must not be empty"));
        }
        build::validate_service_name(&name).map_err(|e| {
            anyhow!(
                "bindings[{index}].name is invalid: {}",
                e.to_string().replace("name ", "")
            )
        })?;

        if let Some(existing_name) = wit_to_name.get(&wit) {
            if existing_name != &name {
                return Err(anyhow!(
                    "bindings wit '{}' maps to multiple services ('{}' and '{}'); this is ambiguous",
                    wit,
                    existing_name,
                    name
                ));
            }
        } else {
            wit_to_name.insert(wit.clone(), name.clone());
        }

        bindings.push(BindingTarget { name, wit });
    }
    Ok(bindings)
}

fn split_binding_wit(binding_wit: &str) -> anyhow::Result<(&str, &str)> {
    let (package_name, interface_name) = binding_wit.split_once('/').ok_or_else(|| {
        anyhow!(
            "binding wit '{}' must be '<package>/<interface>'",
            binding_wit
        )
    })?;
    if package_name.trim().is_empty() || interface_name.trim().is_empty() {
        return Err(anyhow!(
            "binding wit '{}' must be '<package>/<interface>'",
            binding_wit
        ));
    }
    Ok((package_name.trim(), interface_name.trim()))
}

fn rewrite_bound_wit_packages(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
    lock_entries: &[ImagoLockDependency],
    bindings: &[BindingTarget],
) -> anyhow::Result<()> {
    if bindings.is_empty() {
        return Ok(());
    }

    let mut target_interfaces_by_package = BTreeMap::<String, BTreeSet<String>>::new();
    for binding in bindings {
        let (package_name, interface_name) = split_binding_wit(&binding.wit)?;
        target_interfaces_by_package
            .entry(package_name.to_string())
            .or_default()
            .insert(interface_name.to_string());
    }

    let lock_by_name = lock_entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let deps_root = project_root.join("wit").join("deps");
    for dependency in dependencies {
        let Some(interface_names) = target_interfaces_by_package.get(&dependency.name) else {
            continue;
        };
        let Some(lock_entry) = lock_by_name.get(dependency.name.as_str()) else {
            continue;
        };
        let dependency_wit_root = project_root.join(&lock_entry.wit_path);
        rewrite_dependency_wit_interfaces(
            &dependency.name,
            &dependency_wit_root,
            &deps_root,
            interface_names,
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
                "binding WIT '{}/{}' contains resource methods; `imago update` does not support resources",
                dependency_name,
                interface_name
            ));
        }
        for (_, param_type) in &function.params {
            if type_contains_resource(resolve, param_type, &mut BTreeSet::new()) {
                return Err(anyhow!(
                    "binding WIT '{}/{}' contains resource types; `imago update` does not support resources",
                    dependency_name,
                    interface_name
                ));
            }
        }
        if let Some(result) = &function.result
            && type_contains_resource(resolve, result, &mut BTreeSet::new())
        {
            return Err(anyhow!(
                "binding WIT '{}/{}' contains resource types; `imago update` does not support resources",
                dependency_name,
                interface_name
            ));
        }
    }
    for type_id in interface.types.values() {
        if type_id_contains_resource(resolve, *type_id, &mut BTreeSet::new()) {
            return Err(anyhow!(
                "binding WIT '{}/{}' contains resource definitions; `imago update` does not support resources",
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
        TypeDefKind::FixedSizeList(ty, _) => type_contains_resource(resolve, ty, seen),
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

async fn run_inner_async(project_root: &Path) -> anyhow::Result<()> {
    let dependency_resolver = StandardDependencyResolver;
    let dependencies = build::load_project_dependencies(project_root)?;
    let bindings = load_binding_targets(project_root)?;
    validate_wit_sources_outside_wit_deps(project_root, &dependencies)?;
    validate_wit_output_path_collisions(&dependencies)?;

    let resolved_at = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
    let mut lock_entries = Vec::with_capacity(dependencies.len());
    let mut transitive_records = Vec::new();

    for dependency in &dependencies {
        let cache_entry =
            cache::load_or_refresh_cache_entry(&dependency_resolver, project_root, dependency)
                .await?;
        transitive_records.extend(cache_entry.transitive_packages.iter().map(|transitive| {
            TransitivePackageRecord {
                name: transitive.name.clone(),
                registry: transitive.registry.clone(),
                requirement: transitive.requirement.clone(),
                version: transitive.version.clone(),
                digest: transitive.digest.clone(),
                source: transitive.source.clone(),
                path: transitive.path.clone(),
                via: dependency.name.clone(),
            }
        }));
        lock_entries.push(ImagoLockDependency {
            name: dependency.name.clone(),
            version: dependency.version.clone(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_digest: cache_entry.wit_digest,
            wit_path: cache_entry.wit_path,
            component_source: cache_entry.component_source,
            component_registry: cache_entry.component_registry,
            component_sha256: cache_entry.component_sha256,
            resolved_at: resolved_at.clone(),
        });
    }

    dependency_cache::hydrate_project_wit_deps(project_root, &dependencies)?;
    rewrite_bound_wit_packages(project_root, &dependencies, &lock_entries, &bindings)?;
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

    lock_entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    let lock = ImagoLock {
        version: IMAGO_LOCK_VERSION,
        dependencies: lock_entries,
        wit_packages: collect_wit_packages(transitive_records),
    };
    save_to_project_root(project_root, &lock)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;
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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
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
        assert_eq!(entry.name, "yieldspace:plugin/example");
        assert_eq!(entry.wit_source, "file://registry/example.wit");
        assert_eq!(entry.wit_registry, None);
        assert_eq!(entry.wit_path, "wit/deps/yieldspace-plugin/example");
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
name = "yieldspace:plugin/example"
version = "1.2.3"
kind = "native"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &local_warg_file_path(&root, "yieldspace:plugin/example", "1.2.3", "wit.wit"),
            b"package test:example@1.2.3;\n",
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
        assert_eq!(
            lock.dependencies[0].wit_source,
            "warg://yieldspace:plugin/example@1.2.3"
        );
        assert_eq!(
            lock.dependencies[0].wit_registry,
            Some(plugin_sources::DEFAULT_WARG_REGISTRY.to_string())
        );
        assert_eq!(
            lock.dependencies[0].wit_path,
            "wit/deps/yieldspace-plugin/example"
        );
        assert!(root.join(&lock.dependencies[0].wit_path).exists());

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
name = "yieldspace:plugin/example"
version = "1.2.3"
kind = "wasm"
wit = "file://registry/example.wit"

[dependencies.component]
source = "file://registry/example-component.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
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
            Some("file://registry/example-component.wasm")
        );
        assert_eq!(entry.component_registry, None);
        assert_eq!(
            entry.component_sha256.as_deref(),
            Some(component_sha.as_str())
        );
        assert!(
            root.join(".imago/deps/yieldspace-plugin/example/meta.toml")
                .exists(),
            "dependency cache metadata must be written"
        );
        assert!(
            root.join(".imago/deps/yieldspace-plugin/example/components")
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
name = "root:component"
version = "0.1.0"
kind = "wasm"
wit = "warg://root:component@0.1.0"

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
        assert_eq!(
            entry.component_source.as_deref(),
            Some("warg://root:component@0.1.0")
        );
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
        assert!(root.join("wit/deps/root-component/package.wit").exists());

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
name = "chikoski:hello"
version = "0.1.0"
kind = "wasm"
wit = "warg://chikoski:hello@0.1.0"

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
name = "chikoski:hello"
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
            stderr.contains("no longer accepts https://wa.dev shorthand"),
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
name = "foo:bar"
version = "0.1.0"
kind = "native"
wit = "file://registry/a.wit"

[[dependencies]]
name = "foo-bar"
version = "0.2.0"
kind = "native"
wit = "file://registry/b.wit"

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
            stderr.contains("both resolve to 'wit/deps/foo-bar'"),
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
name = "foo:pkg"
version = "0.1.0"
kind = "native"
wit = "file://registry/a.wit"

[[dependencies]]
name = "foo:pkg/bar"
version = "0.1.0"
kind = "native"
wit = "file://registry/b.wit"

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
            stderr.contains("overlapping WIT output paths"),
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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://wit/deps/vendor/example.wit"

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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://{}"

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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "wasm"
wit = "file://registry/example.wit"

[dependencies.component]
source = "file://wit/deps/vendor/example-component.wasm"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
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
name = "/tmp/pwn"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

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
            stderr.contains("dependencies[0].name is invalid"),
            "unexpected stderr: {stderr}"
        );
        assert!(
            stderr.contains("invalid path components"),
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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
            root.join("wit/deps/chikoski-hello/package.wit").exists(),
            "top-level package should be materialized"
        );
        assert!(
            root.join("wit/deps/chikoski-name/package.wit").exists(),
            "transitive package should be materialized"
        );
        assert!(
            !root
                .join("wit/deps/chikoski-hello/.imago_transitive")
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
        assert_eq!(
            version.source.as_deref(),
            Some("warg://chikoski:name@0.1.0")
        );
        assert_eq!(version.path, "wit/deps/chikoski-name");
        assert_eq!(version.via, vec!["chikoski:hello".to_string()]);
        assert!(version.digest.starts_with("sha256:"));

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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
        assert!(root.join("wit/deps/chikoski-hello/package.wit").exists());
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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
                .join("wit/deps/chikoski-hello/.imago_transitive")
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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
            b"package test:example;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root).await;
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(
            root.join("wit/deps/yieldspace-plugin/example/example.wit")
                .exists()
        );
        assert!(root.join("imago.lock").exists());

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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
            b"package test:example@0.1.0;\n",
        );

        let first = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(first.exit_code, 0, "first update should succeed: {first:?}");

        fs::remove_file(root.join("registry/example.wit")).expect("source should be removable");
        fs::remove_dir_all(root.join("wit/deps")).expect("wit/deps should be removable");

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should succeed from cache: {:?}",
            second.stderr
        );
        assert!(
            root.join("wit/deps/yieldspace-plugin/example/example.wit")
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
name = "yieldspace:plugin/example"
version = "0.1.0"
kind = "native"
wit = "file://registry/example.wit"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/example.wit"),
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
            &root.join("registry/example.wit"),
            b"package test:example@0.2.0;\n",
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
        assert_eq!(
            fs::read_to_string(root.join(
                ".imago/deps/yieldspace-plugin/example/wit/deps/yieldspace-plugin/example/example.wit"
            ))
            .expect("cached wit should exist"),
            "package test:example@0.2.0;\n"
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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
        assert!(root.join("wit/deps/chikoski-hello/package.wit").exists());

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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "warg://chikoski:hello@0.1.0"

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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "file://registry/hello.wit"

[[bindings]]
name = "svc-target"
wit = "chikoski:hello/greet"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello.wit"),
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

        let rewritten = fs::read_to_string(root.join("wit/deps/chikoski-hello/package.wit"))
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
            rewritten.contains("pass-through: func() -> string;"),
            "non-binding interfaces must remain unchanged: {rewritten}"
        );

        let second = run_with_project_root(UpdateArgs {}, &root).await;
        assert_eq!(
            second.exit_code, 0,
            "second update should keep rewrite idempotent: {:?}",
            second.stderr
        );
        let rewritten_second = fs::read_to_string(root.join("wit/deps/chikoski-hello/package.wit"))
            .expect("rewritten package.wit should exist");
        let use_count = rewritten_second
            .matches("use imago:node/rpc@0.1.0.{connection};")
            .count();
        assert_eq!(use_count, 1, "connection use should not be duplicated");
        build::build_project("default", &root)
            .expect("build should succeed after rewrite by using synchronized dependency cache");

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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "file://registry/hello.wit"

[[bindings]]
name = "svc-target"
wit = "chikoski:hello/greet"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello.wit"),
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
name = "chikoski:hello"
version = "0.1.0"
kind = "native"
wit = "file://registry/hello.wit"

[[bindings]]
name = "svc-target-a"
wit = "chikoski:hello/greet"

[[bindings]]
name = "svc-target-b"
wit = "chikoski:hello/greet"

[target.default]
remote = "127.0.0.1:4443"
"#,
        );
        write(
            &root.join("registry/hello.wit"),
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
}
