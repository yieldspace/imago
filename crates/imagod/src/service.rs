use std::{
    env,
    ffi::OsStr,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, anyhow, bail};
use imagod_config::resolve_config_path;

const DEFAULT_SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/imagod.service";
const DEFAULT_INITD_SCRIPT_PATH: &str = "/etc/init.d/imagod";
const DEFAULT_LAUNCHD_PLIST_PATH: &str = "/Library/LaunchDaemons/imagod.plist";
const SERVICE_NAME: &str = "imagod";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceManager {
    Systemd,
    Initd,
    Launchd,
}

impl ServiceManager {
    fn label(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Initd => "initd",
            Self::Launchd => "launchd",
        }
    }
}

impl fmt::Display for ServiceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

pub(crate) fn install(config_path: Option<PathBuf>) -> Result<ServiceManager, anyhow::Error> {
    let manager = detect_service_manager()?;
    if manager == ServiceManager::Launchd && !is_root_user() {
        bail!("launchd service install requires root privileges");
    }

    let current_exe =
        env::current_exe().context("failed to resolve current imagod executable path")?;
    let config_path = resolve_service_config_path(config_path)?;

    match manager {
        ServiceManager::Systemd => install_systemd(&current_exe, &config_path)?,
        ServiceManager::Initd => install_initd(&current_exe, &config_path)?,
        ServiceManager::Launchd => install_launchd(&current_exe, &config_path)?,
    }

    Ok(manager)
}

pub(crate) fn uninstall() -> Result<Vec<ServiceManager>, anyhow::Error> {
    let installed = installed_managers();
    for manager in &installed {
        match manager {
            ServiceManager::Systemd => uninstall_systemd()?,
            ServiceManager::Initd => uninstall_initd()?,
            ServiceManager::Launchd => uninstall_launchd()?,
        }
    }
    Ok(installed)
}

pub(crate) fn restart_hint() -> Option<String> {
    installed_managers().first().map(|manager| match manager {
        ServiceManager::Systemd => "sudo systemctl restart imagod.service".to_string(),
        ServiceManager::Initd => "sudo service imagod restart".to_string(),
        ServiceManager::Launchd => "sudo launchctl kickstart -k system/imagod".to_string(),
    })
}

fn detect_service_manager() -> Result<ServiceManager, anyhow::Error> {
    if let Some(forced) = env::var_os("IMAGOD_TEST_SERVICE_MANAGER") {
        return match forced.to_string_lossy().as_ref() {
            "systemd" => Ok(ServiceManager::Systemd),
            "initd" => Ok(ServiceManager::Initd),
            "launchd" => Ok(ServiceManager::Launchd),
            "none" => Err(anyhow!("no supported init system detected")),
            other => Err(anyhow!(
                "IMAGOD_TEST_SERVICE_MANAGER must be one of: systemd, initd, launchd, none (got {other})"
            )),
        };
    }

    if cfg!(target_os = "macos") {
        if command_exists("launchctl") {
            return Ok(ServiceManager::Launchd);
        }
        bail!("launchctl not found; cannot install imagod as a launchd service");
    }

    if cfg!(target_os = "linux") {
        if command_exists("systemctl") && service_manager_probe_dir().is_dir() {
            return Ok(ServiceManager::Systemd);
        }
        if initd_root_dir().is_dir() {
            return Ok(ServiceManager::Initd);
        }
        bail!("no supported init system detected");
    }

    bail!("service install is not supported on this operating system")
}

fn installed_managers() -> Vec<ServiceManager> {
    let mut managers = Vec::new();
    if systemd_unit_path().exists() {
        managers.push(ServiceManager::Systemd);
    }
    if initd_script_path().exists() || rc_links_exist() {
        managers.push(ServiceManager::Initd);
    }
    if launchd_plist_path().exists() {
        managers.push(ServiceManager::Launchd);
    }
    managers
}

fn resolve_service_config_path(config_path: Option<PathBuf>) -> Result<PathBuf, anyhow::Error> {
    let config_path = resolve_config_path(config_path);
    if config_path.is_absolute() {
        return Ok(config_path);
    }

    let cwd = env::current_dir().context("failed to resolve current directory for --config")?;
    Ok(cwd.join(config_path))
}

fn install_systemd(current_exe: &Path, config_path: &Path) -> Result<(), anyhow::Error> {
    write_text_file(
        &systemd_unit_path(),
        &render_systemd_unit(current_exe, config_path),
        0o644,
    )?;
    run_command("systemctl", ["daemon-reload"])?;
    run_command("systemctl", ["enable", "--now", "imagod.service"])?;
    Ok(())
}

fn uninstall_systemd() -> Result<(), anyhow::Error> {
    let unit_path = systemd_unit_path();
    if !unit_path.exists() {
        return Ok(());
    }

    let _ = run_command("systemctl", ["disable", "--now", "imagod.service"]);
    remove_file_if_exists(&unit_path)?;
    let _ = run_command("systemctl", ["daemon-reload"]);
    Ok(())
}

fn install_initd(current_exe: &Path, config_path: &Path) -> Result<(), anyhow::Error> {
    write_text_file(
        &initd_script_path(),
        &render_initd_script(current_exe, config_path),
        0o755,
    )?;
    if command_exists("update-rc.d") {
        run_command("update-rc.d", ["imagod", "defaults"])?;
    } else if command_exists("chkconfig") {
        run_command("chkconfig", ["--add", "imagod"])?;
    }

    if command_exists("service") {
        run_command("service", ["imagod", "start"])?;
    } else {
        run_command(initd_script_path(), ["start"])?;
    }
    Ok(())
}

fn uninstall_initd() -> Result<(), anyhow::Error> {
    let script_path = initd_script_path();
    if !script_path.exists() && !rc_links_exist() {
        return Ok(());
    }

    if script_path.exists() {
        if command_exists("service") {
            let _ = run_command("service", ["imagod", "stop"]);
        } else {
            let _ = run_command(&script_path, ["stop"]);
        }
    }

    if command_exists("update-rc.d") {
        let _ = run_command("update-rc.d", ["-f", "imagod", "remove"]);
    } else if command_exists("chkconfig") {
        let _ = run_command("chkconfig", ["--del", "imagod"]);
    }

    remove_rc_links()?;
    remove_file_if_exists(&script_path)?;
    Ok(())
}

fn install_launchd(current_exe: &Path, config_path: &Path) -> Result<(), anyhow::Error> {
    let plist_path = launchd_plist_path();
    write_text_file(
        &plist_path,
        &render_launchd_plist(current_exe, config_path),
        0o644,
    )?;
    let plist = plist_path.to_string_lossy().to_string();
    let _ = run_command("launchctl", ["bootout", "system", &plist]);
    run_command("launchctl", ["bootstrap", "system", &plist])?;
    run_command("launchctl", ["enable", "system/imagod"])?;
    run_command("launchctl", ["kickstart", "-k", "system/imagod"])?;
    Ok(())
}

fn uninstall_launchd() -> Result<(), anyhow::Error> {
    let plist_path = launchd_plist_path();
    if !plist_path.exists() {
        return Ok(());
    }

    let plist = plist_path.to_string_lossy().to_string();
    let _ = run_command("launchctl", ["bootout", "system", &plist]);
    remove_file_if_exists(&plist_path)?;
    Ok(())
}

fn render_systemd_unit(current_exe: &Path, config_path: &Path) -> String {
    format!(
        "[Unit]\n\
Description=imagod daemon\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={} --config {}\n\
RuntimeDirectory=imago\n\
RuntimeDirectoryMode=0755\n\
Restart=on-failure\n\
RestartSec=2\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n",
        systemd_quote_arg(current_exe),
        systemd_quote_arg(config_path)
    )
}

fn systemd_quote_arg(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for ch in path.as_os_str().to_string_lossy().chars() {
        match ch {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn render_initd_script(current_exe: &Path, config_path: &Path) -> String {
    format!(
        "#!/bin/sh\n\
### BEGIN INIT INFO\n\
# Provides:          imagod\n\
# Required-Start:    $remote_fs $network\n\
# Required-Stop:     $remote_fs $network\n\
# Default-Start:     2 3 4 5\n\
# Default-Stop:      0 1 6\n\
# Short-Description: imagod daemon\n\
### END INIT INFO\n\
\n\
DAEMON='{}'\n\
CONFIG_PATH='{}'\n\
PIDFILE=\"/var/run/imagod.pid\"\n\
NAME=\"imagod\"\n\
\n\
start() {{\n\
  mkdir -p /run/imago\n\
  if command -v start-stop-daemon >/dev/null 2>&1; then\n\
    start-stop-daemon --start --quiet --background --make-pidfile --pidfile \"$PIDFILE\" --exec \"$DAEMON\" -- --config \"$CONFIG_PATH\"\n\
    return $?\n\
  fi\n\
\n\
  \"$DAEMON\" --config \"$CONFIG_PATH\" >/dev/null 2>&1 &\n\
  echo $! > \"$PIDFILE\"\n\
}}\n\
\n\
stop() {{\n\
  if command -v start-stop-daemon >/dev/null 2>&1; then\n\
    start-stop-daemon --stop --quiet --pidfile \"$PIDFILE\" --retry 5\n\
    rm -f \"$PIDFILE\"\n\
    return $?\n\
  fi\n\
\n\
  if [ -f \"$PIDFILE\" ]; then\n\
    kill \"$(cat \"$PIDFILE\")\" >/dev/null 2>&1 || true\n\
    rm -f \"$PIDFILE\"\n\
  fi\n\
}}\n\
\n\
status() {{\n\
  if [ -f \"$PIDFILE\" ] && kill -0 \"$(cat \"$PIDFILE\")\" >/dev/null 2>&1; then\n\
    echo \"$NAME is running\"\n\
    exit 0\n\
  fi\n\
  echo \"$NAME is not running\"\n\
  exit 3\n\
}}\n\
\n\
case \"$1\" in\n\
  start) start ;;\n\
  stop) stop ;;\n\
  restart) stop; start ;;\n\
  status) status ;;\n\
  *) echo \"Usage: /etc/init.d/$NAME {{start|stop|restart|status}}\"; exit 1 ;;\n\
esac\n",
        shell_single_quote(current_exe),
        shell_single_quote(config_path)
    )
}

fn shell_single_quote(path: &Path) -> String {
    path.as_os_str().to_string_lossy().replace('\'', "'\\''")
}

fn render_launchd_plist(current_exe: &Path, config_path: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n\
  <string>imagod</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{}</string>\n\
    <string>--config</string>\n\
    <string>{}</string>\n\
  </array>\n\
  <key>RunAtLoad</key>\n\
  <true/>\n\
  <key>KeepAlive</key>\n\
  <true/>\n\
</dict>\n\
</plist>\n",
        xml_escape(current_exe),
        xml_escape(config_path)
    )
}

fn xml_escape(path: &Path) -> String {
    let mut escaped = String::new();
    for ch in path.as_os_str().to_string_lossy().chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn run_command<I, S>(program: impl AsRef<OsStr>, args: I) -> Result<(), anyhow::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let output = Command::new(program.as_ref())
        .args(&args)
        .output()
        .with_context(|| {
            format!(
                "failed to start command {}",
                Path::new(program.as_ref()).display()
            )
        })?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "command {} {:?} failed with status {}: {}",
        Path::new(program.as_ref()).display(),
        args,
        output.status,
        stderr.trim()
    ))
}

fn command_exists(program: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|entry| entry.join(program).exists())
}

fn write_text_file(path: &Path, contents: &str, mode: u32) -> Result<(), anyhow::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("service path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("failed to set mode on {}", path.display()))?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<bool, anyhow::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn rc_links_exist() -> bool {
    rc_directories().into_iter().any(|dir| {
        matching_rc_entries(&dir)
            .map(|entries| !entries.is_empty())
            .unwrap_or(false)
    })
}

fn remove_rc_links() -> Result<(), anyhow::Error> {
    for dir in rc_directories() {
        for entry in matching_rc_entries(&dir)? {
            remove_file_if_exists(&entry)?;
        }
    }
    Ok(())
}

fn matching_rc_entries(dir: &Path) -> Result<Vec<PathBuf>, anyhow::Error> {
    let mut entries = Vec::new();
    if !dir.exists() {
        return Ok(entries);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to inspect {}", dir.display()))?;
        let file_name = entry.file_name();
        if is_imagod_rc_link_name(&file_name.to_string_lossy()) {
            entries.push(entry.path());
        }
    }
    Ok(entries)
}

fn is_imagod_rc_link_name(file_name: &str) -> bool {
    let Some(service_name) = file_name.get(3..) else {
        return false;
    };
    file_name.len() == SERVICE_NAME.len() + 3
        && matches!(file_name.as_bytes()[0], b'S' | b'K')
        && file_name.as_bytes()[1].is_ascii_digit()
        && file_name.as_bytes()[2].is_ascii_digit()
        && service_name == SERVICE_NAME
}

fn service_manager_probe_dir() -> PathBuf {
    env::var_os("IMAGOD_TEST_SYSTEMD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/systemd/system"))
}

fn initd_root_dir() -> PathBuf {
    initd_script_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/etc/init.d"))
}

fn systemd_unit_path() -> PathBuf {
    env::var_os("IMAGOD_TEST_SYSTEMD_UNIT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SYSTEMD_UNIT_PATH))
}

fn initd_script_path() -> PathBuf {
    env::var_os("IMAGOD_TEST_INITD_SCRIPT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_INITD_SCRIPT_PATH))
}

fn launchd_plist_path() -> PathBuf {
    env::var_os("IMAGOD_TEST_LAUNCHD_PLIST_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LAUNCHD_PLIST_PATH))
}

fn rc_directories() -> Vec<PathBuf> {
    if let Some(root) = env::var_os("IMAGOD_TEST_RC_ROOT") {
        let root = PathBuf::from(root);
        return [
            "rc0.d", "rc1.d", "rc2.d", "rc3.d", "rc4.d", "rc5.d", "rc6.d",
        ]
        .into_iter()
        .map(|dir| root.join(dir))
        .collect();
    }

    [
        "/etc/rc0.d",
        "/etc/rc1.d",
        "/etc/rc2.d",
        "/etc/rc3.d",
        "/etc/rc4.d",
        "/etc/rc5.d",
        "/etc/rc6.d",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn is_root_user() -> bool {
    if let Some(value) = env::var_os("IMAGOD_TEST_IS_ROOT") {
        return value == "1";
    }
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct EnvGuard {
        saved: Vec<(String, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&str, &Path)]) -> Self {
            let mut saved = Vec::with_capacity(vars.len());
            for (key, value) in vars {
                saved.push(((*key).to_string(), env::var_os(key)));
                // SAFETY: tests serialize environment updates via env_lock.
                unsafe { env::set_var(key, value) };
            }
            Self { saved }
        }

        fn set_str(vars: &[(&str, &str)]) -> Self {
            let mut saved = Vec::with_capacity(vars.len());
            for (key, value) in vars {
                saved.push(((*key).to_string(), env::var_os(key)));
                // SAFETY: tests serialize environment updates via env_lock.
                unsafe { env::set_var(key, value) };
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..).rev() {
                match value {
                    Some(value) => {
                        // SAFETY: tests serialize environment updates via env_lock.
                        unsafe { env::set_var(&key, value) };
                    }
                    None => {
                        // SAFETY: tests serialize environment updates via env_lock.
                        unsafe { env::remove_var(&key) };
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "imagod-service-tests-{label}-{}-",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock should be after epoch")
                    .as_nanos()
            ))
            .tempdir()
            .expect("temp dir should be created")
    }

    struct CurrentDirGuard {
        saved: PathBuf,
    }

    impl CurrentDirGuard {
        fn set(path: &Path) -> Self {
            let saved = env::current_dir().expect("current dir should resolve");
            env::set_current_dir(path).expect("current dir should change");
            Self { saved }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            env::set_current_dir(&self.saved).expect("current dir should restore");
        }
    }

    fn write_stub_command(path: &Path, log_path: &Path) {
        fs::write(
            path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\n",
                log_path.display()
            ),
        )
        .expect("stub command should be written");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("stub command should be executable");
    }

    fn current_test_binary() -> PathBuf {
        env::current_exe().expect("current exe should resolve")
    }

    #[test]
    fn render_systemd_unit_includes_execstart_and_config() {
        let rendered = render_systemd_unit(Path::new("/tmp/imagod"), Path::new("/tmp/imagod.toml"));
        assert!(rendered.contains("ExecStart=\"/tmp/imagod\" --config \"/tmp/imagod.toml\""));
        assert!(rendered.contains("RuntimeDirectory=imago"));
    }

    #[test]
    fn render_initd_script_includes_execstart_and_config() {
        let rendered = render_initd_script(Path::new("/tmp/imagod"), Path::new("/tmp/imagod.toml"));
        assert!(rendered.contains("DAEMON='/tmp/imagod'"));
        assert!(rendered.contains("CONFIG_PATH='/tmp/imagod.toml'"));
        assert!(rendered.contains("-- --config \"$CONFIG_PATH\""));
        assert!(rendered.contains("\"$DAEMON\" --config \"$CONFIG_PATH\""));
    }

    #[test]
    fn render_launchd_plist_includes_execstart_and_config() {
        let rendered =
            render_launchd_plist(Path::new("/tmp/imagod"), Path::new("/tmp/imagod.toml"));
        assert!(rendered.contains("<string>/tmp/imagod</string>"));
        assert!(rendered.contains("<string>/tmp/imagod.toml</string>"));
    }

    #[test]
    fn render_launchd_plist_escapes_xml_metacharacters() {
        let rendered = render_launchd_plist(
            Path::new("/tmp/imago&d"),
            Path::new("/tmp/config<prod>.toml"),
        );
        assert!(rendered.contains("<string>/tmp/imago&amp;d</string>"));
        assert!(rendered.contains("<string>/tmp/config&lt;prod&gt;.toml</string>"));
    }

    #[test]
    fn render_systemd_unit_quotes_paths_with_spaces() {
        let rendered = render_systemd_unit(
            Path::new("/tmp/imagod binary"),
            Path::new("/tmp/imagod config.toml"),
        );
        assert!(
            rendered
                .contains("ExecStart=\"/tmp/imagod binary\" --config \"/tmp/imagod config.toml\"")
        );
    }

    #[test]
    fn render_initd_script_quotes_config_path_with_spaces() {
        let rendered = render_initd_script(
            Path::new("/tmp/imagod"),
            Path::new("/tmp/path with spaces/imagod.toml"),
        );
        assert!(rendered.contains("CONFIG_PATH='/tmp/path with spaces/imagod.toml'"));
        assert!(rendered.contains("-- --config \"$CONFIG_PATH\""));
        assert!(rendered.contains("\"$DAEMON\" --config \"$CONFIG_PATH\""));
    }

    #[test]
    fn is_imagod_rc_link_name_matches_only_exact_service_links() {
        assert!(is_imagod_rc_link_name("S50imagod"));
        assert!(is_imagod_rc_link_name("K01imagod"));
        assert!(!is_imagod_rc_link_name("S20myimagod"));
        assert!(!is_imagod_rc_link_name("imagod"));
        assert!(!is_imagod_rc_link_name("S5imagod"));
    }

    #[test]
    fn resolve_service_config_path_makes_relative_paths_absolute() {
        let _env_lock = env_lock();
        let root = temp_dir("relative-config");
        let _cwd = CurrentDirGuard::set(root.path());

        let resolved = resolve_service_config_path(Some(PathBuf::from("imagod.toml")))
            .expect("relative config path should resolve");
        assert!(resolved.is_absolute());
        assert_eq!(resolved.file_name(), Some(OsStr::new("imagod.toml")));
        assert_eq!(
            fs::canonicalize(
                resolved
                    .parent()
                    .expect("absolute path should have a parent directory")
            )
            .expect("resolved parent should canonicalize"),
            fs::canonicalize(root.path()).expect("temp dir should canonicalize")
        );
    }

    #[test]
    fn detect_service_manager_respects_override() {
        let _env_lock = env_lock();
        let _guard = EnvGuard::set_str(&[("IMAGOD_TEST_SERVICE_MANAGER", "initd")]);
        assert_eq!(
            detect_service_manager().expect("manager should resolve"),
            ServiceManager::Initd
        );
    }

    #[test]
    fn install_systemd_writes_unit_and_runs_enable_flow() {
        let _env_lock = env_lock();
        let root = temp_dir("systemd-install");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let unit_path = root.path().join("imagod.service");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_stub_command(&bin_dir.join("systemctl"), &log_path);
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let config_path = root.path().join("imagod.toml");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_SYSTEMD_UNIT_PATH", &unit_path),
            ("IMAGOD_TEST_SERVICE_MANAGER", Path::new("systemd")),
            ("PATH", Path::new(&path_value)),
        ]);

        let manager = install(Some(config_path.clone())).expect("install should succeed");
        assert_eq!(manager, ServiceManager::Systemd);
        let unit = fs::read_to_string(&unit_path).expect("unit should exist");
        assert!(unit.contains(&format!(
            "ExecStart=\"{}\" --config \"{}\"",
            current_test_binary().display(),
            config_path.display()
        )));
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("systemctl daemon-reload"));
        assert!(commands.contains("systemctl enable --now imagod.service"));
    }

    #[test]
    fn uninstall_systemd_is_idempotent_when_unit_missing() {
        let _env_lock = env_lock();
        let root = temp_dir("systemd-uninstall-missing");
        let unit_path = root.path().join("imagod.service");
        let _guard = EnvGuard::set(&[("IMAGOD_TEST_SYSTEMD_UNIT_PATH", &unit_path)]);

        let removed = uninstall().expect("uninstall should succeed");
        assert!(removed.is_empty());
    }

    #[test]
    fn uninstall_systemd_removes_unit_and_reload() {
        let _env_lock = env_lock();
        let root = temp_dir("systemd-uninstall");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let unit_path = root.path().join("imagod.service");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_stub_command(&bin_dir.join("systemctl"), &log_path);
        fs::write(&unit_path, "unit").expect("unit should exist");
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_SYSTEMD_UNIT_PATH", &unit_path),
            ("PATH", Path::new(&path_value)),
        ]);

        let removed = uninstall().expect("uninstall should succeed");
        assert_eq!(removed, vec![ServiceManager::Systemd]);
        assert!(!unit_path.exists());
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("systemctl disable --now imagod.service"));
        assert!(commands.contains("systemctl daemon-reload"));
    }

    #[test]
    fn uninstall_systemd_ignores_daemon_reload_failure_after_removal() {
        let _env_lock = env_lock();
        let root = temp_dir("systemd-uninstall-daemon-reload-failure");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let unit_path = root.path().join("imagod.service");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        fs::write(
            bin_dir.join("systemctl"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nif [ \"${{1:-}}\" = \"daemon-reload\" ]; then\n  exit 1\nfi\n",
                log_path.display()
            ),
        )
        .expect("stub command should be written");
        fs::set_permissions(bin_dir.join("systemctl"), fs::Permissions::from_mode(0o755))
            .expect("stub command should be executable");
        fs::write(&unit_path, "unit").expect("unit should exist");
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_SYSTEMD_UNIT_PATH", &unit_path),
            ("PATH", Path::new(&path_value)),
        ]);

        let removed = uninstall().expect("uninstall should still succeed");
        assert_eq!(removed, vec![ServiceManager::Systemd]);
        assert!(!unit_path.exists());
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("systemctl disable --now imagod.service"));
        assert!(commands.contains("systemctl daemon-reload"));
    }

    #[test]
    fn install_initd_writes_script_and_runs_registration_flow() {
        let _env_lock = env_lock();
        let root = temp_dir("initd-install");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let initd_path = root.path().join("init.d").join("imagod");
        let rc_root = root.path().join("rc");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_stub_command(&bin_dir.join("update-rc.d"), &log_path);
        write_stub_command(&bin_dir.join("service"), &log_path);
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let config_path = root.path().join("imagod.toml");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_INITD_SCRIPT_PATH", &initd_path),
            ("IMAGOD_TEST_RC_ROOT", &rc_root),
            ("IMAGOD_TEST_SERVICE_MANAGER", Path::new("initd")),
            ("PATH", Path::new(&path_value)),
        ]);

        let manager = install(Some(config_path.clone())).expect("install should succeed");
        assert_eq!(manager, ServiceManager::Initd);
        let script = fs::read_to_string(&initd_path).expect("script should exist");
        assert!(script.contains(&format!("DAEMON='{}'", current_test_binary().display())));
        assert!(script.contains(&format!("CONFIG_PATH='{}'", config_path.display())));
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("update-rc.d imagod defaults"));
        assert!(commands.contains("service imagod start"));
    }

    #[test]
    fn uninstall_initd_removes_script_and_rc_links() {
        let _env_lock = env_lock();
        let root = temp_dir("initd-uninstall");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let initd_path = root.path().join("init.d").join("imagod");
        let rc_root = root.path().join("rc");
        let rc2 = rc_root.join("rc2.d");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        fs::create_dir_all(initd_path.parent().expect("parent")).expect("initd dir");
        fs::create_dir_all(&rc2).expect("rc dir");
        write_stub_command(&bin_dir.join("service"), &log_path);
        fs::write(&initd_path, "script").expect("script");
        fs::write(rc2.join("S50imagod"), "").expect("rc link placeholder");
        fs::write(rc2.join("S20myimagod"), "").expect("unrelated rc link placeholder");
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_INITD_SCRIPT_PATH", &initd_path),
            ("IMAGOD_TEST_RC_ROOT", &rc_root),
            ("PATH", Path::new(&path_value)),
        ]);

        let removed = uninstall().expect("uninstall should succeed");
        assert_eq!(removed, vec![ServiceManager::Initd]);
        assert!(!initd_path.exists());
        assert!(!rc2.join("S50imagod").exists());
        assert!(rc2.join("S20myimagod").exists());
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("service imagod stop"));
    }

    #[test]
    fn install_launchd_writes_plist_and_runs_bootstrap_flow() {
        let _env_lock = env_lock();
        let root = temp_dir("launchd-install");
        let bin_dir = root.path().join("bin");
        let log_path = root.path().join("commands.log");
        let plist_path = root.path().join("imagod.plist");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_stub_command(&bin_dir.join("launchctl"), &log_path);
        let path_value = env::join_paths([bin_dir.as_path()]).expect("PATH should join");
        let config_path = root.path().join("imagod.toml");
        let _guard = EnvGuard::set(&[
            ("IMAGOD_TEST_LAUNCHD_PLIST_PATH", &plist_path),
            ("IMAGOD_TEST_SERVICE_MANAGER", Path::new("launchd")),
            ("PATH", Path::new(&path_value)),
        ]);
        let _guard2 = EnvGuard::set_str(&[("IMAGOD_TEST_IS_ROOT", "1")]);

        let manager = install(Some(config_path.clone())).expect("install should succeed");
        assert_eq!(manager, ServiceManager::Launchd);
        let plist = fs::read_to_string(&plist_path).expect("plist should exist");
        assert!(plist.contains(&format!(
            "<string>{}</string>",
            current_test_binary().display()
        )));
        assert!(plist.contains(&format!("<string>{}</string>", config_path.display())));
        let commands = fs::read_to_string(&log_path).expect("commands should be logged");
        assert!(commands.contains("launchctl bootout system"));
        assert!(commands.contains("launchctl bootstrap system"));
        assert!(commands.contains("launchctl enable system/imagod"));
        assert!(commands.contains("launchctl kickstart -k system/imagod"));
    }

    #[test]
    fn restart_hint_prefers_installed_manager() {
        let _env_lock = env_lock();
        let root = temp_dir("restart-hint");
        let unit_path = root.path().join("imagod.service");
        fs::write(&unit_path, "unit").expect("unit");
        let _guard = EnvGuard::set(&[("IMAGOD_TEST_SYSTEMD_UNIT_PATH", &unit_path)]);
        assert_eq!(
            restart_hint().as_deref(),
            Some("sudo systemctl restart imagod.service")
        );
    }
}
