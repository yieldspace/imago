# Imago Documentation

Imago is a Wasm Component deployment and runtime platform for embedded Linux environments.
This documentation is organized for quick onboarding first, then deeper protocol and runtime references.

## Basics

- [Architecture](./architecture.md)
- [imago.toml Reference](./imago-configuration.md)
- [imagod.toml Reference](./imagod-configuration.md)

```mermaid
flowchart LR
    A["imago.toml"] --> B["imago build"]
    B --> C["build/manifest.json"]
    C --> D["imago deploy"]
    D --> E["imagod manager"]
    E --> F["runner process"]
    F --> G["Wasm component"]
```

## Further Reading

- [Network RPC Model](./network-rpc.md)
- [WIT Plugins](./wit-plugins.md)
- [Specification Index](./spec/README.md)
- [Specification Examples](./spec/examples/README.md)

## For Developers

- [Configuration Specification](./spec/config.md)
- [Manifest Specification](./spec/manifest.md)
- [Deploy Protocol Specification](./spec/deploy-protocol.md)
- [Observability Specification](./spec/observability.md)
- [CLI Output Specification](./spec/cli-output.md)
- [imagod Server Overview](./spec/imagod.md)
- [imago-protocol Overview](./spec/imago-protocol.md)
- [imagod Internal Reference](./spec/imagod-internals.md)
- [imago-protocol Internal Reference](./spec/imago-protocol-internals.md)
