# Codex Relay Capability Profile - Evidence And Gates

Status: Active
Last updated: 2026-05-19

## Smallest Current Repro

The first proof should be a pure static capability-profile test:

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

## Gate Set

### Static Profile Gate

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Proves that Codex client gates are represented consistently for patch modes, auth shape, provider
identity, model metadata, and WebSocket disabled behavior.

### Relay Probe Gate

```bash
cargo nextest run -p codex-helper-core codex_relay_probe
```

Proves that relay responses can be classified without assuming sub2api-specific behavior.

### Operator Surface Gate

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Proves that the CLI/admin output includes expected support, observed support, confidence, and
mismatch reasons.

### Recommendation Gate

```bash
cargo nextest run -p codex-helper-core codex_patch_mode_recommendation
```

Proves deterministic recommendations for `default`, `imagegen-bridge`, `official-relay-bridge`,
and `official-imagegen-bridge`.

### Formatting Gate

```bash
cargo fmt --check
```

Proves Rust formatting did not drift.

### Broader Closeout Gate

```bash
cargo nextest run -p codex-helper-core
```

Use the core package gate for closeout unless the final implementation touches TUI, GUI, or another
package; then add the touched package gates.

### Review Gate

Run `review-workstream` before accepting task or lane completion. Record blocking findings, missing
gates, and residual risks here or link to the review note.

## Evidence Anchors

- `docs/workstreams/codex-relay-capability-profile/DESIGN.md`
- `docs/workstreams/codex-relay-capability-profile/TODO.md`
- `docs/workstreams/codex-relay-capability-profile/MILESTONES.md`
- `crates/core/src/codex_integration.rs`
- `crates/core/src/codex_capability_profile.rs`
- `crates/core/src/proxy/models_compat.rs`

## Fresh Evidence

### 2026-05-19 - RCP-020 static profile

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Result: passed, 7 tests run.

Proves: the static capability profile maps patch modes and Codex model metadata to expected remote
compaction v1, hosted image generation, WebSocket, web search, apply_patch, and reasoning-summary
exposure. Also proves the helper-translated OpenAI `/models` response can be interpreted as a Codex
catalog.

Re-run after review tightening:

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Result: passed, 9 tests run.

Proves additionally: auth shape can be measured independently from patch mode, and WebSocket support
comes from provider metadata rather than being hard-coded to a mode.

```bash
cargo nextest run -p codex-helper-core codex_capability_profile_understands_translated_openai_models_list
```

Result: passed, 1 test run.

Proves: `models_compat` translation produces enough Codex metadata for the static profile to expose
hosted image generation and remote compaction expectations under `official-imagegen-bridge`.

```bash
cargo fmt --check
```

Result: passed.

Proves: Rust formatting is clean after RCP-020.

Review gate: self-review found one important design risk before completion: auth shape, provider
identity, and WebSocket support must be measurable independently from patch mode so future probes can
report real state instead of a mode-derived approximation. The static profile input was tightened to
accept those values explicitly, and two tests were added to lock that behavior.

Skipped: `cargo nextest run -p codex-helper-core` because RCP-020 only adds a pure static profile
module plus a focused `models_compat` unit test. Run the core package gate before lane closeout or
before merging larger probe/API work.

### 2026-05-19 - RCP-030 relay probe primitives

```bash
cargo nextest run -p codex-helper-core codex_relay_probe
```

Result: passed, 10 tests run.

Proves: `/models` probe classification distinguishes Codex `models`, raw OpenAI `data`, malformed
JSON, and unsupported endpoints; `/responses` and `/responses/compact` classify validation-only
400/422 responses as supported endpoints, classify explicit compact-not-supported responses as
unsupported, send exactly one validation request with resolved upstream auth, target only the
explicit upstream passed to the probe client, and avoid double `/v1` URL construction.

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Result: passed, 9 tests run.

Proves: the RCP-030 split between raw probe shape detection and normal `/models` translation did
not regress the static profile or the Codex catalog translation used by the normal proxy path.

```bash
cargo fmt --check
```

Result: passed.

Proves: Rust formatting is clean after RCP-030.

Review gate: self-review found one important observability risk before completion: reusing the
normal `/models` response helper inside probes translated OpenAI `data` lists into Codex `models`
before classification, hiding whether a relay needs helper-side translation. The decode and
translation steps are now separate; normal proxy responses still translate for Codex, while probes
decode without translation and report `translation_required = true` for raw OpenAI lists.

Skipped: `cargo nextest run -p codex-helper-core` because RCP-030 adds a bounded probe primitive
and focused tests for its public seam. Run the core package gate before lane closeout or before
merging the operator-facing RCP-040/RCP-050 work.

### 2026-05-19 - RCP-040 operator capability diagnostics

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Result: passed, 1 test run.

Proves: the admin API exposes `/__codex_helper/api/v1/codex/relay-capabilities`, advertises it
through the API capabilities manifest and operator-summary links, sends exactly one active probe to
each selected upstream endpoint (`/models`, `/responses`, `/responses/compact`), reports expected
Codex capability exposure from patch mode plus translated model metadata, preserves observed raw
upstream `/models` shape as `openai_data_list` with `translation_required = true`, and reports a
remote compaction mismatch when `official-imagegen-bridge` expects `/responses/compact` but the
upstream returns compact-not-supported.

```bash
cargo nextest run -p codex-helper-core codex_relay_probe
```

Result: passed, 10 tests run.

Proves: adding raw probe observations for the admin API did not regress single-upstream probe
classification or bounded request behavior.

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Result: passed, 9 tests run.

Proves: expected capability computation still agrees with patch mode, auth shape, provider identity,
and translated Codex model metadata after the admin API started consuming it.

```bash
cargo fmt --check
```

Result: passed.

Proves: Rust formatting is clean after RCP-040.

Review gate: self-review found no blocking findings. Important design check: the operator endpoint
uses POST and calls the probe client directly for one selected upstream, so diagnostics stay opt-in
and do not mutate normal routing state, request ledger, session affinity, passive health, or runtime
health. Residual risk: `/responses` and `/responses/compact` probes are validation-only but still
send upstream requests, so automatic polling should not be added without rate limiting and explicit
operator intent.

Skipped: `cargo nextest run -p codex-helper-core` because RCP-040 is an admin API surface over the
already-tested profile/probe primitives. Run the core package gate before lane closeout or before
merging RCP-050/RCP-060.

### 2026-05-19 - RCP-050 patch-mode recommendations

```bash
cargo nextest run -p codex-helper-core codex_patch_mode_recommendation
```

Result: passed, 6 tests run.

Proves: the recommendation matrix chooses `official-imagegen-bridge` only when ordinary
`/responses`, `/responses/compact`, and selected model image capability are all supported; chooses
`official-relay-bridge` when compact is supported but the selected model should not expose hosted
image generation; chooses `imagegen-bridge` when imagegen gates are available but remote compaction
is unsupported or unknown; chooses `default` when no official-like gate is proven; and warns instead
of upgrading when the ordinary `/responses` endpoint is unavailable.

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Result: passed, 15 tests run.

Proves: the static profile and recommendation tests agree in one module, including translated
OpenAI `/models` catalog handling.

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Result: passed, 2 tests run.

Proves: the admin Codex relay diagnostics response now includes the recommendation object and, for a
relay whose `/models` list can be translated and whose `/responses/compact` is unsupported,
recommends moving from `official-imagegen-bridge` to `imagegen-bridge`. Also proves that when the
payload omits `patch_mode`, diagnostics use the current Codex switch status before falling back to
helper config/default.

```bash
cargo nextest run -p codex-helper-core codex_relay_probe
```

Result: passed, 10 tests run.

Proves: recommendation integration did not regress raw relay probe classification.

```bash
cargo fmt --check
```

Result: passed.

Proves: Rust formatting is clean after RCP-050.

Review gate: self-review found no blocking findings. Important design check: recommendations are a
pure function over current mode, model catalog profile, ordinary `/responses` support, and
`/responses/compact` support. They deliberately do not treat missing probe evidence as support and
do not claim image generation entitlement, because hosted image generation is not actively probed in
this lane.

Skipped: `cargo nextest run -p codex-helper-core` because RCP-050 changes a pure recommendation
module plus the existing admin diagnostics surface. Run the core package gate before closeout.

### 2026-05-19 - RCP-060 documentation and changelog

```bash
cargo fmt --check
```

Result: passed.

Proves: documentation updates did not leave Rust formatting drift after RCP-060.

```bash
rg "relay-capabilities|translation_required|official-imagegen-bridge|recommendation" docs/CONFIGURATION.md docs/CONFIGURATION.zh.md CHANGELOG.md -n
```

Result: passed; matches found in English configuration docs, Chinese configuration docs, and
CHANGELOG.md.

Proves: the docs and changelog mention the new Codex relay capabilities endpoint, the conservative
recommendation output, model catalog translation reporting, and the bridge mode affected by the
diagnostics.

Review gate: self-review found no blocking doc/code mismatch. The docs describe the endpoint as
`POST`, identify the three active probes, explain that normal routing/retry/ledger/health paths are
not involved, cover sub2api and non-sub2api model catalog behavior, and keep hosted image generation
active probes, WebSocket relay, and remote compaction v2 outside the enabled behavior.

Skipped: `cargo nextest run -p codex-helper-core` because RCP-060 only changed markdown docs and
changelog. Run the core package gate before closing the lane.

### 2026-05-19 - RCP-070 closeout

```bash
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Result: passed, 2 tests run.

Proves: the operator diagnostics endpoint still advertises itself, emits expected/observed/mismatch
and recommendation fields, probes one selected upstream per endpoint, preserves model translation
observability, and defaults omitted `patch_mode` from the current Codex switch status.

```bash
cargo nextest run -p codex-helper-core codex_patch_mode_recommendation
```

Result: passed, 6 tests run.

Proves: the recommendation matrix remained stable after closeout review changes.

```bash
cargo fmt --check
```

Result: passed.

Proves: Rust formatting is clean before closeout.

```bash
cargo nextest run -p codex-helper-core
```

Result: passed, 528 tests run.

Proves: the full core package gate is green after static profile, relay probes, admin diagnostics,
recommendations, docs, and closeout consistency fixes.

Review gate: closeout review found one important consistency issue before completion: diagnostics
defaulted omitted `patch_mode` from helper config instead of the current Codex switch status used by
the request path. The endpoint now prefers current Codex switch status, then helper config, then
`default`; a regression test covers that behavior.

## Notes

- Hosted `image_generation` active probes must be explicit because they may cost money or create
  artifacts.
- Remote compaction v2 should be diagnostic-only until upstream Codex and relay semantics stabilize.
- WebSocket capability should remain reported as unsupported by helper until an actual relay is
  implemented and tested.
- Fresh verification is required before marking any task or this lane complete.
