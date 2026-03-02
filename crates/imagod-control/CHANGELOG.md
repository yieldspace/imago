# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-control-v0.1.0) - 2026-03-02

### Other

- Optimize logs path and heartbeat lookup for #230/#231/#232 ([#265](https://github.com/yieldspace/imago/pull/265))
- Optimize imagod runner memory and component loading ([#254](https://github.com/yieldspace/imago/pull/254))
- Reduce manager memory overhead in deploy and retained logs ([#248](https://github.com/yieldspace/imago/pull/248))
- Optimize IPC decode and RPC payload ownership in invoke path ([#244](https://github.com/yieldspace/imago/pull/244))
- Add scenario-driven imagod security test redesign ([#241](https://github.com/yieldspace/imago/pull/241))
- Rename CLI commands to v2 hierarchy ([#239](https://github.com/yieldspace/imago/pull/239))
- Add imago experimental GPIO native plugin and local example ([#236](https://github.com/yieldspace/imago/pull/236))
- Bound HTTP request queue memory under burst traffic ([#234](https://github.com/yieldspace/imago/pull/234))
- Add resources section foundation for host policy ([#235](https://github.com/yieldspace/imago/pull/235))
- Update logs output format and add --with-timestamp support ([#226](https://github.com/yieldspace/imago/pull/226))
- Replace compatibility_date with protocol version negotiation ([#218](https://github.com/yieldspace/imago/pull/218))
- remove docs/spec and adopt code-first source of truth ([#217](https://github.com/yieldspace/imago/pull/217))
- Stop mirroring wasm stdout/stderr to imagod console ([#208](https://github.com/yieldspace/imago/pull/208))
- Add HTTP outbound e2e tests and CIDR IPC round-trip fix ([#206](https://github.com/yieldspace/imago/pull/206))
- Add deploy failure wasm log diagnostics and e2e coverage ([#205](https://github.com/yieldspace/imago/pull/205))
- Update deploy failure observability and stale start recovery ([#201](https://github.com/yieldspace/imago/pull/201))
- Fix deploy rollback recovery when service is busy ([#182](https://github.com/yieldspace/imago/pull/182))
- Merge dotenv into wasi env and add e2e coverage ([#176](https://github.com/yieldspace/imago/pull/176))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Add imago ps and compose ps listing commands ([#168](https://github.com/yieldspace/imago/pull/168))
- Allow logs of retained services while imagod is running ([#164](https://github.com/yieldspace/imago/pull/164))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Add WIT-native plugin flow and imago-lockfile v1 ([#157](https://github.com/yieldspace/imago/pull/157))
- Add type=socket runtime support and local UDP echo example ([#151](https://github.com/yieldspace/imago/pull/151))
- Add imago run/stop commands and restart policy handling ([#150](https://github.com/yieldspace/imago/pull/150))
- imagod起動時にactive_releaseサービスを自動復元する ([#148](https://github.com/yieldspace/imago/pull/148))
- Split runtime backend into feature-gated crates ([#147](https://github.com/yieldspace/imago/pull/147))
- Implement logs over QUIC DATAGRAM and fix datagram oversize ([#145](https://github.com/yieldspace/imago/pull/145))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
