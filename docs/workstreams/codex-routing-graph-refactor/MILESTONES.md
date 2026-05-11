# Milestones: Codex Routing Graph

## P0 - Shape Freeze

- [x] Define the v4 route graph schema.
- [x] Define route node strategies and validation rules for `ordered-failover`, `manual-sticky`, and `tag-preferred`.
- [x] Define how route nodes reference providers and other route nodes.
- [x] Define how the compiler expands the graph into an ordered candidate plan.
- [x] Define how runtime health state stays separate from config.

Acceptance:

- one provider, one ordered chain, monthly group + paygo fallback, tag preference, and manual pin are all expressible without a special pool syntax;
- the graph can be explained in plain language;
- cycle detection and missing-reference errors are deterministic.

## P1 - Migration

- [x] Migrate v3 routing into v4 route graphs.
- [x] Preserve explicit tags and provider metadata.
- [x] Preserve clear ordered chains where they exist.
- [x] Migrate legacy v3 `pool-fallback` into nested route nodes.
- [x] Emit warnings when older station/group migration cannot safely preserve semantics.

Acceptance:

- v3 files can be rewritten into v4 with no silent policy invention;
- monthly pool intent survives migration as a named route node;
- ambiguous flattening is reported, not guessed.

## P1 - CLI Authoring Surface

- [ ] Add dedicated route-node creation and editing commands.
- [ ] Add entry-point selection commands.
- [x] Add route-node inspection and explanation output through `routing show` / `routing explain`.
- [x] Support pinning and tag preference without raw TOML edits for common cases.
- [ ] Support conditional routing after the core graph is stable.

Acceptance:

- operators can create the common recipes from the CLI;
- the CLI shows the same route graph that the compiler sees.

## P1 - UI / UX

- [ ] Show the full route graph in TUI and GUI.
- [ ] Distinguish provider leaves from route nodes visually.
- [ ] Explain why a node was chosen, skipped, or cooled down.
- [x] Keep runtime health and config intent visually separate enough that balances do not rewrite static config.

Acceptance:

- users can see the route tree before they edit it;
- balance/package state is still visible, but not confused with authoring intent.

## P1 - Control Plane Write-Back

- [x] Add structured write paths for route nodes and the graph entry point.
- [x] Keep provider CRUD v4-native.
- [x] Keep graph edits stable under UI/CLI/provider CRUD writes without flattening.

Acceptance:

- GUI/TUI/admin writes do not collapse the graph back into a flat compatibility shape;
- the persisted file remains v4-native after edits.

## P2 - Optional Presets

- [ ] Consider named recipes only after the route graph is stable.

Acceptance:

- presets add value instead of just adding aliases.

## Done When

- The graph is easier to reason about than the old flat surface.
- The compiler output matches the UI preview.
- The migration path is documented and tested.
