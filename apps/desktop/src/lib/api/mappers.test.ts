import { describe, expect, it } from "vitest";

import type { ApiFinishedRequest, ApiOperatorSummary, ApiUsageDayView } from "@/lib/api/admin-types";
import {
  mapAdminDashboardData,
  mapProvidersData,
  mapUsageData,
} from "@/lib/api/mappers";

const operatorSummary: ApiOperatorSummary = {
  api_version: 1,
  service_name: "codex",
  runtime: {
    runtime_loaded_at_ms: Date.now(),
    configured_active_station: "codex-air",
    effective_active_station: "codex-air",
    default_profile: "chatgpt-bridge",
  },
  counts: {
    providers: 2,
    recent_requests: 1,
  },
  retry: {
    upstream_max_attempts: 2,
    provider_max_attempts: 2,
  },
  health: {
    stations_with_probe_failures: 1,
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
          base_url: "https://ai.input.im/v1",
          continuity_domain: "relay-cluster-a",
          effective_continuity_domain: "relay-cluster-a",
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
          base_url: "https://api.openai.com/v1",
          configured_enabled: true,
          effective_enabled: true,
          routable: false,
          runtime_state: "breaker_open",
        },
      ],
    },
  ],
};

const finishedRequest: ApiFinishedRequest = {
  id: 7,
  trace_id: "codex-7",
  model: "gpt-5.4",
  reasoning_effort: "high",
  service_tier: "priority",
  station_name: "codex-air",
  provider_id: "codex-air",
  upstream_base_url: "https://ai.input.im/v1",
  usage: {
    input_tokens: 1000,
    output_tokens: 500,
    total_tokens: 1500,
    cached_input_tokens: 200,
  },
  cost: {
    input_cost_usd: "0.001",
    output_cost_usd: "0.002",
    cache_read_cost_usd: "0.0001",
    total_cost_usd: "0.0031",
    confidence: "estimated",
    pricing_source: "operator catalog",
  },
  observability: {
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
  summary: {
    requests_total: 12,
    requests_error: 1,
    duration_ms_total: 24_000,
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
      confidence: "estimated",
      priced_requests: 12,
    },
  },
  hourly: [
    {
      hour: 7,
      bucket: {
        requests_total: 12,
        usage: { total_tokens: 12_000 },
        cost: { total_cost_usd: "0.025", confidence: "estimated" },
      },
    },
  ],
  provider_rows: [
    {
      name: "codex-air",
      bucket: {
        requests_total: 12,
        duration_ms_total: 24_000,
        usage: { total_tokens: 12_000 },
        cost: { total_cost_usd: "0.025", confidence: "estimated" },
      },
    },
  ],
  model_rows: [
    {
      name: "gpt-5.4",
      bucket: {
        requests_total: 12,
        usage: { total_tokens: 12_000 },
      },
    },
  ],
  project_rows: [
    {
      name: "codex-helper",
      bucket: {
        requests_total: 8,
        usage: { total_tokens: 8_000 },
      },
    },
  ],
  coverage: {
    source: "request-ledger",
    loaded_requests: 12,
    scanned_lines: 12,
    day_may_be_partial: true,
    partial_reason: "replay started after local day start",
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
      usageSummary: [
        {
          group_value: "codex-air",
          aggregate: {
            requests: 1,
            total_tokens: 1500,
            duration_ms_total: 1500,
          },
        },
      ],
      adminBaseUrl: "http://127.0.0.1:4211",
      appVersion: "0.20.0",
    });

    expect(data.runtime.port).toBe(3211);
    expect(data.runtime.adminPort).toBe(4211);
    expect(data.runtime.provider).toBe("codex-air");
    expect(data.providers[0]).toMatchObject({
      name: "CodeX Air",
      baseUrl: "https://ai.input.im/v1",
      continuityDomain: "relay-cluster-a",
      host: "ai.input.im",
      endpointCount: 1,
      editable: true,
      health: "Healthy",
      active: true,
    });
    expect(data.recentRequests[0]).toMatchObject({
      id: "codex-7",
      model: "gpt-5.4",
      status: "ok",
      cost: "$0.0031",
    });
    expect(data.metrics.find((metric) => metric.label === "今日请求")?.value).toBe("12");
  });

  it("maps providers into route-order data", () => {
    const data = mapProvidersData(operatorSummary);

    expect(data.providers).toHaveLength(2);
    expect(data.providers[0].capabilities).toContain("continuity domain");
    expect(data.providers[1].health).toBe("Error");
    expect(data.routeOrder[0].active).toBe(true);
  });

  it("maps active policy action projections into provider health", () => {
    const data = mapProvidersData({
      ...operatorSummary,
      providers: [
        {
          ...operatorSummary.providers![0],
          endpoints: [
            {
              ...operatorSummary.providers![0].endpoints![0],
              policy_actions: [
                {
                  active_cooldown: true,
                  code: "cooldown",
                  reason: "upstream_rate_limited",
                  cooldown_remaining_secs: 30,
                  action_id: "a1",
                },
              ],
              runtime_state_override: "breaker_open",
            },
          ],
        },
      ],
    });

    expect(data.providers[0].health).toBe("Warning");
    expect(data.providers[0].controlSummary).toBe("2 active control events");
    expect(data.providers[0].controlBadges).toEqual([
      expect.objectContaining({
        key: expect.stringContaining("runtime_state"),
        label: "state breaker_open",
        tone: "warning",
      }),
      expect.objectContaining({
        key: expect.stringContaining("policy:a1"),
        label: "cooldown",
        detail: expect.stringContaining("remaining 30s"),
        tone: "warning",
      }),
    ]);
  });

  it("maps request-ledger rows into usage table rows and summary cards", () => {
    const data = mapUsageData({
      recentRequests: [finishedRequest],
      usageSummary: [],
    });

    expect(data.summary.totalRows).toBe(1);
    expect(data.summary.estimatedCost).toBe("$0.0031");
    expect(data.rows[0]).toMatchObject({
      provider: "codex-air",
      type: "流式",
      firstToken: "420ms",
      duration: "1.5s",
    });
    expect(data.rows[0].tokens.cache).toBe("20%");
  });

  it("uses usage_day as the canonical daily usage source", () => {
    const data = mapUsageData({
      usageDay,
      recentRequests: [],
      usageSummary: [],
    });

    expect(data.summary).toMatchObject({
      totalRows: 12,
      totalRequests: "12",
      estimatedCost: "$0.025",
      averageDuration: "2.0s",
      averageFirstToken: "500ms",
      dayLabel: "2026-05-21",
    });
    expect(data.rows).toHaveLength(0);
    expect(data.hourly[7]).toMatchObject({ requests: 12, totalTokens: 12000 });
    expect(data.providerRows[0]).toMatchObject({ name: "codex-air", requests: 12 });
    expect(data.coverage).toMatchObject({
      isPartial: true,
      reason: "replay started after local day start",
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
          provider_signals: [{ kind: "rate_limit", code: "provider_rate_limited" }],
          policy_actions: [{ kind: "cooldown", code: "provider_cooldown" }],
        },
      ],
      usageSummary: [],
      adminBaseUrl: "http://127.0.0.1:4211",
      appVersion: "0.20.0",
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
            upstream_chain: [],
            route_attempts: [
              {
                decision: "failed_status",
                provider_signals: [{ kind: "rate_limit" }],
                policy_actions: [{ kind: "cooldown" }],
              },
            ],
          },
        },
      ],
      usageSummary: [],
      adminBaseUrl: "http://127.0.0.1:4211",
      appVersion: "0.20.0",
    });

    expect(data.recentRequests[0]).toMatchObject({
      status: "warn",
      providerControl: "signal rate_limit · action cooldown",
    });
  });
});
