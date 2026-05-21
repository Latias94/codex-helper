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
export const mockProviderCardViews: ProviderCardView[] = mockProviders.map((provider) => ({
  ...provider,
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

export const mockUsageRowViews: UsageRowView[] = mockUsageRows.map((row) => ({
  ...row,
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
  },
  rows: mockUsageRowViews,
};
