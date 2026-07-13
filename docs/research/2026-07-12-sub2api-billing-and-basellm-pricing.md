# Sub2API Billing and codex-helper BaseLLM Pricing

Date: 2026-07-12

## Scope and source versions

This note verifies:

1. Sub2API's OpenAI token normalization, cost formula, multiplier ordering, and
   GPT-5.6 long-context policy.
2. codex-helper's remote pricing URL, upstream GitHub repository, CLI import
   path, local persistence, and runtime merge path.
3. The live BaseLLM price record for `gpt-5.6-sol` at the time of research.

Primary sources used:

- checked-in Sub2API snapshot `6f43986c376d76144cb39c7a562c179e19ac7439`;
- checked-in codex-helper snapshot `aedcc28a894e29575440b5226945fdf8b9d8752c`;
- BaseLLM's published `all.json` response fetched at
  `2026-07-12T06:57:24Z`, ETag `W/"6a530b8c-52e148"`;
- BaseLLM repository `basellm/llm-metadata`, whose `main` HEAD was
  `a26a048caeff6ba3c52e41fd2393fac49fdd8ac5` at the same check;
- Sub2API's configured `Wei-Shaw/model-price-repo` JSON fetched at
  `2026-07-12T06:59:21Z`, ETag
  `W/"5ad0d8dada9ada3f51bfe27d3c6fef919629217fad45ef58b10b5d1037010ec8"`.

No credential, endpoint secret, API-key name, IP address, or other private
provider configuration is included here.

## Executive findings

- Sub2API removes cache-read tokens from ordinary input before billing, then
  charges ordinary input, output, cache read, and cache creation separately.
- For the recognized GPT-5.4/5.5/5.6 family, a request whose ordinary input
  plus cache-read input is **strictly greater than 272,000** receives a 2x
  multiplier on input-side prices and a 1.5x multiplier on output price.
- `TotalCost` is the price before the account/group rate multiplier;
  `ActualCost = TotalCost * RateMultiplier`. The user CSV exports these as
  `Original Cost` and `Billed Cost`, respectively.
- The current BaseLLM `gpt-5.6-sol` base prices are `$5` input, `$30` output,
  `$0.50` cache read, and `$6.25` cache write per million tokens. Its
  272,000-token context tier is `$10`, `$45`, `$1`, and `$12.50`, respectively.
  These values agree with Sub2API's effective 2x/1.5x long-context policy.
- codex-helper imports BaseLLM prices only when the operator runs
  `pricing sync-basellm`; it writes `~/.codex-helper/pricing_overrides.toml` and
  merges those rows over the bundled catalog for runtime estimates.
- codex-helper's background BaseLLM metadata sync is a separate capability
  cache. It does not update prices.
- The current codex-helper BaseLLM importer reads only base cost fields. It
  ignores BaseLLM's `tiers` and `context_over_200k` objects, so a locally
  estimated long-context `gpt-5.6-sol` request will be underpriced even after a
  successful base-price sync.

## Sub2API billing path

### Price resolution

Sub2API first asks its dynamic pricing service for a model row and falls back
to the built-in table only when no usable dynamic token price exists
(`repo-ref/sub2api/backend/internal/service/billing_service.go:708-761`). The
dynamic service downloads, parses, persists, and swaps pricing data into memory
(`repo-ref/sub2api/backend/internal/service/pricing_service.go:288-354`). Its
default remote source is the `Wei-Shaw/model-price-repo` JSON, with a checked-in
resource as fallback (`repo-ref/sub2api/backend/internal/config/config.go:1871-1876`).

The checked-in resource's `gpt-5.6-sol` row declares the following per-token
prices (`repo-ref/sub2api/backend/resources/model-pricing/model_prices_and_context_window.json:4963-4979`):

| Component | Base per token | Base per 1M | Above-272k per token | Above-272k per 1M |
| --- | ---: | ---: | ---: | ---: |
| Input | `0.000005` | `$5` | `0.000010` | `$10` |
| Output | `0.000030` | `$30` | `0.000045` | `$45` |
| Cache read | `0.0000005` | `$0.50` | `0.000001` | `$1` |

The live dynamic source checked at `2026-07-12T06:59:21Z` contained the same
three component prices and additionally declared cache creation at
`0.00000625` per token (`$6.25` per 1M) and its above-272k price at
`0.0000125` (`$12.50` per 1M). The parser reads the base cache-creation field
(`repo-ref/sub2api/backend/internal/service/pricing_service.go:394-417`); the
long-context policy supplies its 2x uplift in the billing core.

The generic raw dynamic-pricing parser does not deserialize the source's
`*_above_272k_tokens` fields
(`repo-ref/sub2api/backend/internal/service/pricing_service.go:89-104`). Instead,
Sub2API applies a model-specific policy after resolving the base row. The
policy supplies threshold `272000`, input multiplier `2.0`, and output
multiplier `1.5` when those values are absent
(`repo-ref/sub2api/backend/internal/service/billing_service.go:1043-1063`).
`gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-5.6-luna` are explicitly recognized by
that policy (`repo-ref/sub2api/backend/internal/service/billing_service.go:1077-1084`).

This distinction matters: the base prices are data, while the long-context
behavior is currently also encoded in billing policy. For `gpt-5.6-sol`, the
two agree.

### Cache-token normalization

OpenAI usage reports include cache-read tokens inside `input_tokens`. Sub2API
therefore computes:

```text
ordinary_input = max(reported_input - cache_read, 0)
```

It then passes `ordinary_input`, output, cache creation, and cache read as
separate `UsageTokens` fields
(`repo-ref/sub2api/backend/internal/service/openai_gateway_usage.go:120-135`).
This prevents the same cached tokens from being charged once at the normal
input rate and again at the cache-read rate.

### Core formula

For a normal token-priced request, define:

```text
I = max(reported_input_tokens - cache_read_tokens, 0)
O = output_tokens
R = cache_read_tokens
W = cache_creation_tokens
M = rate_multiplier

long_context = (I + R) > 272000
```

The strict `>` comparison is in
`repo-ref/sub2api/backend/internal/service/billing_service.go:1066-1074`.
Because `I + R` reconstructs the reported input total for normal OpenAI usage,
cache hits still count toward the context threshold.

For `gpt-5.6-sol` with the current live dynamic price row:

```text
if not long_context:
    original = I * 5e-6 + O * 30e-6 + R * 0.5e-6 + W * 6.25e-6

if long_context:
    original = I * 10e-6 + O * 45e-6 + R * 1e-6 + W * 12.5e-6

billed = original * M
```

The billing core multiplies input and cache-read prices by the long-context
input multiplier, output by the output multiplier, and carries the input
multiplier into cache-creation cost
(`repo-ref/sub2api/backend/internal/service/billing_service.go:884-955`). It then
sums all components into `TotalCost` and applies `RateMultiplier` last to
produce `ActualCost`
(`repo-ref/sub2api/backend/internal/service/billing_service.go:957-967`).

If a resolver provides explicit interval pricing, Sub2API selects that row by
`ordinary_input + cache_read` and suppresses the extra model-specific
long-context multiplication, avoiding a double uplift
(`repo-ref/sub2api/backend/internal/service/billing_service.go:855-869`).

### CSV field meaning

The Sub2API frontend writes `log.actual_cost` under `Billed Cost` and
`log.total_cost` under `Original Cost`
(`repo-ref/sub2api/frontend/src/views/user/UsageView.vue:638-675`). Therefore:

```text
Original Cost = TotalCost
Billed Cost   = ActualCost = TotalCost * Rate Multiplier
```

The usage log persists `actualInputTokens`, not the upstream inclusive input
counter, in its `input_tokens` field
(`repo-ref/sub2api/backend/internal/service/openai_gateway_usage.go:224-232`).
Consequently, when recalculating an exported CSV row, use:

```text
I = CSV "Input Tokens"
R = CSV "Cache Read Tokens"
```

Do **not** subtract the CSV cache-read column from the CSV input column again.
Doing so would undercount ordinary input.

When every exported `Rate Multiplier` is `1`, equality between the two columns
is expected; it is not evidence that the component price calculation was
skipped.

### Local CSV reconciliation

The supplied export contained 8,405 `gpt-5.6-sol` rows from
`2026-07-11T11:19:59+08:00` through `2026-07-12T10:28:17+08:00`. Every row
used rate multiplier `1`. The aggregate token and cost audit was:

| Item | Audited value |
| --- | ---: |
| Ordinary input tokens | `56,636,015` |
| Output tokens | `5,377,737` |
| Cache-read tokens | `1,098,208,768` |
| Cache-creation tokens | `0` |
| Base input cost | `$283.180075` |
| Base output cost | `$161.332110` |
| Base cache-read cost | `$549.104384` |
| Base cost before long-context uplift | `$993.616569` |
| Requests above 272,000 input-side tokens | `38` |
| Long-context uplift | `$7.985116` |
| Recalculated and exported billed cost | `$1,001.601685` |

The formula reproduced all 8,405 exported rows exactly; the maximum absolute
row residual was zero. `Original Cost` and `Billed Cost` were also equal on
every row, as expected from multiplier `1`.

The export crosses a midnight quota boundary, so its total must not be divided
by its 23.14-hour wall-clock span and presented as one natural day's use. The
calendar-period split is:

| Asia/Shanghai date | Exported cost |
| --- | ---: |
| 2026-07-11 | `$501.493510` |
| 2026-07-12 | `$500.108175` |

At the time of verification, the remote `input20` key-level `today_used` value
was exactly `$500.108175`. This makes `input20` attribution strongly supported
and shows that the export contains two approximately `$500` daily quota
periods. A naive rolling normalization produces `$1,038.91 per 24h`, but that
figure is not the key's midnight-to-midnight daily consumption.

## BaseLLM source and current `gpt-5.6-sol` record

codex-helper's constant and CLI default both point to:

```text
https://basellm.github.io/llm-metadata/api/all.json
```

The constant is in `crates/core/src/pricing.rs:10`, and the CLI default is in
`src/cli_types.rs:1131-1138`.

The publishing repository is:

```text
https://github.com/basellm/llm-metadata
```

Its README describes the service as a static API served through GitHub Pages
and states that its data comes from `models.dev/api.json` plus BaseLLM community
contributions. BaseLLM is therefore a third-party pricing aggregate, not an
OpenAI-owned billing API. It is appropriate for an estimate catalog, but the
relay's billed balance remains the financial source of truth.

The live `openai.models.gpt-5.6-sol.cost` object fetched at
`2026-07-12T06:57:24Z` contained:

```json
{
  "input": 5,
  "output": 30,
  "cache_read": 0.5,
  "cache_write": 6.25,
  "tiers": [
    {
      "tier": { "type": "context", "size": 272000 },
      "input": 10,
      "output": 45,
      "cache_read": 1,
      "cache_write": 12.5
    }
  ]
}
```

The same response also exposed a `context_over_200k` object with the same
uplifted values. The numeric tier boundary, rather than that legacy-looking
field name, agrees with Sub2API's 272,000-token rule.

## codex-helper synchronization and runtime use

### Explicit pricing import

`pricing sync-basellm` performs an HTTP(S) GET, parses BaseLLM `all.json`, and
passes the resulting price snapshot to the common import path
(`src/commands/pricing.rs:93-103`, `src/commands/pricing.rs:115-142`). By
default, import merges matched rows with existing local overrides; `--replace`
starts from an empty override document. `--dry-run` does not write
(`src/commands/pricing.rs:145-180`).

The importer reads only these scalar base fields:

- `cost.input`
- `cost.output`
- `cost.cache_read`
- `cost.cache_write`

That mapping is explicit in `crates/core/src/pricing.rs:703-755`. The local
schema likewise contains only input, output, cache-read, and cache-creation
per-million values (`crates/core/src/pricing.rs:338-370`). There is no context
tier field in `ModelPrice`.

The installed `ch.exe` was also checked non-destructively:

```text
ch.exe pricing sync-basellm --model gpt-5.6-sol --dry-run
Would import 1 pricing row(s) ... into ~/.codex-helper/pricing_overrides.toml
```

This confirms the running CLI build recognizes the current remote row. The
dry run did not change the override file.

### Persistence and runtime merge

The override path is constructed as
`~/.codex-helper/pricing_overrides.toml`
(`crates/core/src/pricing.rs:814-815`). Save validates, normalizes, serializes,
and writes that TOML document (`crates/core/src/pricing.rs:837-853`).

At runtime, codex-helper clones the bundled catalog and inserts local rows over
it (`crates/core/src/pricing.rs:873-888`). The operator estimator rebuilds that
merged catalog from disk and uses it for cost calculation
(`crates/core/src/pricing.rs:961-1016`). Both live request completion and
request-log reconstruction call the operator estimator
(`crates/core/src/state.rs:3130-3141`,
`crates/core/src/request_ledger.rs:612-625`).

The bundled seed currently starts at `gpt-5.5` and has no `gpt-5.6-sol` row
(`crates/core/src/pricing.rs:1173-1215`). Consequently, a BaseLLM import or a
manual override is required for a priced `gpt-5.6-sol` estimate in this
snapshot.

### Background metadata sync is separate

`sync_basellm_metadata_cache_background` fetches the same BaseLLM URL and uses
ETag/Last-Modified conditional requests
(`crates/core/src/basellm_metadata.rs:65-151`). However, its cache model stores
display name, description, context limits, modalities, and capability flags;
it has no price fields (`crates/core/src/basellm_metadata.rs:31-44`). Its parser
likewise selects capability metadata only
(`crates/core/src/basellm_metadata.rs:179-240`).

Therefore this background job cannot refresh
`pricing_overrides.toml`, and a successful metadata update must not be shown as
a successful price update.

## Confirmed accuracy gap and recommended correction

Base-price synchronization works, but tiered pricing does not. For a
long-context `gpt-5.6-sol` request, codex-helper currently estimates all tokens
at `$5/$30/$0.50/$6.25` instead of applying the
`$10/$45/$1/$12.50` tier. Sub2API and the current BaseLLM dataset agree on the
tier, so this is a codex-helper schema/import limitation rather than an
upstream ambiguity.

Recommended implementation order:

1. Extend `ModelPrice` and the local override schema with context-tier rows
   containing a threshold and component prices.
2. Import `cost.tiers` from BaseLLM; retain the base row as a fallback and
   record source URL, ETag, and fetch time.
3. Apply the tier using ordinary input plus cache-read input, matching the
   relay's context accounting. Add boundary tests for exactly 272,000 and
   272,001 tokens.
4. Reuse the existing conditional-fetch cache machinery for scheduled pricing
   refreshes, but keep the last known-good catalog on HTTP or validation
   failure.
5. Continue treating remote relay quota/balance deltas as total shared-key
   consumption. Use the local priced request ledger for project attribution
   and expose any difference as external or unattributed usage.

Until tier support exists, the UI should label long-context local cost as a
lower-bound estimate rather than an exact billed amount.

## Public source links

- BaseLLM repository: <https://github.com/basellm/llm-metadata>
- BaseLLM published catalog: <https://basellm.github.io/llm-metadata/api/all.json>
- Sub2API snapshot: <https://github.com/Wei-Shaw/sub2api/tree/6f43986c376d76144cb39c7a562c179e19ac7439>
- Sub2API dynamic price source: <https://github.com/Wei-Shaw/model-price-repo>
