//! Lockfile types and resolution helpers for dependency/build determinism.
//!
//! `imago.lock` is the resolved-state contract used by `imago deps sync`,
//! `imago build`, and deploy-time artifact assembly.

mod hash;
mod resolve;
mod types;
mod validation;

pub use resolve::{
    build_requested_snapshot, collect_resolved_packages_and_edges, compute_binding_request_id,
    compute_dependency_request_id, compute_requested_fingerprint, ensure_requested_fingerprint,
    load_from_project_root, resolve_binding_wits, resolve_dependencies, resolved_package_ref,
    save_to_project_root,
};
pub use types::{
    BindingWitExpectation, ComponentExpectation, DependencyExpectation, IMAGO_LOCK_VERSION,
    ImagoLock, ImagoLockRequested, ImagoLockRequestedBinding, ImagoLockRequestedDependency,
    ImagoLockResolved, ImagoLockResolvedBinding, ImagoLockResolvedDependency,
    ImagoLockResolvedPackage, ImagoLockResolvedPackageEdge, LockCapabilityPolicy,
    LockDependencyKind, LockEdgeFromKind, LockPackageEdgeReason, LockSourceKind,
    ResolvedBindingWit, ResolvedDependency, TransitivePackageRecord, default_lock_version,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, fs, path::Path};

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file write should succeed");
    }

    fn new_temp_dir(test_name: &str) -> std::path::PathBuf {
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

    fn sample_dependency_expectation(name: &str) -> DependencyExpectation {
        DependencyExpectation {
            name: name.to_string(),
            kind: LockDependencyKind::Native,
            version: "0.1.0".to_string(),
            source_kind: LockSourceKind::Wit,
            source: "demo:test".to_string(),
            registry: Some("wa.dev".to_string()),
            sha256: None,
            requires: vec![],
            capabilities: LockCapabilityPolicy::default(),
            component: None,
        }
    }

    #[test]
    fn requested_snapshot_is_deterministic() {
        let mut ns = BTreeMap::new();
        ns.insert("wasi".to_string(), "wasi.dev".to_string());

        let deps_a = vec![
            sample_dependency_expectation("a"),
            sample_dependency_expectation("b"),
        ];
        let deps_b = vec![
            sample_dependency_expectation("b"),
            sample_dependency_expectation("a"),
        ];

        let requested_a = build_requested_snapshot(&deps_a, &[], Some(&ns));
        let requested_b = build_requested_snapshot(&deps_b, &[], Some(&ns));

        assert_eq!(requested_a.fingerprint, requested_b.fingerprint);
        assert_eq!(requested_a.dependencies, requested_b.dependencies);
    }

    #[test]
    fn dependency_request_id_changes_when_requires_changes() {
        let mut dependency_a = sample_dependency_expectation("dep");
        dependency_a.requires = vec!["foo:bar".to_string()];
        let mut dependency_b = sample_dependency_expectation("dep");
        dependency_b.requires = vec!["foo:baz".to_string()];

        assert_ne!(
            compute_dependency_request_id(&dependency_a),
            compute_dependency_request_id(&dependency_b)
        );
    }

    #[test]
    fn dependency_request_id_changes_when_capabilities_change() {
        let mut dependency_a = sample_dependency_expectation("dep");
        dependency_a.capabilities.privileged = false;
        let mut dependency_b = sample_dependency_expectation("dep");
        dependency_b.capabilities.privileged = true;

        assert_ne!(
            compute_dependency_request_id(&dependency_a),
            compute_dependency_request_id(&dependency_b)
        );
    }

    #[test]
    fn dependency_request_id_changes_when_source_kind_changes() {
        let dependency_a = sample_dependency_expectation("dep");
        let mut dependency_b = sample_dependency_expectation("dep");
        dependency_b.source_kind = LockSourceKind::Path;

        assert_ne!(
            compute_dependency_request_id(&dependency_a),
            compute_dependency_request_id(&dependency_b)
        );
    }

    #[test]
    fn dependency_request_id_is_stable_for_reordered_requires_and_capabilities() {
        let mut dependency_a = sample_dependency_expectation("dep");
        dependency_a.requires = vec![
            "foo:b".to_string(),
            "foo:a".to_string(),
            "foo:b".to_string(),
        ];
        dependency_a
            .capabilities
            .deps
            .insert("*".to_string(), vec!["b".to_string(), "a".to_string()]);
        dependency_a.capabilities.wasi.insert(
            "io".to_string(),
            vec!["streams".to_string(), "poll".to_string()],
        );

        let mut dependency_b = sample_dependency_expectation("dep");
        dependency_b.requires = vec![
            "foo:a".to_string(),
            "foo:b".to_string(),
            "foo:a".to_string(),
        ];
        dependency_b
            .capabilities
            .deps
            .insert("*".to_string(), vec!["a".to_string(), "b".to_string()]);
        dependency_b.capabilities.wasi.insert(
            "io".to_string(),
            vec!["poll".to_string(), "streams".to_string()],
        );

        assert_eq!(
            compute_dependency_request_id(&dependency_a),
            compute_dependency_request_id(&dependency_b)
        );
    }

    #[test]
    fn resolve_dependencies_uses_request_id() {
        let root = new_temp_dir("resolve-dependency");
        write(
            &root.join("wit/deps/demo-test-0.1.0/package.wit"),
            b"package demo:test@0.1.0;\n",
        );

        let expectation = sample_dependency_expectation("path-source-0");
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None);

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![ImagoLockResolvedDependency {
                    request_id: request_id.clone(),
                    resolved_name: "demo:test".to_string(),
                    resolved_version: "0.1.0".to_string(),
                    wit_path: "wit/deps/demo-test-0.1.0".to_string(),
                    wit_tree_digest: {
                        use crate::hash::compute_path_digest_hex;
                        compute_path_digest_hex(&root.join("wit/deps/demo-test-0.1.0"))
                            .expect("digest")
                    },
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                    requires_request_ids: vec![],
                }],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let resolved =
            resolve_dependencies(&root, &lock, std::slice::from_ref(&expectation)).expect("ok");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved["path-source-0"].request_id, request_id);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_resolved_package_ref_mismatch_with_name_version_registry() {
        let root = new_temp_dir("package-ref-mismatch");
        write(
            &root.join("wit/deps/demo-test-0.1.0/package.wit"),
            b"package demo:test@0.1.0;\n",
        );
        write(
            &root.join("wit/deps/test-dep/package.wit"),
            b"package test:dep;\n",
        );

        let expectation = sample_dependency_expectation("path-source-0");
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None);
        let wit_tree_digest = {
            use crate::hash::compute_path_digest_hex;
            compute_path_digest_hex(&root.join("wit/deps/demo-test-0.1.0")).expect("digest")
        };
        let transitive_digest = format!(
            "sha256:{}",
            crate::hash::compute_sha256_hex(&root.join("wit/deps/test-dep/package.wit"))
                .expect("digest")
        );

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![ImagoLockResolvedDependency {
                    request_id,
                    resolved_name: "demo:test".to_string(),
                    resolved_version: "0.1.0".to_string(),
                    wit_path: "wit/deps/demo-test-0.1.0".to_string(),
                    wit_tree_digest,
                    component_source: None,
                    component_registry: None,
                    component_sha256: None,
                    requires_request_ids: vec![],
                }],
                bindings: vec![],
                packages: vec![ImagoLockResolvedPackage {
                    package_ref: "test:dep@0.1.0#".to_string(),
                    name: "test:dep".to_string(),
                    version: None,
                    registry: None,
                    requirement: "*".to_string(),
                    source: None,
                    path: "wit/deps/test-dep".to_string(),
                    digest: transitive_digest,
                }],
                package_edges: vec![],
            },
        };

        let err = resolve_dependencies(&root, &lock, std::slice::from_ref(&expectation))
            .expect_err("must fail on non-canonical package_ref");
        assert!(err.to_string().contains("non-canonical package_ref"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_requested_fingerprint_detects_mismatch() {
        let expectation = sample_dependency_expectation("a");
        let mut lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested: build_requested_snapshot(std::slice::from_ref(&expectation), &[], None),
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };
        lock.requested.fingerprint = "deadbeef".to_string();

        let err = ensure_requested_fingerprint(&lock, &[expectation], &[], None)
            .expect_err("must fail on mismatch");
        assert!(err.to_string().contains("requested fingerprint mismatch"));
    }

    #[test]
    fn load_from_project_root_rejects_old_lock_shape() {
        let root = new_temp_dir("old-shape");
        write(
            &root.join("imago.lock"),
            br#"
version = 1

[[dependencies]]
name = "demo:test"
version = "0.1.0"
wit_source = "demo:test"
wit_digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
wit_path = "wit/deps/demo"
"#,
        );

        let err = load_from_project_root(&root).expect_err("old shape should fail parse");
        assert!(err.to_string().contains("failed to parse imago.lock"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unknown_package_edge_reason() {
        let root = new_temp_dir("unknown-edge-reason");
        write(
            &root.join("imago.lock"),
            br#"
version = 1

[requested]
fingerprint = "f"

[[requested.dependencies]]
id = "dep:1"
kind = "native"
version = "0.1.0"
source_kind = "wit"
source = "demo:test"
declared_requires = []

[resolved]
dependencies = []
bindings = []
packages = []

[[resolved.package_edges]]
from_kind = "dependency"
from_ref = "dep:1"
to_package_ref = "demo:test@0.1.0#wa.dev"
reason = "not-allowed"
"#,
        );

        let err = load_from_project_root(&root).expect_err("unknown edge reason must fail parse");
        assert!(err.to_string().contains("failed to parse imago.lock"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_known_package_edge_reason_values_roundtrip() {
        let reasons = [
            LockPackageEdgeReason::DeclaredRequires,
            LockPackageEdgeReason::WitImport,
            LockPackageEdgeReason::ComponentWorld,
            LockPackageEdgeReason::AutoWasi,
            LockPackageEdgeReason::WitDirClosure,
        ];

        for reason in reasons {
            let encoded = toml::to_string(&ImagoLockResolvedPackageEdge {
                from_kind: LockEdgeFromKind::Dependency,
                from_ref: "dep:1".to_string(),
                to_package_ref: "demo:test@0.1.0#wa.dev".to_string(),
                reason,
            })
            .expect("edge should serialize");
            let decoded: ImagoLockResolvedPackageEdge =
                toml::from_str(&encoded).expect("edge should deserialize");
            assert_eq!(decoded.reason, reason);
        }
    }
}
