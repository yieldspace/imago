use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow};

use crate::{
    cli::UpdateArgs,
    commands::{
        CommandResult,
        build::{self, ImagoLock, ImagoLockDependency, ManifestDependencyKind},
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

async fn run_inner_async(project_root: &Path) -> anyhow::Result<()> {
    let dependencies = build::load_project_dependencies(project_root)?;

    let wit_root = project_root.join("wit").join("deps");
    if wit_root.exists() {
        fs::remove_dir_all(&wit_root)
            .with_context(|| format!("failed to reset wit root: {}", wit_root.display()))?;
    }
    fs::create_dir_all(&wit_root)
        .with_context(|| format!("failed to create wit root: {}", wit_root.display()))?;

    let resolved_at = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
    let mut lock_entries = Vec::with_capacity(dependencies.len());

    for dependency in dependencies {
        let target_rel = PathBuf::from("wit")
            .join("deps")
            .join(plugin_sources::sanitize_dependency_name(&dependency.name));
        let target_path = project_root.join(&target_rel);
        fs::create_dir_all(&target_path).with_context(|| {
            format!(
                "failed to create dependency wit output dir: {}",
                target_path.display()
            )
        })?;
        plugin_sources::materialize_wit_source(
            project_root,
            &dependency.wit.source,
            dependency.wit.registry.as_deref(),
            &dependency.version,
            &target_path,
        )
        .await
        .with_context(|| format!("failed to resolve dependency '{}'", dependency.name))?;

        let digest = build::compute_path_digest_hex(&target_path)?;
        let (component_source, component_registry, component_sha256) = match dependency.kind {
            ManifestDependencyKind::Native => (None, None, None),
            ManifestDependencyKind::Wasm => {
                let component = dependency.component.as_ref().ok_or_else(|| {
                    anyhow!(
                        "dependencies entry '{}' is missing component configuration",
                        dependency.name
                    )
                })?;
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
        version: build::default_imago_lock_version(),
        dependencies: lock_entries,
    };
    let lock_bytes = toml::to_string_pretty(&lock).context("failed to serialize imago.lock")?;
    let lock_path = project_root.join("imago.lock");
    fs::write(&lock_path, lock_bytes)
        .with_context(|| format!("failed to write {}", lock_path.display()))?;

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
        assert_eq!(lock.version, 2);
        assert_eq!(lock.dependencies.len(), 1);
        let entry = &lock.dependencies[0];
        assert_eq!(entry.name, "yieldspace:plugin/example");
        assert_eq!(entry.wit_source, "file://registry/example.wit");
        assert_eq!(entry.wit_registry, None);
        assert_eq!(entry.wit_path, "wit/deps/yieldspace_plugin_example");
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
            &root.join(".imago/warg/yieldspace_plugin_example/1.2.3/wit.wit"),
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
            "wit/deps/yieldspace_plugin_example"
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
            &root.join(".imago/warg/chikoski_hello/0.1.0/wit.wasm"),
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
            root.join("wit/deps/chikoski_hello/package.wit").exists(),
            "top-level package should be materialized"
        );
        assert!(
            root.join("wit/deps/chikoski_name/package.wit").exists(),
            "transitive package should be materialized"
        );
        assert!(
            root.join("wit/deps/chikoski_hello/.imago_transitive/chikoski_name/package.wit")
                .exists(),
            "dep snapshot should include transitive package"
        );

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
            &root.join(".imago/warg/chikoski_hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/chikoski_hello/package.wit").exists());
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
            &root.join(".imago/warg/chikoski_hello/0.1.0/wit.wasm"),
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
            &root.join(".imago/warg/chikoski_hello/0.1.0/wit.wasm"),
            &wit_package_bytes,
        );

        let result = run_with_project_root(UpdateArgs {}, &root);
        assert!(
            result.exit_code == 0,
            "update should succeed: {:?}",
            result.stderr
        );
        assert!(root.join("wit/deps/chikoski_name/package.wit").exists());
        assert!(
            root.join("wit/deps/chikoski_hello/.imago_transitive/chikoski_name/package.wit")
                .exists()
        );

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
        assert!(root.join("wit/deps/yieldspace_plugin_example/example.wit").exists());
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
            &root.join(".imago/warg/chikoski_hello/0.1.0/wit.wit"),
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
