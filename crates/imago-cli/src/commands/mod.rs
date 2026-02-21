use std::{collections::BTreeMap, time::Instant};

pub mod build;
pub mod certs;
pub(crate) mod command_common;
pub mod compose;
pub(crate) mod dependency_cache;
pub mod deploy;
pub(crate) mod error_diagnostics;
pub mod logs;
pub(crate) mod plugin_sources;
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
    pub skip_json_summary: bool,
}

impl CommandResult {
    pub fn success(command: &str, started_at: Instant) -> Self {
        Self {
            command: command.to_string(),
            exit_code: 0,
            stderr: None,
            duration_ms: started_at.elapsed().as_millis(),
            meta: BTreeMap::new(),
            skip_json_summary: false,
        }
    }

    pub fn failure(command: &str, started_at: Instant, message: String) -> Self {
        Self {
            command: command.to_string(),
            exit_code: 2,
            stderr: Some(message),
            duration_ms: started_at.elapsed().as_millis(),
            meta: BTreeMap::new(),
            skip_json_summary: false,
        }
    }

    pub fn without_json_summary(mut self) -> Self {
        self.skip_json_summary = true;
        self
    }
}
