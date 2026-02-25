use super::certs::{generate_key_material, write_known_hosts};
use super::cli::{CmdOutput, run_imago_cli};
use super::cluster::Cluster;
use super::http::wait_http_response;
use super::projects::{AppKind, ProjectLayout, TargetSpec};
use super::wasm_assets::{WasmArtifact, wasm_file_name, wasm_path};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::{Builder as TempDirBuilder, TempDir};

pub type TestResult<T = ()> = Result<T>;

#[derive(Debug)]
struct Service {
    name: String,
    app_kind: AppKind,
    project: ProjectLayout,
    default_target_name: String,
    targets: Vec<TargetSpec>,
}

#[derive(Debug, Clone)]
pub struct ServiceHandle {
    service_name: String,
}

impl ServiceHandle {
    pub fn name(&self) -> &str {
        &self.service_name
    }

    pub fn deploy(&self, scenario: &Scenario, target: &str) -> TestResult<CmdOutput> {
        scenario.deploy_service(&self.service_name, target)
    }

    pub fn replace_wasm(&self, scenario: &mut Scenario, wasm: WasmArtifact) -> TestResult<()> {
        scenario.replace_service_wasm(&self.service_name, wasm)
    }

    pub fn stop(&self, scenario: &Scenario, target: &str) -> TestResult<CmdOutput> {
        scenario.stop(&self.service_name, target)
    }

    pub fn logs(&self, scenario: &Scenario, target: &str, tail: u32) -> TestResult<CmdOutput> {
        scenario.logs(&self.service_name, target, tail)
    }

    pub fn append_imago_toml(&self, scenario: &Scenario, body: &str) -> TestResult<()> {
        scenario.append_service_imago_toml(&self.service_name, body)
    }

    pub fn write_dotenv(&self, scenario: &Scenario, body: &str) -> TestResult<()> {
        scenario.write_service_dotenv(&self.service_name, body)
    }
}

pub struct Scenario {
    workspace_root: PathBuf,
    _temp_dir: TempDir,
    control_home: PathBuf,
    control_admin_key_path: PathBuf,
    cluster: Cluster,
    projects_base_dir: PathBuf,
    services: BTreeMap<String, Service>,
    current_service: Option<String>,
}

impl Scenario {
    pub fn new(test_name: &str) -> TestResult<Self> {
        Self::new_with_daemon_package(test_name, "imagod")
    }

    pub fn new_with_daemon_package(test_name: &str, daemon_package: &str) -> TestResult<Self> {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let prefix = short_prefix(test_name);
        let temp_dir = TempDirBuilder::new().prefix(&prefix).tempdir()?;
        let root = temp_dir.path().to_path_buf();

        let control_dir = root.join("c");
        fs::create_dir_all(&control_dir)?;
        let control_keys = generate_key_material(&control_dir)?;

        let control_home = root.join("h");
        fs::create_dir_all(&control_home)?;

        let cluster = Cluster::new_with_daemon_package(
            workspace_root.clone(),
            root.join("n"),
            control_keys.admin_public_hex,
            daemon_package,
        )?;
        let projects_base_dir = root.join("p");
        fs::create_dir_all(&projects_base_dir)?;

        Ok(Self {
            workspace_root,
            _temp_dir: temp_dir,
            control_home,
            control_admin_key_path: control_keys.admin_key_path,
            cluster,
            projects_base_dir,
            services: BTreeMap::new(),
            current_service: None,
        })
    }

    pub fn cluster(&mut self) -> &mut Cluster {
        &mut self.cluster
    }

    pub fn node_authority(&self, node_name: &str) -> TestResult<String> {
        self.cluster.authority_for(node_name)
    }

    pub fn add_service(
        &mut self,
        service_name: &str,
        app: AppKind,
        target: &str,
        wasm: WasmArtifact,
    ) -> TestResult<ServiceHandle> {
        if !self.cluster.has_target(target) {
            bail!("unknown target '{target}'");
        }
        if self.services.contains_key(service_name) {
            bail!("service '{service_name}' already exists");
        }

        let service_index = self.services.len();
        let project = ProjectLayout::new(&self.projects_base_dir, &format!("s{service_index}"))?;
        let _ = project.copy_control_key(&self.control_admin_key_path)?;

        let main_path = copy_wasm_to_project(&project, wasm)?;

        let targets = self.cluster.targets();
        let default_target = targets
            .iter()
            .find(|item| item.name == target)
            .cloned()
            .ok_or_else(|| anyhow!("unknown target '{target}'"))?;
        project.write_imago_toml(service_name, &main_path, app, &default_target, &targets)?;

        write_known_hosts(&self.control_home, &self.cluster.known_hosts_entries())?;

        self.services.insert(
            service_name.to_string(),
            Service {
                name: service_name.to_string(),
                app_kind: app,
                project,
                default_target_name: target.to_string(),
                targets,
            },
        );
        self.current_service = Some(service_name.to_string());

        Ok(ServiceHandle {
            service_name: service_name.to_string(),
        })
    }

    pub fn deploy(&self, target: &str) -> TestResult<CmdOutput> {
        let service_name = self
            .current_service
            .as_deref()
            .ok_or_else(|| anyhow!("no current service"))?;
        self.deploy_service(service_name, target)
    }

    pub fn deploy_service(&self, service_name: &str, target: &str) -> TestResult<CmdOutput> {
        let service = self.service(service_name)?;
        ensure_target_exists(service, target)?;
        let args = ["deploy", "--target", target, "--detach"];
        let output = self.run_service_cli(service_name, &args)?;
        output.ensure_success(&args)?;
        Ok(output)
    }

    pub fn stop(&self, service_name: &str, target: &str) -> TestResult<CmdOutput> {
        let service = self.service(service_name)?;
        ensure_target_exists(service, target)?;
        let args = ["stop", service_name, "--target", target];
        let output = self.run_service_cli(service_name, &args)?;
        output.ensure_success(&args)?;
        Ok(output)
    }

    pub fn logs(&self, service_name: &str, target: &str, tail: u32) -> TestResult<CmdOutput> {
        let service = self.service(service_name)?;
        ensure_target_exists(service, target)?;
        let tail_text = tail.to_string();
        let args = ["logs", service_name, "--tail", tail_text.as_str()];
        let output = self.run_service_cli(service_name, &args)?;
        output.ensure_success(&args)?;
        Ok(output)
    }

    pub fn bindings_cert_deploy(
        &self,
        from: &str,
        to: &str,
        via_target: &str,
    ) -> TestResult<CmdOutput> {
        let service_name = self
            .current_service
            .as_deref()
            .ok_or_else(|| anyhow!("no current service"))?;
        let service = self.service(service_name)?;
        ensure_target_exists(service, via_target)?;

        let args = ["bindings", "cert", "deploy", "--from", from, "--to", to];
        let output = self.run_service_cli(service_name, &args)?;
        output.ensure_success(&args)?;
        Ok(output)
    }

    pub fn run_service_cli(&self, service_name: &str, args: &[&str]) -> TestResult<CmdOutput> {
        let service = self.service(service_name)?;
        run_imago_cli(
            &self.workspace_root,
            &service.project.project_dir,
            &self.control_home,
            args,
        )
    }

    pub fn wait_http_response(&self, port: u16, timeout: Duration) -> TestResult<String> {
        wait_http_response(port, timeout)
    }

    fn replace_service_wasm(&mut self, service_name: &str, wasm: WasmArtifact) -> TestResult<()> {
        let service = self.service_mut(service_name)?;
        let main_path = copy_wasm_to_project(&service.project, wasm)?;
        let default_target = service
            .targets
            .iter()
            .find(|item| item.name == service.default_target_name)
            .cloned()
            .ok_or_else(|| anyhow!("unknown default target '{}'", service.default_target_name))?;
        service.project.write_imago_toml(
            &service.name,
            &main_path,
            service.app_kind,
            &default_target,
            &service.targets,
        )?;
        Ok(())
    }

    fn append_service_imago_toml(&self, service_name: &str, body: &str) -> TestResult<()> {
        let service = self.service(service_name)?;
        let imago_toml_path = service.project.project_dir.join("imago.toml");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&imago_toml_path)
            .with_context(|| format!("failed to open {}", imago_toml_path.display()))?;
        file.write_all(body.as_bytes())
            .with_context(|| format!("failed to append {}", imago_toml_path.display()))?;
        Ok(())
    }

    fn write_service_dotenv(&self, service_name: &str, body: &str) -> TestResult<()> {
        let service = self.service(service_name)?;
        let dotenv_path = service.project.project_dir.join(".env");
        fs::write(&dotenv_path, body)
            .with_context(|| format!("failed to write {}", dotenv_path.display()))?;
        Ok(())
    }

    fn service(&self, name: &str) -> TestResult<&Service> {
        self.services
            .get(name)
            .ok_or_else(|| anyhow!("unknown service '{name}'"))
    }

    fn service_mut(&mut self, name: &str) -> TestResult<&mut Service> {
        self.services
            .get_mut(name)
            .ok_or_else(|| anyhow!("unknown service '{name}'"))
    }
}

fn ensure_target_exists(service: &Service, target: &str) -> TestResult<()> {
    if service.targets.iter().any(|item| item.name == target) || target == "default" {
        return Ok(());
    }
    bail!("unknown target '{target}'");
}

fn short_prefix(test_name: &str) -> String {
    let mut out = String::from("ie");
    for ch in test_name.chars() {
        if ch.is_ascii_lowercase() {
            out.push(ch);
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

fn copy_wasm_to_project(project: &ProjectLayout, artifact: WasmArtifact) -> TestResult<PathBuf> {
    let source = wasm_path(artifact)?;
    let bytes = fs::read(&source)
        .with_context(|| format!("failed to read wasm artifact {}", source.display()))?;
    let file_name = wasm_file_name(artifact);
    project
        .write_component_file(file_name, &bytes)
        .with_context(|| format!("failed to install wasm artifact {file_name}"))
}
