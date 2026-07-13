export type RuntimeMode = "running" | "attached" | "stopped" | "unavailable";

export type CostEstimate = {
  amount: string;
  disclaimer: string;
};

export type DataSource = "live" | "none";

export type RuntimeDataStatus =
  | "loading"
  | "live"
  | "refreshing"
  | "unavailable"
  | "disconnected"
  | "auth-required"
  | "empty"
  | "stale";

export type RuntimeOwnerMode = "desktop-owned" | "attached" | "unknown";
export type RuntimeDataSeverity = "neutral" | "info" | "success" | "warning" | "danger";

export type DesktopRuntimeConnectionMode = "desktop-owned" | "attached" | "stopped" | "unknown";
export type CodexSwitchPhase = "off" | "prepared" | "applied" | "recovery_required";

export type CodexSwitchSnapshot = {
  phase?: CodexSwitchPhase | null;
  enabled: boolean;
  managed: boolean;
  baseUrl?: string | null;
  recoveryReason?: string | null;
  errorMessage?: string | null;
};

export type DesktopControlState = {
  connectionMode: DesktopRuntimeConnectionMode;
  proxyPort: number;
  adminPort: number;
  proxyBaseUrl: string;
  adminBaseUrl: string;
  reachable: boolean;
  owner?: unknown;
  codexSwitch: CodexSwitchSnapshot;
  canStart: boolean;
  canAttach: boolean;
  canSwitchOn: boolean;
  canSwitchOff: boolean;
};

export type DesktopActionResult = {
  ok: boolean;
  action: string;
  message: string;
  state?: DesktopControlState;
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
  name: string;
  alias?: string;
  configuredEnabled: boolean;
  effectiveEnabled: boolean;
  routableEndpoints: number;
  endpointCount: number;
  capacity?: string;
  endpoints: ProviderEndpointInventoryView[];
  controlSummary: string;
  controlBadges: ProviderControlBadgeView[];
};

export type ProviderEndpointInventoryView = {
  key: string;
  name: string;
  origin: string;
  priority: number;
  configuredEnabled: boolean;
  effectiveEnabled: boolean;
  routable: boolean;
  runtimeState: string;
  capacity?: string;
  policyActionCount: number;
};

export type ProviderControlBadgeView = {
  key: string;
  label: string;
  detail: string;
  tone: "warning" | "teal" | "muted";
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
};

export type UsageData = {
  summary: UsageSummaryView;
  hourly: UsageHourView[];
  providerRows: UsageDimensionRowView[];
  providerEndpointRows: UsageDimensionRowView[];
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
