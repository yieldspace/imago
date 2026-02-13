pub mod build;
pub mod certs;
pub mod deploy;
pub mod service;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stderr: Option<String>,
}
