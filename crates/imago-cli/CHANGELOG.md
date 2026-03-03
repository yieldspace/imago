# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/yieldspace/imago/compare/imago-v0.1.0...imago-v0.1.1) - 2026-03-03

### Other

- Refactor imago-cli internals and expand unit test coverage ([#279](https://github.com/yieldspace/imago/pull/279))

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imago-v0.1.0) - 2026-03-02

### Other

- Add release-based imagod installer and checksum assets ([#266](https://github.com/yieldspace/imago/pull/266))
- Strengthen imago.lock v1 validation and component source resolution ([#264](https://github.com/yieldspace/imago/pull/264))
- Rework deps sync resolution and dependency source contracts ([#249](https://github.com/yieldspace/imago/pull/249))
- Reduce manager memory overhead in deploy and retained logs ([#248](https://github.com/yieldspace/imago/pull/248))
- Improve project init template flow and add componentize-py example ([#246](https://github.com/yieldspace/imago/pull/246))
- Fix spinner leakage before delegated log streaming ([#247](https://github.com/yieldspace/imago/pull/247))
- Rename CLI commands to v2 hierarchy ([#239](https://github.com/yieldspace/imago/pull/239))
- Use resources.gpio.digital_pins for experimental GPIO plugin ([#237](https://github.com/yieldspace/imago/pull/237))
- Add imago experimental GPIO native plugin and local example ([#236](https://github.com/yieldspace/imago/pull/236))
- Add resources section foundation for host policy ([#235](https://github.com/yieldspace/imago/pull/235))
- Update logs output format and add --with-timestamp support ([#226](https://github.com/yieldspace/imago/pull/226))
- Remove --json output mode from imago CLI ([#221](https://github.com/yieldspace/imago/pull/221))
- Bump wit-component from 0.244.0 to 0.245.1 ([#192](https://github.com/yieldspace/imago/pull/192))
- Replace compatibility_date with protocol version negotiation ([#218](https://github.com/yieldspace/imago/pull/218))
- remove docs/spec and adopt code-first source of truth ([#217](https://github.com/yieldspace/imago/pull/217))
- Add auto log follow for run and deploy ([#210](https://github.com/yieldspace/imago/pull/210))
- Add HTTP outbound e2e tests and CIDR IPC round-trip fix ([#206](https://github.com/yieldspace/imago/pull/206))
- Add deploy failure wasm log diagnostics and e2e coverage ([#205](https://github.com/yieldspace/imago/pull/205))
- Harden deploy stream handling for post-accept command.start failures ([#202](https://github.com/yieldspace/imago/pull/202))
- Update init template validation to parse TOML config ([#198](https://github.com/yieldspace/imago/pull/198))
- Update certs generate to client-only output ([#184](https://github.com/yieldspace/imago/pull/184))
- Add imago init command with template auto-detection ([#183](https://github.com/yieldspace/imago/pull/183))
- Add wildcard support for capabilities.deps ([#181](https://github.com/yieldspace/imago/pull/181))
- Support deep OCI WIT source paths and dependency package checks ([#179](https://github.com/yieldspace/imago/pull/179))
- Fix transitive WARG registry resolution for wasi packages ([#178](https://github.com/yieldspace/imago/pull/178))
- Merge dotenv into wasi env and add e2e coverage ([#176](https://github.com/yieldspace/imago/pull/176))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Add imago ps and compose ps listing commands ([#168](https://github.com/yieldspace/imago/pull/168))
- Add help docstrings for imago-cli commands ([#166](https://github.com/yieldspace/imago/pull/166))
- Allow logs of retained services while imagod is running ([#164](https://github.com/yieldspace/imago/pull/164))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Add WIT-native plugin flow and imago-lockfile v1 ([#157](https://github.com/yieldspace/imago/pull/157))
- Return logs as JSON lines ([#156](https://github.com/yieldspace/imago/pull/156))
- Unify logs filter identifier to name ([#152](https://github.com/yieldspace/imago/pull/152))
- Add type=socket runtime support and local UDP echo example ([#151](https://github.com/yieldspace/imago/pull/151))
- Add imago run/stop commands and restart policy handling ([#150](https://github.com/yieldspace/imago/pull/150))
- Split runtime backend into feature-gated crates ([#147](https://github.com/yieldspace/imago/pull/147))
- Implement logs over QUIC DATAGRAM and fix datagram oversize ([#145](https://github.com/yieldspace/imago/pull/145))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
- Implement #71 idempotency key hashing and upload resume retry ([#136](https://github.com/yieldspace/imago/pull/136))
- Handle invalid certificate failures as E_UNAUTHORIZED ([#135](https://github.com/yieldspace/imago/pull/135))
- Add imago build pipeline and manifest-relative main packaging ([#134](https://github.com/yieldspace/imago/pull/134))
- Implement imagod deploy core (QUIC/WebTransport + async Wasmtime) and Phase 1 sub-issues ([#133](https://github.com/yieldspace/imago/pull/133))
- Add imago-cli deploy基盤crate ([#131](https://github.com/yieldspace/imago/pull/131))
