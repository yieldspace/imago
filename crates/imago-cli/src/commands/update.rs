use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};
use imago_lockfile::{
    IMAGO_LOCK_VERSION, ImagoLock, ImagoLockDependency, TransitivePackageRecord,
    collect_wit_packages, save_to_project_root,
};

use crate::{
    cli::UpdateArgs,
    commands::{
        CommandResult,
        build::{self, ManifestDependencyKind},
        plugin_sources,
    },
};

pub fn run(args: UpdateArgs) -> CommandResult {
    run_with_project_root(args, Path::new("."))
}

pub(crate) fn run_with_project_root(_args: UpdateArgs, project_root: &Path) -> CommandResult {
    match run_inner(project_root) {
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

fn run_inner(project_root: &Path) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime for update command")?;
    runtime.block_on(run_inner_async(project_root))
}

fn wit_deps_target_rel(dependency_name: &str) -> PathBuf {
    PathBuf::from("wit")
        .join("deps")
        .join(plugin_sources::sanitize_wit_deps_name(dependency_name))
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

fn validate_wit_sources_outside_wit_deps(
    project_root: &Path,
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<()> {
    let wit_deps_root = normalize_path_for_compare(&project_root.join("wit").join("deps"));
    for dependency in dependencies {
        let Some(raw_path) = dependency.wit.source.strip_prefix("file://") else {
            continue;
        };
        let source_path = if Path::new(raw_path).is_absolute() {
            PathBuf::from(raw_path)
        } else {
            project_root.join(raw_path)
        };
        let normalized_source = normalize_path_for_compare(&source_path);
        if normalized_source.starts_with(&wit_deps_root) {
            return Err(anyhow!(
                "dependency '{}' wit source '{}' points under wit/deps, which `imago update` resets; move the source outside wit/deps",
                dependency.name,
                dependency.wit.source
            ));
        }
    }
    Ok(())
}

fn validate_wit_output_path_collisions(
    dependencies: &[build::ProjectDependency],
) -> anyhow::Result<()> {
    let mut path_to_dependency: BTreeMap<PathBuf, &str> = BTreeMap::new();
    for dependency in dependencies {
        let target_rel = wit_deps_target_rel(&dependency.name);
        if let Some(existing_dependency) =
            path_to_dependency.insert(target_rel.clone(), dependency.name.as_str())
        {
            return Err(anyhow!(
                "dependencies '{}' and '{}' both resolve to '{}'; dependency WIT output paths must be unique",
                existing_dependency,
                dependency.name,
                plugin_sources::path_to_manifest_string(&target_rel)
            ));
        }
    }
    Ok(())
}

async fn run_inner_async(project_root: &Path) -> anyhow::Result<()> {
    let dependencies = build::load_project_dependencies(project_root)?;
    validate_wit_sources_outside_wit_deps(project_root, &dependencies)?;
    validate_wit_output_path_collisions(&dependencies)?;

    let wit_root = project_root.join("wit").join("deps");
    if wit_root.exists() {
        fs::remove_dir_all(&wit_root)
            .with_context(|| format!("failed to reset wit root: {}", wit_root.display()))?;
    }
    fs::create_dir_all(&wit_root)
        .with_context(|| format!("failed to create wit root: {}", wit_root.display()))?;

    let resolved_at = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
    let mut lock_entries = Vec::with_capacity(dependencies.len());
    let mut transitive_records = Vec::new();

    for dependency in dependencies {
        let target_rel = wit_deps_target_rel(&dependency.name);
        let target_path = project_root.join(&target_rel);
        fs::create_dir_all(&target_path).with_context(|| {
            format!(
                "failed to create dependency wit output dir: {}",
                target_path.display()
            )
        })?;
        let materialized = plugin_sources::materialize_wit_source(
            project_root,
            &dependency.wit.source,
            dependency.wit.registry.as_deref(),
            &dependency.version,
            &target_path,
        )
        .await
        .with_context(|| format!("failed to resolve dependency '{}'", dependency.name))?;
        transitive_records.extend(materialized.transitive_packages.iter().map(|transitive| {
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

        let digest = build::compute_path_digest_hex(&target_path)?;
        let (component_source, component_registry, component_sha256) = match dependency.kind {
            ManifestDependencyKind::Native => (None, None, None),
            ManifestDependencyKind::Wasm => {
                if let Some(component) = dependency.component.as_ref() {
                    let digest = plugin_sources::resolve_component_sha256(
                        project_root,
                        &component.source,
                        component.registry.as_deref(),
                        component.sha256.as_deref(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to resolve component sha256 for dependency '{}'",
                            dependency.name
                        )
                    })?;
                    (
                        Some(component.source.clone()),
                        component.registry.clone(),
                        Some(digest),
                    )
                } else if let Some(derived) = materialized.derived_component.as_ref() {
                    (
                        Some(derived.source.clone()),
                        derived.registry.clone(),
                        Some(derived.sha256.clone()),
                    )
                } else {
                    return Err(anyhow!(
                        "dependencies entry '{}' is kind=\"wasm\" but no component source was provided and wit source '{}' did not decode as a component",
                        dependency.name,
                        dependency.wit.source
                    ));
                }
            }
        };

        lock_entries.push(ImagoLockDependency {
            name: dependency.name.clone(),
            version: dependency.version.clone(),
            wit_source: dependency.wit.source.clone(),
            wit_registry: dependency.wit.registry.clone(),
            wit_digest: digest,
            wit_path: plugin_sources::path_to_manifest_string(&target_rel),
            component_source,
            component_registry,
            component_sha256,
            resolved_at: resolved_at.clone(),
        });
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

    #[test]
    fn update_resolves_file_source_into_wit_and_lock() {
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

        let result = run_with_project_root(UpdateArgs {}, &root);
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

        let second = run_with_project_root(UpdateArgs {}, &root);
        assert_eq!(second.exit_code, 0);
        let lock_raw_2 =
            fs::read_to_string(root.join("imago.lock")).expect("lock should exist after rerun");
        let lock_2: ImagoLock = toml::from_str(&lock_raw_2).expect("lock should parse");
        assert_eq!(lock_2.dependencies[0].wit_digest, entry.wit_digest);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_uses_default_warg_source_when_wit_is_omitted() {
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
            &root.join(".imago/warg/yieldspace-plugin/example/1.2.3/wit.wit"),
            b"package test:example@1.2.3;\n",
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_records_component_source_and_sha_without_materializing_component_file() {
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

        let result = run_with_project_root(UpdateArgs {}, &root);
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
            !root
                .join(".imago/components")
                .join(format!("{component_sha}.wasm"))
                .exists(),
            "update must not materialize component cache"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_derives_component_info_from_wit_component_source() {
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
            &root.join(".imago/warg/root-component/0.1.0/wit.wasm"),
            &component_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
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
        assert!(root.join("wit/deps/root-component/package.wit").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_rejects_wasm_dependency_without_component_when_wit_is_not_component() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_rejects_wa_dev_wit_shorthand() {
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
        let result = run_with_project_root(UpdateArgs {}, &root);
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("no longer accepts https://wa.dev shorthand"),
            "unexpected stderr: {stderr}"
        );
        assert!(!root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_rejects_sanitized_wit_output_path_collisions() {
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

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_rejects_file_source_under_wit_deps_before_reset() {
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

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_materializes_warg_transitive_wit_packages() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_allows_warg_top_package_without_version() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/chikoski-hello/package.wit").exists());
        assert!(root.join("imago.lock").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_rejects_warg_top_package_version_mismatch() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("top-level WIT package 'chikoski:hello' version mismatch"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_allows_warg_transitive_package_without_version() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_allows_file_source_package_without_version() {
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

        let result = run_with_project_root(UpdateArgs {}, &root);
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

    #[test]
    fn update_rejects_plain_wit_with_foreign_imports_from_warg_source() {
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
            &root.join(".imago/warg/chikoski-hello/0.1.0/wit.wit"),
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

        let result = run_with_project_root(UpdateArgs {}, &root);
        assert_eq!(result.exit_code, 2);
        let stderr = result.stderr.unwrap_or_default();
        assert!(
            stderr.contains("contains foreign imports in plain .wit form"),
            "unexpected stderr: {stderr}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
