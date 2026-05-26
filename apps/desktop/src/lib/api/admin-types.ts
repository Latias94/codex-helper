export type ApiRuntimeConfigState = "normal" | "draining" | "breaker_open" | "half_open";

export type ApiCostConfidence = "unknown" | "partial" | "estimated" | "exact";

export type ApiUsageMetrics = {
  input_tokens?: number;
  output_tokens?: number;
  reasoning_tokens?: number;
  reasoning_output_tokens?: number;
  total_tokens?: number;
  cached_input_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
  cache_creation_5m_input_tokens?: number;
  cache_creation_1h_input_tokens?: number;
};

export type ApiCostBreakdown = {
  input_cost_usd?: string;
  output_cost_usd?: string;
  cache_read_cost_usd?: string;
  cache_creation_cost_usd?: string;
  service_tier_multiplier?: string;
  provider_cost_multiplier?: string;
  total_cost_usd?: string;
  confidence?: ApiCostConfidence | string;
  pricing_source?: string;
};

export type ApiProviderEndpointOption = {
  provider_name: string;
  name: string;
  base_url: string;
  continuity_domain?: string | null;
  effective_continuity_domain?: string | null;
  priority?: number;
  configured_enabled?: boolean;
  effective_enabled?: boolean;
  routable?: boolean;
  runtime_enabled_override?: boolean | null;
  runtime_state?: ApiRuntimeConfigState;
  runtime_state_override?: ApiRuntimeConfigState | null;
};

export type ApiProviderOption = {
  name: string;
  alias?: string | null;
  configured_enabled?: boolean;
  effective_enabled?: boolean;
  routable_endpoints?: number;
  endpoints?: ApiProviderEndpointOption[];
};

export type ApiStationCapabilitySummary = {
  model_catalog_kind?: string;
  supported_models?: string[];
  supports_service_tier?: string;
  supports_reasoning_effort?: string;
};

export type ApiStationOption = {
  name: string;
  alias?: string | null;
  enabled?: boolean;
  level?: number;
  configured_enabled?: boolean;
  configured_level?: number;
  runtime_enabled_override?: boolean | null;
  runtime_level_override?: number | null;
  runtime_state?: ApiRuntimeConfigState;
  runtime_state_override?: ApiRuntimeConfigState | null;
  capabilities?: ApiStationCapabilitySummary;
};

export type ApiOperatorRuntimeSummary = {
  runtime_loaded_at_ms?: number | null;
  runtime_source_mtime_ms?: number | null;
  configured_active_station?: string | null;
  effective_active_station?: string | null;
  global_station_override?: string | null;
  global_route_target_override?: string | null;
  configured_default_profile?: string | null;
  default_profile?: string | null;
  default_profile_summary?: {
    name: string;
    station?: string | null;
    model?: string | null;
    reasoning_effort?: string | null;
    service_tier?: string | null;
    fast_mode?: boolean;
  } | null;
};

export type ApiOperatorSummaryCounts = {
  active_requests?: number;
  recent_requests?: number;
  sessions?: number;
  stations?: number;
  profiles?: number;
  providers?: number;
};

export type ApiOperatorRetrySummary = {
  configured_profile?: string | null;
  supports_write?: boolean;
  upstream_max_attempts?: number;
  provider_max_attempts?: number;
  allow_cross_station_before_first_output?: boolean;
  recent_retried_requests?: number;
  recent_cross_station_failovers?: number;
  recent_same_station_retries?: number;
  recent_fast_mode_requests?: number;
};

export type ApiOperatorHealthSummary = {
  stations_draining?: number;
  stations_breaker_open?: number;
  stations_half_open?: number;
  stations_with_active_health_checks?: number;
  stations_with_probe_failures?: number;
  stations_with_degraded_passive_health?: number;
  stations_with_failing_passive_health?: number;
  stations_with_cooldown?: number;
  stations_with_usage_exhaustion?: number;
};

export type ApiOperatorSummaryLinks = {
  snapshot?: string;
  status_active?: string;
  runtime_status?: string;
  runtime_reload?: string;
  runtime_shutdown?: string;
  status_recent?: string;
  status_session_stats?: string;
  status_health_checks?: string;
  status_station_health?: string;
  request_ledger_recent?: string;
  request_ledger_summary?: string;
  control_trace?: string;
  retry_config?: string;
  pricing_catalog?: string;
  providers?: string;
  provider_balance_refresh?: string;
  provider_specs?: string;
  profiles?: string;
  default_profile?: string;
  persisted_default_profile?: string;
};

export type ApiControlPlaneSurfaceCapabilities = {
  runtime_status?: boolean;
  request_ledger_recent?: boolean;
  request_ledger_summary?: boolean;
  providers?: boolean;
  provider_balance_refresh?: boolean;
  provider_specs?: boolean;
  routing_explain?: boolean;
  [key: string]: boolean | undefined;
};

export type ApiRemoteAdminAccessCapabilities = {
  loopback_without_token?: boolean;
  remote_requires_token?: boolean;
  remote_enabled?: boolean;
  token_header?: string;
  token_env_var?: string;
};

export type ApiOperatorSummary = {
  api_version: number;
  service_name: string;
  runtime: ApiOperatorRuntimeSummary;
  counts: ApiOperatorSummaryCounts;
  retry: ApiOperatorRetrySummary;
  health?: ApiOperatorHealthSummary | null;
  stations?: ApiStationOption[];
  providers?: ApiProviderOption[];
  links?: ApiOperatorSummaryLinks | null;
  surface_capabilities?: ApiControlPlaneSurfaceCapabilities;
  remote_admin_access?: ApiRemoteAdminAccessCapabilities;
};

export type ApiRuntimeStatus = {
  runtime_source_path: string;
  config_path: string;
  loaded_at_ms: number;
  source_mtime_ms?: number | null;
  shutdown_available?: boolean;
};

export type ApiRequestObservability = {
  trace_id?: string;
  duration_ms?: number;
  ttfb_ms?: number;
  generation_ms?: number;
  output_tokens_per_second?: number;
  attempt_count?: number;
  route_attempt_count?: number;
  retried?: boolean;
  cross_station_failover?: boolean;
  same_station_retry?: boolean;
  fast_mode?: boolean;
  streaming?: boolean;
};

export type ApiFinishedRequest = {
  id: number;
  trace_id?: string;
  session_id?: string;
  model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  station_name?: string;
  provider_id?: string;
  upstream_base_url?: string;
  usage?: ApiUsageMetrics;
  cost?: ApiCostBreakdown;
  observability?: ApiRequestObservability;
  service: string;
  method: string;
  path: string;
  status_code: number;
  duration_ms: number;
  ttfb_ms?: number;
  streaming?: boolean;
  ended_at_ms: number;
};

export type ApiRequestUsageAggregate = {
  requests?: number;
  duration_ms_total?: number;
  input_tokens?: number;
  output_tokens?: number;
  reasoning_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
  total_tokens?: number;
};

export type ApiRequestUsageSummaryRow = {
  group_value: string;
  aggregate: ApiRequestUsageAggregate;
};
