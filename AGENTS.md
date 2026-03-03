# Repository Guidelines

## Scope & Coverage
This file applies to all paths in this repository.
If a deeper `AGENTS.md` exists in a subdirectory, that file overrides this one for its scope.

## Workspace Reality
This repository is a Rust workspace for embedded Linux use cases, not a single-crate project.
Workspace members include `crates/*`, `plugins/*`, `examples/local-imagod*`, and `e2e`.
Manage dependency versions and internal path dependencies in root `workspace.dependencies`, then reference them from member crates with `workspace = true`.
Protocol and runtime contracts are source-of-truth in code under `crates/*` (module docs, type definitions, validation logic, and tests). User-facing guides live under `docs/`.
Keep generated artifacts such as `target/` out of commits.

## Cross-cutting Engineering Rules
imago targets resource-constrained embedded Linux devices. Every change should reduce CPU, memory, and artifact size.
Prefer bounded data structures and avoid unnecessary heap allocation/copy.
Avoid steady background CPU load and justify long-running tasks.
Keep dependencies minimal and justify new crates in the PR.
Include release-build impact notes in PRs when behavior or dependencies change.
Use Rust 2024 edition conventions and `rustfmt` defaults (4-space indentation).
Use `snake_case` for functions/modules/files, `PascalCase` for types/traits, and `SCREAMING_SNAKE_CASE` for constants.
Prefer simplicity over pathological correctness. Follow YAGNI, KISS, and DRY.
Do not add backward-compat shims or fallback paths unless they come for free without increasing cyclomatic complexity.
Keep modules focused and explicit.
Implement new plugin host interfaces as async by default (for example, `wasmtime::component::bindgen!` imports with async settings).
If device/backend APIs are blocking, isolate them from the async executor (for example, `tokio::task::spawn_blocking`) instead of blocking async host functions directly.
When behavior changes, update related code docs (`//!`/`///`) and user-facing docs in the same PR so implementation and documentation stay aligned.

## WIT Writing Conventions
These rules apply to `plugins/*/wit/package.wit`.
- Every public WIT declaration must include a `///` docstring (`world`, `interface`, `resource`, `record`, `enum`, `variant`, `flags`, and function declarations).
- Every public WIT declaration must include `@since(version = <semver>)` immediately before the declaration.
- Keep declaration order consistent as `///` docstring, then `@since(...)`, then the declaration body.
- Preserve existing `@since` values as the first package version that introduced the declaration.
- For new declarations, set `@since` to the package version where the declaration is introduced.
- Keep docstrings short and concrete; include units, defaults, and error behavior when relevant.

## Build/Test Commands
- `cargo check --workspace`: fast compile-time validation without producing full build artifacts.
- `cargo build --workspace`: build all workspace crates in debug mode.
- `cargo build --release --workspace`: build optimized artifacts and verify embedded footprint.
- `cargo test --workspace`: run all tests across the workspace.
- `cargo test -p imago-protocol`: run tests for the protocol crate only.
- `cargo fmt --all`: apply standard Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint strictly and fail on warnings.

## Design Change Documentation Policy
When implementation introduces or changes protocol contracts, defaults, validation rules, or structured error contracts, update the corresponding source modules and code docs in the same PR.
Default workflow:
- Update the module docs/docstrings at the contract boundary (`crates/*`) together with implementation changes.
- If a user-facing explanation is needed, update the relevant guide pages under `docs/`.
- Do not use workstation-specific absolute paths in documentation; always use repository-relative paths and commands.
- Keep schema examples synchronized using tests/fixtures or in-code test cases where applicable.
PR body requirements for design-impacting changes must be written in `.github/pull_request_template.md` sections:
- `## Motivation`: describe why the design change is needed.
- `## Summary`: summarize design deltas and list updated spec files.
- `## Validation`: include validation commands and key results.

## Commit & PR Rules
Follow imperative commit subjects such as `Add ...`, `Update ...`, or `docs: ...`.
Keep each commit focused on one logical change.
PR body must follow `.github/pull_request_template.md` and explicitly fill `## Motivation`, `## Summary`, and `## Validation`.
Treat these three template sections as the only authoritative PR-body contract.
Include required details inside those sections: what changed and why, linked issue/PR context, validation commands, and spec/compatibility impact notes.

### PR Title Prefix and Breaking Footer (release-plz)
- Use Conventional Commit style for PR titles: `<prefix> <summary>`.
- Start every PR title with exactly one allowed prefix: `fix:`, `feat:`, `feat!:`, `ci:`, or `docs:`.
- Prefix-to-impact mapping for release-plz:
  - `feat!:` => Major
  - `feat:` => Minor
  - `fix:` => Patch
  - `ci:` and `docs:` => usually no release impact
- If a change is breaking, the commit message footer must include `BREAKING CHANGE:` even when the title prefix is not `feat!`.
- Any change that includes a `BREAKING CHANGE:` footer is treated as Major impact.
- In the `BREAKING CHANGE:` footer, describe the compatibility break and migration path (if needed) in 1-2 sentences.
- Impact precedence is: `BREAKING CHANGE footer` > `feat!:` > `feat:` > `fix:` > `ci:/docs:`.
- For squash merges, preserve the same prefix in the final merge commit title and keep the `BREAKING CHANGE:` footer when applicable.
- Prefixes outside the allowed set are not permitted. If additional prefixes are needed, update this rule first and update `release-plz.toml` as needed.

## Testing Guidelines
Use t-wada style TDD for new behavior: Red (failing test first), Green (minimum implementation), Refactor (cleanup with tests still green).
Add unit tests close to implementation with `#[cfg(test)] mod tests`.
Name tests by behavior (for example, `rejects_invalid_manifest_hash`).
For protocol or schema changes, add both success and failure-path tests and keep test fixtures/examples synchronized in code or `tests/fixtures` when applicable.
No strict coverage target exists, but new logic should cover key branches.
