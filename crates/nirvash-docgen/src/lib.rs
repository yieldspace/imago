use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use nirvash_core::DocGraphSpec;
use quote::ToTokens;
use serde::Deserialize;
use syn::{
    Attribute, ImplItem, Item, ItemFn, ItemImpl, ItemMacro, ItemMod, LitStr, Path as SynPath,
    PathArguments, Type,
};

type DynError = Box<dyn Error>;

const CATEGORY_ORDER: [RegistrationKind; 6] = [
    RegistrationKind::Invariant,
    RegistrationKind::Property,
    RegistrationKind::Fairness,
    RegistrationKind::StateConstraint,
    RegistrationKind::ActionConstraint,
    RegistrationKind::Symmetry,
];
const MERMAID_RUNTIME_SOURCE: &str = include_str!("../assets/mermaid/mermaid.min.js");

/// Generate rustdoc fragments for `nirvash` specs in the current crate.
pub fn generate() -> Result<(), Box<dyn Error>> {
    if env::var_os("NIRVASH_DOCGEN_SKIP").is_some() {
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

impl SpecKind {
    fn label(self) -> &'static str {
        match self {
            Self::Subsystem => "subsystem_spec",
            Self::System => "system_spec",
        }
    }
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

    fn label(self) -> &'static str {
        self.attr_name()
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
    subsystems: Vec<String>,
    registrations: BTreeMap<RegistrationKind, Vec<String>>,
    doc_graphs: Vec<nirvash_core::DocGraphCase>,
}

#[derive(Debug, Clone)]
struct PendingSpec {
    kind: SpecKind,
    full_path: Vec<String>,
    tail_ident: String,
    state_ty: String,
    action_ty: String,
    model_cases: Option<String>,
    subsystems: Vec<String>,
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
                    "nirvash-docgen only supports impl TransitionSystem for <simple path>",
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
            let target: SynPath = attr.parse_args().map_err(|error| {
                err(format!(
                    "failed to parse #[{}(...)] on `{}`: {error}",
                    kind.attr_name(),
                    item_fn.sig.ident
                ))
            })?;
            self.registrations.push(PendingRegistration {
                kind,
                target_spec: normalize_path(&target, module_path)?,
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
            let path_key = path_key(&spec.full_path);
            if let Some(existing) = tail_to_path.get(&spec.tail_ident) {
                return Err(err(format!(
                    "duplicate spec tail ident `{}` for `{existing}` and `{}`",
                    spec.tail_ident, path_key
                )));
            }
            tail_to_path.insert(spec.tail_ident.clone(), path_key.clone());
            by_path.insert(
                path_key,
                SpecDoc {
                    kind: Some(spec.kind),
                    full_path: spec.full_path,
                    tail_ident: spec.tail_ident,
                    state_ty: spec.state_ty,
                    action_ty: spec.action_ty,
                    model_cases: spec.model_cases,
                    subsystems: spec.subsystems,
                    registrations: BTreeMap::new(),
                    doc_graphs: Vec::new(),
                },
            );
        }

        for registration in self.registrations {
            let key = path_key(&registration.target_spec);
            let Some(spec) = by_path.get_mut(&key) else {
                return Err(err(format!(
                    "registration `{}` targets unknown spec `{}`",
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
        for runtime_spec in collect_runtime_graphs(manifest_dir, out_dir, &runtime_spec_paths)? {
            if let Some(path_key) = tail_to_path.get(&runtime_spec.spec_name)
                && let Some(spec) = by_path.get_mut(path_key)
            {
                spec.doc_graphs = runtime_spec.cases;
            }
        }

        let doc_dir = out_dir.join("nirvash-doc");
        fs::create_dir_all(&doc_dir).map_err(|error| {
            err(format!(
                "failed to create documentation fragment directory {}: {error}",
                doc_dir.display()
            ))
        })?;

        let mut fragments = Vec::new();
        for spec in by_path.values_mut() {
            for names in spec.registrations.values_mut() {
                names.sort();
            }
            let env_key = format!("NIRVASH_DOC_FRAGMENT_{}", to_upper_snake(&spec.tail_ident));
            let path = doc_dir.join(format!("{}.md", spec.tail_ident));
            fs::write(&path, render_fragment(spec)).map_err(|error| {
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

#[derive(Default)]
struct ParsedSpecArgs {
    model_cases: Option<String>,
    subsystems: Vec<String>,
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
                        let value: LitStr = content.parse()?;
                        args.subsystems.push(value.value());
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
    collector.finish(manifest_dir, out_dir)
}

fn collect_runtime_graphs(
    manifest_dir: &Path,
    out_dir: &Path,
    spec_paths: &[Vec<String>],
) -> Result<Vec<DocGraphSpec>, DynError> {
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
    let nirvash_core_manifest = metadata
        .packages
        .iter()
        .find(|package| package.name == "nirvash-core")
        .and_then(|package| package.manifest_path.parent().map(Path::to_path_buf))
        .ok_or_else(|| err("failed to locate `nirvash-core` package in cargo metadata"))?;

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
        render_runner_manifest(manifest_dir, &current_package.name, &nirvash_core_manifest),
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

    let output = Command::new(cargo_binary())
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&runner_manifest)
        .arg("--target-dir")
        .arg(out_dir.join("nirvash-doc-runner-target"))
        .env("NIRVASH_DOCGEN_SKIP", "1")
        .output()
        .map_err(|error| {
            err(format!(
                "failed to execute runtime graph runner {}: {error}",
                runner_manifest.display()
            ))
        })?;

    if !output.status.success() {
        return Err(err(format!(
            "runtime graph runner failed for {}:\n{}",
            runner_manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        err(format!(
            "failed to parse runtime graph output from {}: {error}",
            runner_manifest.display()
        ))
    })
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
    nirvash_core_dir: &Path,
) -> String {
    format!(
        "[package]\nname = \"nirvash-doc-runner\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nserde_json = \"1\"\nnirvash_core = {{ package = \"nirvash-core\", path = \"{}\" }}\ndoc_target = {{ package = \"{}\", path = \"{}\" }}\n\n[profile.dev]\ndebug = 0\nincremental = false\n",
        escape_toml_path(nirvash_core_dir),
        escape_toml_str(current_package_name),
        escape_toml_path(manifest_dir),
    )
}

fn render_runner_main(spec_paths: &[Vec<String>]) -> String {
    let mut output = String::from("extern crate doc_target;\n\nfn main() {\n");
    for path in spec_paths {
        output.push_str("    ");
        output.push_str(&render_link_call(path));
        output.push('\n');
    }
    output.push_str(
        "    let specs = nirvash_core::collect_doc_graph_specs();\n    println!(\"{}\", serde_json::to_string(&specs).expect(\"serialize doc graphs\"));\n}\n",
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
    path.push_str("::__nirvash_doc_graph_provider_link_");
    path.push_str(&tail.to_lowercase());
    path.push_str("();");
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

fn render_fragment(spec: &SpecDoc) -> String {
    let mut output = String::new();
    if !spec.doc_graphs.is_empty() {
        output.push_str(&render_state_graph_section(spec));
        output.push_str("\n\n");
        let relation_schema = render_relation_schema_section(spec);
        if !relation_schema.is_empty() {
            output.push_str(&relation_schema);
            output.push_str("\n\n");
        }
    }

    let mermaid = render_meta_model_mermaid(spec);
    let model_cases = spec.model_cases.as_deref().unwrap_or("default");
    output.push_str("## Meta Model\n\n");
    output.push_str(&render_mermaid_block(&mermaid));
    output.push_str("\n\nLegend:\n\n");
    output.push_str(&format!("- kind: `{}`\n", spec.kind.unwrap().label()));
    output.push_str(&format!("- state: `{}`\n", spec.state_ty));
    output.push_str(&format!("- action: `{}`\n", spec.action_ty));
    output.push_str(&format!("- model_cases = `{model_cases}`\n"));
    if spec.kind == Some(SpecKind::System) {
        let subsystems = if spec.subsystems.is_empty() {
            "none".to_string()
        } else {
            spec.subsystems
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        output.push_str(&format!("- subsystems: {subsystems}\n"));
    }
    output.push_str("\nRegistered functions:\n\n");
    for kind in CATEGORY_ORDER {
        let names = spec
            .registrations
            .get(&kind)
            .filter(|values| !values.is_empty())
            .map(|values| {
                values
                    .iter()
                    .map(|value| format!("`{value}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "none".to_string());
        output.push_str(&format!("- {}: {}\n", kind.label(), names));
    }
    output.push('\n');
    output.push_str(&mermaid_render_script());
    output
}

fn render_state_graph_section(spec: &SpecDoc) -> String {
    let mut output = String::from("## State Graph\n\n");
    for case in &spec.doc_graphs {
        let reduced_graph = ::nirvash_core::reduce_doc_graph(&case.graph);
        let visible_edges = visible_reduced_edges(&reduced_graph);
        output.push_str(&format!("### {}\n\n", case.label));
        if reduced_graph.truncated {
            output.push_str("Warning: truncated by checker limits.\n\n");
        }
        if reduced_graph.stutter_omitted {
            output.push_str("Note: stutter omitted from rendered edges.\n\n");
        }
        output.push_str(&render_mermaid_block(&render_state_graph_mermaid(
            spec,
            &reduced_graph,
            &visible_edges,
        )));
        output.push_str("\n\n<details><summary>Full State Legend</summary>\n\n");
        for state in &reduced_graph.states {
            if !state.state.relation_fields.is_empty() {
                output.push_str("Relations:\n\n```text\n");
                for relation in &state.state.relation_fields {
                    output.push_str(&relation.notation);
                    output.push('\n');
                }
                output.push_str("```\n\n");
            }
            output.push_str(&format!(
                "#### S{}\n\n```text\n{}\n```\n\n",
                state.original_index, state.state.full
            ));
        }
        let initial = reduced_graph
            .states
            .iter()
            .filter(|state| state.is_initial)
            .map(|state| format!("S{}", state.original_index))
            .collect::<Vec<_>>();
        if !initial.is_empty() {
            output.push_str(&format!("- initial: {}\n", initial.join(", ")));
        }
        let deadlocks = reduced_graph
            .states
            .iter()
            .filter(|state| state.is_deadlock)
            .map(|state| format!("S{}", state.original_index))
            .collect::<Vec<_>>();
        if !deadlocks.is_empty() {
            output.push_str(&format!("- deadlocks: {}\n", deadlocks.join(", ")));
        }
        output.push_str("\n</details>\n\n");
        let collapsed_details = render_collapsed_path_details(case, &visible_edges);
        if !collapsed_details.is_empty() {
            output.push_str(&collapsed_details);
        }
    }
    output
}

fn render_state_graph_mermaid(
    spec: &SpecDoc,
    graph: &nirvash_core::ReducedDocGraph,
    visible_edges: &[&nirvash_core::ReducedDocGraphEdge],
) -> String {
    let mut output = String::from("stateDiagram-v2\n");
    output.push_str(&format!(
        "%% reachable state graph for {}\n",
        spec.tail_ident
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
            mermaid_edge_label(&edge.label)
        ));
    }

    output
}

fn visible_reduced_edges(
    graph: &nirvash_core::ReducedDocGraph,
) -> Vec<&nirvash_core::ReducedDocGraphEdge> {
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
    graph: &nirvash_core::ReducedDocGraph,
    state: &nirvash_core::ReducedDocGraphNode,
) -> String {
    let mut parts = Vec::new();
    if state.is_deadlock {
        parts.push("DEADLOCK".to_string());
    }
    parts.extend(state_display_lines(graph, state));

    mermaid_state_label(&parts.join("\n"))
}

fn state_display_lines(
    graph: &nirvash_core::ReducedDocGraph,
    state: &nirvash_core::ReducedDocGraphNode,
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

fn preferred_predecessor(graph: &nirvash_core::ReducedDocGraph, target: usize) -> Option<usize> {
    let mut candidates = graph
        .edges
        .iter()
        .filter(|edge| edge.source != target && edge.target == target)
        .map(|edge| edge.source)
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.into_iter().next()
}

fn render_collapsed_path_details(
    case: &nirvash_core::DocGraphCase,
    visible_edges: &[&nirvash_core::ReducedDocGraphEdge],
) -> String {
    let collapsed_edges = visible_edges
        .iter()
        .copied()
        .filter(|edge| !edge.collapsed_state_indices.is_empty())
        .collect::<Vec<_>>();
    if collapsed_edges.is_empty() {
        return String::new();
    }

    let mut output = String::from("<details><summary>Collapsed Path Details</summary>\n\n");
    for edge in collapsed_edges {
        output.push_str(&format!("#### S{} -> S{}\n\n", edge.source, edge.target));
        output.push_str(&format!(
            "- collapsed: {}\n\n",
            edge.collapsed_state_indices
                .iter()
                .map(|index| format!("S{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for index in &edge.collapsed_state_indices {
            output.push_str(&format!(
                "##### S{index}\n\n```text\n{}\n```\n\n",
                case.graph.states[*index].full
            ));
        }
    }
    output.push_str("</details>\n\n");
    output
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
    relation_fields: &[nirvash_core::RelationFieldSummary],
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

fn render_relation_schema_section(spec: &SpecDoc) -> String {
    let schemas = collect_relation_schemas(spec);
    if schemas.is_empty() {
        return String::new();
    }

    let set_relations = schemas
        .iter()
        .filter(|schema| schema.kind == nirvash_core::RelationFieldKind::Set)
        .collect::<Vec<_>>();
    let binary_relations = schemas
        .iter()
        .filter(|schema| schema.kind == nirvash_core::RelationFieldKind::Binary)
        .collect::<Vec<_>>();

    let mut output = String::from("## Relation Schema\n\n");
    if !binary_relations.is_empty() {
        output.push_str(&render_mermaid_block(&render_relation_schema_mermaid(
            &binary_relations,
        )));
        output.push_str("\n\n");
    }
    if !set_relations.is_empty() {
        output.push_str("Set relations:\n\n");
        for schema in set_relations {
            output.push_str(&format!(
                "- `{}`: set of `{}`\n",
                schema.name, schema.from_type
            ));
        }
        output.push('\n');
    }
    if !binary_relations.is_empty() {
        output.push_str("Binary relations:\n\n");
        for schema in binary_relations {
            output.push_str(&format!(
                "- `{}`: `{}` -> `{}`\n",
                schema.name,
                schema.from_type,
                schema.to_type.as_deref().unwrap_or("?")
            ));
        }
    }
    output
}

fn collect_relation_schemas(spec: &SpecDoc) -> Vec<nirvash_core::RelationFieldSchema> {
    let mut seen = BTreeSet::new();
    let mut schemas = Vec::new();
    for case in &spec.doc_graphs {
        for state in &case.graph.states {
            for schema in &state.relation_schema {
                let key = (
                    schema.name.clone(),
                    schema.kind,
                    schema.from_type.clone(),
                    schema.to_type.clone(),
                );
                if seen.insert(key) {
                    schemas.push(schema.clone());
                }
            }
        }
    }
    schemas.sort_by(|left, right| left.name.cmp(&right.name));
    schemas
}

fn render_relation_schema_mermaid(schemas: &[&nirvash_core::RelationFieldSchema]) -> String {
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

fn render_meta_model_mermaid(spec: &SpecDoc) -> String {
    let mut output = String::from("flowchart LR\n");
    output.push_str("classDef spec fill:#dceeff,stroke:#1d4ed8,stroke-width:2px,color:#0f172a;\n");
    output.push_str("classDef state fill:#e7f7e7,stroke:#15803d,stroke-width:2px,color:#0f172a;\n");
    output
        .push_str("classDef action fill:#fff1d6,stroke:#b45309,stroke-width:2px,color:#0f172a;\n");
    output
        .push_str("classDef system fill:#ffe8f2,stroke:#be185d,stroke-width:2px,color:#0f172a;\n");
    output.push_str(
        "classDef category fill:#ffffff,stroke:#334155,stroke-width:2px,color:#0f172a;\n",
    );

    output.push_str(&format!(
        "spec[\"{}<br/>kind={}<br/>path={}\"]\n",
        escape_mermaid_label(&spec.tail_ident),
        spec.kind.expect("kind").label(),
        escape_mermaid_label(&path_key(&spec.full_path))
    ));
    output.push_str(&format!(
        "state[\"State<br/>{}\"]\n",
        escape_mermaid_label(&spec.state_ty)
    ));
    output.push_str(&format!(
        "action[\"Action<br/>{}\"]\n",
        escape_mermaid_label(&spec.action_ty)
    ));
    output.push_str("spec --> state\n");
    output.push_str("spec --> action\n");
    output.push_str("class spec spec;\nclass state state;\nclass action action;\n");

    if spec.kind == Some(SpecKind::System) {
        let subsystem_label = if spec.subsystems.is_empty() {
            "Subsystems<br/>none".to_owned()
        } else {
            format!(
                "Subsystems<br/>{}",
                mermaid_multiline(&spec.subsystems.join("\n"))
            )
        };
        output.push_str(&format!("subsystems[\"{subsystem_label}\"]\n"));
        output.push_str(
            "composition[\"SystemComposition<br/>invariants<br/>properties<br/>fairness<br/>constraints\"]\n",
        );
        output.push_str("spec --> subsystems\n");
        output.push_str("spec --> composition\n");
        output.push_str("class subsystems system;\nclass composition system;\n");
    }

    for kind in CATEGORY_ORDER {
        let names = spec
            .registrations
            .get(&kind)
            .filter(|values| !values.is_empty())
            .map(|values| values.join("<br/>"))
            .unwrap_or_else(|| "none".to_owned());
        let node_id = format!("category_{}", kind.label());
        output.push_str(&format!(
            "{node_id}[\"{}<br/>{}\"]\n",
            kind.label(),
            mermaid_multiline(&names.replace("<br/>", "\n"))
        ));
        output.push_str(&format!("spec --> {node_id}\n"));
        output.push_str(&format!("class {node_id} category;\n"));
    }

    output
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

fn mermaid_multiline(input: &str) -> String {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(escape_mermaid_label)
        .collect::<Vec<_>>()
        .join("<br/>")
}

fn mermaid_edge_label(input: &str) -> String {
    format!("\"{}\"", escape_mermaid_edge_label(input))
}

fn escape_mermaid_edge_label(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generate_collects_supported_module_tree_and_renders_mermaid() {
        let dir = tempdir().expect("tempdir");
        let manifest_dir = dir.path();
        let src_dir = manifest_dir.join("src");
        let out_dir = manifest_dir.join("out");
        fs::create_dir_all(&src_dir).expect("src");

        fs::write(
            src_dir.join("lib.rs"),
            r#"
pub mod child;
pub mod system;

mod inline_parent {
    use nirvash_core::{Ltl, StatePredicate, TransitionSystem};
    use nirvash_macros::{invariant, property, subsystem_spec};

    pub struct InlineState;
    pub struct InlineAction;
    pub struct InlineSpec;

    #[subsystem_spec(model_cases(inline_model_cases))]
    impl TransitionSystem for InlineSpec {
        type State = InlineState;
        type Action = InlineAction;

        fn initial_states(&self) -> Vec<Self::State> { vec![InlineState] }
        fn actions(&self) -> Vec<Self::Action> { vec![InlineAction] }
        fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> { Some(InlineState) }
    }

    nirvash_core::invariant!(self::InlineSpec, inline_invariant(state) => {
        let _ = state;
        true
    });

    mod nested {
        use super::{InlineAction, InlineState};
        use nirvash_core::{Ltl, StatePredicate};
        use nirvash_macros::{invariant, property};

        #[invariant(super::InlineSpec)]
        fn super_invariant() -> StatePredicate<InlineState> { todo!() }

        #[property(crate::inline_parent::InlineSpec)]
        fn crate_property() -> Ltl<InlineState, InlineAction> { todo!() }
    }

    fn inline_model_cases() {}
}
"#,
        )
        .expect("lib.rs");

        fs::write(
            src_dir.join("child.rs"),
            r#"
use nirvash_core::{
    ActionConstraint, Fairness, Ltl, StateConstraint, StatePredicate, TransitionSystem,
};
use nirvash_macros::{invariant, property, subsystem_spec};

pub struct ChildState;
pub struct ChildAction;
pub struct ChildSpec;

#[subsystem_spec]
impl TransitionSystem for ChildSpec {
    type State = ChildState;
    type Action = ChildAction;

    fn initial_states(&self) -> Vec<Self::State> { vec![ChildState] }
    fn actions(&self) -> Vec<Self::Action> { vec![ChildAction] }
    fn transition(&self, _: &Self::State, _: &Self::Action) -> Option<Self::State> {
        Some(ChildState)
    }
}

nirvash_core::invariant!(ChildSpec, child_invariant(state) => {
    let _ = state;
    true
});

nirvash_core::state_constraint!(ChildSpec, child_state_constraint(state) => {
    let _ = state;
    true
});

nirvash_core::action_constraint!(ChildSpec, child_action_constraint(prev, action, next) => {
    let _ = (prev, action, next);
    true
});

nirvash_core::property!(ChildSpec, child_property => leads_to(
    (pred!(child_busy(state) => {
        let _ = state;
        true
    })),
    (pred!(child_idle(state) => {
        let _ = state;
        true
    }))
));

nirvash_core::fairness!(weak ChildSpec, child_fairness(prev, action, next) => {
    let _ = (prev, action, next);
    true
});
"#,
        )
        .expect("child.rs");

        fs::write(
            src_dir.join("system.rs"),
            r#"
use nirvash_core::{Ltl, StatePredicate, TransitionSystem};
use nirvash_macros::{invariant, property, system_spec};

pub struct SystemState;
pub struct SystemAction;
pub struct RootSystemSpec;

#[system_spec(subsystems("child", "inline_parent"), model_cases(system_model_cases))]
impl TransitionSystem for RootSystemSpec {
    type State = SystemState;
    type Action = SystemAction;

    fn initial_states(&self) -> Vec<Self::State> { vec![SystemState] }
    fn successors(&self, _: &Self::State) -> Vec<(Self::Action, Self::State)> {
        vec![(SystemAction, SystemState)]
    }
}

#[invariant(RootSystemSpec)]
fn system_invariant() -> StatePredicate<SystemState> { todo!() }

#[property(RootSystemSpec)]
fn system_property() -> Ltl<SystemState, SystemAction> { todo!() }

fn system_model_cases() {}
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
        assert!(inline_doc.contains("## Meta Model"));
        assert!(inline_doc.contains("<pre class=\"mermaid nirvash-mermaid\">"));
        assert!(inline_doc.contains("flowchart LR"));
        assert!(inline_doc.contains("InlineSpec"));
        assert!(inline_doc.contains("inline_invariant"));
        assert!(inline_doc.contains("super_invariant"));
        assert!(inline_doc.contains("crate_property"));
        assert!(inline_doc.contains("model_cases = `inline_model_cases`"));
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
        assert!(system_doc.contains("SystemComposition"));
        assert!(system_doc.contains("child"));
        assert!(system_doc.contains("inline_parent"));
        assert!(system_doc.contains("system_invariant"));

        assert_eq!(
            output.rerun_if_changed,
            vec![
                manifest_dir.join("Cargo.toml"),
                src_dir.join("child.rs"),
                src_dir.join("lib.rs"),
                src_dir.join("system.rs"),
            ]
        );
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
use nirvash_core::TransitionSystem;
use nirvash_macros::subsystem_spec;

pub struct State;
pub struct Action;
pub struct DuplicateSpec;

#[subsystem_spec]
impl TransitionSystem for DuplicateSpec {
    type State = State;
    type Action = Action;

    fn initial_states(&self) -> Vec<Self::State> { vec![State] }
    fn successors(&self, _: &Self::State) -> Vec<(Self::Action, Self::State)> {
        vec![(Action, State)]
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
    fn render_fragment_prefers_runtime_state_graph_when_present() {
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
            doc_graphs: vec![nirvash_core::DocGraphCase {
                label: "default".to_owned(),
                graph: nirvash_core::DocGraphSnapshot {
                    states: vec![
                        nirvash_core::DocGraphState {
                            summary: "Idle".to_owned(),
                            full: "Idle".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                        nirvash_core::DocGraphState {
                            summary: "Busy".to_owned(),
                            full: "Busy".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                    ],
                    edges: vec![
                        vec![nirvash_core::DocGraphEdge {
                            label: "Start".to_owned(),
                            target: 1,
                        }],
                        vec![nirvash_core::DocGraphEdge {
                            label: "Stop".to_owned(),
                            target: 0,
                        }],
                    ],
                    initial_indices: vec![0],
                    deadlocks: vec![],
                    truncated: false,
                    stutter_omitted: true,
                    focus_indices: Vec::new(),
                    reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });
        assert!(fragment.contains("## State Graph"));
        assert!(fragment.contains("<pre class=\"mermaid nirvash-mermaid\">"));
        assert!(fragment.contains("stateDiagram-v2"));
        assert!(fragment.contains("default"));
        assert!(fragment.contains("state &quot;Idle&quot; as S0"));
        assert!(!fragment.contains("state &quot;Busy&quot; as S1"));
        assert!(fragment.contains("[*] --&gt; S0") || fragment.contains("[*] --> S0"));
        assert!(
            fragment.contains("Start -&amp;gt; Stop")
                || fragment.contains("Start -&gt; Stop")
                || fragment.contains("Start -> Stop")
        );
        assert!(fragment.contains("Note: stutter omitted"));
        assert!(fragment.contains("<details><summary>Collapsed Path Details</summary>"));
        assert!(fragment.contains("#### S0 -&gt; S0") || fragment.contains("#### S0 -> S0"));
        assert!(fragment.contains("collapsed: S1"));
        assert!(fragment.contains("## Meta Model"));
        assert!(fragment.contains("flowchart LR"));
        assert!(fragment.contains("#### S0"));
        assert!(fragment.contains("```text\nIdle\n```"));
        assert!(fragment.contains("```text\nBusy\n```"));
        assert!(fragment.contains("runtime.textContent = "));
        assert!(fragment.contains("<details><summary>Full State Legend</summary>"));
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
            doc_graphs: vec![nirvash_core::DocGraphCase {
                label: "default".to_owned(),
                graph: nirvash_core::DocGraphSnapshot {
                    states: vec![
                        nirvash_core::DocGraphState {
                            summary: "unused".to_owned(),
                            full: "unused".to_owned(),
                            relation_fields: vec![
                                nirvash_core::RelationFieldSummary {
                                    name: "requires".to_owned(),
                                    notation: "requires = Root->Dependency".to_owned(),
                                },
                                nirvash_core::RelationFieldSummary {
                                    name: "allowed".to_owned(),
                                    notation: "allowed = Root".to_owned(),
                                },
                            ],
                            relation_schema: vec![
                                nirvash_core::RelationFieldSchema {
                                    name: "requires".to_owned(),
                                    kind: nirvash_core::RelationFieldKind::Binary,
                                    from_type: "PluginAtom".to_owned(),
                                    to_type: Some("PluginAtom".to_owned()),
                                },
                                nirvash_core::RelationFieldSchema {
                                    name: "allowed".to_owned(),
                                    kind: nirvash_core::RelationFieldKind::Set,
                                    from_type: "PluginAtom".to_owned(),
                                    to_type: None,
                                },
                            ],
                        },
                        nirvash_core::DocGraphState {
                            summary: "unused-next".to_owned(),
                            full: "unused-next".to_owned(),
                            relation_fields: vec![nirvash_core::RelationFieldSummary {
                                name: "requires".to_owned(),
                                notation: "requires = Root->Dependency".to_owned(),
                            }],
                            relation_schema: vec![nirvash_core::RelationFieldSchema {
                                name: "requires".to_owned(),
                                kind: nirvash_core::RelationFieldKind::Binary,
                                from_type: "PluginAtom".to_owned(),
                                to_type: Some("PluginAtom".to_owned()),
                            }],
                        },
                    ],
                    edges: vec![
                        vec![nirvash_core::DocGraphEdge {
                            label: "Advance".to_owned(),
                            target: 1,
                        }],
                        Vec::new(),
                    ],
                    initial_indices: vec![0],
                    deadlocks: vec![],
                    truncated: false,
                    stutter_omitted: false,
                    focus_indices: Vec::new(),
                    reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });

        assert!(fragment.contains("## Relation Schema"));
        assert!(fragment.contains("erDiagram"));
        assert!(fragment.contains("`requires`: `PluginAtom` -> `PluginAtom`"));
        assert!(fragment.contains("`allowed`: set of `PluginAtom`"));
        assert!(
            fragment.contains("requires = Root-&gt;Dependency")
                || fragment.contains("requires = Root->Dependency")
        );
    }

    #[test]
    fn render_state_graph_quotes_edge_labels_with_parentheses() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::reduce_doc_graph(&nirvash_core::DocGraphSnapshot {
            states: vec![
                nirvash_core::DocGraphState {
                    summary: "Init".to_owned(),
                    full: "Init".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
                nirvash_core::DocGraphState {
                    summary: "Next".to_owned(),
                    full: "Next".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
            ],
            edges: vec![
                vec![nirvash_core::DocGraphEdge {
                    label: "Manager(LoadExistingConfig)".to_owned(),
                    target: 1,
                }],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        });
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(diagram.contains("S0 --> S1: \"Manager(...)\""));
    }

    #[test]
    fn render_state_graph_quotes_collapsed_edge_labels_with_arrows() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::ReducedDocGraph {
            states: vec![
                nirvash_core::ReducedDocGraphNode {
                    original_index: 0,
                    state: nirvash_core::DocGraphState {
                        summary: "Init".to_owned(),
                        full: "Init".to_owned(),
                        relation_fields: Vec::new(),
                        relation_schema: Vec::new(),
                    },
                    is_initial: true,
                    is_deadlock: false,
                },
                nirvash_core::ReducedDocGraphNode {
                    original_index: 1,
                    state: nirvash_core::DocGraphState {
                        summary: "Stopped".to_owned(),
                        full: "Stopped".to_owned(),
                        relation_fields: Vec::new(),
                        relation_schema: Vec::new(),
                    },
                    is_initial: false,
                    is_deadlock: true,
                },
            ],
            edges: vec![nirvash_core::ReducedDocGraphEdge {
                source: 0,
                target: 1,
                label: "StartListening -> ... -> FinishShutdown (3 steps)".to_owned(),
                collapsed_state_indices: vec![2, 3],
            }],
            truncated: false,
            stutter_omitted: false,
        };
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(
            diagram.contains("S0 --> S1: \"StartListening -> ... -> FinishShutdown (3 steps)\"")
        );
    }

    #[test]
    fn render_state_graph_highlights_deadlocks_in_red() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::ReducedDocGraph {
            states: vec![nirvash_core::ReducedDocGraphNode {
                original_index: 3,
                state: nirvash_core::DocGraphState {
                    summary: "Stopped".to_owned(),
                    full: "Stopped".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
                is_initial: false,
                is_deadlock: true,
            }],
            edges: Vec::new(),
            truncated: false,
            stutter_omitted: false,
        };
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(diagram.contains("state \"DEADLOCK<br/>Stopped\" as S3"));
        assert!(diagram.contains("classDef deadlock fill:#fee2e2,stroke:#b91c1c"));
        assert!(diagram.contains("class S3 deadlock\n"));
        assert!(!diagram.contains("class S3 deadlock;"));
    }

    #[test]
    fn render_state_graph_uses_circle_nodes_and_compact_delta_labels() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::reduce_doc_graph(&nirvash_core::DocGraphSnapshot {
            states: vec![
                nirvash_core::DocGraphState {
                    summary: "State { phase: Booting, unchanged: false }".to_owned(),
                    full: "State {\n    phase: Booting,\n    unchanged: false,\n}\n".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
                nirvash_core::DocGraphState {
                    summary: "State { phase: Listening, unchanged: false }".to_owned(),
                    full: "State {\n    phase: Listening,\n    unchanged: false,\n}\n".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
            ],
            edges: vec![
                vec![nirvash_core::DocGraphEdge {
                    label: "Advance".to_owned(),
                    target: 1,
                }],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        });
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(diagram.contains("state \"phase: Booting\" as S0"));
        assert!(diagram.contains("state \"phase: Listening\" as S1"));
        assert!(!diagram.contains("\\n"));
        assert!(!diagram.contains("state \"S0"));
        assert!(!diagram.contains("unchanged: false"));
    }

    #[test]
    fn render_state_graph_truncates_verbose_edge_payloads() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::reduce_doc_graph(&nirvash_core::DocGraphSnapshot {
            states: vec![
                nirvash_core::DocGraphState {
                    summary: "Init".to_owned(),
                    full: "Init".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
                nirvash_core::DocGraphState {
                    summary: "Running".to_owned(),
                    full: "Running".to_owned(),
                    relation_fields: Vec::new(),
                    relation_schema: Vec::new(),
                },
            ],
            edges: vec![
                vec![nirvash_core::DocGraphEdge {
                    label: "Start { request_id: 1, payload: VeryVerbosePayload }".to_owned(),
                    target: 1,
                }],
                Vec::new(),
            ],
            initial_indices: vec![0],
            deadlocks: vec![],
            truncated: false,
            stutter_omitted: false,
            focus_indices: Vec::new(),
            reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
            max_edge_actions_in_label: 2,
        });
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(diagram.contains("S0 --> S1: \"Start{...}\""));
    }

    #[test]
    fn render_state_graph_omits_nonessential_self_loops() {
        let spec = SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: Vec::new(),
        };
        let graph = nirvash_core::ReducedDocGraph {
            states: vec![
                nirvash_core::ReducedDocGraphNode {
                    original_index: 0,
                    state: nirvash_core::DocGraphState {
                        summary: "S0".to_owned(),
                        full: "S0".to_owned(),
                        relation_fields: Vec::new(),
                        relation_schema: Vec::new(),
                    },
                    is_initial: true,
                    is_deadlock: false,
                },
                nirvash_core::ReducedDocGraphNode {
                    original_index: 1,
                    state: nirvash_core::DocGraphState {
                        summary: "S1".to_owned(),
                        full: "S1".to_owned(),
                        relation_fields: Vec::new(),
                        relation_schema: Vec::new(),
                    },
                    is_initial: false,
                    is_deadlock: false,
                },
            ],
            edges: vec![
                nirvash_core::ReducedDocGraphEdge {
                    source: 0,
                    target: 0,
                    label: "Retry".to_owned(),
                    collapsed_state_indices: Vec::new(),
                },
                nirvash_core::ReducedDocGraphEdge {
                    source: 0,
                    target: 1,
                    label: "Advance".to_owned(),
                    collapsed_state_indices: Vec::new(),
                },
                nirvash_core::ReducedDocGraphEdge {
                    source: 1,
                    target: 1,
                    label: "Loop".to_owned(),
                    collapsed_state_indices: Vec::new(),
                },
            ],
            truncated: false,
            stutter_omitted: false,
        };
        let visible_edges = visible_reduced_edges(&graph);
        let diagram = render_state_graph_mermaid(&spec, &graph, &visible_edges);

        assert!(!diagram.contains("S0 --> S0: \"Retry\""));
        assert!(diagram.contains("S0 --> S1: \"Advance\""));
        assert!(diagram.contains("S1 --> S1: \"Loop\""));
    }

    #[test]
    fn render_state_graph_section_includes_collapsed_path_details_only_when_reduced() {
        let section = render_state_graph_section(&SpecDoc {
            kind: Some(SpecKind::Subsystem),
            full_path: vec!["demo".to_owned(), "DemoSpec".to_owned()],
            tail_ident: "DemoSpec".to_owned(),
            state_ty: "DemoState".to_owned(),
            action_ty: "DemoAction".to_owned(),
            model_cases: None,
            subsystems: Vec::new(),
            registrations: BTreeMap::new(),
            doc_graphs: vec![nirvash_core::DocGraphCase {
                label: "default".to_owned(),
                graph: nirvash_core::DocGraphSnapshot {
                    states: vec![
                        nirvash_core::DocGraphState {
                            summary: "Init".to_owned(),
                            full: "Init".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                        nirvash_core::DocGraphState {
                            summary: "Middle".to_owned(),
                            full: "Middle".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                        nirvash_core::DocGraphState {
                            summary: "Done".to_owned(),
                            full: "Done".to_owned(),
                            relation_fields: Vec::new(),
                            relation_schema: Vec::new(),
                        },
                    ],
                    edges: vec![
                        vec![nirvash_core::DocGraphEdge {
                            label: "Start".to_owned(),
                            target: 1,
                        }],
                        vec![nirvash_core::DocGraphEdge {
                            label: "Finish".to_owned(),
                            target: 2,
                        }],
                        Vec::new(),
                    ],
                    initial_indices: vec![0],
                    deadlocks: vec![2],
                    truncated: false,
                    stutter_omitted: false,
                    focus_indices: Vec::new(),
                    reduction: nirvash_core::DocGraphReductionMode::BoundaryPaths,
                    max_edge_actions_in_label: 2,
                },
            }],
        });

        assert!(section.contains("Collapsed Path Details"));
        assert!(section.contains("#### S0 -> S2"));
        assert!(section.contains("##### S1"));
    }

    #[test]
    fn upper_snake_names_match_fragment_keys() {
        assert_eq!(to_upper_snake("ImagodSystemSpec"), "IMAGOD_SYSTEM_SPEC");
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
