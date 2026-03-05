use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "prup")]
#[command(about = "Cargo workspace release orchestrator")]
pub struct Cli {
    #[arg(long, default_value = ".")]
    pub repo_root: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Doctor(CommonPlanArgs),
    Plan(PlanArgs),
    Apply(ApplyArgs),
    Explain(ExplainArgs),
    ReleaseTargets(ReleaseTargetsArgs),
    ReleasePrTargets(ReleasePrTargetsArgs),
}

#[derive(Debug, Clone, clap::Args)]
pub struct CommonPlanArgs {
    #[arg(long)]
    pub base_ref: Option<String>,

    #[arg(long)]
    pub line: Option<String>,

    #[arg(long)]
    pub allow_dirty: bool,
}

#[derive(Debug, Clone, clap::Args)]
pub struct PlanArgs {
    #[command(flatten)]
    pub common: CommonPlanArgs,

    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    pub common: CommonPlanArgs,

    #[arg(long)]
    pub from_plan: Option<PathBuf>,

    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, clap::Args)]
pub struct ExplainArgs {
    #[command(flatten)]
    pub common: CommonPlanArgs,

    pub target: String,
}

#[derive(Debug, Clone, clap::Args)]
pub struct ReleaseTargetsArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct ReleasePrTargetsArgs {
    #[command(flatten)]
    pub common: CommonPlanArgs,

    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,

    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}
