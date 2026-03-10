# imagod-spec-formal

`imagod-spec` が持つ shared contract を基底に、`imagod` の formal spec と runtime conformance を表す crate です。

主な公開面は次のとおりです。

- `system`: daemon-visible contract を束ねる unified top-level spec
- `deploy`, `supervision`, `rpc`, `plugin_platform`, `manager_runtime`, `session_auth`, `wire_protocol`
- `command_projection`, `router_projection`, `session_auth_projection`, `logs_projection`, `runtime_projection`, `manager_runtime_projection`

`nirvash-core` / `nirvash-macros` / `nirvash-docgen` への依存はこの crate に閉じ込めます。  
runtime crate は通常依存では `imagod-spec` の contract だけを使い、dev/test で formal な期待値が必要な境界だけ `imagod-spec-formal` に依存します。

projection spec は `nirvash_projection_contract` で `ProbeState/ProbeOutput -> SummaryState/SummaryOutput -> AbstractState/ExpectedOutput` の写像を宣言し、runtime 側は concrete probe を観測するだけで grouped conformance を回せる形を正本にしています。
