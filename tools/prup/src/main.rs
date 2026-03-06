mod apply;
mod cli;
mod config;
mod error;
mod explain;
mod git;
mod graph;
mod planner;
mod resolver;
mod workspace;

use crate::cli::{ApplyArgs, Cli, Commands, CommonPlanArgs, OutputFormat};
use crate::config::LoadedConfig;
use crate::planner::{CurrentReleaseTarget, LineScopeInput, ReleasePlan, ReleasePrTarget};
use crate::resolver::ResolvedPolicy;
use anyhow::{Result, anyhow};
use clap::Parser;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct ReleaseTargetsOutput {
    prerelease: bool,
    targets: Vec<CurrentReleaseTarget>,
}

#[derive(Debug, Serialize)]
struct ReleasePrTargetsOutput {
    targets: Vec<ReleasePrTarget>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("prup error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = normalize_repo_root(&cli.repo_root)?;
    let loaded = config::load(&repo_root)?;
    let workspace = workspace::load(&repo_root)?;
    let resolved = resolver::resolve(&loaded.config, &workspace)?;

    match cli.command {
        Commands::Doctor(args) => run_doctor(&repo_root, &loaded, &resolved, &args),
        Commands::Plan(args) => run_plan(&repo_root, &loaded, &workspace, &resolved, args),
        Commands::Apply(args) => run_apply(&repo_root, &loaded, &workspace, &resolved, args),
        Commands::Explain(args) => {
            let allow_dirty = effective_allow_dirty(&resolved, &args.common);
            let plan = build_git_plan(
                &repo_root,
                &loaded,
                &workspace,
                &resolved,
                args.common.base_ref.as_deref(),
                args.common.line.as_deref(),
                allow_dirty,
            )?;
            println!("{}", explain::render_explanation(&plan, &args.target));
            Ok(())
        }
        Commands::ReleaseTargets(args) => {
            run_release_targets(&repo_root, &loaded, &workspace, &resolved, args)
        }
        Commands::ReleasePrTargets(args) => {
            run_release_pr_targets(&repo_root, &loaded, &workspace, &resolved, args)
        }
    }
}

fn run_doctor(
    repo_root: &Path,
    loaded: &LoadedConfig,
    resolved: &ResolvedPolicy,
    common: &CommonPlanArgs,
) -> Result<()> {
    let allow_dirty = effective_allow_dirty(resolved, common);
    git::ensure_clean(repo_root, allow_dirty)?;

    if let Some(cycle) = graph::find_line_cycle(&loaded.config) {
        return Err(anyhow!(
            "line propagation cycle detected: {}",
            cycle.join(" -> ")
        ));
    }

    if resolved.emit_tag_crates().is_empty() {
        return Err(anyhow!(
            "at least one emit_tag crate is required for release target calculation"
        ));
    }

    ensure_baseline_tags(repo_root, resolved)?;

    println!("doctor: ok");
    Ok(())
}

fn run_plan(
    repo_root: &Path,
    loaded: &LoadedConfig,
    workspace: &workspace::WorkspaceInfo,
    resolved: &ResolvedPolicy,
    args: cli::PlanArgs,
) -> Result<()> {
    let allow_dirty = effective_allow_dirty(resolved, &args.common);
    let plan = build_git_plan(
        repo_root,
        loaded,
        workspace,
        resolved,
        args.common.base_ref.as_deref(),
        args.common.line.as_deref(),
        allow_dirty,
    )?;

    if let Some(output_path) = args.output {
        let json = serde_json::to_string_pretty(&plan)?;
        fs::write(&output_path, json)?;
    }

    match args.format {
        OutputFormat::Human => print_human_plan(&plan),
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&plan)?;
            println!("{json}");
        }
    }

    Ok(())
}

fn run_apply(
    repo_root: &Path,
    loaded: &LoadedConfig,
    workspace: &workspace::WorkspaceInfo,
    resolved: &ResolvedPolicy,
    args: ApplyArgs,
) -> Result<()> {
    let allow_dirty = effective_allow_dirty(resolved, &args.common);
    git::ensure_clean(repo_root, allow_dirty)?;

    let plan = if let Some(path) = args.from_plan {
        let raw = fs::read_to_string(&path)?;
        serde_json::from_str(&raw)?
    } else {
        build_git_plan(
            repo_root,
            loaded,
            workspace,
            resolved,
            args.common.base_ref.as_deref(),
            args.common.line.as_deref(),
            allow_dirty,
        )?
    };

    if plan.crate_updates.is_empty() {
        println!("apply: no changes");
        return Ok(());
    }

    apply::apply_plan(repo_root, workspace, &plan, args.dry_run)?;

    if args.dry_run {
        println!("apply: dry-run completed (would update Cargo.toml files and Cargo.lock)");
    } else {
        println!("apply: updated Cargo.toml files and Cargo.lock");
    }

    Ok(())
}

fn run_release_targets(
    repo_root: &Path,
    loaded: &LoadedConfig,
    workspace: &workspace::WorkspaceInfo,
    resolved: &ResolvedPolicy,
    args: cli::ReleaseTargetsArgs,
) -> Result<()> {
    ensure_baseline_tags(repo_root, resolved)?;
    let line_scopes = collect_line_scopes(repo_root, resolved, None, None)?;
    let github_repo_name_with_owner = git::github_repo_name_with_owner(repo_root)?;
    let targets = planner::build_current_release_targets(
        resolved,
        workspace,
        &loaded.workspace_version,
        &line_scopes,
        github_repo_name_with_owner.as_deref(),
    )?;

    let output = ReleaseTargetsOutput {
        prerelease: resolved.github_prerelease,
        targets,
    };

    if let Some(path) = args.output {
        let json = serde_json::to_string_pretty(&output)?;
        fs::write(path, json)?;
    }

    match args.format {
        OutputFormat::Human => {
            if output.targets.is_empty() {
                println!("release-targets: none");
                return Ok(());
            }
            println!("release-targets:");
            for target in output.targets {
                println!(
                    "- {} {} -> {} (github_release={})",
                    target.crate_name, target.version, target.tag, target.github_release
                );
            }
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
    }

    Ok(())
}

fn run_release_pr_targets(
    repo_root: &Path,
    loaded: &LoadedConfig,
    workspace: &workspace::WorkspaceInfo,
    resolved: &ResolvedPolicy,
    args: cli::ReleasePrTargetsArgs,
) -> Result<()> {
    let allow_dirty = effective_allow_dirty(resolved, &args.common);
    git::ensure_clean(repo_root, allow_dirty)?;
    ensure_baseline_tags(repo_root, resolved)?;

    let mut top_crates = resolved.emit_tag_crates();
    top_crates.sort_by(|a, b| a.line.cmp(&b.line).then(a.name.cmp(&b.name)));

    if let Some(line_id) = args.common.line.as_deref()
        && !top_crates
            .iter()
            .any(|crate_policy| crate_policy.line == line_id)
    {
        return Err(anyhow!(
            "selected line {} is not a release line with emit_tag=true",
            line_id
        ));
    }

    let mut targets = Vec::new();
    for crate_policy in top_crates {
        if args
            .common
            .line
            .as_deref()
            .is_some_and(|line_id| crate_policy.line != line_id)
        {
            continue;
        }

        let plan = build_git_plan(
            repo_root,
            loaded,
            workspace,
            resolved,
            args.common.base_ref.as_deref(),
            Some(crate_policy.line.as_str()),
            allow_dirty,
        )?;

        if plan.crate_updates.is_empty() {
            continue;
        }

        targets.push(planner::build_release_pr_target(
            resolved,
            &plan,
            crate_policy.line.as_str(),
        )?);
    }

    let output = ReleasePrTargetsOutput { targets };

    if let Some(path) = args.output {
        let json = serde_json::to_string_pretty(&output)?;
        fs::write(path, json)?;
    }

    match args.format {
        OutputFormat::Human => {
            if output.targets.is_empty() {
                println!("release-pr-targets: none");
                return Ok(());
            }
            println!("release-pr-targets:");
            for target in output.targets {
                println!(
                    "- {}: {} -> {} [{}]",
                    target.line_id, target.before_version, target.after_version, target.branch
                );
            }
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
    }

    Ok(())
}

fn build_git_plan(
    repo_root: &Path,
    loaded: &LoadedConfig,
    workspace: &workspace::WorkspaceInfo,
    resolved: &ResolvedPolicy,
    base_ref_override: Option<&str>,
    selected_line: Option<&str>,
    allow_dirty: bool,
) -> Result<ReleasePlan> {
    git::ensure_clean(repo_root, allow_dirty)?;
    ensure_baseline_tags(repo_root, resolved)?;

    let line_scopes = collect_line_scopes(repo_root, resolved, base_ref_override, selected_line)?;

    planner::build_plan_from_line_scopes(
        resolved,
        workspace,
        &loaded.workspace_version,
        &line_scopes,
    )
}

fn collect_line_scopes(
    repo_root: &Path,
    resolved: &ResolvedPolicy,
    base_ref_override: Option<&str>,
    selected_line: Option<&str>,
) -> Result<Vec<LineScopeInput>> {
    let line_map = resolved.line_map();
    let mut top_crates = resolved.emit_tag_crates();
    top_crates.sort_by(|a, b| a.line.cmp(&b.line).then(a.name.cmp(&b.name)));

    if let Some(line_id) = selected_line
        && !top_crates
            .iter()
            .any(|crate_policy| crate_policy.line == line_id)
    {
        return Err(anyhow!(
            "selected line {} is not a release line with emit_tag=true",
            line_id
        ));
    }

    let mut scopes = Vec::new();

    for crate_policy in top_crates {
        if selected_line.is_some_and(|line_id| crate_policy.line != line_id) {
            continue;
        }

        let line_policy = line_map
            .get(crate_policy.line.as_str())
            .ok_or_else(|| anyhow!("unknown line {}", crate_policy.line))?;

        let base_ref = if let Some(explicit) = base_ref_override {
            explicit.to_string()
        } else {
            let glob = planner::baseline_tag_glob(crate_policy, line_policy);
            match git::latest_tag(repo_root, &glob)? {
                Some(tag) => tag,
                None if resolved.baseline_tag_required => {
                    return Err(anyhow!(
                        "baseline tag is missing for emit_tag crate {} ({glob})",
                        crate_policy.name
                    ));
                }
                None => resolved.base_ref.clone(),
            }
        };

        scopes.push(LineScopeInput {
            line_id: crate_policy.line.clone(),
            base_ref: base_ref.clone(),
            commits: git::commits_since(repo_root, &base_ref)?,
        });
    }

    Ok(scopes)
}

fn ensure_baseline_tags(repo_root: &Path, resolved: &ResolvedPolicy) -> Result<()> {
    if !resolved.baseline_tag_required {
        return Ok(());
    }

    let line_map = resolved.line_map();

    let mut missing = Vec::new();
    for crate_policy in resolved.emit_tag_crates() {
        let Some(line_policy) = line_map.get(crate_policy.line.as_str()) else {
            missing.push(format!(
                "{} (unknown line {})",
                crate_policy.name, crate_policy.line
            ));
            continue;
        };
        let glob = planner::baseline_tag_glob(crate_policy, line_policy);
        if git::latest_tag(repo_root, &glob)?.is_none() {
            missing.push(format!("{} ({glob})", crate_policy.name));
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "baseline tag is missing for crates: {}",
        missing.join(", ")
    ))
}

fn effective_allow_dirty(resolved: &ResolvedPolicy, common: &CommonPlanArgs) -> bool {
    common.allow_dirty || resolved.allow_dirty
}

fn print_human_plan(plan: &ReleasePlan) {
    if !plan.line_base_refs.is_empty() {
        println!("line base refs:");
        for (line_id, base_ref) in &plan.line_base_refs {
            println!("- {}: {}", line_id, base_ref);
        }
    }

    println!("changed_crates: {}", plan.changed_crates.join(", "));
    println!("impacted_crates: {}", plan.impacted_crates.join(", "));

    if plan.line_bumps.is_empty() {
        println!("line_bumps: none");
        return;
    }

    println!("line_bumps:");
    for line in &plan.line_bumps {
        println!("- {} ({:?})", line.line_id, line.bump);
    }

    if let Some(update) = &plan.workspace_version_update {
        println!(
            "workspace.version: {} -> {} ({:?})",
            update.before, update.after, update.bump
        );
    }

    if !plan.package_version_updates.is_empty() {
        println!("package versions:");
        for update in &plan.package_version_updates {
            println!(
                "- {}: {} -> {} ({:?})",
                update.crate_name, update.before, update.after, update.bump
            );
        }
    }

    if !plan.tags.is_empty() {
        println!("tags:");
        for tag in &plan.tags {
            println!("- {}", tag.tag);
        }
    }
}

fn normalize_repo_root(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    Ok(absolute)
}
