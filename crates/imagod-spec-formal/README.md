# imagod-spec-formal

`imagod-spec-formal` は `imagod-spec` の canonical domain contract を基底に、`imagod` 全体の authored semantics を 1 つの `SystemSpec` として記述する crate です。

公開面は次の module に限定します。

- `bounds`
  - explicit / symbolic / docgen で共有する探索上限
- `system`
  - canonical `SystemSpec`、`SystemState`、`SystemAction`
- `manager_view`
  - manager lifecycle の derived projection と focused invariant
- `control_view`
  - session/auth/request の derived projection と focused invariant
- `service_view`
  - service lifecycle / binding の derived projection と focused invariant
- `operation_view`
  - command / RPC の derived projection と focused invariant
- `authz_view`
  - message / operation authorization の derived projection と focused invariant

`imagod-spec-formal` では `system.rs` だけが authored transition source です。  
manager/control/service/operation/authz は独立 state machine ではなく、canonical `SystemState` / `SystemAction` の projection として扱います。

`ModelInstance` は broad explicit case と symbolic-focused case を同一 `SystemSpec` 上で持ちます。  
explicit では multi-session / multi-service scenario、symbolic では 1 session / 1 service / 1 authority に絞った parity case を検証します。
