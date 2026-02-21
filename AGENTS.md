# Repository Guidelines

## Scope & Coverage
This file applies to all paths in this repository.
If a deeper `AGENTS.md` exists in a subdirectory, that file overrides this one for its scope.

## Workspace Reality
This repository is a Rust workspace for embedded Linux use cases, not a single-crate project.
Workspace members include `crates/*`, `plugins/*`, `examples/local-imagod*`, and `e2e`.
Manage dependency versions and internal path dependencies in root `workspace.dependencies`, then reference them from member crates with `workspace = true`.
Protocol and runtime specs live under `docs/spec/`, and JSON samples live under `docs/spec/examples/`.
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
When behavior changes, update related `docs/spec/` files in the same PR so implementation and documentation stay aligned.

## Build/Test Commands
- `cargo check --workspace`: fast compile-time validation without producing full build artifacts.
- `cargo build --workspace`: build all workspace crates in debug mode.
- `cargo build --release --workspace`: build optimized artifacts and verify embedded footprint.
- `cargo test --workspace`: run all tests across the workspace.
- `cargo test -p imago-protocol`: run tests for the protocol crate only.
- `cargo fmt --all`: apply standard Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint strictly and fail on warnings.

## Design Change Documentation Policy
When implementation introduces or changes protocol contracts, defaults, validation rules, or structured error contracts, update corresponding files under `docs/spec/` in the same PR.
Default workflow:
- If the delta is too large for inline notes, create a separate document and link it from both `docs/spec/README.md` and the related spec file(s).
- Keep `docs/spec/examples/` synchronized for schema and contract changes.
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

## Testing Guidelines
Use t-wada style TDD for new behavior: Red (failing test first), Green (minimum implementation), Refactor (cleanup with tests still green).
Add unit tests close to implementation with `#[cfg(test)] mod tests`.
Name tests by behavior (for example, `rejects_invalid_manifest_hash`).
For protocol or schema changes, add both success and failure-path tests and keep `docs/spec/examples/` synchronized.
No strict coverage target exists, but new logic should cover key branches.
