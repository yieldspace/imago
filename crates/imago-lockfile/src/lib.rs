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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                resolved_at: "0".to_string(),
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
                    wit_source: "warg://beta@0.1.0".to_string(),
                    wit_registry: Some("wa.dev".to_string()),
                    wit_digest: beta_digest.clone(),
                    wit_path: "wit/deps/beta".to_string(),
                    interfaces: vec!["beta/pkg".to_string()],
                    resolved_at: "1".to_string(),
                },
                ImagoLockBindingWit {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_digest: alpha_digest.clone(),
                    wit_path: "wit/deps/alpha".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                    resolved_at: "2".to_string(),
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
                },
                BindingWitExpectation {
                    name: "beta".to_string(),
                    wit_source: "warg://beta@0.1.0".to_string(),
                    wit_registry: Some("wa.dev".to_string()),
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
                wit_digest: "deadbeef".to_string(),
                wit_path: "wit/deps/alpha".to_string(),
                interfaces: vec!["alpha/export".to_string()],
                resolved_at: "0".to_string(),
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
                wit_digest: digest,
                wit_path: "wit/deps/alpha".to_string(),
                interfaces: vec!["invalid-interface".to_string()],
                resolved_at: "0".to_string(),
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
                    wit_digest: "a".repeat(64),
                    wit_path: "wit/deps/alpha".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                    resolved_at: "0".to_string(),
                },
                ImagoLockBindingWit {
                    name: "alpha".to_string(),
                    wit_source: "file://registry/alpha.wit".to_string(),
                    wit_registry: None,
                    wit_digest: "b".repeat(64),
                    wit_path: "wit/deps/alpha-copy".to_string(),
                    interfaces: vec!["alpha/export".to_string()],
                    resolved_at: "1".to_string(),
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
}
