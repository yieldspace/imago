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

```bash
cargo install imago
```

```bash
git clone https://github.com/yieldspace/imago
cd imago
./examples/local-imagod/scripts/quickstart.sh
```

他の example は [`examples/README.md`](examples/README.md)、詳細手順は [`QUICKSTART.md`](QUICKSTART.md) を参照してください。

## WITプラグイン

imagoは依存関係として**WIT**を利用し、プラグインを導入できます。

プラグインには、

1. imagoビルド時に同梱されている**ネイティブプラグイン**
2. **Wasm Componentベース**のプラグイン

の二種類があります。

`imago.toml`の`[[dependencies]]`に記述し、`imago update`を実行すると依存WIT/Componentを`.imago/deps/`（project内キャッシュ）へ解決し、`wit/deps/`を再生成した上で`imago.lock`へ固定できます。

```toml
[[dependencies]]
name = "sizumita:ferris"
version = "0.1.0"
kind = "wasm"
wit = "warg://sizumita:ferris@0.1.0"

[capabilities]
privileged = false

[capabilities.deps]
"sizumita:ferris" = ["sizumita:ferris/says@0.1.0.say"]
```

`warg://sizumita:ferris@0.1.0` は component を返すため、`[dependencies.component]` は不要です。
`kind="wasm"` かつ `wit` が component ではない場合のみ、`[dependencies.component]` で source を指定します。

`imago update` は依存を `.imago/deps/` に保存し、そこから `wit/deps/` を再生成します。`imago.lock (version=1)` には direct 依存の `wit_*` と transitive 依存の `[[wit_packages]]` を固定します。  
`kind="wasm"` で `dependencies.component` を省略した場合でも、`wit` source が component なら WIT 抽出と `component_*` 固定を自動で行います。  
`warg://` で取得した WIT package に transitive import がある場合、依存パッケージも `wit/deps/` に同時展開されます。  
`.imago_transitive` は使用しません。`imago build` は `imago.lock` の `[[wit_packages]]` を使って transitive package の digest を検証します。  
plain `.wit` 形式で foreign import を含む WIT は解決できないため、`imago update` はエラーになります。  
`warg://` の direct dependency で WIT 側に version が書かれている場合は、`warg://...@version` と一致している必要があります。  
`imago build` / `imago deploy` は source (`file://` / `warg://`) を直接参照せず、`.imago/deps/` を正本として利用します。  
必要なキャッシュが不足している場合は失敗し、`imago update` を要求します。

`warg://sizumita:ferris@0.1.0` を使った wasm plugin 実行例は
`examples/local-imagod-plugin-hello` を参照してください。

## License

Apache-2.0
