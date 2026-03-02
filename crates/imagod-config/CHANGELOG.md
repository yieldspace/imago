# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-config-v0.1.0) - 2026-03-02

### Other

- Add release-based imagod installer and checksum assets ([#266](https://github.com/yieldspace/imago/pull/266))
- Optimize imagod runner memory and component loading ([#254](https://github.com/yieldspace/imago/pull/254))
- Reduce manager memory overhead in deploy and retained logs ([#248](https://github.com/yieldspace/imago/pull/248))
- Add scenario-driven imagod security test redesign ([#241](https://github.com/yieldspace/imago/pull/241))
- Bound HTTP request queue memory under burst traffic ([#234](https://github.com/yieldspace/imago/pull/234))
- Replace compatibility_date with protocol version negotiation ([#218](https://github.com/yieldspace/imago/pull/218))
- remove docs/spec and adopt code-first source of truth ([#217](https://github.com/yieldspace/imago/pull/217))
- Harden deploy stream handling for post-accept command.start failures ([#202](https://github.com/yieldspace/imago/pull/202))
- Replace nanokvm e2e linker test with runtime tests ([#199](https://github.com/yieldspace/imago/pull/199))
- imagod autocreate: generate server key and allow empty client key allowlist ([#172](https://github.com/yieldspace/imago/pull/172))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Update imagod storage_root defaults by OS and build-time override ([#149](https://github.com/yieldspace/imago/pull/149))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
