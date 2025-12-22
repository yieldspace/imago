# QUICKSTART

imagoは組込Linux向けの軽量コンテナ代替ツールです。
Dockerのようにアプリケーションを隔離して実行しますが、コンテナの代わりにWebAssembly（Wasm）をサンドボックスとして使用します。
これにより、OpenWrtなどのリソースが限られた環境でも効率的にアプリケーションを実行できます。

## Install CLI

```bash
curl -sSf https://imago.yield.space | sh
```

From cargo:

```bash
cargo install imago
```

## プロジェクトの作成

プロジェクトの雛形をgithubからcloneします。

```bash
git clone https://github.com/yieldspace/imago_template
```

## コードを書く

`src/main.rs`を開いて、Hello, Worldプログラムを書きます。

```rust
fn main() {
    println!("Hello, World!")
}
```

## ビルド

`imago build`でWasmモジュールにコンパイルします。ビルドコマンドは`imago.toml`で変更できます。

```bash
imago build
```

## デーモンの起動

imagoを動作させるサーバーでデーモンを起動させます。

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

## デプロイ先の設定

imagoはdaemonが動作しているサーバーに対しリモートでデプロイが可能です。

```toml:imago.toml
[target.default]
remote = "192.168.1.100"
```

```bash
imago deploy
```

デプロイ後のログは`imago logs <process id>`で確認可能です。
