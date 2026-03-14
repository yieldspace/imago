use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(bin_name = "cargo nirvash")]
struct Cli {
    #[arg(long, global = true)]
    base: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    ListTests {
        #[arg(long)]
        spec: Option<String>,
        #[arg(long)]
        binding: Option<String>,
        #[arg(long)]
        profile: Option<String>,
    },
    MaterializeTests {
        #[arg(long)]
        spec: String,
        #[arg(long)]
        binding: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        replay: Option<PathBuf>,
    },
    Replay {
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let base = cli.base.unwrap_or_else(cargo_nirvash::target_nirvash_dir);

    match cli.command {
        Command::ListTests {
            spec,
            binding,
            profile,
        } => {
            for manifest in cargo_nirvash::list_tests(
                &base,
                &cargo_nirvash::ManifestFilter {
                    spec,
                    binding,
                    profile,
                },
            )? {
                println!(
                    "{}\t{}\t{}",
                    manifest.spec, manifest.profile, manifest.binding
                );
            }
        }
        Command::MaterializeTests {
            spec,
            binding,
            profile,
            replay,
        } => {
            for path in cargo_nirvash::materialize_tests(
                &base,
                &cargo_nirvash::MaterializeRequest {
                    spec,
                    binding,
                    profile,
                    replay,
                },
            )? {
                println!("{}", path.display());
            }
        }
        Command::Replay { path } => {
            for output in cargo_nirvash::materialize_replay(&base, path)? {
                println!("{}", output.display());
            }
        }
    }

    Ok(())
}
