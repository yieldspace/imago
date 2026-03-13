# imagod-spec-formal

`imagod-spec-formal` は `imagod-spec` の shared contract を基底に、`imagod` の formal semantics を記述する crate です。

公開面は次の module に限定します。

- `atoms`: bounded model で使う service/session/stream/authority の atom
- `bounds`: explicit / symbolic の両 backend で共有する有限境界
- `manager_plane`: boot/config/listening/shutdown/maintenance の manager semantics
- `control_plane`: session accept/auth/request-response/log-follow/authority upload の control semantics
- `service_plane`: artifact upload/commit/promote/rollback と service lifecycle の semantics
- `operation_plane`: command lifecycle、binding、local/remote RPC の semantics
- `system`: 4 plane を合成した top-level system semantics

`SystemState` / `SystemAction` はこの crate の正本です。  
`ModelInstance` は同一 spec 上で `explicit_*` と `symbolic_*` の 2 lane を持ち、explicit では広い scenario、symbolic では AST-native に安全な focused case を検証します。

projection、probe/summary surface、runtime conformance trait はこの crate に含めません。  
実コード検証は後続の別 boundary として扱い、この crate は formal semantics、model checking、docgen に責務を限定します。
