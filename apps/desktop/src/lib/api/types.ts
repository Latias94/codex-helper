export type RuntimeMode = "running" | "attached" | "stopped" | "unavailable";

export type ProviderHealth = "healthy" | "warning" | "error" | "unknown";

export type CostEstimate = {
  amount: string;
  disclaimer: string;
};

export type DataSource = "live" | "mock";

export type RuntimeDataStatus =
  | "loading"
  | "live"
  | "refreshing"
  | "mock"
  | "unavailable"
  | "disconnected"
  | "auth-required"
  | "empty"
  | "stale";

export type RuntimeOwnerMode = "desktop-owned" | "attached" | "unknown";
export type RuntimeDataSeverity = "neutral" | "info" | "success" | "warning" | "danger";

export type DesktopRuntimeConnectionMode = "desktop-owned" | "attached" | "stopped" | "unknown";

export type CodexSwitchSnapshot = {
  enabled: boolean;
  modelProvider?: string | null;
  providerName?: string | null;
  baseUrl?: string | null;
  preset?: string | null;
  requiresOpenaiAuth?: boolean | null;
  supportsWebsockets?: boolean | null;
  remoteCompactionV2Enabled: boolean;
  hasSwitchState: boolean;
  errorMessage?: string | null;
};

export type DesktopControlState = {
  connectionMode: DesktopRuntimeConnectionMode;
  proxyPort: number;
  adminPort: number;
  proxyBaseUrl: string;
  adminBaseUrl: string;
  reachable: boolean;
  shutdownAvailable: boolean;
  owner?: unknown;
  codexSwitch: CodexSwitchSnapshot;
  canStart: boolean;
  canAttach: boolean;
  canStopOwned: boolean;
  canRemoteStop: boolean;
  canSwitchOn: boolean;
  canSwitchOff: boolean;
};

export type DesktopActionResult = {
  ok: boolean;
  action: string;
  message: string;
  state?: DesktopControlState;
  payload?: unknown;
};

export type RuntimeDataState = {
  status: RuntimeDataStatus;
  source: DataSource;
  severity: RuntimeDataSeverity;
  title: string;
  description: string;
  badge: string;
  canUseLiveActions: boolean;
  canStartProxy: boolean;
  canAttachProxy: boolean;
  canStopProxy: boolean;
  isFallback: boolean;
  isStale: boolean;
  ownerMode: RuntimeOwnerMode;
  lastUpdatedAt?: number;
  errorCode?: string;
  errorMessage?: string;
};

export type RuntimeSummary = {
  mode: RuntimeMode;
  ownerMode: RuntimeOwnerMode;
  proxy: string;
  port: number;
  adminPort: number;
  codex: string;
  provider: string;
  balance: string;
  version: string;
  endpoint: string;
  adminEndpoint: string;
  updatedAtLabel: string;
};

export type DashboardMetricTone = "success" | "warning" | "teal" | "blue" | "default";

export type DashboardMetric = {
  label: string;
  value: string;
  note: string;
  tone: DashboardMetricTone;
};

export type RecentRequestView = {
  id: string;
  model: string;
  status: "ok" | "warn" | "error";
  provider: string;
  tokens: string;
  cost: string;
  duration: string;
  time: string;
  providerControl?: string;
};

export type ProviderCardView = {
  id?: string;
  name: string;
  alias?: string | null;
  baseUrl: string;
  continuityDomain?: string | null;
  host: string;
  enabled: boolean;
  endpointCount: number;
  endpointName?: string;
  editable: boolean;
  editBlockedReason?: string;
  auth: string;
  balance: string;
  health: "Healthy" | "Warning" | "Error" | "Unknown";
  latency: string;
  capabilities: string[];
  usage: string;
  lastUsed: string;
  active: boolean;
};

export type UsageRowView = {
  id: string;
  requestId: number;
  traceId?: string;
  sessionId?: string;
  provider: string;
  key: string;
  model: string;
  effort: string;
  endpoint: string;
  type: string;
  billing: string;
  tokens: {
    input: number;
    output: number;
    cache: string;
  };
  cost: string;
  costBreakdown: {
    input: string;
    output: string;
    cacheRead: string;
    cacheCreation: string;
    serviceTierMultiplier: string;
    providerMultiplier: string;
    confidence: string;
    source: string;
  };
  firstToken: string;
  duration: string;
  time: string;
};

export type UsageSummaryView = {
  totalRequests: string;
  totalRows: number;
  totalTokens: string;
  estimatedCost: string;
  averageDuration: string;
  averageFirstToken: string;
  cacheRate: string;
  errorRate: string;
  dayLabel: string;
};

export type UsageHourView = {
  hour: number;
  label: string;
  requests: number;
  totalTokens: number;
  cost: string;
  height: number;
};

export type UsageDimensionRowView = {
  name: string;
  requests: number;
  totalTokens: string;
  cost: string;
  averageDuration: string;
  errorRate: string;
};

export type UsageCoverageView = {
  source: string;
  isPartial: boolean;
  reason?: string;
  loadedRequests: number;
  scannedLines: number;
  truncated: boolean;
};

export type UsageRetryGateView = {
  active: number;
  activeCooldowns: number;
  maxRemaining: string;
  reasons: Array<{ reason: string; active: number }>;
};

export type DashboardData = {
  runtime: RuntimeSummary;
  metrics: DashboardMetric[];
  recentRequests: RecentRequestView[];
  providers: ProviderCardView[];
  chartBars: number[];
};

export type ProvidersData = {
  providers: ProviderCardView[];
  routeOrder: ProviderCardView[];
};

export type UsageData = {
  summary: UsageSummaryView;
  hourly: UsageHourView[];
  providerRows: UsageDimensionRowView[];
  stationRows: UsageDimensionRowView[];
  modelRows: UsageDimensionRowView[];
  sessionRows: UsageDimensionRowView[];
  projectRows: UsageDimensionRowView[];
  coverage: UsageCoverageView;
  retryGate: UsageRetryGateView;
  rows: UsageRowView[];
};

export type QueryBackedData<T> = {
  data: T;
  source: DataSource;
  state: RuntimeDataState;
  isLoading: boolean;
  isRefreshing: boolean;
  errorMessage?: string;
  refetch: () => void;
};
