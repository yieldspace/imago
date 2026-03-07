# local-imagod-wasi-nn-openvino-person-detection example

## 目的

同一マシンで `wasi-nn` 対応 CLI アプリを `imago service deploy` し、`assets` に同梱した OpenVINO person detection model と入力画像を guest から読み込んで人物検出するサンプルです。

## 前提

Rust toolchain と `wasm32-wasip2` target を用意します（未導入なら `rustup target add wasm32-wasip2`）。

`imagod` は `wasi-nn-openvino` feature 付きで起動してください。
ネイティブの `imagod` 実行には OpenVINO runtime の共有ライブラリ解決が必要です。OpenVINO のインストールと動的ライブラリパス設定を先に済ませてください。

Homebrew 版の OpenVINO 2026.0.0 では `plugins.xml` が同梱されないため、`/opt/homebrew/opt/openvino/lib/plugins.xml` を次の内容で作成してください。

```xml
<ie>
    <plugins>
        <plugin name="CPU" location="openvino-2026.0.0/libopenvino_arm_cpu_plugin.so" />
    </plugins>
</ie>
```

## 実行

```bash
# ターミナル1
cd examples/local-imagod-wasi-nn-openvino-person-detection
OPENVINO_INSTALL_DIR=/opt/homebrew/opt/openvino \
DYLD_LIBRARY_PATH=/opt/homebrew/opt/openvino/lib:/opt/homebrew/opt/tbb/lib:/opt/homebrew/opt/pugixml/lib \
cargo run -p imagod --no-default-features --features "runtime-wasmtime,wasi-nn-openvino" -- --config imagod.toml
```

```bash
# ターミナル2
cd examples/local-imagod-wasi-nn-openvino-person-detection
# ターミナル1 で imagod が起動したことを確認してから実行
OPENVINO_INSTALL_DIR=/opt/homebrew/opt/openvino \
DYLD_LIBRARY_PATH=/opt/homebrew/opt/openvino/lib:/opt/homebrew/opt/tbb/lib:/opt/homebrew/opt/pugixml/lib \
cargo run -p imago-cli -- service deploy --target default --detach
cargo run -p imago-cli -- service logs local-imagod-wasi-nn-openvino-person-detection-app --tail 200
```

## 成功判定

`imago-cli service logs` の出力に `detected_persons=` が含まれ、少なくとも 1 件の `bbox[...]` 行が続けば成功です。

## メモ

- model は `assets/model.xml` と `assets/model.bin`、入力画像は `assets/people.ppm` として artifact に同梱され、`[[resources.read_only_mounts]]` により guest から `/app/assets` 配下で読めます。
- `wasi-nn` 自体は runtime が提供し、model の preload は行いません。guest がファイルを読んで `wasi:nn/graph.load` を呼びます。
- asset の出典は `assets/SOURCES.md` に記載しています。
