use super::binaries::resolve_imagod_binary;
use super::certs::generate_key_material;
use super::projects::TargetSpec;
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Cluster {
    workspace_root: PathBuf,
    base_dir: PathBuf,
    daemon_package: String,
    nodes: Vec<Node>,
    running: Vec<NodeProcess>,
}

#[derive(Debug, Clone)]
pub struct NodeHandle {
    name: String,
}

impl NodeHandle {
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    target: TargetSpec,
    listen_addr: String,
    work_dir: PathBuf,
    home_dir: PathBuf,
    storage_root: PathBuf,
    imagod_config_path: PathBuf,
}

#[derive(Debug)]
struct NodeProcess {
    child: Child,
}

impl Cluster {
    pub fn new(workspace_root: PathBuf, base_dir: PathBuf) -> Result<Self> {
        Self::new_with_daemon_package(workspace_root, base_dir, "imagod")
    }

    pub fn new_with_daemon_package(
        workspace_root: PathBuf,
        base_dir: PathBuf,
        daemon_package: impl Into<String>,
    ) -> Result<Self> {
        fs::create_dir_all(&base_dir)
            .with_context(|| format!("failed to create {}", base_dir.display()))?;
        Ok(Self {
            workspace_root,
            base_dir,
            daemon_package: daemon_package.into(),
            nodes: Vec::new(),
            running: Vec::new(),
        })
    }

    pub fn add_node(&mut self, name: &str) -> Result<NodeHandle> {
        if self.nodes.iter().any(|node| node.name == name) {
            bail!("node '{name}' already exists");
        }

        let index = self.nodes.len();
        let work_dir = self.base_dir.join(format!("n{index}"));
        let cert_dir = work_dir.join("c");
        let storage_root = work_dir.join("d");
        let home_dir = work_dir.join("h");
        fs::create_dir_all(&cert_dir)
            .with_context(|| format!("failed to create {}", cert_dir.display()))?;
        fs::create_dir_all(&storage_root)
            .with_context(|| format!("failed to create {}", storage_root.display()))?;
        fs::create_dir_all(&home_dir)
            .with_context(|| format!("failed to create {}", home_dir.display()))?;

        let keys = generate_key_material(&cert_dir)?;
        let port = pick_free_port()?;
        let imagod_config_path = work_dir.join("i.toml");
        let control_socket_path = work_dir.join("control.sock");
        let config = render_imagod_config(
            port,
            control_socket_path.as_path(),
            keys.server_key_path.as_path(),
            keys.server_public_hex.as_str(),
        );
        fs::write(&imagod_config_path, config)
            .with_context(|| format!("failed to write {}", imagod_config_path.display()))?;

        let target = TargetSpec::new(
            name,
            format!(
                "ssh://localhost?socket={}",
                control_socket_path.to_string_lossy()
            ),
        );

        self.nodes.push(Node {
            name: name.to_string(),
            target,
            listen_addr: format!("127.0.0.1:{port}"),
            work_dir,
            home_dir,
            storage_root,
            imagod_config_path,
        });

        Ok(NodeHandle {
            name: name.to_string(),
        })
    }

    pub fn start_all(&mut self) -> Result<()> {
        struct NodeStartup {
            name: String,
            listen_addr: String,
            manager_socket_path: PathBuf,
            child: Child,
        }

        if !self.running.is_empty() {
            return Ok(());
        }

        let daemon_binary = resolve_imagod_binary(&self.workspace_root, &self.daemon_package)?;
        let mut startups = Vec::with_capacity(self.nodes.len());

        for node in &self.nodes {
            let child = Command::new(&daemon_binary)
                .arg("--config")
                .arg(&node.imagod_config_path)
                .current_dir(&node.work_dir)
                .env("HOME", &node.home_dir)
                .env("USERPROFILE", &node.home_dir)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .with_context(|| format!("failed to spawn imagod for node '{}'", node.name))?;

            startups.push(NodeStartup {
                name: node.name.clone(),
                listen_addr: node.listen_addr.clone(),
                manager_socket_path: node
                    .storage_root
                    .join("runtime")
                    .join("ipc")
                    .join("manager-control.sock"),
                child,
            });
        }

        for index in 0..startups.len() {
            let failure = {
                let startup = &mut startups[index];
                wait_for_imagod_ready(
                    &mut startup.child,
                    &startup.manager_socket_path,
                    &startup.listen_addr,
                    Duration::from_secs(90),
                )
                .err()
                .map(|err| (startup.name.clone(), err))
            };
            if let Some((node_name, err)) = failure {
                for pending in &mut startups {
                    let _ = pending.child.kill();
                    let _ = pending.child.wait();
                }
                return Err(err).with_context(|| {
                    format!("imagod readiness check failed for node '{}'", node_name)
                });
            }
        }

        self.running
            .extend(startups.into_iter().map(|startup| NodeProcess {
                child: startup.child,
            }));

        Ok(())
    }

    pub fn targets(&self) -> Vec<TargetSpec> {
        self.nodes.iter().map(|node| node.target.clone()).collect()
    }

    pub fn has_target(&self, name: &str) -> bool {
        self.nodes.iter().any(|node| node.target.name == name)
    }

    pub fn target(&self, name: &str) -> Result<TargetSpec> {
        self.nodes
            .iter()
            .find(|node| node.target.name == name)
            .map(|node| node.target.clone())
            .ok_or_else(|| anyhow!("unknown target '{name}'"))
    }

    pub fn authority_for(&self, name: &str) -> Result<String> {
        self.nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| format!("rpc://{}", node.listen_addr))
            .ok_or_else(|| anyhow!("unknown node '{name}'"))
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        for proc in &mut self.running {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }
}

fn wait_for_imagod_ready(
    child: &mut Child,
    manager_socket_path: &Path,
    listen_addr: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() > deadline {
            bail!(
                "timed out waiting for imagod readiness: socket={}, listen_addr={}",
                manager_socket_path.display(),
                listen_addr
            );
        }

        if manager_socket_path.exists() || is_tcp_listening(listen_addr) {
            return Ok(());
        }

        if let Some(status) = child
            .try_wait()
            .context("failed while waiting for imagod")?
        {
            bail!("imagod exited before ready: {status}");
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn is_tcp_listening(listen_addr: &str) -> bool {
    TcpStream::connect_timeout(
        &match listen_addr.parse() {
            Ok(addr) => addr,
            Err(_) => return false,
        },
        Duration::from_millis(150),
    )
    .is_ok()
}

fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to pick free port")?;
    Ok(listener.local_addr()?.port())
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn render_imagod_config(
    port: u16,
    control_socket_path: &Path,
    server_key_path: &Path,
    server_public_hex: &str,
) -> String {
    format!(
        "listen_addr = \"127.0.0.1:{port}\"\ncontrol_socket_path = \"{}\"\nstorage_root = \"d\"\n\n[runtime]\nmax_chunks = 128\nchunk_timeout_ms = 10000\nidle_ttl_secs = 300\nhttp_max_body_bytes = 1048576\ntick_interval_ms = 5000\nrunner_ready_timeout_secs = 30\nhttp_queue_memory_budget_bytes = 67108864\nboot_plugin_gc_enabled = false\nboot_restore_enabled = false\n\n[tls]\nserver_key = \"{}\"\nclient_public_keys = [\"{}\"]\nknown_public_keys = {{}}\n",
        toml_escape(control_socket_path.to_string_lossy().as_ref()),
        toml_escape(server_key_path.to_string_lossy().as_ref()),
        server_public_hex,
    )
}

#[cfg(test)]
mod tests {
    use super::render_imagod_config;
    use std::path::Path;

    #[test]
    fn render_imagod_config_sets_absolute_control_socket_path() {
        let config = render_imagod_config(
            4443,
            Path::new("/tmp/imago-e2e/n0/control.sock"),
            Path::new("/tmp/imago-e2e/n0/c/server.key"),
            "client-public-key",
        );

        assert!(config.contains("control_socket_path = \"/tmp/imago-e2e/n0/control.sock\""));
        assert!(!config.contains("/run/imago/imagod.sock"));
        assert!(!config.contains("admin_public_keys"));
    }
}
