# Design: Routing Config Surface

## Problem Statement

The current public config shape is still too close to runtime internals.
The public authoring model should stop centering on `active provider` / `active_station` and instead expose `active routing` as the primary user-facing control.

For common setups, users want to express:

- this provider is the primary one;
- this provider is the backup;
- this provider should be preferred when the tag says `billing=monthly`;
- if everything is exhausted, stop or continue based on a clear rule.

They do not want to model station grouping, internal `level` ordering, or nested upstream structure just to describe a single relay endpoint.

## Design Goals

- Keep the authoring model thin.
- Keep the compile target compatible with the existing runtime routing engine.
- Make “active routing” the user-facing concept, not “active provider”.
- Make tags useful without turning them into hidden policy.
- Preserve deterministic fallback order.
- Make migration from legacy config automatic.

## Public Shape

Recommended public shape:

```toml
version = 3

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"
tags = { billing = "paygo", region = "us" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["input", "backup"]
on_exhausted = "continue"
```

Rules:

- `providers` is the catalog.
- `routing` is the active route recipe.
- A single-endpoint provider should use the inline `base_url` shorthand.
- Multi-endpoint providers may expand to an `endpoints` table only when needed.
- `tags` are optional metadata.
- `policy` decides how to use the routing inputs.

## Policy Examples

### 1. Manual Sticky

```toml
[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"

[codex.routing]
policy = "manual-sticky"
target = "input"
```

Evaluation:

- easiest to understand;
- safest for long sessions;
- no automatic recovery if the target dies.

### 2. Ordered Failover

```toml
[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"

[codex.routing]
policy = "ordered-failover"
order = ["input", "backup"]
on_exhausted = "stop"
```

Evaluation:

- closest to user intuition;
- easiest UI;
- best default for most users;
- very low migration risk.

### 3. Tag Preferred

```toml
[codex.providers.monthly]
base_url = "https://monthly.example/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "continue"
```

Evaluation:

- matches the “monthly first, fallback later” mental model;
- explicit enough for GUI/TUI;
- depends on correct user tagging;
- much better than trying to infer “monthly” from balance APIs.

### 4. Tag Preferred, Hard Stop

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "stop"
```

Evaluation:

- useful for users who care more about budget boundaries than availability;
- very clear failure mode;
- should be an advanced option, not the default.

## Self-Evaluation

| Policy | Readability | UI Ease | Migration Cost | Safety | Recommendation |
| --- | --- | --- | --- | --- | --- |
| `manual-sticky` | high | high | low | high for continuity, low for availability | keep |
| `ordered-failover` | high | high | low | high | default |
| `tag-preferred` | medium-high | medium | medium | high if tags are correct | add |
| `tag-preferred + stop` | medium | medium | medium | strict but explicit | advanced |
| `balanced` | low right now | low | high | not trustworthy without measured latency/cost signals | defer |

Conclusion:

- `ordered-failover` should be the default public policy.
- `tag-preferred` is the right way to express “monthly first”.
- `balanced` should stay out of the first public config surface until the runtime has real speed and cost signals.

## Migration Strategy

The migration should be deterministic and boring.

### From Legacy Shapes

- legacy `active` / `active_station` maps to the routing target for `manual-sticky`, or the first entry in `order` for `ordered-failover`;
- legacy `level` maps to the initial order;
- legacy grouped upstreams map to provider entries plus endpoint entries;
- legacy `preferred` becomes the first entry in the route order or the first item inside a provider group;
- existing explicit tags are preserved;
- inferred business tags such as `billing=monthly` are not guessed.

### Versioning

- The public authoring model should use a new version number if the schema is materially different from the current one.
- The runtime can still compile old and new inputs to the same internal routing model.
- Old files should load, migrate, and re-save into the new shape.

## UI / UX Implications

- The provider editor should mainly edit identity, auth, endpoint(s), and tags.
- The routing editor should mainly edit policy, order, target, and tag preferences.
- A single provider should not force the user to think about station membership.
- New providers should append to the end of the routing order unless the user explicitly promotes them.
- The preview should show:
  - preferred candidates;
  - fallback order;
  - skipped candidates and reasons;
  - stop-vs-continue behavior when exhausted.

## Non-Goals

- Do not reintroduce a hard `active provider` concept in the public authoring model.
- Do not require users to model monthly/paygo as separate config layers.
- Do not infer policy from balance numbers alone.
- Do not turn tags into a hidden scoring engine before the product has real metrics.
