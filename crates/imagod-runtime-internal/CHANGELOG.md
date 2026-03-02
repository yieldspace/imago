# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-runtime-internal-v0.1.0) - 2026-03-02

### Other

- Optimize imagod runner memory and component loading ([#254](https://github.com/yieldspace/imago/pull/254))
- Bound HTTP request queue memory under burst traffic ([#234](https://github.com/yieldspace/imago/pull/234))
- Reduce HTTP response body copy amplification in runtime ([#233](https://github.com/yieldspace/imago/pull/233))
- Add resources section foundation for host policy ([#235](https://github.com/yieldspace/imago/pull/235))
- Add HTTP outbound e2e tests and CIDR IPC round-trip fix ([#206](https://github.com/yieldspace/imago/pull/206))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Add WIT-native plugin flow and imago-lockfile v1 ([#157](https://github.com/yieldspace/imago/pull/157))
- Add type=socket runtime support and local UDP echo example ([#151](https://github.com/yieldspace/imago/pull/151))
- Split runtime backend into feature-gated crates ([#147](https://github.com/yieldspace/imago/pull/147))
