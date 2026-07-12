# Upstream Usage and Balance APIs for Shared-Key Quota Forecasting

Date: 2026-07-12

## Scope

This note evaluates whether codex-helper can estimate real quota burn by polling
the relay instead of relying only on requests observed by the local proxy. It is
based exclusively on the checked-in upstream source snapshots:

- `repo-ref/new-api` at `6ce7305cd36f`
- `repo-ref/sub2api` at `6f43986c376d`

No local provider configuration, endpoint, credential, account identifier, or
runtime secret was used in this research.

## Executive Conclusion

Yes: a remote cumulative `used` or `remaining` value can include consumption
from every computer using the same relay key. Both implementations resolve the
key to a server-side record and update centralized counters, so a balance delta
is not limited to requests seen by codex-helper on the current machine.

However, a value is only meaningful after its scope is identified:

| Scope | Includes | Safe interpretation |
| --- | --- | --- |
| API key | All clients using that exact key | Shared-key burn |
| User wallet | All keys and clients billed to the user | Shared-wallet burn |
| User subscription | All keys and clients billed to that subscription pool | Shared-package burn |
| Upstream account | Traffic scheduled through one relay-owned upstream account | Operator capacity, not an end-user package |

Remote polling should therefore be the source of truth for total pool burn.
Local request records should remain the source for project attribution. The
difference is external or unattributed usage; it must not be assigned to a
local project as if it were observed locally.

## New API

### API-key quota endpoint

`GET /api/usage/token/` is the strongest endpoint available when codex-helper
only has the model API key. The route uses read-only token authentication
(`repo-ref/new-api/router/api-router.go:246-253`,
`repo-ref/new-api/middleware/auth.go:237-310`). Its response is keyed to the
authenticated token and contains:

- `total_granted`
- `total_used`
- `total_available`
- `unlimited_quota`
- `expires_at`

The response mapping is in `repo-ref/new-api/controller/token.go:118-164`.
These values are token-level, not machine-level and not necessarily the user's
subscription pool.

Every billed request updates the centralized token row by decreasing
`remain_quota` and increasing `used_quota`
(`repo-ref/new-api/model/token.go:412-439`). Consequently, polling the same key
observes traffic sent with that key from other computers as well.

There is an important freshness asymmetry when Redis token caching is enabled.
The cache adjustment changes only `remain_quota`; it does not increment the
cached `used_quota` (`repo-ref/new-api/model/token_cache.go:30-40`). A short
sampling loop should therefore prefer the decrease in `total_available` over
the increase in `total_used`, while retaining both for consistency checks.

This endpoint has no daily window and no reset timestamp. To derive today's
use from it, codex-helper must persist a sample at the period boundary and
subtract later samples. Starting after the boundary cannot recover the earlier
part of the day from this endpoint alone.

### Legacy OpenAI-compatible billing endpoints

The following aliases also accept the model API key:

- `GET /dashboard/billing/subscription`
- `GET /v1/dashboard/billing/subscription`
- `GET /dashboard/billing/usage`
- `GET /v1/dashboard/billing/usage`

Their routes and authentication are defined in
`repo-ref/new-api/router/dashboard.go:10-21`. Their scope is configuration
dependent: with `DisplayTokenStatEnabled` enabled, they use token counters;
otherwise they use user counters (`repo-ref/new-api/controller/billing.go:11-27`,
`repo-ref/new-api/common/constants.go:65`).

The `usage` handler ignores `start_date` and `end_date` and returns cumulative
`used_quota`, not usage for the requested day
(`repo-ref/new-api/controller/billing.go:71-106`). `access_until` is token
expiry, not quota reset time (`repo-ref/new-api/controller/billing.go:59-67`).
These endpoints are useful as compatibility fallbacks, but their scope must be
probed and recorded rather than assumed.

### User-authenticated account and subscription endpoints

The console user APIs require a user access token plus a matching
`New-Api-User` header; a model API key is not sufficient
(`repo-ref/new-api/middleware/auth.go:37-123`). When that authentication is
explicitly configured, the following endpoints provide broader and more useful
pool data.

`GET /api/subscription/self` returns all active and historical subscriptions
for the authenticated user (`repo-ref/new-api/router/api-router.go:150-156`,
`repo-ref/new-api/controller/subscription.go:53-74`). Each subscription instance
contains `amount_total`, `amount_used`, `start_time`, `end_time`,
`last_reset_time`, and `next_reset_time`
(`repo-ref/new-api/model/subscription.go:252-280`). This is the preferred source
for a native subscription pool because it exposes both usage and a stable pool
identity.

The native reset model supports daily, weekly, monthly, custom, and never.
Daily resets align to the next local midnight, weekly resets to the next Monday
midnight, and monthly resets to the first day of the next month
(`repo-ref/new-api/model/subscription.go:344-382`). A master-node task checks for
due resets every minute (`repo-ref/new-api/service/subscription_reset_task.go:17-43`).

For time-ranged consumption:

- `GET /api/log/self/stat` sums billed quota across the authenticated user's
  server-side consume logs for an explicit timestamp range
  (`repo-ref/new-api/controller/log.go:125-149`,
  `repo-ref/new-api/model/log.go:618-669`). It covers all keys belonging to that
  user, subject to log retention and consume logging being enabled.
- `GET /api/data/self` returns hourly account aggregates for up to one month
  (`repo-ref/new-api/controller/usedata.go:63-84`,
  `repo-ref/new-api/model/usedata.go:152-160`). This data exists only when data
  export is enabled and is flushed on a configurable interval whose default is
  five minutes (`repo-ref/new-api/model/usedata.go:41-47`,
  `repo-ref/new-api/common/constants.go:69`).
- `GET /api/data/flow/self` additionally groups the authenticated user's data by
  token, group, and model (`repo-ref/new-api/model/usedata_flow.go:25-54`).

These account-wide log APIs include other computers and other keys under the
same user. They are not suitable for isolating one key unless the flow endpoint
or token-specific records are used.

## Sub2API

### API-key usage endpoint

`GET /v1/usage` is explicitly implemented as the API-key self-service usage
endpoint. The `/v1` group applies API-key authentication before routing to the
handler (`repo-ref/sub2api/backend/internal/server/routes/gateway.go:88-125`).
Authentication accepts a bearer header or supported API-key headers and rejects
query-string credentials
(`repo-ref/sub2api/backend/internal/server/middleware/api_key_auth.go:31-65`).

The response contains API-key-scoped server statistics:

- today's and cumulative requests, tokens, standard cost, and actual billed
  cost;
- daily history for the current key;
- model statistics for a selectable date range.

The handler explicitly passes the authenticated API-key ID into these queries
(`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1250-1268`,
`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1300-1345`). The
repository computes both cumulative and today's totals from centralized usage
logs filtered by `api_key_id`
(`repo-ref/sub2api/backend/internal/repository/usage_log_repo_dashboard.go:548-615`).
This is key-level data and includes all computers using the key.

The quota portion is mode dependent:

1. `quota_limited`: if the key has an independent quota or rate limit, the
   response returns key-level `limit`, `used`, and `remaining`. It also returns
   5-hour, 1-day, and 7-day rate-limit windows with their window start and reset
   time (`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1270-1278`,
   `repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1348-1418`).
2. `unrestricted` subscription: it returns the active user-plus-group
   subscription's daily, weekly, and monthly usage and limits, plus expiry
   (`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1441-1477`).
   This response does not include that subscription's reset timestamps.
3. `unrestricted` wallet: it reloads the user and returns the latest wallet
   `balance` and `remaining`
   (`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1481-1505`).

The middleware deliberately loads the active user-plus-group subscription even
though billing enforcement is skipped for `/v1/usage`
(`repo-ref/sub2api/backend/internal/server/middleware/api_key_auth.go:145-168`).
Therefore subscription usage represents the shared package, not only the
current key. Wallet balance similarly represents all keys billed to the user.

### Server-side accounting and shared-device coverage

The server atomically updates key, subscription, or wallet state after billing:

- key quota is incremented by API-key ID
  (`repo-ref/sub2api/backend/internal/repository/api_key_repo.go:587-625`);
- subscription daily, weekly, and monthly usage is incremented by subscription
  ID (`repo-ref/sub2api/backend/internal/repository/user_subscription_repo.go:396-430`);
- the billing path selects subscription usage or user balance, and then updates
  the independent key quota when configured
  (`repo-ref/sub2api/backend/internal/service/gateway_usage_billing.go:123-160`).

This proves that a remote delta includes other computers using the same key. A
subscription or wallet delta can additionally include other keys sharing that
pool.

Production billing is committed before the request's usage log is written on a
best-effort basis (`repo-ref/sub2api/backend/internal/service/gateway_usage_billing.go:733-750`).
For pool burn, the authoritative order should therefore be remote balance or
quota counters first, server usage logs second, and codex-helper's local cost
estimate last.

### User-authenticated endpoints

The `/api/v1` user routes use JWT authentication
(`repo-ref/sub2api/backend/internal/server/router.go:106-113`,
`repo-ref/sub2api/backend/internal/server/routes/user.go:18-20`). With an
explicitly configured console credential, useful endpoints include:

- `GET /api/v1/user/profile`: user wallet balance
  (`repo-ref/sub2api/backend/internal/server/routes/user.go:23-35`,
  `repo-ref/sub2api/backend/internal/handler/dto/types.go:11-31`).
- `GET /api/v1/keys`: each key's quota, quota used, 5-hour/1-day/7-day usage,
  window starts, and reset times
  (`repo-ref/sub2api/backend/internal/server/routes/user.go:58-66`,
  `repo-ref/sub2api/backend/internal/handler/dto/types.go:52-82`,
  `repo-ref/sub2api/backend/internal/handler/dto/mappers.go:90-120`).
- `GET /api/v1/subscriptions/progress`: each active subscription's limit, used,
  remaining, percentage, window start, reset timestamp, and seconds to reset
  (`repo-ref/sub2api/backend/internal/server/routes/user.go:111-118`,
  `repo-ref/sub2api/backend/internal/service/subscription_service.go:1019-1139`).
- `GET /api/v1/user/platform-quotas`: user-plus-platform daily, weekly, and
  monthly usage, limits, and reset timestamps
  (`repo-ref/sub2api/backend/internal/handler/user_handler.go:46-69`,
  `repo-ref/sub2api/backend/internal/handler/quotaview/helpers.go:12-35`).
- `GET /api/v1/usage/dashboard/stats` and
  `GET /api/v1/usage/dashboard/trend`: user-wide current totals and time buckets
  across all of the user's keys
  (`repo-ref/sub2api/backend/internal/server/routes/user.go:81-95`,
  `repo-ref/sub2api/backend/internal/handler/usage_handler.go:438-476`).

These JWT endpoints are better than `/v1/usage` for stable pool identity and
explicit subscription reset timestamps. They must remain opt-in because a
model API key alone cannot call them.

### Admin-only upstream-account endpoints

The admin API exposes `GET /api/v1/admin/accounts/:id/usage` and
`GET /api/v1/admin/accounts/:id/today-stats`
(`repo-ref/sub2api/backend/internal/server/routes/admin.go:12-36`,
`repo-ref/sub2api/backend/internal/server/routes/admin.go:294-319`). The first
can return upstream-account utilization windows and reset timestamps; the
second aggregates today's local relay traffic scheduled through that upstream
account (`repo-ref/sub2api/backend/internal/handler/admin/account_handler.go:2027-2050`,
`repo-ref/sub2api/backend/internal/handler/admin/account_handler.go:2143-2158`).

The usage model includes utilization, reset timestamp, seconds remaining, and
local window stats (`repo-ref/sub2api/backend/internal/service/account_usage_service.go:136-150`,
`repo-ref/sub2api/backend/internal/service/account_usage_service.go:180-217`).
Support depends on upstream account type; some account types cannot query a
remote usage API (`repo-ref/sub2api/backend/internal/service/account_usage_service.go:323-327`,
`repo-ref/sub2api/backend/internal/service/account_usage_service.go:368-462`).
These endpoints are operator capacity signals and should not be conflated with
an end user's relay subscription.

## Reset Semantics

"Daily" does not universally mean local midnight:

- New API native subscription daily windows align to the server-local next
  midnight (`repo-ref/new-api/model/subscription.go:344-356`).
- Sub2API's today usage uses the configured server timezone; its default is
  `Asia/Shanghai` (`repo-ref/sub2api/backend/internal/config/config.go:1879-1880`,
  `repo-ref/sub2api/backend/internal/pkg/timezone/timezone.go:89-99`).
- Sub2API subscription windows expose their actual start and compute daily as
  start plus 24 hours, weekly as start plus 7 days, and monthly as start plus
  30 days (`repo-ref/sub2api/backend/internal/service/user_subscription.go:65-114`).
- Sub2API API-key `1d` rate limits are rolling windows and expose `reset_at` as
  window start plus window duration
  (`repo-ref/sub2api/backend/internal/handler/gateway_handler.go:1388-1413`).

Forecasting must use an explicit `reset_at` when available. A configured
midnight should be an adapter-level fallback with reduced confidence, not a
universal rule.

## Accuracy Boundaries

Remote polling is more complete than local request counting, but it is not an
instantaneous financial ledger.

1. Cached values can lag. New API updates cached remaining quota but not cached
   used quota. Sub2API's API-key authentication snapshot contains quota used
   and defaults to a 15-second L1 and 300-second L2 TTL; subscription auth has a
   10-second default L1 TTL
   (`repo-ref/sub2api/backend/internal/service/api_key_auth_cache.go:5-29`,
   `repo-ref/sub2api/backend/internal/config/config.go:1882-1893`).
2. Top-ups, refunds, administrative adjustments, quota resets, and plan changes
   can move a balance independently of request consumption. Such intervals must
   be marked as adjustments or new periods, not negative burn.
3. In-flight requests and delayed settlement can appear in a later sample.
   Short-window rates should use multiple samples rather than one pair.
4. A wallet or subscription value may intentionally include several keys. It
   must not be added once per configured endpoint.
5. A remote pool delta cannot identify a project path. Project allocation still
   depends on local request metadata and must expose an external/unattributed
   remainder.

## Recommended Sampling Model

Persist an immutable sample every 60 to 120 seconds:

```text
quota_sample
  pool_identity_hash
  scope                 # key | wallet | subscription | upstream_account
  observed_at
  used
  remaining
  limit
  reset_at
  source_endpoint_kind
  freshness
  confidence
```

Never persist the raw key. Derive a stable local fingerprint and keep the
credential in the existing secret-resolution path.

For two valid samples in the same period:

```text
burn_per_hour = max(0, used_2 - used_1) / elapsed_hours

# If used is unavailable:
burn_per_hour = max(0, remaining_1 - remaining_2) / elapsed_hours

allowed_per_hour = remaining_2 / hours_until_reset
pace_ratio = burn_per_hour / allowed_per_hour
```

Calculate at least 15-minute and 60-minute slopes. Use the 60-minute slope for
the primary forecast and the 15-minute slope as acceleration/deceleration
context. Split the series when reset, limit, plan, or total balance changes.

For project attribution:

```text
external_or_unattributed =
  max(0, remote_pool_delta - sum(local_project_estimated_deltas))
```

The remainder is valuable product information: it directly shows consumption
from another computer, another key sharing the pool, unsupported local request
paths, or pricing-estimate drift. It should be shown explicitly instead of
being silently distributed across local projects.

## Adapter Priority

1. Prefer a native subscription progress endpoint with a stable pool identity,
   `used`, `remaining`, `limit`, and `reset_at`.
2. Otherwise prefer a key-level endpoint with cumulative `used` and
   `remaining`.
3. Otherwise use wallet balance deltas, with adjustment detection.
4. Use server-side time-ranged usage to validate and explain the pool delta.
5. Use codex-helper's request ledger for project attribution and as a fallback,
   never as a substitute for shared remote consumption when the latter exists.

Each provider adapter should report its discovered scope and capabilities. A
successful HTTP response alone is not enough to declare the forecast accurate.
