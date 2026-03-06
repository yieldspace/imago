use super::*;
use crate::commands::build::{ParsedTargetRemote, parse_target_remote};

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

    let server_name = optional_string(target_table, "server_name")?;
    if target_table.contains_key("ca_cert") {
        return Err(anyhow!(
            "target key 'ca_cert' is no longer supported; use target.<name>.client_key with RPK+TOFU"
        ));
    }
    if target_table.contains_key("client_cert") {
        return Err(anyhow!(
            "target key 'client_cert' is no longer supported; use target.<name>.client_key with RPK+TOFU"
        ));
    }
    if target_table.contains_key("known_hosts") {
        return Err(anyhow!(
            "target key 'known_hosts' is no longer supported; CLI always uses ~/.imago/known_hosts"
        ));
    }
    let client_key = optional_target_credential_path(target_table, "client_key", project_root)?;

    if matches!(parse_target_remote(&remote)?, ParsedTargetRemote::Ssh(_)) {
        if server_name.is_some() {
            return Err(anyhow!(
                "target key 'server_name' is not supported for ssh targets"
            ));
        }
        if client_key.is_some() {
            return Err(anyhow!(
                "target key 'client_key' is not supported for ssh targets"
            ));
        }
    }

    Ok(TargetConfig {
        remote,
        server_name,
        client_key,
    })
}

pub(in crate::commands::build) fn optional_string(
    table: &toml::Table,
    key: &str,
) -> anyhow::Result<Option<String>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("target key '{}' must be a string", key))?
        .to_string();
    Ok(Some(text))
}

pub(in crate::commands::build) fn optional_target_credential_path(
    table: &toml::Table,
    key: &str,
    project_root: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| anyhow!("target key '{}' must be a string", key))?;
    Ok(Some(resolve_target_credential_path(
        text,
        key,
        project_root,
    )?))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use toml::Value as TomlValue;

    use super::{optional_string, optional_target_credential_path, parse_target};
    use crate::commands::build::{ParsedTargetRemote, parse_target_remote};

    fn parse_table(raw: &str) -> toml::Table {
        toml::from_str::<TomlValue>(raw)
            .expect("toml should parse")
            .as_table()
            .expect("value should be table")
            .clone()
    }

    #[test]
    fn parse_target_reads_remote_and_optional_server_name() {
        let root = parse_table(
            r#"
[target.default]
remote = "127.0.0.1:7443"
server_name = "example.local"
"#,
        );
        let target =
            parse_target(&root, "default", Path::new("/tmp/project")).expect("target should parse");
        assert_eq!(target.remote, "127.0.0.1:7443");
        assert_eq!(target.server_name.as_deref(), Some("example.local"));
        assert!(target.client_key.is_none());
    }

    #[test]
    fn parse_target_rejects_removed_ca_cert_key() {
        let root = parse_table(
            r#"
[target.default]
remote = "127.0.0.1:7443"
ca_cert = "ca.pem"
"#,
        );
        let err = parse_target(&root, "default", Path::new("/tmp/project"))
            .expect_err("ca_cert should be rejected");
        assert!(err.to_string().contains("ca_cert"));
    }

    #[test]
    fn optional_target_credential_path_resolves_relative_to_project_root() {
        let table = parse_table(r#"client_key = "certs/client.key""#);
        let root = Path::new("/tmp/project");
        let resolved = optional_target_credential_path(&table, "client_key", root)
            .expect("client_key should parse")
            .expect("client_key should be present");
        assert_eq!(resolved, PathBuf::from("/tmp/project/certs/client.key"));
    }

    #[test]
    fn optional_string_returns_none_when_key_is_absent() {
        let table = parse_table(r#"remote = "127.0.0.1:7443""#);
        let value = optional_string(&table, "server_name").expect("optional parse should pass");
        assert!(value.is_none());
    }

    #[test]
    fn parse_target_rejects_server_name_for_ssh_remote() {
        let root = parse_table(
            r#"
[target.default]
remote = "ssh://root@example.com"
server_name = "example.com"
"#,
        );
        let err = parse_target(&root, "default", Path::new("/tmp/project"))
            .expect_err("server_name should be rejected for ssh remote");
        assert!(err.to_string().contains("server_name"));
    }

    #[test]
    fn parse_target_rejects_client_key_for_ssh_remote() {
        let root = parse_table(
            r#"
[target.default]
remote = "ssh://root@example.com"
client_key = "certs/client.key"
"#,
        );
        let err = parse_target(&root, "default", Path::new("/tmp/project"))
            .expect_err("client_key should be rejected for ssh remote");
        assert!(err.to_string().contains("client_key"));
    }

    #[test]
    fn parse_target_remote_reads_ssh_socket_query() {
        let parsed =
            parse_target_remote("ssh://root@example.com:2222?socket=/run/imago/custom.sock")
                .expect("ssh remote should parse");
        assert_eq!(
            parsed,
            ParsedTargetRemote::Ssh(crate::commands::build::SshTargetRemote {
                user: "root".to_string(),
                host: "example.com".to_string(),
                port: Some(2222),
                socket_path: Some("/run/imago/custom.sock".to_string()),
            })
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
}
