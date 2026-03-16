use std::{collections::BTreeMap, path::Path};

use anyhow::anyhow;

use crate::lockfile::{BindingWitExpectation, ImagoLock, ResolvedBindingWit};

use super::super::{hash::DigestProvider, validation::PathVerifier};
use super::packages::verify_resolved_packages_and_edges;
use super::{
    build_requested_snapshot, compute_binding_request_id, ensure_supported_lock_version,
    new_digest_provider, new_path_verifier, validate_binding_interfaces,
    validate_resolved_wit_tree,
};

pub fn resolve_binding_wits(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[BindingWitExpectation],
) -> anyhow::Result<Vec<ResolvedBindingWit>> {
    let digest_provider = new_digest_provider();
    let path_verifier = new_path_verifier();
    resolve_binding_wits_with(
        project_root,
        lock,
        expectations,
        &digest_provider,
        &path_verifier,
    )
}

fn resolve_binding_wits_with(
    project_root: &Path,
    lock: &ImagoLock,
    expectations: &[BindingWitExpectation],
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<Vec<ResolvedBindingWit>> {
    ensure_supported_lock_version(lock.version)?;

    let expected_requested = build_requested_snapshot(&[], expectations, &[], None)?;

    let mut requested_by_id = BTreeMap::new();
    for requested in &lock.requested.bindings {
        if requested_by_id
            .insert(requested.id.clone(), requested)
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.requested.bindings contains duplicate id '{}'; run `imago deps sync`",
                requested.id
            ));
        }
    }

    let expected_by_id = expected_requested
        .bindings
        .iter()
        .map(|expected| (expected.id.clone(), expected))
        .collect::<BTreeMap<_, _>>();

    if requested_by_id.len() != expected_by_id.len() {
        return Err(anyhow!(
            "binding request set does not match imago.lock.requested; run `imago deps sync`"
        ));
    }
    for (request_id, expected) in &expected_by_id {
        let actual = requested_by_id.get(request_id).ok_or_else(|| {
            anyhow!(
                "binding request '{}' is missing in imago.lock.requested; run `imago deps sync`",
                request_id
            )
        })?;
        if actual != expected {
            return Err(anyhow!(
                "binding request '{}' differs from imago.lock.requested; run `imago deps sync`",
                request_id
            ));
        }
    }

    let mut resolved_by_request_id = BTreeMap::new();
    for resolved in &lock.resolved.bindings {
        if resolved_by_request_id
            .insert(
                resolved.request_id.clone(),
                ResolvedBindingWit::from(resolved),
            )
            .is_some()
        {
            return Err(anyhow!(
                "imago.lock.resolved.bindings contains duplicate request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        if !requested_by_id.contains_key(&resolved.request_id) {
            return Err(anyhow!(
                "imago.lock.resolved.bindings contains unknown request_id '{}'; run `imago deps sync`",
                resolved.request_id
            ));
        }

        validate_resolved_wit_tree(
            project_root,
            &resolved.wit_path,
            &resolved.wit_tree_digest,
            &format!(
                "imago.lock.resolved.bindings['{}'].wit_path",
                resolved.request_id
            ),
            digest_provider,
            path_verifier,
        )?;

        validate_binding_interfaces(resolved)?;
    }

    verify_resolved_packages_and_edges(project_root, lock, digest_provider, path_verifier)?;

    let mut resolved = Vec::with_capacity(expectations.len());
    for expectation in expectations {
        let request_id = compute_binding_request_id(expectation);
        let entry = resolved_by_request_id.get(&request_id).ok_or_else(|| {
            anyhow!(
                "binding '{}' is not resolved in imago.lock; run `imago deps sync`",
                expectation.name
            )
        })?;
        resolved.push(entry.clone());
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
        BindingWitExpectation, IMAGO_LOCK_VERSION, ImagoLock, ImagoLockResolved,
        ImagoLockResolvedBinding, LockSourceKind, build_requested_snapshot,
        compute_binding_request_id,
    };

    use super::resolve_binding_wits;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "imago-cli-lockfile-resolve-binding-tests-{test_name}-{}-{}",
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

    fn sample_expectation() -> BindingWitExpectation {
        BindingWitExpectation {
            name: "svc".to_string(),
            source_kind: LockSourceKind::Path,
            source: "registry/binding".to_string(),
            registry: None,
            version: "0.1.0".to_string(),
            sha256: None,
        }
    }

    fn sample_resolved_binding(
        root: &Path,
        request_id: &str,
        wit_path: &str,
        interfaces: Vec<String>,
    ) -> ImagoLockResolvedBinding {
        write(
            &root.join(wit_path).join("package.wit"),
            b"package acme:binding@0.1.0;\n",
        );
        let digest = crate::lockfile::hash::compute_path_digest_hex(&root.join(wit_path))
            .expect("digest should compute");
        ImagoLockResolvedBinding {
            request_id: request_id.to_string(),
            name: "svc".to_string(),
            resolved_package: "acme:binding".to_string(),
            resolved_version: Some("0.1.0".to_string()),
            wit_path: wit_path.to_string(),
            wit_tree_digest: digest,
            interfaces,
        }
    }

    #[test]
    fn resolve_binding_wits_rejects_duplicate_resolved_request_id() {
        let root = new_temp_dir("duplicate");
        let expectation = sample_expectation();
        let request_id = compute_binding_request_id(&expectation);
        let requested =
            build_requested_snapshot(&[], std::slice::from_ref(&expectation), &[], None)
                .expect("requested snapshot should build");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![
                    sample_resolved_binding(
                        &root,
                        &request_id,
                        "wit/deps/acme-binding-0.1.0",
                        vec!["acme:binding/run".to_string()],
                    ),
                    sample_resolved_binding(
                        &root,
                        &request_id,
                        "wit/deps/acme-binding-0.1.1",
                        vec!["acme:binding/run".to_string()],
                    ),
                ],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let err = resolve_binding_wits(&root, &lock, &[expectation])
            .expect_err("duplicate request_id should fail");
        assert!(err.to_string().contains("duplicate request_id"));
    }

    #[test]
    fn resolve_binding_wits_rejects_empty_interfaces() {
        let root = new_temp_dir("empty-interfaces");
        let expectation = sample_expectation();
        let request_id = compute_binding_request_id(&expectation);
        let requested =
            build_requested_snapshot(&[], std::slice::from_ref(&expectation), &[], None)
                .expect("requested snapshot should build");

        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested,
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![sample_resolved_binding(
                    &root,
                    &request_id,
                    "wit/deps/acme-binding-0.1.0",
                    vec![],
                )],
                packages: vec![],
                package_edges: vec![],
            },
        };

        let err = resolve_binding_wits(&root, &lock, &[expectation])
            .expect_err("empty interfaces should fail");
        assert!(err.to_string().contains("interfaces must not be empty"));
    }
}
