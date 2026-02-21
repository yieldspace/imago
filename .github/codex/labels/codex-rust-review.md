# Codex Rust Review Checklist

Use this label when requesting a Rust-specific review.

## Rust-Specific Focus Areas
- Crate responsibility: Keep each crate/module focused on a clear single responsibility.
- Assertion style: Use precise assertions with clear failure messages where helpful.
- Avoid unsafe: Do not introduce `unsafe` unless strictly required and fully justified.
- `Cargo.toml` sorting: Keep dependency entries and related sections consistently sorted.
- Ownership/borrowing: Prefer clear lifetimes and avoid unnecessary cloning/heap allocation.
- Error handling: Propagate typed errors clearly; avoid opaque or lossy error paths.
- Clippy/rustfmt hygiene: Keep code lint-clean and formatting-consistent.
