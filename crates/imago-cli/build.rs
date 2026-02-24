use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(err) = generate_init_templates() {
        panic!("failed to generate init templates: {err:#}");
    }
}

fn generate_init_templates() -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let templates_dir = manifest_dir.join("templates").join("imago");

    println!("cargo:rerun-if-changed={}", templates_dir.display());

    if !templates_dir.exists() {
        return Err(format!(
            "templates directory does not exist: {}",
            templates_dir.display()
        )
        .into());
    }

    let mut templates = Vec::new();
    for entry in fs::read_dir(&templates_dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if !file_type.is_file() || path.extension() != Some(OsStr::new("toml")) {
            continue;
        }

        println!("cargo:rerun-if-changed={}", path.display());

        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Err(
                format!("template file stem must be valid UTF-8: {}", path.display()).into(),
            );
        };

        validate_lang_id(stem, &path)?;

        let canonical = fs::canonicalize(&path)?;
        templates.push((stem.to_string(), canonical));
    }

    if templates.is_empty() {
        return Err(format!("no templates found in {}", templates_dir.display()).into());
    }

    templates.sort_by(|a, b| a.0.cmp(&b.0));

    let mut seen = BTreeSet::new();
    for (id, _) in &templates {
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate template id detected: {id}").into());
        }
    }

    let mut output = String::new();
    output.push_str("pub const INIT_TEMPLATES: &[(&str, &str)] = &[\n");
    for (id, path) in templates {
        let path_literal = path.to_string_lossy();
        output.push_str(&format!(
            "    ({id:?}, include_str!({path:?})),\n",
            id = id,
            path = path_literal
        ));
    }
    output.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out_dir.join("init_templates.rs"), output)?;

    Ok(())
}

fn validate_lang_id(id: &str, path: &Path) -> Result<()> {
    if id.is_empty() {
        return Err(format!("template id must not be empty: {}", path.display()).into());
    }

    let Some(first) = id.chars().next() else {
        return Err(format!("template id must not be empty: {}", path.display()).into());
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "template id must start with lowercase ASCII letter or digit: {id} ({})",
            path.display()
        )
        .into());
    }

    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "template id must match [a-z0-9][a-z0-9_-]*: {id} ({})",
            path.display()
        )
        .into());
    }

    Ok(())
}
