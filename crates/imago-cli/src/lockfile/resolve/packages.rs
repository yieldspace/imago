use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, anyhow};

use crate::lockfile::{
    ImagoLock, ImagoLockResolvedPackage, ImagoLockResolvedPackageEdge, LockEdgeFromKind,
    LockPackageEdgeReason, TransitivePackageRecord,
};

use super::super::{
    hash::DigestProvider,
    validation::{PathVerifier, parse_prefixed_sha256, validate_wit_source},
};

pub fn collect_resolved_packages_and_edges(
    records: impl IntoIterator<Item = TransitivePackageRecord>,
) -> anyhow::Result<(
    Vec<ImagoLockResolvedPackage>,
    Vec<ImagoLockResolvedPackageEdge>,
)> {
    let mut packages_by_ref = BTreeMap::<String, ImagoLockResolvedPackage>::new();
    let mut edges = BTreeSet::<(u8, String, String, LockPackageEdgeReason)>::new();

    for record in records {
        let package_ref = resolved_package_ref(
            &record.name,
            record.version.as_deref(),
            record.registry.as_deref(),
        );

        let package = ImagoLockResolvedPackage {
            package_ref: package_ref.clone(),
            name: record.name,
            version: record.version,
            registry: record.registry,
            requirement: record.requirement,
            source: record.source,
            path: record.path,
            digest: record.digest,
        };

        if let Some(existing) = packages_by_ref.get(&package_ref) {
            if existing != &package {
                return Err(anyhow!(
                    "transitive package '{}' has conflicting lock records; run `imago deps sync`",
                    package_ref
                ));
            }
        } else {
            packages_by_ref.insert(package_ref.clone(), package);
        }

        if let (Some(from_kind), Some(from_ref), Some(reason)) =
            (record.from_kind, record.from_ref, record.reason)
            && !from_ref.trim().is_empty()
        {
            edges.insert((edge_kind_sort_key(from_kind), from_ref, package_ref, reason));
        }
    }

    let packages = packages_by_ref.into_values().collect::<Vec<_>>();
    let package_edges = edges
        .into_iter()
        .map(
            |(kind, from_ref, to_package_ref, reason)| ImagoLockResolvedPackageEdge {
                from_kind: edge_sort_key_to_kind(kind),
                from_ref,
                to_package_ref,
                reason,
            },
        )
        .collect::<Vec<_>>();

    Ok((packages, package_edges))
}

pub fn resolved_package_ref(name: &str, version: Option<&str>, registry: Option<&str>) -> String {
    let version = version.unwrap_or("*");
    let registry = registry.unwrap_or("");
    format!("{name}@{version}#{registry}")
}

pub(super) fn verify_resolved_packages_and_edges(
    project_root: &Path,
    lock: &ImagoLock,
    digest_provider: &impl DigestProvider,
    path_verifier: &impl PathVerifier,
) -> anyhow::Result<()> {
    let dependency_request_ids = lock
        .requested
        .dependencies
        .iter()
        .map(|dependency| dependency.id.clone())
        .collect::<BTreeSet<_>>();
    let binding_request_ids = lock
        .requested
        .bindings
        .iter()
        .map(|binding| binding.id.clone())
        .collect::<BTreeSet<_>>();

    let mut package_refs = BTreeSet::new();
    for package in &lock.resolved.packages {
        if package.package_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages[].package_ref must not be empty; run `imago deps sync`"
            ));
        }
        if !package_refs.insert(package.package_ref.clone()) {
            return Err(anyhow!(
                "imago.lock.resolved.packages contains duplicate package_ref '{}'; run `imago deps sync`",
                package.package_ref
            ));
        }
        let expected_package_ref = resolved_package_ref(
            &package.name,
            package.version.as_deref(),
            package.registry.as_deref(),
        );
        if package.package_ref != expected_package_ref {
            return Err(anyhow!(
                "imago.lock.resolved.packages has non-canonical package_ref '{}' (expected '{}'); run `imago deps sync`",
                package.package_ref,
                expected_package_ref
            ));
        }
        if package.requirement.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages['{}'].requirement must not be empty; run `imago deps sync`",
                package.package_ref
            ));
        }
        if package.path.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.packages['{}'].path must not be empty; run `imago deps sync`",
                package.package_ref
            ));
        }

        if let Some(source) = package.source.as_deref() {
            validate_wit_source(
                source,
                &format!(
                    "imago.lock.resolved.packages['{}'].source",
                    package.package_ref
                ),
            )?;
            if source.contains('@') {
                return Err(anyhow!(
                    "imago.lock.resolved.packages['{}'].source must not include '@version'; run `imago deps sync`",
                    package.package_ref
                ));
            }
        }

        let expected_digest = parse_prefixed_sha256(
            &package.digest,
            &format!(
                "imago.lock.resolved.packages['{}'].digest",
                package.package_ref
            ),
        )?;
        let relative_path = path_verifier.validate_safe_wit_path(
            &package.path,
            &format!(
                "imago.lock.resolved.packages['{}'].path",
                package.package_ref
            ),
        )?;
        path_verifier.ensure_no_symlink_in_relative_path(
            project_root,
            &relative_path,
            &format!(
                "imago.lock.resolved.packages['{}'].path",
                package.package_ref
            ),
        )?;

        let package_wit_file = project_root.join(relative_path).join("package.wit");
        if !package_wit_file.is_file() {
            return Err(anyhow!(
                "transitive wit package '{}' is missing package.wit at '{}'; run `imago deps sync`",
                package.package_ref,
                package_wit_file.display()
            ));
        }
        let actual_digest = digest_provider
            .compute_sha256_hex(&package_wit_file)
            .with_context(|| {
                format!(
                    "failed to hash transitive wit package '{}' at '{}'",
                    package.package_ref,
                    package_wit_file.display()
                )
            })?;
        if !actual_digest.eq_ignore_ascii_case(expected_digest) {
            return Err(anyhow!(
                "lock digest mismatch for transitive wit package '{}'; run `imago deps sync`",
                package.package_ref
            ));
        }
    }

    let mut seen_edges = BTreeSet::<(u8, String, String, LockPackageEdgeReason)>::new();
    for edge in &lock.resolved.package_edges {
        if edge.from_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges[].from_ref must not be empty; run `imago deps sync`"
            ));
        }
        if edge.to_package_ref.trim().is_empty() {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges[].to_package_ref must not be empty; run `imago deps sync`"
            ));
        }
        if !package_refs.contains(&edge.to_package_ref) {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges points to unknown package_ref '{}'; run `imago deps sync`",
                edge.to_package_ref
            ));
        }

        let from_ok = match edge.from_kind {
            LockEdgeFromKind::Dependency => dependency_request_ids.contains(&edge.from_ref),
            LockEdgeFromKind::Binding => binding_request_ids.contains(&edge.from_ref),
            LockEdgeFromKind::Package => package_refs.contains(&edge.from_ref),
        };
        if !from_ok {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges contains unknown from_ref '{}' for kind '{}'; run `imago deps sync`",
                edge.from_ref,
                edge_from_kind_label(edge.from_kind)
            ));
        }

        let key = (
            edge_kind_sort_key(edge.from_kind),
            edge.from_ref.clone(),
            edge.to_package_ref.clone(),
            edge.reason,
        );
        if !seen_edges.insert(key) {
            return Err(anyhow!(
                "imago.lock.resolved.package_edges contains duplicate edge from '{}' to '{}'; run `imago deps sync`",
                edge.from_ref,
                edge.to_package_ref
            ));
        }
    }

    Ok(())
}

fn edge_from_kind_label(kind: LockEdgeFromKind) -> &'static str {
    match kind {
        LockEdgeFromKind::Dependency => "dependency",
        LockEdgeFromKind::Binding => "binding",
        LockEdgeFromKind::Package => "package",
    }
}

fn edge_kind_sort_key(kind: LockEdgeFromKind) -> u8 {
    match kind {
        LockEdgeFromKind::Dependency => 0,
        LockEdgeFromKind::Binding => 1,
        LockEdgeFromKind::Package => 2,
    }
}

fn edge_sort_key_to_kind(key: u8) -> LockEdgeFromKind {
    match key {
        0 => LockEdgeFromKind::Dependency,
        1 => LockEdgeFromKind::Binding,
        _ => LockEdgeFromKind::Package,
    }
}

#[cfg(test)]
mod tests {
    use crate::lockfile::{
        IMAGO_LOCK_VERSION, ImagoLock, ImagoLockRequested, ImagoLockResolved,
        ImagoLockResolvedPackage, LockEdgeFromKind, LockPackageEdgeReason, TransitivePackageRecord,
    };

    use super::{collect_resolved_packages_and_edges, verify_resolved_packages_and_edges};

    #[test]
    fn collect_resolved_packages_and_edges_normalizes_edge_ordering() {
        let records = vec![
            TransitivePackageRecord {
                name: "wasi:http".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "^0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: format!("sha256:{}", "a".repeat(64)),
                source: Some("wasi:http".to_string()),
                path: "wit/deps/wasi-http-0.2.0".to_string(),
                from_kind: Some(LockEdgeFromKind::Package),
                from_ref: Some("wasi:io@0.2.0#wasi.dev".to_string()),
                reason: Some(LockPackageEdgeReason::WitImport),
            },
            TransitivePackageRecord {
                name: "wasi:io".to_string(),
                registry: Some("wasi.dev".to_string()),
                requirement: "^0.2.0".to_string(),
                version: Some("0.2.0".to_string()),
                digest: format!("sha256:{}", "b".repeat(64)),
                source: Some("wasi:io".to_string()),
                path: "wit/deps/wasi-io-0.2.0".to_string(),
                from_kind: Some(LockEdgeFromKind::Dependency),
                from_ref: Some("dep:abc".to_string()),
                reason: Some(LockPackageEdgeReason::DeclaredRequires),
            },
        ];

        let (_packages, edges) =
            collect_resolved_packages_and_edges(records).expect("records should collect");
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].from_kind, LockEdgeFromKind::Dependency);
        assert_eq!(edges[1].from_kind, LockEdgeFromKind::Package);
    }

    #[test]
    fn verify_resolved_packages_and_edges_rejects_non_canonical_package_ref() {
        let lock = ImagoLock {
            version: IMAGO_LOCK_VERSION,
            requested: ImagoLockRequested {
                fingerprint: "fp".to_string(),
                dependencies: vec![],
                bindings: vec![],
            },
            resolved: ImagoLockResolved {
                dependencies: vec![],
                bindings: vec![],
                packages: vec![ImagoLockResolvedPackage {
                    package_ref: "non-canonical".to_string(),
                    name: "wasi:io".to_string(),
                    version: Some("0.2.0".to_string()),
                    registry: Some("wasi.dev".to_string()),
                    requirement: "^0.2.0".to_string(),
                    source: Some("wasi:io".to_string()),
                    path: "wit/deps/wasi-io-0.2.0".to_string(),
                    digest: format!("sha256:{}", "c".repeat(64)),
                }],
                package_edges: vec![],
            },
        };

        let err = verify_resolved_packages_and_edges(
            std::path::Path::new("."),
            &lock,
            &super::super::super::hash::Sha256DigestProvider,
            &super::super::super::validation::StrictPathVerifier,
        )
        .expect_err("non-canonical package_ref should fail");
        assert!(err.to_string().contains("non-canonical package_ref"));
    }
}
