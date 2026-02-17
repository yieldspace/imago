pub mod build;
pub mod certs;
pub(crate) mod command_common;
pub(crate) mod dependency_cache;
pub mod deploy;
pub mod logs;
pub(crate) mod plugin_sources;
pub mod run;
pub mod stop;
pub mod update;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stderr: Option<String>,
}
