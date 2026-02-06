# Core ↔ Deploy Protocol（MVP）

このドキュメントは、imago CLI（デプロイ基盤）と imagod（コア）をつなぐプロトコルの設計案。
MVPスコープは **build → package → upload → apply → restart**。

## 目的
- CLI からのデプロイ手順を **一貫した API と状態遷移**で表現する
- 転送・検証・適用・再起動を分離して **失敗時の復旧**を簡単にする
- 将来の拡張（resume / roll back / blue-green）に備える

## 役割
- **client**: `imago` CLI（build/deployの実行主体）
- **server**: `imagod`（デーモン / ランタイム / 配置の担当）

## トランスポート
- **QUIC + WebTransport + CBOR**
- **mTLS 認証**（接続時に相互認証）

### チャンネル設計
- **Control stream**: CBOR メッセージ（request/response/events）
- **Data stream**: tar.gz を raw で送るストリーム

## メッセージ共通フォーマット
- CBOR map を基本にする
- MVPでは **string key** で読みやすく

例:
```
{
  "type": "deploy.begin",
  "id": "req-uuid",
  "payload": { ... }
}
```

### 共通フィールド
- `type`: メッセージ種別
- `id`: request id（UUID推奨）
- `payload`: 具体データ
- `error`: 失敗時のみ（code/message）

### エラーコード（MVP）
- `E_UNAUTHORIZED`（認証失敗）
- `E_BAD_REQUEST`（必須項目不足）
- `E_BAD_MANIFEST`（manifest不正）
- `E_HASH_MISMATCH`（sha256不一致）
- `E_BUSY`（同時デプロイ不可）
- `E_APPLY_FAILED`（展開/配置失敗）
- `E_NOT_FOUND`（対象なし）
- `E_INVALID_STATE`（状態遷移不正）
- `E_INTERNAL`（内部エラー）

## 状態遷移（MVP）
1. connect + hello
2. deploy.begin
3. deploy.upload
4. deploy.apply
5. runtime.restart

## メッセージ定義（MVP）

### 1) hello / hello.ok
**目的**: プロトコル互換確認
- request payload:
  - `protocol_version`（例: 1）
  - `client_version`
- response payload:
  - `protocol_version`
  - `server_version`
  - `features`（例: ["upload_stream", "apply", "restart"]）

### 2) deploy.begin / deploy.accepted
**目的**: アップロード準備と事前検証
- request payload:
  - `name`
  - `type`（cli/http/socket）
  - `target`（host/group）
  - `package_sha256`
  - `package_size`
  - `manifest`（任意）
  - `manifest_sha256`（任意）
- response payload:
  - `deploy_id`
  - `already_uploaded`（true/false）

**補足**
- `manifest` を送った場合、server は事前検証して NG ならこの時点でエラー
- `already_uploaded=true` なら upload を省略可能

### 3) deploy.upload.ready / deploy.upload.complete
**目的**: tar.gz の送信
- deploy.begin 成功後、server が `deploy.upload.ready` を返す
- payload:
  - `deploy_id`
  - `stream_id`（data stream の識別子）

**データ送信**
- client は `stream_id` で data stream を開く
- tar.gz の raw bytes を送る（サイズは `package_size` に一致）

**完了応答**
- server は sha256 を検証して `deploy.upload.complete`
- payload:
  - `deploy_id`
  - `verified`（true/false）

### 4) deploy.apply / deploy.applied
**目的**: 展開と配置
- request payload:
  - `deploy_id`
- server の処理:
  - `/etc/imago/<name>/<hash>/` に展開
  - manifest 登録
  - 旧版クリーンアップ（起動前）
- response payload:
  - `release_id`
  - `path`

### 5) runtime.restart / runtime.restarted
**目的**: 新版起動
- request payload:
  - `name` もしくは `release_id`
- response payload:
  - `process_id`
  - `status`（running/failed）

### 6) deploy.abort（任意）
**目的**: クライアント都合の中断
- request payload:
  - `deploy_id`
- server は一時領域をクリーンアップ

## 例: デプロイシーケンス（MVP）
- hello → hello.ok
- deploy.begin → deploy.accepted
- deploy.upload.ready
- data stream で tar.gz 送信
- deploy.upload.complete
- deploy.apply → deploy.applied
- runtime.restart → runtime.restarted

## 検証ルール
- tar.gz 内の `manifest.json` は必須
- `manifest_sha256` を送った場合は一致必須
- `package_sha256` は必須

## 拡張ポイント（MVP後）
- resume upload（offset 指定）
- blue-green deploy
- rollback
- progress event（deploy.progress）
- 差分転送

---

### 参考: build/ の最小構成
- `manifest.json`
- `app.wasm`
- `imago.lock`（任意）

この構成が tar.gz で送られる前提。
