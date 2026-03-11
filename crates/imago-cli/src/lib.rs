mod cli;
mod commands;
mod lockfile;
pub mod runtime;

use std::{path::Path, sync::Arc};

use cli::{
    ArtifactCommands, ArtifactSubcommandArgs, Cli, Commands, DepsCommands, DepsSubcommandArgs,
    ProjectCommands, ProjectSubcommandArgs, ServiceCommands, ServiceSubcommandArgs,
    TrustCertCommands, TrustCertSubcommandArgs, TrustClientKeyCommands,
    TrustClientKeySubcommandArgs, TrustCommands, TrustSubcommandArgs,
};
use commands::CommandResult;

pub use crate::cli::Cli as ParsedCli;
pub use crate::commands::CommandResult as CliCommandResult;

fn uses_default_project_root(project_root: &Path) -> bool {
    project_root == Path::new(".")
}

async fn dispatch_with_project_root_async(cli: Cli, project_root: &Path) -> CommandResult {
    match cli.command {
        Commands::Project(ProjectSubcommandArgs { command }) => match command {
            ProjectCommands::Init(args) => {
                if uses_default_project_root(project_root) {
                    commands::init::run(args)
                } else {
                    commands::init::run_with_cwd(args, project_root)
                }
            }
        },
        Commands::Artifact(ArtifactSubcommandArgs { command }) => match command {
            ArtifactCommands::Build(args) => {
                if uses_default_project_root(project_root) {
                    commands::build::run(args)
                } else {
                    commands::build::run_with_project_root(args, project_root)
                }
            }
        },
        Commands::Deps(DepsSubcommandArgs { command }) => match command {
            DepsCommands::Sync(args) => {
                if uses_default_project_root(project_root) {
                    commands::update::run(args).await
                } else {
                    commands::update::run_with_project_root(args, project_root).await
                }
            }
        },
        Commands::Service(ServiceSubcommandArgs { command }) => match command {
            ServiceCommands::Deploy(args) => {
                if uses_default_project_root(project_root) {
                    commands::deploy::run(args).await
                } else {
                    commands::deploy::run_with_project_root(args, project_root).await
                }
            }
            ServiceCommands::Start(args) => {
                if uses_default_project_root(project_root) {
                    commands::run::run(args).await
                } else {
                    commands::run::run_with_project_root(args, project_root).await
                }
            }
            ServiceCommands::Stop(args) => {
                if uses_default_project_root(project_root) {
                    commands::stop::run(args).await
                } else {
                    commands::stop::run_with_project_root(args, project_root).await
                }
            }
            ServiceCommands::Ls(args) => {
                if uses_default_project_root(project_root) {
                    commands::ps::run(args).await
                } else {
                    commands::ps::run_with_project_root(args, project_root).await
                }
            }
            ServiceCommands::Logs(args) => {
                if uses_default_project_root(project_root) {
                    commands::logs::run(args).await
                } else {
                    commands::logs::run_with_project_root(args, project_root).await
                }
            }
        },
        Commands::Stack(args) => {
            if uses_default_project_root(project_root) {
                commands::compose::run(args).await
            } else {
                commands::compose::run_with_project_root(args, project_root).await
            }
        }
        Commands::Trust(TrustSubcommandArgs { command }) => match command {
            TrustCommands::Cert(TrustCertSubcommandArgs { command }) => match command {
                TrustCertCommands::Upload(args) => {
                    if uses_default_project_root(project_root) {
                        commands::certs::run_bindings_cert_upload(args).await
                    } else {
                        commands::certs::run_bindings_cert_upload_with_project_root(
                            args,
                            project_root,
                        )
                        .await
                    }
                }
                TrustCertCommands::Replicate(args) => {
                    if uses_default_project_root(project_root) {
                        commands::certs::run_bindings_cert_deploy(args).await
                    } else {
                        commands::certs::run_bindings_cert_deploy_with_project_root(
                            args,
                            project_root,
                        )
                        .await
                    }
                }
            },
            TrustCommands::ClientKey(TrustClientKeySubcommandArgs { command }) => match command {
                TrustClientKeyCommands::Generate(args) => {
                    if uses_default_project_root(project_root) {
                        commands::certs::run_generate(args)
                    } else {
                        commands::certs::run_generate_with_project_root(args, project_root)
                    }
                }
            },
        },
    }
}

pub async fn dispatch_with_runtime(cli: Cli, runtime: Arc<runtime::CliRuntime>) -> CommandResult {
    install_rustls_provider();
    runtime::scope(runtime.clone(), async move {
        let _ = commands::ui::initialize();
        commands::ui::emit_startup_banner(env!("CARGO_PKG_VERSION"));
        let result = dispatch_with_project_root_async(cli, runtime.project_root()).await;
        commands::ui::finalize_result(&result);
        result
    })
    .await
}

pub async fn dispatch(cli: Cli) -> CommandResult {
    dispatch_with_runtime(
        cli,
        Arc::new(runtime::CliRuntime::production(Path::new("."))),
    )
    .await
}

pub fn install_rustls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return;
    }

    let provider = web_transport_quinn::crypto::default_provider();
    if let Some(provider) = std::sync::Arc::into_inner(provider) {
        let _ = provider.install_default();
    }
}
