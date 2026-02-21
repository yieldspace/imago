use super::certs::{KnownHostEntry, generate_key_material};
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
    control_admin_public_hex: String,
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
    work_dir: PathBuf,
    home_dir: PathBuf,
    storage_root: PathBuf,
    imagod_config_path: PathBuf,
    known_host: KnownHostEntry,
}

#[derive(Debug)]
struct NodeProcess {
    child: Child,
}

impl Cluster {
    pub fn new(
        workspace_root: PathBuf,
        base_dir: PathBuf,
        control_admin_public_hex: String,
    ) -> Result<Self> {
        fs::create_dir_all(&base_dir)
            .with_context(|| format!("failed to create {}", base_dir.display()))?;
        Ok(Self {
            workspace_root,
            base_dir,
            control_admin_public_hex,
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
        let config = format!(
            "listen_addr = \"127.0.0.1:{port}\"\nstorage_root = \"d\"\n\n[runtime]\nmax_chunks = 128\nchunk_timeout_ms = 10000\nidle_ttl_secs = 300\nhttp_max_body_bytes = 1048576\ntick_interval_ms = 5000\nrunner_ready_timeout_secs = 10\n\n[tls]\nserver_key = \"{}\"\nadmin_public_keys = [\"{}\"]\nclient_public_keys = [\"{}\"]\n",
            toml_escape(keys.server_key_path.to_string_lossy().as_ref()),
            self.control_admin_public_hex.as_str(),
            keys.server_public_hex.as_str(),
        );
        fs::write(&imagod_config_path, config)
            .with_context(|| format!("failed to write {}", imagod_config_path.display()))?;

        let target = TargetSpec::new(
            name,
            format!("127.0.0.1:{port}"),
            "localhost",
            "certs/control.key",
        );

        self.nodes.push(Node {
            name: name.to_string(),
            target,
            work_dir,
            home_dir,
            storage_root,
            imagod_config_path,
            known_host: KnownHostEntry {
                authority: format!("localhost:{port}"),
                public_key_hex: keys.server_public_hex,
            },
        });

        Ok(NodeHandle {
            name: name.to_string(),
        })
    }

    pub fn start_all(&mut self) -> Result<()> {
        if !self.running.is_empty() {
            return Ok(());
        }

        for node in &self.nodes {
            let mut child = Command::new("cargo")
                .arg("run")
                .arg("--quiet")
                .arg("--manifest-path")
                .arg(self.workspace_root.join("Cargo.toml"))
                .arg("-p")
                .arg("imagod")
                .arg("--")
                .arg("--config")
                .arg(&node.imagod_config_path)
                .current_dir(&node.work_dir)
                .env("HOME", &node.home_dir)
                .env("USERPROFILE", &node.home_dir)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .with_context(|| format!("failed to spawn imagod for node '{}'", node.name))?;

            let manager_socket_path = node
                .storage_root
                .join("runtime")
                .join("ipc")
                .join("manager-control.sock");
            wait_for_imagod_ready(
                &mut child,
                &manager_socket_path,
                &node.target.remote,
                Duration::from_secs(90),
            )?;
            self.running.push(NodeProcess { child });
        }

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

    pub fn known_hosts_entries(&self) -> Vec<KnownHostEntry> {
        self.nodes
            .iter()
            .map(|node| node.known_host.clone())
            .collect()
    }

    pub fn authority_for(&self, name: &str) -> Result<String> {
        self.nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| format!("rpc://{}", node.target.remote))
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
