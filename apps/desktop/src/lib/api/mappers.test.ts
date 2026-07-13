import { describe, expect, it } from "vitest";

import type {
  ApiOperatorRequestSummary,
  ApiOperatorSummary,
  ApiUsageBucket,
  ApiUsageDayView,
  ApiUsageMetrics,
} from "@/lib/api/admin-types";
import {
  mapAdminDashboardData,
  mapProvidersData,
  mapUsageData,
} from "@/lib/api/mappers";

function usageMetrics(overrides: Partial<ApiUsageMetrics> = {}): ApiUsageMetrics {
  return {
    input_tokens: 0,
    output_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    ...overrides,
  };
}

function usageBucket(
  overrides: Omit<Partial<ApiUsageBucket>, "usage"> & {
    usage?: Partial<ApiUsageMetrics>;
  } = {},
): ApiUsageBucket {
  const { usage, ...bucketOverrides } = overrides;
  return {
    requests_total: 0,
    requests_error: 0,
    duration_ms_total: 0,
    requests_with_usage: 0,
    duration_ms_with_usage_total: 0,
    generation_ms_total: 0,
    ttfb_ms_total: 0,
    ttfb_samples: 0,
    usage: usageMetrics(usage),
    ...bucketOverrides,
  };
}

function emptyUsageDay(): ApiUsageDayView {
  return {
    day: 0,
    label: "",
    start_ms: 0,
    end_ms: 0,
    generated_at_ms: 0,
    summary: usageBucket(),
    hourly: [],
    provider_rows: [],
    provider_endpoint_rows: [],
    model_rows: [],
    session_rows: [],
    project_rows: [],
    retry_gate: {
      active: 0,
      active_cooldowns: 0,
      max_remaining_secs: null,
      reasons: [],
    },
    coverage: {
      source: "runtime_store",
      loaded_first_ms: null,
      loaded_last_ms: null,
      loaded_requests: 0,
      day_may_be_partial: false,
    },
  };
}

const operatorSummary: ApiOperatorSummary = {
  api_version: 1,
  service_name: "codex",
  runtime: {
    runtime_loaded_at_ms: Date.now(),
    runtime_source_mtime_ms: null,
    configured_default_profile: "chatgpt-bridge",
    default_profile: "chatgpt-bridge",
    default_profile_summary: {
      name: "chatgpt-bridge",
      model: null,
      reasoning_effort: null,
      service_tier: null,
      fast_mode: false,
    },
  },
  counts: {
    active_requests: 0,
    providers: 2,
    recent_requests: 1,
    sessions: 0,
    profiles: 0,
  },
  retry: {
    configured_profile: "balanced",
    upstream_max_attempts: 2,
    provider_max_attempts: 2,
    recent_retried_requests: 0,
    recent_cross_provider_failovers: 0,
    recent_same_provider_retries: 0,
    recent_fast_mode_requests: 0,
  },
  providers: [
    {
      name: "codex-air",
      alias: "CodeX Air",
      configured_enabled: true,
      effective_enabled: true,
      routable_endpoints: 1,
      endpoints: [
        {
          provider_name: "codex-air",
          name: "responses-compact",
          provider_endpoint_key: "endpoint:sha256:air",
          origin: "https://ai.input.im",
          priority: 0,
          configured_enabled: true,
          effective_enabled: true,
          routable: true,
          runtime_state: "normal",
        },
      ],
    },
    {
      name: "backup",
      configured_enabled: true,
      effective_enabled: true,
      routable_endpoints: 0,
      endpoints: [
        {
          provider_name: "backup",
          name: "openai",
          provider_endpoint_key: "endpoint:sha256:backup",
          origin: "https://api.openai.com",
          priority: 0,
          configured_enabled: true,
          effective_enabled: true,
          routable: false,
          runtime_state: "breaker_open",
        },
      ],
    },
  ],
  sessions: [],
  profiles: [],
};

const finishedRequest: ApiOperatorRequestSummary = {
  id: 7,
  session_key: "session:sha256:test",
  model: "gpt-5.4",
  reasoning_effort: "high",
  service_tier: "priority",
  provider_id: "codex-air",
  endpoint_id: "responses",
  provider_endpoint_key: "endpoint:sha256:air",
  route_path: ["default", "codex-air", "responses"],
  upstream_origin: "https://ai.input.im",
  usage: usageMetrics({
    input_tokens: 1000,
    output_tokens: 500,
    total_tokens: 1500,
    cached_input_tokens: 200,
  }),
  cost: {
    input_cost_usd: "0.001",
    output_cost_usd: "0.002",
    cache_read_cost_usd: "0.0001",
    total_cost_usd: "0.0031",
    confidence: "estimated",
    pricing_source: "operator catalog",
    pricing_provider: "openai",
    pricing_generation: "remote:generation-7",
    effective_pricing_revision: "pricing:sha256:test",
    selected_tier: {
      tier_type: "context_length",
      threshold_tokens: 200_000,
      matched_input_tokens: 240_000,
    },
  },
  observability: {
    attempt_count: 1,
    route_attempt_count: 1,
    retried: false,
    cross_provider_failover: false,
    same_provider_retry: false,
    fast_mode: false,
    streaming: true,
  },
  service: "codex",
  method: "POST",
  path: "/v1/responses",
  status_code: 200,
  duration_ms: 1500,
  ttfb_ms: 420,
  streaming: true,
  ended_at_ms: Date.UTC(2026, 4, 21, 7, 30, 0),
};

const usageDay: ApiUsageDayView = {
  day: 20_229,
  label: "2026-05-21",
  start_ms: Date.UTC(2026, 4, 21),
  end_ms: Date.UTC(2026, 4, 22),
  generated_at_ms: Date.UTC(2026, 4, 21, 8),
  summary: usageBucket({
    requests_total: 12,
    requests_error: 1,
    duration_ms_total: 24_000,
    requests_with_usage: 12,
    duration_ms_with_usage_total: 24_000,
    generation_ms_total: 21_000,
    ttfb_ms_total: 3_000,
    ttfb_samples: 6,
    usage: {
      input_tokens: 10_000,
      output_tokens: 2_000,
      total_tokens: 12_000,
      cache_read_input_tokens: 4_000,
    },
    cost: {
      total_cost_usd: "0.025",
      confidence: "partial",
      priced_requests: 10,
      unpriced_requests: 2,
      partial_requests: 3,
      exact_requests: 4,
    },
  }),
  hourly: [
    {
      hour: 7,
      bucket: usageBucket({
        requests_total: 12,
        usage: { total_tokens: 12_000 },
        cost: { total_cost_usd: "0.025", confidence: "estimated" },
      }),
    },
  ],
  provider_rows: [
    {
      name: "codex-air",
      bucket: usageBucket({
        requests_total: 12,
        duration_ms_total: 24_000,
        usage: { total_tokens: 12_000 },
        cost: { total_cost_usd: "0.025", confidence: "estimated" },
      }),
    },
  ],
  provider_endpoint_rows: [
    {
      name: "codex/codex-air/default",
      bucket: usageBucket({
        requests_total: 12,
        duration_ms_total: 24_000,
        usage: { total_tokens: 12_000 },
        cost: { total_cost_usd: "0.025", confidence: "estimated" },
      }),
    },
  ],
  model_rows: [
    {
      name: "gpt-5.4",
      bucket: usageBucket({
        requests_total: 12,
        usage: { total_tokens: 12_000 },
      }),
    },
  ],
  project_rows: [
    {
      name: "codex-helper",
      bucket: usageBucket({
        requests_total: 8,
        usage: { total_tokens: 8_000 },
      }),
    },
  ],
  session_rows: [],
  coverage: {
    source: "runtime_store",
    loaded_first_ms: null,
    loaded_last_ms: null,
    loaded_requests: 12,
    day_may_be_partial: true,
    partial_reason: "loaded data starts after local day start",
  },
  retry_gate: {
    active: 2,
    active_cooldowns: 1,
    max_remaining_secs: 90,
    reasons: [{ reason: "upstream_rate_limited", active: 1 }],
  },
};

describe("admin API mappers", () => {
  it("maps operator summary into dashboard data", () => {
    const data = mapAdminDashboardData({
      summary: operatorSummary,
      recentRequests: [finishedRequest],
      usageDay,
      endpoint: {
        proxyPort: 3211,
        adminPort: 4211,
        proxyBaseUrl: "http://127.0.0.1:3211",
        adminBaseUrl: "http://127.0.0.1:4211",
      },
      appVersion: "0.20.0",
      capturedAtMs: Date.now(),
    });

    expect(data.runtime.port).toBe(3211);
    expect(data.runtime.adminPort).toBe(4211);
    expect(data.runtime.provider).toBe("codex-air");
    expect(data.providers[0]).toMatchObject({
      name: "codex-air",
      alias: "CodeX Air",
      endpointCount: 1,
      routableEndpoints: 1,
    });
    expect(data.providers[0]).not.toHaveProperty("active");
    expect(data.recentRequests[0]).toMatchObject({
      id: "7",
      model: "gpt-5.4",
      status: "ok",
      cost: "$0.0031",
    });
    expect(data.metrics.find((metric) => metric.label === "今日请求")?.value).toBe("12");
  });

  it("maps canonical provider inventory without fabricating route order", () => {
    const data = mapProvidersData(operatorSummary);

    expect(data.providers).toHaveLength(2);
    expect(data.providers[0]).not.toHaveProperty("capabilities");
    expect(data.providers[1]).toMatchObject({
      configuredEnabled: true,
      effectiveEnabled: true,
      routableEndpoints: 0,
    });
    expect(data.providers[1].endpoints[0]).toMatchObject({
      runtimeState: "breaker_open",
      routable: false,
    });
    expect(Object.keys(data)).toEqual(["providers"]);
  });

  it("does not infer an active provider without an explicit canonical fact", () => {
    const data = mapAdminDashboardData({
      summary: operatorSummary,
      recentRequests: [],
      usageDay: emptyUsageDay(),
      endpoint: {
        proxyPort: 3211,
        adminPort: 4211,
        proxyBaseUrl: "http://127.0.0.1:3211",
        adminBaseUrl: "http://127.0.0.1:4211",
      },
      appVersion: "0.20.0",
      capturedAtMs: Date.now(),
    });

    expect(data.runtime.provider).toBe("unknown");
    expect(data.providers.every((provider) => !("active" in provider))).toBe(true);
    expect(data.metrics).toContainEqual(expect.objectContaining({
      label: "最近供应商",
      value: "unknown",
    }));
  });

  it("maps active policy action projections into provider control inventory", () => {
    const data = mapProvidersData({
      ...operatorSummary,
      providers: [
        {
          ...operatorSummary.providers[0],
          endpoints: [
            {
              ...operatorSummary.providers[0].endpoints[0],
              policy_actions: [
                {
                  active_cooldown: true,
                  code: "cooldown",
                  cooldown_remaining_secs: 30,
                },
              ],
              runtime_state_override: "breaker_open",
            },
          ],
        },
      ],
    });

    expect(data.providers[0].endpoints[0]).toMatchObject({
      runtimeState: "normal",
      routable: true,
      policyActionCount: 1,
    });
    expect(data.providers[0].controlSummary).toBe("2 active control events");
    expect(data.providers[0].controlBadges).toEqual([
      expect.objectContaining({
        key: expect.stringContaining("runtime_state"),
        label: "state breaker_open",
        tone: "warning",
      }),
      expect.objectContaining({
        key: expect.stringContaining("policy:cooldown"),
        label: "cooldown",
        detail: expect.stringContaining("remaining 30s"),
        tone: "warning",
      }),
    ]);
  });

  it("does not rebuild daily economics from recent request rows", () => {
    const data = mapUsageData({
      recentRequests: [finishedRequest],
      usageDay: emptyUsageDay(),
    });

    expect(data.summary.totalRows).toBe(0);
    expect(data.summary.totalTokens).toBe("0");
    expect(data.summary.estimatedCost).toBe("unknown");
    expect(data.rows[0]).toMatchObject({
      provider: "codex-air",
      type: "流式",
      firstToken: "420ms",
      duration: "1.5s",
      cost: "$0.0031",
      costBreakdown: {
        source: "operator catalog",
        pricingProvider: "openai",
        pricingGeneration: "remote:generation-7",
        effectivePricingRevision: "pricing:sha256:test",
        selectedTier: {
          type: "context_length",
          thresholdTokens: 200_000,
          matchedInputTokens: 240_000,
        },
      },
    });
    expect(data.rows[0].tokens.cache).toBe("—");
  });

  it("does not reconstruct canonical totals or cache rate from token aliases", () => {
    const data = mapUsageData({
      usageDay: {
        ...usageDay,
        summary: {
          ...usageDay.summary,
          usage: usageMetrics({
            input_tokens: 10_000,
            output_tokens: 2_000,
            cached_input_tokens: 4_000,
            cache_creation_input_tokens: 1_000,
          }),
        },
        hourly: [
          {
            hour: 7,
            bucket: usageBucket({
              requests_total: 12,
              usage: {
                input_tokens: 10_000,
                output_tokens: 2_000,
                cache_read_input_tokens: 4_000,
              },
            }),
          },
        ],
      },
      recentRequests: [],
    });

    expect(data.summary.totalTokens).toBe("0");
    expect(data.summary.cacheRate).toBe("—");
    expect(data.hourly[7].totalTokens).toBe(0);
  });

  it("uses usage_day as the canonical daily usage source", () => {
    const data = mapUsageData({
      usageDay,
      recentRequests: [],
    });

    expect(data.summary).toMatchObject({
      totalRows: 12,
      totalRequests: "12",
      estimatedCost: "$0.025",
      averageDuration: "2.0s",
      averageFirstToken: "500ms",
      dayLabel: "2026-05-21",
      costCoverage: {
        confidence: "partial",
        pricedRequests: 10,
        unpricedRequests: 2,
        partialRequests: 3,
        exactRequests: 4,
      },
    });
    expect(data.rows).toHaveLength(0);
    expect(data.hourly[7]).toMatchObject({ requests: 12, totalTokens: 12000 });
    expect(data.providerRows[0]).toMatchObject({ name: "codex-air", requests: 12 });
    expect(data.providerEndpointRows[0]).toMatchObject({
      name: "codex/codex-air/default",
      requests: 12,
    });
    expect(data.coverage).toEqual({
      source: "runtime_store",
      isPartial: true,
      reason: "loaded data starts after local day start",
      loadedRequests: 12,
    });
    expect(data.retryGate).toMatchObject({
      active: 2,
      activeCooldowns: 1,
      maxRemaining: "2m",
    });
  });

  it("maps provider control evidence from top-level request evidence", () => {
    const data = mapAdminDashboardData({
      summary: operatorSummary,
      recentRequests: [
        {
          ...finishedRequest,
          status_code: 429,
          provider_signal_codes: ["provider_rate_limited"],
          policy_action_codes: ["provider_cooldown"],
        },
      ],
      usageDay,
      endpoint: {
        proxyPort: 3211,
        adminPort: 4211,
        proxyBaseUrl: "http://127.0.0.1:3211",
        adminBaseUrl: "http://127.0.0.1:4211",
      },
      appVersion: "0.20.0",
      capturedAtMs: Date.now(),
    });

    expect(data.recentRequests[0]).toMatchObject({
      status: "warn",
      providerControl: "signal provider_rate_limited · action provider_cooldown",
    });
  });

  it("maps provider control evidence from retry attempts", () => {
    const data = mapAdminDashboardData({
      summary: operatorSummary,
      recentRequests: [
        {
          ...finishedRequest,
          status_code: 429,
          retry: {
            attempts: 2,
            route_attempts: [
              {
                attempt_index: 0,
                code: "failed_status",
                skipped: false,
                provider_signal_codes: ["rate_limit"],
                policy_action_codes: ["cooldown"],
              },
            ],
          },
        },
      ],
      usageDay,
      endpoint: {
        proxyPort: 3211,
        adminPort: 4211,
        proxyBaseUrl: "http://127.0.0.1:3211",
        adminBaseUrl: "http://127.0.0.1:4211",
      },
      appVersion: "0.20.0",
      capturedAtMs: Date.now(),
    });

    expect(data.recentRequests[0]).toMatchObject({
      status: "warn",
      providerControl: "signal rate_limit · action cooldown",
    });
  });
});
