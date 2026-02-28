//! Lockfile types and resolution helpers for dependency/build determinism.
//!
//! `imago.lock` is the resolved-state contract used by `imago deps sync`,
//! `imago build`, and deploy-time artifact assembly.

mod hash;
mod resolve;
mod types;
mod validation;

pub use resolve::{
    collect_wit_packages, load_from_project_root, resolve_binding_wits, resolve_dependencies,
    save_to_project_root,
};
pub use types::{
    BindingWitExpectation, ComponentExpectation, DependencyExpectation, IMAGO_LOCK_VERSION,
    ImagoLock, ImagoLockBindingWit, ImagoLockDependency, ImagoLockWitPackage,
    ImagoLockWitPackageVersion, ResolvedBindingWit, ResolvedDependency, TransitivePackageRecord,
    default_lock_version,
};

#[cfg(test)]
pub(crate) use hash::{compute_path_digest_hex, compute_sha256_hex};
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-lockfile-tests-{}-{}-{}",
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

    #[test]
    fn resolve_dependencies_rejects_unsupported_lock_version() {
        let root = new_temp_dir("unsupported-version");
        let lock = ImagoLock {
            version: 2,
            dependencies: vec![],
            binding_wits: vec![],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(&root, &lock, &[]).expect_err("must fail");
        assert!(err.to_string().contains("version '2' is not supported"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_dependency_mismatch() {
        let root = new_temp_dir("dep-mismatch");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.2.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("does not match lock version"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_absolute_dependency_wit_path() {
        let root = new_temp_dir("dep-absolute-wit-path");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: "deadbeef".to_string(),
                wit_path: "/tmp/evil".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("absolute wit_path must fail");
        assert!(
            err.to_string()
                .contains("must be a relative path under wit/deps"),
            "unexpected error: {err:#}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_parent_traversal_dependency_wit_path() {
        let root = new_temp_dir("dep-parent-wit-path");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: "deadbeef".to_string(),
                wit_path: "wit/deps/../../evil".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("parent traversal wit_path must fail");
        assert!(
            err.to_string().contains("contains invalid path components"),
            "unexpected error: {err:#}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_transitive_digest_mismatch() {
        let root = new_temp_dir("transitive-digest-mismatch");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest:
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["demo:test".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(
            err.to_string()
                .contains("lock digest mismatch for transitive wit package")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_absolute_transitive_wit_path() {
        let root = new_temp_dir("transitive-absolute-path");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest:
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    source: None,
                    path: "/tmp/evil".to_string(),
                    via: vec!["demo:test".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("absolute transitive wit path must fail");
        assert!(
            err.to_string()
                .contains("must be a relative path under wit/deps"),
            "unexpected error: {err:#}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_unknown_via_dependency() {
        let root = new_temp_dir("unknown-via");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/transitive/package.wit")).expect("digest")
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest: transitive_digest,
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["demo:other".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("via contains unknown dependency"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_via_dependency_not_in_expectations_even_if_in_lock() {
        let root = new_temp_dir("stale-lock-via");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/transitive/package.wit")).expect("digest")
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![
                ImagoLockDependency {
                    name: "demo:test".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/demo.wit".to_string(),
                    wit_registry: None,
                    wit_digest: digest.clone(),
                    wit_path: "wit/deps/demo".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                },
                ImagoLockDependency {
                    name: "ghost:pkg".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "file://registry/ghost.wit".to_string(),
                    wit_registry: None,
                    wit_digest: digest,
                    wit_path: "wit/deps/ghost".to_string(),
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                },
            ],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest: transitive_digest,
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["ghost:pkg".to_string()],
                }],
            }],
        };
        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("must fail on stale lock via dependency");
        assert!(err.to_string().contains("via contains unknown dependency"));
        assert!(err.to_string().contains("ghost:pkg"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_accepts_via_for_path_expectation_resolved_to_lock_name() {
        let root = new_temp_dir("path-expectation-via-lock-name");
        write(
            &root.join("wit/deps/actual-pkg/package.wit"),
            b"package actual:pkg@0.1.0;\n",
        );
        write(
            &root.join("wit/deps/transitive/package.wit"),
            b"package transitive:dep;\n",
        );
        let direct_digest =
            compute_path_digest_hex(&root.join("wit/deps/actual-pkg")).expect("digest");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/transitive/package.wit")).expect("digest")
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "actual:pkg".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "registry/example".to_string(),
                wit_registry: None,
                wit_digest: direct_digest,
                wit_path: "wit/deps/actual-pkg".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest: transitive_digest,
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["actual:pkg".to_string()],
                }],
            }],
        };

        let resolved = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "path-source-0".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "registry/example".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect("via should accept resolved lock dependency name");
        assert_eq!(resolved.len(), 1);
        let entry = resolved
            .get("path-source-0")
            .expect("path expectation should resolve");
        assert_eq!(entry.name, "actual:pkg");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_rejects_reusing_lock_entry_for_multiple_path_expectations() {
        let root = new_temp_dir("path-expectation-lock-reuse");
        write(
            &root.join("wit/deps/actual-pkg/package.wit"),
            b"package actual:pkg@0.1.0;\n",
        );
        let direct_digest =
            compute_path_digest_hex(&root.join("wit/deps/actual-pkg")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "actual:pkg".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "registry/example".to_string(),
                wit_registry: None,
                wit_digest: direct_digest,
                wit_path: "wit/deps/actual-pkg".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![],
        };

        let err = resolve_dependencies(
            &root,
            &lock,
            &[
                DependencyExpectation {
                    name: "path-source-0".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "registry/example".to_string(),
                    wit_registry: None,
                    component: None,
                },
                DependencyExpectation {
                    name: "path-source-1".to_string(),
                    version: "0.1.0".to_string(),
                    wit_source: "registry/example".to_string(),
                    wit_registry: None,
                    component: None,
                },
            ],
        )
        .expect_err("must fail when one lock dependency is matched more than once");
        assert!(
            err.to_string()
                .contains("already matched by another dependency")
        );
        assert!(err.to_string().contains("actual:pkg"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_dependencies_accepts_empty_via_for_auto_wasi_records() {
        let root = new_temp_dir("empty-via-allowed");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        write(
            &root.join("wit/deps/wasi-io/package.wit"),
            b"package wasi:io@0.2.6;\ninterface streams { read: func(); }\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");
        let transitive_digest = format!(
            "sha256:{}",
            compute_sha256_hex(&root.join("wit/deps/wasi-io/package.wit")).expect("digest")
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "wasi:io".to_string(),
                registry: Some("wasi.dev".to_string()),
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "=0.2.6".to_string(),
                    version: Some("0.2.6".to_string()),
                    digest: transitive_digest,
                    source: Some("wasi:io".to_string()),
                    path: "wit/deps/wasi-io".to_string(),
                    via: vec![],
                }],
            }],
        };

        let resolved = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect("empty via should be accepted");
        assert!(resolved.contains_key("demo:test"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_from_project_root_rejects_legacy_resolved_at_fields() {
        let root = new_temp_dir("legacy-resolved-at");
        write(
            &root.join("imago.lock"),
            br#"
version = 1

[[dependencies]]
name = "demo:test"
version = "0.1.0"
wit_source = "file://registry/demo.wit"
wit_digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
wit_path = "wit/deps/demo"
resolved_at = "1700000000"

[[binding_wits]]
name = "svc"
wit_source = "file://registry/demo.wit"
wit_digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
wit_path = "wit/deps/demo"
interfaces = ["demo:test/greet"]
resolved_at = "1700000000"
"#,
        );

        let err = load_from_project_root(&root).expect_err("legacy resolved_at must fail");
        let error_text = format!("{err:#}");
        assert!(
            error_text.contains("unknown field") && error_text.contains("resolved_at"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dependencies_rejects_symlinked_dependency_wit_path() {
        let root = new_temp_dir("symlinked-direct-wit-path");
        let outside = root.join("outside");
        write(&outside.join("package.wit"), b"package outside:dep;\n");
        fs::create_dir_all(root.join("wit/deps")).expect("wit/deps should be creatable");
        symlink(&outside, root.join("wit/deps/demo")).expect("symlink should be creatable");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: "deadbeef".to_string(),
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![],
        };

        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("symlinked dependency wit path must fail");
        assert!(
            err.to_string().contains("resolves through symlink"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dependencies_rejects_symlinked_transitive_wit_path() {
        let root = new_temp_dir("symlinked-transitive-wit-path");
        write(
            &root.join("wit/deps/demo/package.wit"),
            b"package demo:test;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/demo")).expect("digest");

        let outside = root.join("outside-transitive");
        write(&outside.join("package.wit"), b"package outside:dep;\n");
        fs::create_dir_all(root.join("wit/deps")).expect("wit/deps should be creatable");
        symlink(&outside, root.join("wit/deps/transitive")).expect("symlink should be creatable");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![ImagoLockDependency {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                wit_digest: digest,
                wit_path: "wit/deps/demo".to_string(),
                component_source: None,
                component_registry: None,
                component_sha256: None,
            }],
            binding_wits: vec![],
            wit_packages: vec![ImagoLockWitPackage {
                name: "transitive:dep".to_string(),
                registry: None,
                versions: vec![ImagoLockWitPackageVersion {
                    requirement: "*".to_string(),
                    version: None,
                    digest:
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    source: None,
                    path: "wit/deps/transitive".to_string(),
                    via: vec!["demo:test".to_string()],
                }],
            }],
        };

        let err = resolve_dependencies(
            &root,
            &lock,
            &[DependencyExpectation {
                name: "demo:test".to_string(),
                version: "0.1.0".to_string(),
                wit_source: "file://registry/demo.wit".to_string(),
                wit_registry: None,
                component: None,
            }],
        )
        .expect_err("symlinked transitive wit path must fail");
        assert!(
            err.to_string().contains("resolves through symlink"),
            "unexpected error: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_success() {
        let root = new_temp_dir("binding-wits-success");
        write(
            &root.join("wit/deps/alpha/package.wit"),
            b"package alpha:bindings;\n",
        );
        write(
            &root.join("wit/deps/beta/package.wit"),
            b"package beta:bindings;\n",
        );
        let alpha_digest = compute_path_digest_hex(&root.join("wit/deps/alpha")).expect("digest");
        let beta_digest = compute_path_digest_hex(&root.join("wit/deps/beta")).expect("digest");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![
                ImagoLockBindingWit {
                    name: "beta".to_string(),
                    wit_source: "beta:bindings".to_string(),
                    wit_registry: Some("wa.dev".to_string()),
                    wit_version: "0.1.0".to_string(),
                    wit_digest: beta_digest.clone(),
                    wit_path: "wit/deps/beta".to_string(),
                    interfaces: vec!["beta/pkg".to_string()],
                },
                ImagoLockBindingWit {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_version: "0.1.0".to_string(),
                    wit_digest: alpha_digest.clone(),
                    wit_path: "wit/deps/alpha".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                },
            ],
            wit_packages: vec![],
        };

        let resolved = resolve_binding_wits(
            &root,
            &lock,
            &[
                BindingWitExpectation {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_version: "0.1.0".to_string(),
                },
                BindingWitExpectation {
                    name: "beta".to_string(),
                    wit_source: "beta:bindings".to_string(),
                    wit_registry: Some("wa.dev".to_string()),
                    wit_version: "0.1.0".to_string(),
                },
            ],
        )
        .expect("binding wits should resolve");

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "alpha");
        assert_eq!(resolved[0].wit_digest, alpha_digest);
        assert_eq!(resolved[1].name, "beta");
        assert_eq!(resolved[1].wit_digest, beta_digest);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_rejects_missing_lock_entry() {
        let root = new_temp_dir("binding-wits-missing");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![],
            wit_packages: vec![],
        };

        let err = resolve_binding_wits(
            &root,
            &lock,
            &[BindingWitExpectation {
                name: "missing".to_string(),
                wit_source: "file://registry/missing.wit".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
            }],
        )
        .expect_err("missing lock entry must fail");
        assert!(err.to_string().contains("is not resolved in imago.lock"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_rejects_digest_mismatch() {
        let root = new_temp_dir("binding-wits-digest-mismatch");
        write(
            &root.join("wit/deps/alpha/package.wit"),
            b"package alpha:bindings;\n",
        );
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![ImagoLockBindingWit {
                name: "alpha".to_string(),
                wit_source: "file://registry/alpha.wit".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
                wit_digest: "deadbeef".to_string(),
                wit_path: "wit/deps/alpha".to_string(),
                interfaces: vec!["alpha/export".to_string()],
            }],
            wit_packages: vec![],
        };

        let err = resolve_binding_wits(
            &root,
            &lock,
            &[BindingWitExpectation {
                name: "alpha".to_string(),
                wit_source: "file://registry/alpha.wit".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
            }],
        )
        .expect_err("digest mismatch must fail");
        assert!(err.to_string().contains("lock digest mismatch"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_rejects_invalid_interface_format() {
        let root = new_temp_dir("binding-wits-invalid-interface");
        write(
            &root.join("wit/deps/alpha/package.wit"),
            b"package alpha:bindings;\n",
        );
        let digest = compute_path_digest_hex(&root.join("wit/deps/alpha")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![ImagoLockBindingWit {
                name: "alpha".to_string(),
                wit_source: "file://registry/alpha.wit".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
                wit_digest: digest,
                wit_path: "wit/deps/alpha".to_string(),
                interfaces: vec!["invalid-interface".to_string()],
            }],
            wit_packages: vec![],
        };

        let err = resolve_binding_wits(
            &root,
            &lock,
            &[BindingWitExpectation {
                name: "alpha".to_string(),
                wit_source: "file://registry/alpha.wit".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
            }],
        )
        .expect_err("invalid interface format must fail");
        assert!(err.to_string().contains("<package>/<interface>"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_rejects_duplicate_lock_entries() {
        let root = new_temp_dir("binding-wits-duplicate-lock");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![
                ImagoLockBindingWit {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_version: "0.1.0".to_string(),
                    wit_digest: "a".repeat(64),
                    wit_path: "wit/deps/alpha".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                },
                ImagoLockBindingWit {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_version: "0.1.0".to_string(),
                    wit_digest: "b".repeat(64),
                    wit_path: "wit/deps/alpha-copy".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                },
            ],
            wit_packages: vec![],
        };

        let err = resolve_binding_wits(&root, &lock, &[]).expect_err("duplicate lock must fail");
        assert!(
            err.to_string()
                .contains("contains duplicate binding_wits entry")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_binding_wits_accepts_path_source_with_at_character() {
        let root = new_temp_dir("binding-wits-path-source-with-at");
        write(
            &root.join("wit/deps/alpha/package.wit"),
            b"package alpha:bindings;\n",
        );
        let alpha_digest = compute_path_digest_hex(&root.join("wit/deps/alpha")).expect("digest");
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            dependencies: vec![],
            binding_wits: vec![ImagoLockBindingWit {
                name: "alpha".to_string(),
                wit_source: "registry/bind@dev".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
                wit_digest: alpha_digest.clone(),
                wit_path: "wit/deps/alpha".to_string(),
                interfaces: vec!["alpha/export".to_string()],
            }],
            wit_packages: vec![],
        };

        let resolved = resolve_binding_wits(
            &root,
            &lock,
            &[BindingWitExpectation {
                name: "alpha".to_string(),
                wit_source: "registry/bind@dev".to_string(),
                wit_registry: None,
                wit_version: "0.1.0".to_string(),
            }],
        )
        .expect("binding wit path source with '@' should resolve");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "alpha");
        assert_eq!(resolved[0].wit_digest, alpha_digest);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn collect_wit_packages_groups_and_sorts_deterministically() {
        let packages = collect_wit_packages(vec![
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wa.dev".to_string()),
                requirement: "=0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.0".to_string()),
                path: "wit/deps/wasi_io".to_string(),
                via: "a:dep".to_string(),
            },
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wa.dev".to_string()),
                requirement: "=0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.0".to_string()),
                path: "wit/deps/wasi_io".to_string(),
                via: "b:dep".to_string(),
            },
        ]);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "wasi:io");
        assert_eq!(packages[0].versions.len(), 1);
        assert_eq!(
            packages[0].versions[0].via,
            vec!["a:dep".to_string(), "b:dep".to_string()]
        );
    }

    #[test]
    fn collect_wit_packages_drops_empty_via_when_non_empty_exists_and_keeps_empty_only() {
        let packages = collect_wit_packages(vec![
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "=0.2.6".to_string(),
                version: Some("0.2.6".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.6".to_string()),
                path: "wit/deps/wasi-io".to_string(),
                via: "".to_string(),
            },
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "=0.2.6".to_string(),
                version: Some("0.2.6".to_string()),
                digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                source: Some("warg://wasi:io@0.2.6".to_string()),
                path: "wit/deps/wasi-io".to_string(),
                via: "demo:test".to_string(),
            },
            TransitivePackageRecord {
                name: "wasi:random".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "=0.2.6".to_string(),
                version: Some("0.2.6".to_string()),
                digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
                source: Some("warg://wasi:random@0.2.6".to_string()),
                path: "wit/deps/wasi-random".to_string(),
                via: "".to_string(),
            },
        ]);

        let wasi_io = packages
            .iter()
            .find(|package| package.name == "wasi:io")
            .expect("wasi:io should exist");
        assert_eq!(wasi_io.versions[0].via, vec!["demo:test".to_string()]);

        let wasi_random = packages
            .iter()
            .find(|package| package.name == "wasi:random")
            .expect("wasi:random should exist");
        assert!(wasi_random.versions[0].via.is_empty());
    }
}
