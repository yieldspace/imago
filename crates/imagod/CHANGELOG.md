# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-v0.1.0) - 2026-03-02

### Other

- Update imagod version/help protocol display ([#269](https://github.com/yieldspace/imago/pull/269))
- Optimize imagod runner memory and component loading ([#254](https://github.com/yieldspace/imago/pull/254))
- Reduce manager memory overhead in deploy and retained logs ([#248](https://github.com/yieldspace/imago/pull/248))
- Add imago:usb rusb plugin with wasi-usb parity APIs ([#242](https://github.com/yieldspace/imago/pull/242))
- Add imago experimental GPIO native plugin and local example ([#236](https://github.com/yieldspace/imago/pull/236))
- Bound HTTP request queue memory under burst traffic ([#234](https://github.com/yieldspace/imago/pull/234))
- Add async experimental i2c native plugin ([#225](https://github.com/yieldspace/imago/pull/225))
- Replace nanokvm e2e linker test with runtime tests ([#199](https://github.com/yieldspace/imago/pull/199))
- Add imagod library API and NanoKVM custom daemon/plugin ([#175](https://github.com/yieldspace/imago/pull/175))
- imagod autocreate: generate server key and allow empty client key allowlist ([#172](https://github.com/yieldspace/imago/pull/172))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Add WIT-native plugin flow and imago-lockfile v1 ([#157](https://github.com/yieldspace/imago/pull/157))
- imagod起動時にactive_releaseサービスを自動復元する ([#148](https://github.com/yieldspace/imago/pull/148))
- Split runtime backend into feature-gated crates ([#147](https://github.com/yieldspace/imago/pull/147))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
- Implement imagod deploy core (QUIC/WebTransport + async Wasmtime) and Phase 1 sub-issues ([#133](https://github.com/yieldspace/imago/pull/133))
