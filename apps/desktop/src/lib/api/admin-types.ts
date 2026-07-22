export type ApiRuntimeConfigState = "normal" | "draining" | "breaker_open" | "half_open";

export type ApiCostConfidence = "unknown" | "partial" | "estimated" | "exact";

export type ApiEconomicsStatus = "complete" | "partial" | "conflict";

export type ApiUsageTotalSource =
  | "derived"
  | "derived_without_convention"
  | "reported"
  | "aggregated";

export type ApiUsageEvidenceSource =
  | "responses_input_tokens_details_cached_tokens"
  | "chat_prompt_tokens_details_cached_tokens"
  | "cached_input_tokens_alias"
  | "cached_tokens_alias"
  | "cache_read_input_tokens_alias"
  | "cache_read_tokens_alias"
  | "responses_input_tokens_details_cache_write_tokens"
  | "chat_prompt_tokens_details_cache_write_tokens"
  | "responses_input_tokens_details_cache_creation_tokens"
  | "chat_prompt_tokens_details_cache_creation_tokens"
  | "cache_creation_input_tokens_alias"
  | "cache_write_input_tokens_alias"
  | "cache_creation_tokens_alias"
  | "cache_write_tokens_alias"
  | "anthropic_cache_creation_ttl";

export type ApiUsageEvidenceState =
  | "missing"
  | "present_zero"
  | "present_value"
  | "invalid"
  | "conflict";

export type ApiUsageTokenObservation = {
  source: ApiUsageEvidenceSource;
  value: number;
};

export type ApiUsageTokenEvidence = {
  state: ApiUsageEvidenceState;
  selected: ApiUsageTokenObservation | null;
  observations: ApiUsageTokenObservation[];
  invalid_sources: ApiUsageEvidenceSource[];
};

export type ApiUsageEvidence = {
  cache_read_input_tokens: ApiUsageTokenEvidence;
  cache_write_input_tokens: ApiUsageTokenEvidence;
  aggregate_status?: ApiEconomicsStatus;
};

export type ApiCacheTokenInclusion = "unknown" | "separate" | "included_in_input";

export type ApiCacheAccountingConvention = {
  cache_read: ApiCacheTokenInclusion;
  cache_write: ApiCacheTokenInclusion;
};

export type ApiSelectedPriceTier = {
  tier_type: string;
  threshold_tokens: number;
  matched_input_tokens: number;
};

export type ApiUsageMetrics = {
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  reasoning_output_tokens?: number;
  total_tokens: number;
  total_tokens_source?: ApiUsageTotalSource;
  cached_input_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
  cache_creation_5m_input_tokens?: number;
  cache_creation_1h_input_tokens?: number;
  evidence?: ApiUsageEvidence;
};

export type ApiCostBreakdown = {
  input_cost_usd?: string;
  output_cost_usd?: string;
  cache_read_cost_usd?: string;
  cache_creation_cost_usd?: string;
  service_tier_multiplier?: string;
  provider_cost_multiplier?: string;
  total_cost_usd?: string;
  confidence: ApiCostConfidence;
  pricing_source?: string;
  pricing_provider?: string;
  pricing_generation?: string;
  effective_pricing_revision?: string;
  selected_tier?: ApiSelectedPriceTier;
};

export type ApiOperatorProfileSummary = {
  name: string;
  model: string | null;
  reasoning_effort: string | null;
  service_tier: string | null;
  fast_mode: boolean;
};

export type ApiEffectiveDefaultProfileSource = "none" | "configured" | "runtime_override";

export type ApiOperatorRuntimeSummary = {
  runtime_loaded_at_ms: number | null;
  runtime_source_mtime_ms: number | null;
  configured_default_profile: string | null;
  default_profile: string | null;
  runtime_default_profile_override: string | null;
  default_profile_source: ApiEffectiveDefaultProfileSource;
  default_profile_control_revision: number;
  profile_catalog_key: string;
  default_profile_summary: ApiOperatorProfileSummary | null;
  operator_actions?: ApiOperatorActionCapabilities;
};

export type ApiOperatorActionCapabilities = {
  refresh_provider_balances: boolean;
  mutate_routing: boolean;
  mutate_session_affinity: boolean;
  mutate_session_binding: boolean;
  reload_runtime: boolean;
  mutate_default_profile: boolean;
  inspect_relay_capabilities: boolean;
  run_relay_live_smoke: boolean;
};

export type ApiOperatorSummaryCounts = {
  active_requests: number;
  recent_requests: number;
  sessions: number;
  profiles: number;
  providers: number;
};

export type ApiRouteValueSource =
  | "request_payload"
  | "session_override"
  | "global_override"
  | "profile_default"
  | "provider_mapping"
  | "runtime_fallback";

export type ApiSessionContinuityMode = "default_profile" | "manual_profile";

export type ApiSessionBindingProjection = {
  revision: string;
  profile_name?: string;
  model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  continuity_mode?: ApiSessionContinuityMode;
};

export type ApiRetryProfileName =
  | "balanced"
  | "same-upstream"
  | "aggressive-failover"
  | "cost-primary";

export type ApiRetryStrategy = "failover" | "same_upstream";
export type ApiReasoningGuardAction = "observe" | "block" | "retry";
export type ApiReasoningGuardStreamMode = "off" | "observe" | "strict-buffer";
export type ApiReasoningGuardRetryExhaustedAction = "pass" | "block";

export type ApiOperatorRetryLayerSummary = {
  max_attempts: number;
  backoff_ms: number;
  backoff_max_ms: number;
  jitter_ms: number;
  on_status: string;
  on_class: string[];
  strategy: ApiRetryStrategy;
};

export type ApiOperatorReasoningGuardSummary = {
  enabled: boolean;
  reasoning_equals: number[];
  boundary_sequence_max_n: number;
  paths: string[];
  action: ApiReasoningGuardAction;
  stream_mode: ApiReasoningGuardStreamMode;
  max_guard_retries: number;
  on_retry_exhausted: ApiReasoningGuardRetryExhaustedAction;
  log_matches: boolean;
};

export type ApiOperatorRetryPolicySummary = {
  upstream: ApiOperatorRetryLayerSummary;
  provider: ApiOperatorRetryLayerSummary;
  never_on_status: string;
  never_on_class: string[];
  cloudflare_challenge_cooldown_secs: number;
  cloudflare_timeout_cooldown_secs: number;
  transport_cooldown_secs: number;
  cooldown_backoff_factor: number;
  cooldown_backoff_max_secs: number;
  reasoning_guard: ApiOperatorReasoningGuardSummary;
};

export type ApiResolvedRouteValue = {
  value: string;
  source: ApiRouteValueSource;
};

export type ApiOperatorRouteDecision = {
  decided_at_ms: number;
  binding_profile_name?: string;
  binding_continuity_mode?: ApiSessionContinuityMode;
  effective_model?: ApiResolvedRouteValue;
  effective_reasoning_effort?: ApiResolvedRouteValue;
  effective_service_tier?: ApiResolvedRouteValue;
  effective_upstream_base_url?: ApiResolvedRouteValue;
  provider_id?: string;
  endpoint_id?: string;
  route_path?: string[];
};

export type ApiOperatorSessionRouteAffinitySummary = {
  revision: string;
  provider_id: string;
  endpoint_id: string;
  upstream_origin: string;
  route_path?: string[];
  last_selected_at_ms: number;
  last_changed_at_ms: number;
  change_reason: string;
};

export type ApiOperatorRetrySummary = {
  configured_profile: ApiRetryProfileName | null;
  upstream_max_attempts: number;
  provider_max_attempts: number;
  policy?: ApiOperatorRetryPolicySummary;
  recent_retried_requests: number;
  recent_cross_provider_failovers: number;
  recent_same_provider_retries: number;
  recent_fast_mode_requests: number;
};

export type ApiOperatorSummary = {
  api_version: number;
  service_name: string;
  runtime: ApiOperatorRuntimeSummary;
  counts: ApiOperatorSummaryCounts;
  retry: ApiOperatorRetrySummary;
  credential_readiness?: ApiCredentialAggregateReadiness;
  sessions: ApiOperatorSessionSummary[];
  profiles: ApiControlProfileOption[];
  providers: ApiOperatorProviderSummary[];
};

export type ApiRequestObservability = {
  trace_id?: string;
  duration_ms?: number;
  ttfb_ms?: number;
  generation_ms?: number;
  output_tokens_per_second?: number;
  attempt_count: number;
  route_attempt_count: number;
  retried?: boolean;
  cross_provider_failover?: boolean;
  same_provider_retry?: boolean;
  fast_mode?: boolean;
  streaming?: boolean;
};

export type ApiProviderEndpointKey = {
  service_name: string;
  provider_id: string;
  endpoint_id: string;
};

export type ApiSessionIdentitySource =
  | "header"
  | "body_session_id"
  | "prompt_cache_key"
  | "metadata_session_id"
  | "previous_response_id";

export type ApiProviderSignalKind =
  | "quota"
  | "rate_limit"
  | "capacity"
  | "transport"
  | "balance"
  | "service_status"
  | "capability"
  | "local_concurrency"
  | "unknown";

export type ApiProviderSignalSource =
  | "upstream_response"
  | "response_headers"
  | "balance_snapshot"
  | "service_status"
  | "capability_probe"
  | "local_scheduler"
  | "route_attempt";

export type ApiProviderSignalConfidence = "low" | "medium" | "high";
export type ApiPolicyActionKind = "cooldown" | "unknown";
export type ApiPolicyActionOwner = "codex_helper";
export type ApiPolicyActionRecoveryState = "active";

export type ApiProviderSignalTarget =
  | { provider_endpoint: { provider_endpoint_key: ApiProviderEndpointKey } }
  | { provider: { service: string; provider_id: string } }
  | { service: { service: string } };

export type ApiRequestChainSelector = {
  trace_id?: string;
  request_id?: number;
  session_id?: string;
};

export type ApiRequestChainRouteAttempt = {
  attempt_index: number;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  preference_group?: number;
  route_path?: string[];
  provider_attempt?: number;
  upstream_attempt?: number;
  provider_max_attempts?: number;
  upstream_max_attempts?: number;
  avoided_total?: number;
  total_upstreams?: number;
  decision: string;
  code: string;
  status_code?: number;
  error_class?: string;
  model?: string;
  upstream_headers_ms?: number;
  duration_ms?: number;
  cooldown_secs?: number;
  skipped?: boolean;
  provider_signals: ApiRequestChainProviderSignal[];
  policy_actions: ApiRequestChainPolicyAction[];
};

export type ApiRequestChainProviderSignal = {
  kind: ApiProviderSignalKind;
  code: string;
  source: ApiProviderSignalSource;
  target: ApiProviderSignalTarget;
  confidence: ApiProviderSignalConfidence;
  observed_at_ms: number;
  route_facing?: boolean;
  retry_after_secs?: number;
  reset_after_secs?: number;
  error_class?: string;
  trace_id?: string;
};

export type ApiRequestChainPolicyAction = {
  id: string;
  kind: ApiPolicyActionKind;
  code: string;
  owner: ApiPolicyActionOwner;
  provider_endpoint_key: string;
  source_signal: ApiRequestChainProviderSignal;
  confidence: ApiProviderSignalConfidence;
  created_at_ms: number;
  expires_at_ms: number;
  recovery_state: ApiPolicyActionRecoveryState;
  generation: number;
};

export type ApiRequestChainTimelineEvent = {
  order: number;
  at_ms?: number;
  kind: string;
  code: string;
  attempt_index?: number;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  status_code?: number;
  model?: string;
};

export type ApiRequestChainRequest = {
  request_id: number;
  trace_id?: string;
  session_id?: string;
  session_identity_source?: ApiSessionIdentitySource;
  client_name?: string;
  model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  route_path?: string[];
  usage?: ApiUsageMetrics;
  cost?: ApiCostBreakdown;
  observability: ApiRequestObservability;
  service: string;
  method: string;
  path: string;
  status_code: number;
  duration_ms: number;
  ttfb_ms?: number;
  streaming: boolean;
  ended_at_ms: number;
  attempts_truncated: boolean;
  provider_signals_truncated: boolean;
  policy_actions_truncated: boolean;
  route_attempts: ApiRequestChainRouteAttempt[];
  provider_signals: ApiRequestChainProviderSignal[];
  policy_actions: ApiRequestChainPolicyAction[];
  timeline: ApiRequestChainTimelineEvent[];
};

export type ApiRequestChainExport = {
  api_version: number;
  selector: ApiRequestChainSelector;
  limit: number;
  truncated: boolean;
  requests: ApiRequestChainRequest[];
};

export type ApiRequestUsageAggregate = {
  requests: number;
  duration_ms_total: number;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
  cache_read_input_tokens: number;
  cache_creation_input_tokens: number;
  total_tokens: number;
};

export type ApiRequestUsageSummaryRow = {
  group_value: string;
  aggregate: ApiRequestUsageAggregate;
};

export type ApiRequestUsageSummaryGroup =
  | "provider_endpoint"
  | "provider"
  | "model"
  | "session";

export type ApiRequestUsageSummaryCoverage = {
  source: string;
  first_terminal_at_ms: number | null;
  last_terminal_at_ms: number | null;
  requests: number;
  all_history: boolean;
};

export type ApiRequestUsageSummary = {
  group: ApiRequestUsageSummaryGroup;
  coverage: ApiRequestUsageSummaryCoverage;
  rows: ApiRequestUsageSummaryRow[];
};

export type ApiUsageCostSummary = {
  total_cost_usd?: string;
  confidence: ApiCostConfidence;
  priced_requests?: number;
  unpriced_requests?: number;
  partial_requests?: number;
  exact_requests?: number;
};

export type ApiUsageBucket = {
  requests_total: number;
  requests_error: number;
  duration_ms_total: number;
  requests_with_usage: number;
  duration_ms_with_usage_total: number;
  generation_ms_total: number;
  ttfb_ms_total: number;
  ttfb_samples: number;
  usage: ApiUsageMetrics;
  cost?: ApiUsageCostSummary;
};

export type ApiUsageDayHourRow = {
  hour: number;
  bucket: ApiUsageBucket;
};

export type ApiUsageDayDimensionRow = {
  name: string;
  bucket: ApiUsageBucket;
};

export type ApiUsageDayCoverage = {
  source: string;
  loaded_first_ms: number | null;
  loaded_last_ms: number | null;
  loaded_requests: number;
  day_may_be_partial: boolean;
  partial_reason?: string;
};

export type ApiUsageRetryGateReasonRow = {
  reason: string;
  active: number;
};

export type ApiUsageRetryGateSummary = {
  active: number;
  active_cooldowns: number;
  max_remaining_secs: number | null;
  reasons: ApiUsageRetryGateReasonRow[];
};

export type ApiUsageDayView = {
  day: number;
  label: string;
  start_ms: number;
  end_ms: number;
  generated_at_ms: number;
  summary: ApiUsageBucket;
  hourly: ApiUsageDayHourRow[];
  provider_rows: ApiUsageDayDimensionRow[];
  provider_endpoint_rows: ApiUsageDayDimensionRow[];
  model_rows: ApiUsageDayDimensionRow[];
  session_rows: ApiUsageDayDimensionRow[];
  project_rows: ApiUsageDayDimensionRow[];
  retry_gate: ApiUsageRetryGateSummary;
  coverage: ApiUsageDayCoverage;
};

export type ApiControlProfileOption = {
  name: string;
  extends?: string;
  model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  fast_mode: boolean;
  is_default: boolean;
};

export type ApiOperatorSessionSummary = {
  session_key: string;
  active_count: number;
  active_started_at_ms_min?: number;
  last_status?: number;
  last_duration_ms?: number;
  last_ended_at_ms?: number;
  last_model?: string;
  last_reasoning_effort?: string;
  last_service_tier?: string;
  last_provider_id?: string;
  last_usage?: ApiUsageMetrics;
  total_usage?: ApiUsageMetrics;
  turns_total?: number;
  turns_with_usage?: number;
  last_output_tokens_per_second?: number;
  avg_output_tokens_per_second?: number;
  binding_profile_name?: string;
  binding_continuity_mode?: ApiSessionContinuityMode;
  binding: ApiSessionBindingProjection;
  last_route_decision?: ApiOperatorRouteDecision;
  route_affinity?: ApiOperatorSessionRouteAffinitySummary;
  effective_model?: ApiResolvedRouteValue;
  effective_reasoning_effort?: ApiResolvedRouteValue;
  effective_service_tier?: ApiResolvedRouteValue;
};

export type ApiOperatorPolicyActionSummary = {
  active_cooldown: boolean;
  code: string;
  cooldown_remaining_secs?: number;
};

export type ApiCredentialReadinessCode =
  | "ready"
  | "stale"
  | "missing"
  | "invalid"
  | "locked"
  | "permission_denied"
  | "interaction_required"
  | "backend_unavailable"
  | "unsupported";

export type ApiCredentialBindingKind = "bearer" | "api_key";
export type ApiCredentialAggregateReadiness = "ready" | "degraded" | "blocked";

export type ApiCredentialReadinessDetail = {
  kind?: ApiCredentialBindingKind;
  code: ApiCredentialReadinessCode;
  stale_cause?: ApiCredentialReadinessCode;
  source_kind?: string;
  reference?: string;
};

export type ApiOperatorProviderCapacity = {
  configured_max_concurrent_requests?: number;
  effective_max_concurrent_requests?: number;
  active?: number;
  limit?: number;
  saturated: boolean;
  inherited_from_provider?: boolean;
};

export type ApiOperatorProviderEndpointSummary = {
  provider_name: string;
  name: string;
  provider_endpoint_key: string;
  origin?: string;
  priority: number;
  configured_enabled: boolean;
  effective_enabled: boolean;
  routable: boolean;
  credential_readiness?: ApiCredentialReadinessCode;
  credential_details?: ApiCredentialReadinessDetail[];
  runtime_enabled_override?: boolean;
  runtime_state: ApiRuntimeConfigState;
  runtime_state_override?: ApiRuntimeConfigState;
  capacity?: ApiOperatorProviderCapacity;
  policy_actions?: ApiOperatorPolicyActionSummary[];
};

export type ApiOperatorProviderSummary = {
  name: string;
  alias?: string;
  configured_enabled: boolean;
  effective_enabled: boolean;
  routable_endpoints: number;
  credential_readiness?: ApiCredentialAggregateReadiness;
  endpoints: ApiOperatorProviderEndpointSummary[];
  capacity?: ApiOperatorProviderCapacity;
};

export type ApiOperatorRequestObservability = {
  duration_ms?: number;
  ttfb_ms?: number;
  generation_ms?: number;
  output_tokens_per_second?: number;
  attempt_count: number;
  route_attempt_count: number;
  retried: boolean;
  cross_provider_failover: boolean;
  same_provider_retry: boolean;
  fast_mode: boolean;
  streaming: boolean;
};

export type ApiOperatorRouteAttemptSummary = {
  attempt_index: number;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  preference_group?: number;
  provider_attempt?: number;
  upstream_attempt?: number;
  provider_max_attempts?: number;
  upstream_max_attempts?: number;
  avoided_total?: number;
  total_upstreams?: number;
  code: string;
  status_code?: number;
  model?: string;
  upstream_headers_ms?: number;
  duration_ms?: number;
  cooldown_secs?: number;
  skipped: boolean;
  provider_signal_codes?: string[];
  policy_action_codes?: string[];
};

export type ApiOperatorRequestSummary = {
  id: number;
  trace_key?: string;
  session_key?: string;
  model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  route_path?: string[];
  upstream_origin?: string;
  usage?: ApiUsageMetrics;
  cache_accounting_convention: ApiCacheAccountingConvention;
  cost?: ApiCostBreakdown;
  retry?: ApiOperatorRetrySummaryView;
  provider_signal_codes?: string[];
  policy_action_codes?: string[];
  observability: ApiOperatorRequestObservability;
  service: string;
  method: string;
  path: string;
  status_code: number;
  duration_ms: number;
  ttfb_ms?: number;
  streaming: boolean;
  ended_at_ms: number;
};

export type ApiOperatorRetrySummaryView = {
  attempts: number;
  route_attempts?: ApiOperatorRouteAttemptSummary[];
};

export type ApiOperatorActiveRequestSummary = {
  id: number;
  runtime_revision: number;
  policy_revision: number;
  session_key?: string;
  model?: string;
  requested_model?: string;
  reasoning_effort?: string;
  service_tier?: string;
  requested_service_tier?: string;
  provider_id?: string;
  endpoint_id?: string;
  provider_endpoint_key?: string;
  route_path?: string[];
  upstream_origin?: string;
  service: string;
  method: string;
  path: string;
  started_at_ms: number;
};

export type ApiWindowStats = {
  total: number;
  ok_2xx: number;
  err_429: number;
  err_4xx: number;
  err_5xx: number;
  p50_ms: number | null;
  p95_ms: number | null;
  avg_attempts: number | null;
  retry_rate: number | null;
  top_provider: [string, number] | null;
  top_provider_endpoint: [string, number] | null;
};

export type ApiUsageRollupCoverage = {
  requested_days: number;
  all_loaded: boolean;
  loaded_first_day: number | null;
  loaded_last_day: number | null;
  loaded_days_with_data: number;
  loaded_requests: number;
  window_first_day: number | null;
  window_last_day: number | null;
  window_days_with_data: number;
  window_requests: number;
  window_exceeds_loaded_start: boolean;
};

export type ApiUsageRollupView = {
  loaded: ApiUsageBucket;
  window: ApiUsageBucket;
  coverage: ApiUsageRollupCoverage;
  by_day: Array<[number, ApiUsageBucket]>;
  by_provider_endpoint: Array<[string, ApiUsageBucket]>;
  by_provider_endpoint_day: Record<string, Array<[number, ApiUsageBucket]>>;
  by_provider: Array<[string, ApiUsageBucket]>;
  by_provider_day: Record<string, Array<[number, ApiUsageBucket]>>;
};

export type ApiQuotaAnalyticsSupport = "unsupported" | "supported";

export type ApiQuotaRateStatus =
  | "available"
  | "insufficient_samples"
  | "short_span"
  | "stale"
  | "gap"
  | "adjustment"
  | "negative_delta"
  | "unordered"
  | "no_counter"
  | "overflow";

export type ApiQuotaPaceStatus =
  | "unlimited"
  | "faster"
  | "on_pace"
  | "slower"
  | "no_reset"
  | "reset_unknown"
  | "low_sample"
  | "stale"
  | "unavailable";

export type ApiQuotaFreshnessStatus = "fresh" | "stale" | "offline" | "unknown";

export type ApiQuotaReconciliationStatus =
  | "available"
  | "incomplete_coverage"
  | "stale_remote"
  | "incompatible_unit"
  | "incompatible_generation"
  | "window_mismatch"
  | "no_remote_delta"
  | "overflow"
  | "unavailable";

export type ApiQuotaUnit = "raw" | "usd" | "tokens" | "unknown";

export type ApiQuotaWindowKind =
  | "calendar_day"
  | "rolling"
  | "custom"
  | "monthly"
  | "resetless"
  | "unknown";

export type ApiQuotaResetKind =
  | "explicit_timestamp"
  | "configured_calendar_boundary"
  | "no_reset"
  | "unknown";

export type ApiQuotaAdjustmentKind =
  | "discontinuity"
  | "counter_reset_or_rollback"
  | "top_up"
  | "limit_or_plan_changed"
  | "normalization_changed";

export type ApiQuotaScope =
  | { kind: "account" }
  | { kind: "api_key" }
  | { kind: "subscription" }
  | { kind: "organization" }
  | { kind: "endpoint" }
  | { kind: "custom"; value: string }
  | { kind: "unknown" };

export type ApiQuotaIdentityEvidence =
  | "remote_quota_owner_id"
  | "remote_stable_id"
  | "explicit_pool_id"
  | "credential_fingerprint"
  | "endpoint_origin"
  | "unknown";

export type ApiQuotaIdentityConfidence = "high" | "medium" | "low" | "unknown";

export type ApiQuotaConversionSource = "remote" | "configured" | "bundled" | "unknown";

export type ApiQuotaWindowSemantics = {
  kind: ApiQuotaWindowKind;
  reset: ApiQuotaResetKind;
  reset_timezone?: string;
  rolling_duration_ms?: number;
};

export type ApiQuotaCapabilities = {
  used: boolean;
  remaining: boolean;
  direct_total: boolean;
  limit: boolean;
  reset: boolean;
  window: boolean;
  conversion: boolean;
  cumulative: boolean;
  unlimited?: boolean;
  raw_unit?: boolean;
};

export type ApiQuotaPoolIdentity = {
  key: string;
  origin: string;
  scope: ApiQuotaScope;
  revision: number;
  evidence: ApiQuotaIdentityEvidence;
  confidence: ApiQuotaIdentityConfidence;
  aggregation_eligible?: boolean;
  conflicting_evidence?: boolean;
};

export type ApiQuotaQuantity = {
  value: string;
  scale: number;
  unit: ApiQuotaUnit;
  conversion_generation?: number;
};

export type ApiQuotaConversion = {
  source: ApiQuotaConversionSource;
  divisor: number | null;
  generation: number | null;
};

export type ApiProjectIdentityKind = "git_root" | "path_fallback" | "unknown";

export type ApiProjectIdentity = {
  kind: ApiProjectIdentityKind;
  path?: string;
};

export type ApiAttributionCoverage = {
  loaded_first_ms: number | null;
  loaded_last_ms: number | null;
  queried_first_ms: number | null;
  queried_last_ms: number | null;
  time_truncated: boolean;
  count_truncated: boolean;
  dedupe_truncated: boolean;
  boundary_partial: boolean;
  leading_boundary_partial: boolean;
  trailing_boundary_partial: boolean;
  cost_overflow: boolean;
  duplicate_requests: number;
  partial_captured_price_requests: number;
  reconstructed_price_requests: number;
  invalid_captured_price_requests: number;
  unpriced_requests: number;
  unmatched_endpoint_requests: number;
  unmatched_pool_requests: number;
  unknown_project_requests: number;
};

export type ApiQuotaRateWindow = {
  status: ApiQuotaRateStatus;
  rate_per_hour: ApiQuotaQuantity | null;
  lower_bound: boolean;
  sample_count: number;
  span_ms: number;
};

export type ApiQuotaPacingView = {
  status: ApiQuotaPaceStatus;
  required_rate_per_hour: ApiQuotaQuantity | null;
  pace_ratio_basis_points: number | null;
  exhaustion_eta_ms: number | null;
  projected_remaining_at_reset: ApiQuotaQuantity | null;
  reset_at_ms: number | null;
};

export type ApiQuotaProjectRow = {
  project: ApiProjectIdentity;
  local_cost: ApiQuotaQuantity;
  requests: number;
};

export type ApiQuotaReconciliationView = {
  status: ApiQuotaReconciliationStatus;
  remote_total: ApiQuotaQuantity | null;
  local_known: ApiQuotaQuantity | null;
  local_unknown: ApiQuotaQuantity | null;
  external_unattributed: ApiQuotaQuantity | null;
  signed_delta: string | null;
  projects: ApiQuotaProjectRow[];
  omitted_projects: number;
  omitted_local_known: ApiQuotaQuantity | null;
  coverage: ApiAttributionCoverage;
};

export type ApiPoolQuotaAnalytics = {
  identity: ApiQuotaPoolIdentity;
  observed_at_ms: number;
  last_success_at_ms: number | null;
  last_attempt_at_ms: number | null;
  freshness: ApiQuotaFreshnessStatus;
  latest_adjustment: ApiQuotaAdjustmentKind | null;
  source: string;
  unit: ApiQuotaUnit;
  conversion: ApiQuotaConversion | null;
  capabilities: ApiQuotaCapabilities;
  window: ApiQuotaWindowSemantics;
  epoch_start_ms: number;
  epoch_end_ms: number | null;
  remote_used: ApiQuotaQuantity | null;
  remote_direct_total: ApiQuotaQuantity | null;
  remote_remaining: ApiQuotaQuantity | null;
  remote_limit: ApiQuotaQuantity | null;
  observed_burn: ApiQuotaQuantity | null;
  rate_15m: ApiQuotaRateWindow;
  rate_60m: ApiQuotaRateWindow;
  pacing: ApiQuotaPacingView;
  reconciliation: ApiQuotaReconciliationView;
};

export type ApiQuotaAnalyticsView = {
  support: ApiQuotaAnalyticsSupport;
  generated_at_ms: number;
  registry_generation: number;
  pools: ApiPoolQuotaAnalytics[];
  omitted_pools: number;
};

export type ApiModelPriceTierView = {
  threshold_tokens: number;
  input_per_1m_usd?: string;
  output_per_1m_usd?: string;
  cache_read_input_per_1m_usd?: string;
  cache_creation_input_per_1m_usd?: string;
};

export type ApiModelPriceView = {
  provider: string;
  model_id: string;
  display_name?: string;
  aliases?: string[];
  input_per_1m_usd: string;
  output_per_1m_usd: string;
  cache_read_input_per_1m_usd?: string;
  cache_creation_input_per_1m_usd?: string;
  tiers?: ApiModelPriceTierView[];
  source: string;
  source_generation?: string;
  confidence: ApiCostConfidence;
};

export type ApiModelPriceCatalogSnapshot = {
  source: string;
  model_count: number;
  models: ApiModelPriceView[];
};

export type ApiBalanceSnapshotStatus = "unknown" | "ok" | "exhausted" | "stale" | "error";

export type ApiProviderUsageAlertKind =
  | "daily_usage_80"
  | "daily_usage_95"
  | "low_balance"
  | "subscription_expiring_soon"
  | "subscription_expired";

export type ApiProviderUsageWindow = {
  period: string;
  used_usd?: string;
  limit_usd?: string;
  remaining_usd?: string;
  unlimited?: boolean;
};

export type ApiProviderUsageRateSnapshot = {
  average_duration_ms?: string;
  rpm?: string;
  tpm?: string;
};

export type ApiProviderUsageModelStat = {
  model: string;
  request_count?: number;
  input_tokens?: number;
  output_tokens?: number;
  total_tokens?: number;
  input_cost_usd?: string;
  output_cost_usd?: string;
  total_cost_usd?: string;
};

export type ApiOperatorProviderBalanceSummary = {
  observation_provider_id: string;
  provider_id: string;
  endpoint_id: string;
  provider_endpoint_key: string;
  fetched_at_ms: number;
  stale_after_ms?: number;
  stale: boolean;
  status: ApiBalanceSnapshotStatus;
  exhausted?: boolean;
  exhaustion_affects_routing: boolean;
  plan_name?: string;
  total_balance_usd?: string;
  subscription_balance_usd?: string;
  paygo_balance_usd?: string;
  monthly_budget_usd?: string;
  monthly_spent_usd?: string;
  quota_period?: string;
  quota_remaining_usd?: string;
  quota_limit_usd?: string;
  quota_used_usd?: string;
  quota_resets_at_ms?: number;
  unlimited_quota?: boolean;
  total_used_usd?: string;
  today_used_usd?: string;
  total_requests?: number;
  today_requests?: number;
  total_tokens?: number;
  today_tokens?: number;
  subscription_expires_at?: string;
  usage_windows?: ApiProviderUsageWindow[];
  usage_rate?: ApiProviderUsageRateSnapshot;
  usage_model_stats?: ApiProviderUsageModelStat[];
  alert_codes?: ApiProviderUsageAlertKind[];
  error?: string;
};

export type ApiOperatorRevisionBundle = {
  runtime_revision: number;
  runtime_digest: string;
  route_digest: string;
  catalog_revision: string;
  pricing_revision: string;
  operator_pricing_revision: string;
  policy_revision: number;
  ledger_revision: string;
};

export type ApiServiceStatusKind = "ok" | "slow" | "failed" | "unknown";

export type ApiServiceStatusProbeSample = {
  ts_ms?: number;
  ok?: boolean;
  latency_ms?: number;
  error?: string;
};

export type ApiServiceStatusCellSnapshot = {
  kind: ApiServiceStatusKind;
  probe?: ApiServiceStatusProbeSample;
};

export type ApiServiceStatusServiceSnapshot = {
  model: string;
  uptime_pct?: string;
  latest_kind: ApiServiceStatusKind;
  latest?: ApiServiceStatusProbeSample;
  history: ApiServiceStatusCellSnapshot[];
};

export type ApiServiceStatusProbeSnapshot = {
  id: string;
  url: string;
  fetched_at_ms: number;
  generated_at_ms?: number;
  all_ok?: boolean;
  services: ApiServiceStatusServiceSnapshot[];
  credential_readiness?: ApiCredentialReadinessCode;
  credential_details?: ApiCredentialReadinessDetail[];
  error?: string;
};

export type ApiServiceStatusSnapshot = {
  generated_at_ms: number;
  configured: boolean;
  enabled: boolean;
  refresh_interval_secs: number;
  history_cells: number;
  probes: ApiServiceStatusProbeSnapshot[];
  error?: string;
};

export type ApiOperatorReadData = {
  summary: ApiOperatorSummary;
  routing?: ApiOperatorRoutingSummary;
  active_requests: ApiOperatorActiveRequestSummary[];
  recent_requests: ApiOperatorRequestSummary[];
  usage_summaries: ApiRequestUsageSummary[];
  usage_day: ApiUsageDayView;
  usage_rollup: ApiUsageRollupView;
  quota_analytics: ApiQuotaAnalyticsView;
  stats_5m: ApiWindowStats;
  stats_1h: ApiWindowStats;
  pricing_catalog: ApiModelPriceCatalogSnapshot;
  service_status?: ApiServiceStatusSnapshot;
  provider_balances: ApiOperatorProviderBalanceSummary[];
};

export type ApiRouteStrategy =
  | "manual-sticky"
  | "ordered-failover"
  | "round-robin"
  | "tag-preferred"
  | "conditional";
export type ApiRouteAffinityPolicy = "off" | "preferred-group" | "fallback-sticky" | "hard";
export type ApiSchedulingPreset = "continuity-first" | "balanced" | "throughput-first";

export type ApiOperatorRouteTargetSummary = {
  provider_id: string;
  endpoint_id: string;
};

export type ApiOperatorRouteCandidateSummary = {
  route_order: number;
  provider_id: string;
  endpoint_id: string;
  preference_group: number;
  route_path?: string[];
};

export type ApiOperatorRoutingSummary = {
  route_graph_key: string;
  control_revision: number;
  provider_policy_revision: number;
  entry: string;
  entry_strategy: ApiRouteStrategy;
  entry_target?: string;
  new_session_preference?: ApiOperatorRouteTargetSummary;
  affinity_policy: ApiRouteAffinityPolicy;
  scheduling_preset: ApiSchedulingPreset;
  fallback_ttl_ms?: number;
  reprobe_preferred_after_ms?: number;
  candidates: ApiOperatorRouteCandidateSummary[];
};

export type ApiOperatorReadStatus = "ready" | "stale" | "disconnected" | "auth_required";
export type ApiOperatorReadIssue = "refresh_failed" | "disconnected" | "auth_required";

type ApiOperatorReadModelBase = {
  api_version: 1;
  service_name: string;
  captured_at_ms: number;
};

export type ApiOperatorReadModel =
  | (ApiOperatorReadModelBase & {
      status: "ready";
      revisions: ApiOperatorRevisionBundle;
      data: ApiOperatorReadData;
      issue?: never;
    })
  | (ApiOperatorReadModelBase & {
      status: "stale";
      revisions: ApiOperatorRevisionBundle;
      data: ApiOperatorReadData;
      issue: "refresh_failed";
    })
  | (ApiOperatorReadModelBase & {
      status: "disconnected";
      revisions?: never;
      data?: never;
      issue: "disconnected";
    })
  | (ApiOperatorReadModelBase & {
      status: "auth_required";
      revisions?: never;
      data?: never;
      issue: "auth_required";
    });

export type ApiOperatorReadModelWire = {
  api_version: number;
  service_name: string;
  status: ApiOperatorReadStatus;
  captured_at_ms: number;
  revisions?: ApiOperatorRevisionBundle;
  data?: ApiOperatorReadData;
  issue?: ApiOperatorReadIssue;
};
