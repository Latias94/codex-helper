# Milestones: Routing Config Surface

## P0 - Public Shape and Compiler

- [x] Add the new public routing config schema.
- [x] Support inline single-endpoint providers.
- [x] Support explicit routing policy and order.
- [x] Compile the new shape into the existing runtime routing model.
- [x] Make `config init` emit a routing-first v3 template.
- [x] Keep legacy config loading working during the transition.

Acceptance:

- one provider with one endpoint is a short, readable block;
- ordered failover is deterministic;
- tag-preferred routing is deterministic;
- no user-visible active-provider clone remains in the new authoring model.

## P1 - Migration

- [x] Add a first migration path from legacy `active / level / upstream` config.
- [x] Preserve explicit tags.
- [x] Preserve endpoint order and preferred ordering for common provider-level routes.
- [x] Emit warnings when the migration cannot infer a clean order.

Acceptance:

- `config migrate` can rewrite old configs into the new shape;
- the migrated config is shorter or clearer for common single-endpoint cases;
- the migration output does not invent business tags.

## P1 - CLI Authoring Surface

- [x] Add a first-class `routing` command group for v3 files.
- [x] Support `routing show`, `routing pin`, `routing order`, `routing prefer-tag`, `routing clear-target`, and low-level `routing set`.
- [x] Keep `routing` writes v3-native and reject legacy/v2 documents instead of silently projecting through station schema.
- [x] Normalize CLI-written order so listed providers are promoted and remaining providers are retained.
- [x] Add a first-class provider command group for v3 catalog edits.
- [x] Remove compatibility `station add/set-active/set-level/enable/disable` writes from the public CLI now that provider/routing commands exist.
- [x] Move schema/file operations to `config init` / `config migrate`, and move read-only route views to `routing list` / `routing explain`.
- [x] Remove the top-level `station` CLI surface from the public command tree.

Acceptance:

- common routing policies are editable without touching `[codex.stations.*]`;
- pinning, ordering, and monthly-tag preference are one-command operations;
- provider edits do not require users to think in station/group terms;
- route edits preserve the user's provider catalog and explicit fallback order.

## P1 - UI / UX

- [x] Show balance/package summaries in TUI session details and provider switch menus.
- [x] Show explicit provider tags in the TUI switch menu so tag-preferred routing is inspectable before editing.
- [x] Add a TUI v3 routing quick editor for pinning, ordered failover, provider enable/disable, monthly tag preference, exhausted behavior, fallback order, and `billing` tags.
- [x] Retarget legacy TUI station/provider shortcut keys under v3 so provider switching uses routing instead of the internal `routing` station.
- Update the provider editor to prioritize identity, auth, endpoint, and tags.
- Add a routing editor with policy, order, target, and tag-preference controls.
- Show a preview of preferred candidates and fallback behavior.
- [x] Ensure GUI raw config parsing/saving accepts v3 documents.

Acceptance:

- a user can add a new provider without re-learning the entire config structure;
- a user can see which candidate has balance/package quota before pinning or switching to it;
- the routing preview explains why a provider is first, skipped, or used as fallback.

## P1 - Control Plane Write-Back

- [x] Load the persisted document as v3 when the file is v3, instead of editing a compacted v2 projection.
- [x] Preserve `providers`, `routing`, tags, profiles, and top-level metadata during local/remote control-plane edits.
- [x] Add a first-class v3 routing API for `policy`, `order`, `target`, `prefer_tags`, and `on_exhausted`.
- [x] Remove v3 writes from compatibility station quick-switch and station settings APIs.
- [x] Keep provider spec CRUD v3-native and append newly created providers to an existing explicit `routing.order`.
- [x] Keep profile CRUD/default-profile write-back v3-native.
- [x] Reject station spec reads/writes on v3 files instead of silently reintroducing station/group schema.
- [x] Remove compatibility-only control-plane aliases such as `/stations/config-active` and `station_persisted_config`.

Acceptance:

- GUI/TUI/remote admin writes never turn a `version = 3` file back into `[codex.stations.*]`;
- hand-written v3 routing intent survives metadata, provider, profile, and active-target edits;
- compatibility station APIs are v2-only and are not the canonical v3 authoring model.

## P2 - Optional Preset Expansion

- Only after the new surface is stable, consider named routing presets.
- Do not add more presets before the basic policy surface is intuitive.

Acceptance:

- presets add value instead of just adding more labels.

## Done When

- The new config is easier to edit by hand than the legacy shape.
- The runtime still sees a complete routing model.
- The GUI/TUI preview matches the compiler output.
- The migration path is documented and tested.
