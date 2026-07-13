import {
  BadgeDollarSign,
  Cable,
  Clock3,
  Database,
  Network,
  Server,
  Zap,
  type LucideIcon,
} from "lucide-react";

import type {
  ApiCostBreakdown,
  ApiOperatorProviderCapacity,
  ApiOperatorProviderSummary,
  ApiOperatorRequestSummary,
  ApiOperatorSummary,
  ApiUsageBucket,
  ApiUsageDayDimensionRow,
  ApiUsageDayView,
  ApiUsageMetrics,
} from "@/lib/api/admin-types";
import type {
  DashboardData,
  DashboardMetric,
  DashboardMetricTone,
  ProviderCardView,
  ProviderControlBadgeView,
  RecentRequestView,
  RuntimeSummary,
  UsageData,
  UsageDimensionRowView,
  UsageHourView,
  UsageRowView,
  UsageSummaryView,
} from "@/lib/api/types";
import { compactInteger } from "@/lib/format/number";
import type { AdminEndpointConfig } from "@/lib/tauri/commands";

type IconMetric = DashboardMetric & { icon: LucideIcon };

export const metricIconByLabel: Record<string, LucideIcon> = {
  本地代理: Server,
  "Codex 连接": Cable,
  最近供应商: Database,
  今日请求: Database,
  "今日 Tokens": Zap,
  预估花费: BadgeDollarSign,
  平均响应: Clock3,
  "Provider Routing": Network,
};

export function attachMetricIcons(metrics: DashboardMetric[]): IconMetric[] {
  return metrics.map((metric) => ({
    ...metric,
    icon: metricIconByLabel[metric.label] ?? Database,
  }));
}

export function mapAdminDashboardData(input: {
  summary: ApiOperatorSummary;
  recentRequests: ApiOperatorRequestSummary[];
  usageDay: ApiUsageDayView;
  endpoint: AdminEndpointConfig;
  appVersion: string;
  capturedAtMs: number;
}): DashboardData {
  const providers = mapProviders(input.summary.providers);
  const recentRequests = mapRecentRequests(input.recentRequests);
  const usageSummary = summarizeUsageDay(input.usageDay);
  const runtime = mapRuntimeSummary({
    endpoint: input.endpoint,
    appVersion: input.appVersion,
    recentRequests: input.recentRequests,
    capturedAtMs: input.capturedAtMs,
  });
  const providerRouting = providerRoutingSummary(providers, input.summary);

  return {
    runtime,
    metrics: [
      {
        label: "本地代理",
        value: runtime.proxy,
        note: runtime.endpoint.replace("http://", ""),
        tone: runtime.mode === "unavailable" ? "warning" : "success",
      },
      {
        label: "Codex 连接",
        value: runtime.codex,
        note: input.summary.runtime.default_profile ?? input.summary.service_name,
        tone: "success",
      },
      {
        label: "最近供应商",
        value: runtime.provider,
        note: `Provider ${input.summary.counts.providers}`,
        tone: "teal",
      },
      {
        label: "今日请求",
        value: usageSummary.totalRequests,
        note: `usage_day ${usageSummary.dayLabel}`,
        tone: "blue",
      },
      {
        label: "今日 Tokens",
        value: usageSummary.totalTokens,
        note: "input / output / cache",
        tone: "default",
      },
      {
        label: "预估花费",
        value: usageSummary.estimatedCost,
        note: "以供应商结算为准",
        tone: "warning",
      },
      {
        label: "平均响应",
        value: usageSummary.averageDuration,
        note: `首 token ${usageSummary.averageFirstToken}`,
        tone: "default",
      },
      {
        label: "Provider Routing",
        value: providerRouting.value,
        note: providerRouting.note,
        tone: providerRouting.tone,
      },
    ],
    recentRequests,
    providers,
    chartBars: usageChartBars(input.usageDay),
  };
}

export function mapProvidersData(summary: ApiOperatorSummary) {
  return {
    providers: mapProviders(summary.providers),
  };
}

export function mapUsageData(input: {
  recentRequests: ApiOperatorRequestSummary[];
  usageDay: ApiUsageDayView;
}): UsageData {
  const rows = mapUsageRows(input.recentRequests);
  const usageDay = input.usageDay;
  return {
    summary: summarizeUsageDay(usageDay),
    hourly: mapUsageHours(usageDay),
    providerRows: mapUsageDimensionRows(usageDay?.provider_rows),
    providerEndpointRows: mapUsageDimensionRows(usageDay?.provider_endpoint_rows),
    modelRows: mapUsageDimensionRows(usageDay?.model_rows),
    sessionRows: mapUsageDimensionRows(usageDay?.session_rows),
    projectRows: mapUsageDimensionRows(usageDay?.project_rows),
    coverage: {
      source: usageDay?.coverage?.source ?? "unavailable",
      isPartial: Boolean(usageDay?.coverage?.day_may_be_partial),
      reason: usageDay?.coverage?.partial_reason ?? undefined,
      loadedRequests: positive(usageDay?.coverage?.loaded_requests),
    },
    retryGate: {
      active: positive(usageDay?.retry_gate?.active),
      activeCooldowns: positive(usageDay?.retry_gate?.active_cooldowns),
      maxRemaining: formatSeconds(usageDay?.retry_gate?.max_remaining_secs ?? undefined),
      reasons: usageDay?.retry_gate?.reasons ?? [],
    },
    rows,
  };
}

export function mapRuntimeSummary(
  options: {
    endpoint: AdminEndpointConfig;
    appVersion: string;
    recentRequests: ApiOperatorRequestSummary[];
    capturedAtMs: number;
  },
): RuntimeSummary {
  const { adminBaseUrl, adminPort, proxyBaseUrl, proxyPort } = options.endpoint;
  const provider = options.recentRequests.find((request) => request.provider_id)?.provider_id ?? "unknown";

  return {
    mode: "running",
    ownerMode: "unknown",
    proxy: "Running",
    port: proxyPort,
    adminPort,
    codex: "已连接",
    provider,
    balance: "预估",
    version: `v${options.appVersion}`,
    endpoint: proxyBaseUrl,
    adminEndpoint: adminBaseUrl,
    updatedAtLabel: formatRelativeMs(options.capturedAtMs),
  };
}

export function mapProviders(
  providers: ApiOperatorProviderSummary[],
): ProviderCardView[] {
  return providers.map((provider) => {
    const endpoints = provider.endpoints ?? [];
    const endpointCount = endpoints.length;
    const controlBadges = providerControlBadges(provider);

    return {
      name: provider.name,
      alias: provider.alias,
      configuredEnabled: provider.configured_enabled,
      effectiveEnabled: provider.effective_enabled,
      routableEndpoints: provider.routable_endpoints,
      endpointCount,
      capacity: capacitySummary(provider.capacity),
      endpoints: endpoints.map((endpoint) => ({
        key: endpoint.provider_endpoint_key,
        name: endpoint.name,
        origin: endpoint.origin ?? "-",
        priority: endpoint.priority,
        configuredEnabled: endpoint.configured_enabled,
        effectiveEnabled: endpoint.effective_enabled,
        routable: endpoint.routable,
        runtimeState: endpoint.runtime_state,
        capacity: capacitySummary(endpoint.capacity),
        policyActionCount: endpoint.policy_actions?.length ?? 0,
      })),
      controlSummary:
        controlBadges.length > 0
          ? `${controlBadges.length} active control event${controlBadges.length === 1 ? "" : "s"}`
          : "无 active retry gate 或 runtime override",
      controlBadges,
    };
  });
}

export function mapRecentRequests(requests: ApiOperatorRequestSummary[]): RecentRequestView[] {
  return requests.slice(0, 5).map((request) => ({
    id: String(request.id),
    model: request.model ?? "unknown",
    status: requestStatus(request.status_code),
    provider: request.provider_id ?? "-",
    tokens: requestTokensLabel(request.usage),
    cost: formatCost(request.cost),
    duration: formatMs(request.duration_ms),
    time: formatClock(request.ended_at_ms),
    providerControl: providerControlSummary(request),
  }));
}

function providerControlBadges(provider: ApiOperatorProviderSummary): ProviderControlBadgeView[] {
  return (provider.endpoints ?? []).flatMap((endpoint) => {
    const endpointLabel = endpoint.name || endpoint.provider_endpoint_key || provider.name;
    const badges: ProviderControlBadgeView[] = [];
    if (endpoint.runtime_enabled_override !== null && endpoint.runtime_enabled_override !== undefined) {
      badges.push({
        key: `${endpointLabel}:runtime_enabled`,
        label: endpoint.runtime_enabled_override ? "enabled override" : "disabled override",
        detail: `${endpointLabel} runtime enabled=${endpoint.runtime_enabled_override}`,
        tone: endpoint.runtime_enabled_override ? "teal" : "warning",
      });
    }
    if (endpoint.runtime_state_override) {
      badges.push({
        key: `${endpointLabel}:runtime_state`,
        label: `state ${endpoint.runtime_state_override}`,
        detail: `${endpointLabel} runtime state override`,
        tone: endpoint.runtime_state_override === "normal" ? "muted" : "warning",
      });
    }
    for (const action of endpoint.policy_actions ?? []) {
      if (!action.active_cooldown) {
        continue;
      }
      const code = action.code ?? "cooldown";
      badges.push({
        key: `${endpointLabel}:policy:${code}`,
        label: code,
        detail: [
          endpointLabel,
          action.cooldown_remaining_secs ? `remaining ${formatSeconds(action.cooldown_remaining_secs)}` : undefined,
        ].filter(Boolean).join(" · "),
        tone: "warning",
      });
    }
    return badges;
  });
}

export function mapUsageRows(requests: ApiOperatorRequestSummary[]): UsageRowView[] {
  return requests.map((request) => {
    const usage = request.usage;
    return {
      id: String(request.id),
      requestId: request.id,
      sessionId: request.session_key,
      provider: request.provider_id ?? "-",
      key: request.provider_endpoint_key ?? request.provider_id ?? "unresolved route",
      model: request.model ?? "unknown",
      effort: request.reasoning_effort ?? request.service_tier ?? "—",
      endpoint: request.path,
      type: request.streaming || request.observability?.streaming ? "流式" : "同步",
      billing:
        request.cost?.confidence && request.cost.confidence !== "unknown" ? "按量" : "未知",
      tokens: {
        input: positive(usage?.input_tokens),
        output: positive(usage?.output_tokens),
        cache: "—",
      },
      cost: formatCost(request.cost),
      costBreakdown: formatCostBreakdown(request.cost),
      firstToken: request.ttfb_ms ? formatMs(request.ttfb_ms) : "—",
      duration: formatMs(request.duration_ms),
      time: formatClock(request.ended_at_ms),
    };
  });
}

function summarizeUsageDay(usageDay: ApiUsageDayView): UsageSummaryView {
  const bucket = usageDay.summary;
  const requests = positive(bucket.requests_total);
  return {
    totalRequests: compactInteger(requests),
    totalRows: requests,
    totalTokens: compactInteger(bucketTotalTokens(bucket)),
    estimatedCost: formatCostSummary(bucket),
    averageDuration: formatAverageMs(bucket.duration_ms_total, bucket.requests_total),
    averageFirstToken: formatAverageMs(bucket.ttfb_ms_total, bucket.ttfb_samples),
    cacheRate: "—",
    errorRate: formatRatio(bucket.requests_error, bucket.requests_total),
    dayLabel: usageDay.label || "today",
  };
}

function providerRoutingSummary(providers: ProviderCardView[], summary: ApiOperatorSummary) {
  const routable = providers.filter((provider) => provider.routableEndpoints > 0).length;
  const total = providers.length || summary.counts.providers;
  const routableEndpoints = providers.reduce(
    (count, provider) => count + provider.routableEndpoints,
    0,
  );
  const endpointCount = providers.reduce(
    (count, provider) => count + provider.endpointCount,
    0,
  );

  return {
    value: `${routable}/${total}`,
    note: `${routableEndpoints}/${endpointCount} endpoints routable`,
    tone: routable < total ? ("warning" as DashboardMetricTone) : ("success" as DashboardMetricTone),
  };
}

function capacitySummary(capacity?: ApiOperatorProviderCapacity): string | undefined {
  if (!capacity) {
    return undefined;
  }
  const parts: string[] = [];
  if (capacity.active !== undefined && capacity.limit !== undefined) {
    parts.push(`active ${capacity.active}/${capacity.limit}`);
  } else if (capacity.limit !== undefined) {
    parts.push(`limit ${capacity.limit}`);
  }
  if (capacity.configured_max_concurrent_requests !== undefined) {
    parts.push(`configured ${capacity.configured_max_concurrent_requests}`);
  }
  if (capacity.effective_max_concurrent_requests !== undefined) {
    parts.push(`effective ${capacity.effective_max_concurrent_requests}`);
  }
  if (capacity.inherited_from_provider) {
    parts.push("inherited");
  }
  if (capacity.saturated) {
    parts.push("saturated");
  }
  return parts.length > 0 ? parts.join(" · ") : undefined;
}

function requestStatus(statusCode: number): RecentRequestView["status"] {
  if (statusCode >= 500) {
    return "error";
  }
  if (statusCode >= 400) {
    return "warn";
  }
  return "ok";
}

function requestTokensLabel(usage?: ApiUsageMetrics) {
  if (!usage) {
    return "—";
  }
  return `${compactInteger(positive(usage.input_tokens))} / ${compactInteger(positive(usage.output_tokens))}`;
}

function providerControlSummary(request: ApiOperatorRequestSummary): string | undefined {
  const signals = [
    ...(request.provider_signal_codes ?? []),
    ...(request.retry?.route_attempts ?? []).flatMap((attempt) => attempt.provider_signal_codes ?? []),
  ];
  const actions = [
    ...(request.policy_action_codes ?? []),
    ...(request.retry?.route_attempts ?? []).flatMap((attempt) => attempt.policy_action_codes ?? []),
  ];

  const signal = firstUnique(signals)[0];
  const action = firstUnique(actions)[0];
  if (!signal && !action) {
    return undefined;
  }
  return [signal ? `signal ${signal}` : undefined, action ? `action ${action}` : undefined]
    .filter(Boolean)
    .join(" · ");
}

function firstUnique(items: string[]): string[] {
  return items.filter((item, index) => items.indexOf(item) === index);
}

function usageChartBars(usageDay: ApiUsageDayView) {
  return mapUsageHours(usageDay).map((row) => row.height);
}

function mapUsageHours(usageDay?: ApiUsageDayView): UsageHourView[] {
  const rows = new Map((usageDay?.hourly ?? []).map((row) => [row.hour, row.bucket]));
  const values = Array.from({ length: 24 }, (_, hour) => bucketTotalTokens(rows.get(hour)));
  const max = Math.max(...values, 1);
  return Array.from({ length: 24 }, (_, hour) => {
    const bucket = rows.get(hour);
    const totalTokens = bucketTotalTokens(bucket);
    return {
      hour,
      label: `${hour.toString().padStart(2, "0")}:00`,
      requests: positive(bucket?.requests_total),
      totalTokens,
      cost: formatCostSummary(bucket),
      height: totalTokens > 0 ? Math.max(8, Math.round((totalTokens / max) * 100)) : 0,
    };
  });
}

function mapUsageDimensionRows(rows: ApiUsageDayDimensionRow[] | undefined): UsageDimensionRowView[] {
  return (rows ?? []).slice(0, 8).map((row) => ({
    name: row.name,
    requests: positive(row.bucket.requests_total),
    totalTokens: compactInteger(bucketTotalTokens(row.bucket)),
    cost: formatCostSummary(row.bucket),
    averageDuration: formatAverageMs(row.bucket.duration_ms_total, row.bucket.requests_total),
    errorRate: formatRatio(row.bucket.requests_error, row.bucket.requests_total),
  }));
}

function formatCostBreakdown(cost?: ApiCostBreakdown): UsageRowView["costBreakdown"] {
  return {
    input: cost?.input_cost_usd ? `$${cost.input_cost_usd}` : "—",
    output: cost?.output_cost_usd ? `$${cost.output_cost_usd}` : "—",
    cacheRead: cost?.cache_read_cost_usd ? `$${cost.cache_read_cost_usd}` : "—",
    cacheCreation: cost?.cache_creation_cost_usd ? `$${cost.cache_creation_cost_usd}` : "—",
    serviceTierMultiplier: cost?.service_tier_multiplier ?? "—",
    providerMultiplier: cost?.provider_cost_multiplier ?? "—",
    confidence: cost?.confidence ?? "unknown",
    source: cost?.pricing_source ?? "unknown",
  };
}

function formatCost(cost?: ApiCostBreakdown) {
  return cost?.total_cost_usd ? `$${cost.total_cost_usd}` : "unknown";
}

function formatCostSummary(bucket?: ApiUsageBucket) {
  const value = bucket?.cost?.total_cost_usd;
  if (!value) {
    return "unknown";
  }
  return value.startsWith("$") ? value : `$${value}`;
}

function bucketTotalTokens(bucket?: ApiUsageBucket) {
  return positive(bucket?.usage?.total_tokens);
}

function formatAverageMs(total: number | undefined, count: number | undefined) {
  const denominator = positive(count);
  if (denominator === 0) {
    return "—";
  }
  return formatMs(Math.round(positive(total) / denominator));
}

function formatRatio(numerator: number | undefined, denominator: number | undefined) {
  const bottom = positive(denominator);
  if (bottom === 0) {
    return "0%";
  }
  return `${Math.round((positive(numerator) / bottom) * 100)}%`;
}

function formatSeconds(value: number | undefined) {
  if (!value) {
    return "—";
  }
  if (value >= 3600) {
    return `${(value / 3600).toFixed(value >= 36_000 ? 0 : 1)}h`;
  }
  if (value >= 60) {
    return `${Math.round(value / 60)}m`;
  }
  return `${value}s`;
}

function positive(value: number | null | undefined) {
  return Math.max(value ?? 0, 0);
}

function formatMs(value: number) {
  if (value <= 0) {
    return "—";
  }
  if (value >= 1000) {
    return `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}s`;
  }
  return `${value}ms`;
}

function formatClock(ms: number) {
  if (!ms) {
    return "—";
  }
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(ms));
}

function formatRelativeMs(ms?: number | null) {
  if (!ms) {
    return "刚刚";
  }
  const diff = Date.now() - ms;
  if (diff < 60_000) {
    return "刚刚";
  }
  if (diff < 3_600_000) {
    return `${Math.round(diff / 60_000)} 分钟前`;
  }
  return `${Math.round(diff / 3_600_000)} 小时前`;
}
