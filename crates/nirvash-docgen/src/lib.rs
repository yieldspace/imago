use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use quote::ToTokens;
use serde::Deserialize;
use syn::{
    Attribute, ImplItem, Item, ItemFn, ItemImpl, ItemMacro, ItemMod, Path as SynPath,
    PathArguments, Token, Type,
};

type DynError = Box<dyn Error>;

const MERMAID_RUNTIME_SOURCE: &str = include_str!("../assets/mermaid/mermaid.min.js");

/// Generate rustdoc fragments for `nirvash` specs in the current crate.
pub fn generate() -> Result<(), Box<dyn Error>> {
    if env::var_os("NIRVASH_DOCGEN_SKIP").is_some() || env::var_os("RUSTDOC").is_none() {
        return Ok(());
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let output = generate_at(&manifest_dir, &out_dir)?;
    for path in &output.rerun_if_changed {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets/mermaid/mermaid.min.js")
            .display()
    );
    for fragment in &output.fragments {
        println!(
            "cargo:rustc-env={}={}",
            fragment.env_key,
            fragment.path.display()
        );
    }
    Ok(())
}

#[derive(Debug)]
struct MessageError(String);

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for MessageError {}

fn err(message: impl Into<String>) -> DynError {
    Box::new(MessageError(message.into()))
}

fn mermaid_render_script() -> String {
    let runtime_source =
        serde_json::to_string(MERMAID_RUNTIME_SOURCE).expect("mermaid runtime escapes");
    format!(
        r#"<script>
(() => {{
  const registry = globalThis.__nirvashMermaidRegistry ??= {{
    initialized: false,
    nextId: 0,
  }};

  if (!globalThis.mermaid) {{
    const runtime = document.createElement('script');
    runtime.textContent = {runtime_source};
    document.head.appendChild(runtime);
  }}

  const currentTheme = () => {{
    const rustdocTheme = globalThis.localStorage?.getItem('rustdoc-theme');
    return rustdocTheme === 'dark' || rustdocTheme === 'ayu' ? 'dark' : 'default';
  }};

  const renderBlocks = async () => {{
    const mermaid = globalThis.mermaid;
    if (!mermaid) {{
      console.error('nirvash mermaid runtime failed to initialize');
      return;
    }}

    if (!registry.initialized) {{
      mermaid.initialize({{
        startOnLoad: false,
        securityLevel: 'loose',
        theme: currentTheme(),
      }});
      registry.initialized = true;
    }}

    const blocks = [...document.querySelectorAll('pre.nirvash-mermaid:not([data-nirvash-rendered="true"])')];
    for (const block of blocks) {{
      block.dataset.nirvashRendered = 'true';
      const source = block.textContent ?? '';
      const id = `nirvash-mermaid-${{registry.nextId++}}`;
      try {{
        const {{ svg }} = await mermaid.render(id, source);
        const container = document.createElement('div');
        container.className = 'nirvash-mermaid-diagram';
        container.innerHTML = svg;
        block.replaceWith(container);
      }} catch (error) {{
        console.error('nirvash mermaid render failed', error);
      }}
    }}
  }};

  void renderBlocks();
}})();
</script>"#
    )
}

#[derive(Debug)]
struct GenerationOutput {
    fragments: Vec<GeneratedFragment>,
    rerun_if_changed: Vec<PathBuf>,
}

#[derive(Debug)]
struct GeneratedFragment {
    env_key: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecKind {
    Subsystem,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RegistrationKind {
    Invariant,
    Property,
    Fairness,
    StateConstraint,
    ActionConstraint,
    Symmetry,
}

impl RegistrationKind {
    fn attr_name(self) -> &'static str {
        match self {
            Self::Invariant => "invariant",
            Self::Property => "property",
            Self::Fairness => "fairness",
            Self::StateConstraint => "state_constraint",
            Self::ActionConstraint => "action_constraint",
            Self::Symmetry => "symmetry",
        }
    }
}

#[derive(Debug, Default, Clone)]
struct SpecDoc {
    kind: Option<SpecKind>,
    full_path: Vec<String>,
    tail_ident: String,
    state_ty: String,
    action_ty: String,
    model_cases: Option<String>,
    subsystems: Vec<nirvash::SpecVizSubsystem>,
    registrations: BTreeMap<RegistrationKind, Vec<String>>,
    doc_graphs: Vec<nirvash::DocGraphCase>,
}

impl SpecDoc {
    fn viz_bundle(&self) -> nirvash::SpecVizBundle {
        let metadata = nirvash::SpecVizMetadata {
            spec_id: path_key(&self.full_path),
            kind: self.kind.map(|kind| match kind {
                SpecKind::Subsystem => nirvash::SpecVizKind::Subsystem,
                SpecKind::System => nirvash::SpecVizKind::System,
            }),
            state_ty: self.state_ty.clone(),
            action_ty: self.action_ty.clone(),
            model_cases: self.model_cases.clone(),
            subsystems: self.subsystems.clone(),
            registrations: nirvash::SpecVizRegistrationSet {
                invariants: self
                    .registrations
                    .get(&RegistrationKind::Invariant)
                    .cloned()
                    .unwrap_or_default(),
                properties: self
                    .registrations
                    .get(&RegistrationKind::Property)
                    .cloned()
                    .unwrap_or_default(),
                fairness: self
                    .registrations
                    .get(&RegistrationKind::Fairness)
                    .cloned()
                    .unwrap_or_default(),
                state_constraints: self
                    .registrations
                    .get(&RegistrationKind::StateConstraint)
                    .cloned()
                    .unwrap_or_default(),
                action_constraints: self
                    .registrations
                    .get(&RegistrationKind::ActionConstraint)
                    .cloned()
                    .unwrap_or_default(),
                symmetries: self
                    .registrations
                    .get(&RegistrationKind::Symmetry)
                    .cloned()
                    .unwrap_or_default(),
            },
            policy: nirvash::VizPolicy::default(),
        };

        nirvash::SpecVizBundle::from_doc_graph_spec(
            self.tail_ident.clone(),
            metadata,
            self.doc_graphs.clone(),
        )
    }
}

#[derive(Debug, Clone)]
struct PendingSpec {
    kind: SpecKind,
    full_path: Vec<String>,
    tail_ident: String,
    state_ty: String,
    action_ty: String,
    model_cases: Option<String>,
    subsystems: Vec<SynPath>,
}

#[derive(Debug, Clone)]
struct PendingRegistration {
    kind: RegistrationKind,
    target_spec: Vec<String>,
    function_name: String,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    kind: Vec<String>,
}

struct SourceCollector {
    visited: HashSet<PathBuf>,
    rerun_if_changed: BTreeSet<PathBuf>,
    specs: Vec<PendingSpec>,
    registrations: Vec<PendingRegistration>,
}

impl SourceCollector {
    fn new() -> Self {
        Self {
            visited: HashSet::new(),
            rerun_if_changed: BTreeSet::new(),
            specs: Vec::new(),
            registrations: Vec::new(),
        }
    }

    fn collect_root(&mut self, manifest_dir: &Path) -> Result<(), DynError> {
        let src_dir = manifest_dir.join("src");
        let root = src_dir.join("lib.rs");
        self.collect_file(&root, &[], &src_dir)
    }

    fn collect_file(
        &mut self,
        file: &Path,
        module_path: &[String],
        module_dir: &Path,
    ) -> Result<(), DynError> {
        let canonical = fs::canonicalize(file)
            .map_err(|error| err(format!("failed to resolve {}: {error}", file.display())))?;
        if !self.visited.insert(canonical) {
            return Ok(());
        }
        self.rerun_if_changed.insert(file.to_path_buf());

        let source = fs::read_to_string(file)
            .map_err(|error| err(format!("failed to read {}: {error}", file.display())))?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| err(format!("failed to parse {}: {error}", file.display())))?;
        self.collect_items(&parsed.items, module_path, module_dir)
    }

    fn collect_items(
        &mut self,
        items: &[Item],
        module_path: &[String],
        module_dir: &Path,
    ) -> Result<(), DynError> {
        for item in items {
            let attrs = item_attrs(item);
            if is_cfg_test(attrs) {
                continue;
            }
            match item {
                Item::Mod(item_mod) => self.collect_module(item_mod, module_path, module_dir)?,
                Item::Impl(item_impl) => self.collect_spec(item_impl, module_path)?,
                Item::Fn(item_fn) => self.collect_registration(item_fn, module_path)?,
                Item::Macro(item_macro) => {
                    self.collect_macro_registration(item_macro, module_path)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn collect_module(
        &mut self,
        item_mod: &ItemMod,
        module_path: &[String],
        module_dir: &Path,
    ) -> Result<(), DynError> {
        if has_path_attr(&item_mod.attrs) {
            return Err(err(format!(
                "unsupported #[path = ...] on module `{}` in nirvash-docgen",
                item_mod.ident
            )));
        }

        let mut next_module_path = module_path.to_vec();
        next_module_path.push(item_mod.ident.to_string());
        let next_module_dir = module_dir.join(item_mod.ident.to_string());

        if let Some((_, items)) = &item_mod.content {
            return self.collect_items(items, &next_module_path, &next_module_dir);
        }

        let file = resolve_module_file(item_mod, module_dir)?;
        self.collect_file(&file, &next_module_path, &next_module_dir)
    }

    fn collect_spec(
        &mut self,
        item_impl: &ItemImpl,
        module_path: &[String],
    ) -> Result<(), DynError> {
        let mut spec_kind = None;
        let mut args = ParsedSpecArgs::default();
        for attr in &item_impl.attrs {
            if attr.path().is_ident("subsystem_spec") {
                spec_kind = Some(SpecKind::Subsystem);
                args = parse_spec_args(attr)?;
            } else if attr.path().is_ident("system_spec") {
                spec_kind = Some(SpecKind::System);
                args = parse_spec_args(attr)?;
            }
        }
        let Some(kind) = spec_kind else {
            return Ok(());
        };

        let self_path = match &*item_impl.self_ty {
            Type::Path(type_path) if type_path.qself.is_none() => &type_path.path,
            _ => {
                return Err(err(
                    "nirvash-docgen only supports impl FrontendSpec for <simple path>",
                ));
            }
        };
        let full_path = normalize_path(self_path, module_path)?;
        let tail_ident = full_path
            .last()
            .cloned()
            .ok_or_else(|| err("spec path cannot be empty"))?;

        let state_ty = associated_type_string(item_impl, "State")?;
        let action_ty = associated_type_string(item_impl, "Action")?;

        self.specs.push(PendingSpec {
            kind,
            full_path,
            tail_ident,
            state_ty,
            action_ty,
            model_cases: args.model_cases,
            subsystems: args.subsystems,
        });
        Ok(())
    }

    fn collect_registration(
        &mut self,
        item_fn: &ItemFn,
        module_path: &[String],
    ) -> Result<(), DynError> {
        for attr in &item_fn.attrs {
            let Some(kind) = registration_kind(attr) else {
                continue;
            };
            let target = attr
                .parse_args::<ParsedRegistrationArgs>()
                .map_err(|error| {
                    err(format!(
                        "failed to parse #[{}(...)] on `{}`: {error}",
                        kind.attr_name(),
                        item_fn.sig.ident
                    ))
                })?;
            self.registrations.push(PendingRegistration {
                kind,
                target_spec: normalize_path(&target.target_spec, module_path)?,
                function_name: item_fn.sig.ident.to_string(),
            });
        }
        Ok(())
    }

    fn collect_macro_registration(
        &mut self,
        item_macro: &ItemMacro,
        module_path: &[String],
    ) -> Result<(), DynError> {
        let Some(kind) = registration_kind_for_path(&item_macro.mac.path) else {
            return Ok(());
        };

        let parsed = if kind == RegistrationKind::Fairness {
            let parsed: ParsedFairnessMacroRegistration =
                syn::parse2(item_macro.mac.tokens.clone()).map_err(|error| {
                    err(format!(
                        "failed to parse `{}` declaration macro: {error}",
                        pretty_tokens(&item_macro.mac.path)
                    ))
                })?;
            ParsedMacroRegistration {
                target_spec: parsed.target_spec,
                function_name: parsed.function_name,
            }
        } else {
            syn::parse2::<ParsedMacroRegistration>(item_macro.mac.tokens.clone()).map_err(
                |error| {
                    err(format!(
                        "failed to parse `{}` declaration macro: {error}",
                        pretty_tokens(&item_macro.mac.path)
                    ))
                },
            )?
        };

        self.registrations.push(PendingRegistration {
            kind,
            target_spec: normalize_path(&parsed.target_spec, module_path)?,
            function_name: parsed.function_name,
        });
        Ok(())
    }

    fn finish(self, manifest_dir: &Path, out_dir: &Path) -> Result<GenerationOutput, DynError> {
        let mut by_path = BTreeMap::<String, SpecDoc>::new();
        let mut tail_to_path = HashMap::<String, String>::new();

        for spec in self.specs {
            let spec_path_key = path_key(&spec.full_path);
            if let Some(existing) = tail_to_path.get(&spec.tail_ident) {
                return Err(err(format!(
                    "duplicate spec tail ident `{}` for `{existing}` and `{}`",
                    spec.tail_ident, spec_path_key
                )));
            }
            tail_to_path.insert(spec.tail_ident.clone(), spec_path_key.clone());
            let subsystem_module_path =
                spec.full_path[..spec.full_path.len().saturating_sub(1)].to_vec();
            by_path.insert(
                spec_path_key,
                SpecDoc {
                    kind: Some(spec.kind),
                    full_path: spec.full_path,
                    tail_ident: spec.tail_ident,
                    state_ty: spec.state_ty,
                    action_ty: spec.action_ty,
                    model_cases: spec.model_cases,
                    subsystems: spec
                        .subsystems
                        .into_iter()
                        .map(|path| {
                            let normalized = normalize_path(&path, &subsystem_module_path)?;
                            let label = normalized
                                .last()
                                .cloned()
                                .ok_or_else(|| err("subsystem path cannot be empty"))?;
                            Ok(nirvash::SpecVizSubsystem::new(
                                crate::path_key(&normalized),
                                label,
                            ))
                        })
                        .collect::<Result<Vec<_>, DynError>>()?,
                    registrations: BTreeMap::new(),
                    doc_graphs: Vec::new(),
                },
            );
        }

        for registration in self.registrations {
            let key = path_key(&registration.target_spec);
            let Some(spec) = by_path.get_mut(&key) else {
                return Err(err(format!(
                    "registration `{}` targets unknown spec `{}`; nirvash-docgen does not resolve `use` aliases here, so registration targets must use a fully-qualified path such as `crate::system::SystemSpec`",
                    registration.function_name, key
                )));
            };
            spec.registrations
                .entry(registration.kind)
                .or_default()
                .push(registration.function_name);
        }

        let runtime_spec_paths = by_path
            .values()
            .map(|spec| spec.full_path.clone())
            .collect::<Vec<_>>();
        let mut runtime_bundles =
            collect_runtime_graphs(manifest_dir, out_dir, &runtime_spec_paths)?;
        for spec in by_path.values() {
            if runtime_bundles
                .iter()
                .all(|bundle| bundle.spec_name != spec.tail_ident)
            {
                runtime_bundles.push(spec.viz_bundle());
            }
        }
        let specs_by_tail = by_path
            .values()
            .map(|spec| (spec.tail_ident.clone(), spec.clone()))
            .collect::<BTreeMap<_, _>>();
        for bundle in &mut runtime_bundles {
            if let Some(spec) = specs_by_tail.get(&bundle.spec_name) {
                overlay_spec_doc_metadata(bundle, spec);
            }
        }
        runtime_bundles.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));

        let doc_dir = out_dir.join("nirvash-doc");
        fs::create_dir_all(&doc_dir).map_err(|error| {
            err(format!(
                "failed to create documentation fragment directory {}: {error}",
                doc_dir.display()
            ))
        })?;
        let viz_dir = out_dir.join("viz");
        fs::create_dir_all(&viz_dir).map_err(|error| {
            err(format!(
                "failed to create visualization bundle directory {}: {error}",
                viz_dir.display()
            ))
        })?;
        let active_names = runtime_bundles
            .iter()
            .map(|bundle| bundle.spec_name.clone())
            .collect::<BTreeSet<_>>();
        remove_stale_generated_outputs(&doc_dir, "md", &active_names)?;
        remove_stale_generated_outputs(&viz_dir, "json", &active_names)?;

        let mut fragments = Vec::new();
        for bundle in &runtime_bundles {
            let spec_name = bundle.spec_name.clone();
            let env_key = format!("NIRVASH_DOC_FRAGMENT_{}", to_upper_snake(&spec_name));
            let path = doc_dir.join(format!("{spec_name}.md"));
            let viz_path = viz_dir.join(format!("{spec_name}.json"));
            fs::write(
                &viz_path,
                serde_json::to_vec_pretty(&bundle).map_err(|error| {
                    err(format!(
                        "failed to serialize visualization bundle {}: {error}",
                        viz_path.display()
                    ))
                })?,
            )
            .map_err(|error| {
                err(format!(
                    "failed to write visualization bundle {}: {error}",
                    viz_path.display()
                ))
            })?;
            fs::write(
                &path,
                render_viz_fragment_with_catalog(bundle, &runtime_bundles),
            )
            .map_err(|error| {
                err(format!(
                    "failed to write documentation fragment {}: {error}",
                    path.display()
                ))
            })?;
            fragments.push(GeneratedFragment { env_key, path });
        }

        fragments.sort_by(|left, right| left.env_key.cmp(&right.env_key));

        Ok(GenerationOutput {
            fragments,
            rerun_if_changed: self.rerun_if_changed.into_iter().collect(),
        })
    }
}

fn overlay_spec_doc_metadata(bundle: &mut nirvash::SpecVizBundle, spec: &SpecDoc) {
    bundle.metadata.spec_id = path_key(&spec.full_path);
    bundle.metadata.subsystems = spec.subsystems.clone();
    if bundle.metadata.model_cases.is_none() {
        bundle.metadata.model_cases = spec.model_cases.clone();
    }
    if bundle.metadata.kind.is_none() {
        bundle.metadata.kind = spec.kind.map(|kind| match kind {
            SpecKind::Subsystem => nirvash::SpecVizKind::Subsystem,
            SpecKind::System => nirvash::SpecVizKind::System,
        });
    }

    merge_registration_names(
        &mut bundle.metadata.registrations.invariants,
        spec.registrations.get(&RegistrationKind::Invariant),
    );
    merge_registration_names(
        &mut bundle.metadata.registrations.properties,
        spec.registrations.get(&RegistrationKind::Property),
    );
    merge_registration_names(
        &mut bundle.metadata.registrations.fairness,
        spec.registrations.get(&RegistrationKind::Fairness),
    );
    merge_registration_names(
        &mut bundle.metadata.registrations.state_constraints,
        spec.registrations.get(&RegistrationKind::StateConstraint),
    );
    merge_registration_names(
        &mut bundle.metadata.registrations.action_constraints,
        spec.registrations.get(&RegistrationKind::ActionConstraint),
    );
    merge_registration_names(
        &mut bundle.metadata.registrations.symmetries,
        spec.registrations.get(&RegistrationKind::Symmetry),
    );
}

fn merge_registration_names(target: &mut Vec<String>, source: Option<&Vec<String>>) {
    let Some(source) = source else {
        return;
    };
    for name in source {
        if !target.contains(name) {
            target.push(name.clone());
        }
    }
}

#[derive(Default)]
struct ParsedSpecArgs {
    model_cases: Option<String>,
    subsystems: Vec<SynPath>,
}

impl syn::parse::Parse for ParsedSpecArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self::default();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            match ident.to_string().as_str() {
                "model_cases" => {
                    let path: SynPath = content.parse()?;
                    if !content.is_empty() {
                        return Err(syn::Error::new(
                            content.span(),
                            "expected model_cases(...) to contain exactly one function path",
                        ));
                    }
                    args.model_cases = Some(path_to_string_syn(&path)?);
                }
                "subsystems" => {
                    while !content.is_empty() {
                        args.subsystems.push(content.parse()?);
                        if content.peek(syn::Token![,]) {
                            let _ = content.parse::<syn::Token![,]>()?;
                        }
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unsupported nirvash spec argument `{other}`"),
                    ));
                }
            }

            if input.peek(syn::Token![,]) {
                let _ = input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(args)
    }
}

fn generate_at(manifest_dir: &Path, out_dir: &Path) -> Result<GenerationOutput, DynError> {
    let mut collector = SourceCollector::new();
    collector.collect_root(manifest_dir)?;
    collector
        .rerun_if_changed
        .insert(manifest_dir.join("Cargo.toml"));
    collector
        .rerun_if_changed
        .insert(manifest_dir.join("build.rs"));
    collector.rerun_if_changed.insert(manifest_dir.join("src"));
    collector.finish(manifest_dir, out_dir)
}

fn collect_runtime_graphs(
    manifest_dir: &Path,
    out_dir: &Path,
    spec_paths: &[Vec<String>],
) -> Result<Vec<nirvash::SpecVizBundle>, DynError> {
    let manifest_path = manifest_dir.join("Cargo.toml");
    if !manifest_path.exists() {
        return Ok(Vec::new());
    }

    let metadata = read_cargo_metadata(&manifest_path)?;
    let canonical_manifest = fs::canonicalize(&manifest_path).map_err(|error| {
        err(format!(
            "failed to resolve manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let current_package = metadata
        .packages
        .iter()
        .find(|package| {
            fs::canonicalize(&package.manifest_path)
                .map(|path| path == canonical_manifest)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            err(format!(
                "failed to locate current package for {} in cargo metadata",
                manifest_path.display()
            ))
        })?;
    current_package
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == "lib"))
        .ok_or_else(|| {
            err(format!(
                "nirvash-docgen requires a library target in package `{}`",
                current_package.name
            ))
        })?;
    let nirvash_manifest = metadata
        .packages
        .iter()
        .find(|package| package.name == "nirvash")
        .and_then(|package| package.manifest_path.parent().map(Path::to_path_buf))
        .ok_or_else(|| err("failed to locate `nirvash` package in cargo metadata"))?;

    let runner_dir = out_dir.join("nirvash-doc-runner");
    let runner_src_dir = runner_dir.join("src");
    fs::create_dir_all(&runner_src_dir).map_err(|error| {
        err(format!(
            "failed to create runtime graph runner directory {}: {error}",
            runner_src_dir.display()
        ))
    })?;

    let runner_manifest = runner_dir.join("Cargo.toml");
    let runner_main = runner_src_dir.join("main.rs");
    fs::write(
        &runner_manifest,
        render_runner_manifest(manifest_dir, &current_package.name, &nirvash_manifest),
    )
    .map_err(|error| {
        err(format!(
            "failed to write runtime graph runner manifest {}: {error}",
            runner_manifest.display()
        ))
    })?;
    fs::write(&runner_main, render_runner_main(spec_paths)).map_err(|error| {
        err(format!(
            "failed to write runtime graph runner source {}: {error}",
            runner_main.display()
        ))
    })?;

    let runner_target_dir = out_dir.join("nirvash-doc-runner-target");
    let runner_stdout = out_dir.join("nirvash-doc-runner.stdout.ndjson");
    let runner_stderr = out_dir.join("nirvash-doc-runner.stderr.log");
    let progress_enabled = env::var_os("NIRVASH_DOCGEN_PROGRESS").is_some();
    let stdout_file = File::create(&runner_stdout).map_err(|error| {
        err(format!(
            "failed to create runtime graph runner stdout {}: {error}",
            runner_stdout.display()
        ))
    })?;
    let stderr_file = File::create(&runner_stderr).map_err(|error| {
        err(format!(
            "failed to create runtime graph runner stderr {}: {error}",
            runner_stderr.display()
        ))
    })?;
    if progress_enabled {
        println!(
            "cargo:warning=nirvash-docgen progress log: {}",
            runner_stderr.display()
        );
    }
    let runner_stderr_for_thread = runner_stderr.clone();

    let mut command = Command::new(cargo_binary());
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&runner_manifest)
        .arg("--target-dir")
        .arg(&runner_target_dir)
        .env("NIRVASH_DOCGEN_SKIP", "1")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::piped());
    if progress_enabled {
        command.env("NIRVASH_DOCGEN_PROGRESS", "1");
    }
    let mut child = command.spawn().map_err(|error| {
        err(format!(
            "failed to execute runtime graph runner {}: {error}",
            runner_manifest.display()
        ))
    })?;
    let child_stderr = child.stderr.take().ok_or_else(|| {
        err(format!(
            "failed to capture runtime graph runner stderr for {}",
            runner_manifest.display()
        ))
    })?;
    let stderr_forwarder = std::thread::spawn(move || -> Result<(), String> {
        let reader = BufReader::new(child_stderr);
        let mut log_file = stderr_file;
        for line in reader.lines() {
            let line = line.map_err(|error| {
                format!(
                    "failed to read runtime graph runner stderr {}: {error}",
                    runner_stderr_for_thread.display()
                )
            })?;
            writeln!(log_file, "{line}").map_err(|error| {
                format!(
                    "failed to write runtime graph runner stderr log {}: {error}",
                    runner_stderr_for_thread.display()
                )
            })?;
            if progress_enabled {
                eprintln!("{line}");
            }
        }
        log_file.flush().map_err(|error| {
            format!(
                "failed to flush runtime graph runner stderr log {}: {error}",
                runner_stderr_for_thread.display()
            )
        })
    });
    let status = child.wait().map_err(|error| {
        err(format!(
            "failed to wait for runtime graph runner {}: {error}",
            runner_manifest.display()
        ))
    })?;
    stderr_forwarder
        .join()
        .map_err(|_| err("runtime graph runner stderr forwarder panicked"))?
        .map_err(err)?;

    if !status.success() {
        let stderr = fs::read_to_string(&runner_stderr).unwrap_or_else(|error| {
            format!(
                "failed to read runtime graph runner stderr {}: {error}",
                runner_stderr.display()
            )
        });
        return Err(err(format!(
            "runtime graph runner failed for {}:\n{}",
            runner_manifest.display(),
            stderr
        )));
    }

    read_runtime_graph_bundles(&runner_stdout, &runner_manifest)
}

fn read_runtime_graph_bundles(
    runner_stdout: &Path,
    runner_manifest: &Path,
) -> Result<Vec<nirvash::SpecVizBundle>, DynError> {
    let stdout_file = File::open(runner_stdout).map_err(|error| {
        err(format!(
            "failed to open runtime graph output {}: {error}",
            runner_stdout.display()
        ))
    })?;
    let reader = BufReader::new(stdout_file);
    let mut bundles_by_name = BTreeMap::<String, nirvash::SpecVizBundle>::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| {
            err(format!(
                "failed to read runtime graph output line {} from {}: {error}",
                line_index + 1,
                runner_stdout.display()
            ))
        })?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bundle = serde_json::from_str::<nirvash::SpecVizBundle>(line).map_err(|error| {
            err(format!(
                "failed to parse runtime graph output line {} from {} (runner {}): {error}",
                line_index + 1,
                runner_stdout.display(),
                runner_manifest.display()
            ))
        })?;
        nirvash::upsert_spec_viz_bundle(&mut bundles_by_name, bundle);
    }

    let mut bundles = bundles_by_name.into_values().collect::<Vec<_>>();
    bundles.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));
    Ok(bundles)
}

fn read_cargo_metadata(manifest_path: &Path) -> Result<CargoMetadata, DynError> {
    let output = Command::new(cargo_binary())
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .map_err(|error| {
            err(format!(
                "failed to execute `cargo metadata` for {}: {error}",
                manifest_path.display()
            ))
        })?;

    if !output.status.success() {
        return Err(err(format!(
            "`cargo metadata` failed for {}:\n{}",
            manifest_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        err(format!(
            "failed to parse `cargo metadata` output for {}: {error}",
            manifest_path.display()
        ))
    })
}

fn cargo_binary() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

fn render_runner_manifest(
    manifest_dir: &Path,
    current_package_name: &str,
    nirvash_dir: &Path,
) -> String {
    format!(
        "[package]\nname = \"nirvash-doc-runner\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nserde_json = \"1\"\nnirvash = {{ path = \"{}\" }}\ndoc_target = {{ package = \"{}\", path = \"{}\" }}\n\n[profile.dev]\ndebug = 0\nincremental = false\n",
        escape_toml_path(nirvash_dir),
        escape_toml_str(current_package_name),
        escape_toml_path(manifest_dir),
    )
}

fn render_runner_main(spec_paths: &[Vec<String>]) -> String {
    let mut output = String::from(
        "extern crate doc_target;\n\nuse std::{collections::VecDeque, io::{self, Write}, sync::{mpsc, Arc, Mutex}, thread};\n\nfn main() {\n",
    );
    for path in spec_paths {
        output.push_str("    ");
        output.push_str(&render_link_call(path));
        output.push('\n');
    }
    output.push_str(
        "    let registrations = nirvash::collect_primary_spec_viz_provider_registrations();\n    let total_specs = registrations.len();\n    let progress_enabled = std::env::var_os(\"NIRVASH_DOCGEN_PROGRESS\").is_some();\n    let worker_count = std::env::var(\"NIRVASH_DOCGEN_JOBS\")\n        .ok()\n        .and_then(|value| value.parse::<usize>().ok())\n        .filter(|value| *value > 0)\n        .unwrap_or_else(|| thread::available_parallelism().map(|value| value.get()).unwrap_or(1))\n        .min(total_specs.max(1))\n        .min(4);\n    if progress_enabled {\n        eprintln!(\"nirvash-docgen starting {total_specs} spec(s) with {worker_count} worker(s)\");\n    }\n    let queue = Arc::new(Mutex::new(VecDeque::from(registrations)));\n    let (tx, rx) = mpsc::channel();\n    let mut workers = Vec::new();\n    for _ in 0..worker_count {\n        let queue = Arc::clone(&queue);\n        let tx = tx.clone();\n        workers.push(thread::spawn(move || {\n            loop {\n                let registration = {\n                    let mut queue = queue.lock().expect(\"lock doc graph queue\");\n                    queue.pop_front()\n                };\n                let Some(registration) = registration else {\n                    break;\n                };\n                let spec_name = registration.spec_name.to_owned();\n                let bundle = (registration.build)().bundle();\n                tx.send((spec_name, bundle)).expect(\"send doc graph bundle\");\n            }\n        }));\n    }\n    drop(tx);\n    let started_at = std::time::Instant::now();\n    let stdout = io::stdout();\n    let mut writer = io::BufWriter::new(stdout.lock());\n    let mut completed = 0usize;\n    for (spec_name, bundle) in rx {\n        completed += 1;\n        if progress_enabled {\n            eprintln!(\n                \"nirvash-docgen spec progress {completed}/{total_specs} after {:?}: {}\",\n                started_at.elapsed(),\n                spec_name,\n            );\n        }\n        serde_json::to_writer(&mut writer, &bundle).expect(\"serialize doc graph bundle\");\n        writer.write_all(b\"\\n\").expect(\"write doc graph bundle separator\");\n    }\n    for worker in workers {\n        worker.join().expect(\"join doc graph worker\");\n    }\n    writer.flush().expect(\"flush doc graph bundles\");\n}\n",
    );
    output
}

fn escape_toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn escape_toml_str(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn render_link_call(spec_path: &[String]) -> String {
    let (tail, modules) = spec_path
        .split_last()
        .expect("spec path always contains at least one segment");
    let mut path = String::from("doc_target");
    for module in modules {
        path.push_str("::");
        path.push_str(module);
    }
    path.push_str("::");
    path.push_str(tail);
    path.push_str("::spec_kind();");
    path
}

fn parse_spec_args(attr: &Attribute) -> Result<ParsedSpecArgs, DynError> {
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok(ParsedSpecArgs::default());
    }
    attr.parse_args::<ParsedSpecArgs>().map_err(|error| {
        err(format!(
            "failed to parse #[{}(...)] arguments: {error}",
            attr.path().to_token_stream()
        ))
    })
}

fn registration_kind(attr: &Attribute) -> Option<RegistrationKind> {
    registration_kind_for_path(attr.path())
}

fn registration_kind_for_path(path: &SynPath) -> Option<RegistrationKind> {
    match path.segments.last()?.ident.to_string().as_str() {
        "invariant" => Some(RegistrationKind::Invariant),
        "property" => Some(RegistrationKind::Property),
        "fairness" => Some(RegistrationKind::Fairness),
        "state_constraint" => Some(RegistrationKind::StateConstraint),
        "action_constraint" => Some(RegistrationKind::ActionConstraint),
        "symmetry" => Some(RegistrationKind::Symmetry),
        _ => None,
    }
}

struct ParsedMacroRegistration {
    target_spec: SynPath,
    function_name: String,
}

struct ParsedRegistrationArgs {
    target_spec: SynPath,
}

impl syn::parse::Parse for ParsedRegistrationArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let target_spec: SynPath = input.parse()?;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            let _: syn::Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            let _: proc_macro2::TokenStream = content.parse()?;
        }
        Ok(Self { target_spec })
    }
}

impl syn::parse::Parse for ParsedMacroRegistration {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let target_spec: SynPath = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let function_name: syn::Ident = input.parse()?;
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let _: proc_macro2::TokenStream = content.parse()?;
        }
        input.parse::<syn::Token![=>]>()?;
        let _: proc_macro2::TokenStream = input.parse()?;
        Ok(Self {
            target_spec,
            function_name: function_name.to_string(),
        })
    }
}

struct ParsedFairnessMacroRegistration {
    target_spec: SynPath,
    function_name: String,
}

impl syn::parse::Parse for ParsedFairnessMacroRegistration {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let strength: syn::Ident = input.parse()?;
        match strength.to_string().as_str() {
            "weak" | "strong" => {}
            _ => {
                return Err(syn::Error::new(
                    strength.span(),
                    "fairness! expects `weak` or `strong` before the spec path",
                ));
            }
        }

        let target_spec: SynPath = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let function_name: syn::Ident = input.parse()?;
        let content;
        syn::parenthesized!(content in input);
        let _: proc_macro2::TokenStream = content.parse()?;
        input.parse::<syn::Token![=>]>()?;
        let _: proc_macro2::TokenStream = input.parse()?;
        Ok(Self {
            target_spec,
            function_name: function_name.to_string(),
        })
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        (attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
            && attr.meta.to_token_stream().to_string().contains("test")
    })
}

fn has_path_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("path"))
}

fn resolve_module_file(item_mod: &ItemMod, module_dir: &Path) -> Result<PathBuf, DynError> {
    let module_name = item_mod.ident.to_string();
    let flat = module_dir.join(format!("{module_name}.rs"));
    let nested = module_dir.join(&module_name).join("mod.rs");

    match (flat.exists(), nested.exists()) {
        (true, false) => Ok(flat),
        (false, true) => Ok(nested),
        (false, false) => Err(err(format!(
            "failed to resolve module `{module_name}` under {}",
            module_dir.display()
        ))),
        (true, true) => Err(err(format!(
            "module `{module_name}` is ambiguous under {}",
            module_dir.display()
        ))),
    }
}

fn associated_type_string(item_impl: &ItemImpl, name: &str) -> Result<String, DynError> {
    item_impl
        .items
        .iter()
        .find_map(|item| match item {
            ImplItem::Type(assoc) if assoc.ident == name => Some(pretty_tokens(&assoc.ty)),
            _ => None,
        })
        .ok_or_else(|| err(format!("missing type {name} = ... in spec impl")))
}

fn normalize_path(path: &SynPath, module_path: &[String]) -> Result<Vec<String>, DynError> {
    if path.segments.is_empty() {
        return Err(err("path cannot be empty"));
    }
    let segments = path
        .segments
        .iter()
        .map(|segment| {
            if !matches!(segment.arguments, PathArguments::None) {
                return Err(err(format!(
                    "unsupported path argument in `{}`",
                    segment.ident
                )));
            }
            Ok(segment.ident.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut absolute = Vec::new();
    let mut index = 0;
    match segments.first().map(String::as_str) {
        Some("crate") => index = 1,
        Some("self") => {
            absolute.extend_from_slice(module_path);
            index = 1;
        }
        Some("super") => {
            absolute.extend_from_slice(module_path);
            while matches!(segments.get(index).map(String::as_str), Some("super")) {
                if absolute.pop().is_none() {
                    return Err(err(format!(
                        "path `{}` escapes above crate root",
                        path_to_string(path)?
                    )));
                }
                index += 1;
            }
        }
        _ => absolute.extend_from_slice(module_path),
    }

    absolute.extend(segments.into_iter().skip(index));
    if absolute.is_empty() {
        return Err(err("normalized path cannot be empty"));
    }
    Ok(absolute)
}

fn path_key(path: &[String]) -> String {
    format!("crate::{}", path.join("::"))
}

fn path_to_string(path: &SynPath) -> Result<String, DynError> {
    for segment in &path.segments {
        if !matches!(segment.arguments, PathArguments::None) {
            return Err(err(format!(
                "unsupported path argument in `{}`",
                segment.ident
            )));
        }
    }
    Ok(path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::"))
}

fn path_to_string_syn(path: &SynPath) -> syn::Result<String> {
    for segment in &path.segments {
        if !matches!(segment.arguments, PathArguments::None) {
            return Err(syn::Error::new(
                segment.ident.span(),
                format!("unsupported path argument in `{}`", segment.ident),
            ));
        }
    }
    Ok(path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::"))
}

fn pretty_tokens(value: &impl ToTokens) -> String {
    let mut text = value.to_token_stream().to_string();
    for (from, to) in [
        (" :: ", "::"),
        (" < ", "<"),
        (" > ", ">"),
        (" , ", ", "),
        (" ( ", "("),
        (" ) ", ")"),
        (" [ ", "["),
        (" ] ", "]"),
        (" & ", "&"),
    ] {
        text = text.replace(from, to);
    }
    text
}

fn to_upper_snake(input: &str) -> String {
    let mut output = String::new();
    let mut previous_is_lower = false;
    for character in input.chars() {
        if character.is_ascii_uppercase() {
            if previous_is_lower && !output.ends_with('_') {
                output.push('_');
            }
            output.push(character);
            previous_is_lower = false;
        } else if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_uppercase());
            previous_is_lower = true;
        } else {
            if !output.ends_with('_') && !output.is_empty() {
                output.push('_');
            }
            previous_is_lower = false;
        }
    }
    output
}

fn to_lower_snake(input: &str) -> String {
    let mut output = String::new();
    let mut previous_is_lower = false;
    for character in input.chars() {
        if character.is_ascii_uppercase() {
            if previous_is_lower && !output.ends_with('_') {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
            previous_is_lower = false;
        } else if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            previous_is_lower = true;
        } else {
            if !output.ends_with('_') && !output.is_empty() {
                output.push('_');
            }
            previous_is_lower = false;
        }
    }
    output
}

#[cfg(test)]
fn render_fragment(spec: &SpecDoc) -> String {
    let bundle = spec.viz_bundle();
    render_viz_fragment(&bundle)
}

fn subsystem_labels(subsystems: &[nirvash::SpecVizSubsystem]) -> Vec<String> {
    subsystems
        .iter()
        .map(|subsystem| subsystem.label.clone())
        .collect()
}

#[derive(Debug)]
struct BundleCatalog<'a> {
    bundles: &'a [nirvash::SpecVizBundle],
    by_spec_id: BTreeMap<String, usize>,
    parents_by_subsystem_id: BTreeMap<String, Vec<usize>>,
}

impl<'a> BundleCatalog<'a> {
    fn new(bundles: &'a [nirvash::SpecVizBundle]) -> Self {
        let mut by_spec_id = BTreeMap::new();
        let mut parents_by_subsystem_id = BTreeMap::<String, Vec<usize>>::new();
        for (index, bundle) in bundles.iter().enumerate() {
            by_spec_id.insert(bundle.metadata.spec_id.clone(), index);
            for subsystem in &bundle.metadata.subsystems {
                parents_by_subsystem_id
                    .entry(subsystem.spec_id.clone())
                    .or_default()
                    .push(index);
            }
        }
        for indices in parents_by_subsystem_id.values_mut() {
            indices.sort_unstable_by(|left, right| {
                bundles[*left].spec_name.cmp(&bundles[*right].spec_name)
            });
            indices.dedup();
        }
        Self {
            bundles,
            by_spec_id,
            parents_by_subsystem_id,
        }
    }

    fn bundle(&self, spec_id: &str) -> Option<&'a nirvash::SpecVizBundle> {
        self.by_spec_id
            .get(spec_id)
            .and_then(|index| self.bundles.get(*index))
    }

    fn parent_systems(&self, spec_id: &str) -> Vec<&'a nirvash::SpecVizBundle> {
        self.parents_by_subsystem_id
            .get(spec_id)
            .into_iter()
            .flatten()
            .filter_map(|index| self.bundles.get(*index))
            .collect()
    }
}

#[derive(Debug, Default)]
struct VizPage {
    sections: Vec<PageSection>,
}

#[derive(Debug)]
struct PageSection {
    title: &'static str,
    blocks: Vec<PageBlock>,
}

#[derive(Debug)]
enum PageBlock {
    Markdown(String),
    Mermaid(String),
    Details { summary: String, body: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SpecLink {
    label: String,
    spec_id: String,
}

impl SpecLink {
    fn href(&self) -> &str {
        &self.spec_id
    }

    fn markdown(&self) -> String {
        format!("[`{}`]({})", self.label, self.href())
    }
}

#[derive(Debug, Clone)]
struct MermaidAliasMap {
    ordered: Vec<(String, String)>,
    ids: BTreeMap<String, String>,
}

impl MermaidAliasMap {
    fn new(labels: &[String], prefix: &str) -> Self {
        let mut ordered = Vec::new();
        let mut ids = BTreeMap::new();
        let mut collisions = BTreeMap::<String, usize>::new();
        for label in labels {
            if ids.contains_key(label) {
                continue;
            }
            let base = format!("{}_{}", prefix, mermaid_entity_id(label));
            let counter = collisions.entry(base.clone()).or_default();
            *counter += 1;
            let id = if *counter == 1 {
                base
            } else {
                format!("{base}_{}", *counter)
            };
            ids.insert(label.clone(), id.clone());
            ordered.push((label.clone(), id));
        }
        Self { ordered, ids }
    }

    fn id(&self, label: &str) -> String {
        self.ids
            .get(label)
            .cloned()
            .unwrap_or_else(|| mermaid_entity_id(label))
    }

    fn note_scope(&self) -> String {
        match self.ordered.as_slice() {
            [] => "Spec".to_owned(),
            [(_, id)] => id.clone(),
            [(_, first_id), .., (_, last_id)] => format!("{first_id},{last_id}"),
        }
    }
}

#[cfg(test)]
fn render_viz_fragment(bundle: &nirvash::SpecVizBundle) -> String {
    render_viz_fragment_with_catalog(bundle, std::slice::from_ref(bundle))
}

fn render_viz_fragment_with_catalog(
    bundle: &nirvash::SpecVizBundle,
    bundles: &[nirvash::SpecVizBundle],
) -> String {
    let catalog = BundleCatalog::new(bundles);
    let page = build_viz_page(bundle, &catalog);
    render_viz_page(&page)
}

fn build_viz_page(bundle: &nirvash::SpecVizBundle, catalog: &BundleCatalog<'_>) -> VizPage {
    VizPage {
        sections: vec![
            build_system_map_section(bundle, catalog),
            build_scenario_atlas_section(bundle),
            build_actor_flows_section(bundle),
            build_state_space_section(bundle),
            build_contracts_data_section(bundle),
        ],
    }
}

fn bundle_case_groups(
    bundle: &nirvash::SpecVizBundle,
) -> Vec<(String, Vec<&nirvash::SpecVizCase>)> {
    let mut groups = Vec::<(String, Vec<&nirvash::SpecVizCase>)>::new();
    for case in &bundle.cases {
        let surface = case.surface.clone().unwrap_or_else(|| "System".to_owned());
        if let Some((_, grouped_cases)) = groups.iter_mut().find(|(name, _)| *name == surface) {
            grouped_cases.push(case);
        } else {
            groups.push((surface, vec![case]));
        }
    }
    groups
}

fn bundle_has_surface_groups(bundle: &nirvash::SpecVizBundle) -> bool {
    matches!(bundle.metadata.kind, Some(nirvash::SpecVizKind::System))
        && bundle.cases.iter().any(|case| case.surface.is_some())
}

fn render_viz_page(page: &VizPage) -> String {
    let mut output = String::new();
    for (index, section) in page.sections.iter().enumerate() {
        if index > 0 {
            output.push_str("\n\n");
        }
        output.push_str(&format!("## {}\n\n", section.title));
        for (block_index, block) in section.blocks.iter().enumerate() {
            if block_index > 0 {
                output.push_str("\n\n");
            }
            output.push_str(&render_page_block(block));
        }
    }
    output.push('\n');
    output.push_str(&mermaid_render_script());
    output
}

fn render_page_block(block: &PageBlock) -> String {
    match block {
        PageBlock::Markdown(markdown) => markdown.trim_end().to_owned(),
        PageBlock::Mermaid(diagram) => render_mermaid_block(diagram),
        PageBlock::Details { summary, body } => format!(
            "<details><summary>{}</summary>\n\n{}\n\n</details>",
            escape_html(summary),
            body.trim_end()
        ),
    }
}

fn build_system_map_section(
    bundle: &nirvash::SpecVizBundle,
    catalog: &BundleCatalog<'_>,
) -> PageSection {
    let kind = match bundle.metadata.kind {
        Some(nirvash::SpecVizKind::System) => "system_spec",
        Some(nirvash::SpecVizKind::Subsystem) => "subsystem_spec",
        None => "unknown",
    };
    let mut blocks = Vec::new();
    blocks.push(PageBlock::Markdown(format!(
        "| field | value |\n| --- | --- |\n| spec | `{}` |\n| kind | `{kind}` |\n| spec id | `{}` |\n| model cases | `{}` |",
        bundle.spec_name,
        bundle.metadata.spec_id,
        bundle.metadata.model_cases.as_deref().unwrap_or("default")
    )));

    blocks.push(PageBlock::Mermaid(render_system_map_mermaid(
        bundle, catalog,
    )));

    let mut navigation = String::new();
    match bundle.metadata.kind {
        Some(nirvash::SpecVizKind::System) => {
            let subsystem_links = bundle
                .metadata
                .subsystems
                .iter()
                .map(|subsystem| {
                    resolve_spec_link(catalog, subsystem.spec_id.as_str(), &subsystem.label)
                })
                .collect::<Vec<_>>();
            if bundle_has_surface_groups(bundle) {
                navigation.push_str("### Views\n\n");
                for (surface, cases) in bundle_case_groups(bundle) {
                    let projections = cases
                        .iter()
                        .filter_map(|case| case.projection.as_deref())
                        .collect::<BTreeSet<_>>();
                    if projections.is_empty() {
                        navigation.push_str(&format!("- `{surface}`\n"));
                    } else {
                        navigation.push_str(&format!(
                            "- `{surface}`: projection `{}`\n",
                            projections.into_iter().collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            } else {
                navigation.push_str("### Subsystems\n\n");
                if subsystem_links.is_empty() {
                    navigation.push_str("- none\n");
                } else {
                    for link in subsystem_links {
                        navigation.push_str(&format!("- {}\n", link.markdown()));
                    }
                }
            }

            let actors = collect_bundle_actors(bundle);
            navigation.push_str("\n### Actors\n\n");
            if actors.is_empty() {
                navigation.push_str("- none\n");
            } else {
                for actor in &actors {
                    navigation.push_str(&format!("- `{actor}`\n"));
                }
            }

            let channels = collect_system_channels(bundle);
            navigation.push_str("\n### Channels\n\n");
            if channels.is_empty() {
                navigation.push_str("- none\n");
            } else {
                for (from, to, label) in channels {
                    navigation.push_str(&format!("- `{from} -> {to}`: `{label}`\n"));
                }
            }
        }
        Some(nirvash::SpecVizKind::Subsystem) | None => {
            let parent_links = catalog
                .parent_systems(&bundle.metadata.spec_id)
                .into_iter()
                .map(spec_link_from_bundle)
                .collect::<Vec<_>>();
            let related_links = related_subsystem_links(bundle, catalog);

            navigation.push_str("### Parent Systems\n\n");
            if parent_links.is_empty() {
                navigation.push_str("- none\n");
            } else {
                for link in &parent_links {
                    navigation.push_str(&format!("- {}\n", link.markdown()));
                }
            }

            navigation.push_str("\n### Related Subsystems\n\n");
            if related_links.is_empty() {
                navigation.push_str("- none\n");
            } else {
                for link in &related_links {
                    navigation.push_str(&format!("- {}\n", link.markdown()));
                }
            }
        }
    }
    blocks.push(PageBlock::Markdown(navigation));

    PageSection {
        title: "System Map",
        blocks,
    }
}

fn build_scenario_atlas_section(bundle: &nirvash::SpecVizBundle) -> PageSection {
    let mut blocks = Vec::new();
    let grouped = bundle_has_surface_groups(bundle);
    for (surface, cases) in bundle_case_groups(bundle) {
        if grouped {
            blocks.push(PageBlock::Markdown(format!("### {surface}\n")));
        }
        for case in cases {
            let mut heading = format!(
                "{} {}\n\n- backend: `{}`\n- representative traces: `{}`\n",
                if grouped { "####" } else { "###" },
                case.label,
                render_model_backend(case.backend),
                case.scenarios.len()
            );
            if case.stats.truncated {
                heading.push_str("- checker note: truncated by checker limits\n");
            }
            if case.stats.stutter_omitted {
                heading.push_str("- checker note: stutter omitted from rendered edges\n");
            }
            blocks.push(PageBlock::Markdown(heading));
            if case.scenarios.is_empty() {
                blocks.push(PageBlock::Markdown(
                    "No representative traces selected.".to_owned(),
                ));
                continue;
            }
            for scenario in ordered_viz_scenarios(&case.scenarios) {
                blocks.push(PageBlock::Markdown(format!(
                    "{} {}\n\n- class: `{}`\n- priority: `{}`\n- path: `{}`\n",
                    if grouped { "#####" } else { "####" },
                    scenario.label,
                    scenario_atlas_label(scenario.kind),
                    scenario_max_priority(scenario),
                    scenario_path_label(&scenario.state_path)
                )));
                if scenario.actors.len() >= 2 {
                    blocks.push(PageBlock::Mermaid(render_viz_sequence_diagram_mermaid(
                        bundle, case, scenario,
                    )));
                } else {
                    blocks.push(PageBlock::Markdown(render_viz_step_table(scenario)));
                }
            }
        }
    }
    PageSection {
        title: "Scenario Atlas",
        blocks,
    }
}

fn build_actor_flows_section(bundle: &nirvash::SpecVizBundle) -> PageSection {
    let mut blocks = Vec::new();
    let grouped = bundle_has_surface_groups(bundle);
    for (surface, cases) in bundle_case_groups(bundle) {
        if grouped {
            blocks.push(PageBlock::Markdown(format!("### {surface}\n")));
        }
        for case in cases {
            blocks.push(PageBlock::Markdown(format!(
                "{} {}\n",
                if grouped { "####" } else { "###" },
                case.label
            )));
            let scenarios = ordered_viz_scenarios(&case.scenarios);
            let actors = collect_actor_flow_actors(case, &scenarios);
            for actor in actors {
                blocks.push(PageBlock::Markdown(format!(
                    "{} `{actor}`\n",
                    if grouped { "#####" } else { "####" }
                )));
                blocks.push(PageBlock::Mermaid(render_actor_flow_mermaid(
                    &actor, &scenarios,
                )));
            }
            blocks.push(PageBlock::Details {
                summary: format!("{} process text fallback", case.label),
                body: render_code_block("text", &render_case_process_view(case)),
            });
        }
    }
    PageSection {
        title: "Actor Flows",
        blocks,
    }
}

fn build_state_space_section(bundle: &nirvash::SpecVizBundle) -> PageSection {
    let mut blocks = Vec::new();
    let threshold = bundle.metadata.policy.large_graph_threshold;
    let grouped = bundle_has_surface_groups(bundle);
    for (surface, cases) in bundle_case_groups(bundle) {
        if grouped {
            blocks.push(PageBlock::Markdown(format!("### {surface}\n")));
        }
        for case in cases {
            let mut heading = format!(
                "{} {}\n\n- states: full=`{}`, reduced=`{}`, focus=`{}`\n- edges: full=`{}`, reduced=`{}`\n",
                if grouped { "####" } else { "###" },
                case.label,
                case.stats.full_state_count,
                case.stats.reduced_state_count,
                case.stats.focus_state_count,
                case.stats.full_edge_count,
                case.stats.reduced_edge_count
            );
            if case.stats.truncated {
                heading.push_str("- checker note: truncated by checker limits\n");
            }
            if case.stats.stutter_omitted {
                heading.push_str("- checker note: stutter omitted from rendered edges\n");
            }
            blocks.push(PageBlock::Markdown(heading));

            if case.reduced_graph.states.len() <= threshold {
                blocks.push(PageBlock::Markdown(
                    "Rendered graph: reduced reachable graph.".to_owned(),
                ));
                blocks.push(PageBlock::Mermaid(render_viz_state_graph_mermaid(
                    bundle,
                    case,
                    &case.reduced_graph,
                    &visible_reduced_edges(&case.reduced_graph),
                )));
                blocks.push(PageBlock::Details {
                    summary: "State legend".to_owned(),
                    body: render_state_legend(&case.reduced_graph),
                });
                continue;
            }

            if let Some(focus_graph) = case.focus_graph.as_ref()
                && focus_graph.states.len() <= threshold
            {
                blocks.push(PageBlock::Markdown(format!(
                    "Reduced graph omitted because {} reduced states exceed limit {}. Rendering focus graph selected from representative scenarios.",
                    case.reduced_graph.states.len(),
                    threshold
                )));
                blocks.push(PageBlock::Mermaid(render_viz_state_graph_mermaid(
                    bundle,
                    case,
                    focus_graph,
                    &visible_reduced_edges(focus_graph),
                )));
                blocks.push(PageBlock::Details {
                    summary: "Focus state legend".to_owned(),
                    body: render_state_legend(focus_graph),
                });
                continue;
            }

            blocks.push(PageBlock::Markdown(format!(
                "Reduced graph omitted because {} reduced states exceed limit {}. Focus graph also exceeds the inline threshold, so scenario mini diagrams are shown instead.",
                case.reduced_graph.states.len(),
                threshold
            )));
            for scenario in ordered_viz_scenarios(&case.scenarios) {
                blocks.push(PageBlock::Markdown(format!(
                    "{} {}\n",
                    if grouped { "#####" } else { "####" },
                    scenario.label
                )));
                blocks.push(PageBlock::Mermaid(render_scenario_state_space_mermaid(
                    case, scenario,
                )));
            }
        }
    }
    PageSection {
        title: "State Space",
        blocks,
    }
}

fn build_contracts_data_section(bundle: &nirvash::SpecVizBundle) -> PageSection {
    let mut blocks = Vec::new();
    let mut spec_table = String::from("### Spec Contract\n\n| field | value |\n| --- | --- |\n");
    spec_table.push_str(&format!("| state | `{}` |\n", bundle.metadata.state_ty));
    spec_table.push_str(&format!("| action | `{}` |\n", bundle.metadata.action_ty));
    spec_table.push_str(&format!(
        "| model cases | `{}` |\n",
        bundle.metadata.model_cases.as_deref().unwrap_or("default")
    ));
    spec_table.push_str(&format!(
        "| subsystems | {} |\n",
        if bundle.metadata.subsystems.is_empty() {
            "none".to_owned()
        } else {
            subsystem_labels(&bundle.metadata.subsystems).join(", ")
        }
    ));
    blocks.push(PageBlock::Markdown(spec_table));

    let mut case_table = String::from(
        "### Case Summary\n\n| case | backend | full states | reduced states | traces | rendering |\n| --- | --- | --- | --- | --- | --- |\n",
    );
    for case in &bundle.cases {
        case_table.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} |\n",
            case.label,
            render_model_backend(case.backend),
            case.stats.full_state_count,
            case.stats.reduced_state_count,
            case.scenarios.len(),
            if case.reduced_graph.states.len() <= bundle.metadata.policy.large_graph_threshold {
                "reduced graph"
            } else if case.focus_graph.as_ref().is_some_and(
                |graph| graph.states.len() <= bundle.metadata.policy.large_graph_threshold
            ) {
                "focus graph"
            } else {
                "scenario mini diagrams"
            }
        ));
    }
    blocks.push(PageBlock::Markdown(case_table));

    let mut actions = String::from("### Action Vocabulary\n\n");
    if bundle.action_vocabulary.is_empty() {
        actions.push_str("- none\n");
    } else {
        for action in &bundle.action_vocabulary {
            actions.push_str(&format!(
                "- `{}`",
                action
                    .compact_label
                    .as_deref()
                    .unwrap_or(action.label.as_str())
            ));
            if let Some(priority) = action.scenario_priority {
                actions.push_str(&format!(" priority={priority}"));
            }
            if action.compact_label.is_some() {
                actions.push_str(&format!(" (`{}`)", action.label));
            }
            actions.push('\n');
        }
    }
    blocks.push(PageBlock::Markdown(actions));

    let relation_section = render_contract_relation_schema(bundle);
    if !relation_section.is_empty() {
        blocks.extend(relation_section);
    }

    let mut constraints = String::from("### Constraints\n\n");
    render_named_block(
        &mut constraints,
        "invariants",
        &bundle.metadata.registrations.invariants,
    );
    render_named_block(
        &mut constraints,
        "properties",
        &bundle.metadata.registrations.properties,
    );
    render_named_block(
        &mut constraints,
        "fairness",
        &bundle.metadata.registrations.fairness,
    );
    render_named_block(
        &mut constraints,
        "state_constraints",
        &bundle.metadata.registrations.state_constraints,
    );
    render_named_block(
        &mut constraints,
        "action_constraints",
        &bundle.metadata.registrations.action_constraints,
    );
    render_named_block(
        &mut constraints,
        "symmetries",
        &bundle.metadata.registrations.symmetries,
    );
    blocks.push(PageBlock::Markdown(constraints));

    PageSection {
        title: "Contracts & Data",
        blocks,
    }
}

fn render_contract_relation_schema(bundle: &nirvash::SpecVizBundle) -> Vec<PageBlock> {
    if bundle.relation_schema.is_empty() {
        return vec![PageBlock::Markdown(
            "### Relation Schema\n\n- none".to_owned(),
        )];
    }

    let set_relations = bundle
        .relation_schema
        .iter()
        .filter(|schema| schema.kind == nirvash::RelationFieldKind::Set)
        .collect::<Vec<_>>();
    let binary_relations = bundle
        .relation_schema
        .iter()
        .filter(|schema| schema.kind == nirvash::RelationFieldKind::Binary)
        .collect::<Vec<_>>();

    let mut blocks = vec![PageBlock::Markdown("### Relation Schema".to_owned())];
    if !binary_relations.is_empty() {
        blocks.push(PageBlock::Mermaid(render_relation_schema_mermaid(
            &binary_relations,
        )));
    }
    let mut details = String::new();
    if !set_relations.is_empty() {
        details.push_str("Set relations:\n\n");
        for schema in set_relations {
            details.push_str(&format!(
                "- `{}`: set of `{}`\n",
                schema.name, schema.from_type
            ));
        }
        details.push('\n');
    }
    if !binary_relations.is_empty() {
        details.push_str("Binary relations:\n\n");
        for schema in binary_relations {
            details.push_str(&format!(
                "- `{}`: `{}` -> `{}`\n",
                schema.name,
                schema.from_type,
                schema.to_type.as_deref().unwrap_or("?")
            ));
        }
    }
    if !details.trim().is_empty() {
        blocks.push(PageBlock::Markdown(details));
    }
    blocks
}

fn spec_link_from_bundle(bundle: &nirvash::SpecVizBundle) -> SpecLink {
    SpecLink {
        label: bundle.spec_name.clone(),
        spec_id: bundle.metadata.spec_id.clone(),
    }
}

fn resolve_spec_link(catalog: &BundleCatalog<'_>, spec_id: &str, label: &str) -> SpecLink {
    catalog
        .bundle(spec_id)
        .map(spec_link_from_bundle)
        .unwrap_or_else(|| SpecLink {
            label: label.to_owned(),
            spec_id: spec_id.to_owned(),
        })
}

fn related_subsystem_links(
    bundle: &nirvash::SpecVizBundle,
    catalog: &BundleCatalog<'_>,
) -> Vec<SpecLink> {
    let mut seen = BTreeSet::new();
    let mut related = Vec::new();
    for parent in catalog.parent_systems(&bundle.metadata.spec_id) {
        for subsystem in &parent.metadata.subsystems {
            if subsystem.spec_id == bundle.metadata.spec_id {
                continue;
            }
            let link = resolve_spec_link(catalog, &subsystem.spec_id, &subsystem.label);
            if seen.insert((link.spec_id.clone(), link.label.clone())) {
                related.push(link);
            }
        }
    }
    related.sort_by(|left, right| left.label.cmp(&right.label));
    related
}

fn collect_bundle_actors(bundle: &nirvash::SpecVizBundle) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut actors = Vec::new();
    for case in &bundle.cases {
        for actor in &case.actors {
            if seen.insert(actor.clone()) {
                actors.push(actor.clone());
            }
        }
    }
    actors
}

fn collect_system_channels(bundle: &nirvash::SpecVizBundle) -> Vec<(String, String, String)> {
    let mut channels = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for case in &bundle.cases {
        for outgoing in &case.graph.edges {
            for edge in outgoing {
                for step in &edge.interaction_steps {
                    if let (Some(from), Some(to)) = (&step.from, &step.to) {
                        channels
                            .entry((from.clone(), to.clone()))
                            .or_default()
                            .insert(step.label.clone());
                    }
                }
            }
        }
    }

    channels
        .into_iter()
        .map(|((from, to), labels)| {
            let mut labels = labels.into_iter().collect::<Vec<_>>();
            labels.sort();
            let preview = labels.into_iter().take(3).collect::<Vec<_>>().join(" / ");
            (from, to, preview)
        })
        .collect()
}

fn render_system_map_mermaid(
    bundle: &nirvash::SpecVizBundle,
    catalog: &BundleCatalog<'_>,
) -> String {
    let mut output = String::from("flowchart LR\n");
    let current_label = match bundle.metadata.kind {
        Some(nirvash::SpecVizKind::System) => {
            format!("{}<br/>system", escape_mermaid_label(&bundle.spec_name))
        }
        Some(nirvash::SpecVizKind::Subsystem) => {
            format!("{}<br/>subsystem", escape_mermaid_label(&bundle.spec_name))
        }
        None => escape_mermaid_label(&bundle.spec_name),
    };
    output.push_str(&format!("CURRENT[\"{current_label}\"]\n"));

    match bundle.metadata.kind {
        Some(nirvash::SpecVizKind::System) => {
            if bundle_has_surface_groups(bundle) {
                let views = bundle_case_groups(bundle)
                    .into_iter()
                    .map(|(surface, _)| surface)
                    .collect::<Vec<_>>();
                let view_aliases = MermaidAliasMap::new(&views, "VIEW");
                for view in &views {
                    output.push_str(&format!(
                        "{}[\"{}<br/>view\"]\n",
                        view_aliases.id(view),
                        escape_mermaid_label(view)
                    ));
                    output.push_str(&format!("CURRENT --> {}\n", view_aliases.id(view)));
                }
            } else {
                let subsystem_labels = bundle
                    .metadata
                    .subsystems
                    .iter()
                    .map(|subsystem| subsystem.label.clone())
                    .collect::<Vec<_>>();
                let subsystem_aliases = MermaidAliasMap::new(&subsystem_labels, "SUB");
                for subsystem in &bundle.metadata.subsystems {
                    output.push_str(&format!(
                        "{}[\"{}<br/>subsystem\"]\n",
                        subsystem_aliases.id(&subsystem.label),
                        escape_mermaid_label(&subsystem.label)
                    ));
                    output.push_str(&format!(
                        "CURRENT --> {}\n",
                        subsystem_aliases.id(&subsystem.label)
                    ));
                }
            }

            let actors = collect_bundle_actors(bundle);
            let actor_aliases = MermaidAliasMap::new(&actors, "ACT");
            for actor in &actors {
                output.push_str(&format!(
                    "{}[\"{}\"]\n",
                    actor_aliases.id(actor),
                    escape_mermaid_label(actor)
                ));
                output.push_str(&format!("CURRENT -.-> {}\n", actor_aliases.id(actor)));
            }
            for (from, to, label) in collect_system_channels(bundle) {
                output.push_str(&format!(
                    "{} -->|{}| {}\n",
                    actor_aliases.id(&from),
                    escape_mermaid_edge_label(&label),
                    actor_aliases.id(&to)
                ));
            }
        }
        Some(nirvash::SpecVizKind::Subsystem) | None => {
            let parents = catalog.parent_systems(&bundle.metadata.spec_id);
            let parent_labels = parents
                .iter()
                .map(|parent| parent.spec_name.clone())
                .collect::<Vec<_>>();
            let parent_aliases = MermaidAliasMap::new(&parent_labels, "SYS");
            for parent in &parents {
                output.push_str(&format!(
                    "{}[\"{}<br/>system\"]\n",
                    parent_aliases.id(&parent.spec_name),
                    escape_mermaid_label(&parent.spec_name)
                ));
                output.push_str(&format!(
                    "{} --> CURRENT\n",
                    parent_aliases.id(&parent.spec_name)
                ));
            }

            let related = related_subsystem_links(bundle, catalog);
            let related_labels = related
                .iter()
                .map(|link| link.label.clone())
                .collect::<Vec<_>>();
            let related_aliases = MermaidAliasMap::new(&related_labels, "REL");
            for link in &related {
                output.push_str(&format!(
                    "{}[\"{}<br/>subsystem\"]\n",
                    related_aliases.id(&link.label),
                    escape_mermaid_label(&link.label)
                ));
            }
            for parent in &parents {
                for link in &related {
                    output.push_str(&format!(
                        "{} --> {}\n",
                        parent_aliases.id(&parent.spec_name),
                        related_aliases.id(&link.label)
                    ));
                }
            }
        }
    }

    output
}

fn remove_stale_generated_outputs(
    dir: &Path,
    extension: &str,
    active_names: &BTreeSet<String>,
) -> Result<(), DynError> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)
        .map_err(|error| err(format!("failed to read {}: {error}", dir.display())))?
    {
        let entry =
            entry.map_err(|error| err(format!("failed to inspect {}: {error}", dir.display())))?;
        let path = entry.path();
        let Some(file_extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_extension != extension {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !active_names.contains(stem) {
            fs::remove_file(&path).map_err(|error| {
                err(format!(
                    "failed to remove stale generated output {}: {error}",
                    path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn ordered_viz_scenarios(scenarios: &[nirvash::VizScenario]) -> Vec<&nirvash::VizScenario> {
    let mut ordered = scenarios.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        scenario_display_rank(left.kind)
            .cmp(&scenario_display_rank(right.kind))
            .then(scenario_max_priority(right).cmp(&scenario_max_priority(left)))
            .then(left.state_path.len().cmp(&right.state_path.len()))
            .then(left.label.cmp(&right.label))
    });
    ordered
}

fn scenario_display_rank(kind: nirvash::VizScenarioKind) -> usize {
    match kind {
        nirvash::VizScenarioKind::HappyPath => 0,
        nirvash::VizScenarioKind::FocusPath => 1,
        nirvash::VizScenarioKind::DeadlockPath => 2,
        nirvash::VizScenarioKind::CycleWitness => 3,
    }
}

fn scenario_atlas_label(kind: nirvash::VizScenarioKind) -> &'static str {
    match kind {
        nirvash::VizScenarioKind::HappyPath => "happy path",
        nirvash::VizScenarioKind::FocusPath => "focus path",
        nirvash::VizScenarioKind::DeadlockPath => "failure witness",
        nirvash::VizScenarioKind::CycleWitness => "cycle witness",
    }
}

fn scenario_max_priority(scenario: &nirvash::VizScenario) -> i32 {
    scenario
        .steps
        .iter()
        .filter_map(|step| step.scenario_priority)
        .max()
        .unwrap_or_default()
}

fn scenario_path_label(path: &[usize]) -> String {
    path.iter()
        .map(|state| format!("S{state}"))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn collect_actor_flow_actors(
    case: &nirvash::SpecVizCase,
    scenarios: &[&nirvash::VizScenario],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut actors = Vec::new();
    for scenario in scenarios {
        for step in &scenario.steps {
            for process_step in &step.process_steps {
                let actor = process_step.actor.as_deref().unwrap_or("Spec").to_owned();
                if seen.insert(actor.clone()) {
                    actors.push(actor);
                }
            }
        }
    }
    if actors.is_empty() {
        if case.actors.is_empty() {
            actors.push("Spec".to_owned());
        } else {
            actors.extend(case.actors.clone());
        }
    }
    actors
}

fn render_actor_flow_mermaid(actor: &str, scenarios: &[&nirvash::VizScenario]) -> String {
    let mut output = String::from("flowchart TD\n");
    for (scenario_index, scenario) in scenarios.iter().enumerate() {
        let steps = scenario
            .steps
            .iter()
            .flat_map(|step| {
                step.process_steps.iter().filter_map(|process_step| {
                    let owner = process_step.actor.as_deref().unwrap_or("Spec");
                    (owner == actor).then(|| render_process_step(process_step))
                })
            })
            .collect::<Vec<_>>();
        let subgraph_id = format!("SC{}", scenario_index + 1);
        output.push_str(&format!(
            "subgraph {subgraph_id}[\"{}\"]\n",
            escape_mermaid_label(&scenario.label)
        ));
        if steps.is_empty() {
            output.push_str(&format!(
                "    {}_EMPTY[\"no actor-specific steps\"]\n",
                subgraph_id
            ));
        } else {
            let mut previous = None::<String>;
            for (step_index, step) in steps.iter().enumerate() {
                let node_id = format!("{}_{}", subgraph_id, step_index + 1);
                output.push_str(&format!(
                    "    {node_id}[\"{}\"]\n",
                    escape_mermaid_label(step)
                ));
                if let Some(previous) = &previous {
                    output.push_str(&format!("    {previous} --> {node_id}\n"));
                }
                previous = Some(node_id);
            }
        }
        output.push_str("end\n");
    }
    output
}

fn render_state_legend(graph: &nirvash::ReducedDocGraph) -> String {
    let mut output = String::new();
    for state in &graph.states {
        output.push_str(&format!(
            "#### S{}\n\n```text\n{}\n```\n\n",
            state.original_index, state.state.full
        ));
    }
    output
}

fn render_scenario_state_space_mermaid(
    case: &nirvash::SpecVizCase,
    scenario: &nirvash::VizScenario,
) -> String {
    let mut output = String::from("flowchart LR\n");
    for state_index in &scenario.state_path {
        let state = &case.graph.states[*state_index];
        let label = compact_state_lines(&state.full, &state.summary, &state.relation_fields)
            .into_iter()
            .next()
            .unwrap_or_else(|| state.summary.clone());
        output.push_str(&format!(
            "S{state_index}[\"S{state_index}<br/>{}\"]\n",
            escape_mermaid_label(&label)
        ));
    }
    for step in &scenario.steps {
        output.push_str(&format!(
            "S{} -->|{}| S{}\n",
            step.source,
            escape_mermaid_edge_label(step.compact_label.as_deref().unwrap_or(&step.label)),
            step.target
        ));
    }
    output
}

fn render_model_backend(backend: nirvash::ModelBackend) -> &'static str {
    match backend {
        nirvash::ModelBackend::Explicit => "explicit",
        nirvash::ModelBackend::Symbolic => "symbolic",
    }
}

fn render_named_block(output: &mut String, title: &str, values: &[String]) {
    output.push_str(&format!("{title}:\n"));
    if values.is_empty() {
        output.push_str("  - none\n\n");
        return;
    }
    for value in values {
        output.push_str(&format!("  - {value}\n"));
    }
    output.push('\n');
}

fn render_viz_state_graph_mermaid(
    bundle: &nirvash::SpecVizBundle,
    case: &nirvash::SpecVizCase,
    graph: &nirvash::ReducedDocGraph,
    visible_edges: &[&nirvash::ReducedDocGraphEdge],
) -> String {
    let mut output = String::from("stateDiagram-v2\n");
    output.push_str(&format!(
        "%% reachable state graph for {}::{}\n",
        bundle.spec_name, case.label
    ));

    for state in &graph.states {
        let label = render_state_node_label(graph, state);
        output.push_str(&format!(
            "state \"{}\" as S{}\n",
            label, state.original_index
        ));
    }

    let deadlocks = graph
        .states
        .iter()
        .filter(|state| state.is_deadlock)
        .map(|state| format!("S{}", state.original_index))
        .collect::<Vec<_>>();
    if !deadlocks.is_empty() {
        output.push_str(
            "classDef deadlock fill:#fee2e2,stroke:#b91c1c,stroke-width:3px,color:#7f1d1d;\n",
        );
        output.push_str(&format!("class {} deadlock\n", deadlocks.join(",")));
    }

    for state in &graph.states {
        if state.is_initial {
            output.push_str(&format!("[*] --> S{}\n", state.original_index));
        }
    }

    for edge in visible_edges {
        output.push_str(&format!(
            "S{} --> S{}: {}\n",
            edge.source,
            edge.target,
            state_diagram_edge_label(&edge.label)
        ));
    }

    output
}

fn render_viz_sequence_diagram_mermaid(
    bundle: &nirvash::SpecVizBundle,
    case: &nirvash::SpecVizCase,
    scenario: &nirvash::VizScenario,
) -> String {
    let actors = if scenario.actors.is_empty() {
        vec!["Spec".to_owned()]
    } else {
        scenario.actors.clone()
    };
    let mut output = String::from("sequenceDiagram\n");
    output.push_str(&format!(
        "%% selected trace for {}::{}::{}\n",
        bundle.spec_name, case.label, scenario.label
    ));
    let aliases = MermaidAliasMap::new(&actors, "SEQ");
    render_viz_sequence_participants(&mut output, &aliases);
    if let Some(initial) = scenario.state_path.first() {
        output.push_str(&format!(
            "Note over {}: {}\n",
            aliases.note_scope(),
            mermaid_sequence_text(&format!("initial S{initial}"))
        ));
    }

    for step in &scenario.steps {
        if step.interaction_steps.len() > 1 {
            for (index, interaction) in step.interaction_steps.iter().enumerate() {
                let keyword = if index == 0 { "par" } else { "and" };
                output.push_str(&format!(
                    "{}{} {}\n",
                    sequence_indent(0),
                    keyword,
                    mermaid_sequence_text(&interaction_step_branch_label(interaction))
                ));
                render_viz_sequence_messages(
                    &mut output,
                    &aliases,
                    std::slice::from_ref(interaction),
                    1,
                );
            }
            output.push_str("end\n");
        } else if !step.interaction_steps.is_empty() {
            render_viz_sequence_messages(&mut output, &aliases, &step.interaction_steps, 0);
        } else {
            output.push_str(&format!(
                "Note over {}: {}\n",
                aliases.note_scope(),
                mermaid_sequence_text(step.compact_label.as_deref().unwrap_or(&step.label))
            ));
        }

        output.push_str(&format!(
            "Note over {}: {}\n",
            aliases.note_scope(),
            mermaid_sequence_text(&format!("S{} -> S{}", step.source, step.target))
        ));
    }

    if let Some(last) = scenario.state_path.last() {
        output.push_str(&format!(
            "Note over {}: {}\n",
            aliases.note_scope(),
            mermaid_sequence_text(&format!("reach S{last}"))
        ));
    }
    output
}

fn render_viz_sequence_participants(output: &mut String, aliases: &MermaidAliasMap) {
    for (label, id) in &aliases.ordered {
        output.push_str(&format!(
            "participant {id} as {}\n",
            mermaid_edge_label(label)
        ));
    }
}

fn render_viz_sequence_messages(
    output: &mut String,
    aliases: &MermaidAliasMap,
    steps: &[nirvash::DocGraphInteractionStep],
    indent: usize,
) {
    for step in steps {
        match (&step.from, &step.to) {
            (Some(from), Some(to)) => output.push_str(&format!(
                "{}{}->>{}: {}\n",
                sequence_indent(indent),
                aliases.id(from),
                aliases.id(to),
                mermaid_sequence_text(&step.label)
            )),
            (Some(actor), None) | (None, Some(actor)) => output.push_str(&format!(
                "{}Note over {}: {}\n",
                sequence_indent(indent),
                aliases.id(actor),
                mermaid_sequence_text(&step.label)
            )),
            (None, None) => output.push_str(&format!(
                "{}Note over {}: {}\n",
                sequence_indent(indent),
                aliases.note_scope(),
                mermaid_sequence_text(&step.label)
            )),
        }
    }
}

fn render_viz_step_table(scenario: &nirvash::VizScenario) -> String {
    let mut output = String::from("| # | transition | action |\n| --- | --- | --- |\n");
    for (index, step) in scenario.steps.iter().enumerate() {
        output.push_str(&format!(
            "| {} | `S{} -> S{}` | `{}` |\n",
            index + 1,
            step.source,
            step.target,
            step.compact_label.as_deref().unwrap_or(&step.label)
        ));
    }
    output
}

fn render_case_process_view(case: &nirvash::SpecVizCase) -> String {
    let mut output = String::new();
    output.push_str(&format!("case {}:\n", case.label));
    if !case.loop_groups.is_empty() {
        output.push_str("loop blocks:\n");
        for (index, group) in case.loop_groups.iter().enumerate() {
            output.push_str(&format!(
                "  loop#{} = {}\n",
                index + 1,
                group
                    .iter()
                    .map(|state| format!("S{state}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        output.push('\n');
    }

    let actors = if case.actors.len() >= 2 {
        case.actors.clone()
    } else {
        vec!["Spec".to_owned()]
    };
    for actor in actors {
        let actor_name = actor.as_str();
        output.push_str(&format!("process {actor_name}:\n"));
        for scenario in &case.scenarios {
            let steps = scenario
                .steps
                .iter()
                .flat_map(|step| {
                    step.process_steps.iter().filter(move |process_step| {
                        actor_name == "Spec"
                            || process_step
                                .actor
                                .as_deref()
                                .is_none_or(|name| name == actor_name)
                    })
                })
                .collect::<Vec<_>>();
            if steps.is_empty() {
                continue;
            }
            output.push_str(&format!("  scenario {}:\n", scenario.label));
            for step in steps {
                output.push_str(&format!("    {}\n", render_process_step(step)));
            }
        }
        output.push('\n');
    }
    output
}

fn interaction_step_branch_label(step: &nirvash::DocGraphInteractionStep) -> String {
    match (&step.from, &step.to) {
        (Some(from), Some(to)) => {
            format!(
                "{} -> {}: {}",
                to_lower_snake(from),
                to_lower_snake(to),
                step.label
            )
        }
        (Some(from), None) => format!("{}: {}", to_lower_snake(from), step.label),
        (None, Some(to)) => format!("{}: {}", to_lower_snake(to), step.label),
        (None, None) => step.label.clone(),
    }
}

fn sequence_indent(level: usize) -> String {
    "    ".repeat(level)
}

fn visible_reduced_edges(graph: &nirvash::ReducedDocGraph) -> Vec<&nirvash::ReducedDocGraphEdge> {
    let non_self_outgoing = graph
        .edges
        .iter()
        .filter(|edge| edge.source != edge.target)
        .map(|edge| edge.source)
        .collect::<BTreeSet<_>>();

    graph
        .edges
        .iter()
        .filter(|edge| edge.source != edge.target || !non_self_outgoing.contains(&edge.source))
        .collect()
}

fn render_state_node_label(
    graph: &nirvash::ReducedDocGraph,
    state: &nirvash::ReducedDocGraphNode,
) -> String {
    let mut parts = Vec::new();
    if state.is_deadlock {
        parts.push("DEADLOCK".to_string());
    }
    parts.extend(state_display_lines(graph, state));

    mermaid_state_label(&parts.join("\n"))
}

fn state_display_lines(
    graph: &nirvash::ReducedDocGraph,
    state: &nirvash::ReducedDocGraphNode,
) -> Vec<String> {
    if let Some(predecessor) = preferred_predecessor(graph, state.original_index)
        && let Some(previous_state) = graph
            .states
            .iter()
            .find(|node| node.original_index == predecessor)
        && let Some(delta) = state_delta_lines(&previous_state.state.full, &state.state.full)
    {
        return delta;
    }

    compact_state_lines(
        &state.state.full,
        &state.state.summary,
        &state.state.relation_fields,
    )
}

fn render_process_step(step: &nirvash::DocGraphProcessStep) -> String {
    let verb = match step.kind {
        nirvash::DocGraphProcessKind::Do => "do",
        nirvash::DocGraphProcessKind::Send => "send",
        nirvash::DocGraphProcessKind::Receive => "receive",
        nirvash::DocGraphProcessKind::Wait => "wait",
        nirvash::DocGraphProcessKind::Emit => "emit",
    };
    format!("{verb} {}", step.label)
}

fn render_code_block(language: &str, body: &str) -> String {
    format!("```{language}\n{body}\n```")
}

fn preferred_predecessor(graph: &nirvash::ReducedDocGraph, target: usize) -> Option<usize> {
    let mut candidates = graph
        .edges
        .iter()
        .filter(|edge| edge.source != target && edge.target == target)
        .map(|edge| edge.source)
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.into_iter().next()
}

fn mermaid_state_label(input: &str) -> String {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(escape_mermaid_label)
        .collect::<Vec<_>>()
        .join("<br/>")
}

fn state_delta_lines(previous: &str, current: &str) -> Option<Vec<String>> {
    const MAX_NODE_DETAIL_LINES: usize = 2;

    let previous_lines = normalized_debug_lines(previous);
    let current_lines = normalized_debug_lines(current);
    let changed = current_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            (previous_lines.get(index) != Some(line))
                .then(|| simplify_state_line(line))
                .filter(|line| !line.is_empty())
        })
        .take(MAX_NODE_DETAIL_LINES)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        None
    } else {
        Some(changed)
    }
}

fn normalized_debug_lines(input: &str) -> Vec<String> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_structural_debug_line(line))
        .map(ToOwned::to_owned)
        .collect()
}

fn is_structural_debug_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed
        .chars()
        .all(|character| matches!(character, '{' | '}' | '[' | ']' | '(' | ')' | ','))
        || ((trimmed.ends_with('{') || trimmed.ends_with('[') || trimmed.ends_with('('))
            && !trimmed.contains(':'))
}

fn compact_state_lines(
    full: &str,
    summary: &str,
    relation_fields: &[nirvash::RelationFieldSummary],
) -> Vec<String> {
    const MAX_NODE_DETAIL_LINES: usize = 2;

    let relation_lines = relation_fields
        .iter()
        .map(|field| simplify_state_line(&field.notation))
        .filter(|line| !line.is_empty())
        .take(MAX_NODE_DETAIL_LINES)
        .collect::<Vec<_>>();
    if !relation_lines.is_empty() {
        return relation_lines;
    }

    let from_full = normalized_debug_lines(full)
        .into_iter()
        .map(|line| simplify_state_line(&line))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let filtered_from_full = from_full
        .iter()
        .filter(|line| !is_low_signal_state_line(line))
        .take(MAX_NODE_DETAIL_LINES)
        .cloned()
        .collect::<Vec<_>>();
    if !filtered_from_full.is_empty() {
        return filtered_from_full;
    }
    if !from_full.is_empty() {
        return from_full.into_iter().take(MAX_NODE_DETAIL_LINES).collect();
    }

    let from_summary = summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(simplify_state_line)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let filtered_from_summary = from_summary
        .iter()
        .filter(|line| !is_low_signal_state_line(line))
        .take(MAX_NODE_DETAIL_LINES)
        .cloned()
        .collect::<Vec<_>>();
    if !filtered_from_summary.is_empty() {
        return filtered_from_summary;
    }

    from_summary
        .into_iter()
        .take(MAX_NODE_DETAIL_LINES)
        .collect()
}

fn render_relation_schema_mermaid(schemas: &[&nirvash::RelationFieldSchema]) -> String {
    let mut type_ids = BTreeMap::new();
    let mut output = String::from("erDiagram\n");
    for schema in schemas {
        type_ids
            .entry(schema.from_type.clone())
            .or_insert_with(|| mermaid_entity_id(&schema.from_type));
        if let Some(to_type) = &schema.to_type {
            type_ids
                .entry(to_type.clone())
                .or_insert_with(|| mermaid_entity_id(to_type));
        }
    }
    for (type_name, entity_id) in &type_ids {
        output.push_str(&format!(
            "    {entity_id} {{\n        string atom \"{}\"\n    }}\n",
            escape_mermaid_label(type_name)
        ));
    }
    for schema in schemas {
        let from = type_ids
            .get(&schema.from_type)
            .expect("from type id exists");
        let to = type_ids
            .get(
                schema
                    .to_type
                    .as_ref()
                    .expect("binary relation target type exists"),
            )
            .expect("to type id exists");
        output.push_str(&format!(
            "    {from} }}o--o{{ {to} : \"{}\"\n",
            escape_mermaid_edge_label(&schema.name)
        ));
    }
    output
}

fn mermaid_entity_id(type_name: &str) -> String {
    let mut id = String::new();
    for character in type_name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_uppercase());
        } else {
            id.push('_');
        }
    }
    if id.is_empty() {
        "RELATION_ENTITY".to_owned()
    } else {
        id
    }
}

fn simplify_state_line(line: &str) -> String {
    const MAX_LINE_CHARS: usize = 32;

    let trimmed = line.trim().trim_end_matches(',');
    if trimmed.is_empty() {
        return String::new();
    }

    if let Some((field, value)) = trimmed.split_once(':') {
        let value = shorten_token(value.trim(), MAX_LINE_CHARS.saturating_sub(field.len() + 2));
        return format!("{}: {}", field.trim(), value);
    }

    shorten_token(trimmed, MAX_LINE_CHARS)
}

fn is_low_signal_state_line(line: &str) -> bool {
    let Some((field, value)) = line.split_once(':') else {
        return false;
    };

    let field = field.trim();
    let value = value.trim();
    matches!(field, "unchanged" | "updated_at" | "updated_at_unix_secs")
        || matches!(value, "false" | "None" | "\"\"" | "[]" | "{}" | "0")
}

fn shorten_token(input: &str, max_chars: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return normalized;
    }

    if let Some(index) = normalized.find('{') {
        return format!("{}{{...}}", normalized[..index].trim_end());
    }
    if let Some(index) = normalized.find('(') {
        return format!("{}(...)", normalized[..index].trim_end());
    }
    if let Some(index) = normalized.find('[') {
        return format!("{}[...]", normalized[..index].trim_end());
    }

    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let shortened = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{shortened}...")
}

fn render_mermaid_block(diagram: &str) -> String {
    format!(
        "<pre class=\"mermaid nirvash-mermaid\">{}</pre>",
        escape_html(diagram)
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_mermaid_label(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn mermaid_sequence_text(input: &str) -> String {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("<br/>")
}

fn mermaid_edge_label(input: &str) -> String {
    format!("\"{}\"", escape_mermaid_edge_label(input))
}

fn state_diagram_edge_label(input: &str) -> String {
    sanitize_state_diagram_edge_label(input)
}

fn escape_mermaid_edge_label(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sanitize_state_diagram_edge_label(input: &str) -> String {
    input
        .replace("<br/>", " / ")
        .replace("->", "→")
        .replace(':', " -")
        .replace('"', "'")
        .replace('\n', " / ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn demo_edge(label: &str, target: usize) -> nirvash::DocGraphEdge {
        nirvash::DocGraphEdge {
            label: label.to_owned(),
            compact_label: None,
            scenario_priority: None,
            interaction_steps: Vec::new(),
            process_steps: vec![nirvash::DocGraphProcessStep::new(
                nirvash::DocGraphProcessKind::Do,
                label,
            )],
            target,
        }
    }

    fn demo_graph_case(label: &str) -> nirvash::DocGraphCase {
        nirvash::DocGraphCase {
            label: label.to_owned(),
            surface: None,
            projection: None,
            backend: nirvash::ModelBackend::Explicit,
            trust_tier: nirvash::TrustTier::Exact,
            graph: nirvash::DocGraphSnapshot {
                states: vec![nirvash::DocGraphState {
                    summary: format!("{label}State"),
                    full: format!("{label}State"),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                }],
                edges: vec![Vec::new()],
                initial_indices: vec![0],
                deadlocks: vec![0],
                truncated: false,
                stutter_omitted: false,
                focus_indices: Vec::new(),
                reduction: nirvash::DocGraphReductionMode::BoundaryPaths,
                max_edge_actions_in_label: 2,
            },
        }
    }

    fn demo_bundle(
        spec_name: &str,
        spec_id: &str,
        kind: Option<nirvash::SpecVizKind>,
        cases: Vec<nirvash::DocGraphCase>,
    ) -> nirvash::SpecVizBundle {
        nirvash::SpecVizBundle::from_doc_graph_spec(
            spec_name,
            nirvash::SpecVizMetadata {
                spec_id: spec_id.to_owned(),
                kind,
                state_ty: format!("{spec_name}State"),
                action_ty: format!("{spec_name}Action"),
                model_cases: Some(format!("{spec_name}_model_cases")),
                subsystems: Vec::new(),
                registrations: nirvash::SpecVizRegistrationSet::default(),
                policy: nirvash::VizPolicy::default(),
            },
            cases,
        )
    }

    #[test]
    fn render_runner_main_uses_parallel_bundle_workers() {
        let main = render_runner_main(&[
            vec![
                "crate".to_owned(),
                "system".to_owned(),
                "SystemSpec".to_owned(),
            ],
            vec![
                "crate".to_owned(),
                "manager".to_owned(),
                "ManagerSpec".to_owned(),
            ],
        ]);

        assert!(main.contains("collect_primary_spec_viz_provider_registrations"));
        assert!(main.contains("thread::available_parallelism"));
        assert!(main.contains("NIRVASH_DOCGEN_JOBS"));
        assert!(main.contains("NIRVASH_DOCGEN_PROGRESS"));
        assert!(main.contains("thread::spawn"));
        assert!(main.contains("VecDeque::from(registrations)"));
        assert!(main.contains("nirvash-docgen spec progress"));
        assert!(main.contains("serde_json::to_writer"));
        assert!(!main.contains("collect_spec_viz_bundles()"));
    }

    #[test]
    fn read_runtime_graph_bundles_merges_ndjson_stream() {
        let dir = tempdir().expect("tempdir");
        let output_path = dir.path().join("bundles.ndjson");
        let duplicate_stub = nirvash::SpecVizBundle {
            spec_name: "AlphaSpec".to_owned(),
            metadata: nirvash::SpecVizMetadata {
                spec_id: String::new(),
                kind: None,
                state_ty: "AlphaState".to_owned(),
                action_ty: "AlphaAction".to_owned(),
                model_cases: None,
                subsystems: Vec::new(),
                registrations: nirvash::SpecVizRegistrationSet::default(),
                policy: nirvash::VizPolicy::default(),
            },
            action_vocabulary: Vec::new(),
            relation_schema: Vec::new(),
            cases: Vec::new(),
        };
        let duplicate_full = demo_bundle(
            "AlphaSpec",
            "crate::alpha::AlphaSpec",
            Some(nirvash::SpecVizKind::Subsystem),
            vec![demo_graph_case("alpha")],
        );
        let beta = demo_bundle(
            "BetaSpec",
            "crate::beta::BetaSpec",
            Some(nirvash::SpecVizKind::System),
            vec![demo_graph_case("beta")],
        );

        let mut file = File::create(&output_path).expect("output file");
        for bundle in [&beta, &duplicate_stub, &duplicate_full] {
            writeln!(
                file,
                "{}",
                serde_json::to_string(bundle).expect("bundle json")
            )
            .expect("write ndjson");
        }
        drop(file);

        let bundles = read_runtime_graph_bundles(&output_path, Path::new("runner/Cargo.toml"))
            .expect("parse ndjson");
        let mut expected_map = BTreeMap::new();
        for bundle in [beta.clone(), duplicate_stub, duplicate_full] {
            nirvash::upsert_spec_viz_bundle(&mut expected_map, bundle);
        }
        let mut expected = expected_map.into_values().collect::<Vec<_>>();
        expected.sort_by(|left, right| left.spec_name.cmp(&right.spec_name));

        assert_eq!(bundles, expected);
    }

    #[test]
    fn generate_collects_supported_module_tree_and_renders_mermaid() {
        let dir = tempdir().expect("tempdir");
        let manifest_dir = dir.path();
        let src_dir = manifest_dir.join("src");
        let out_dir = manifest_dir.join("out");
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_path_buf();
        fs::create_dir_all(&src_dir).expect("src");
        fs::write(
            manifest_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"demo-doc-target\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[dependencies]\nnirvash = {{ path = \"{}\" }}\nnirvash-lower = {{ path = \"{}\" }}\nnirvash-macros = {{ path = \"{}\" }}\n",
                workspace_root.join("crates/nirvash").display(),
                workspace_root.join("crates/nirvash-lower").display(),
                workspace_root.join("crates/nirvash-macros").display(),
            ),
        )
        .expect("Cargo.toml");

        fs::write(
            src_dir.join("lib.rs"),
            r#"
pub mod child;
pub mod system;

pub mod inline_parent {
    use nirvash::{BoolExpr, Ltl, TransitionProgram};
    use nirvash_lower::FrontendSpec;
    use nirvash_macros::{invariant, nirvash_expr, nirvash_transition_program, property, subsystem_spec};

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InlineState;
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InlineAction;
    pub struct InlineSpec;

    #[subsystem_spec(model_cases(inline_model_cases))]
    impl FrontendSpec for InlineSpec {
        type State = InlineState;
        type Action = InlineAction;

        fn initial_states(&self) -> Vec<Self::State> { vec![InlineState] }
        fn actions(&self) -> Vec<Self::Action> { vec![InlineAction] }
        fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
            Some(nirvash_transition_program! {
                rule inline_transition when true => {
                    set self <= InlineState;
                }
            })
        }
    }

    nirvash::invariant!(self::InlineSpec, inline_invariant(state) => {
        let _ = state;
        true
    });

    mod nested {
        use super::{InlineAction, InlineState};
        use nirvash::{BoolExpr, Ltl};
        use nirvash_macros::{invariant, nirvash_expr, property};

        #[invariant(super::InlineSpec)]
        fn super_invariant() -> BoolExpr<InlineState> {
            nirvash_expr! { super_invariant(_state) => true }
        }

        #[property(crate::inline_parent::InlineSpec)]
        fn crate_property() -> Ltl<InlineState, InlineAction> {
            Ltl::pred(nirvash_expr! { crate_property_state(_state) => true })
        }
    }

    fn inline_model_cases() -> Vec<nirvash_lower::ModelInstance<InlineState, InlineAction>> {
        Vec::new()
    }
}
"#,
        )
        .expect("lib.rs");

        fs::write(
            src_dir.join("child.rs"),
            r#"
use nirvash::{BoolExpr, Fairness, Ltl, StepExpr, TransitionProgram};
use nirvash_lower::FrontendSpec;
use nirvash_macros::{invariant, nirvash_expr, nirvash_transition_program, property, subsystem_spec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildState;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildAction;
pub struct ChildSpec;

#[subsystem_spec]
impl FrontendSpec for ChildSpec {
    type State = ChildState;
    type Action = ChildAction;

    fn initial_states(&self) -> Vec<Self::State> { vec![ChildState] }
    fn actions(&self) -> Vec<Self::Action> { vec![ChildAction] }
    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule child_transition when true => {
                set self <= ChildState;
            }
        })
    }
}

nirvash::invariant!(ChildSpec, child_invariant(state) => {
    let _ = state;
    true
});

nirvash::state_constraint!(ChildSpec, child_state_constraint(state) => {
    let _ = state;
    true
});

nirvash::action_constraint!(ChildSpec, child_action_constraint(prev, action, next) => {
    let _ = (prev, action, next);
    true
});

#[property(ChildSpec)]
fn child_property() -> Ltl<ChildState, ChildAction> {
    Ltl::leads_to(
        Ltl::pred(nirvash_expr! { child_busy(_state) => true }),
        Ltl::pred(nirvash_expr! { child_idle(_state) => true }),
    )
}

nirvash::fairness!(weak ChildSpec, child_fairness(prev, action, next) => {
    let _ = (prev, action, next);
    true
});
"#,
        )
        .expect("child.rs");

        fs::write(
            src_dir.join("system.rs"),
            r#"
use nirvash::{BoolExpr, Ltl, TransitionProgram};
use nirvash_lower::FrontendSpec;
use nirvash_macros::{invariant, nirvash_expr, nirvash_transition_program, property, system_spec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemState;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemAction;
pub struct RootSystemSpec;

#[system_spec(
    subsystems(crate::child::ChildSpec, crate::inline_parent::InlineSpec),
    model_cases(system_model_cases)
)]
impl FrontendSpec for RootSystemSpec {
    type State = SystemState;
    type Action = SystemAction;

    fn initial_states(&self) -> Vec<Self::State> { vec![SystemState] }
    fn actions(&self) -> Vec<Self::Action> { vec![SystemAction] }
    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule root_system_transition when true => {
                set self <= SystemState;
            }
        })
    }
}

#[invariant(RootSystemSpec)]
fn system_invariant() -> BoolExpr<SystemState> {
    nirvash_expr! { system_invariant(_state) => true }
}

#[property(RootSystemSpec)]
fn system_property() -> Ltl<SystemState, SystemAction> {
    Ltl::pred(nirvash_expr! { system_property_state(_state) => true })
}

fn system_model_cases() -> Vec<nirvash_lower::ModelInstance<SystemState, SystemAction>> {
    Vec::new()
}
"#,
        )
        .expect("system.rs");

        let output = generate_at(manifest_dir, &out_dir).expect("docgen succeeds");
        let env_keys = output
            .fragments
            .iter()
            .map(|fragment| fragment.env_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            env_keys,
            vec![
                "NIRVASH_DOC_FRAGMENT_CHILD_SPEC",
                "NIRVASH_DOC_FRAGMENT_INLINE_SPEC",
                "NIRVASH_DOC_FRAGMENT_ROOT_SYSTEM_SPEC",
            ]
        );

        let inline_fragment = output
            .fragments
            .iter()
            .find(|fragment| fragment.env_key == "NIRVASH_DOC_FRAGMENT_INLINE_SPEC")
            .expect("inline fragment");
        let inline_doc = fs::read_to_string(&inline_fragment.path).expect("inline doc");
        assert!(inline_doc.contains("## System Map"));
        assert!(inline_doc.contains("## Contracts & Data"));
        assert!(inline_doc.contains("InlineSpec"));
        assert!(inline_doc.contains("inline_invariant"));
        assert!(inline_doc.contains("super_invariant"));
        assert!(inline_doc.contains("crate_property"));
        assert!(inline_doc.contains("| model cases | `inline_model_cases` |"));
        assert!(inline_doc.contains("nirvash mermaid runtime failed to initialize"));
        assert!(inline_doc.contains("runtime.textContent = "));
        assert!(!inline_doc.contains("mermaid.min.js"));
        assert!(!inline_doc.contains("type=\"module\""));

        let child_fragment = output
            .fragments
            .iter()
            .find(|fragment| fragment.env_key == "NIRVASH_DOC_FRAGMENT_CHILD_SPEC")
            .expect("child fragment");
        let child_doc = fs::read_to_string(&child_fragment.path).expect("child doc");
        assert!(child_doc.contains("child_invariant"));
        assert!(child_doc.contains("child_property"));
        assert!(child_doc.contains("child_fairness"));
        assert!(child_doc.contains("child_state_constraint"));
        assert!(child_doc.contains("child_action_constraint"));

        let system_fragment = output
            .fragments
            .iter()
            .find(|fragment| fragment.env_key == "NIRVASH_DOC_FRAGMENT_ROOT_SYSTEM_SPEC")
            .expect("system fragment");
        let system_doc = fs::read_to_string(&system_fragment.path).expect("system doc");
        assert!(system_doc.contains("RootSystemSpec"));
        assert!(system_doc.contains("## System Map"));
        assert!(system_doc.contains("## Scenario Atlas"));
        assert!(system_doc.contains("[`ChildSpec`](crate::child::ChildSpec)"));
        assert!(system_doc.contains("[`InlineSpec`](crate::inline_parent::InlineSpec)"));
        assert!(system_doc.contains("system_invariant"));

        assert_eq!(
            output.rerun_if_changed,
            vec![
                manifest_dir.join("Cargo.toml"),
                manifest_dir.join("build.rs"),
                manifest_dir.join("src"),
                src_dir.join("child.rs"),
                src_dir.join("lib.rs"),
                src_dir.join("system.rs"),
            ]
        );

        let runner_target_dir = out_dir.join("nirvash-doc-runner-target");
        assert!(runner_target_dir.exists());

        let second_output =
            generate_at(manifest_dir, &out_dir).expect("docgen succeeds with reused runner target");
        let second_env_keys = second_output
            .fragments
            .iter()
            .map(|fragment| fragment.env_key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(second_env_keys, env_keys);
        assert!(runner_target_dir.exists());

        let second_inline_fragment = second_output
            .fragments
            .iter()
            .find(|fragment| fragment.env_key == "NIRVASH_DOC_FRAGMENT_INLINE_SPEC")
            .expect("second inline fragment");
        let second_inline_doc =
            fs::read_to_string(&second_inline_fragment.path).expect("second inline doc");
        assert_eq!(second_inline_doc, inline_doc);
    }

    #[test]
    fn generate_rejects_duplicate_spec_tail_ident() {
        let dir = tempdir().expect("tempdir");
        let manifest_dir = dir.path();
        let src_dir = manifest_dir.join("src");
        fs::create_dir_all(&src_dir).expect("src");

        fs::write(src_dir.join("lib.rs"), "pub mod left;\npub mod right;\n").expect("lib.rs");

        for module in ["left", "right"] {
            fs::write(
                src_dir.join(format!("{module}.rs")),
                r#"
use nirvash::TransitionProgram;
use nirvash_lower::FrontendSpec;
use nirvash_macros::{nirvash_transition_program, subsystem_spec};

pub struct State;
pub struct Action;
pub struct DuplicateSpec;

#[subsystem_spec]
impl FrontendSpec for DuplicateSpec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> { vec![State] }
    fn actions(&self) -> Vec<Self::Action> { vec![Action] }
    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule duplicate_transition when true => {
                set self <= State;
            }
        })
    }
}
"#,
            )
            .expect("module");
        }

        let error = generate_at(manifest_dir, &manifest_dir.join("out"))
            .expect_err("duplicate spec tail idents must fail");
        assert!(
            error
                .to_string()
                .contains("duplicate spec tail ident `DuplicateSpec`")
        );
    }

    #[test]
    fn generate_removes_stale_plane_outputs_when_system_spec_replaces_them() {
        let dir = tempdir().expect("tempdir");
        let manifest_dir = dir.path();
        let src_dir = manifest_dir.join("src");
        let out_dir = manifest_dir.join("out");
        let doc_dir = out_dir.join("nirvash-doc");
        let viz_dir = out_dir.join("viz");
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_path_buf();

        fs::create_dir_all(&src_dir).expect("src");
        fs::create_dir_all(&doc_dir).expect("doc dir");
        fs::create_dir_all(&viz_dir).expect("viz dir");
        fs::write(doc_dir.join("ManagerPlaneSpec.md"), "stale manager").expect("stale manager doc");
        fs::write(doc_dir.join("ControlPlaneSpec.md"), "stale control").expect("stale control doc");
        fs::write(viz_dir.join("ManagerPlaneSpec.json"), "{}").expect("stale manager viz");
        fs::write(viz_dir.join("ControlPlaneSpec.json"), "{}").expect("stale control viz");

        fs::write(
            manifest_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"demo-doc-cleanup\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[build-dependencies]\nnirvash-docgen = {{ path = \"{}\" }}\n\n[dependencies]\nnirvash = {{ path = \"{}\" }}\nnirvash-lower = {{ path = \"{}\" }}\nnirvash-macros = {{ path = \"{}\" }}\n",
                workspace_root.join("crates/nirvash-docgen").display(),
                workspace_root.join("crates/nirvash").display(),
                workspace_root.join("crates/nirvash-lower").display(),
                workspace_root.join("crates/nirvash-macros").display(),
            ),
        )
        .expect("Cargo.toml");
        fs::write(
            src_dir.join("lib.rs"),
            r#"
use nirvash::TransitionProgram;
use nirvash_lower::FrontendSpec;
use nirvash_macros::{nirvash_transition_program, system_spec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemState;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemAction;
pub struct SystemSpec;

#[system_spec]
impl FrontendSpec for SystemSpec {
    type State = SystemState;
    type Action = SystemAction;

    fn initial_states(&self) -> Vec<Self::State> { vec![SystemState] }
    fn actions(&self) -> Vec<Self::Action> { vec![SystemAction] }
    fn transition_program(&self) -> Option<TransitionProgram<Self::State, Self::Action>> {
        Some(nirvash_transition_program! {
            rule step when true => {
                set self <= SystemState;
            }
        })
    }
}
"#,
        )
        .expect("lib.rs");

        let output = generate_at(manifest_dir, &out_dir).expect("docgen succeeds");

        assert!(!doc_dir.join("ManagerPlaneSpec.md").exists());
        assert!(!doc_dir.join("ControlPlaneSpec.md").exists());
        assert!(!viz_dir.join("ManagerPlaneSpec.json").exists());
        assert!(!viz_dir.join("ControlPlaneSpec.json").exists());
        assert!(doc_dir.join("SystemSpec.md").exists());
        assert!(viz_dir.join("SystemSpec.json").exists());
        assert_eq!(
            output
                .fragments
                .iter()
                .map(|fragment| fragment.env_key.as_str())
                .collect::<Vec<_>>(),
            vec!["NIRVASH_DOC_FRAGMENT_SYSTEM_SPEC"]
        );
    }

    #[test]
    fn render_fragment_uses_system_first_sections() {
        let fragment = render_fragment(&SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: Some("demo_model_cases".to_owned()),
            subsystems: Vec::new(),
            registrations: BTreeMap::from([
                (
                    RegistrationKind::Invariant,
                    vec!["demo_invariant".to_owned()],
                ),
                (RegistrationKind::Property, vec!["demo_property".to_owned()]),
            ]),
            doc_graphs: vec![nirvash::DocGraphCase {
                label: "default".to_owned(),
                surface: None,
                projection: None,
                backend: nirvash::ModelBackend::Explicit,
                trust_tier: nirvash::TrustTier::Exact,
                graph: nirvash::DocGraphSnapshot {
                    states: vec![
                        nirvash::DocGraphState {
                            summary: "Idle".to_owned(),
                            full: "Idle".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                        nirvash::DocGraphState {
                            summary: "Busy".to_owned(),
                            full: "Busy".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                    ],
                    edges: vec![vec![demo_edge("Start", 1)], vec![demo_edge("Stop", 0)]],
                    initial_indices: vec![0],
                    deadlocks: vec![],
                    truncated: false,
                    stutter_omitted: true,
                    focus_indices: Vec::new(),
                    reduction: nirvash::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });
        assert!(fragment.contains("## System Map"));
        assert!(fragment.contains("## Scenario Atlas"));
        assert!(fragment.contains("## Actor Flows"));
        assert!(fragment.contains("## State Space"));
        assert!(fragment.contains("## Contracts & Data"));
        assert!(fragment.contains("<pre class=\"mermaid nirvash-mermaid\">"));
        assert!(fragment.contains("stateDiagram-v2"));
        assert!(fragment.contains("default"));
        assert!(fragment.contains("state &quot;Idle&quot; as S0"));
        assert!(!fragment.contains("state &quot;Busy&quot; as S1"));
        assert!(fragment.contains("[*] --&gt; S0") || fragment.contains("[*] --> S0"));
        assert!(
            fragment.contains("Start → Stop")
                || fragment.contains("Start &#8594; Stop")
                || fragment.contains("Start &rarr; Stop")
        );
        assert!(fragment.contains("stutter omitted"));
        assert!(fragment.contains("<details><summary>State legend</summary>"));
        assert!(!fragment.contains("## Sequence Diagram"));
        assert!(fragment.contains("process Spec:"));
        assert!(fragment.contains("scenario cycle witness"));
        assert!(fragment.contains("do Start"));
        assert!(fragment.contains("| # | transition | action |"));
        assert!(fragment.contains("<details><summary>default process text fallback</summary>"));
        assert!(fragment.contains("### Action Vocabulary"));
        assert!(fragment.contains("#### S0"));
        assert!(fragment.contains("```text\nIdle\n```"));
        assert!(fragment.contains("runtime.textContent = "));
        assert!(fragment.contains("<details><summary>State legend</summary>"));
    }

    #[test]
    fn render_system_fragment_groups_surface_views() {
        let bundle = demo_bundle(
            "SystemSpec",
            "crate::system::SystemSpec",
            Some(nirvash::SpecVizKind::System),
            vec![
                demo_graph_case("default"),
                nirvash::DocGraphCase {
                    label: "explicit_manager_view".to_owned(),
                    surface: Some("Manager View".to_owned()),
                    projection: Some("ManagerViewState".to_owned()),
                    ..demo_graph_case("manager_view")
                },
                nirvash::DocGraphCase {
                    label: "explicit_control_view".to_owned(),
                    surface: Some("Control View".to_owned()),
                    projection: Some("ControlViewState".to_owned()),
                    ..demo_graph_case("control_view")
                },
            ],
        );

        let fragment = render_viz_fragment(&bundle);

        assert!(fragment.contains("### Views"));
        assert!(fragment.contains("`Manager View`: projection `ManagerViewState`"));
        assert!(fragment.contains("`Control View`: projection `ControlViewState`"));
        assert!(fragment.contains("### System"));
        assert!(fragment.contains("### Manager View"));
        assert!(fragment.contains("### Control View"));
        assert!(fragment.contains("#### explicit_manager_view"));
        assert!(fragment.contains("#### explicit_control_view"));
    }

    #[test]
    fn render_fragment_includes_relation_schema_and_relation_notation() {
        let fragment = render_fragment(&SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "RelationalSpec".to_owned()],
            tail_ident: "RelationalSpec".to_owned(),
            state_ty: "RelationalState".to_owned(),
            action_ty: "RelationalAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: vec![nirvash::DocGraphCase {
                label: "default".to_owned(),
                surface: None,
                projection: None,
                backend: nirvash::ModelBackend::Explicit,
                trust_tier: nirvash::TrustTier::Exact,
                graph: nirvash::DocGraphSnapshot {
                    states: vec![
                        nirvash::DocGraphState {
                            summary: "unused".to_owned(),
                            full: "unused".to_owned(),
                            relation_fields: vec![
                                nirvash::RelationFieldSummary {
                                    name: "requires".to_owned(),
                                    notation: "requires = Root->Dependency".to_owned(),
                                },
                                nirvash::RelationFieldSummary {
                                    name: "allowed".to_owned(),
                                    notation: "allowed = Root".to_owned(),
                                },
                            ],
                            relation_schema: vec![
                                nirvash::RelationFieldSchema {
                                    name: "requires".to_owned(),
                                    kind: nirvash::RelationFieldKind::Binary,
                                    from_type: "PluginAtom".to_owned(),
                                    to_type: Some("PluginAtom".to_owned()),
                                },
                                nirvash::RelationFieldSchema {
                                    name: "allowed".to_owned(),
                                    kind: nirvash::RelationFieldKind::Set,
                                    from_type: "PluginAtom".to_owned(),
                                    to_type: None,
                                },
                            ],
                        },
                        nirvash::DocGraphState {
                            summary: "unused-next".to_owned(),
                            full: "unused-next".to_owned(),
                            relation_fields: vec![nirvash::RelationFieldSummary {
                                name: "requires".to_owned(),
                                notation: "requires = Root->Dependency".to_owned(),
                            }],
                            relation_schema: vec![nirvash::RelationFieldSchema {
                                name: "requires".to_owned(),
                                kind: nirvash::RelationFieldKind::Binary,
                                from_type: "PluginAtom".to_owned(),
                                to_type: Some("PluginAtom".to_owned()),
                            }],
                        },
                    ],
                    edges: vec![vec![demo_edge("Advance", 1)], Vec::new()],
                    initial_indices: vec![0],
                    deadlocks: vec![],
                    truncated: false,
                    stutter_omitted: false,
                    focus_indices: Vec::new(),
                    reduction: nirvash::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });

        assert!(fragment.contains("## Contracts & Data"));
        assert!(fragment.contains("### Relation Schema"));
        assert!(fragment.contains("requires"));
        assert!(fragment.contains("allowed"));
    }

    #[test]
    fn render_fragment_resolves_subsystem_and_parent_links_by_spec_id() {
        let child = nirvash::SpecVizBundle::from_doc_graph_spec(
            "ChildSpec",
            nirvash::SpecVizMetadata {
                spec_id: "crate::child::ChildSpec".to_owned(),
                kind: Some(nirvash::SpecVizKind::Subsystem),
                state_ty: "ChildState".to_owned(),
                action_ty: "ChildAction".to_owned(),
                model_cases: None,
                subsystems: Vec::new(),
                registrations: nirvash::SpecVizRegistrationSet::default(),
                policy: nirvash::VizPolicy::default(),
            },
            Vec::new(),
        );
        let parent = nirvash::SpecVizBundle::from_doc_graph_spec(
            "RootSpec",
            nirvash::SpecVizMetadata {
                spec_id: "crate::system::RootSpec".to_owned(),
                kind: Some(nirvash::SpecVizKind::System),
                state_ty: "RootState".to_owned(),
                action_ty: "RootAction".to_owned(),
                model_cases: None,
                subsystems: vec![nirvash::SpecVizSubsystem::new(
                    "crate::child::ChildSpec",
                    "ChildSpec",
                )],
                registrations: nirvash::SpecVizRegistrationSet::default(),
                policy: nirvash::VizPolicy::default(),
            },
            Vec::new(),
        );
        let catalog = vec![child.clone(), parent.clone()];

        let system_fragment = render_viz_fragment_with_catalog(&parent, &catalog);
        let child_fragment = render_viz_fragment_with_catalog(&child, &catalog);

        assert!(system_fragment.contains("[`ChildSpec`](crate::child::ChildSpec)"));
        assert!(child_fragment.contains("[`RootSpec`](crate::system::RootSpec)"));
        assert!(child_fragment.contains("### Parent Systems"));
        assert!(child_fragment.contains("### Related Subsystems"));
    }

    #[test]
    fn render_fragment_keeps_mermaid_actor_ids_stable_on_sanitized_collisions() {
        let fragment = render_fragment(&SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "CollisionSpec".to_owned()],
            tail_ident: "CollisionSpec".to_owned(),
            state_ty: "CollisionState".to_owned(),
            action_ty: "CollisionAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: vec![nirvash::DocGraphCase {
                label: "default".to_owned(),
                surface: None,
                projection: None,
                backend: nirvash::ModelBackend::Explicit,
                trust_tier: nirvash::TrustTier::Exact,
                graph: nirvash::DocGraphSnapshot {
                    states: vec![
                        nirvash::DocGraphState {
                            summary: "Init".to_owned(),
                            full: "Init".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                        nirvash::DocGraphState {
                            summary: "Done".to_owned(),
                            full: "Done".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                    ],
                    edges: vec![
                        vec![nirvash::DocGraphEdge {
                            label: "Dispatch".to_owned(),
                            compact_label: None,
                            scenario_priority: Some(5),
                            interaction_steps: vec![nirvash::DocGraphInteractionStep::between(
                                "Client-Manager",
                                "Client Manager",
                                "Dispatch",
                            )],
                            process_steps: vec![
                                nirvash::DocGraphProcessStep::for_actor(
                                    "Client-Manager",
                                    nirvash::DocGraphProcessKind::Send,
                                    "Dispatch",
                                ),
                                nirvash::DocGraphProcessStep::for_actor(
                                    "Client Manager",
                                    nirvash::DocGraphProcessKind::Receive,
                                    "Dispatch",
                                ),
                            ],
                            target: 1,
                        }],
                        Vec::new(),
                    ],
                    initial_indices: vec![0],
                    deadlocks: vec![1],
                    truncated: false,
                    stutter_omitted: false,
                    focus_indices: Vec::new(),
                    reduction: nirvash::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });

        assert!(
            fragment.contains("participant SEQ_CLIENT_MANAGER as &quot;Client Manager&quot;")
                || fragment
                    .contains("participant SEQ_CLIENT_MANAGER as &quot;Client-Manager&quot;")
        );
        assert!(
            fragment.contains("participant SEQ_CLIENT_MANAGER_2 as &quot;Client Manager&quot;")
                || fragment
                    .contains("participant SEQ_CLIENT_MANAGER_2 as &quot;Client-Manager&quot;")
        );
        assert!(
            fragment.contains("SEQ_CLIENT_MANAGER-&gt;&gt;SEQ_CLIENT_MANAGER_2: Dispatch")
                || fragment.contains("SEQ_CLIENT_MANAGER->>SEQ_CLIENT_MANAGER_2: Dispatch")
                || fragment.contains("SEQ_CLIENT_MANAGER_2-&gt;&gt;SEQ_CLIENT_MANAGER: Dispatch")
                || fragment.contains("SEQ_CLIENT_MANAGER_2->>SEQ_CLIENT_MANAGER: Dispatch")
        );
    }

    #[test]
    fn render_fragment_falls_back_to_focus_graph_for_large_state_spaces() {
        let states = (0..51)
            .map(|index| nirvash::DocGraphState {
                summary: format!("S{index}"),
                full: format!("S{index}"),
                relation_fields: Vec::new(),
                relation_schema: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut edges = vec![Vec::new(); 51];
        for (index, bucket) in edges.iter_mut().enumerate().take(50) {
            bucket.push(demo_edge(&format!("Step{index}"), index + 1));
        }
        let bundle = nirvash::SpecVizBundle::from_doc_graph_spec(
            "LargeSpec",
            nirvash::SpecVizMetadata {
                spec_id: "crate::demo::LargeSpec".to_owned(),
                kind: Some(nirvash::SpecVizKind::Subsystem),
                state_ty: "LargeState".to_owned(),
                action_ty: "LargeAction".to_owned(),
                model_cases: None,
                subsystems: Vec::new(),
                registrations: nirvash::SpecVizRegistrationSet::default(),
                policy: nirvash::VizPolicy {
                    max_scenarios: 1,
                    ..nirvash::VizPolicy::default()
                },
            },
            vec![nirvash::DocGraphCase {
                label: "large".to_owned(),
                surface: None,
                projection: None,
                backend: nirvash::ModelBackend::Explicit,
                trust_tier: nirvash::TrustTier::Exact,
                graph: nirvash::DocGraphSnapshot {
                    states,
                    edges,
                    initial_indices: vec![0],
                    deadlocks: vec![],
                    truncated: false,
                    stutter_omitted: false,
                    focus_indices: vec![2],
                    reduction: nirvash::DocGraphReductionMode::Full,
                    max_edge_actions_in_label: 2,
                },
            }],
        );

        let fragment = render_viz_fragment(&bundle);
        assert!(fragment.contains("Rendering focus graph selected from representative scenarios."));
        assert!(!fragment.contains("scenario mini diagrams are shown instead"));
    }

    #[test]
    fn render_fragment_omits_legacy_section_names() {
        let bundle = nirvash::SpecVizBundle {
            spec_name: "DemoSpec".to_owned(),
            metadata: nirvash::SpecVizMetadata {
                spec_id: "demo::DemoSpec".to_owned(),
                kind: Some(nirvash::SpecVizKind::Subsystem),
                state_ty: "DemoState".to_owned(),
                action_ty: "DemoAction".to_owned(),
                ..nirvash::SpecVizMetadata::default()
            },
            action_vocabulary: Vec::new(),
            relation_schema: Vec::new(),
            cases: Vec::new(),
        };

        let fragment = render_viz_fragment(&bundle);

        assert!(!fragment.contains("## Overview"));
        assert!(!fragment.contains("## Reachability"));
        assert!(!fragment.contains("## Scenario Traces"));
        assert!(!fragment.contains("## Process View"));
        assert!(!fragment.contains("## Algorithm View"));
    }

    #[test]
    fn upper_snake_names_match_fragment_keys() {
        assert_eq!(to_upper_snake("SystemSpec"), "SYSTEM_SPEC");
        assert_eq!(to_upper_snake("HTTPState"), "HTTPSTATE");
    }

    #[test]
    fn mermaid_render_script_embeds_runtime_inline() {
        let script = mermaid_render_script();

        assert!(script.contains("runtime.textContent = "));
        assert!(script.contains("nirvash mermaid runtime failed to initialize"));
        assert!(!script.contains("mermaid.min.js"));
        assert!(!script.contains("static.files"));
    }
}
