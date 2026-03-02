# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/yieldspace/imago/releases/tag/imagod-server-v0.1.0) - 2026-03-02

### Other

- Optimize logs path and heartbeat lookup for #230/#231/#232 ([#265](https://github.com/yieldspace/imago/pull/265))
- Reduce manager memory overhead in deploy and retained logs ([#248](https://github.com/yieldspace/imago/pull/248))
- Optimize IPC decode and RPC payload ownership in invoke path ([#244](https://github.com/yieldspace/imago/pull/244))
- Add scenario-driven imagod security test redesign ([#241](https://github.com/yieldspace/imago/pull/241))
- Update logs output format and add --with-timestamp support ([#226](https://github.com/yieldspace/imago/pull/226))
- Replace compatibility_date with protocol version negotiation ([#218](https://github.com/yieldspace/imago/pull/218))
- remove docs/spec and adopt code-first source of truth ([#217](https://github.com/yieldspace/imago/pull/217))
- Harden deploy stream handling for post-accept command.start failures ([#202](https://github.com/yieldspace/imago/pull/202))
- Update deploy failure observability and stale start recovery ([#201](https://github.com/yieldspace/imago/pull/201))
- Apply workspace-wide cargo-deny guardrails ([#169](https://github.com/yieldspace/imago/pull/169))
- Add imago ps and compose ps listing commands ([#168](https://github.com/yieldspace/imago/pull/168))
- Allow logs of retained services while imagod is running ([#164](https://github.com/yieldspace/imago/pull/164))
- Imago Networkの実装 ([#162](https://github.com/yieldspace/imago/pull/162))
- Refactor workspace boundaries and harden Tokio runtime paths ([#159](https://github.com/yieldspace/imago/pull/159))
- Unify logs filter identifier to name ([#152](https://github.com/yieldspace/imago/pull/152))
- Implement logs over QUIC DATAGRAM and fix datagram oversize ([#145](https://github.com/yieldspace/imago/pull/145))
- migrate to multi-process runtime and workspace-managed deps ([#138](https://github.com/yieldspace/imago/pull/138))
