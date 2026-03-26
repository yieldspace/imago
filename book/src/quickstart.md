# QUICKSTART

## Goal

このクイックスタートガイドは、[Milk-V Duo S](https://milkv.io/docs/duo/getting-started/duos)をターゲットに、
ImagoをインストールしHello Worldを動かしたのち、USBカメラを接続し、取得したデータをhttp serverとして表示する手順を示します。

このクイックスタートガイドでは、以下の内容を含みます。

1. `imago` と `imagod` をインストールする
2. Imagoのprojectを作成する。
3. Hello Worldが実機で動くことを確認する。
4. カメラ操作用のプラグインを導入し、Milk-V Duo Sに接続したUSBカメラからデータを取得し、表示する

## Requirements

以下のものを用意して下さい:

パソコンに入れるもの

- Rustの環境

物理的なもの

- Milk-V Duo S
  - 秋月電子等で購入できます。
- micro SD Card
  - Milk-V Duo SのV2 OSをインストールして下さい。
  - https://milkv.io/docs/duo/getting-started/boot
- USB-C ケーブル
- USB ACアダプタ
- LANケーブル
- USBカメラ(USB-A)

## Install `imago` CLI

`imago`は、imagodを操作するためのCLIです。クライアントとなるパソコンに導入します:

```bash
curl -sSLf https://cli.imago.sh | sh
```

## Milk-V Duo Sをセットアップする

Milk-V Duo Sにmicro SD Cardを差し込み、USB-Cポートと、あなたのパソコンのUSBポートを接続して下さい。
Duo Sの青いランプが点滅すると起動が成功しています。

Duo Sは、デフォルトではUSB経由のルーターとして振る舞います。`ssh root@192.168.42.1`にssh接続を行い、パスワード`milkv`でコンソールに入って下さい。

## `imagod`をインストールする

Imagoのデーモンである`imagod`をDuo Sに導入します。Duo Sのシェルに以下のコマンドを用いてimagodを導入して下さい。

```sh
curl -sSLf https://install.imago.sh | sh -s -- --install-dir /usr/bin
```

imagodをバックグラウンドで動作させるため、init.dへのサービスの登録を行います:

```sh
imagod service install
```


## Create a New Project

Cargoを利用して、新しいプロジェクトを作成します。

```sh
cargo new imago-milkv
```

## Install the Wasm Target

WebAseemblyの環境を整えるため、wasm32-wasip2のターゲットをrustupに追加して下さい。

```bash
rustup target add wasm32-wasip2
```

## `imago.toml`を作成する

`imago.toml`ファイルをproject rootに作成し、以下の内容を記述します:

```toml
name.cargo = true

main = "target/wasm32-wasip2/release/example-service.wasm"
type = "cli"

[build]
command = "cargo build --target wasm32-wasip2 --release"

[capabilities]
wasi = true
```

`name.cargo = true`を設定すると、`Cargo.toml`の`[package].name`を自動的に読み取りサービス名として設定します。
独自の名前を指定したい場合は`name = "..."`と設定して下さい。

## Hello, Worldを動作させる

サービスをSSH経由でMilk-V Duo Sにデプロイします。

```sh
imago service deploy
```

シェルにHello, World!と表示されたら、成功です。

## TBD
