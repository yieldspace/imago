# MVP計画（NanoKVM）

## 目的（MVP）
- **NanoKVMで動作**する imago を作る
- 次の2ユースケースを動かす
  - **Syslog収集→一時保存→外部転送（再送あり）**
  - **NanoKVM画面キャプチャ → Discord Webhook送信**

## MVPで必要な基盤機能（Q&Aから抽出）
### 1) ランタイム/デバイス対応
- Wasmtime を **RISC‑V向け**に動作させる
- NanoKVMは **Buildrootベースの独自Linux**（LicheeRV Nano SDK + MaixCDK）

### 2) デプロイ/実行フロー
- `imago dev build` → `build/` に成果物生成
  - `build/manifest.json` を生成（**env/secret含め全部**）
- `imago deploy` は **build → package → upload → apply → restart**
  - パッケージは **送信時にtar.gz化**
  - **SHA‑256**で整合性チェック
  - applyで**展開/配置/manifest登録/旧版クリーンアップ**
  - 旧版クリーンアップは **起動前**
- 配置先: **`/etc/imago/<name>/<hash>/`**

### 3) 通信・管理
- CLI ↔ imagod: **QUIC + WebTransport + CBOR**
- 認証: **mTLS（手動配布、グループ単位で共有可）**
- 最小コマンド: **run / stop / logs / ps**

### 4) 設定/互換性
- `imago.toml`必須キー: **name / main / type / target**
- `env`方式（wrangler式）
- `compatibility_date`方式
- `imago.lock` で依存固定（`imago dev update` で更新）

### 5) 権限/ケイパビリティ
- デフォルトは **全拒否**
- 明示指定: `capabilities.fs / net / dev`
- `dev` は **/dev配下のデバイス名**で指定
- `privileged = true` で **全許可**

### 6) ログ
- `logs` は **name / process id** フィルタ
- `-f` で follow、表示順は **古い→新しい**

### 7) socket/http/cli 実行モデル
- `cli`: 基本単発（内部ループなら常駐）
- `http`: 常駐、TLS終端は runtime
- `socket`: 常駐、TCP/UDP + inbound/outbound 指定

## MVPユースケース別の必要機能
### A. Syslog収集→外部転送
- **type=socket** で syslog受信（**UDPのみ / ポート514固定**）
- フォーマットは **よくある方のRFC（仮: RFC3164）**
- **fs書込み許可**（一時保存）
- 一時保存の**サイズ/保持は環境変数で指定**
- **net outbound許可**（外部転送）
- 失敗時のリトライは **アプリ側ロジック**

### B. NanoKVM画面キャプチャ → Discord
- **NanoKVM専用プラグイン**（WIT）
- **定期実行**でキャプチャ
- **MJPEGストリームAPIからJPEG取得**
- **送信頻度: 1分に1回**
- **net outbound許可**（Discord Webhook）
- `secret` は **.env → manifest.json**に含めて送信

## MVPから除外（現時点）
- blue‑green
- ヘルスチェックの失敗許容回数（TBD）
- メトリクス

## 実装ステップ案
1. **imagod基盤**（QUIC+WebTransport+CBOR、run/stop/logs/ps）
2. **build/deploy基盤**（manifest生成、tar.gz、SHA‑256、apply）
3. **NanoKVM向け起動/配置**（/etc/imago/<name>/<hash>）
4. **socket type実装**（TCP/UDP inbound/outbound）
5. **syslog app**（保存/再送）
6. **NanoKVMプラグイン**（画面キャプチャ）
7. **Discord webhook送信**

## MVP追加仕様（決定）
### Syslog
- 受信プロトコル: **UDP**
- ポート: **514固定**
- フォーマット: **よくある方のRFC（仮: RFC3164）**
- 一時保存の**サイズ/保持は環境変数で指定**

### NanoKVMキャプチャ
- トリガー: **定期実行**
- 取得方法: **MJPEGストリームAPIからJPEG抽出**
- 送信頻度: **1分に1回**

### Discord
- Webhook送信頻度: **1分に1回**

## MVP追加仕様（決定・追記）
- Syslogフォーマット: **RFC3164**
- 一時保存の保存先: **`/var/tmp/imago-cache`**
- Discord送信失敗時: **リトライしない**

## 環境変数（デフォルト）
- `SYSLOG_CACHE_DIR` = `/var/tmp/imago-cache`
- `SYSLOG_CACHE_MAX_MB` = `100`
- `SYSLOG_CACHE_TTL_SEC` = `86400`

## 未決（要確認）
- なし
