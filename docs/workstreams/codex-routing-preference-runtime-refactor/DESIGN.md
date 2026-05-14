# Design: Codex Routing Preference Runtime

## Background

The current route graph gives users a strong authoring model, but automatic
runtime affinity can still bypass that intent after fallback. The next runtime
must separate these concepts:

- static route graph intent;
- derived preference groups;
- transient health and cooldown state;
- automatic affinity;
- explicit operator overrides.

## Current Failure Mode

For v4 route graph configs, route execution already carries route metadata, but
station compatibility state still leaks into selection:

- v4 candidates are represented through a compatibility station named
  `routing`;
- route candidate selection can reuse `last_good_index` or session affinity;
- once a fallback provider succeeds, later requests can start directly from that
  provider;
- the fallback success does not necessarily mean the preferred group is still
  exhausted.

The bug is not that fallback exists. The bug is that fallback success can become
stronger than the user's configured preference.

## Target Model

### Station Retirement

Station is not part of the new runtime model.

Allowed station uses:

- parsing older configs;
- producing migration diagnostics;
- reading historical request logs that already contain station fields.

Forbidden station uses:

- route plan IR;
- request execution;
- automatic affinity;
- explicit pin targets;
- runtime health, cooldown, or usage state keys;
- new public API contracts.

Old station-shaped inputs must be converted before the runtime receives them.
There should be no synthetic `routing` station in the v5 executor.

### Route Candidate Identity

Every executable candidate has a stable identity:

```text
service + route_graph_key + route_path + provider_id + endpoint_id
```

`base_url` remains part of state migration and diagnostics, but it is not the
primary identity.

### Preference Groups

The route compiler assigns each candidate a `preference_group` integer.

Lower numbers are preferred:

```text
0 preferred monthly group
1 first fallback group
2 later fallback group
```

The group is derived from the route graph:

- `tag-preferred` creates a preferred group for matching children and fallback
  groups for non-matching children.
- `ordered-failover` preserves child order and group boundaries from nested
  children.
- `manual-sticky` produces one group unless it targets a route node that already
  has group structure.
- `conditional` evaluates the selected branch first, then uses that branch's
  group structure.

The compiler must preserve route-node boundaries that affect fallback intent.

### Runtime State

Runtime state is keyed by provider endpoint identity:

- failure count;
- cooldown;
- passive health;
- trusted usage exhaustion;
- last success timestamp;
- last failure timestamp;
- last selected timestamp.

There is no station/upstream state in the new runtime. Older station state is
accepted only long enough to migrate it into provider endpoint state, when that
can be done without guessing. Otherwise the migration warns and drops the
ambiguous runtime state.

### Affinity Policy

Default policy:

```toml
[codex.routing]
affinity_policy = "preferred-group"
```

Modes:

- `off`
  - no automatic provider affinity;
  - every request starts from the best available preference group.
- `preferred-group`
  - affinity can reorder candidates only inside the currently best available
    preference group;
  - fallback affinity is temporary and loses to recovered preferred candidates.
- `fallback-sticky`
  - preserves the old behavior for operators who want maximum cache locality;
  - fallback success can be reused while healthy until TTL or failure.
- `hard`
  - explicit operator pin behavior, not automatic affinity.

Default mode should be `preferred-group`.

### Selection Algorithm

For each request:

1. Compile the request-aware route plan.
2. Build runtime state for all candidates.
3. Apply explicit manual overrides first.
4. Partition candidates by `preference_group`.
5. Starting from the lowest group:
   - remove candidates that are disabled, unsupported, missing auth, hard
     unavailable, cooled down, or trusted exhausted;
   - apply affinity only if it points to a candidate in this group;
   - select the first viable candidate in group order.
6. If the group has no viable candidate, continue to the next group.
7. If fallback is selected, record why all higher-priority groups were skipped.
8. On success, update affinity according to the configured policy.
9. On failure, update runtime state and retry within the same group before
   moving to lower-priority groups.

### Preferred Reprobe

When a session is using fallback affinity:

- requests may keep using fallback only until `fallback_ttl_ms`;
- after `reprobe_preferred_after_ms`, the selector must test whether a higher
  preference group has a viable candidate;
- if a higher group is viable, selection returns to that group.

This prevents one transient monthly outage from pinning a long-lived session to
paygo.

## Config Schema

The preferred target is a new persisted schema version because this workstream
changes both runtime semantics and public station compatibility:

```toml
version = 5

[codex.routing]
entry = "monthly_first"
affinity_policy = "preferred-group"
```

Migration from v4 should:

- preserve all providers, auth references, tags, endpoints, and route nodes;
- add `affinity_policy` only when it differs from the v5 default;
- document that v4's implicit session affinity now behaves as
  `preferred-group`;
- offer an opt-in compatibility recipe for old fallback-sticky behavior.

Migration from station-shaped configs should:

- convert each station upstream into a provider endpoint or provider leaf;
- convert active/manual station routing into route graph nodes;
- convert station profile bindings into provider or route defaults when the
  target is unambiguous;
- warn and skip ambiguous runtime-only state instead of inventing route policy.

After migration, station writes should fail with a clear error that points
operators to provider, route, endpoint, and affinity APIs.

## Observability

Every routed request should be explainable with:

- selected provider and endpoint;
- selected route path;
- selected preference group;
- affinity source, if used;
- skipped higher-priority groups;
- skip reasons per candidate;
- fallback TTL and preferred reprobe status;
- explicit override source, if any.

Request logs must keep existing fields, but new fields should be structured and
first-class instead of encoded only in strings.

Historical station fields remain readable as log data. New route logs should
not use station identity as the canonical decision key.

## Acceptance Examples

### Monthly Recovers After Fallback

Given `monthly_pool -> chili`:

1. `input2` and `input3` return `502`.
2. The request succeeds on `chili`.
3. Later `input2` becomes healthy.
4. The next request selects `input2` or another viable monthly candidate.

### Fallback Stays During Short Outage

Given fallback TTL is five minutes:

1. Monthly providers remain cooled down.
2. The session has fallback affinity to `chili`.
3. New requests may reuse `chili` until a preferred candidate becomes viable or
   the TTL expires.

### Explicit Pin Still Wins

If the operator explicitly pins `chili`, the selector uses `chili` until the pin
is cleared or the provider is not viable.

Automatic affinity must never be confused with that explicit pin.
