use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestFilter {
    pub spec: Option<String>,
    pub binding: Option<String>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeRequest {
    pub spec: String,
    pub binding: String,
    pub profile: String,
    pub replay: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRecord {
    #[serde(alias = "spec_name")]
    pub spec: String,
    #[serde(default)]
    pub spec_slug: String,
    #[serde(default)]
    pub spec_path: String,
    pub export_module: String,
    #[serde(default)]
    pub crate_package: String,
    #[serde(default)]
    pub crate_manifest_dir: String,
    #[serde(default)]
    pub default_profiles: Vec<String>,
    #[serde(alias = "binding_path")]
    pub binding: String,
    pub profile: String,
    #[serde(default, alias = "engines")]
    pub engine: Value,
    #[serde(default)]
    pub coverage: Vec<Value>,
    #[serde(default)]
    pub replay_dir: Option<String>,
    #[serde(default)]
    pub materialize_failures: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KaniObligationRecord {
    profile: String,
    depth: usize,
    obligation_id: String,
    actions: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayBundleRecord {
    #[serde(alias = "spec_name")]
    pub spec: String,
    pub profile: String,
    pub engine: String,
    pub detail: Value,
    pub action_trace: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySummary {
    pub json_path: PathBuf,
    pub ndjson_path: PathBuf,
    pub spec: String,
    pub profile: String,
    pub engine: String,
    pub event_count: usize,
}

pub fn target_nirvash_dir() -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    target_dir.join("nirvash")
}

pub fn list_tests(base: impl AsRef<Path>, filter: &ManifestFilter) -> Result<Vec<ManifestRecord>> {
    let mut manifests = Vec::new();
    let manifest_dir = base.as_ref().join("manifest");
    if !manifest_dir.exists() {
        return Ok(manifests);
    }

    for path in read_json_files(&manifest_dir)? {
        let manifest: ManifestRecord = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?,
        )
        .with_context(|| format!("failed to decode manifest {}", path.display()))?;
        if manifest_matches(&manifest, filter) {
            manifests.push(manifest);
        }
    }

    manifests.sort_by(|lhs, rhs| {
        lhs.spec
            .cmp(&rhs.spec)
            .then(lhs.profile.cmp(&rhs.profile))
            .then(lhs.binding.cmp(&rhs.binding))
    });
    Ok(manifests)
}

pub fn materialize_tests(
    base: impl AsRef<Path>,
    request: &MaterializeRequest,
) -> Result<Vec<PathBuf>> {
    let base = base.as_ref();
    let manifests = list_tests(
        base,
        &ManifestFilter {
            spec: Some(request.spec.clone()),
            binding: Some(request.binding.clone()),
            profile: Some(request.profile.clone()),
        },
    )?;
    if manifests.is_empty() {
        bail!(
            "no manifest matched spec={} binding={} profile={}",
            request.spec,
            request.binding,
            request.profile
        );
    }

    let replay_override = request
        .replay
        .as_deref()
        .map(canonical_bundle_json_path)
        .transpose()?;
    let mut paths = Vec::new();
    for manifest in manifests {
        let materialized_dir = materialized_output_dir(base, &manifest);
        fs::create_dir_all(&materialized_dir)
            .with_context(|| format!("failed to create {}", materialized_dir.display()))?;
        let replay_path = if let Some(path) = replay_override.clone() {
            path
        } else {
            latest_matching_replay_bundle(base, &manifest)?
        };
        let output_path = materialized_dir.join(format!(
            "{}_{}_replay.rs",
            sanitize_path(manifest_spec_stem(&manifest)),
            sanitize_path(&manifest.profile),
        ));
        fs::write(
            &output_path,
            render_materialized_test(&manifest, &replay_path),
        )
        .with_context(|| format!("failed to write {}", output_path.display()))?;
        paths.push(output_path);

        if manifest_has_kani(&manifest.engine) {
            let obligations = collect_kani_obligations_for_manifest(base, &manifest)?;
            if !obligations.is_empty() {
                let kani_output = materialized_dir.join(format!(
                    "{}_{}_kani.rs",
                    sanitize_path(manifest_spec_stem(&manifest)),
                    sanitize_path(&manifest.profile),
                ));
                fs::write(
                    &kani_output,
                    render_materialized_kani(&manifest, &obligations),
                )
                .with_context(|| format!("failed to write {}", kani_output.display()))?;
                paths.push(kani_output);
            }
        }
    }

    Ok(paths)
}

pub fn materialize_replay(
    base: impl AsRef<Path>,
    replay_path: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let base = base.as_ref();
    let replay_path = canonical_bundle_json_path(replay_path.as_ref())?;
    let bundle = read_replay_record(&replay_path)?;
    let manifests = list_tests(
        base,
        &ManifestFilter {
            spec: Some(bundle.spec.clone()),
            binding: None,
            profile: Some(bundle.profile.clone()),
        },
    )?;
    if manifests.is_empty() {
        bail!(
            "no manifest matched replay bundle spec={} profile={}",
            bundle.spec,
            bundle.profile
        );
    }
    if manifests.len() != 1 {
        bail!(
            "replay bundle matched multiple bindings for spec={} profile={}; rerun materialize-tests with --binding",
            bundle.spec,
            bundle.profile
        );
    }
    let manifest = manifests
        .into_iter()
        .next()
        .expect("manifest presence already checked");
    materialize_tests(
        base,
        &MaterializeRequest {
            spec: manifest.spec,
            binding: manifest.binding,
            profile: manifest.profile,
            replay: Some(replay_path),
        },
    )
}

pub fn replay(base: impl AsRef<Path>) -> Result<Vec<ReplaySummary>> {
    let replay_dir = base.as_ref().join("replay");
    if !replay_dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    for json_path in read_json_files(&replay_dir)? {
        let file_name = json_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !file_name.ends_with("_bundle.json") {
            continue;
        }

        let bundle = read_replay_record(&json_path)?;
        let ndjson_path = json_path.with_file_name(file_name.replace(".json", ".ndjson"));
        let event_count = validate_ndjson(&ndjson_path)?;
        summaries.push(ReplaySummary {
            json_path,
            ndjson_path,
            spec: bundle.spec,
            profile: bundle.profile,
            engine: bundle.engine,
            event_count,
        });
    }

    summaries.sort_by(|lhs, rhs| {
        lhs.spec
            .cmp(&rhs.spec)
            .then(lhs.profile.cmp(&rhs.profile))
            .then(lhs.engine.cmp(&rhs.engine))
    });
    Ok(summaries)
}

fn manifest_matches(manifest: &ManifestRecord, filter: &ManifestFilter) -> bool {
    filter
        .spec
        .as_ref()
        .is_none_or(|spec| manifest.spec == *spec)
        && filter.binding.as_ref().is_none_or(|binding| {
            manifest.binding == *binding
                || manifest
                    .binding
                    .rsplit("::")
                    .next()
                    .is_some_and(|tail| tail == binding)
        })
        && filter
            .profile
            .as_ref()
            .is_none_or(|profile| manifest.profile == *profile)
}

fn latest_matching_replay_bundle(base: &Path, manifest: &ManifestRecord) -> Result<PathBuf> {
    let replay_dir = base.join("replay");
    if !replay_dir.exists() {
        bail!(
            "no replay bundle found for spec={} profile={}",
            manifest.spec,
            manifest.profile
        );
    }

    let mut matches = Vec::new();
    for path in read_json_files(&replay_dir)? {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !file_name.ends_with("_bundle.json") {
            continue;
        }
        let bundle = read_replay_record(&path)?;
        if bundle.spec == manifest.spec && bundle.profile == manifest.profile {
            matches.push(path);
        }
    }
    matches.sort();
    matches.pop().with_context(|| {
        format!(
            "no replay bundle found for spec={} profile={}",
            manifest.spec, manifest.profile
        )
    })
}

fn read_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to enumerate {}", dir.display()))?;
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"));
    paths.sort();
    Ok(paths)
}

fn read_replay_record(path: &Path) -> Result<ReplayBundleRecord> {
    serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("failed to decode replay bundle {}", path.display()))
}

fn canonical_bundle_json_path(path: &Path) -> Result<PathBuf> {
    if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
        return Ok(path.to_path_buf());
    }
    if path.extension().and_then(|ext| ext.to_str()) == Some("ndjson") {
        let json_path = path.with_extension("json");
        if json_path.exists() {
            return Ok(json_path);
        }
    }
    bail!(
        "replay path must point to a replay bundle .json or sibling .ndjson: {}",
        path.display()
    )
}

fn workspace_root_from_base(base: &Path) -> PathBuf {
    let components = base
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect::<Vec<_>>();
    if let Some(target_index) = components
        .iter()
        .rposition(|component| component == "target")
    {
        if target_index > 0 {
            let mut root = PathBuf::new();
            for component in &components[..target_index] {
                root.push(component);
            }
            if !root.as_os_str().is_empty() {
                return root;
            }
        }
    }

    base.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| base.to_path_buf())
}

fn materialized_output_dir(base: &Path, manifest: &ManifestRecord) -> PathBuf {
    if !manifest.crate_manifest_dir.is_empty() {
        return PathBuf::from(&manifest.crate_manifest_dir)
            .join("tests")
            .join("generated");
    }
    workspace_root_from_base(base)
        .join("tests")
        .join("generated")
}

fn render_materialized_test(manifest: &ManifestRecord, replay_path: &Path) -> String {
    let crate_ident = crate_root_ident(&manifest.crate_package);
    let export_module = normalize_subject_path(&manifest.export_module, &crate_ident);
    let binding = normalize_subject_path(&manifest.binding, &crate_ident);
    let test_name = format!(
        "replay_{}_{}",
        sanitize_ident(manifest_spec_stem(manifest)),
        sanitize_ident(&manifest.profile),
    );
    format!(
        "// Generated by `cargo nirvash materialize-tests`\n\
         // spec: {spec}\n\
         // binding: {binding}\n\
         // replay: {replay}\n\n\
         #[test]\n\
         fn {test_name}() {{\n\
             {export_module}::replay::run::<{binding}>(r#\"{replay}\"#)\n\
                 .expect(\"materialized replay should pass\");\n\
         }}\n",
        spec = manifest.spec,
        binding = binding,
        export_module = export_module,
        replay = replay_path.display(),
        test_name = test_name,
    )
}

fn manifest_has_kani(engine: &Value) -> bool {
    !extract_kani_depths(engine).is_empty()
}

fn extract_kani_depths(engine: &Value) -> Vec<usize> {
    let mut depths = Vec::new();
    if let Some(values) = engine.as_array() {
        for value in values {
            let depth = value
                .get("k_bounded")
                .and_then(|value| value.get("depth"))
                .and_then(Value::as_u64)
                .or_else(|| value.get("depth").and_then(Value::as_u64));
            if let Some(depth) = depth {
                let depth = depth as usize;
                if !depths.contains(&depth) {
                    depths.push(depth);
                }
            }
        }
    }
    depths
}

fn collect_kani_obligations_for_manifest(
    base: &Path,
    manifest: &ManifestRecord,
) -> Result<Vec<KaniObligationRecord>> {
    let helper_dir = helper_dir_for_manifest(base, manifest);
    let src_dir = helper_dir.join("src");
    fs::create_dir_all(&src_dir)
        .with_context(|| format!("failed to create {}", src_dir.display()))?;
    let workspace_root = workspace_root_from_base(base);
    fs::write(
        helper_dir.join("Cargo.toml"),
        render_helper_cargo_toml(manifest, &workspace_root),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            helper_dir.join("Cargo.toml").display()
        )
    })?;
    fs::write(src_dir.join("main.rs"), render_helper_main_rs(manifest))
        .with_context(|| format!("failed to write {}", src_dir.join("main.rs").display()))?;

    let output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(helper_dir.join("Cargo.toml"))
        .current_dir(&workspace_root)
        .env(
            "CARGO_TARGET_DIR",
            workspace_root
                .join("target")
                .join("nirvash")
                .join("materialize-helper-target"),
        )
        .output()
        .with_context(|| "failed to run kani materialize helper".to_owned())?;
    if !output.status.success() {
        bail!(
            "kani materialize helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut obligations = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        obligations.push(
            serde_json::from_str::<KaniObligationRecord>(line).with_context(|| {
                format!("failed to decode helper kani obligation line `{line}`")
            })?,
        );
    }
    Ok(obligations)
}

fn helper_dir_for_manifest(base: &Path, manifest: &ManifestRecord) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    manifest.spec.hash(&mut hasher);
    manifest.spec_slug.hash(&mut hasher);
    manifest.binding.hash(&mut hasher);
    manifest.profile.hash(&mut hasher);
    manifest.export_module.hash(&mut hasher);
    manifest.spec_path.hash(&mut hasher);
    manifest.crate_package.hash(&mut hasher);
    manifest.crate_manifest_dir.hash(&mut hasher);
    let hash = format!("{:016x}", hasher.finish());
    base.join("materialize-helper").join(hash)
}

fn crate_root_ident(package: &str) -> String {
    if package.is_empty() {
        "crate".to_owned()
    } else {
        package.replace('-', "_")
    }
}

fn manifest_spec_stem(manifest: &ManifestRecord) -> &str {
    if manifest.spec_slug.is_empty() {
        &manifest.spec
    } else {
        &manifest.spec_slug
    }
}

fn normalize_subject_path(path: &str, root: &str) -> String {
    let mut segments = path.split("::");
    let Some(head) = segments.next() else {
        return root.to_owned();
    };
    let tail = segments.collect::<Vec<_>>();
    if head == "crate" {
        if tail.is_empty() {
            root.to_owned()
        } else {
            format!("{root}::{}", tail.join("::"))
        }
    } else if head == root {
        path.to_owned()
    } else if tail.is_empty() {
        path.to_owned()
    } else {
        path.to_owned()
    }
}

fn normalize_external_module_path(path: &str) -> String {
    let mut segments = path.split("::");
    let _ = segments.next();
    let tail = segments.collect::<Vec<_>>();
    if tail.is_empty() {
        "subject_crate".to_owned()
    } else {
        format!("subject_crate::{}", tail.join("::"))
    }
}

fn normalize_local_module_path(path: &str) -> String {
    let mut segments = path.split("::");
    let _ = segments.next();
    let tail = segments.collect::<Vec<_>>();
    if tail.is_empty() {
        "crate".to_owned()
    } else {
        format!("crate::{}", tail.join("::"))
    }
}

fn render_helper_cargo_toml(manifest: &ManifestRecord, workspace_root: &Path) -> String {
    format!(
        "[package]\nname = \"nirvash_materialize_helper\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nsubject_crate = {{ package = {crate_package:?}, path = {crate_manifest_dir:?} }}\nnirvash-conformance = {{ path = {conformance_path:?} }}\nserde_json = \"1\"\n",
        crate_package = manifest.crate_package,
        crate_manifest_dir = manifest.crate_manifest_dir,
        conformance_path = workspace_root
            .join("crates")
            .join("nirvash-conformance")
            .display()
            .to_string(),
    )
}

fn render_helper_main_rs(manifest: &ManifestRecord) -> String {
    let export_module = normalize_external_module_path(&manifest.export_module);
    format!(
        "fn main() {{\n    let profile = {export_module}::plans::profile_for_label({profile:?})\n        .expect(\"kani profile should exist\");\n    let depth = profile\n        .engines\n        .iter()\n        .find_map(|engine| match engine {{\n            nirvash_conformance::EnginePlan::KaniBounded {{ depth }} => Some(*depth),\n            _ => None,\n        }})\n        .expect(\"kani profile should include EnginePlan::KaniBounded\");\n    let obligations = {export_module}::plans::collect_kani_obligations_for({profile:?})\n        .expect(\"kani obligations should collect\");\n    for obligation in obligations {{\n        println!(\"{{}}\", serde_json::json!({{\n            \"profile\": {profile:?},\n            \"depth\": depth,\n            \"obligation_id\": obligation.id,\n            \"actions\": obligation.actions,\n        }}));\n    }}\n}}\n",
        export_module = export_module,
        profile = manifest.profile,
    )
}

fn render_materialized_kani(
    manifest: &ManifestRecord,
    obligations: &[KaniObligationRecord],
) -> String {
    let local_export_module = normalize_local_module_path(&manifest.export_module);
    let action_ty = format!("{local_export_module}::bindings::Action");
    let proofs = obligations
        .iter()
        .enumerate()
        .map(|(index, obligation)| {
            let fn_name = format!(
                "{}_{}_{}",
                sanitize_ident(&manifest.profile),
                index,
                sanitize_ident(&obligation.obligation_id),
            );
            let encoded_actions =
                serde_json::to_string(&obligation.actions).expect("encode materialized actions");
            format!(
                "#[cfg(kani)]\n#[kani::proof]\nfn {fn_name}() {{\n    let actions: ::std::vec::Vec<{action_ty}> = ::serde_json::from_str(r#\"{encoded_actions}\"#)\n        .expect(\"materialized kani actions should decode\");\n    {local_export_module}::install::__run_materialized_kani::<__NirvashBinding>(\n        __NIRVASH_BINDING_NAME,\n        {profile:?},\n        {obligation_id:?},\n        &actions,\n    )\n    .expect(\"materialized kani proof should pass\");\n}}\n",
                fn_name = fn_name,
                action_ty = action_ty,
                encoded_actions = encoded_actions,
                local_export_module = local_export_module,
                profile = obligation.profile,
                obligation_id = obligation.obligation_id,
            )
        })
        .collect::<String>();
    format!(
        "// Generated by `cargo nirvash materialize-tests`\n// spec: {spec}\n// binding: {binding}\n// profile: {profile}\n\n{proofs}",
        spec = manifest.spec,
        binding = manifest.binding,
        profile = manifest.profile,
        proofs = proofs,
    )
}

fn validate_ndjson(path: &Path) -> Result<usize> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut count = 0;
    for (index, line) in content.lines().enumerate() {
        serde_json::from_str::<Value>(line).with_context(|| {
            format!(
                "failed to decode NDJSON line {} in {}",
                index + 1,
                path.display()
            )
        })?;
        count += 1;
    }
    Ok(count)
}

fn sanitize_path(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch,
            _ => '_',
        })
        .collect()
}

fn sanitize_ident(raw: &str) -> String {
    let mut chars = raw.chars();
    let first = chars.next().unwrap_or('_');
    let mut ident = String::new();
    ident.push(match first {
        'a'..='z' | 'A'..='Z' | '_' => first,
        '0'..='9' => '_',
        _ => '_',
    });
    ident.extend(chars.map(|ch| match ch {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => ch,
        _ => '_',
    }));
    ident
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manifest_record(crate_manifest_dir: &Path) -> ManifestRecord {
        ManifestRecord {
            spec: "demo.spec".to_owned(),
            spec_slug: String::new(),
            spec_path: "demo_crate::DemoSpec".to_owned(),
            export_module: "crate::demo::generated".to_owned(),
            crate_package: "demo-crate".to_owned(),
            crate_manifest_dir: crate_manifest_dir.display().to_string(),
            default_profiles: vec!["small".to_owned()],
            binding: "crate::demo::DemoBinding".to_owned(),
            profile: "small".to_owned(),
            engine: serde_json::json!(["explicit_suite"]),
            coverage: vec![serde_json::json!("transitions")],
            replay_dir: Some("target/nirvash/replay".to_owned()),
            materialize_failures: true,
        }
    }

    fn write_replay_files(base: &Path) -> (PathBuf, PathBuf) {
        let replay_dir = base.join("replay");
        fs::create_dir_all(&replay_dir).expect("replay dir");
        let json_path = replay_dir.join("demo_spec_small_explicit_bundle.json");
        let ndjson_path = replay_dir.join("demo_spec_small_explicit_bundle.ndjson");
        fs::write(
            &json_path,
            serde_json::to_vec_pretty(&ReplayBundleRecord {
                spec: "demo.spec".to_owned(),
                profile: "small".to_owned(),
                engine: "explicit".to_owned(),
                detail: serde_json::json!({ "events": [] }),
                action_trace: serde_json::json!({ "steps": [] }),
            })
            .expect("encode replay"),
        )
        .expect("write replay");
        fs::write(
            &ndjson_path,
            [
                serde_json::json!({ "kind": "initial", "state": "idle" }).to_string(),
                serde_json::json!({ "kind": "stutter" }).to_string(),
            ]
            .join("\n"),
        )
        .expect("write ndjson");
        (json_path, ndjson_path)
    }

    #[test]
    fn list_tests_filters_by_spec_binding_and_profile() {
        let temp = tempdir().expect("tempdir");
        let base = temp.path().join("nirvash");
        let manifest_dir = base.join("manifest");
        fs::create_dir_all(&manifest_dir).expect("manifest dir");
        let crate_manifest_dir = temp.path().join("demo-crate");
        fs::create_dir_all(&crate_manifest_dir).expect("crate manifest dir");
        let manifest_path = manifest_dir.join("demo_spec__demo_binding__small.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest_record(&crate_manifest_dir))
                .expect("encode manifest"),
        )
        .expect("write manifest");

        let manifests = list_tests(
            &base,
            &ManifestFilter {
                spec: Some("demo.spec".to_owned()),
                binding: Some("DemoBinding".to_owned()),
                profile: Some("small".to_owned()),
            },
        )
        .expect("list manifests");

        assert_eq!(manifests, vec![manifest_record(&crate_manifest_dir)]);
    }

    #[test]
    fn materialize_tests_writes_rust_replay_file() {
        let temp = tempdir().expect("tempdir");
        let base = temp.path().join("nirvash");
        let manifest_dir = base.join("manifest");
        fs::create_dir_all(&manifest_dir).expect("manifest dir");
        let crate_manifest_dir = temp.path().join("demo-crate");
        fs::create_dir_all(&crate_manifest_dir).expect("crate manifest dir");
        let manifest_path = manifest_dir.join("demo_spec__demo_binding__small.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest_record(&crate_manifest_dir))
                .expect("encode manifest"),
        )
        .expect("write manifest");
        let (json_path, _) = write_replay_files(&base);

        let materialized = materialize_tests(
            &base,
            &MaterializeRequest {
                spec: "demo.spec".to_owned(),
                binding: "DemoBinding".to_owned(),
                profile: "small".to_owned(),
                replay: Some(json_path.clone()),
            },
        )
        .expect("materialize replay");

        assert_eq!(materialized.len(), 1);
        assert_eq!(
            materialized[0],
            crate_manifest_dir.join("tests/generated/demo_spec_small_replay.rs")
        );
        let body = fs::read_to_string(&materialized[0]).expect("read materialized file");
        assert!(
            body.contains(
                "demo_crate::demo::generated::replay::run::<demo_crate::demo::DemoBinding>"
            )
        );
        assert!(body.contains(&json_path.display().to_string()));
    }

    #[test]
    fn materialize_tests_prefers_spec_slug_for_output_paths() {
        let temp = tempdir().expect("tempdir");
        let base = temp.path().join("nirvash");
        let manifest_dir = base.join("manifest");
        fs::create_dir_all(&manifest_dir).expect("manifest dir");
        let crate_manifest_dir = temp.path().join("demo-crate");
        fs::create_dir_all(&crate_manifest_dir).expect("crate manifest dir");
        let mut manifest = manifest_record(&crate_manifest_dir);
        manifest.spec_slug = "tests_fixtures_demo_spec_line42_col1_Spec".to_owned();
        let manifest_path = manifest_dir.join("demo_spec__demo_binding__small.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
        )
        .expect("write manifest");
        let (json_path, _) = write_replay_files(&base);

        let materialized = materialize_tests(
            &base,
            &MaterializeRequest {
                spec: "demo.spec".to_owned(),
                binding: "DemoBinding".to_owned(),
                profile: "small".to_owned(),
                replay: Some(json_path),
            },
        )
        .expect("materialize replay");

        assert_eq!(
            materialized[0],
            crate_manifest_dir
                .join("tests/generated/tests_fixtures_demo_spec_line42_col1_Spec_small_replay.rs")
        );
    }

    #[test]
    fn materialize_replay_uses_ndjson_path_as_sugar() {
        let temp = tempdir().expect("tempdir");
        let base = temp.path().join("nirvash");
        let manifest_dir = base.join("manifest");
        fs::create_dir_all(&manifest_dir).expect("manifest dir");
        let crate_manifest_dir = temp.path().join("demo-crate");
        fs::create_dir_all(&crate_manifest_dir).expect("crate manifest dir");
        let manifest_path = manifest_dir.join("demo_spec__demo_binding__small.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest_record(&crate_manifest_dir))
                .expect("encode manifest"),
        )
        .expect("write manifest");
        let (_, ndjson_path) = write_replay_files(&base);

        let materialized = materialize_replay(&base, &ndjson_path).expect("materialize replay");

        assert_eq!(materialized.len(), 1);
        assert_eq!(
            materialized[0],
            crate_manifest_dir.join("tests/generated/demo_spec_small_replay.rs")
        );
    }

    #[test]
    fn kani_detection_and_rendering_use_obligation_ids() {
        let temp = tempdir().expect("tempdir");
        let crate_manifest_dir = temp.path().join("demo-crate");
        fs::create_dir_all(&crate_manifest_dir).expect("crate manifest dir");
        let mut manifest = manifest_record(&crate_manifest_dir);
        manifest.engine = serde_json::json!([
            { "k_bounded": { "depth": 4 } }
        ]);

        assert!(manifest_has_kani(&manifest.engine));

        let body = render_materialized_kani(
            &manifest,
            &[KaniObligationRecord {
                profile: "small".to_owned(),
                depth: 4,
                obligation_id: "explicit-cover-0".to_owned(),
                actions: serde_json::json!(["tick"]),
            }],
        );

        assert!(body.contains("#[kani::proof]"));
        assert!(body.contains("__run_materialized_kani::<__NirvashBinding>"));
        assert!(body.contains("explicit-cover-0"));
        assert!(body.contains("serde_json::from_str"));
    }

    #[test]
    fn replay_reads_json_and_ndjson_pairs() {
        let temp = tempdir().expect("tempdir");
        let base = temp.path().join("nirvash");
        let (_, ndjson_path) = write_replay_files(&base);

        let summaries = replay(&base).expect("replay summary");

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].spec, "demo.spec");
        assert_eq!(summaries[0].event_count, 2);
        assert_eq!(summaries[0].ndjson_path, ndjson_path);
    }
}
