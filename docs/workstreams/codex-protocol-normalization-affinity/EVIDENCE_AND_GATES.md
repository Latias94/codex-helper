# Codex Protocol Normalization And Affinity — Evidence And Gates

Status: Complete
Last updated: 2026-05-20

## Gate Set

### Targeted request normalization

```powershell
cargo nextest run -p codex-helper-core request_content_encoding --no-fail-fast
```

### Targeted session affinity

```powershell
cargo nextest run -p codex-helper-core prompt_cache_key_affinity --no-fail-fast
```

### Closeout gates

```powershell
cargo fmt --check
cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity --no-fail-fast
cargo nextest run -p codex-helper-core
```

If the full core gate is too expensive, record the narrower gate and reason here.

## Fresh Evidence

### 2026-05-20 — CPNA-010 Scope Freeze

Claim: this workstream should be separate from `codex-responses-websocket-relay`.

Evidence:

- Existing WebSocket workstream is active but focuses on upgrade handling and WS live smoke.
- The new tasks are HTTP request normalization and session identity extraction, which affect normal
  `/responses` and `/responses/compact` before any upstream relay sees the request.
- `repo-ref/sub2api` already decodes compressed request bodies and uses `prompt_cache_key` as an
  explicit session signal; helper should preserve that capability instead of requiring direct Codex
  connection to sub2api.
- `repo-ref/new-api` supports `gzip/br` request decode but not `zstd`, so helper-side normalization
  improves compatibility without pretending the relay supports extra official features.

Commands: source inspection and workstream review only.

### 2026-05-20 — CPNA-020/030/040/050 Implementation Evidence

Claim: the implementation matches the lane scope and still does not synthesize upstream features.

Evidence in working tree:

- `crates/core/src/proxy/request_encoding.rs` adds request `Content-Encoding` normalization for
  `zstd`, `gzip` / `x-gzip`, `br`, and zlib/raw `deflate`, including stacked encodings and a
  64 MiB decompressed body cap.
- `crates/core/src/proxy/request_context.rs` calls normalization before JSON inspection, session
  fallback extraction, model/effort/service-tier overrides, routing context construction, and
  upstream forwarding.
- `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` explicitly preserves raw compressed bodies and
  headers for rare relay compatibility; the default remains auto-normalization.
- `crates/core/src/proxy/client_identity.rs` preserves header identity priority and adds decoded
  JSON `prompt_cache_key` fallback.
- `crates/core/src/proxy/responses_websocket.rs` applies the same session fallback to the first
  WebSocket `response.create` frame.
- README, README_EN, CONFIGURATION, CONFIGURATION.zh, CHANGELOG, and the generated config template
  document default normalization, the passthrough escape hatch, prompt-cache affinity, and the
  explicit non-goal that helper does not add missing compact/WebSocket/hosted-tool support.

Targeted test evidence from the implementation pass:

```powershell
cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity --no-fail-fast
```

Result observed before final doc updates: 10 tests run, 10 passed, 578 skipped. The final closeout
gate below must be rerun after formatting and docs updates.

### 2026-05-20 — CPNA-060 Fresh Closeout Gates

Claim: request content-encoding normalization, passthrough escape hatch, and prompt-cache affinity
are implemented, documented, and verified for `codex-helper-core`.

Fresh commands:

```powershell
cargo fmt --check
```

Result: passed with exit code 0.

```powershell
cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity --no-fail-fast
```

Result: passed with exit code 0. Summary: 11 tests run, 11 passed, 578 skipped.

```powershell
cargo nextest run -p codex-helper-core
```

Result: passed with exit code 0. Summary: 589 tests run, 589 passed, 0 skipped.

Behavior proven:

- Supported request encodings decode before upstream forwarding: `zstd`, `gzip`, `br`,
  zlib `deflate`, raw `deflate`, plus stacked `Content-Encoding` values.
- The HTTP proxy forwards decoded JSON without stale `Content-Encoding`.
- Corrupt zstd is rejected as a client error before upstream hit.
- `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` preserves raw compressed body and header.
- Header session identity remains stronger than `prompt_cache_key`.
- Decoded `prompt_cache_key` records route affinity and keeps `/responses/compact` on the same
  selected provider after a `/responses` fallback.

Review notes:

- Workstream compliance: CPNA-020/030/040/050/060 scope is satisfied; no helper-side
  `/responses/compact` fallback or vendor fingerprinting was added.
- Code quality: normalization is centralized at the HTTP request-preparation boundary, before JSON
  inspection and routing; WebSocket first-frame session fallback reuses the shared client identity
  helper. Residual risk is limited to unusual relays that require raw compressed bodies, covered by
  the documented env passthrough.
