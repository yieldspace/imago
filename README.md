# imago

imagoは、**組込み開発の敷居を下げる**、Wasm Component Modelベースの**実行・配布基盤**です。  
最小単位はWasm Componentで、**ケイパビリティベースの権限制御**により安全境界を明確にします。  
同一Wasmがどの環境でも動き、Dockerのようにリモートへデプロイできる体験を目指します。

また、imagoは**組込Linux向けの軽量コンテナ代替**として、Wasmをサンドボックスに利用し、OpenWrtなどリソースが限られた環境でも効率的にアプリケーションを実行できます。

## 特徴

- **Wasm Componentが最小単位**
- **ケイパビリティベース**でコンポーネントごとに権限を制御
- **同一Wasmがどこでも動く**ポータビリティ
- **Docker的なリモートデプロイ体験**を志向
- **組込Linux向けの軽量実行**（OpenWrtなど）

## コンセプト

- **Wasm Component Model**により、言語や環境差分を吸収
- **ケイパビリティ**で「できること」を明示的に制限し、安全境界を明確化
- **同一Wasmをどこでも動かす**ことで、組込み開発の敷居を下げる

## Quickstart

### Install CLI

```bash
curl -sSf https://imago.yield.space | sh
```

From cargo:

```bash
cargo install imago
```

### プロジェクト作成

```bash
git clone https://github.com/yieldspace/imago_template
```

### コードを書く

`src/main.rs`にコードを書きます。

```rust
fn main() {
    println!("Hello, World!")
}
```

### ビルド

```bash
imago build
```

ビルドコマンドは`imago.toml`の`[build].command`で変更できます。

## デーモンの起動

1. Install `imago` service.

```bash
imago service install
```

2. Start the service.

```bash
# Linux
systemctl start imago
# or
/etc/init.d start imago
```

## リモートデプロイ

imagoはdaemonが動作しているサーバーに対しリモートでデプロイできます。

```toml
[target.default]
remote = "192.168.1.100"
```

```bash
imago deploy
```

`imago deploy`は内部で`imago build`相当を毎回実行してから送信します。

デプロイ後のログは`imago logs <process id>`で確認できます。

## WITプラグイン

imagoは依存関係として**WIT**を利用し、プラグインを導入できます。

プラグインには、

1. imagoビルド時に同梱されている**ネイティブプラグイン**
2. **Wasm Componentベース**のプラグイン

の二種類があります。

`imago.toml`の`[[dependencies]]`に記述すると、`imago dev update`で自動でWITをダウンロードできます。

```toml
[[dependencies]]
name = "yieldspace:imago-experimental"
version = "0.0.1"
# プラグインがどのように提供されるか。builtinの場合はimagoに同梱されており、providedの場合はwasmとして提供される。
# type = "provided" # or "builtin"
# `type=provided`の場合ociベースで行われる。

# OCIベースの場合、配信元のregistry.
# registry = "ghcr.io"
```

## License

Apache-2.0
