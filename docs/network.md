# Imago Networkの設計

現在は他のserviceの関数を実行する土台となるUDSが用意されているだけだが、
Network RPC は manager を中継点にし、他ノードのサービス呼び出しを `manager control` 経由で実行する。
呼び出し元 service は `imago:node/rpc` で local manager に要求を渡し、local manager が remote manager に接続して RPC を中継する。
これにより local/remote の呼び出し経路を manager control に統一し、IoT デバイス同士の連携を行えるようにする。

## デプロイ

`--env` は廃止済み。
また、`imago-compose.toml`の概念を作成し、複数のサービスを一つのimagodにdeployしたり、複数のserviceの設定をすることができるようになる。

```toml
[[compose.nanokvm.services]]
imago = "path/to/imago.toml"

[[compose.nanokvm-pro.services]]
imago = "path/to/pro/imago.toml"

[[compose.nanokvm-pro.services]]
imago = "path/to/pro/imago.toml"

[profile.nanokvm-mini]
config = "nanokvm"

[profile.nanokvm-pro]
config = "nanokvm-pro"

[target.nanokvm-cube]
remote = "..."
server_name = "..."
client_key = "certs/client.key"
```

compose update: `imago compose update nanokvm-mini`  
compose build: `imago compose build nanokvm-mini --target nanokvm-cube`  
compose deploy: `imago compose deploy nanokvm-mini --target nanokvm-cube`  
compose logs: `imago compose logs nanokvm-mini --target nanokvm-cube --name <service>`

`compose build/deploy/logs` は `imago-compose.toml` の `[target.<name>]` を使って接続先を解決する。
このとき各 service の `imago.toml` 側に `target.<name>` を定義する必要はない。

実ファイル例: `examples/imago-compose-bindings/`

## サービス

他のサービスからアクセスが可能なサービスは、`type = "rpc"`で定義される。
RPC の接続確立・呼び出し・切断は manager control が受け付けてハンドリングする。
remote 呼び出しは runner へ直接接続せず、`local manager -> remote manager control -> target runner` の順で中継する。

## 接続・実行

imagoは、特定のrpcのノードに対して接続するためのnative pluginを持つ。

```wit
package imago:node@0.1.0;

interface rpc {
  resource connection {
    invoke: func(target-service: string, interface-id: string, function: string, args-cbor: list<u8>) -> result<list<u8>, string>;
    disconnect: func();
  }
  connect: func(addr: string) -> result<connection, error>
  local: func() -> result<connection, error>
}
```

これで接続されたrpcはcloseされるまで論理コネクションを保持する。
addrは`rpc://ip:port`の形。portは任意(defaultになる)。
`connect(addr)`で作られた remote connection は manager control 中継で管理される。`connect` / `invoke` / `disconnect` は local manager control へ送られ、local manager が remote manager control を経由して `ResolveInvocationTarget -> RunnerInboundRequest::Invoke` を実行する。
`local()`で作られたconnectionも同一ノード内の UDS (`manager_control_endpoint` / runner endpoint) 経由で同じ invoke パスを使い、TLS 認証なしで呼び出す。

接続元のサービスは、
```toml
[[bindings]]
name = "..."
wit = "warg://sizumita:ferris@0.1.0"
```

のように `wit` へ source を登録する（`file://...` / `warg://...`）。

imagoは、`imago update`コマンドが実行された時にこの `wit` source を読み、
WIT package 内の全 interface を `manifest.bindings` の `<package>/<interface>` 形式へ展開する。
そのうえで全ての関数の定義の先頭に`imago:node/rpc`のconnectionを引数を受け取るように改造された上でwit/depsに置かれる。
関数が実行されると、そのrpcの先にがservice名（bindings.name）とwitの構造が同じサービスが存在するかを確認したのち（失敗したらerrorが帰る）、関数を実行する。
resourceは渡すことができないし、witに存在したら`imago update`コマンド時にエラーになる。
rpcが閉じられれている場合は呼び出しがエラーになるため、全ての関数の返り値はresultを上からwrapしたものになる。

## 認証

接続されるimagod側のimagod.tomlに、`client_public_keys`というフィールドを追加し、ここに登録されているed25519公開鍵は認証される。
接続する側のimagod.tomlはTOFUで一回接続したら`known_public_keys`フィールドに登録し（ipとのペア）、次回から違った場合は接続時エラー。

imago-cliから、`imago bindings cert deploy --to <rpc先のip(:port)> --from <rpc元のip(:port)>`コマンドを実行すると、
imago-cliがどちらにも接続できる場合、自動で公開鍵をfromからtoに対してアップロードし、鍵の設定を再読み込みする。
このための鍵追加・削除のimago-cliとの間のrpcを追加する。

`imago bindings cert upload <public_key> --to <rpc先のip(:port)>`のようなコマンドも用意する。
