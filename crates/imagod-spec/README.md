# imagod-spec

`imagod-spec` は `imagod` の shared contract を所有する crate です。

現行の source-of-truth は command contract 単体ではなく、次の canonical domain modules です。

- `identity`
  - transport principal / session role / bounded service-session-authority ids
- `authorization`
  - external message permission、internal operation permission、binding grant、denial reason
- `manager`
  - boot/config/listening/shutdown/maintenance lifecycle
- `service`
  - artifact/runtime をまとめた service lifecycle
- `operation`
  - command slot と manager-auth fragment
- `rpc`
  - local/remote RPC outcome fragment
- `system`
  - daemon-visible `SystemEvent` と canonical `SystemStateFragment`

既存の `wire` / `messages` / `ipc` / `envelope` / `error` / `validate` は残しますが、役割は adapter です。  
domain semantics は上の canonical modules に置き、wire 型はそこへの transport-specific projection として扱います。

`imagod-spec` 自体は runtime が常時依存できる軽い契約層です。  
formal semantics、model checking、derived view は `imagod-spec-formal` 側で扱います。
