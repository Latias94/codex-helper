import {
  BadgeDollarSign,
  Cable,
  Clock3,
  Database,
  Server,
  ShieldCheck,
  Zap,
  type LucideIcon,
} from "lucide-react";

import {
  DEFAULT_PROXY_PORT,
  adminPortForProxyPort,
  proxyBaseUrlForAdminBaseUrl,
} from "@/lib/api/admin-client";
import type {
  ApiCostBreakdown,
  ApiFinishedRequest,
  ApiOperatorSummary,
  ApiProviderOption,
  ApiRequestUsageSummaryRow,
  ApiRuntimeStatus,
  ApiUsageMetrics,
} from "@/lib/api/admin-types";
import type {
  DashboardData,
  DashboardMetric,
  DashboardMetricTone,
  ProviderCardView,
  RecentRequestView,
  RuntimeSummary,
  UsageData,
  UsageRowView,
  UsageSummaryView,
} from "@/lib/api/types";
import { compactInteger } from "@/lib/format/number";

type IconMetric = DashboardMetric & { icon: LucideIcon };

export const metricIconByLabel: Record<string, LucideIcon> = {
  本地代理: Server,
  "Codex 连接": Cable,
  当前供应商: Database,
  今日请求: Database,
  "今日 Tokens": Zap,
  预估花费: BadgeDollarSign,
  平均响应: Clock3,
  "Provider Health": ShieldCheck,
};

export function attachMetricIcons(metrics: DashboardMetric[]): IconMetric[] {
  return metrics.map((metric) => ({
    ...metric,
    icon: metricIconByLabel[metric.label] ?? Database,
  }));
}

export function mapAdminDashboardData(input: {
  summary: ApiOperatorSummary;
  runtimeStatus?: ApiRuntimeStatus;
  providers?: ApiProviderOption[];
  recentRequests?: ApiFinishedRequest[];
  usageSummary?: ApiRequestUsageSummaryRow[];
  adminBaseUrl: string;
  appVersion: string;
}): DashboardData {
  const providers = mapProviders(input.providers ?? input.summary.providers ?? [], input.summary);
  const recentRequests = mapRecentRequests(input.recentRequests ?? []);
  const usageSummary = summarizeUsage(input.usageSummary ?? [], input.recentRequests ?? []);
  const runtime = mapRuntimeSummary(input.summary, {
    adminBaseUrl: input.adminBaseUrl,
    appVersion: input.appVersion,
    runtimeStatus: input.runtimeStatus,
    recentRequests: input.recentRequests,
  });
  const providerHealth = providerHealthSummary(providers, input.summary);

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
        note: input.summary.runtime.default_profile ?? input.summary.service_name ?? "local",
        tone: "success",
      },
      {
        label: "当前供应商",
        value: runtime.provider,
        note: `Provider ${input.summary.counts.providers ?? providers.length}`,
        tone: "teal",
      },
      {
        label: "今日请求",
        value: usageSummary.totalRequests,
        note: "request-ledger recent",
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
        label: "Provider Health",
        value: providerHealth.value,
        note: providerHealth.note,
        tone: providerHealth.tone,
      },
    ],
    recentRequests,
    providers,
    chartBars: usageChartBars(input.usageSummary ?? [], input.recentRequests ?? []),
  };
}

export function mapProvidersData(
  summary: ApiOperatorSummary,
  providersPayload?: ApiProviderOption[],
  recentRequests?: ApiFinishedRequest[],
) {
  const providers = mapProviders(providersPayload ?? summary.providers ?? [], summary, recentRequests);
  return {
    providers,
    routeOrder: [...providers].sort((left, right) => Number(right.active) - Number(left.active)),
  };
}

export function mapUsageData(input: {
  recentRequests?: ApiFinishedRequest[];
  usageSummary?: ApiRequestUsageSummaryRow[];
}): UsageData {
  const rows = mapUsageRows(input.recentRequests ?? []);
  return {
    summary: summarizeUsage(input.usageSummary ?? [], input.recentRequests ?? []),
    rows,
  };
}

export function mapRuntimeSummary(
  summary: ApiOperatorSummary,
  options: {
    adminBaseUrl: string;
    appVersion: string;
    runtimeStatus?: ApiRuntimeStatus;
    recentRequests?: ApiFinishedRequest[];
  },
): RuntimeSummary {
  const adminUrl = new URL(options.adminBaseUrl);
  const adminPort = Number(adminUrl.port) || adminPortForProxyPort(DEFAULT_PROXY_PORT);
  const proxyBaseUrl = proxyBaseUrlForAdminBaseUrl(options.adminBaseUrl);
  const proxyPort = Number(new URL(proxyBaseUrl).port) || DEFAULT_PROXY_PORT;
  const providerNames = new Set(summary.providers?.map((item) => item.name));
  const stationCandidate =
    summary.runtime.default_profile_summary?.station ??
    summary.runtime.effective_active_station ??
    summary.runtime.configured_active_station;
  const recentProvider = options.recentRequests?.find((request) => request.provider_id)?.provider_id;
  const provider =
    (stationCandidate && providerNames.has(stationCandidate) ? stationCandidate : undefined) ??
    recentProvider ??
    stationCandidate ??
    summary.providers?.find((item) => item.effective_enabled)?.name ??
    summary.providers?.[0]?.name ??
    "未配置";

  return {
    mode: "running",
    proxy: "Running",
    port: proxyPort,
    adminPort,
    codex: "已连接",
    provider,
    balance: "预估",
    version: `v${options.appVersion}`,
    endpoint: proxyBaseUrl,
    adminEndpoint: options.adminBaseUrl,
    updatedAtLabel: formatRelativeMs(options.runtimeStatus?.loaded_at_ms ?? summary.runtime.runtime_loaded_at_ms),
  };
}

export function mapProviders(
  providers: ApiProviderOption[],
  summary?: ApiOperatorSummary,
  recentRequests?: ApiFinishedRequest[],
): ProviderCardView[] {
  const providerNames = new Set(providers.map((provider) => provider.name));
  const stationCandidate =
    summary?.runtime.default_profile_summary?.station ??
    summary?.runtime.effective_active_station ??
    summary?.runtime.configured_active_station;
  const recentProvider = recentRequests?.find((request) => request.provider_id)?.provider_id;
  const activeProvider =
    (stationCandidate && providerNames.has(stationCandidate) ? stationCandidate : undefined) ??
    recentProvider ??
    providers.find((provider) => provider.effective_enabled)?.name;

  return providers.map((provider, index) => {
    const endpoints = provider.endpoints ?? [];
    const primaryEndpoint =
      endpoints.find((endpoint) => endpoint.routable) ??
      endpoints.find((endpoint) => endpoint.effective_enabled) ??
      endpoints[0];
    const health = providerHealth(provider);
    const active = provider.name === activeProvider || (!activeProvider && index === 0);

    return {
      name: provider.alias || provider.name,
      host: hostFromUrl(primaryEndpoint?.base_url),
      auth: "本机配置 / 环境变量",
      balance: "unknown",
      health,
      latency: provider.routable_endpoints ? "可路由" : "—",
      capabilities: providerCapabilities(provider),
      usage: `${provider.routable_endpoints ?? 0}/${endpoints.length} endpoints`,
      lastUsed: active ? "当前路由" : "等待请求",
      active,
    };
  });
}

export function mapRecentRequests(requests: ApiFinishedRequest[]): RecentRequestView[] {
  return requests.slice(0, 5).map((request) => ({
    id: String(request.trace_id ?? request.id),
    model: request.model ?? "unknown",
    status: requestStatus(request.status_code),
    provider: request.provider_id ?? request.station_name ?? "-",
    tokens: requestTokensLabel(request.usage),
    cost: formatCost(request.cost),
    duration: formatMs(request.duration_ms),
    time: formatClock(request.ended_at_ms),
  }));
}

export function mapUsageRows(requests: ApiFinishedRequest[]): UsageRowView[] {
  return requests.map((request) => {
    const usage = request.usage;
    return {
      id: String(request.trace_id ?? request.id),
      provider: request.provider_id ?? request.station_name ?? "-",
      key: request.provider_id ? `provider ${request.provider_id}` : "local config",
      model: request.model ?? "unknown",
      effort: request.reasoning_effort ?? request.service_tier ?? "—",
      endpoint: request.path,
      type: request.streaming || request.observability?.streaming ? "流式" : "同步",
      billing: request.cost?.confidence === "unknown" ? "估算" : "按量",
      tokens: {
        input: positive(usage?.input_tokens),
        output: positive(usage?.output_tokens),
        cache: formatCacheRate(usage, request.service),
      },
      cost: formatCost(request.cost),
      costBreakdown: formatCostBreakdown(request.cost),
      firstToken: request.ttfb_ms ? formatMs(request.ttfb_ms) : "—",
      duration: formatMs(request.duration_ms),
      time: formatClock(request.ended_at_ms),
    };
  });
}

export function summarizeUsage(
  summaryRows: ApiRequestUsageSummaryRow[],
  recentRequests: ApiFinishedRequest[],
): UsageSummaryView {
  const summaryAggregate = summaryRows.reduce(
    (acc, row) => {
      const aggregate = row.aggregate;
      acc.requests += aggregate.requests ?? 0;
      acc.totalTokens += aggregate.total_tokens ?? 0;
      acc.durationMs += aggregate.duration_ms_total ?? 0;
      return acc;
    },
    { requests: 0, totalTokens: 0, durationMs: 0 },
  );

  const recentAggregate = recentRequests.reduce(
    (acc, request) => {
      acc.requests += 1;
      acc.totalTokens += totalTokens(request.usage, request.service);
      acc.durationMs += request.duration_ms;
      acc.ttfbMs += request.ttfb_ms ?? 0;
      if (request.ttfb_ms) {
        acc.ttfbCount += 1;
      }
      acc.cost += Number(request.cost?.total_cost_usd ?? 0);
      return acc;
    },
    { requests: 0, totalTokens: 0, durationMs: 0, ttfbMs: 0, ttfbCount: 0, cost: 0 },
  );

  const requests = summaryAggregate.requests || recentAggregate.requests;
  const tokens = summaryAggregate.totalTokens || recentAggregate.totalTokens;
  const durationMs = summaryAggregate.durationMs || recentAggregate.durationMs;

  return {
    totalRequests: compactInteger(requests),
    totalRows: requests,
    totalTokens: compactInteger(tokens),
    estimatedCost: recentAggregate.cost > 0 ? `$${recentAggregate.cost.toFixed(4)}` : "unknown",
    averageDuration: formatMs(requests > 0 ? Math.round(durationMs / requests) : 0),
    averageFirstToken:
      recentAggregate.ttfbCount > 0
        ? formatMs(Math.round(recentAggregate.ttfbMs / recentAggregate.ttfbCount))
        : "—",
  };
}

function providerHealth(provider: ApiProviderOption): ProviderCardView["health"] {
  const endpoints = provider.endpoints ?? [];
  if (!provider.effective_enabled) {
    return "Warning";
  }
  if (endpoints.some((endpoint) => endpoint.runtime_state === "breaker_open")) {
    return "Error";
  }
  if (endpoints.some((endpoint) => endpoint.runtime_state === "draining" || endpoint.runtime_state === "half_open")) {
    return "Warning";
  }
  if ((provider.routable_endpoints ?? 0) > 0) {
    return "Healthy";
  }
  return "Unknown";
}

function providerCapabilities(provider: ApiProviderOption) {
  const capabilities = new Set<string>();
  for (const endpoint of provider.endpoints ?? []) {
    const text = `${endpoint.name} ${endpoint.base_url}`.toLowerCase();
    capabilities.add("responses");
    if (text.includes("compact")) {
      capabilities.add("compact");
    }
    if (text.includes("image") || text.includes("img")) {
      capabilities.add("imagegen");
    }
  }
  if (capabilities.size === 0) {
    capabilities.add("responses");
  }
  return [...capabilities];
}

function providerHealthSummary(providers: ProviderCardView[], summary: ApiOperatorSummary) {
  const healthy = providers.filter((provider) => provider.health === "Healthy").length;
  const total = providers.length || summary.counts.providers || 0;
  const warningCount =
    (summary.health?.stations_breaker_open ?? 0) +
    (summary.health?.stations_with_probe_failures ?? 0) +
    (summary.health?.stations_with_usage_exhaustion ?? 0);

  return {
    value: `${healthy}/${total}`,
    note: warningCount > 0 ? `${warningCount} 个状态需要关注` : "全部可路由",
    tone: warningCount > 0 || healthy < total ? ("warning" as DashboardMetricTone) : ("success" as DashboardMetricTone),
  };
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

function usageChartBars(summaryRows: ApiRequestUsageSummaryRow[], recentRequests: ApiFinishedRequest[]) {
  const values = summaryRows.length > 0
    ? summaryRows.slice(0, 12).map((row) => row.aggregate.total_tokens ?? 0)
    : recentRequests.slice(0, 12).map((request) => totalTokens(request.usage, request.service));

  if (values.length === 0) {
    return [8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8];
  }

  const max = Math.max(...values, 1);
  return values.map((value) => Math.max(8, Math.round((value / max) * 100)));
}

function formatCostBreakdown(cost?: ApiCostBreakdown): UsageRowView["costBreakdown"] {
  return {
    input: cost?.input_cost_usd ? `$${cost.input_cost_usd}` : "—",
    output: cost?.output_cost_usd ? `$${cost.output_cost_usd}` : "—",
    cacheRead: cost?.cache_read_cost_usd ? `$${cost.cache_read_cost_usd}` : "—",
    cacheCreation: cost?.cache_creation_cost_usd ? `$${cost.cache_creation_cost_usd}` : "—",
    serviceTierMultiplier: cost?.service_tier_multiplier ?? "1.0x",
    providerMultiplier: cost?.provider_cost_multiplier ?? "1.0x",
    confidence: cost?.confidence ?? "unknown",
    source: cost?.pricing_source ?? "operator catalog",
  };
}

function formatCost(cost?: ApiCostBreakdown) {
  return cost?.total_cost_usd ? `$${cost.total_cost_usd}` : "unknown";
}

function formatCacheRate(usage: ApiUsageMetrics | undefined, service: string) {
  if (!usage) {
    return "—";
  }
  const read = positive(usage.cached_input_tokens) + positive(usage.cache_read_input_tokens);
  const create =
    positive(usage.cache_creation_input_tokens) +
    positive(usage.cache_creation_5m_input_tokens) +
    positive(usage.cache_creation_1h_input_tokens);
  const input = Math.max(positive(usage.input_tokens) - (service === "codex" ? read : 0), 0);
  const denominator = input + read + create;
  if (denominator === 0) {
    return "0%";
  }
  return `${Math.round((read / denominator) * 100)}%`;
}

function totalTokens(usage: ApiUsageMetrics | undefined, service: string) {
  if (!usage) {
    return 0;
  }
  if (positive(usage.total_tokens) > 0) {
    return positive(usage.total_tokens);
  }
  const read = positive(usage.cached_input_tokens) + positive(usage.cache_read_input_tokens);
  const create =
    positive(usage.cache_creation_input_tokens) +
    positive(usage.cache_creation_5m_input_tokens) +
    positive(usage.cache_creation_1h_input_tokens);
  const input = Math.max(positive(usage.input_tokens) - (service === "codex" ? read : 0), 0);
  return input + positive(usage.output_tokens) + read + create;
}

function hostFromUrl(value?: string) {
  if (!value) {
    return "not configured";
  }
  try {
    return new URL(value).host;
  } catch {
    return value;
  }
}

function positive(value: number | undefined) {
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
