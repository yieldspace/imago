use crate::cli::DeployArgs;

pub const NOT_IMPLEMENTED_MESSAGE: &str = "imago deploy is not implemented yet.";

#[derive(Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stderr: Option<&'static str>,
}

pub fn run(_args: DeployArgs) -> CommandResult {
    CommandResult {
        exit_code: 2,
        stderr: Some(NOT_IMPLEMENTED_MESSAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_exit_code_two() {
        let result = run(DeployArgs {
            env: None,
            target: None,
        });

        assert_eq!(result.exit_code, 2);
    }

    #[test]
    fn returns_not_implemented_message() {
        let result = run(DeployArgs {
            env: Some("prod".to_string()),
            target: Some("default".to_string()),
        });

        assert_eq!(result.stderr, Some(NOT_IMPLEMENTED_MESSAGE));
    }
}
