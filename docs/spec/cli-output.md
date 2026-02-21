# CLI Output Specification

## 1. 目的

`imago` CLI の標準出力フォーマット（`Rich` / `Plain` / `Json`）と、JSON line 契約を固定する。

関連仕様:

- 観測仕様: [`observability.md`](./observability.md)
- 通信手順: [`deploy-protocol.md`](./deploy-protocol.md)

## 2. 出力モード

CLI は起動時に 1 つの出力モードを選ぶ。

- `Rich`: 対話端末向け（progress UI を表示）
- `Plain`: CI など非対話環境向け（プレーンテキスト）
- `Json`: 機械処理向け（JSON Lines）

### 2.1 判定優先順位

1. `--json` が指定されていれば `Json`
2. それ以外で `CI=true`（`CI=1` 含む）なら `Plain`
3. 上記以外は `Rich`

本仕様の優先順位は `--json > CI=true > Rich` を正本とする。

## 3. JSON line 契約

### 3.1 共通

- 1 行に 1 JSON オブジェクトを出力する（JSON Lines）。

### 3.2 `command.summary`

通常コマンドの終端時に 1 行出力する。

- `type`: `"command.summary"`
- `command`: コマンド名
- `status`: `"completed"` または `"failed"`
- `duration_ms`: 実行時間（ミリ秒）
- `timestamp`: RFC 3339 UTC 文字列（例: `2026-02-20T12:34:56Z`）
- `meta`: 補助メタデータ（`map<string,string>`）
- `error`: 失敗時メッセージ（成功時は `null`）
  - 複数行文字列を許容する。
  - 実装は `causes:` / `hints:` セクションを含む診断文を設定しうる。

### 3.3 `command.error`

`command.summary` を出さないコマンドの失敗通知に使う。

- `type`: `"command.error"`
- `command`: コマンド名
- `message`: エラーメッセージ
  - 複数行文字列を許容する。
  - 実装は `causes:` / `hints:` セクションを含む診断文を設定しうる。
- `stage`: 失敗ステージ
- `code`: エラーコード

`command.error` は失敗時のみ出力し、成功時は出力しない。

## 4. `logs --json` 契約

`logs` コマンドの `Json` モードは line-only 出力を行う。

- 成功時:
  - `type="log.line"` の行のみを出力する
  - `command.summary` は出力しない
- 失敗時:
  - 失敗時のみ `command.error` を 1 行出力する
  - `command.summary` は出力しない

`log.line` のフィールドは以下とする。

- `type`: `"log.line"`
- `name`: サービス名
- `stream`: `"stdout"` / `"stderr"` / `"composite"`
- `timestamp`: Unix time（秒）の文字列
- `log`: 1 行のログ本文（改行なし）

## 5. `ps --json` 契約

`ps` コマンドの `Json` モードは line-only 出力を行う。

- 成功時:
  - `type="service.state"` の行のみを出力する
  - `command.summary` は出力しない
- 0 件時:
  - 行を出力せず正常終了する（`command.summary` も出力しない）
- 失敗時:
  - 失敗時のみ `command.error` を 1 行出力する
  - `command.summary` は出力しない

`service.state` のフィールドは以下とする。

- `type`: `"service.state"`
- `name`: サービス名
- `state`: `"running"` / `"stopping"` / `"stopped"`
- `release`: リリース識別子
- `started_at`: 起動時刻（protocol の Unix 秒文字列を CLI 実行マシンのローカル時刻文字列へ変換した値。変換不可時は入力文字列をそのまま出力）

## 6. 実装反映ノート（CLI UI mode/summary 契約 / 2026-02-20）

- 出力モード判定優先順位を `--json > CI=true > Rich` に固定した。
- JSON 契約を `command.summary` と `command.error` に分離し、`logs --json` は `log.line` + 失敗時 `command.error` のみとした。

## 7. 実装反映ノート（接続コンテキスト統合表示 / 2026-02-20）

- 対象コマンド（`deploy` / `run` / `stop` / `logs` / `bindings cert upload` / `bindings cert deploy`）は、接続前に以下キーを `Rich` / `Plain` のみで表示する。  
  `cli`, `project`, `service`, `target`, `remote`, `server_name`
- `hello.negotiate` 成功後は、以下キーを `Rich` / `Plain` のみで表示する。  
  `authority`, `resolved`, `server_version`, `limit_chunk_size`, `limit_max_inflight_chunks`, `limit_deploy_stream_timeout_secs`
- `Rich` は dim + インデント（`  > ...`）で補助表示し、`Plain` は `[info]` 1行表示とする。
- `Json` モードは追加の情報行を出さず、既存 JSON line 契約を維持する。
- `compose deploy` / `compose logs` はサービス個別処理の前に `profile`, `target`, `services` の全体サマリを 1 行表示する。

## 8. 実装反映ノート（起動ヘッダー表示 / 2026-02-20）

- CLI 起動時に `Rich` / `Plain` モードのみ先頭へ 2 行のヘッダーを表示する。  
  1 行目は `imago <version>`、2 行目は同じ文字幅の横線（`─`）とする。
- `Json` モードでは起動ヘッダーを表示せず、既存 JSON line 契約（`command.summary` / `log.line` / `command.error`）を維持する。

## 9. 実装反映ノート（失敗診断メッセージ拡張 / 2026-02-21）

- `build` / `deploy` / `run` / `stop` / `compose` / `logs` / `certs` / `update` は失敗時に詳細診断メッセージを `CommandResult` へ格納する。
- 詳細診断メッセージは複数行で、`causes:` と `hints:` を含みうる。
- 進行表示用の `command_finish(..., false, detail)` は短文サマリ（`err.to_string()`）を維持し、終端要約/JSON エラー側で詳細診断を扱う。
- `logs --json` の特例（`command.summary` 非出力）は維持しつつ、失敗時 `command.error.message` は詳細診断文を出力する。

## 10. 実装反映ノート（`ps --json` line 契約 / 2026-02-21）

- `ps --json` は `type="service.state"` の JSON Lines を出力する契約を追加した。
- `service.state` は `name` / `state` / `release` / `started_at` を含み、`state` は `running` / `stopping` / `stopped` を許可する。
- `ps --json` は `command.summary` を出力せず、失敗時のみ `command.error` を 1 行出力する。
