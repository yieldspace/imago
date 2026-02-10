# Repository Guidelines

## Project Structure & Module Organization
This repository is a Rust workspace for embedded Linux use cases. The root `Cargo.toml` defines members under `crates/`, and the current crate is `crates/imago-protocol`. Core Rust code lives in `crates/imago-protocol/src/` (`lib.rs`). Protocol documentation is in `docs/spec/`, with JSON samples in `docs/spec/examples/`. Keep generated artifacts in `target/` out of commits.

## Build, Test, and Development Commands
- `cargo check --workspace`: fast compile-time validation without producing full build artifacts.
- `cargo build --workspace`: build all workspace crates in debug mode.
- `cargo build --release --workspace`: build optimized artifacts and verify embedded footprint.
- `cargo test --workspace`: run all tests across the workspace.
- `cargo test -p imago-protocol`: run tests for the protocol crate only.
- `cargo fmt --all`: apply standard Rust formatting.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint strictly and fail on warnings.

## Embedded Linux Resource Discipline
imago targets resource-constrained embedded Linux devices. Every change should reduce CPU, memory, and artifact size.
- Prefer bounded data structures and avoid unnecessary heap allocation/copy.
- Avoid steady background CPU load; justify long-running tasks.
- Keep dependencies minimal and justify new crates in the PR.
- Include release-build impact notes in PRs when behavior or dependencies change.

## Coding Style & Naming Conventions
Use Rust 2024 edition conventions and `rustfmt` defaults (4-space indentation). Prefer:
- `snake_case` for functions, modules, and filenames.
- `PascalCase` for structs, enums, and traits.
- `SCREAMING_SNAKE_CASE` for constants.

Keep modules focused and explicit. When behavior changes, update related specs in `docs/spec/` in the same PR so implementation and documentation stay aligned.

## Testing Guidelines
Use t-wada style TDD for new behavior: Red (failing test first), Green (minimum implementation), Refactor (cleanup with tests still green). Add unit tests close to implementation using `#[cfg(test)] mod tests`. Name tests by behavior (for example, `rejects_invalid_manifest_hash`). For protocol or schema changes, add both success and failure-path tests and keep `docs/spec/examples/` synchronized. No strict coverage target exists, but new logic should cover key branches.

## Commit & Pull Request Guidelines
Follow the observed commit style: short, imperative subjects such as `Add ...`, `Update ...`, or scoped forms like `docs: ...`. Keep commits focused on one logical change.  
PRs should include:
- What changed and why.
- Linked issue/PR context.
- Validation commands you ran (for example, `cargo test --workspace`, `cargo clippy ...`).
- Notes on spec or compatibility impact when touching `docs/spec/`.

## Phase / Issue 運用ルール
- 現在の Phase は `0` とする。
- `close/development` に紐付けるのは、現在の Phase の issue のみに限定する。
- `close/development` へ issue を紐付けるときは、sub issue になっているものから先に行う。
