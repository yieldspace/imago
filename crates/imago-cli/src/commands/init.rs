use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::{
    cli::InitArgs,
    commands::{
        CommandResult,
        error_diagnostics::{format_command_error, summarize_command_failure},
        ui,
    },
};

const INIT_COMMAND: &str = "init";
const INIT_FILE_NAME: &str = "imago.toml";
const GITIGNORE_FILE_NAME: &str = ".gitignore";
const GITIGNORE_REQUIRED_ENTRIES: [&str; 2] = [".imago", "/build"];

mod generated {
    include!(concat!(env!("OUT_DIR"), "/init_templates.rs"));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitOutput {
    output_path: PathBuf,
    template_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitTemplate {
    id: &'static str,
    body: &'static str,
}

pub fn run(args: InitArgs) -> CommandResult {
    run_with_cwd(args, Path::new("."))
}

pub(crate) fn run_with_cwd(args: InitArgs, cwd: &Path) -> CommandResult {
    let mut prompt = prompt_template_id;
    run_with_cwd_and_prompt(args, cwd, is_interactive_session(), &mut prompt)
}

fn run_with_cwd_and_prompt(
    args: InitArgs,
    cwd: &Path,
    interactive: bool,
    prompt: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> CommandResult {
    let started_at = Instant::now();
    ui::command_start(INIT_COMMAND, "starting");
    ui::command_stage(INIT_COMMAND, "resolve-template", "selecting template");

    match run_init_inner(args, cwd, interactive, prompt) {
        Ok(output) => {
            ui::command_info(
                INIT_COMMAND,
                &format!(
                    "path={} template={}",
                    output.output_path.display(),
                    output.template_id
                ),
            );
            ui::command_finish(INIT_COMMAND, true, "");
            let mut result = CommandResult::success(INIT_COMMAND, started_at);
            result
                .meta
                .insert("path".to_string(), output.output_path.display().to_string());
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
    prompt: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> anyhow::Result<InitOutput> {
    let template = select_template(args.lang.as_deref(), interactive, prompt)?;
    let output_dir = resolve_output_dir(cwd, args.path.as_ref());
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create init directory: {}", output_dir.display()))?;

    let output_path = output_dir.join(INIT_FILE_NAME);
    write_init_file_create_new(&output_path, template.body)?;
    if let Err(err) = ensure_gitignore_entries(&output_dir) {
        if let Err(remove_err) = fs::remove_file(&output_path) {
            return Err(err.context(format!(
                "failed to update {} and failed to rollback {}: {}",
                output_dir.join(GITIGNORE_FILE_NAME).display(),
                output_path.display(),
                remove_err
            )));
        }
        return Err(err.context(format!(
            "failed to update {}; {} was rolled back",
            output_dir.join(GITIGNORE_FILE_NAME).display(),
            output_path.display()
        )));
    }

    Ok(InitOutput {
        output_path,
        template_id: template.id.to_string(),
    })
}

fn resolve_output_dir(cwd: &Path, requested_path: Option<&PathBuf>) -> PathBuf {
    match requested_path {
        None => cwd.to_path_buf(),
        Some(path) if path == Path::new(".") => cwd.to_path_buf(),
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => cwd.join(path),
    }
}

fn write_init_file_create_new(output_path: &Path, template_body: &str) -> anyhow::Result<()> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            return Err(anyhow!(
                "{} already exists: {}",
                INIT_FILE_NAME,
                output_path.display()
            ));
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to create {} template file: {}",
                    INIT_FILE_NAME,
                    output_path.display()
                )
            });
        }
    };

    file.write_all(template_body.as_bytes()).with_context(|| {
        format!(
            "failed to write {} template: {}",
            INIT_FILE_NAME,
            output_path.display()
        )
    })?;
    file.flush().with_context(|| {
        format!(
            "failed to flush {} template: {}",
            INIT_FILE_NAME,
            output_path.display()
        )
    })?;
    Ok(())
}

fn detected_templates() -> Vec<InitTemplate> {
    let mut templates: Vec<InitTemplate> = generated::INIT_TEMPLATES
        .iter()
        .map(|(id, body)| InitTemplate { id, body })
        .collect();
    templates.sort_by(|a, b| a.id.cmp(b.id));
    templates
}

fn select_template(
    lang: Option<&str>,
    interactive: bool,
    prompt: &mut dyn FnMut(&[InitTemplate]) -> anyhow::Result<String>,
) -> anyhow::Result<InitTemplate> {
    let templates = detected_templates();
    let available = available_template_ids(&templates);

    let selected_id = match lang {
        Some(raw) => normalize_lang_id(raw),
        None if interactive => normalize_lang_id(&prompt(&templates)?),
        None => {
            return Err(anyhow!(
                "--lang is required in non-interactive mode; available template languages: {}",
                available.join(", ")
            ));
        }
    };

    templates
        .into_iter()
        .find(|template| template.id == selected_id)
        .ok_or_else(|| {
            anyhow!(
                "unknown template language '{}'; available template languages: {}",
                selected_id,
                available.join(", ")
            )
        })
}

fn normalize_lang_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

fn available_template_ids(templates: &[InitTemplate]) -> Vec<&'static str> {
    templates.iter().map(|template| template.id).collect()
}

fn is_interactive_session() -> bool {
    if ui::current_mode() != ui::UiMode::Rich {
        return false;
    }
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn prompt_template_id(templates: &[InitTemplate]) -> anyhow::Result<String> {
    if templates.is_empty() {
        return Err(anyhow!("no templates are available"));
    }

    let items: Vec<&str> = templates.iter().map(|template| template.id).collect();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select template language")
        .items(&items)
        .default(0)
        .interact_opt()
        .context("failed to run template selector")?;

    let index = selection.ok_or_else(|| anyhow!("template selection was canceled"))?;
    Ok(templates[index].id.to_string())
}

fn ensure_gitignore_entries(output_dir: &Path) -> anyhow::Result<()> {
    let gitignore_path = output_dir.join(GITIGNORE_FILE_NAME);
    if !gitignore_path.exists() {
        let mut content = String::new();
        for entry in GITIGNORE_REQUIRED_ENTRIES {
            content.push_str(entry);
            content.push('\n');
        }
        fs::write(&gitignore_path, content)
            .with_context(|| format!("failed to write {}", gitignore_path.display()))?;
        return Ok(());
    }

    let existing = fs::read_to_string(&gitignore_path)
        .with_context(|| format!("failed to read {}", gitignore_path.display()))?;
    let mut missing = Vec::new();
    for required in GITIGNORE_REQUIRED_ENTRIES {
        if !existing
            .lines()
            .any(|line| line.trim_end_matches('\r') == required)
        {
            missing.push(required);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for required in missing {
        updated.push_str(required);
        updated.push('\n');
    }

    fs::write(&gitignore_path, updated)
        .with_context(|| format!("failed to write {}", gitignore_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::build;
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

    fn run_inner_with_fixed_choice(
        args: InitArgs,
        cwd: &Path,
        interactive: bool,
        choice: String,
    ) -> anyhow::Result<InitOutput> {
        let mut prompt = move |_: &[InitTemplate]| Ok(choice.clone());
        run_init_inner(args, cwd, interactive, &mut prompt)
    }

    fn assert_template_file(path: &Path, expected: &str) {
        let actual = fs::read_to_string(path).expect("template should be readable");
        assert_eq!(actual, expected);
    }

    fn read_file(path: &Path) -> String {
        fs::read_to_string(path).expect("file should be readable")
    }

    #[test]
    fn detected_templates_are_sorted_by_id() {
        let ids: Vec<&str> = detected_templates()
            .iter()
            .map(|template| template.id)
            .collect();
        assert!(
            ids.windows(2).all(|window| window[0] <= window[1]),
            "template IDs should be sorted, but were: {ids:?}"
        );
        for expected in ["generic", "rust"] {
            assert!(
                ids.contains(&expected),
                "expected template ID '{expected}' to be present in {ids:?}"
            );
        }
    }

    #[test]
    fn templates_are_valid_imago_config_toml() {
        for template in detected_templates() {
            let parsed: toml::Value = toml::from_str(template.body).unwrap_or_else(|err| {
                panic!("template '{}' should be valid TOML: {err}", template.id)
            });
            let root = parsed.as_table().unwrap_or_else(|| {
                panic!("template '{}' root should be a TOML table", template.id)
            });

            let name_value = root.get("name").unwrap_or_else(|| {
                panic!("template '{}' is missing required key 'name'", template.id)
            });
            name_value.as_str().unwrap_or_else(|| {
                panic!("template '{}' key 'name' should be string", template.id)
            });

            let main_value = root.get("main").unwrap_or_else(|| {
                panic!("template '{}' is missing required key 'main'", template.id)
            });
            main_value.as_str().unwrap_or_else(|| {
                panic!("template '{}' key 'main' should be string", template.id)
            });

            let app_type_value = root.get("type").unwrap_or_else(|| {
                panic!("template '{}' is missing required key 'type'", template.id)
            });
            let app_type = app_type_value.as_str().unwrap_or_else(|| {
                panic!("template '{}' key 'type' should be string", template.id)
            });
            assert!(
                build::validate_app_type(app_type).is_ok(),
                "template '{}' key 'type' has unsupported value: {}",
                template.id,
                app_type
            );

            if let Some(restart_value) = root.get("restart") {
                let restart = restart_value.as_str().unwrap_or_else(|| {
                    panic!("template '{}' key 'restart' should be string", template.id)
                });
                assert!(
                    build::is_supported_restart_policy(restart),
                    "template '{}' key 'restart' has unsupported value: {}",
                    template.id,
                    restart
                );
            }
        }
    }

    #[test]
    fn resolves_none_path_to_cwd() {
        let cwd = PathBuf::from("/tmp/sample");
        assert_eq!(resolve_output_dir(&cwd, None), cwd);
    }

    #[test]
    fn resolves_dot_path_to_cwd() {
        let cwd = PathBuf::from("/tmp/sample");
        assert_eq!(
            resolve_output_dir(&cwd, Some(&PathBuf::from("."))),
            PathBuf::from("/tmp/sample")
        );
    }

    #[test]
    fn resolves_relative_path_under_cwd() {
        let cwd = PathBuf::from("/tmp/sample");
        assert_eq!(
            resolve_output_dir(&cwd, Some(&PathBuf::from("services/api"))),
            PathBuf::from("/tmp/sample/services/api")
        );
    }

    #[test]
    fn resolves_absolute_path_as_is() {
        let cwd = PathBuf::from("/tmp/sample");
        let absolute = PathBuf::from("/var/tmp/imago-test");
        assert_eq!(resolve_output_dir(&cwd, Some(&absolute)), absolute);
    }

    #[test]
    fn writes_rust_template_when_lang_is_rust() {
        let cwd = temp_dir("writes-rust-template");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "generic".to_string(),
        )
        .expect("init should succeed");

        let rust = detected_templates()
            .into_iter()
            .find(|template| template.id == "rust")
            .expect("rust template should exist");
        assert_eq!(output.template_id, "rust");
        assert_template_file(&output.output_path, rust.body);

        cleanup(&cwd);
    }

    #[test]
    fn writes_generic_template_when_lang_is_generic() {
        let cwd = temp_dir("writes-generic-template");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("generic".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let generic = detected_templates()
            .into_iter()
            .find(|template| template.id == "generic")
            .expect("generic template should exist");
        assert_eq!(output.template_id, "generic");
        assert_template_file(&output.output_path, generic.body);

        cleanup(&cwd);
    }

    #[test]
    fn creates_missing_directories_before_writing() {
        let cwd = temp_dir("creates-missing-directory");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("a/b/c")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        assert!(output.output_path.exists());
        assert!(cwd.join("a/b/c").is_dir());

        cleanup(&cwd);
    }

    #[test]
    fn creates_gitignore_when_missing() {
        let cwd = temp_dir("creates-gitignore-when-missing");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let gitignore_path = output
            .output_path
            .parent()
            .expect("parent should exist")
            .join(".gitignore");
        assert_eq!(read_file(&gitignore_path), ".imago\n/build\n");

        cleanup(&cwd);
    }

    #[test]
    fn appends_missing_gitignore_entries_only() {
        let cwd = temp_dir("appends-missing-gitignore-entries-only");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        fs::write(output_dir.join(".gitignore"), "target\n").expect(".gitignore should be written");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let gitignore_path = output
            .output_path
            .parent()
            .expect("parent should exist")
            .join(".gitignore");
        assert_eq!(read_file(&gitignore_path), "target\n.imago\n/build\n");

        cleanup(&cwd);
    }

    #[test]
    fn does_not_duplicate_existing_gitignore_entries() {
        let cwd = temp_dir("does-not-duplicate-existing-gitignore-entries");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        let original = ".imago\n/build\n";
        fs::write(output_dir.join(".gitignore"), original).expect(".gitignore should be written");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let gitignore_path = output
            .output_path
            .parent()
            .expect("parent should exist")
            .join(".gitignore");
        assert_eq!(read_file(&gitignore_path), original);

        cleanup(&cwd);
    }

    #[test]
    fn does_not_duplicate_existing_gitignore_entries_with_crlf() {
        let cwd = temp_dir("does-not-duplicate-existing-gitignore-entries-with-crlf");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        let original = ".imago\r\n/build\r\n";
        fs::write(output_dir.join(".gitignore"), original).expect(".gitignore should be written");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let gitignore_path = output
            .output_path
            .parent()
            .expect("parent should exist")
            .join(".gitignore");
        assert_eq!(read_file(&gitignore_path), original);

        cleanup(&cwd);
    }

    #[test]
    fn appends_missing_entries_when_gitignore_has_no_trailing_newline() {
        let cwd = temp_dir("appends-missing-entries-without-trailing-newline");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        fs::write(output_dir.join(".gitignore"), ".imago").expect(".gitignore should be written");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let gitignore_path = output
            .output_path
            .parent()
            .expect("parent should exist")
            .join(".gitignore");
        assert_eq!(read_file(&gitignore_path), ".imago\n/build\n");

        cleanup(&cwd);
    }

    #[test]
    fn does_not_update_gitignore_when_imago_toml_already_exists() {
        let cwd = temp_dir("does-not-update-gitignore-when-imago-toml-exists");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        fs::write(output_dir.join("imago.toml"), "name = \"existing\"\n")
            .expect("existing file should be written");
        fs::write(output_dir.join(".gitignore"), "target\n").expect(".gitignore should be written");

        let err = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect_err("existing imago.toml should fail");

        assert!(err.to_string().contains("already exists"));
        assert_eq!(read_file(&output_dir.join(".gitignore")), "target\n");

        cleanup(&cwd);
    }

    #[test]
    fn rolls_back_imago_toml_when_gitignore_update_fails() {
        let cwd = temp_dir("rolls-back-imago-toml-when-gitignore-update-fails");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(output_dir.join(".gitignore")).expect("directory should be created");

        let err = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect_err("gitignore update should fail");

        assert!(err.to_string().contains("failed to update"));
        assert!(!output_dir.join("imago.toml").exists());
        assert!(output_dir.join(".gitignore").is_dir());

        cleanup(&cwd);
    }

    #[test]
    fn writes_gitignore_in_same_directory_as_imago_toml_for_relative_path() {
        let cwd = temp_dir("writes-gitignore-in-same-directory-as-imago-toml");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("nested/service")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect("init should succeed");

        let output_dir = output.output_path.parent().expect("parent should exist");
        assert!(output_dir.join("imago.toml").exists());
        assert!(output_dir.join(".gitignore").exists());

        cleanup(&cwd);
    }

    #[test]
    fn rejects_when_imago_toml_already_exists() {
        let cwd = temp_dir("rejects-existing");
        let output_dir = cwd.join("svc");
        fs::create_dir_all(&output_dir).expect("output dir should be created");
        fs::write(output_dir.join("imago.toml"), "name = \"existing\"\n")
            .expect("existing file should be written");

        let err = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("rust".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect_err("existing imago.toml should fail");

        assert!(err.to_string().contains("already exists"));

        cleanup(&cwd);
    }

    #[test]
    fn requires_lang_in_non_interactive_mode() {
        let cwd = temp_dir("requires-lang");

        let err = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: None,
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect_err("missing lang should fail in non-interactive mode");

        assert!(err.to_string().contains("--lang is required"));
        assert!(err.to_string().contains("generic"));
        assert!(err.to_string().contains("rust"));

        cleanup(&cwd);
    }

    #[test]
    fn rejects_unknown_lang_with_available_options() {
        let cwd = temp_dir("unknown-lang");

        let err = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: Some("zig".to_string()),
            },
            &cwd,
            false,
            "rust".to_string(),
        )
        .expect_err("unknown lang should fail");

        assert!(err.to_string().contains("unknown template language"));
        assert!(err.to_string().contains("generic"));
        assert!(err.to_string().contains("rust"));

        cleanup(&cwd);
    }

    #[test]
    fn interactive_mode_uses_prompt_choice() {
        let cwd = temp_dir("interactive-choice");

        let output = run_inner_with_fixed_choice(
            InitArgs {
                path: Some(PathBuf::from("svc")),
                lang: None,
            },
            &cwd,
            true,
            "rust".to_string(),
        )
        .expect("interactive selection should succeed");

        assert_eq!(output.template_id, "rust");

        cleanup(&cwd);
    }
}
