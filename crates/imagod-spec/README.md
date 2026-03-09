# imagod-spec

`imagod` の shared contract を所有する crate です。

この crate は runtime が常時依存できる軽い正本で、公開面は次の 3 つに限定します。

- `command_contract`: command lifecycle と command protocol の contract
- `wire`: request / response / event / log datagram を含む wire contract
- `ipc`: manager-runner 間の IPC contract と plugin / capability metadata

`imagod-spec` 自体は `imago-protocol` や `imagod-ipc` の contract type を再 export しません。  
逆に `imago-protocol` は CBOR codec helper、`imagod-ipc` は transport / auth helper としてこの crate の型を使います。

formal spec、projection、docgen、model checking は `imagod-spec-formal` に分離しています。
