use super::*;
use crate::commands::build::parse_target_remote;

pub(in crate::commands::build) fn parse_target(
    root: &toml::Table,
    target_name: &str,
    project_root: &Path,
) -> anyhow::Result<TargetConfig> {
    let targets = root
        .get("target")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| anyhow!("imago.toml missing required key: target"))?;
    let raw_target = targets
        .get(target_name)
        .ok_or_else(|| anyhow!("target '{}' is not defined in imago.toml", target_name))?;
    let target_table = raw_target
        .as_table()
        .ok_or_else(|| anyhow!("target '{}' must be a table", target_name))?;

    let remote = target_table
        .get("remote")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| anyhow!("target '{}' is missing required key: remote", target_name))?
        .to_string();

    let _ = project_root;
    let _ = parse_target_remote(&remote)?;

    Ok(TargetConfig { remote })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toml::Value as TomlValue;

    use super::parse_target;
    use crate::commands::build::parse_target_remote;

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("value should be table")
            .clone()
    }

    #[test]
    fn parse_target_reads_ssh_remote() {
        let root = parse_table(
            r#"
[target.default]
remote = "ssh://example.local?socket=/run/imago/imagod.sock"
"#,
        );
        let target =
            parse_target(&root, "default", Path::new("/tmp/project")).expect("target should parse");
        assert_eq!(
            target.remote,
            "ssh://example.local?socket=/run/imago/imagod.sock"
        );
    }

    #[test]
    fn parse_target_remote_reads_ssh_socket_query() {
        let parsed = parse_target_remote("ssh://example.com:2222?socket=/run/imago/custom.sock")
            .expect("ssh remote should parse");
        assert_eq!(
            parsed,
            crate::commands::build::SshTargetRemote {
                user: None,
                host: "example.com".to_string(),
                port: Some(2222),
                socket_path: Some("/run/imago/custom.sock".to_string()),
            }
        );
    }

    #[test]
    fn parse_target_remote_rejects_unknown_ssh_query_keys() {
        let err = parse_target_remote("ssh://root@example.com?foo=bar")
            .expect_err("unknown ssh query should fail");
        assert!(err.to_string().contains("foo"));
    }

    #[test]
    fn parse_target_remote_rejects_relative_ssh_socket_path() {
        let err = parse_target_remote("ssh://root@example.com?socket=imagod.sock")
            .expect_err("relative socket path should fail");
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn parse_target_remote_accepts_omitted_user() {
        let parsed = parse_target_remote("ssh://localhost?socket=/run/imago/imagod.sock")
            .expect("ssh remote without user should parse");
        assert_eq!(parsed.user, None);
        assert_eq!(parsed.host, "localhost");
    }
}
