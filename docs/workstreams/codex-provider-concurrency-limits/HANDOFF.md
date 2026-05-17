# Handoff

Current state: provider/endpoint local concurrency limits are implemented for v5 route graph execution.

Implemented:

- `ProviderConcurrencyLimits` on provider and endpoint config.
- Effective route candidate concurrency metadata, with endpoint overrides and optional shared `limit_group`.
- Local `ConcurrencyLimiter` permit registry in `ProxyService`.
- Route runtime saturation snapshots before selection.
- Non-blocking permit acquisition immediately before upstream transport, with permit lifetime held through buffered response completion or SSE stream finalization.
- `concurrency_saturated` skip reason in route executor and routing explain.
- User-facing configuration docs and README examples.
- Persisted provider spec API reads, preserves, updates, and clears provider/endpoint `limits`.
- GUI/TUI routing preview formats saturated skips as `concurrency_saturated(active=N/limit=M)`.

Validation evidence is recorded in `EVIDENCE_AND_GATES.md`.

Known caution: the working tree had pre-existing line-ending-only status noise before this workstream. `config_v2.rs` and `config_v4.rs` now both contain real implementation changes from this workstream and should not be reverted without reviewing the diff.

Follow-ons, if desired: expose active/limit columns in TUI/GUI provider tables and add CLI flags for writing `limits` directly.
