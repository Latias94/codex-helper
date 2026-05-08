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

## P1 - UI / UX

- Update the provider editor to prioritize identity, auth, endpoint, and tags.
- Add a routing editor with policy, order, target, and tag-preference controls.
- Show a preview of preferred candidates and fallback behavior.
- [x] Ensure GUI raw config parsing/saving accepts v3 documents.

Acceptance:

- a user can add a new provider without re-learning the entire config structure;
- the routing preview explains why a provider is first, skipped, or used as fallback.

## P1 - Control Plane Write-Back

- [x] Load the persisted document as v3 when the file is v3, instead of editing a compacted v2 projection.
- [x] Preserve `providers`, `routing`, tags, profiles, and top-level metadata during local/remote control-plane edits.
- [x] Map compatibility station quick-switch to `routing.policy = "manual-sticky"` plus `routing.target`.
- [x] Map compatibility station enable/disable to provider enable/disable on v3 files.
- [x] Keep provider spec CRUD v3-native and append newly created providers to an existing explicit `routing.order`.
- [x] Keep profile CRUD/default-profile write-back v3-native.
- [x] Reject v2 station-structure editing on v3 files instead of silently reintroducing station/group schema.
- [ ] Add a first-class routing editor API once the GUI/TUI need structured routing writes beyond quick-switch and provider CRUD.

Acceptance:

- GUI/TUI/remote admin writes never turn a `version = 3` file back into `[codex.stations.*]`;
- hand-written v3 routing intent survives metadata, provider, profile, and active-target edits;
- compatibility station APIs are treated as bridges, not as the canonical v3 authoring model.

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
