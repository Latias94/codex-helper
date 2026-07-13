import { describe, expect, it } from "vitest";

import type {
  ApiAttributionCoverage,
  ApiCostBreakdown,
  ApiModelPriceView,
  ApiQuotaAnalyticsView,
  ApiQuotaQuantity,
} from "@/lib/api/admin-types";

const exactQuotaValue = "9007199254740993";

const usdQuantity = {
  value: exactQuotaValue,
  scale: 2,
  unit: "usd",
  conversion_generation: 7,
} satisfies ApiQuotaQuantity;

const completeCoverage = {
  loaded_first_ms: 1_000,
  loaded_last_ms: 2_000,
  queried_first_ms: 1_000,
  queried_last_ms: 2_000,
  time_truncated: false,
  count_truncated: false,
  dedupe_truncated: false,
  boundary_partial: false,
  leading_boundary_partial: false,
  trailing_boundary_partial: false,
  cost_overflow: false,
  duplicate_requests: 0,
  partial_captured_price_requests: 0,
  reconstructed_price_requests: 0,
  invalid_captured_price_requests: 0,
  unpriced_requests: 0,
  unmatched_endpoint_requests: 0,
  unmatched_pool_requests: 0,
  unknown_project_requests: 0,
} satisfies ApiAttributionCoverage;

const quotaAnalytics = {
  support: "supported",
  generated_at_ms: 2_000,
  registry_generation: 9,
  pools: [
    {
      identity: {
        key: "pool:sha256:test",
        origin: "https://api.example.test",
        scope: { kind: "custom", value: "team" },
        revision: 3,
        evidence: "remote_quota_owner_id",
        confidence: "high",
        aggregation_eligible: true,
      },
      observed_at_ms: 2_000,
      last_success_at_ms: 2_000,
      last_attempt_at_ms: 2_000,
      freshness: "fresh",
      latest_adjustment: null,
      source: "remote_usage",
      unit: "usd",
      conversion: {
        source: "remote",
        divisor: null,
        generation: 7,
      },
      capabilities: {
        used: true,
        remaining: true,
        direct_total: true,
        limit: true,
        reset: true,
        window: true,
        conversion: true,
        cumulative: true,
      },
      window: {
        kind: "monthly",
        reset: "explicit_timestamp",
        reset_timezone: "UTC",
      },
      epoch_start_ms: 1_000,
      epoch_end_ms: null,
      remote_used: usdQuantity,
      remote_direct_total: usdQuantity,
      remote_remaining: usdQuantity,
      remote_limit: usdQuantity,
      observed_burn: usdQuantity,
      rate_15m: {
        status: "available",
        rate_per_hour: usdQuantity,
        lower_bound: false,
        sample_count: 4,
        span_ms: 900_000,
      },
      rate_60m: {
        status: "insufficient_samples",
        rate_per_hour: null,
        lower_bound: false,
        sample_count: 1,
        span_ms: 0,
      },
      pacing: {
        status: "on_pace",
        required_rate_per_hour: usdQuantity,
        pace_ratio_basis_points: 10_000,
        exhaustion_eta_ms: null,
        projected_remaining_at_reset: usdQuantity,
        reset_at_ms: 3_000,
      },
      reconciliation: {
        status: "available",
        remote_total: usdQuantity,
        local_known: usdQuantity,
        local_unknown: null,
        external_unattributed: usdQuantity,
        signed_delta: "-0.25",
        projects: [
          {
            project: { kind: "git_root", path: "/workspace/project" },
            local_cost: usdQuantity,
            requests: 2,
          },
        ],
        omitted_projects: 0,
        omitted_local_known: null,
        coverage: completeCoverage,
      },
    },
  ],
  omitted_pools: 0,
} satisfies ApiQuotaAnalyticsView;

const tieredModelPrice = {
  provider: "openai",
  model_id: "gpt-test",
  aliases: ["gpt-test-latest"],
  input_per_1m_usd: "1.25",
  output_per_1m_usd: "5",
  tiers: [
    {
      threshold_tokens: 200_000,
      input_per_1m_usd: "2.5",
      output_per_1m_usd: "10",
    },
  ],
  source: "remote",
  source_generation: "generation-7",
  confidence: "exact",
} satisfies ApiModelPriceView;

const selectedTierCost = {
  total_cost_usd: "0.42",
  confidence: "exact",
  pricing_source: "remote",
  pricing_provider: "openai",
  pricing_generation: "generation-7",
  effective_pricing_revision: "pricing:sha256:test",
  selected_tier: {
    tier_type: "context_length",
    threshold_tokens: 200_000,
    matched_input_tokens: 240_000,
  },
} satisfies ApiCostBreakdown;

describe("admin wire types", () => {
  it("represents quota analytics without flattening typed quantities or reconciliation evidence", () => {
    expect(quotaAnalytics.pools[0].identity.scope).toEqual({ kind: "custom", value: "team" });
    expect(quotaAnalytics.pools[0].remote_used?.value).toBe(exactQuotaValue);
    expect(quotaAnalytics.pools[0].reconciliation.signed_delta).toBe("-0.25");
  });

  it("represents tiered catalog prices and selected price provenance", () => {
    expect(tieredModelPrice.tiers[0].threshold_tokens).toBe(200_000);
    expect(selectedTierCost.selected_tier.matched_input_tokens).toBe(240_000);
  });
});
