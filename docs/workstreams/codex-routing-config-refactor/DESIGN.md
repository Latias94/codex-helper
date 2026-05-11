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

## Reference Model Takeaways

This design follows the same broad separation used by mature routing systems:

- [LiteLLM routing](https://docs.litellm.ai/docs/routing): keep deployment groups and fallback order explicit instead of hiding provider selection inside one API key.
- [Portkey conditional routing](https://portkey.ai/docs/product/ai-gateway/conditional-routing): make route conditions and fallback chains visible to operators.
- [OpenRouter provider routing](https://openrouter.ai/docs/features/provider-routing): expose provider order and fallback behavior as user-authored policy.
- [Envoy outlier detection](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier): treat passive health, temporary ejection, cooldown, and later recovery as runtime state rather than permanent route rewrites.

The local implication is that `routing.order`, `prefer_tags`, balance exhaustion, cooldown, and reprobe should remain separate concepts even when the common user story is simply “use monthly first, then paygo”.

## Pool, Health, And Reprobe

The current `routing` block can already express a simple ordered fallback chain or a tag-first preference. That is enough for many users, but it is not yet the full mental model for monthly quota providers that may:

- report `unknown` before balance metadata is refreshed;
- return temporary transport failures such as `502` or `429`;
- become usable again later without a config rewrite;
- need a paygo fallback only after the monthly pool is truly unavailable.

For that class of setups, the next semantic layer should treat the preferred monthly providers as a pool or workstream, not as a permanently demoted entry in a flat list. The important distinctions are:

- `unknown` is not exhausted;
- confirmed exhaustion can demote routing;
- temporary failure can trigger cooldown or ejection;
- cooldown must not be permanent;
- reprobe should eventually let the provider back into the preferred pool.

This is a runtime behavior model first and a syntax question second. If we later introduce a first-class `workstreams` or `pools` authoring shape, it should compile into the same runtime routing model, but keep these state transitions explicit instead of hiding them inside `order`.

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
- TUI decision surfaces should show provider balance/package state and explicit tags before asking the user to switch.
- The preview should show:
  - preferred candidates;
  - fallback order;
  - skipped candidates and reasons;
  - stop-vs-continue behavior when exhausted.

### TUI Routing UX Direction

The TUI should be split into three progressively stronger surfaces:

1. Read-only confidence layer: session details, station/provider switch menus, and routing previews show balance, package name, exhaustion state, and useful tags such as `billing=monthly` or `region=hk`.
2. Fast steering layer: session pin, global pin, and ordered routing promotion stay one-keystroke operations, but labels should say provider/routing intent instead of leaking the legacy station model where possible.
3. Persisted editing layer: provider tags and routing policy are edited through the v3 provider/routing control-plane APIs, not by mutating a v2 station projection or by embedding a TOML editor into the TUI.

For tag editing, the first TUI version should support common add/remove operations on provider-level tags. Endpoint-level tags should remain a detail/edit screen because most users only need provider-level `billing`, `vendor`, and `region`.

## Non-Goals

- Do not reintroduce a hard `active provider` concept in the public authoring model.
- Do not require users to model monthly/paygo as separate config layers.
- Do not infer policy from balance numbers alone.
- Do not turn tags into a hidden scoring engine before the product has real metrics.
