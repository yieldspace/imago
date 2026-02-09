# MVP実装タスク詳細（依存順・入れ子MVP）

`MVP_PLAN.md` と `CONCEPT.md` のQ&A確定事項に基づき、MVPを「各フェーズ単体で成立する内側MVP」の連結として分解した実装タスク定義。

## 使い方
- このドキュメントは、実装順と依存を固定した「Issue/Sub-issue定義」である。
- 実装は Phase 0 から順に進め、前フェーズ未完了で次フェーズへ進まない。
- 各親Issueは、`目的` `スコープ` `依存` `Sub-issue` `完了条件` `除外事項` を満たした時点で完了とする。

## Issue ID規約
- 親Issue: `P{phase}-I{number}`
- Sub-issue: `P{phase}-S{number}`

## フェーズ構造（依存順）

| フェーズ | 内側MVPの到達点 | 依存 |
|---|---|---|
| Phase 0 | 仕様凍結MVP（実装判断をなくす） | なし |
| Phase 1 | Deploy Core MVP（`imago deploy`で`cli`型を単一ホスト起動） | Phase 0 |
| Phase 2 | Ops MVP（run/stop/logs/psで運用可能） | Phase 1 |
| Phase 3 | Syslog MVP（UDP514受信→一時保存→外部転送） | Phase 2 |
| Phase 4 | Capture MVP（NanoKVMキャプチャ→Discord送信） | Phase 3 |
| Phase 5 | DX/検証MVP（clone→deploy体験と受け入れ試験） | Phase 4 |

## 公開API/インターフェース/型への重要変更

| 区分 | 変更内容 | 対応Issue |
|---|---|---|
| CLI | `imago dev build`, `imago deploy`, `imago run`, `imago stop`, `imago logs`, `imago ps`, `imago dev update`, `imago deploy --dry-run` のMVP仕様固定 | `P1-I3`, `P2-I1`, `P2-I2`, `P2-I3`, `P4-I1`, `P5-I1` |
| 設定 | `imago.toml` 必須キー `name/main/type/target`、`env`上書き、`capabilities`、`limits.shutdown_timeout`、`runtime.restart_policy` | `P0-I1` |
| マニフェスト | `build/manifest.json` の最小必須フィールド・ハッシュ対象・secret同梱仕様 | `P0-I2`, `P1-I3` |
| Deploy Protocol | QUIC+WebTransport+CBOR+mTLS、`hello.negotiate`、`deploy.prepare`、`artifact.push`、`artifact.commit`、`deploy.execute`、`operation.get/watch`、`operation.cancel`、構造化エラー契約 | `P0-I3`, `P0-I4`, `P1-I1`, `P1-I2`, `P1-I5` |
| Runtime | `/etc/imago/<name>/<hash>/` 配置、旧版クリーンアップ、`cli/http/socket` 実行モデル、ログ/プロセス管理 | `P1-I4`, `P2-I1`, `P2-I2`, `P3-I1` |
| Plugin | `[[dependencies]]` + `imago.lock` + `imago dev update` のWIT解決導線 | `P4-I1` |

## Phase 0: 仕様凍結MVP

### P0-I1: 設定仕様凍結
- 目的:
  - `imago.toml` の必須/任意/既定値を固定し、実装判断をなくす。
- スコープ:
  - 必須キー `name/main/type/target`。
  - `env` 上書きルール（未指定時ベース設定、`--env`指定時 `.env.<env>` を読む）。
  - `capabilities` と `privileged` の解釈。
  - `limits.shutdown_timeout` と `runtime.restart_policy` の既定値。
- 依存:
  - なし。
- Sub-issue:
  - `P0-S1`: 必須キー定義。
  - `P0-S2`: `env` 上書き規則。
  - `P0-S3`: `capabilities`/`privileged` 規則。
  - `P0-S4`: `limits`/`restart_policy` 既定値定義。
- 完了条件:
  - 設定仕様表とバリデーション要件が同一章で矛盾なく定義される。
- 除外事項:
  - blue-green向け設定。
  - メトリクス設定。

### P0-I2: manifest仕様凍結
- 目的:
  - `build/manifest.json` の固定フォーマットを定義し、CLI/daemon双方の入力契約を確定する。
- スコープ:
  - `name/main/type/target`、env反映後vars、assets一覧、dependencies解決結果、全体hash、secret同梱方針。
  - ハッシュ対象: `Wasm + manifest + assets`。
- 依存:
  - なし。
- Sub-issue:
  - `P0-S5`: manifestフィールド定義。
  - `P0-S6`: hash対象固定。
  - `P0-S7`: secret同梱方針明文化。
- 完了条件:
  - 正常系/異常系のmanifest例が定義され、Phase 1実装に直接渡せる。
- 除外事項:
  - 差分配信用メタデータ。

### P0-I3: Deploy Protocol再定義
- 目的:
  - deployプロトコルを再設計し、単一の実装契約に固定する。
- スコープ:
  - `hello.negotiate`、`deploy.prepare`、`artifact.push`、`artifact.commit`、`deploy.execute`、`operation.get/watch`、`operation.cancel`。
  - 構造化エラー（`code/message/retryable/stage/details`）。
  - 冪等性（`idempotency_key`）と前提条件（`expected_current_release`）契約。
  - 自動ロールバック（`auto_rollback=true`既定）。
- 依存:
  - なし。
- Sub-issue:
  - `P0-S8`: 新メッセージ契約定義。
  - `P0-S9`: 状態遷移定義。
  - `P0-S10`: 構造化エラー契約。
  - `P0-S11`: 冪等性/前提条件（CAS）契約。
  - `P0-S12`: Operationライフサイクル定義。
  - `P0-S13`: 自動ロールバック既定化。
- 完了条件:
  - クライアント/サーバ実装者が追加判断なしで実装できる仕様になる。
- 除外事項:
  - blue-green。
  - 差分デプロイ。

### P0-I4: Deploy観測性契約
- 目的:
  - 長時間実行・再接続時に必要な追跡情報を契約化する。
- スコープ:
  - `operation.watch` イベント仕様。
  - `correlation_id`/`request_id` 追跡規約。
  - operation保持期間と取得範囲。
- 依存:
  - `P0-I3`。
- Sub-issue:
  - `P0-S14`: operation.watchイベントスキーマ。
  - `P0-S15`: correlation/request識別規約。
  - `P0-S16`: 保持期間/取得範囲ルール。
- 完了条件:
  - 切断復帰後でも同一operationを追跡できる要件が明文化される。
- 除外事項:
  - 監視ダッシュボードUI。

## Phase 1: Deploy Core MVP

### P1-I1: Transport/Auth実装
- 目的:
  - QUIC+WebTransport+mTLS接続と `hello.negotiate` を成立させる。
- スコープ:
  - 接続確立、証明書ロード、不正証明書拒否、limits/featuresの受け渡し。
- 依存:
  - `P0-I3`。
- Sub-issue:
  - `P1-S1`: 接続層実装。
  - `P1-S2`: 証明書ロード。
  - `P1-S3`: `hello.negotiate` 実装。
  - `P1-S4`: 不正証明書時 `E_UNAUTHORIZED`。
- 完了条件:
  - CLIとimagodが相互認証で接続し、交渉結果を固定できる。
- 除外事項:
  - 自動証明書配布・ローテーション。

### P1-I2: artifact session + resumable upload実装
- 目的:
  - 中断復帰可能なartifact転送をサーバ側で成立させる。
- スコープ:
  - `deploy.prepare`、欠損レンジ返却、`artifact.push`、`artifact.commit`、digest検証。
- 依存:
  - `P1-I1`。
- Sub-issue:
  - `P1-S5`: `deploy.prepare` とセッション生成。
  - `P1-S6`: `artifact.push`（chunk受領/ack）。
  - `P1-S7`: `missing_ranges` による再開制御。
  - `P1-S8`: `artifact.commit` と最終検証。
- 完了条件:
  - 中断後に不足レンジのみ再送してcommit成功できる。
- 除外事項:
  - コンテンツ差分転送。

### P1-I3: CLI build/package/deploy実装
- 目的:
  - `imago deploy` 1コマンドで build→package→prepare→upload→commit→execute→watch を完結させる。
- スコープ:
  - `imago.toml` と `.env` 読込、`build/manifest.json` 生成、`idempotency_key` 生成/再利用、再開転送、operation追跡表示。
- 依存:
  - `P0-I1`、`P0-I2`、`P1-I2`、`P1-I5`。
- Sub-issue:
  - `P1-S9`: `imago.toml` + `.env` 読込。
  - `P1-S10`: `build/manifest.json` 生成。
  - `P1-S11`: `idempotency_key` 管理とresume upload制御。
  - `P1-S12`: operation進捗表示と最終サマリ表示。
- 完了条件:
  - CLIが最終的に deploy結果と rollback結果を区別して表示できる。
- 除外事項:
  - `deploy --dry-run`。

### P1-I4: 配置と起動実装
- 目的:
  - `/etc/imago/<name>/<hash>/` 世代配置と `cli` 型起動を成立させる。
- スコープ:
  - 展開、manifest登録、旧版クリーンアップ（起動前）、Wasmtime `cli` 実行。
- 依存:
  - `P1-I2`。
- Sub-issue:
  - `P1-S13`: 展開先実装。
  - `P1-S14`: 旧版クリーンアップ（起動前）。
  - `P1-S15`: Wasmtime `cli` 実行。
  - `P1-S16`: 起動失敗時の最小ロールバック。
- 完了条件:
  - 新版起動後にプロセスが `running` として観測できる。
- 除外事項:
  - multi-tenant隔離最適化。

### P1-I5: Operation実行基盤
- 目的:
  - 配置/起動を非同期 Operation として追跡可能にする。
- スコープ:
  - `deploy.execute`、`operation.get/watch`、`operation.cancel`、自動ロールバック実行。
- 依存:
  - `P0-I3`、`P0-I4`、`P1-I4`。
- Sub-issue:
  - `P1-S17`: `deploy.execute` 実装。
  - `P1-S18`: `operation.get/watch` 実装。
  - `P1-S19`: `operation.cancel` 実装。
  - `P1-S20`: restart失敗時の自動ロールバック実装。
- 完了条件:
  - 接続切断後の再接続でも同一operationを追跡できる。
- 除外事項:
  - 複数operationの優先度スケジューリング。

## Phase 2: Ops MVP

### P2-I1: run/stop基盤
- 目的:
  - `run` と `stop (--force含む)` の運用コマンドを安定化する。
- スコープ:
  - デプロイ済み参照起動、graceful停止、timeout後SIGKILL、対象なし時エラー。
- 依存:
  - `P1-I4`。
- Sub-issue:
  - `P2-S1`: デプロイ済み参照起動。
  - `P2-S2`: graceful停止（SIGINT）。
  - `P2-S3`: timeout後SIGKILL。
  - `P2-S4`: 対象なし時 `E_NOT_FOUND`。
- 完了条件:
  - `shutdown_timeout` に従う停止挙動が再現する。
- 除外事項:
  - 高度な再起動戦略。

### P2-I2: logs基盤
- 目的:
  - 過去ログとfollowを同一コマンドで扱えるようにする。
- スコープ:
  - ログ保存、`logs -f`、`name/process_id` ORフィルタ、plain/json出力。
- 依存:
  - `P2-I1`。
- Sub-issue:
  - `P2-S5`: ログ保存。
  - `P2-S6`: `logs -f`。
  - `P2-S7`: `name/process_id` ORフィルタ。
  - `P2-S8`: plain/json出力。
- 完了条件:
  - 表示順は古い→新しい、followはCtrl+Cで終了できる。
- 除外事項:
  - 外部ログ集約システム連携。

### P2-I3: ps基盤
- 目的:
  - 実行状態確認を `ps` で標準化する。
- スコープ:
  - テーブル出力、JSON出力、名前順ソート。
- 依存:
  - `P2-I1`。
- Sub-issue:
  - `P2-S9`: テーブル出力。
  - `P2-S10`: JSON出力。
  - `P2-S11`: 名前順ソート。
- 完了条件:
  - `docker compose ps` 相当の主要項目を返す。
- 除外事項:
  - カスタムクエリ言語。

### P2-I4: ケイパビリティ基盤
- 目的:
  - deny-by-defaultと`privileged=true`の境界を実行時に保証する。
- スコープ:
  - fs/net/dev deny初期化、allowlist適用、privileged時全許可。
- 依存:
  - `P0-I1`、`P1-I4`。
- Sub-issue:
  - `P2-S12`: fs/net/dev deny初期化。
  - `P2-S13`: allowlist適用。
  - `P2-S14`: privileged時全許可。
- 完了条件:
  - 未指定は全拒否、明示指定時のみ許可が成立する。
- 除外事項:
  - capability監査UI。

## Phase 3: Syslog MVP

### P3-I1: socket runtime実装
- 目的:
  - `type=socket` 実行モデルをMVP範囲で成立させる。
- スコープ:
  - UDP/TCPソケット起動、inbound/outbound制御、listen addr/port設定、同時接続制限フック。
- 依存:
  - `P2-I4`。
- Sub-issue:
  - `P3-S1`: UDP/TCPソケット起動。
  - `P3-S2`: inbound/outbound制御。
  - `P3-S3`: listen addr/port設定。
  - `P3-S4`: 同時接続制限フック。
- 完了条件:
  - UDP 514固定要件で待受可能になる。
- 除外事項:
  - 高度なL4ロードバランシング。

### P3-I2: Syslog受信/保存
- 目的:
  - RFC3164受信と一時保存を実装する。
- スコープ:
  - RFC3164パース、`/var/tmp/imago-cache` 保存、容量/TTL環境変数反映。
- 依存:
  - `P3-I1`。
- Sub-issue:
  - `P3-S5`: RFC3164パーサ。
  - `P3-S6`: `/var/tmp/imago-cache` 保存。
  - `P3-S7`: サイズ/TTL環境変数反映。
- 完了条件:
  - 受信データが容量・TTL制限下で保持される。
- 除外事項:
  - RFC5424対応。

### P3-I3: 外部転送/再送
- 目的:
  - 外部転送失敗時にアプリ側で再送可能にする。
- スコープ:
  - outbound送信、失敗時キュー、再送ポリシー。
- 依存:
  - `P3-I2`。
- Sub-issue:
  - `P3-S8`: outbound送信。
  - `P3-S9`: 失敗時キュー。
  - `P3-S10`: 再送ポリシー実装。
- 完了条件:
  - 一時的障害から復帰後、未送信分が送信される。
- 除外事項:
  - exactly-once配信保証。

## Phase 4: Capture MVP

### P4-I1: WIT依存解決
- 目的:
  - `imago dev update` と `imago.lock` で依存固定を成立させる。
- スコープ:
  - dependencies解決、lockfile更新、build時lock必須チェック。
- 依存:
  - `P0-I1`。
- Sub-issue:
  - `P4-S1`: dependencies解決。
  - `P4-S2`: lockfile更新。
  - `P4-S3`: build時lock必須チェック。
- 完了条件:
  - 同一入力から再現ビルドできる。
- 除外事項:
  - 依存脆弱性自動修復。

### P4-I2: NanoKVMプラグイン実装
- 目的:
  - MJPEGからJPEG抽出するNanoKVM専用プラグインを実装する。
- スコープ:
  - WIT定義、MJPEG取得、JPEG抽出API。
- 依存:
  - `P4-I1`。
- Sub-issue:
  - `P4-S4`: WIT定義。
  - `P4-S5`: MJPEG取得。
  - `P4-S6`: JPEG抽出API。
- 完了条件:
  - 1回の呼び出しでJPEGバイト列が返る。
- 除外事項:
  - 動画エンコード。

### P4-I3: 定期実行基盤
- 目的:
  - 1分ごとのキャプチャ実行を安定化する。
- スコープ:
  - スケジューラ、失敗時ハンドリング、タイムスタンプ付与。
- 依存:
  - `P4-I2`。
- Sub-issue:
  - `P4-S7`: スケジューラ。
  - `P4-S8`: 実行失敗ハンドリング。
  - `P4-S9`: タイムスタンプ付与。
- 完了条件:
  - 60秒周期で連続実行できる。
- 除外事項:
  - cron互換式の複雑スケジュール。

### P4-I4: Discord送信実装
- 目的:
  - Discord Webhookへ1分ごとに画像送信する。
- スコープ:
  - `.env`からsecret読込、multipart送信、失敗時リトライなし。
- 依存:
  - `P4-I3`。
- Sub-issue:
  - `P4-S10`: `.env` からsecret読込。
  - `P4-S11`: multipart送信。
  - `P4-S12`: 送信失敗時リトライなし。
- 完了条件:
  - 1分ごとに送信され、失敗時は次周期へ進む。
- 除外事項:
  - 複数Webhook同時配信。

## Phase 5: DX/検証MVP

### P5-I1: dry-run/進捗表示
- 目的:
  - `deploy` の副作用なし確認と可視化を提供する。
- スコープ:
  - `--dry-run`、hash/送信一覧/差分表示、ステップ表示。
- 依存:
  - `P1-I3`。
- Sub-issue:
  - `P5-S1`: `--dry-run`。
  - `P5-S2`: hash/送信一覧表示。
  - `P5-S3`: step表示（build/package/prepare/upload/commit/execute/watch）。
- 完了条件:
  - 実送信せず差分判断ができる。
- 除外事項:
  - 進捗UIダッシュボード。

### P5-I2: Quickstart成立
- 目的:
  - clone→deploy体験を手順通りに再現可能にする。
- スコープ:
  - テンプレート整備、ドキュメント更新、初回導入手順検証。
- 依存:
  - `P5-I1`、`P3-I3`、`P4-I4`。
- Sub-issue:
  - `P5-S4`: テンプレート整備。
  - `P5-S5`: ドキュメント更新。
  - `P5-S6`: 初回導入手順検証。
- 完了条件:
  - 新規ユーザーが手順のみでNanoKVM起動まで到達できる。
- 除外事項:
  - GUIセットアップウィザード。

### P5-I3: 受け入れ試験/リリースゲート
- 目的:
  - MVP出荷判定を明文化し、リリース可否を一意にする。
- スコープ:
  - E2Eシナリオ、障害注入試験、合否チェックリスト。
- 依存:
  - 全Issue。
- Sub-issue:
  - `P5-S7`: chunk欠落/破損の障害注入試験。
  - `P5-S8`: 接続断→再開の障害注入試験。
  - `P5-S9`: CAS不一致の障害注入試験。
  - `P5-S10`: restart失敗→rollbackの障害注入試験。
- 完了条件:
  - 主要2ユースケース成功と、主要プロトコル障害系の期待挙動が確認される。
- 除外事項:
  - 本番監視基盤の完全自動化。

### P5-I4: 運用耐性検証
- 目的:
  - 長時間運用時のプロトコル追跡性と再実行安全性を検証する。
- スコープ:
  - 長時間operation再接続追跡、重複requestの冪等性、rollback失敗時の可観測性検証。
- 依存:
  - `P1-I5`、`P5-I3`。
- Sub-issue:
  - `P5-S11`: 長時間operation再接続追跡試験。
  - `P5-S12`: 重複request冪等性試験。
  - `P5-S13`: rollback失敗時の可観測性試験。
- 完了条件:
  - 運用時の再接続・再試行・失敗追跡が手順化され、判定可能になる。
- 除外事項:
  - SLA/SLO策定。

## テストケースと受け入れシナリオ
1. 設定バリデーション: `name/main/type/target` 欠落時に明確なエラー。
2. build成果物: `build/manifest.json` 生成とsecret同梱。
3. 同一 `idempotency_key` で `deploy.prepare` を再送しても同一 `deploy_id` が返る。
4. upload中断後、`missing_ranges` のみ再送して `artifact.commit` できる。
5. chunk hash不一致で `E_CHUNK_HASH_MISMATCH` を返し、再送で回復できる。
6. `expected_current_release` 不一致で `E_PRECONDITION_FAILED` を返す。
7. `deploy.execute` 後に切断しても `operation.watch` 再接続で追跡再開できる。
8. restart失敗時に自動ロールバックし、最終状態が `rolled_back` になる。
9. rollback自体が失敗した場合 `E_ROLLBACK_FAILED` と失敗段階が返る。
10. run/stop: graceful待機後にSIGKILLへ移行。
11. logs: ORフィルタ、古い→新しい順、`-f` 追従停止。
12. socket/syslog: UDP 514でRFC3164受信、TTL/容量制限適用。
13. syslog転送: 外部停止時保留、復旧後再送。
14. capture/discord: 1分周期送信、失敗時リトライなし。
15. dry-run: hash/ファイル一覧/差分表示のみで副作用なし。
16. E2E: clone→`imago deploy` だけで2ユースケース起動。

## 明示的な前提・既定値
1. 優先ターゲットはNanoKVM、MVPは単一ホスト基準。
2. 通信は QUIC + WebTransport + CBOR、認証はmTLS手動配布。
3. `manifest.json` にsecretを含めて送信。
4. tar.gzは送信時生成（ローカル永続ファイルは作らない）。
5. 旧版クリーンアップは新版起動前。
6. capabilities未指定は全拒否、`privileged=true` は全許可。
7. `logs` フィルタ条件はOR、表示順は古い→新しい。
8. Syslogは UDP/514固定、RFC3164、既定値は `SYSLOG_CACHE_DIR=/var/tmp/imago-cache` `SYSLOG_CACHE_MAX_MB=100` `SYSLOG_CACHE_TTL_SEC=86400`。
9. Discord送信失敗時はリトライしない。
10. フェーズは依存順で直列実行。
11. deploy失敗時は `auto_rollback=true` を既定とする。
