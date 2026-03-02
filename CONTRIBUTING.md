# Contributing to imago

Thanks for your interest in contributing to imago.

This project targets embedded Linux systems, so we prioritize predictable behavior, explicit contracts, and resource efficiency.

## Ways to Contribute

- Report bugs with reproducible steps
- Propose and discuss features
- Submit fixes, tests, or documentation improvements
- Review pull requests and share technical feedback

## Before You Start

- Check existing issues and pull requests first
- For large or design-impacting changes, open an issue before implementation
- Keep proposals concrete: expected behavior, constraints, and trade-offs

## Development Workflow

1. Fork and create a focused branch.
2. Implement a single logical change.
3. Add or update tests for behavior changes.
4. Update related docs when contracts, defaults, or validation rules change.
5. Open a pull request with complete context and validation evidence.

## Required Validation

For Rust-impacting changes, run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For documentation-only changes, run the checks needed to validate links, examples, and consistency in the modified docs.

## Pull Request Requirements

Pull requests must follow `.github/pull_request_template.md` and fill all required sections:

- `Motivation`
- `Summary`
- `Validation`

Include exact commands and key results in `Validation`. If a check was not run, explain why.

## Code and Documentation Alignment

When behavior changes affect protocol contracts, defaults, validation rules, or error contracts:

- Update implementation and code docs in the same pull request
- Update user-facing docs under `docs/` in the same pull request

For embedded targets, call out expected CPU, memory, and binary-size impact when relevant.

## Review Expectations

- Maintainers review for correctness, contract clarity, and regression risk.
- Evidence-first review helps: include file paths, test results, and logs when useful.
- Follow-up commits are expected when review comments identify concrete gaps.
