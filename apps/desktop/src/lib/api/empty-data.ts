import type {
  DashboardData,
  ProvidersData,
  RuntimeOwnerMode,
  RuntimeSummary,
  UsageData,
} from "@/lib/api/types";
import type { AdminEndpointConfig } from "@/lib/tauri/commands";

export function emptyRuntimeSummary(
  endpoint: AdminEndpointConfig | undefined,
  appVersion: string,
  ownerMode: RuntimeOwnerMode = "unknown",
): RuntimeSummary {
  return {
    mode: "unavailable",
    ownerMode,
    proxy: "未连接",
    port: endpoint?.proxyPort ?? 0,
    adminPort: endpoint?.adminPort ?? 0,
    codex: "未连接",
    provider: "—",
    balance: "—",
    version: `v${appVersion}`,
    endpoint: endpoint?.proxyBaseUrl ?? "—",
    adminEndpoint: endpoint?.adminBaseUrl ?? "—",
    updatedAtLabel: "—",
  };
}

export function emptyDashboardData(runtime: RuntimeSummary): DashboardData {
  return {
    runtime,
    metrics: [
      { label: "本地代理", value: "—", note: "无运行时事实", tone: "warning" },
      { label: "Codex 连接", value: "—", note: "无运行时事实", tone: "default" },
      { label: "最近供应商", value: "—", note: "无运行时事实", tone: "default" },
      { label: "今日请求", value: "—", note: "无运行时事实", tone: "default" },
      { label: "今日 Tokens", value: "—", note: "无运行时事实", tone: "default" },
      { label: "预估花费", value: "—", note: "无运行时事实", tone: "default" },
      { label: "平均响应", value: "—", note: "无运行时事实", tone: "default" },
      { label: "Provider Routing", value: "—", note: "无运行时事实", tone: "default" },
    ],
    recentRequests: [],
    providers: [],
    chartBars: [],
  };
}

export const emptyProvidersData: ProvidersData = {
  providers: [],
};

export const emptyUsageData: UsageData = {
  summary: {
    totalRequests: "—",
    totalRows: 0,
    totalTokens: "—",
    estimatedCost: "—",
    averageDuration: "—",
    averageFirstToken: "—",
    cacheRate: "—",
    errorRate: "—",
    dayLabel: "—",
  },
  hourly: [],
  providerRows: [],
  providerEndpointRows: [],
  modelRows: [],
  sessionRows: [],
  projectRows: [],
  coverage: {
    source: "unavailable",
    isPartial: false,
    loadedRequests: 0,
  },
  retryGate: {
    active: 0,
    activeCooldowns: 0,
    maxRemaining: "—",
    reasons: [],
  },
  rows: [],
};
