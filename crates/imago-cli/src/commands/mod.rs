use std::{collections::BTreeMap, time::Instant};

pub mod build;
pub mod certs;
pub(crate) mod command_common;
pub mod compose;
pub(crate) mod dependency_cache;
pub mod deploy;
pub(crate) mod error_diagnostics;
pub mod init;
pub mod logs;
pub(crate) mod plugin_sources;
pub mod ps;
pub mod run;
pub(crate) mod shared;
pub mod stop;
pub mod ui;
pub mod update;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub command: String,
    pub exit_code: i32,
    pub stderr: Option<String>,
    pub duration_ms: u128,
    pub meta: BTreeMap<String, String>,
}

impl CommandResult {
    pub fn success(command: &str, started_at: Instant) -> Self {
        Self {
            command: command.to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: started_at.elapsed().as_millis(),
            meta: BTreeMap::new(),
        }
    }

    pub fn failure(command: &str, started_at: Instant, message: String) -> Self {
        Self {
            command: command.to_string(),
            exit_code: 2,
            stderr: Some(message),
            duration_ms: started_at.elapsed().as_millis(),
            meta: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_result_sets_expected_fields() {
        let started_at = Instant::now();
        let result = CommandResult::success("artifact.build", started_at);

        assert_eq!(result.command, "artifact.build");
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_none());
        assert!(result.duration_ms <= started_at.elapsed().as_millis());
        assert!(result.meta.is_empty());
    }

    #[test]
    fn failure_result_sets_expected_fields() {
        let started_at = Instant::now();
        let result = CommandResult::failure(
            "deps.sync",
            started_at,
            "failed to parse config".to_string(),
        );

        assert_eq!(result.command, "deps.sync");
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.stderr.as_deref(), Some("failed to parse config"));
        assert!(result.duration_ms <= started_at.elapsed().as_millis());
        assert!(result.meta.is_empty());
    }
}
