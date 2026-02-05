# Concept: 理念から実装へ（Q&A）

このドキュメントは、imagoの理念を実装方針へ落とし込むためのQ&A。

## Q1. imagoが目指す理念は？
A. **組込み開発の敷居を下げる**。

## Q2. 「敷居」の内訳は？（優先順）
A. **C/C++必須** → **SDKの複雑さ** → **実機配布/更新**。

## Q3. C/C++必須の壁はどう壊す？
A. **Wasmにコンパイルできる言語なら何でもOK**。配布単位はWasmで、**同一Wasmがどこでも動く**ことを目指す。

## Q4. SDKの複雑さはどう隠す？
A. **Wasm Component Model + WITプラグイン**で吸収する。

## Q5. 実機への配布/更新の体験は？
A. **`imago deploy`で一発配布**。K8sのように**アップロード→再起動**を基本とし、将来的に**blue‑green**も視野。

## Q6. blue‑greenのヘルス判定は？
A. 実行コンポーネントが**ヘルスチェック用WITに適合**していることを前提にする。

## Q7. 安全境界（ケイパビリティ）はどう設計する？
A. **ケイパビリティベース**で制御。
- 優先して制御する権限: **fs → net → dev**
- **allowlistは`imago.toml`に記述**
- **fs/net** は **WASI v2** の制限をそのまま利用
- **dev** は独自定義（**デバイス単位**）

## Q8. imago daemonの役割（重要順）は？
A. **コンポーネント実行 → WITプラグイン解決 → リモートデプロイ受け口 → ログ/状態収集 → 権限チェック**。

## Q9. WITプラグインの解決タイミングは？
A. **dynamic linking**は**deploy時に取得し、起動時にリンク**。**static linking**は開発者側で任意に行う。

## Q10. プラグインの配布元は？
A. **OCI registry**を想定。`imago.toml`の`[[dependencies]]`に`name/version/registry`を記述する。**詳細仕様はTBD**。

## Q11. デプロイの最小アーティファクトは？
A. **Wasm + `imago.toml`**。`wrangler.toml`に近い運用で、**実行時に必要な設定を同梱**する。

## Q12. `imago.toml`でまず必須にしたいキーは？
A. **main / target / args / dependencies**。
- **main**: `wrangler.toml`のように、ビルドされたWasmを指す

## Q13. 1つの`imago.toml`は複数コンポーネントを扱う？
A. **1つの`imago.toml`は1つだけ**。将来的に**cargo workspace的な複数管理**も視野。

## Q14. 複数管理の想定ユースケースは？
A. **1デバイスに複数コンポーネント同時デプロイ**。将来的に**コンポーネント同士の通信**も視野。

## Q15. コンポーネント同士の通信はどうする？
A. **WIT経由が理想**だが実現が難しい可能性あり。**設計時に決める（TBD）**。

## Q16. `target`は何を指す？
A. **認証情報は別管理**する想定。`wrangler`のような**login方式**か、**SSH鍵**で扱うのがよさそう。

## Q17. サポートするターゲット形は？
A. **単一ホスト**と**ホストグループ**の両方をやりたい。

## Q18. `imago logs <process id>`のprocess idは？
A. **deployごとに払い出される実行インスタンスID**。

## Q19. `imago deploy`のロールバックはどうする？
A. **TBD（未検討）**。

## Q20. `imago deploy`のフィードバックは？
A. **process id / 成否ステータス / ヘルスチェック結果**を返す。

## Q21. Wasmランタイムはどうする？
A. **まずは固定で開始し、将来的に切り替え可能にする**。

## Q22. 初期の固定ランタイム候補は？
A. **Wasmtime**。Wasm Component Model対応のため。まずは**RISC‑V**で動くようにする。

## Q23. 最初にサポートするデバイス/環境は？
A. **NanoKVM**を第一ターゲットにする。

## Q24. NanoKVMのOS/環境は？
A. **独自のLinuxビルド**が前提。まずは**Linux対応**を優先し、**OpenWrtも対応したい**。
- NanoKVMのイメージは **LicheeRV Nano SDK + MaixCDK** ベースで、**Buildroot**でビルドされている（Sipeed Wiki）

## Q25. RISC‑Vの次に狙いたいアーキは？
A. **OpenWrtのMIPS**。ただし現時点では優先度は高くない。

## Q26. Componentのエントリポイントはどう決める？
A. **typeを選択**できるようにする（例: `type = "http"`）。
- 例: **プロセス / HTTP handler / sockets handler**
- それぞれ **wasi/cli:run**, **wasi/http:incoming_handler** など、既存WITに合わせる

## Q27. `type`はどこに書く？
A. **`imago.toml` に記述する**。

## Q28. `type`ごとのランタイム責務は？
A. ランタイム側が必要な解決を担う。
- **cli**: シンプル（args/env/exit code）
- **http**: TLSなどもランタイム側で面倒を見る（cf. Workers的な体験）
- **socket**: 指定されたTCP/UDPソケットを開いて接続

## Q29. `http`タイプの公開方法は？
A. **Wasmtimeの`serve`サブコマンド準拠**で進める。
- `wasi:http/proxy` worldで実行する
- `--addr=0.0.0.0:8081` のように**バインド先を指定**できる
- 具体仕様はWasmtimeの挙動に合わせる

## Q30. `http`のaddr/port/TLS設定はどこで管理する？
A. **すべて`imago.toml`で管理する**。

## Q31. TLS証明書/鍵はどう持つ？
A. **ファイルパス参照**で、**deploy時にアセットとして同梱**する。

## Q32. `imago deploy`のアセット同梱はどこまで許す？
A. **任意ファイルを同梱できるようにする**。

## Q33. 同梱アセットの配置先は？
A. **`imago.toml`でマウント先を指定**する。

## Q34. `args`は静的か？
A. **`imago.toml`に静的に書く**。

## Q35. 環境変数（env）はどうする？
A. **deploy時に注入**（`.env`などから）。

## Q36. `imago.toml`に含めない方がいい情報は？
A. **`wrangler.toml`のように `vars` と `secret` を分けて管理**する。

## Q37. `vars`/`secret`の管理方法は？
A. **`vars`は`imago.toml`に記述**、**`secret`は`.env`や別ファイルで管理**する。

## Q38. `.env`はどう読み込む？
A. **自動的に読む**のを基本にし、**別ファイル指定も可能**にする。

## Q39. `imago.toml`の最小構成は？
A. **`main` / `type` / `target`** があれば動く。

## Q40. ホストグループのdeploy挙動は？
A. **全台並列**で行う。

## Q41. 一部失敗時の扱いは？
A. **policyで決められるようにする**。

## Q42. policyはどこで指定する？
A. **設定系はすべて`imago.toml`に書く**。

## Q43. policyの種類は？
A. **よくあるやつ**でOK。
- 例: **all‑or‑nothing / best‑effort / quorum** など

## Q44. 再起動ポリシーは？
A. **よくあるやつ**でOK（例: `always / on-failure / never`）。

## Q45. ログの保持/保存は？
A. **デバイス内でローテーション**しつつ、**syslog等の外部送信**もできるようにしたい。

## Q46. メトリクス/監視は？
A. **まずは無し**。

## Q47. imagoのアップデート方針は？
A. **明示的に手動更新**。

## Q48. `imago service install`の対応方針は？
A. **ホストシステムを自動検出して対応**する。

## Q49. `imago deploy`の通信方式は？
A. **独自プロトコル**を使う。ベースは**QUIC + WebTransport**。

## Q50. 認証/認可はどうする？
A. **mTLS**を使いたい。

## Q51. mTLS証明書の配布/更新は？
A. **手動で配布**。特定の**デプロイグループで同一証明書を共有**できると良い。

## Q52. QUIC/WebTransportのデータ形式は？
A. **CBOR**。

## Q53. プロトコルの最小コマンドは？
A. **run / stop / logs / ps**。

## Q54. `run`でWasmをどう渡す？
A. **ローカルパス / OCI URI / バイナリ直送**を全部やる。

## Q55. OCI URIの認証は？
A. **`imago.toml` + `.env`**で扱う。

## Q56. `logs`はストリーム/過去ログ？
A. **docker compose logsと同様に両方**。

## Q57. `ps`の出力項目は？
A. **docker compose psと同じ感じ**。

## Q58. `run`と`deploy`の差分は？
A. **`run`は既にデプロイ済みのものを起動**、**`deploy`は`.env`等を処理してデプロイ**する。

## Q59. `deploy`後は自動起動する？
A. **自動起動**する。

## Q60. ネットワーク制御はどこで指定？
A. **基本は`imago.toml`**。ただし**deployで`.env`等を反映**するため、**上書き可能**にする。

## Q61. capabilities指定の粒度は？
A. **`capabilities.fs` / `capabilities.net.outgoing` / `capabilities.net.udp`** のように**明示指定**できる形が良い。

## Q62. `capabilities.dev`の指定方法は？
A. **`/dev`以下のデバイス名**で指定する。

## Q63. `capabilities.fs`のマウント方式は？
A. **WASIのやり方そのまま**で良い。

## Q64. リソース制限はどうする？
A. **`imago.toml`の`limits`欄で指定**する。あわせて、**全権限のprivilegedアプリ**も書けるようにしたい。

## Q65. privilegedアプリの指定方法は？
A. **`privileged = true`**。

## Q66. `limits`の単位は？
A. **よくある感じ**でOK。`timeout`は**type（cli/http等）で意味が変わる**。

## Q67. `http`のtimeoutは何を指す？
A. **ハンドラ全体の実行時間**。

## Q68. `cli`のtimeoutは？
A. **無しでOK**。

## Q69. limits/featureの有効化は？
A. **ビルド時に機能のオン/オフを切り替えられる**ようにしたい（例: cgroupなど）。

## Q70. `imago.toml`のバリデーションは？
A. **CLIが厳格にチェック**する。

## Q71. `imago.toml`のversioningは？
A. **wranglerの`compatibility_date`的な方式**でやりたい。

## Q72. `imago dev build`のビルド対象は？
A. **wranglerの`[build]`と同様に、コマンド指定でビルド**する。

## Q73. `imago dev build`の出力先は？
A. **`build/`に成果物をまとめる**。deployでそのまま送れる形にする。

## Q74. `build/`の最小セットは？
A. **必要ならWasm**、**`imago.toml`+`.env`をまとめた何か**、**assets**。

## Q75. `imago.toml`+`.env`のまとめ形式は？
A. **`manifest.json`的な形式**が良い。TOML表現差を避けるため、**CBORのファイル**でも良い。

## Q76. `build/`成果物の扱いは？
A. **そのまま送る**。

## Q77. `build`と`deploy`の関係は？
A. **deployが自動でbuildを呼ぶ**。ハッシュで**ビルド済みならスキップ**し、**同一ハッシュなら再デプロイせず再起動のみ**。

## Q78. ハッシュ対象は？
A. **Wasm + manifest + assets** の**全部**。

## Q79. 同一ハッシュ時の再起動は？
A. **既存プロセスを終了→再起動**。

## Q80. `stop`のgraceful/forceは？
A. **`stop --force`で強制停止**。SIGINT等を送れると良い。

## Q81. graceful終了の待ち時間は？
A. **`imago.toml`で指定できるようにする**。

## Q82. `logs`の出力形式は？
A. **プレーン / JSONの両対応**。

## Q83. `logs`のフィルタは？
A. **当面は不要**。

## Q84. `ps`の状態（status）は？
A. **docker compose psと同じ感じ**。

## Q85. インスタンスの`name`は？
A. **`imago.toml`で指定**する。

## Q86. `name`が無い場合は？
A. **必須（必ず指定）**にする。

## Q87. `target`は必須？
A. **必須**。

## Q88. `main`は必須？
A. **必須**。

## Q89. `type`は必須？
A. **必須**。

## Q90. `imago.toml`の必須キーは？
A. **`name / main / type / target`**。

## Q91. 推奨キーは？
A. **args / dependencies / capabilities / limits / vars / assets / restart_policy** など。

## Q92. `dependencies`のバージョン指定は？
A. **semver + range**でいきたい。

## Q93. 依存解決はlockfile固定？
A. **lockfileで固定**したい。

## Q94. lockfile名は？
A. **`imago.lock`**。

## Q95. `imago dev update`の役割は？
A. **WIT依存の解決 + lockfile更新**。

## Q96. `imago dev update`は強制アップデート？
A. **強制アップデート**。

## Q97. `imago dev update`の対象は？
A. **dependenciesのみ**（assets/manifestは触らない）。

## Q98. `imago dev build`はlockfile必須？
A. **必須**。

## Q99. `imago dev build`失敗時の出力は？
A. **stdout/stderrをそのまま出す**。

## Q100. `target`の書き方は？
A. **単一ホストとグループを`imago.toml`で分けて記述**する（例: `host`/`hosts`）。

## Q101. `target`のdefault/env設計は？
A. **wranglerみたいに `env` を用意する**（B）。

## Q102. 採用する方式は？
A. **B（`env`方式）**。

## Q103. `env`で上書きできる範囲は？
A. **すべて**。

## Q104. envの切り替え方法は？
A. **CLIで指定**し、**未指定ならデフォルト**を使う。

## Q105. デフォルトenv名は？
A. **未指定ならベース設定（wranglerと同じ）**。

## Q106. `--env`時の`.env`読込ルールは？
A. **`.env.<env>`のみ**を読む（例: `.env.prod`）。

## Q107. `imago deploy`のdry-runは？
A. **欲しい**。

## Q108. `dry-run`で見たい情報は？
A. **よくある感じ**でOK（例: hash / 送信ファイル一覧 / 差分有無）。

## Q109. manifestのファイル名は？
A. **`manifest.json`**。

## Q110. `manifest.json`の最低限項目は？
A. **全部入れてOK**（name/main/type/target、env反映後のvars、assets一覧、dependencies解決結果、全体hash）。

## Q111. `manifest.json`にsecretは入れる？
A. **入れてOK**（そのまま送信する）。

## Q112. runtimeが参照する設定は？
A. **全部**。

## Q113. manifestの場所は固定？
A. **`build/manifest.json`固定**でOK。

## Q114. `build/`内のassets配置は？
A. **固定でOK**（例: `build/assets/...`）。

## Q115. `type`の実際の値は？
A. **`cli / http / socket`**。

## Q116. `socket`タイプの設定は？
A. **listenポートを`imago.toml`に記述**し、**TCP/UDP両対応**。**inbound/outbound**の扱いも指定できるようにする。

## Q117. `socket`のport範囲/許可ルールは？
A. **`imago.toml`の`socket`セクション**で指定する。

## Q118. `http`のパス/ルーティングは？
A. **コンポーネントに委譲**する。

## Q119. 複数コンポーネント時のHTTPポート競合は？
A. **ポート指定を必須**にする。

## Q120. TLS終端はどこで？
A. **imago runtime**で行う。

## Q121. `http`の実行モデルは？
A. **常駐プロセスが複数リクエストを捌く**。

## Q122. `cli`の実行モデルは？
A. **基本は単発実行**。ただし**内部ループがあれば常駐**になる。

## Q123. `socket`の実行モデルは？
A. **常駐プロセスが複数接続を捌く**。

## Q124. `socket`の同時接続上限は？
A. **`limits`で指定可能**、**デフォは無制限**。

## Q125. `socket`のlisten addrは？
A. **`imago.toml`で指定可能**、**省略時はデフォルト（例: 0.0.0.0）**。

## Q126. `http`のbind addrは？
A. **`imago.toml`で指定可能**、**省略時はデフォルト（例: 0.0.0.0）**。

## Q127. policyのデフォルトは？
A. **all‑or‑nothing**。

## Q128. 再起動ポリシーのデフォルトは？
A. **never**。

## Q129. `capabilities`のデフォルトは？
A. **全部拒否（deny）**にする。

## Q130. `capabilities`未指定時は？
A. **暗黙で全拒否**。

## Q131. `privileged = true`の挙動は？
A. **capabilitiesは全無視（全許可）**。

## Q132. `privileged = true`の警告は？
A. **何もしない**。

## Q133. 外部ログ送信の設定場所は？
A. **`imago.toml`の`logging`セクション**。

## Q134. syslog送信の優先順は？
A. **UDP → TCP → TLS**（一般的に多い順）。

## Q135. `run`の必須入力は？
A. **`name`指定だけで実行できる**ようにする（他はデプロイ済み設定を使う）。

## Q136. `run`でargs/env上書きは？
A. **しない（デプロイ済み設定を使う）**。

## Q137. `run`でtarget/env切替は必要？
A. **指定できるようにする**。未指定なら**デフォルト**。

## Q138. `run`対象が無い場合は？
A. **エラー**。

## Q139. `ps`のtarget/env指定は？
A. **指定できるようにする**。未指定なら**デフォルト**。

## Q140. `logs`のtarget/env指定は？
A. **指定できるようにする**。未指定なら**デフォルト**。
