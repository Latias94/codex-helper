import {
  chartBars,
  metrics as mockMetrics,
  providers as mockProviders,
  recentRequests as mockRecentRequests,
  runtimeSummary as mockRuntimeSummary,
  usageRows as mockUsageRows,
} from "@/mocks/dashboard";
import type {
  DashboardData,
  DashboardMetric,
  ProviderCardView,
  ProvidersData,
  RecentRequestView,
  RuntimeSummary,
  UsageData,
  UsageRowView,
} from "@/lib/api/types";

export const mockRuntime: RuntimeSummary = {
  mode: "running",
  ownerMode: "unknown",
  proxy: mockRuntimeSummary.proxy,
  port: mockRuntimeSummary.port,
  adminPort: mockRuntimeSummary.port + 1000,
  codex: mockRuntimeSummary.codex,
  provider: mockRuntimeSummary.provider,
  balance: mockRuntimeSummary.balance,
  version: mockRuntimeSummary.version,
  endpoint: mockRuntimeSummary.endpoint,
  adminEndpoint: `http://127.0.0.1:${mockRuntimeSummary.port + 1000}`,
  updatedAtLabel: "示例数据",
};

export const mockProviderCards = mockProviders.map((provider) => ({ ...provider }));
export const mockProviderCardViews: ProviderCardView[] = mockProviders.map((provider, index) => ({
  id: provider.name.toLowerCase().replaceAll(" ", "-"),
  ...provider,
  alias: provider.name,
  baseUrl: `https://${provider.host}/v1`,
  enabled: true,
  endpointCount: 1,
  endpointName: "default",
  editable: true,
  health: provider.health as ProviderCardView["health"],
}));

export const mockRecentRequestViews: RecentRequestView[] = mockRecentRequests.map((request, index) => ({
  id: `mock-${index}`,
  ...request,
  status: request.status === "ok" ? "ok" : "warn",
}));

export const mockDashboardMetrics: DashboardMetric[] = mockMetrics.map(({ label, value, note, tone }) => ({
  label,
  value,
  note,
  tone,
}));

export const mockUsageRowViews: UsageRowView[] = mockUsageRows.map((row, index) => ({
  ...row,
  requestId: index + 1,
  traceId: `mock-trace-${index + 1}`,
  costBreakdown: {
    input: "$0.006",
    output: "$0.018",
    cacheRead: "$0.004",
    cacheCreation: "—",
    serviceTierMultiplier: "1.0x",
    providerMultiplier: "1.0x",
    confidence: "estimated",
    source: "mock",
  },
}));

export const mockDashboardData: DashboardData = {
  runtime: mockRuntime,
  metrics: mockDashboardMetrics,
  recentRequests: mockRecentRequestViews,
  providers: mockProviderCardViews,
  chartBars,
};

export const mockProvidersData: ProvidersData = {
  providers: mockProviderCardViews,
  routeOrder: mockProviderCardViews,
};

export const mockUsageData: UsageData = {
  summary: {
    totalRequests: "128",
    totalRows: 128,
    totalTokens: "1.84M",
    estimatedCost: "$0.42",
    averageDuration: "2.4s",
    averageFirstToken: "780ms",
    cacheRate: "42%",
    errorRate: "2%",
    dayLabel: "mock today",
  },
  hourly: Array.from({ length: 24 }, (_, hour) => ({
    hour,
    label: `${hour.toString().padStart(2, "0")}:00`,
    requests: hour % 3 === 0 ? 8 : 3,
    totalTokens: hour % 3 === 0 ? 92_000 : 28_000,
    cost: hour % 3 === 0 ? "$0.04" : "$0.01",
    height: hour % 3 === 0 ? 92 : 28,
  })),
  providerRows: [
    { name: "CodeX Air", requests: 82, totalTokens: "1.2M", cost: "$0.28", averageDuration: "2.1s", errorRate: "1%" },
    { name: "Backup", requests: 46, totalTokens: "640K", cost: "$0.14", averageDuration: "2.9s", errorRate: "4%" },
  ],
  stationRows: [
    { name: "input", requests: 128, totalTokens: "1.84M", cost: "$0.42", averageDuration: "2.4s", errorRate: "2%" },
  ],
  modelRows: [
    { name: "gpt-5.4", requests: 96, totalTokens: "1.4M", cost: "$0.31", averageDuration: "2.2s", errorRate: "1%" },
    { name: "gpt-5.4-mini", requests: 32, totalTokens: "440K", cost: "$0.11", averageDuration: "2.9s", errorRate: "3%" },
  ],
  sessionRows: [
    { name: "codex-session", requests: 44, totalTokens: "620K", cost: "$0.15", averageDuration: "2.5s", errorRate: "0%" },
  ],
  projectRows: [
    { name: "codex-helper", requests: 75, totalTokens: "1.1M", cost: "$0.24", averageDuration: "2.3s", errorRate: "1%" },
  ],
  coverage: {
    source: "mock",
    isPartial: false,
    loadedRequests: 128,
    scannedLines: 128,
    truncated: false,
  },
  retryGate: {
    active: 2,
    activeCooldowns: 1,
    maxRemaining: "4m",
    reasons: [{ reason: "upstream_rate_limited", active: 1 }],
  },
  rows: mockUsageRowViews,
};
