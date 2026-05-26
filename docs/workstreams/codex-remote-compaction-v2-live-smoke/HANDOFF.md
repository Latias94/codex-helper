# Handoff

Status: Complete.

Implemented an explicit-only live-smoke case named `remote_compaction_v2`. It sends a real streaming
`/responses` request containing `compaction_trigger` and only passes when the stream has exactly one
compaction output item plus `response.completed`.

Residual risk: fake upstream tests prove helper wire shape and classification, not that any selected
real relay/provider supports Codex remote compaction v2. The real smoke remains manual because it can
spend upstream quota.
