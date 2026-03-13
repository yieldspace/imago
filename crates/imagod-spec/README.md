# imagod-spec

`imagod` の shared contract を所有する crate です。

この crate は runtime が常時依存できる軽い正本で、公開面は次の module に限定します。

- `command_contract`: command lifecycle と command protocol の contract
- `wire`: request / response / event / log datagram を含む wire contract
- `ipc`: manager-runner 間の IPC contract と plugin / capability metadata
- `messages`: daemon / control plane の message contract
- `envelope`: envelope contract
- `error`: error contract
- `validate`: contract validation helper

`imagod-spec` 自体は `imago-protocol` や `imagod-ipc` の contract type を再 export しません。  
逆に `imago-protocol` は CBOR codec helper、`imagod-ipc` は transport / auth helper としてこの crate の型を使います。

`summary` / `probe` / `projection` / runtime conformance surface はこの crate に含めません。  
formal semantics、model checking、docgen は `imagod-spec-formal` 側で扱います。
