use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub enum AppKind {
    Cli,
    Http { port: u16 },
    Rpc,
}

impl AppKind {
    pub fn as_manifest_type(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Http { .. } => "http",
            Self::Rpc => "rpc",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TargetSpec {
    pub name: String,
    pub remote: String,
    pub server_name: String,
    pub client_key_rel: String,
}

impl TargetSpec {
    pub fn new(
        name: impl Into<String>,
        remote: impl Into<String>,
        server_name: impl Into<String>,
        client_key_rel: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            remote: remote.into(),
            server_name: server_name.into(),
            client_key_rel: client_key_rel.into(),
        }
    }
}

#[derive(Debug)]
pub struct ProjectLayout {
    pub project_dir: PathBuf,
    components_dir: PathBuf,
    certs_dir: PathBuf,
}

impl ProjectLayout {
    pub fn new(base_dir: &Path, short_name: &str) -> Result<Self> {
        let project_dir = base_dir.join(short_name);
        let components_dir = project_dir.join("components");
        let certs_dir = project_dir.join("certs");
        fs::create_dir_all(&components_dir)
            .with_context(|| format!("failed to create {}", components_dir.display()))?;
        fs::create_dir_all(&certs_dir)
            .with_context(|| format!("failed to create {}", certs_dir.display()))?;

        Ok(Self {
            project_dir,
            components_dir,
            certs_dir,
        })
    }

    pub fn write_component_file(&self, file_name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.components_dir.join(file_name);
        fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    pub fn copy_control_key(&self, from: &Path) -> Result<PathBuf> {
        let to = self.certs_dir.join("control.key");
        fs::copy(from, &to)
            .with_context(|| format!("failed to copy {} -> {}", from.display(), to.display()))?;
        Ok(to)
    }

    pub fn write_imago_toml(
        &self,
        service_name: &str,
        main_wasm_path: &Path,
        app_kind: AppKind,
        default_target: &TargetSpec,
        all_targets: &[TargetSpec],
    ) -> Result<()> {
        let main_rel = if main_wasm_path.is_absolute() {
            main_wasm_path
                .strip_prefix(&self.project_dir)
                .map_err(|_| anyhow!("main path must be under project dir"))?
                .to_path_buf()
        } else {
            main_wasm_path.to_path_buf()
        };

        let mut body = format!(
            "name = \"{}\"\nmain = \"{}\"\ntype = \"{}\"\n\n[capabilities]\nwasi = true\n\n",
            toml_escape(service_name),
            toml_escape(main_rel.to_string_lossy().as_ref()),
            app_kind.as_manifest_type(),
        );

        append_target(&mut body, "default", default_target);
        for target in all_targets {
            if target.name == "default" {
                continue;
            }
            append_target(&mut body, &target.name, target);
        }

        if let AppKind::Http { port } = app_kind {
            body.push_str(&format!("\n[http]\nport = {port}\n"));
        }

        let imago_toml_path = self.project_dir.join("imago.toml");
        fs::write(&imago_toml_path, body)
            .with_context(|| format!("failed to write {}", imago_toml_path.display()))?;
        Ok(())
    }
}

fn append_target(body: &mut String, section_name: &str, target: &TargetSpec) {
    body.push_str(&format!(
        "[target.{}]\nremote = \"{}\"\nserver_name = \"{}\"\nclient_key = \"{}\"\n\n",
        toml_key(section_name),
        toml_escape(&target.remote),
        toml_escape(&target.server_name),
        toml_escape(&target.client_key_rel),
    ));
}

fn toml_key(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        value.to_string()
    } else {
        format!("\"{}\"", toml_escape(value))
    }
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
