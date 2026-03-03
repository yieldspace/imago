use std::collections::{BTreeMap, BTreeSet};

use anyhow::anyhow;

use crate::lockfile::{
    BindingWitExpectation, DependencyExpectation, ImagoLockRequested, ImagoLockRequestedBinding,
    ImagoLockRequestedDependency,
};

use super::{
    dependency_kind_label, normalize_capability_policy, normalize_string_set, sha256_hex,
    source_kind_label,
};

pub fn compute_dependency_request_id(expectation: &DependencyExpectation) -> String {
    let mut lines = vec![
        "kind=".to_string() + dependency_kind_label(expectation.kind),
        "version=".to_string() + expectation.version.as_str(),
        "source_kind=".to_string() + source_kind_label(expectation.source_kind),
        "source=".to_string() + expectation.source.as_str(),
        "registry=".to_string() + expectation.registry.as_deref().unwrap_or(""),
        "sha256=".to_string() + expectation.sha256.as_deref().unwrap_or(""),
    ];
    if let Some(component) = expectation.component.as_ref() {
        lines.push("component_kind=".to_string() + source_kind_label(component.source_kind));
        lines.push("component_source=".to_string() + component.source.as_str());
        lines.push("component_registry=".to_string() + component.registry.as_deref().unwrap_or(""));
        lines.push("component_sha256=".to_string() + component.sha256.as_deref().unwrap_or(""));
    }
    for requires in normalize_string_set(expectation.requires.iter().cloned()) {
        lines.push("declared_requires=".to_string() + requires.as_str());
    }
    let capabilities = normalize_capability_policy(&expectation.capabilities);
    lines.push(format!("cap.privileged={}", capabilities.privileged));
    for (key, values) in capabilities.deps {
        lines.push(format!("cap.deps:{key}={}", values.join(",")));
    }
    for (key, values) in capabilities.wasi {
        lines.push(format!("cap.wasi:{key}={}", values.join(",")));
    }

    let joined = lines.join("\n");
    format!("dep:{}", sha256_hex(joined.as_bytes()))
}

pub fn compute_binding_request_id(expectation: &BindingWitExpectation) -> String {
    let lines = [
        "name=".to_string() + expectation.name.as_str(),
        "version=".to_string() + expectation.version.as_str(),
        "source_kind=".to_string() + source_kind_label(expectation.source_kind),
        "source=".to_string() + expectation.source.as_str(),
        "registry=".to_string() + expectation.registry.as_deref().unwrap_or(""),
        "sha256=".to_string() + expectation.sha256.as_deref().unwrap_or(""),
    ];
    let joined = lines.join("\n");
    format!("bind:{}", sha256_hex(joined.as_bytes()))
}

pub fn build_requested_snapshot(
    dependency_expectations: &[DependencyExpectation],
    binding_expectations: &[BindingWitExpectation],
    namespace_registries: Option<&BTreeMap<String, String>>,
) -> anyhow::Result<ImagoLockRequested> {
    let mut dependencies = dependency_expectations
        .iter()
        .map(|expectation| ImagoLockRequestedDependency {
            id: compute_dependency_request_id(expectation),
            kind: expectation.kind,
            version: expectation.version.clone(),
            source_kind: expectation.source_kind,
            source: expectation.source.clone(),
            registry: expectation.registry.clone(),
            sha256: expectation.sha256.clone(),
            declared_requires: normalize_string_set(expectation.requires.iter().cloned()),
            component_source_kind: expectation
                .component
                .as_ref()
                .map(|component| component.source_kind),
            component_source: expectation
                .component
                .as_ref()
                .map(|component| component.source.clone()),
            component_registry: expectation
                .component
                .as_ref()
                .and_then(|component| component.registry.clone()),
            component_sha256: expectation
                .component
                .as_ref()
                .and_then(|component| component.sha256.clone()),
            capabilities: normalize_capability_policy(&expectation.capabilities),
        })
        .collect::<Vec<_>>();
    let mut seen_dependency_ids = BTreeSet::new();
    for dependency in &dependencies {
        if !seen_dependency_ids.insert(dependency.id.clone()) {
            return Err(anyhow!(
                "duplicate dependency request id '{}' in requested snapshot; remove duplicated dependency requests",
                dependency.id
            ));
        }
    }
    dependencies.sort_by(|a, b| a.id.cmp(&b.id));

    let mut bindings = binding_expectations
        .iter()
        .map(|expectation| ImagoLockRequestedBinding {
            id: compute_binding_request_id(expectation),
            name: expectation.name.clone(),
            version: expectation.version.clone(),
            source_kind: expectation.source_kind,
            source: expectation.source.clone(),
            registry: expectation.registry.clone(),
            sha256: expectation.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let mut seen_binding_ids = BTreeSet::new();
    for binding in &bindings {
        if !seen_binding_ids.insert(binding.id.clone()) {
            return Err(anyhow!(
                "duplicate binding request id '{}' in requested snapshot; remove duplicated binding requests",
                binding.id
            ));
        }
    }
    bindings.sort_by(|a, b| a.id.cmp(&b.id));

    let fingerprint = compute_requested_fingerprint(&dependencies, &bindings, namespace_registries);
    Ok(ImagoLockRequested {
        fingerprint,
        dependencies,
        bindings,
    })
}

pub fn compute_requested_fingerprint(
    dependencies: &[ImagoLockRequestedDependency],
    bindings: &[ImagoLockRequestedBinding],
    namespace_registries: Option<&BTreeMap<String, String>>,
) -> String {
    let mut lines = vec!["imago-lock-requested:v1".to_string()];

    if let Some(namespace_registries) = namespace_registries {
        for (namespace, registry) in namespace_registries {
            lines.push(format!("ns:{namespace}={registry}"));
        }
    }

    for dependency in dependencies {
        lines.push(format!("dep.id={}", dependency.id));
        lines.push(format!(
            "dep.kind={}",
            dependency_kind_label(dependency.kind)
        ));
        lines.push(format!("dep.version={}", dependency.version));
        lines.push(format!(
            "dep.source_kind={}",
            source_kind_label(dependency.source_kind)
        ));
        lines.push(format!("dep.source={}", dependency.source));
        lines.push(format!(
            "dep.registry={}",
            dependency.registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.sha256={}",
            dependency.sha256.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_kind={}",
            dependency
                .component_source_kind
                .map(source_kind_label)
                .unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_source={}",
            dependency.component_source.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_registry={}",
            dependency.component_registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "dep.component_sha256={}",
            dependency.component_sha256.as_deref().unwrap_or("")
        ));

        for requires in &dependency.declared_requires {
            lines.push(format!("dep.requires={requires}"));
        }

        lines.push(format!(
            "dep.cap.privileged={}",
            dependency.capabilities.privileged
        ));
        for (key, values) in &dependency.capabilities.deps {
            lines.push(format!("dep.cap.deps:{key}={}", values.join(",")));
        }
        for (key, values) in &dependency.capabilities.wasi {
            lines.push(format!("dep.cap.wasi:{key}={}", values.join(",")));
        }
    }

    for binding in bindings {
        lines.push(format!("bind.id={}", binding.id));
        lines.push(format!("bind.name={}", binding.name));
        lines.push(format!("bind.version={}", binding.version));
        lines.push(format!(
            "bind.source_kind={}",
            source_kind_label(binding.source_kind)
        ));
        lines.push(format!("bind.source={}", binding.source));
        lines.push(format!(
            "bind.registry={}",
            binding.registry.as_deref().unwrap_or("")
        ));
        lines.push(format!(
            "bind.sha256={}",
            binding.sha256.as_deref().unwrap_or("")
        ));
    }

    sha256_hex(lines.join("\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::lockfile::{
        DependencyExpectation, LockCapabilityPolicy, LockDependencyKind, LockSourceKind,
    };

    use super::{build_requested_snapshot, compute_dependency_request_id};

    fn sample_dependency(name: &str) -> DependencyExpectation {
        DependencyExpectation {
            name: name.to_string(),
            kind: LockDependencyKind::Native,
            version: "0.1.0".to_string(),
            source_kind: LockSourceKind::Wit,
            source: "acme:example".to_string(),
            registry: Some("wa.dev".to_string()),
            sha256: None,
            requires: vec![],
            capabilities: LockCapabilityPolicy::default(),
            component: None,
        }
    }

    #[test]
    fn dependency_request_id_is_stable_for_reordered_requires() {
        let mut a = sample_dependency("path-source-0");
        a.requires = vec![
            "wasi:io".to_string(),
            "wasi:http".to_string(),
            "wasi:io".to_string(),
        ];
        let mut b = sample_dependency("path-source-0");
        b.requires = vec![
            "wasi:http".to_string(),
            "wasi:io".to_string(),
            "wasi:http".to_string(),
        ];

        assert_eq!(
            compute_dependency_request_id(&a),
            compute_dependency_request_id(&b)
        );
    }

    #[test]
    fn build_requested_snapshot_rejects_duplicate_dependency_request_ids() {
        let dep_a = sample_dependency("dep-a");
        let dep_b = sample_dependency("dep-b");
        let err = build_requested_snapshot(&[dep_a, dep_b], &[], None)
            .expect_err("duplicated request IDs should fail");
        assert!(err.to_string().contains("duplicate dependency request id"));
    }

    #[test]
    fn requested_fingerprint_is_deterministic_for_dependency_order() {
        let mut ns = BTreeMap::new();
        ns.insert("wasi".to_string(), "wasi.dev".to_string());
        let dep_a = sample_dependency("dep-a");
        let mut dep_b = sample_dependency("dep-b");
        dep_b.source = "acme:other".to_string();

        let left = build_requested_snapshot(&[dep_a.clone(), dep_b.clone()], &[], Some(&ns))
            .expect("snapshot should build");
        let right = build_requested_snapshot(&[dep_b, dep_a], &[], Some(&ns))
            .expect("snapshot should build");
        assert_eq!(left.fingerprint, right.fingerprint);
        assert_eq!(left.dependencies, right.dependencies);
    }
}
