use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::service;

const DEFAULT_RELEASES_API_URL: &str =
    "https://api.github.com/repos/yieldspace/imago/releases?per_page=100";
const DEFAULT_RELEASE_TAG_API_BASE: &str =
    "https://api.github.com/repos/yieldspace/imago/releases/tags";
const DEFAULT_RELEASE_DOWNLOAD_BASE: &str = "https://github.com/yieldspace/imago/releases/download";
const GITHUB_USER_AGENT: &str = "imagod-self-update";
const TRUSTED_GITHUB_AUTH_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfUpdateSummary {
    pub(crate) resolved_tag: String,
    pub(crate) asset_name: String,
    pub(crate) restart_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseSelectionMode {
    Stable,
    Prerelease,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseSummary {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

pub(crate) fn run(
    requested_tag: Option<&str>,
    allow_prerelease: bool,
) -> Result<SelfUpdateSummary, anyhow::Error> {
    let current_exe =
        env::current_exe().context("failed to resolve current imagod executable path")?;
    let asset_name = current_asset_name();
    let selection_mode = match requested_tag {
        Some(_) => ReleaseSelectionMode::Explicit,
        None if allow_prerelease => ReleaseSelectionMode::Prerelease,
        None => ReleaseSelectionMode::Stable,
    };
    let resolved_tag = match requested_tag {
        Some(tag) => normalize_tag(tag)?,
        None => resolve_latest_release_tag(allow_prerelease)?,
    };
    let metadata_bytes = download_release_metadata(&resolved_tag)?;
    let asset_names = parse_release_asset_names_from_metadata(&metadata_bytes)?;
    ensure_required_assets_present(&asset_names, &asset_name, &resolved_tag)?;

    let binary_url = format!("{}/{}", resolve_release_url_base(&resolved_tag), asset_name);
    let checksum_name = format!("{asset_name}.sha256");
    let checksum_url = format!(
        "{}/{}",
        resolve_release_url_base(&resolved_tag),
        checksum_name
    );
    let binary_bytes =
        download_release_asset(&binary_url, selection_mode, &resolved_tag, &asset_name)?;
    let checksum_bytes =
        download_release_asset(&checksum_url, selection_mode, &resolved_tag, &checksum_name)?;

    apply_downloaded_release_to_path(&current_exe, &asset_name, &binary_bytes, &checksum_bytes)?;

    Ok(SelfUpdateSummary {
        resolved_tag,
        asset_name,
        restart_hint: service::restart_hint(),
    })
}

fn current_asset_name() -> String {
    asset_name_for_target_and_features(
        env!("IMAGOD_BUILD_TARGET"),
        &parse_build_features(env!("IMAGOD_BUILD_FEATURES")),
    )
}

fn parse_build_features(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn asset_name_for_target_and_features(target: &str, features: &[String]) -> String {
    if features.is_empty() {
        return format!("imagod-{target}");
    }
    format!("imagod-{target}+{}", features.join("+"))
}

fn normalize_tag(input: &str) -> Result<String, anyhow::Error> {
    let version = input.strip_prefix("imagod-v").unwrap_or(input);
    if !is_valid_release_version(version) {
        bail!("invalid --tag value: {input}");
    }
    Ok(format!("imagod-v{version}"))
}

fn is_valid_release_version(version: &str) -> bool {
    let mut parts = version.splitn(3, '.');
    let Some(major) = parts.next() else {
        return false;
    };
    let Some(minor) = parts.next() else {
        return false;
    };
    let Some(rest) = parts.next() else {
        return false;
    };
    if !major.chars().all(|c| c.is_ascii_digit()) || !minor.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    let patch_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if patch_len == 0 {
        return false;
    }
    let suffix = &rest[patch_len..];
    if suffix.is_empty() {
        return true;
    }
    let Some(first_suffix_char) = suffix.chars().next() else {
        return false;
    };
    (first_suffix_char == '-' || first_suffix_char == '.')
        && suffix.len() > 1
        && suffix[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
}

fn resolve_latest_release_tag(allow_prerelease: bool) -> Result<String, anyhow::Error> {
    let api_url = releases_api_url();
    let supports_paging = release_api_supports_paging(&api_url);
    let mut page = 1usize;

    loop {
        let page_url = release_api_page_url(&api_url, page, supports_paging);
        let page_bytes = download_github_api(&page_url)
            .with_context(|| format!("failed to query GitHub Releases API: {page_url}"))?;
        let releases = parse_release_summaries(&page_bytes)?;
        if let Some(tag) = select_release_tag(&releases, allow_prerelease) {
            return Ok(tag);
        }
        if !supports_paging || releases.is_empty() {
            break;
        }
        page += 1;
    }

    if allow_prerelease {
        bail!(
            "no imagod release found via GitHub Releases API; pass --tag imagod-vX.Y.Z explicitly"
        );
    }
    bail!(
        "no stable imagod release found via GitHub Releases API; rerun with --prerelease or pass --tag imagod-vX.Y.Z"
    )
}

fn select_release_tag(releases: &[ReleaseSummary], allow_prerelease: bool) -> Option<String> {
    releases.iter().find_map(|release| {
        if release.draft {
            return None;
        }
        if !release.tag_name.starts_with("imagod-v")
            || !is_valid_release_version(release.tag_name.trim_start_matches("imagod-v"))
        {
            return None;
        }
        if !allow_prerelease && release.prerelease {
            return None;
        }
        Some(release.tag_name.clone())
    })
}

fn release_api_supports_paging(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn release_api_page_url(base_url: &str, page: usize, supports_paging: bool) -> String {
    if page == 1 || !supports_paging {
        return base_url.to_string();
    }
    if base_url.contains('?') {
        format!("{base_url}&page={page}")
    } else {
        format!("{base_url}?page={page}")
    }
}

fn parse_release_summaries(bytes: &[u8]) -> Result<Vec<ReleaseSummary>, anyhow::Error> {
    let value: Value =
        serde_json::from_slice(bytes).context("failed to parse releases API JSON")?;
    let releases = value
        .as_array()
        .ok_or_else(|| anyhow!("releases API response must be a JSON array"))?;
    let mut parsed = Vec::with_capacity(releases.len());
    for release in releases {
        let tag_name = release
            .get("tag_name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("releases API item is missing tag_name"))?;
        let draft = release
            .get("draft")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let prerelease = release
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        parsed.push(ReleaseSummary {
            tag_name: tag_name.to_string(),
            draft,
            prerelease,
        });
    }
    Ok(parsed)
}

fn download_release_metadata(tag: &str) -> Result<Vec<u8>, anyhow::Error> {
    let metadata_url = env::var("IMAGOD_RELEASE_METADATA_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{}/{}", release_tag_api_base(), tag));
    download_github_api(&metadata_url)
        .with_context(|| format!("failed to query GitHub release metadata: {metadata_url}"))
}

fn parse_release_asset_names_from_metadata(
    bytes: &[u8],
) -> Result<BTreeSet<String>, anyhow::Error> {
    let value: Value =
        serde_json::from_slice(bytes).context("failed to parse release metadata JSON")?;
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("release metadata is missing assets[]"))?;
    let mut names = BTreeSet::new();
    for asset in assets {
        let name = asset
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("release metadata asset is missing name"))?;
        names.insert(name.to_string());
    }
    Ok(names)
}

fn ensure_required_assets_present(
    asset_names: &BTreeSet<String>,
    asset_name: &str,
    resolved_tag: &str,
) -> Result<(), anyhow::Error> {
    if !asset_names.contains(asset_name) {
        bail!("requested imagod variant {asset_name} is not available in {resolved_tag}");
    }
    let checksum_name = format!("{asset_name}.sha256");
    if !asset_names.contains(&checksum_name) {
        bail!("requested imagod checksum {checksum_name} is not available in {resolved_tag}");
    }
    Ok(())
}

fn download_release_asset(
    url: &str,
    selection_mode: ReleaseSelectionMode,
    resolved_tag: &str,
    asset_name: &str,
) -> Result<Vec<u8>, anyhow::Error> {
    match download_bytes(url, false) {
        Ok(bytes) => Ok(bytes),
        Err(err) => match selection_mode {
            ReleaseSelectionMode::Stable => Err(anyhow!(
                "resolved stable release {resolved_tag} does not provide {asset_name} yet; retry later, use --prerelease, or pass --tag imagod-vX.Y.Z: {err:#}"
            )),
            ReleaseSelectionMode::Prerelease => Err(anyhow!(
                "resolved prerelease {resolved_tag} does not provide {asset_name} yet; retry later or pass --tag imagod-vX.Y.Z: {err:#}"
            )),
            ReleaseSelectionMode::Explicit => Err(anyhow!(
                "failed to download {asset_name} from {url}: {err:#}"
            )),
        },
    }
}

fn apply_downloaded_release_to_path(
    current_exe: &Path,
    asset_name: &str,
    asset_bytes: &[u8],
    checksum_bytes: &[u8],
) -> Result<(), anyhow::Error> {
    verify_checksum(asset_name, asset_bytes, checksum_bytes)?;
    let staged_path = write_staged_binary(current_exe, asset_bytes)?;
    swap_in_staged_binary(current_exe, &staged_path)?;
    Ok(())
}

fn verify_checksum(
    asset_name: &str,
    asset_bytes: &[u8],
    checksum_bytes: &[u8],
) -> Result<(), anyhow::Error> {
    let expected = parse_checksum_file(asset_name, checksum_bytes)?;
    let actual = hex::encode(Sha256::digest(asset_bytes));
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!("checksum verification failed for {asset_name}");
    }
    Ok(())
}

fn parse_checksum_file(asset_name: &str, checksum_bytes: &[u8]) -> Result<String, anyhow::Error> {
    let checksum_text =
        std::str::from_utf8(checksum_bytes).context("checksum file is not valid UTF-8")?;
    for line in checksum_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(digest) = parts.next() else {
            continue;
        };
        let Some(file_name) = parts.next() else {
            continue;
        };
        let file_name = file_name.trim_start_matches('*');
        if file_name != asset_name {
            continue;
        }
        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("checksum file contains an invalid digest for {asset_name}");
        }
        return Ok(digest.to_ascii_lowercase());
    }
    bail!("checksum file does not contain an entry for {asset_name}")
}

fn write_staged_binary(current_exe: &Path, asset_bytes: &[u8]) -> Result<PathBuf, anyhow::Error> {
    let parent = current_exe.parent().ok_or_else(|| {
        anyhow!(
            "current executable has no parent: {}",
            current_exe.display()
        )
    })?;
    let file_name = current_exe
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("current executable has an invalid file name"))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staged_path = parent.join(format!(".{file_name}.new-{unique}"));
    fs::write(&staged_path, asset_bytes)
        .with_context(|| format!("failed to write staged binary {}", staged_path.display()))?;

    let current_permissions = fs::metadata(current_exe)
        .with_context(|| format!("failed to stat {}", current_exe.display()))?
        .permissions();
    fs::set_permissions(&staged_path, current_permissions).with_context(|| {
        format!(
            "failed to copy executable permissions onto {}",
            staged_path.display()
        )
    })?;
    Ok(staged_path)
}

fn swap_in_staged_binary(current_exe: &Path, staged_path: &Path) -> Result<(), anyhow::Error> {
    let parent = current_exe.parent().ok_or_else(|| {
        anyhow!(
            "current executable has no parent: {}",
            current_exe.display()
        )
    })?;
    let file_name = current_exe
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("current executable has an invalid file name"))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup_path = parent.join(format!(".{file_name}.old-{unique}"));

    fs::rename(current_exe, &backup_path).with_context(|| {
        format!(
            "failed to move current executable {} aside before update",
            current_exe.display()
        )
    })?;

    match fs::rename(staged_path, current_exe) {
        Ok(()) => {
            let _ = fs::remove_file(&backup_path);
            Ok(())
        }
        Err(err) => {
            let restore_err = fs::rename(&backup_path, current_exe).err();
            let _ = fs::remove_file(staged_path);
            match restore_err {
                Some(restore_err) => Err(anyhow!(
                    "failed to activate updated imagod binary: {err}; rollback restore failed: {restore_err}"
                )),
                None => Err(anyhow!(
                    "failed to activate updated imagod binary and restored previous version: {err}"
                )),
            }
        }
    }
}

fn download_github_api(url: &str) -> Result<Vec<u8>, anyhow::Error> {
    download_bytes(url, true)
}

fn download_bytes(url: &str, github_api: bool) -> Result<Vec<u8>, anyhow::Error> {
    let mut request = ureq::get(url).header("User-Agent", GITHUB_USER_AGENT);
    if github_api {
        request = request
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
    }
    if let Some(token) = github_auth_token().filter(|_| should_send_github_auth(url)) {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let response = request
        .call()
        .map_err(|err| anyhow!("HTTP request to {url} failed: {err}"))?;
    response
        .into_body()
        .read_to_vec()
        .map_err(|err| anyhow!("failed to read HTTP response body from {url}: {err}"))
}

fn github_auth_token() -> Option<String> {
    env::var("GH_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("GITHUB_TOKEN")
                .ok()
                .filter(|value| !value.is_empty())
        })
}

fn should_send_github_auth(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    TRUSTED_GITHUB_AUTH_HOSTS
        .iter()
        .any(|trusted| host.eq_ignore_ascii_case(trusted))
}

fn releases_api_url() -> String {
    env::var("IMAGOD_RELEASES_API_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASES_API_URL.to_string())
}

fn release_tag_api_base() -> String {
    env::var("IMAGOD_RELEASE_TAG_API_BASE")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_TAG_API_BASE.to_string())
}

fn resolve_release_url_base(tag: &str) -> String {
    env::var("IMAGOD_RELEASE_BASE_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{DEFAULT_RELEASE_DOWNLOAD_BASE}/{tag}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        os::unix::fs::PermissionsExt,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedRequest {
        path: String,
        headers: Vec<(String, String)>,
    }

    struct EnvGuard {
        saved: Vec<(String, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&str, &str)]) -> Self {
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
            .expect("env lock")
    }

    fn temp_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "imagod-self-update-tests-{label}-{}-",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock should be after epoch")
                    .as_nanos()
            ))
            .tempdir()
            .expect("temp dir should be created")
    }

    #[test]
    fn normalize_tag_accepts_prefixed_and_plain_versions() {
        assert_eq!(
            normalize_tag("0.6.0").expect("plain semver should work"),
            "imagod-v0.6.0"
        );
        assert_eq!(
            normalize_tag("imagod-v0.6.0-rc.1").expect("prefixed semver should work"),
            "imagod-v0.6.0-rc.1"
        );
    }

    #[test]
    fn normalize_tag_rejects_invalid_versions() {
        let err = normalize_tag("imagod-latest").expect_err("invalid tag must fail");
        assert!(err.to_string().contains("invalid --tag value"));
    }

    #[test]
    fn select_release_tag_prefers_first_stable_release() {
        let releases = vec![
            ReleaseSummary {
                tag_name: "imagod-v0.7.0-rc.1".to_string(),
                draft: false,
                prerelease: true,
            },
            ReleaseSummary {
                tag_name: "imagod-v0.6.1".to_string(),
                draft: false,
                prerelease: false,
            },
        ];
        assert_eq!(
            select_release_tag(&releases, false),
            Some("imagod-v0.6.1".to_string())
        );
        assert_eq!(
            select_release_tag(&releases, true),
            Some("imagod-v0.7.0-rc.1".to_string())
        );
    }

    #[test]
    fn asset_name_for_target_and_features_renders_plus_suffix() {
        assert_eq!(
            asset_name_for_target_and_features(
                "riscv64gc-unknown-linux-musl",
                &[String::from("wasi-nn-cvitek")]
            ),
            "imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek"
        );
    }

    #[test]
    fn parse_release_asset_names_reads_assets_name_field() {
        let names = parse_release_asset_names_from_metadata(
            br#"{
                "assets": [
                    { "name": "imagod-riscv64gc-unknown-linux-musl" },
                    { "name": "imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek" }
                ]
            }"#,
        )
        .expect("asset names should parse");
        assert!(names.contains("imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek"));
    }

    #[test]
    fn ensure_required_assets_present_rejects_missing_variant() {
        let names = BTreeSet::from([String::from("imagod-x86_64-unknown-linux-gnu")]);
        let err = ensure_required_assets_present(
            &names,
            "imagod-riscv64gc-unknown-linux-musl+wasi-nn-cvitek",
            "imagod-v0.6.0",
        )
        .expect_err("missing asset must fail");
        assert!(err.to_string().contains("requested imagod variant"));
    }

    #[test]
    fn verify_checksum_rejects_mismatched_digest() {
        let err = verify_checksum(
            "imagod-x86_64-unknown-linux-gnu",
            b"binary",
            b"0000000000000000000000000000000000000000000000000000000000000000  imagod-x86_64-unknown-linux-gnu\n",
        )
        .expect_err("checksum mismatch must fail");
        assert!(err.to_string().contains("checksum verification failed"));
    }

    #[test]
    fn apply_downloaded_release_to_path_swaps_binary_atomically() {
        let root = temp_dir("swap-success");
        let exe_path = root.path().join("imagod");
        fs::write(&exe_path, b"old-binary").expect("old binary should be written");
        fs::set_permissions(&exe_path, fs::Permissions::from_mode(0o755))
            .expect("binary should be executable");
        let new_bytes = b"new-binary";
        let checksum = format!(
            "{}  imagod-x86_64-unknown-linux-gnu\n",
            hex::encode(Sha256::digest(new_bytes))
        );

        apply_downloaded_release_to_path(
            &exe_path,
            "imagod-x86_64-unknown-linux-gnu",
            new_bytes,
            checksum.as_bytes(),
        )
        .expect("update should succeed");
        assert_eq!(
            fs::read(&exe_path).expect("updated binary should be readable"),
            new_bytes
        );
    }

    #[test]
    fn write_staged_binary_reports_permission_denied() {
        let root = temp_dir("permission-denied");
        let exe_path = root.path().join("imagod");
        fs::write(&exe_path, b"old-binary").expect("old binary should be written");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o555))
            .expect("dir should become read-only");

        let err = write_staged_binary(&exe_path, b"new-binary")
            .expect_err("staging into read-only dir must fail");
        assert!(
            err.to_string().contains("failed to write staged binary"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn resolve_latest_release_tag_uses_override_api_url() {
        let _env_lock = env_lock();
        let server = TestServer::start(vec![(
            "/releases.json".to_string(),
            b"[{\"tag_name\":\"imagod-v0.6.0\",\"draft\":false,\"prerelease\":false}]".to_vec(),
        )]);
        let _guard = EnvGuard::set(&[("IMAGOD_RELEASES_API_URL", &server.url("/releases.json"))]);

        let tag = resolve_latest_release_tag(false).expect("tag should resolve");
        assert_eq!(tag, "imagod-v0.6.0");
    }

    #[test]
    fn should_send_github_auth_only_to_trusted_github_hosts() {
        assert!(should_send_github_auth(
            "https://api.github.com/repos/yieldspace/imago/releases"
        ));
        assert!(should_send_github_auth(
            "https://github.com/yieldspace/imago/releases/download/imagod-v0.6.0/imagod"
        ));
        assert!(!should_send_github_auth(
            "https://example.com/releases.json"
        ));
        assert!(!should_send_github_auth(
            "http://api.github.com/repos/yieldspace/imago/releases"
        ));
        assert!(!should_send_github_auth("not-a-url"));
    }

    #[test]
    fn download_bytes_omits_authorization_for_untrusted_hosts() {
        let _env_lock = env_lock();
        let server = TestServer::start(vec![("/asset".to_string(), b"asset".to_vec())]);
        let _guard = EnvGuard::set(&[("GH_TOKEN", "super-secret-token")]);

        let bytes = download_bytes(&server.url("/asset"), false).expect("download should succeed");
        assert_eq!(bytes, b"asset");

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")),
            "authorization header must not be sent to untrusted hosts: {:?}",
            requests[0].headers
        );
    }

    struct TestServer {
        addr: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start(routes: Vec<(String, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
            listener
                .set_nonblocking(true)
                .expect("listener should become nonblocking");
            let addr = listener.local_addr().expect("local addr");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let requests_for_thread = Arc::clone(&requests);
            let stop_for_thread = Arc::clone(&stop);
            let handle = thread::spawn(move || {
                let routes = routes;
                while !stop_for_thread.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _addr)) => {
                            let request = read_request(&mut stream);
                            requests_for_thread
                                .lock()
                                .expect("request log")
                                .push(request.clone());
                            let Some((_, body)) =
                                routes.iter().find(|(path, _)| path == &request.path)
                            else {
                                write_response(&mut stream, 404, b"not found");
                                continue;
                            };
                            write_response(&mut stream, 200, body);
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                addr: format!("http://{addr}"),
                requests,
                stop,
                handle: Some(handle),
            }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{}", self.addr, path)
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().expect("request log").clone()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> RecordedRequest {
        let mut buffer = [0u8; 4096];
        let size = stream
            .read(&mut buffer)
            .expect("request should be readable");
        let request = String::from_utf8_lossy(&buffer[..size]);
        let mut lines = request.lines();
        let path = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();
        let headers = lines
            .take_while(|line| !line.is_empty())
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        RecordedRequest { path, headers }
    }

    fn write_response(stream: &mut TcpStream, status: u16, body: &[u8]) {
        let _ = write!(
            stream,
            "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            status,
            body.len()
        );
        let _ = stream.write_all(body);
    }
}
