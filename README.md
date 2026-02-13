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
imago dev build
```

ビルドコマンドは`imago.toml`で変更できます。

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

デプロイ後のログは`imago logs <process id>`で確認できます。

### デーモンの自動デプロイ

デプロイ先のサーバーにデーモンが起動していない場合、SSH経由で自動的にデーモンをインストール・起動できます。

`imago.toml`に`ssh_user`を追加すると、デプロイ時にデーモンへの接続に失敗した場合、自動的にSSH経由でデーモンをデプロイします。

```toml
[target.default]
remote = "192.168.1.100:4443"
ca_cert = "certs/ca.crt"
client_cert = "certs/client.crt"
client_key = "certs/client.key"

# SSH接続情報（デーモン自動デプロイ用）
ssh_user = "deploy"                    # 必須
ssh_port = 22                          # オプション。デフォルトは22
ssh_key = "~/.ssh/id_rsa"             # オプション。省略時はSSHデフォルト設定を使用
server_cert = "certs/server.crt"       # オプション。省略時はca_certと同じディレクトリのserver.crt
server_key = "certs/server.key"        # オプション。省略時はca_certと同じディレクトリのserver.key
daemon_path = "target/release/imagod"  # オプション。デフォルトはtarget/release/imagodまたはtarget/debug/imagod
```

証明書は`imago certs generate`で生成できます。生成されたディレクトリをそのまま使う場合、最小構成は以下のようになります:

```toml
[target.default]
remote = "192.168.1.100"
ca_cert = "certs/ca.crt"
client_cert = "certs/client.crt"
client_key = "certs/client.key"
ssh_user = "deploy"
```

デーモンのみをデプロイする場合は`--only-daemon`オプションを使用します。

```bash
imago deploy --only-daemon --target default
```

このコマンドは以下の処理を実行します:

1. SSH接続を確立
2. imagodバイナリをリモートサーバーにアップロード（`/tmp/imago/imagod`）
3. CA証明書・サーバー証明書・サーバー秘密鍵をアップロード
4. imagod設定ファイル（`/tmp/imago/imagod.toml`）を生成
5. 既存のimagodプロセスを停止（存在する場合）
6. 新しいimagodプロセスをバックグラウンドで起動・生存確認
7. SSH接続を切断

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
