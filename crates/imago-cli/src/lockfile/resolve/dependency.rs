use std::{collections::BTreeMap, path::Path};

use anyhow::anyhow;

use crate::lockfile::{DependencyExpectation, ImagoLock, ResolvedDependency};

use super::super::{hash::DigestProvider, validation::PathVerifier};
use super::packages::verify_resolved_packages_and_edges;
use super::{
    build_requested_snapshot, compute_dependency_request_id, ensure_supported_lock_version,
    new_digest_provider, new_path_verifier, validate_resolved_component_against_requested,
    validate_resolved_wit_tree,
};

pub fn resolve_dependencies(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[DependencyExpectation],
) -> anyhow::Result<BTreeMap<String, ResolvedDependency>> {
    let digest_provider = new_digest_provider();
    let path_verifier = new_path_verifier();
    resolve_dependencies_with(
        project_root,
        lock,
        expectations,
        &digest_provider,
        &path_verifier,
    )
}

fn resolve_dependencies_with(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[DependencyExpectation],
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<BTreeMap<String, ResolvedDependency>> {
    ensure_supported_lock_version(lock.version)?;

    let expected_requested = build_requested_snapshot(expectations, &[], None)?;
    let mut requested_by_id = BTreeMap::new();
    for requested in &lock.requested.dependencies {
        if requested_by_id
            .insert(requested.id.clone(), requested)
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.requested.dependencies contains duplicate id '{}'; run `imago deps sync`",
                requested.id
            ));
        }
    }

    let expected_by_id = expected_requested
        .dependencies
        .iter()
        .map(|expected| (expected.id.clone(), expected))
        .collect::<BTreeMap<_, _>>();

    if requested_by_id.len() != expected_by_id.len() {
        return Err(anyhow!(
            "dependency request set does not match imago.lock.requested; run `imago deps sync`"
        ));
    }
    for (request_id, expected) in &expected_by_id {
        let actual = requested_by_id.get(request_id).ok_or_else(|| {
            anyhow!(
                "dependency request '{}' is missing in imago.lock.requested; run `imago deps sync`",
                request_id
            )
        })?;
        if actual != expected {
            return Err(anyhow!(
                "dependency request '{}' differs from imago.lock.requested; run `imago deps sync`",
                request_id
            ));
        }
    }

    let mut resolved_by_request_id = BTreeMap::new();
    for resolved in &lock.resolved.dependencies {
        if resolved_by_request_id
            .insert(
                resolved.request_id.clone(),
                ResolvedDependency::from(resolved),
            )
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.resolved.dependencies contains duplicate request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        let requested = requested_by_id.get(&resolved.request_id).ok_or_else(|| {
            anyhow!(
                "imago.lock.resolved.dependencies contains unknown request_id '{}'; run `imago deps sync`",
                resolved.request_id
            )
        })?;

        for requires in &resolved.requires_request_ids {
            if !requested_by_id.contains_key(requires) {
                return Err(anyhow!(
                    "imago.lock.resolved.dependencies['{}'].requires_request_ids contains unknown request_id '{}'; run `imago deps sync`",
                    resolved.request_id,
                    requires
                ));
            }
        }

        validate_resolved_wit_tree(
            project_root,
            &resolved.wit_path,
            &resolved.wit_tree_digest,
            &format!(
                "imago.lock.resolved.dependencies['{}'].wit_path",
                resolved.request_id
            ),
            digest_provider,
            path_verifier,
        )?;
        validate_resolved_component_against_requested(requested, resolved)?;
    }

    verify_resolved_packages_and_edges(project_root, lock, digest_provider, path_verifier)?;

    let mut resolved = BTreeMap::new();
    for expectation in expectations {
        let request_id = compute_dependency_request_id(expectation);
        let entry = resolved_by_request_id.get(&request_id).ok_or_else(|| {
            anyhow!(
                "dependency '{}' is not resolved in imago.lock; run `imago deps sync`",
                expectation.name
            )
        })?;
        resolved.insert(expectation.name.clone(), entry.clone());
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use crate::lockfile::{
        DependencyExpectation, IMAGO_LOCK_VERSION, ImagoLock, ImagoLockResolved,
        ImagoLockResolvedDependency, LockCapabilityPolicy, LockDependencyKind, LockSourceKind,
        build_requested_snapshot, compute_dependency_request_id,
    };

    use super::resolve_dependencies;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-lockfile-resolve-dependency-tests-{test_name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be created");
        }
        fs::write(path, bytes).expect("file should be written");
    }

    fn sample_expectation(kind: LockDependencyKind) -> DependencyExpectation {
        DependencyExpectation {
            name: "path-source-0".to_string(),
            kind,
            version: "0.1.0".to_string(),
            source_kind: LockSourceKind::Path,
            source: "registry/example".to_string(),
            registry: None,
            sha256: None,
            requires: vec![],
            capabilities: LockCapabilityPolicy::default(),
            component: None,
        }
    }

    fn sample_resolved_dependency(
        root: &Path,
        request_id: &str,
        requires_request_ids: Vec<String>,
    ) -> ImagoLockResolvedDependency {
        let wit_path = "wit/deps/path-source-0-0.1.0";
        write(
            &root.join(wit_path).join("package.wit"),
            b"package acme:example@0.1.0;\n",
        );
        let digest = crate::lockfile::hash::compute_path_digest_hex(&root.join(wit_path))
            .expect("digest should compute");
        ImagoLockResolvedDependency {
            request_id: request_id.to_string(),
            resolved_name: "acme:example".to_string(),
            resolved_version: "0.1.0".to_string(),
            wit_path: wit_path.to_string(),
            wit_tree_digest: digest,
            component_source: None,
            component_registry: None,
            component_sha256: None,
            requires_request_ids,
        }
    }

    #[test]
    fn resolve_dependencies_rejects_duplicate_resolved_request_id() {
        let root = new_temp_dir("duplicate");
        let expectation = sample_expectation(LockDependencyKind::Native);
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None)
            .expect("requested snapshot should build");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![
                    sample_resolved_dependency(&root, &request_id, vec![]),
                    sample_resolved_dependency(&root, &request_id, vec![]),
                ],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let err = resolve_dependencies(&root, &lock, &[expectation])
            .expect_err("duplicate request_id should fail");
        assert!(err.to_string().contains("duplicate request_id"));
    }

    #[test]
    fn resolve_dependencies_rejects_unknown_requires_request_id() {
        let root = new_temp_dir("unknown-requires");
        let expectation = sample_expectation(LockDependencyKind::Native);
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None)
            .expect("requested snapshot should build");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![sample_resolved_dependency(
                    &root,
                    &request_id,
                    vec!["dep:unknown".to_string()],
                )],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let err = resolve_dependencies(&root, &lock, &[expectation])
            .expect_err("unknown requires_request_id should fail");
        assert!(err.to_string().contains("contains unknown request_id"));
    }

    #[test]
    fn resolve_dependencies_rejects_wasm_without_component_sha256() {
        let root = new_temp_dir("missing-component-sha");
        let mut expectation = sample_expectation(LockDependencyKind::Wasm);
        expectation.component = Some(crate::lockfile::ComponentExpectation {
            source_kind: LockSourceKind::Path,
            source: "registry/example-component.wasm".to_string(),
            registry: None,
            sha256: None,
        });
        let request_id = compute_dependency_request_id(&expectation);
        let requested = build_requested_snapshot(std::slice::from_ref(&expectation), &[], None)
            .expect("requested snapshot should build");

        let mut resolved = sample_resolved_dependency(&root, &request_id, vec![]);
        resolved.component_source = Some("registry/example-component.wasm".to_string());
        resolved.component_sha256 = None;

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![resolved],
                bindings: vec![],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let err = resolve_dependencies(&root, &lock, &[expectation])
            .expect_err("wasm dependency requires component_sha256");
        assert!(err.to_string().contains("component_sha256 is missing"));
    }
}
