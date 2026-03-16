# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Other

- Add `wasi-nn-cvitek` runtime feature for CVITEK / Milk-V Duo TPU backends

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-runtime-v0.1.0) - 2026-03-02

### Other

- Optimize imagod runner memory and component loading ([#254](https://github.com/yieldspace/imago/pull/254))
- Add resources section foundation for host policy ([#235](https://github.com/yieldspace/imago/pull/235))
- remove docs/spec and adopt code-first source of truth ([#217](https://github.com/yieldspace/imago/pull/217))
- Add HTTP outbound e2e tests and CIDR IPC round-trip fix ([#206](https://github.com/yieldspace/imago/pull/206))
- Add deploy failure wasm log diagnostics and e2e coverage ([#205](https://github.com/yieldspace/imago/pull/205))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Add WIT-native plugin flow and imago-lockfile v1 ([#157](https://github.com/yieldspace/imago/pull/157))
- Add type=socket runtime support and local UDP echo example ([#151](https://github.com/yieldspace/imago/pull/151))
- Update imagod storage_root defaults by OS and build-time override ([#149](https://github.com/yieldspace/imago/pull/149))
- Split runtime backend into feature-gated crates ([#147](https://github.com/yieldspace/imago/pull/147))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
