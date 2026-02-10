mod cli;
mod commands {
    pub mod deploy;
}

use clap::Parser;
use cli::{Cli, Commands};
use commands::deploy::CommandResult;

fn dispatch(cli: Cli) -> CommandResult {
    match cli.command {
        Commands::Deploy(args) => commands::deploy::run(args),
    }
}

fn main() {
    let cli = Cli::parse();
    let result = dispatch(cli);

    if let Some(message) = result.stderr {
        eprintln!("{message}");
    }

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::DeployArgs;

    #[test]
    fn dispatches_deploy_and_returns_non_zero() {
        let result = dispatch(Cli {
            command: Commands::Deploy(DeployArgs {
                env: None,
                target: None,
            }),
        });

        assert_eq!(result.exit_code, 2);
        assert_eq!(
            result.stderr,
            Some(commands::deploy::NOT_IMPLEMENTED_MESSAGE)
        );
    }
}
