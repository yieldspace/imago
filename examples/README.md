# examples

## 一覧

| ディレクトリ | 概要 | 実行コマンド |
| --- | --- | --- |
| `examples/local-imagod` | `imago service deploy` の最小構成 | `examples/local-imagod/README.md` |
| `examples/local-imagod-http` | `type=http` の実行例 | `examples/local-imagod-http/README.md` |
| `examples/local-imagod-wasi-nn-openvino-person-detection` | `wasi-nn` と asset 同梱 OpenVINO 人物検出 model の実行例 | `examples/local-imagod-wasi-nn-openvino-person-detection/README.md` |
| `examples/local-imagod-socket` | `type=socket` の実行例 | `examples/local-imagod-socket/README.md` |
| `examples/local-imagod-plugin-hello` | Wasm plugin 依存の実行例 | `examples/local-imagod-plugin-hello/README.md` |
| `examples/local-imagod-plugin-camera` | `imago:camera` Wasm plugin と `imago:usb` 連携の実行例 | `examples/local-imagod-plugin-camera/README.md` |
| `examples/local-imagod-plugin-native-admin` | native plugin 依存の実行例 | `examples/local-imagod-plugin-native-admin/README.md` |
| `examples/local-imagod-plugin-native-experimental-gpio` | experimental-gpio native plugin 依存の実行例 | `examples/local-imagod-plugin-native-experimental-gpio/README.md` |
| `examples/local-imagod-plugin-native-experimental-i2c` | experimental-i2c native plugin 依存の実行例 | `examples/local-imagod-plugin-native-experimental-i2c/README.md` |
| `examples/imago-compose-bindings` | stack/trust cert の実行例 | `examples/imago-compose-bindings/README.md` |
| `examples/imago-with-componentize-js-hono` | `componentize-js` + Hono で `type=http` を実行する例 | `examples/imago-with-componentize-js-hono/README.md` |

- `wasi-nn-cvitek` backend は SG200x / CV18xx 系の TPU runtime が必要なため、この repo にはローカル再現用 example を置いていません。guest からは `graph.load(..., autodetect, tpu)` で `.cvimodel` を渡します。feature variant の release asset 名は `imagod-<target>+wasi-nn-cvitek` です。Linux `riscv64` `musl` build で `riscv64-unknown-linux-musl-g++` が無い場合は shared library link へ自動 fallback するので、target では CVITEK TPU `.so` を loader path か `imagod` と同じディレクトリ配下の `lib/` に置いてください。
