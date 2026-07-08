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
  RecentRequestView,
  RuntimeSummary,
  UsageData,
  UsageDimensionRowView,
  UsageHourView,
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
  usageDay?: ApiUsageDayView;
  adminBaseUrl: string;
  appVersion: string;
}): DashboardData {
  const providers = mapProviders(input.providers ?? input.summary.providers ?? [], input.summary);
  const recentRequests = mapRecentRequests(input.recentRequests ?? []);
  const usageSummary = summarizeUsageDay(input.usageDay) ?? summarizeUsage(input.usageSummary ?? [], input.recentRequests ?? []);
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
        note: input.usageDay ? `usage_day ${usageSummary.dayLabel}` : "legacy request-ledger",
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
    chartBars: usageChartBars(input.usageDay, input.usageSummary ?? [], input.recentRequests ?? []),
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
  usageDay?: ApiUsageDayView;
}): UsageData {
  const rows = mapUsageRows(input.recentRequests ?? []);
  const usageDay = input.usageDay;
  return {
    summary: summarizeUsageDay(usageDay) ?? summarizeUsage(input.usageSummary ?? [], input.recentRequests ?? []),
    hourly: mapUsageHours(usageDay),
    providerRows: mapUsageDimensionRows(usageDay?.provider_rows),
    stationRows: mapUsageDimensionRows(usageDay?.station_rows),
    modelRows: mapUsageDimensionRows(usageDay?.model_rows),
    sessionRows: mapUsageDimensionRows(usageDay?.session_rows),
    projectRows: mapUsageDimensionRows(usageDay?.project_rows),
    coverage: {
      source: usageDay?.coverage?.source ?? "unavailable",
      isPartial: Boolean(usageDay?.coverage?.day_may_be_partial),
      reason: usageDay?.coverage?.partial_reason ?? undefined,
      loadedRequests: positive(usageDay?.coverage?.loaded_requests),
      scannedLines: positive(usageDay?.coverage?.scanned_lines),
      truncated: Boolean(usageDay?.coverage?.bytes_truncated || usageDay?.coverage?.lines_truncated),
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
    ownerMode: "unknown",
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
    const endpointCount = endpoints.length;
    const editable = endpointCount === 1 && Boolean(primaryEndpoint?.base_url);
    const policyAction = firstActivePolicyAction(provider);

    return {
      id: provider.name,
      name: provider.alias || provider.name,
      alias: provider.alias ?? null,
      baseUrl: primaryEndpoint?.base_url ?? "",
      continuityDomain: primaryEndpoint?.effective_continuity_domain ?? primaryEndpoint?.continuity_domain ?? null,
      host: hostFromUrl(primaryEndpoint?.base_url),
      enabled: provider.configured_enabled ?? true,
      endpointCount,
      endpointName: primaryEndpoint?.name,
      editable,
      editBlockedReason: providerEditBlockedReason(endpointCount, primaryEndpoint?.base_url),
      auth: "本机配置 / 环境变量",
      balance: "unknown",
      health,
      latency: provider.routable_endpoints ? "可路由" : "—",
      capabilities: providerCapabilities(provider),
      usage: `${provider.routable_endpoints ?? 0}/${endpoints.length} endpoints`,
      lastUsed: policyAction?.reason ? `policy ${policyAction.reason}` : active ? "当前路由" : "等待请求",
      active,
    };
  });
}

function providerEditBlockedReason(endpointCount: number, baseUrl?: string) {
  if (endpointCount > 1) {
    return "多 endpoint provider 暂不提供常用表单，请用 raw TOML 编辑高级路由。";
  }
  if (!baseUrl) {
    return "当前 provider 没有可安全编辑的 base_url，请用 raw TOML 补全。";
  }
  return undefined;
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
    providerControl: providerControlSummary(request),
  }));
}

function firstActivePolicyAction(provider: ApiProviderOption) {
  return (provider.endpoints ?? [])
    .flatMap((endpoint) => endpoint.policy_actions ?? [])
    .find((action) => action.active_cooldown);
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
    cacheRate: "—",
    errorRate: "—",
    dayLabel: "recent",
  };
}

function summarizeUsageDay(usageDay?: ApiUsageDayView): UsageSummaryView | undefined {
  if (!usageDay?.summary) {
    return undefined;
  }
  const bucket = usageDay.summary;
  const requests = positive(bucket.requests_total);
  return {
    totalRequests: compactInteger(requests),
    totalRows: requests,
    totalTokens: compactInteger(bucketTotalTokens(bucket)),
    estimatedCost: formatCostSummary(bucket),
    averageDuration: formatAverageMs(bucket.duration_ms_total, bucket.requests_total),
    averageFirstToken: formatAverageMs(bucket.ttfb_ms_total, bucket.ttfb_samples),
    cacheRate: formatBucketCacheRate(bucket),
    errorRate: formatRatio(bucket.requests_error, bucket.requests_total),
    dayLabel: usageDay.label || "today",
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
  if (endpoints.some((endpoint) => (endpoint.policy_actions ?? []).some((action) => action.active_cooldown))) {
    return "Warning";
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
    if (endpoint.effective_continuity_domain || endpoint.continuity_domain) {
      capabilities.add("continuity domain");
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

function providerControlSummary(request: ApiFinishedRequest): string | undefined {
  const signals = [
    ...(request.provider_signals ?? []),
    ...(request.retry?.route_attempts ?? []).flatMap((attempt) => attempt.provider_signals ?? []),
  ]
    .map((signal) => signal.code ?? signal.kind)
    .filter((kind): kind is string => Boolean(kind));
  const actions = [
    ...(request.policy_actions ?? []),
    ...(request.retry?.route_attempts ?? []).flatMap((attempt) => attempt.policy_actions ?? []),
  ]
    .map((action) => action.code ?? action.kind)
    .filter((kind): kind is string => Boolean(kind));

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

function usageChartBars(
  usageDay: ApiUsageDayView | undefined,
  summaryRows: ApiRequestUsageSummaryRow[],
  recentRequests: ApiFinishedRequest[],
) {
  if (usageDay?.hourly?.length) {
    return mapUsageHours(usageDay).map((row) => row.height);
  }

  const values = summaryRows.length > 0
    ? summaryRows.slice(0, 12).map((row) => row.aggregate.total_tokens ?? 0)
    : recentRequests.slice(0, 12).map((request) => totalTokens(request.usage, request.service));

  if (values.length === 0) {
    return [8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8];
  }

  const max = Math.max(...values, 1);
  return values.map((value) => Math.max(8, Math.round((value / max) * 100)));
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
    serviceTierMultiplier: cost?.service_tier_multiplier ?? "1.0x",
    providerMultiplier: cost?.provider_cost_multiplier ?? "1.0x",
    confidence: cost?.confidence ?? "unknown",
    source: cost?.pricing_source ?? "operator catalog",
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
  if (!bucket?.usage) {
    return 0;
  }
  return totalTokens(bucket.usage, "codex");
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

function formatBucketCacheRate(bucket: ApiUsageBucket) {
  return formatCacheRate(bucket.usage, "codex");
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
