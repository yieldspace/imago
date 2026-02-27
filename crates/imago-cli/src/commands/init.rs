use std::{
    collections::BTreeSet,
    fs,
    io::{self, IsTerminal, Write},
    path::{Component, Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use dialoguer::{Input, Select, theme::ColorfulTheme};
use include_dir::{Dir, include_dir};

use crate::{
    cli::InitArgs,
    commands::{
        CommandResult,
        error_diagnostics::{format_command_error, summarize_command_failure},
        ui,
    },
};

const INIT_COMMAND: &str = "project.init";
static TEMPLATE_ROOT: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitOutput {
    output_dir: PathBuf,
    template_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitTemplate {
    id: String,
    directories: Vec<PathBuf>,
    files: Vec<TemplateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TemplateFile {
    relative_path: PathBuf,
    contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitPlan {
    output_dir: PathBuf,
    directories: Vec<PathBuf>,
    files: Vec<PlannedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedFile {
    destination: PathBuf,
    contents: Vec<u8>,
}

pub fn run(args: InitArgs) -> CommandResult {
    run_with_cwd(args, Path::new("."))
}

pub(crate) fn run_with_cwd(args: InitArgs, cwd: &Path) -> CommandResult {
    let mut prompt_path = prompt_project_path;
    let mut prompt_template = prompt_template_id;
    run_with_cwd_and_prompt(
        args,
        cwd,
        is_interactive_session(),
        &mut prompt_path,
        &mut prompt_template,
    )
}

fn run_with_cwd_and_prompt(
    args: InitArgs,
    cwd: &Path,
    interactive: bool,
    prompt_path: &mut dyn FnMut() -> anyhow::Result<String>,
    prompt_template: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start(INIT_COMMAND, "starting");

    match run_init_inner(args, cwd, interactive, prompt_path, prompt_template) {
        Ok(output) => {
            ui::command_info(
                INIT_COMMAND,
                &format!(
                    "path={} template={}",
                    output.output_dir.display(),
                    output.template_id
                ),
            );
            ui::command_finish(INIT_COMMAND, true, "");
            let mut result = CommandResult::success(INIT_COMMAND, started_at);
            result
                .meta
                .insert("path".to_string(), output.output_dir.display().to_string());
            result
                .meta
                .insert("template".to_string(), output.template_id);
            result
        }
        Err(err) => {
            let summary = summarize_command_failure(INIT_COMMAND, &err);
            let message = format_command_error(INIT_COMMAND, &err);
            ui::command_finish(INIT_COMMAND, false, &summary);
            CommandResult::failure(INIT_COMMAND, started_at, message)
        }
    }
}

fn run_init_inner(
    args: InitArgs,
    cwd: &Path,
    interactive: bool,
    prompt_path: &mut dyn FnMut() -> anyhow::Result<String>,
    prompt_template: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> anyhow::Result<InitOutput> {
    let templates = detected_templates()?;
    let output_dir = resolve_output_dir(cwd, args.path.as_ref(), interactive, prompt_path)?;
    let template = select_template(
        args.template.as_deref(),
        &templates,
        interactive,
        prompt_template,
    )?;
    let plan = build_init_plan(&output_dir, &template);
    ensure_no_conflicts(&plan)?;
    apply_plan(&plan)?;
    Ok(InitOutput {
        output_dir,
        template_id: template.id,
    })
}

fn resolve_output_dir(
    cwd: &Path,
    requested_path: Option<&PathBuf>,
    interactive: bool,
    prompt_path: &mut dyn FnMut() -> anyhow::Result<String>,
) -> anyhow::Result<PathBuf> {
    let requested = match requested_path {
        Some(path) => path.to_path_buf(),
        None if interactive => {
            ui::command_clear(INIT_COMMAND);
            let value = prompt_path()?;
            let trimmed = value.trim();
            if trimmed.is_empty() {
                PathBuf::from(".")
            } else {
                PathBuf::from(trimmed)
            }
        }
        None => return Err(anyhow!("PATH is required in non-interactive mode")),
    };

    Ok(match requested.as_path() {
        path if path == Path::new(".") => cwd.to_path_buf(),
        path if path.is_absolute() => path.to_path_buf(),
        path => cwd.join(path),
    })
}

fn detected_templates() -> anyhow::Result<Vec<InitTemplate>> {
    if let Some(file) = TEMPLATE_ROOT.files().next() {
        return Err(anyhow!(
            "template root must contain only template directories, found file {}",
            file.path().display()
        ));
    }

    let mut templates = Vec::new();
    let mut seen = BTreeSet::new();
    for template_dir in TEMPLATE_ROOT.dirs() {
        let id = extract_template_id(template_dir)?;
        validate_template_id(&id, template_dir.path())?;
        if !seen.insert(id.clone()) {
            return Err(anyhow!("duplicate template id detected: {id}"));
        }
        templates.push(collect_template(template_dir, id)?);
    }

    templates.sort_by(|a, b| a.id.cmp(&b.id));
    if templates.is_empty() {
        return Err(anyhow!("no templates are available"));
    }
    Ok(templates)
}

fn extract_template_id(template_dir: &Dir<'_>) -> anyhow::Result<String> {
    let id = template_dir
        .path()
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "failed to read template id from directory path: {}",
                template_dir.path().display()
            )
        })?
        .to_str()
        .ok_or_else(|| {
            anyhow!(
                "template id must be valid UTF-8: {}",
                template_dir.path().display()
            )
        })?;
    Ok(id.to_string())
}

fn collect_template(template_dir: &Dir<'_>, id: String) -> anyhow::Result<InitTemplate> {
    let mut directories = BTreeSet::new();
    let mut files = Vec::new();
    let mut seen_files = BTreeSet::new();
    collect_template_entries(
        template_dir,
        template_dir.path(),
        &id,
        &mut directories,
        &mut files,
        &mut seen_files,
    )?;
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(InitTemplate {
        id,
        directories: directories.into_iter().collect(),
        files,
    })
}

fn collect_template_entries(
    dir: &Dir<'_>,
    base_dir: &Path,
    template_id: &str,
    directories: &mut BTreeSet<PathBuf>,
    files: &mut Vec<TemplateFile>,
    seen_files: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    let relative_dir = to_relative_template_path(base_dir, dir.path(), template_id)?;
    if !relative_dir.as_os_str().is_empty() {
        validate_relative_template_path(&relative_dir, template_id)?;
        directories.insert(relative_dir);
    }

    for child_dir in dir.dirs() {
        collect_template_entries(
            child_dir,
            base_dir,
            template_id,
            directories,
            files,
            seen_files,
        )?;
    }

    for file in dir.files() {
        let relative_file = to_relative_template_path(base_dir, file.path(), template_id)?;
        validate_relative_template_path(&relative_file, template_id)?;
        if !seen_files.insert(relative_file.clone()) {
            return Err(anyhow!(
                "duplicate file path '{}' detected in template '{}'",
                relative_file.display(),
                template_id
            ));
        }
        files.push(TemplateFile {
            relative_path: relative_file,
            contents: file.contents().to_vec(),
        });
    }

    Ok(())
}

fn to_relative_template_path(
    template_root: &Path,
    target: &Path,
    template_id: &str,
) -> anyhow::Result<PathBuf> {
    target
        .strip_prefix(template_root)
        .map(PathBuf::from)
        .map_err(|_| {
            anyhow!(
                "template '{}' path is outside template root: {}",
                template_id,
                target.display()
            )
        })
}

fn validate_template_id(id: &str, path: &Path) -> anyhow::Result<()> {
    if id.is_empty() {
        return Err(anyhow!("template id must not be empty: {}", path.display()));
    }

    let Some(first) = id.chars().next() else {
        return Err(anyhow!("template id must not be empty: {}", path.display()));
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(anyhow!(
            "template id must start with lowercase ASCII letter or digit: {id} ({})",
            path.display()
        ));
    }

    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(anyhow!(
            "template id must match [a-z0-9][a-z0-9_-]*: {id} ({})",
            path.display()
        ));
    }

    Ok(())
}

fn validate_relative_template_path(path: &Path, template_id: &str) -> anyhow::Result<()> {
    if path.is_absolute() {
        return Err(anyhow!(
            "template '{}' path must be relative: {}",
            template_id,
            path.display()
        ));
    }

    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(anyhow!(
                "template '{}' path contains unsupported component: {}",
                template_id,
                path.display()
            ));
        }
    }

    Ok(())
}

fn select_template(
    requested_template: Option<&str>,
    templates: &[InitTemplate],
    interactive: bool,
    prompt_template: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> anyhow::Result<InitTemplate> {
    let available = available_template_ids(templates);
    let selected_id = match requested_template {
        Some(raw) => normalize_template_id(raw),
        None if interactive => {
            ui::command_clear(INIT_COMMAND);
            normalize_template_id(&prompt_template(templates)?)
        }
        None => {
            return Err(anyhow!(
                "--template is required in non-interactive mode; available templates: {}",
                available.join(", ")
            ));
        }
    };

    templates
        .iter()
        .find(|template| template.id == selected_id)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "unknown template '{}'; available templates: {}",
                selected_id,
                available.join(", ")
            )
        })
}

fn normalize_template_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

fn available_template_ids(templates: &[InitTemplate]) -> Vec<&str> {
    templates
        .iter()
        .map(|template| template.id.as_str())
        .collect()
}

fn build_init_plan(output_dir: &Path, template: &InitTemplate) -> InitPlan {
    let mut directories = BTreeSet::new();
    directories.insert(output_dir.to_path_buf());
    for relative_dir in &template.directories {
        directories.insert(output_dir.join(relative_dir));
    }

    let mut files = Vec::new();
    for template_file in &template.files {
        let destination = output_dir.join(&template_file.relative_path);
        if let Some(parent) = destination.parent() {
            directories.insert(parent.to_path_buf());
        }
        files.push(PlannedFile {
            destination,
            contents: template_file.contents.clone(),
        });
    }

    let mut expanded_directories = BTreeSet::new();
    for dir in directories {
        insert_missing_paths_with_ancestors(&dir, &mut expanded_directories);
    }

    let mut sorted_directories: Vec<PathBuf> = expanded_directories.into_iter().collect();
    sorted_directories.sort_by(|a, b| path_depth(a).cmp(&path_depth(b)).then_with(|| a.cmp(b)));

    InitPlan {
        output_dir: output_dir.to_path_buf(),
        directories: sorted_directories,
        files,
    }
}

fn insert_missing_paths_with_ancestors(path: &Path, directories: &mut BTreeSet<PathBuf>) {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(dir) = current {
        match fs::symlink_metadata(dir) {
            Ok(_) => break,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                missing.push(dir.to_path_buf());
                current = dir.parent();
            }
            Err(_) => break,
        }
    }

    for dir in missing.into_iter().rev() {
        directories.insert(dir);
    }
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

fn ensure_no_conflicts(plan: &InitPlan) -> anyhow::Result<()> {
    ensure_dir_target_safe(&plan.output_dir)?;
    for dir in &plan.directories {
        ensure_dir_target_safe(dir)?;
    }
    for file in &plan.files {
        ensure_file_target_safe(&file.destination)?;
    }
    Ok(())
}

fn ensure_dir_target_safe(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(anyhow!(
                    "refusing to overwrite existing path: {}",
                    path.display()
                ));
            }
            if !metadata.is_dir() {
                return Err(anyhow!(
                    "refusing to overwrite existing path: {}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn ensure_file_target_safe(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(anyhow!(
            "refusing to overwrite existing path: {}",
            path.display()
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn apply_plan(plan: &InitPlan) -> anyhow::Result<()> {
    let mut writer =
        |destination: &Path, contents: &[u8]| write_file_create_new(destination, contents);
    apply_plan_with_writer(plan, &mut writer)
}

fn apply_plan_with_writer(
    plan: &InitPlan,
    writer: &mut dyn FnMut(&Path, &[u8]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut created_dirs = Vec::new();
    for dir in &plan.directories {
        match fs::create_dir(dir) {
            Ok(()) => created_dirs.push(dir.clone()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                ensure_dir_target_safe(dir)?;
            }
            Err(err) => {
                rollback_created_paths(&[], &created_dirs);
                return Err(err)
                    .with_context(|| format!("failed to create directory {}", dir.display()));
            }
        }
    }

    let mut created_files = Vec::new();
    for file in &plan.files {
        if let Err(err) = writer(&file.destination, &file.contents) {
            let mut rollback_files = created_files.clone();
            if fs::symlink_metadata(&file.destination).is_ok() {
                rollback_files.push(file.destination.clone());
            }
            rollback_created_paths(&rollback_files, &created_dirs);
            return Err(err);
        }
        created_files.push(file.destination.clone());
    }

    Ok(())
}

fn rollback_created_paths(created_files: &[PathBuf], created_dirs: &[PathBuf]) {
    for file in created_files.iter().rev() {
        let _ = fs::remove_file(file);
    }
    for dir in created_dirs.iter().rev() {
        let _ = fs::remove_dir(dir);
    }
}

fn write_file_create_new(destination: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            return Err(anyhow!(
                "refusing to overwrite existing path: {}",
                destination.display()
            ));
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!("failed to create template file {}", destination.display())
            });
        }
    };

    file.write_all(contents)
        .with_context(|| format!("failed to write template file {}", destination.display()))
        .inspect_err(|_| {
            let _ = fs::remove_file(destination);
        })?;
    file.flush()
        .with_context(|| format!("failed to flush template file {}", destination.display()))
        .inspect_err(|_| {
            let _ = fs::remove_file(destination);
        })?;
    Ok(())
}

fn is_interactive_session() -> bool {
    if ui::current_mode() != ui::UiMode::Rich {
        return false;
    }
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn prompt_project_path() -> anyhow::Result<String> {
    Input::<String>::with_theme(&ColorfulTheme::default())
        .with_prompt("Project path")
        .default(".".to_string())
        .interact_text()
        .context("failed to read project path")
}

fn prompt_template_id(templates: &[InitTemplate]) -> anyhow::Result<String> {
    if templates.is_empty() {
        return Err(anyhow!("no templates are available"));
    }

    let items: Vec<&str> = templates
        .iter()
        .map(|template| template.id.as_str())
        .collect();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select template")
        .items(&items)
        .default(0)
        .interact_opt()
        .context("failed to run template selector")?;

    let index = selection.ok_or_else(|| anyhow!("template selection was canceled"))?;
    Ok(templates[index].id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(test_name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX_EPOCH")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("imago-cli-init-{test_name}-{unique}"));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    fn run_inner_with_prompts(
        args: InitArgs,
        cwd: &Path,
        interactive: bool,
        prompted_path: &str,
        prompted_template: &str,
    ) -> anyhow::Result<InitOutput> {
        let prompted_path = prompted_path.to_string();
        let prompted_template = prompted_template.to_string();
        let mut path_prompt = move || Ok(prompted_path.clone());
        let mut template_prompt = move |_: &[InitTemplate]| Ok(prompted_template.clone());
        run_init_inner(
            args,
            cwd,
            interactive,
            &mut path_prompt,
            &mut template_prompt,
        )
    }

    fn file_count_recursive(root: &Path) -> usize {
        if !root.exists() {
            return 0;
        }
        let mut count = 0usize;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("directory should be readable") {
                let entry = entry.expect("directory entry should be readable");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn detected_templates_are_sorted_by_id() {
        let templates = detected_templates().expect("templates should load");
        assert!(!templates.is_empty(), "template list must not be empty");
        let ids: Vec<&str> = templates
            .iter()
            .map(|template| template.id.as_str())
            .collect();
        assert!(
            ids.windows(2).all(|window| window[0] <= window[1]),
            "template IDs should be sorted, but were: {ids:?}"
        );
    }

    #[test]
    fn templates_can_be_loaded_and_scanned_recursively() {
        let templates = detected_templates().expect("templates should load");
        assert!(!templates.is_empty(), "template list must not be empty");
        for template in templates {
            for directory in &template.directories {
                validate_relative_template_path(directory, &template.id)
                    .expect("directory paths must be valid");
            }
            for file in &template.files {
                validate_relative_template_path(&file.relative_path, &template.id)
                    .expect("file paths must be valid");
            }
        }
    }

    #[test]
    fn template_flag_takes_precedence_over_prompt_in_interactive_mode() {
        let cwd = temp_dir("template-flag-priority");
        let templates = detected_templates().expect("templates should load");
        let selected = templates[0].id.clone();
        let alternative = templates
            .iter()
            .find(|template| template.id != selected)
            .map(|template| template.id.clone())
            .unwrap_or_else(|| selected.clone());

        let output = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: Some(selected.clone()),
            },
            &cwd,
            true,
            ".",
            &alternative,
        )
        .expect("init should succeed");

        assert_eq!(output.template_id, selected);
        cleanup(&cwd);
    }

    #[test]
    fn interactive_mode_uses_prompted_path_when_path_is_missing() {
        let cwd = temp_dir("interactive-path-prompt");
        let templates = detected_templates().expect("templates should load");
        let selected = templates[0].id.clone();

        let output = run_inner_with_prompts(
            InitArgs {
                path: None,
                template: Some(selected),
            },
            &cwd,
            true,
            "svc-from-prompt",
            "unused",
        )
        .expect("init should succeed");

        assert_eq!(output.output_dir, cwd.join("svc-from-prompt"));
        cleanup(&cwd);
    }

    #[test]
    fn interactive_mode_uses_prompted_template_when_template_is_missing() {
        let cwd = temp_dir("interactive-template-prompt");
        let templates = detected_templates().expect("templates should load");
        let selected = templates[0].id.clone();

        let output = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: None,
            },
            &cwd,
            true,
            ".",
            &selected,
        )
        .expect("init should succeed");

        assert_eq!(output.template_id, selected);
        cleanup(&cwd);
    }

    #[test]
    fn non_interactive_mode_requires_path() {
        let cwd = temp_dir("requires-path-non-interactive");
        let templates = detected_templates().expect("templates should load");
        let selected = templates[0].id.clone();

        let err = run_inner_with_prompts(
            InitArgs {
                path: None,
                template: Some(selected),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect_err("missing path should fail");

        assert!(err.to_string().contains("PATH is required"));
        cleanup(&cwd);
    }

    #[test]
    fn non_interactive_mode_requires_template() {
        let cwd = temp_dir("requires-template-non-interactive");
        let err = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: None,
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect_err("missing template should fail");

        assert!(err.to_string().contains("--template is required"));
        cleanup(&cwd);
    }

    #[test]
    fn unknown_template_reports_available_templates() {
        let cwd = temp_dir("unknown-template");
        let err = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: Some("__unknown__".to_string()),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect_err("unknown template should fail");

        assert!(err.to_string().contains("unknown template"));
        cleanup(&cwd);
    }

    #[test]
    fn creates_missing_directory_and_writes_template_files() {
        let cwd = temp_dir("creates-directory-and-files");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| !template.files.is_empty())
            .expect("at least one template should contain files");

        let output = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: Some(template.id.clone()),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect("init should succeed");

        assert_eq!(output.output_dir, cwd.join("svc"));
        for directory in &template.directories {
            assert!(output.output_dir.join(directory).is_dir());
        }
        for file in &template.files {
            assert_eq!(
                fs::read(output.output_dir.join(&file.relative_path))
                    .expect("file should be readable"),
                file.contents
            );
        }
        cleanup(&cwd);
    }

    #[test]
    fn creates_ancestor_directories_for_nested_output_path() {
        let cwd = temp_dir("creates-ancestor-directories");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| !template.files.is_empty())
            .expect("at least one template should contain files");

        let nested = PathBuf::from("services/example");
        let output = run_inner_with_prompts(
            InitArgs {
                path: Some(nested.clone()),
                template: Some(template.id.clone()),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect("init should succeed for nested output path");

        assert_eq!(output.output_dir, cwd.join(&nested));
        assert!(cwd.join("services").is_dir());
        assert!(output.output_dir.is_dir());
        for file in &template.files {
            assert!(output.output_dir.join(&file.relative_path).is_file());
        }
        cleanup(&cwd);
    }

    #[test]
    fn dot_path_writes_files_into_cwd() {
        let cwd = temp_dir("dot-path");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| !template.files.is_empty())
            .expect("at least one template should contain files");

        let output = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from(".")),
                template: Some(template.id.clone()),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect("init should succeed");

        assert_eq!(output.output_dir, cwd);
        for file in &template.files {
            assert!(cwd.join(&file.relative_path).exists());
        }
        cleanup(&cwd);
    }

    #[test]
    fn fails_without_changes_when_any_conflict_exists() {
        let cwd = temp_dir("conflict-no-change");
        let output_dir = cwd.join("svc");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| !template.files.is_empty())
            .expect("at least one template should contain files");
        let conflict_relative = template
            .files
            .first()
            .expect("template should contain files")
            .relative_path
            .clone();
        let conflict_path = output_dir.join(&conflict_relative);
        let parent = conflict_path
            .parent()
            .expect("conflict file should have parent");
        fs::create_dir_all(parent).expect("parent should be created");
        fs::write(&conflict_path, b"existing").expect("existing file should be written");

        let err = run_inner_with_prompts(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                template: Some(template.id.clone()),
            },
            &cwd,
            false,
            ".",
            "unused",
        )
        .expect_err("conflict should fail");

        assert!(
            err.to_string()
                .contains("refusing to overwrite existing path")
        );
        assert_eq!(
            fs::read(&conflict_path).expect("existing file should be readable"),
            b"existing"
        );
        assert_eq!(file_count_recursive(&output_dir), 1);
        cleanup(&cwd);
    }

    #[test]
    fn apply_plan_rolls_back_created_files_on_write_failure() {
        let cwd = temp_dir("rollback-on-write-failure");
        let output_dir = cwd.join("svc");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| template.files.len() >= 2)
            .expect("at least one template should contain two files");
        let plan = build_init_plan(&output_dir, &template);
        ensure_no_conflicts(&plan).expect("plan should have no conflicts");

        let mut call_count = 0usize;
        let mut writer = |destination: &Path, contents: &[u8]| -> anyhow::Result<()> {
            if call_count == 0 {
                call_count += 1;
                return write_file_create_new(destination, contents);
            }
            Err(anyhow!("injected write failure"))
        };

        let err =
            apply_plan_with_writer(&plan, &mut writer).expect_err("write failure should fail");
        assert!(err.to_string().contains("injected write failure"));

        for file in &plan.files {
            assert!(
                !file.destination.exists(),
                "file should be rolled back: {}",
                file.destination.display()
            );
        }
        assert!(
            !output_dir.exists(),
            "output directory should be removed after rollback"
        );
        cleanup(&cwd);
    }

    #[test]
    fn apply_plan_rolls_back_partial_file_when_writer_fails_after_create() {
        let cwd = temp_dir("rollback-partial-file");
        let output_dir = cwd.join("svc");
        let template = detected_templates()
            .expect("templates should load")
            .into_iter()
            .find(|template| template.files.len() >= 2)
            .expect("at least one template should contain two files");
        let plan = build_init_plan(&output_dir, &template);
        ensure_no_conflicts(&plan).expect("plan should have no conflicts");

        let mut call_count = 0usize;
        let mut writer = |destination: &Path, contents: &[u8]| -> anyhow::Result<()> {
            if call_count == 0 {
                call_count += 1;
                return write_file_create_new(destination, contents);
            }

            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .expect("second file should be creatable");
            file.write_all(b"partial")
                .expect("partial file write should succeed");
            Err(anyhow!("injected failure after create"))
        };

        let err = apply_plan_with_writer(&plan, &mut writer)
            .expect_err("writer failure after create should fail");
        assert!(
            err.to_string().contains("injected failure after create"),
            "unexpected error: {err}"
        );

        for file in &plan.files {
            assert!(
                !file.destination.exists(),
                "file should be rolled back: {}",
                file.destination.display()
            );
        }
        assert!(
            !output_dir.exists(),
            "output directory should be removed after rollback"
        );
        cleanup(&cwd);
    }
}
