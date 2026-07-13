use std::collections::HashMap;

use crate::request_chain::{
    REQUEST_CHAIN_EXPORT_MAX_LIMIT, RequestChainExport, RequestChainSelector,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::{
    CommittedRequestFilter, CommittedRequestIdentityQuery, CommittedRequestPage,
    CommittedRequestProjection, CommittedRequestQuery, RequestAccountingScope, RuntimeStore,
    RuntimeStoreError, RuntimeStoreReader, final_route_provider_endpoint, final_route_provider_id,
};
use crate::state::FinishedRequest;
use crate::usage::{CanonicalUsageBuckets, EconomicsStatus, UsageMetrics};

const LEDGER_SCAN_PAGE_SIZE: usize = 1_024;

/// Typed filters over committed request terminals.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestLogFilters {
    pub service: Option<String>,
    pub trace_id: Option<String>,
    pub request_id: Option<u64>,
    pub session: Option<String>,
    pub model: Option<String>,
    pub provider_endpoint: Option<ProviderEndpointKey>,
    pub provider: Option<String>,
    pub path: Option<String>,
    pub signal_kind: Option<String>,
    pub policy_action_kind: Option<String>,
    pub status_min: Option<u64>,
    pub status_max: Option<u64>,
    pub fast: bool,
    pub retried: bool,
}

impl RequestLogFilters {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn committed_filter(&self) -> CommittedRequestFilter {
        CommittedRequestFilter {
            service: self.service.clone(),
            trace_id: self.trace_id.clone(),
            request_id: self.request_id,
            session: self.session.clone(),
            model: self.model.clone(),
            provider_endpoint: self.provider_endpoint.clone(),
            provider: self.provider.clone(),
            path: self.path.clone(),
            signal_kind: self.signal_kind.clone(),
            policy_action_kind: self.policy_action_kind.clone(),
            status_min: self.status_min,
            status_max: self.status_max,
            fast: self.fast,
            retried: self.retried,
        }
    }
}

/// A source capable of querying the helper-owned committed request authority.
pub trait RequestLedgerSource {
    fn query_committed_requests(
        &self,
        query: &CommittedRequestQuery,
    ) -> Result<CommittedRequestPage, RuntimeStoreError>;

    fn query_committed_request_identities(
        &self,
        query: &CommittedRequestIdentityQuery,
    ) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError>;
}

impl RequestLedgerSource for RuntimeStore {
    fn query_committed_requests(
        &self,
        query: &CommittedRequestQuery,
    ) -> Result<CommittedRequestPage, RuntimeStoreError> {
        RuntimeStore::query_committed_requests(self, query)
    }

    fn query_committed_request_identities(
        &self,
        query: &CommittedRequestIdentityQuery,
    ) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError> {
        RuntimeStore::query_committed_request_identities(self, query)
    }
}

impl RequestLedgerSource for RuntimeStoreReader {
    fn query_committed_requests(
        &self,
        query: &CommittedRequestQuery,
    ) -> Result<CommittedRequestPage, RuntimeStoreError> {
        RuntimeStoreReader::query_committed_requests(self, query)
    }

    fn query_committed_request_identities(
        &self,
        query: &CommittedRequestIdentityQuery,
    ) -> Result<Vec<CommittedRequestProjection>, RuntimeStoreError> {
        RuntimeStoreReader::query_committed_request_identities(self, query)
    }
}

/// Read projections over committed SQLite request terminals.
pub struct RequestLedger<'source, Source: RequestLedgerSource + ?Sized> {
    source: &'source Source,
}

impl<'source, Source: RequestLedgerSource + ?Sized> RequestLedger<'source, Source> {
    pub fn new(source: &'source Source) -> Self {
        Self { source }
    }

    pub fn tail_finished_requests(
        &self,
        limit: usize,
    ) -> Result<Vec<FinishedRequest>, RuntimeStoreError> {
        self.find_finished_requests(&RequestLogFilters::default(), limit)
    }

    pub fn find_finished_requests(
        &self,
        filters: &RequestLogFilters,
        limit: usize,
    ) -> Result<Vec<FinishedRequest>, RuntimeStoreError> {
        let mut finished = Vec::with_capacity(limit.min(LEDGER_SCAN_PAGE_SIZE));
        let mut cursor = None;
        while finished.len() < limit {
            let page = self
                .source
                .query_committed_requests(&CommittedRequestQuery {
                    limit: limit - finished.len(),
                    cursor,
                    terminal_at_or_after_unix_ms: None,
                    filter: filters.committed_filter(),
                })?;
            finished.extend(
                page.items
                    .into_iter()
                    .map(|projection| projection.payload.finished_request),
            );
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(finished)
    }

    pub fn summarize(
        &self,
        group: RequestUsageSummaryGroup,
        filters: &RequestLogFilters,
        limit: usize,
    ) -> Result<Vec<RequestUsageSummaryRow>, RuntimeStoreError> {
        let mut aggregate: HashMap<String, RequestUsageAggregate> = HashMap::new();
        let mut cursor = None;
        loop {
            let page = self
                .source
                .query_committed_requests(&CommittedRequestQuery {
                    limit: LEDGER_SCAN_PAGE_SIZE,
                    cursor,
                    terminal_at_or_after_unix_ms: None,
                    filter: filters.committed_filter(),
                })?;
            for projection in page.items {
                if projection.payload.accounting_scope != RequestAccountingScope::Economic {
                    continue;
                }
                let request = &projection.payload.finished_request;
                aggregate
                    .entry(group.key(request))
                    .or_default()
                    .record_request(request, projection.payload.billable_usage.as_ref());
            }
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }

        let mut rows = aggregate
            .into_iter()
            .map(|(group_value, aggregate)| RequestUsageSummaryRow {
                group_value,
                aggregate,
            })
            .collect::<Vec<_>>();
        sort_usage_summary_rows(&mut rows, limit);
        Ok(rows)
    }

    pub fn export_request_chain(
        &self,
        service_name: &str,
        selector: RequestChainSelector,
        limit: usize,
    ) -> Result<RequestChainExport, RuntimeStoreError> {
        let selector = selector.normalized();
        let limit = limit.clamp(1, REQUEST_CHAIN_EXPORT_MAX_LIMIT);
        let projections =
            self.source
                .query_committed_request_identities(&CommittedRequestIdentityQuery {
                    service: service_name.to_string(),
                    trace_id: selector.trace_id.clone(),
                    request_id: selector.request_id,
                    session_id: selector.session_id.clone(),
                    limit: limit.saturating_add(1),
                })?;
        let mut requests = projections
            .into_iter()
            .map(|projection| projection.payload.finished_request)
            .collect::<Vec<_>>();
        let truncated = requests.len() > limit;
        requests.truncate(limit);
        Ok(RequestChainExport::from_finished_requests(
            selector, limit, truncated, requests,
        ))
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RequestUsageAggregate {
    pub requests: u64,
    pub duration_ms_total: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub total_tokens: i64,
}

impl RequestUsageAggregate {
    pub(crate) fn add_assign(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.duration_ms_total = self
            .duration_ms_total
            .saturating_add(other.duration_ms_total);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(other.cache_creation_input_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }

    pub fn record_request(
        &mut self,
        request: &FinishedRequest,
        billable_usage: Option<&CanonicalUsageBuckets>,
    ) {
        self.record(request.duration_ms, request.usage.as_ref(), billable_usage);
    }

    pub fn record(
        &mut self,
        duration_ms: u64,
        usage: Option<&UsageMetrics>,
        billable_usage: Option<&CanonicalUsageBuckets>,
    ) {
        self.requests = self.requests.saturating_add(1);
        self.duration_ms_total = self.duration_ms_total.saturating_add(duration_ms);
        let Some(usage) = usage else {
            return;
        };
        self.output_tokens = self
            .output_tokens
            .saturating_add(usage.output_tokens.max(0));
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(usage.reasoning_output_tokens_total().max(0));
        let Some(buckets) =
            billable_usage.filter(|buckets| matches!(buckets.status, EconomicsStatus::Complete))
        else {
            return;
        };
        self.input_tokens = self
            .input_tokens
            .saturating_add(buckets.ordinary_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(buckets.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(buckets.cache_write_input_tokens);
        if let Some(total_tokens) = canonical_total_tokens(usage, buckets) {
            self.total_tokens = self.total_tokens.saturating_add(total_tokens);
        }
    }

    pub fn average_duration_ms(&self) -> u64 {
        self.duration_ms_total
            .checked_div(self.requests)
            .unwrap_or(0)
    }

    pub fn summary_line(&self, group_value: &str) -> String {
        format!(
            "{} | {} | {} | {} | {} | {} | {} | {} | {}",
            group_value,
            self.requests,
            self.input_tokens,
            self.output_tokens,
            self.cache_read_input_tokens,
            self.cache_creation_input_tokens,
            self.reasoning_tokens,
            self.total_tokens,
            self.average_duration_ms()
        )
    }
}

#[derive(
    Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum RequestUsageSummaryGroup {
    #[default]
    ProviderEndpoint,
    Provider,
    Model,
    Session,
}

impl RequestUsageSummaryGroup {
    pub const ALL: [Self; 4] = [
        Self::ProviderEndpoint,
        Self::Provider,
        Self::Model,
        Self::Session,
    ];

    pub fn column_name(self) -> &'static str {
        match self {
            Self::ProviderEndpoint => "provider_endpoint_key",
            Self::Provider => "provider_id",
            Self::Model => "model",
            Self::Session => "session_id",
        }
    }

    pub(crate) fn key(self, request: &FinishedRequest) -> String {
        match self {
            Self::ProviderEndpoint => final_route_provider_endpoint(request)
                .map(|provider_endpoint| provider_endpoint.stable_key())
                .unwrap_or_else(|| "-".to_string()),
            Self::Provider => display_value(final_route_provider_id(request)),
            Self::Model => display_value(request.model.as_deref()),
            Self::Session => display_value(request.session_id.as_deref()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RequestUsageSummaryRow {
    pub group_value: String,
    pub aggregate: RequestUsageAggregate,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RequestUsageSummaryCoverage {
    pub source: String,
    pub first_terminal_at_ms: Option<u64>,
    pub last_terminal_at_ms: Option<u64>,
    pub requests: u64,
    pub all_history: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RequestUsageSummary {
    pub group: RequestUsageSummaryGroup,
    pub coverage: RequestUsageSummaryCoverage,
    pub rows: Vec<RequestUsageSummaryRow>,
}

pub(crate) fn sort_usage_summary_rows(rows: &mut Vec<RequestUsageSummaryRow>, limit: usize) {
    rows.sort_by(|left, right| {
        right
            .aggregate
            .total_tokens
            .cmp(&left.aggregate.total_tokens)
            .then_with(|| left.group_value.cmp(&right.group_value))
    });
    rows.truncate(limit);
}

/// Formats one typed terminal without consulting mutable price configuration.
pub fn format_finished_request_lines(request: &FinishedRequest) -> Vec<String> {
    let provider_endpoint = final_route_provider_endpoint(request)
        .map(|provider_endpoint| provider_endpoint.stable_key())
        .unwrap_or_else(|| "-".to_string());
    let observability = request.observability_view();
    let mut lines = vec![format!(
        "[{}] {} {} {} status={} provider_endpoint_key={} provider={} model={} tier={}",
        request.ended_at_ms,
        request.service,
        request.method,
        request.path,
        request.status_code,
        provider_endpoint,
        display_value(final_route_provider_id(request)),
        display_value(request.model.as_deref()),
        display_value(request.service_tier.as_deref()),
    )];
    lines.push(format!(
        "    timing duration={}ms ttfb={} output_speed={} cost={}",
        request.duration_ms,
        format_optional_ms(observability.ttfb_ms),
        format_optional_speed(observability.output_tokens_per_second),
        request.cost.display_total_with_confidence(),
    ));
    if let Some(usage) = request.usage.as_ref() {
        lines.push(format!(
            "    tokens input={} output={} cache_read={} cache_create={} reasoning={} total={}",
            usage.input_tokens.max(0),
            usage.output_tokens.max(0),
            usage.cache_read_tokens_total().max(0),
            usage.cache_creation_tokens_total().max(0),
            usage.reasoning_output_tokens_total().max(0),
            usage.total_tokens.max(0),
        ));
    } else {
        lines.push("    tokens -".to_string());
    }

    let mut signal_codes = request
        .provider_signals
        .iter()
        .map(|signal| signal.stable_code().to_string())
        .collect::<Vec<_>>();
    let mut action_codes = request
        .policy_actions
        .iter()
        .map(|action| action.stable_code().to_string())
        .collect::<Vec<_>>();
    if let Some(retry) = request.retry.as_ref() {
        for attempt in &retry.route_attempts {
            signal_codes.extend(
                attempt
                    .provider_signals
                    .iter()
                    .map(|signal| signal.stable_code().to_string()),
            );
            action_codes.extend(
                attempt
                    .policy_actions
                    .iter()
                    .map(|action| action.stable_code().to_string()),
            );
        }
    }
    dedup(&mut signal_codes);
    dedup(&mut action_codes);
    if !signal_codes.is_empty() || !action_codes.is_empty() {
        lines.push(format!(
            "    provider_control signals={} actions={}",
            list_or_dash(&signal_codes),
            list_or_dash(&action_codes),
        ));
    }
    lines
}

fn canonical_total_tokens(usage: &UsageMetrics, buckets: &CanonicalUsageBuckets) -> Option<i64> {
    buckets
        .ordinary_input_tokens
        .checked_add(buckets.cache_read_input_tokens)
        .and_then(|value| value.checked_add(buckets.cache_write_input_tokens))
        .and_then(|value| value.checked_add(usage.output_tokens))
}

fn display_value(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn format_optional_ms(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_optional_speed(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| format!("{value:.2} tok/s"))
        .unwrap_or_else(|| "-".to_string())
}

fn dedup(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn list_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{CostBreakdown, CostConfidence};
    use crate::runtime_identity::ProviderEndpointKey;
    use crate::state::{RequestObservability, RouteDecisionProvenance};

    fn finished_request() -> FinishedRequest {
        let mut cost = CostBreakdown::unknown();
        cost.total_cost_usd = Some("1.25".to_string());
        cost.confidence = CostConfidence::Exact;
        FinishedRequest {
            id: 7,
            trace_id: Some("trace-7".to_string()),
            session_id: Some("session-7".to_string()),
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: Some("priority".to_string()),
            provider_id: Some("legacy-top-level-provider".to_string()),
            route_decision: Some(RouteDecisionProvenance {
                provider_id: Some("provider-a".to_string()),
                endpoint_id: Some("endpoint-a".to_string()),
                ..RouteDecisionProvenance::default()
            }),
            usage: Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 2,
                total_tokens: 12,
                ..UsageMetrics::default()
            }),
            cost,
            accounting: crate::state::RequestAccountingFacts::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 100,
            ttfb_ms: Some(20),
            streaming: false,
            ended_at_ms: 1_000,
        }
    }

    #[test]
    fn terminal_formatter_uses_frozen_cost() {
        let lines = format_finished_request_lines(&finished_request());

        assert!(lines[1].contains("cost=$1.25 (exact)"));
    }

    #[test]
    fn terminal_formatter_uses_only_canonical_final_route_identity() {
        let lines = format_finished_request_lines(&finished_request());

        assert!(lines[0].contains("provider_endpoint_key=codex/provider-a/endpoint-a"));
        assert!(lines[0].contains("provider=provider-a"));
        assert!(!lines[0].contains("legacy-top-level-provider"));
        assert!(!lines[0].contains("station="));
    }

    #[test]
    fn usage_summary_groups_by_canonical_provider_endpoint() {
        let request = finished_request();

        assert_eq!(
            RequestUsageSummaryGroup::ProviderEndpoint.key(&request),
            "codex/provider-a/endpoint-a"
        );
        assert_eq!(
            RequestUsageSummaryGroup::Provider.key(&request),
            "provider-a"
        );
        assert_eq!(
            RequestUsageSummaryGroup::ProviderEndpoint.column_name(),
            "provider_endpoint_key"
        );
        assert!(serde_json::from_str::<RequestUsageSummaryGroup>("\"station\"").is_err());
    }

    #[test]
    fn typed_endpoint_filter_preserves_structured_identity() {
        let provider_endpoint = ProviderEndpointKey::new("codex", "provider-a", "endpoint-a");
        let filters = RequestLogFilters {
            provider_endpoint: Some(provider_endpoint.clone()),
            ..RequestLogFilters::default()
        };

        assert_eq!(
            filters.committed_filter().provider_endpoint,
            Some(provider_endpoint)
        );
    }

    #[test]
    fn legacy_json_identity_fields_are_ignored_without_fallback() {
        let mut request = finished_request();
        request.route_decision = None;
        let mut value = serde_json::to_value(request).expect("serialize terminal fixture");
        let object = value
            .as_object_mut()
            .expect("terminal fixture is an object");
        object.insert(
            "station_name".to_string(),
            serde_json::json!("legacy-station"),
        );
        object.insert(
            "upstream_base_url".to_string(),
            serde_json::json!("https://legacy.example/v1"),
        );
        object.insert(
            "provider_endpoint_key".to_string(),
            serde_json::json!("codex/forged/opaque"),
        );
        let decoded: FinishedRequest =
            serde_json::from_value(value).expect("ignore removed JSON identity fields");

        let lines = format_finished_request_lines(&decoded);
        assert!(lines[0].contains("provider_endpoint_key=-"));
        assert!(lines[0].contains("provider=-"));
        assert!(!lines[0].contains("legacy-station"));
        assert!(!lines[0].contains("legacy.example"));
        assert!(!lines[0].contains("forged"));
    }

    #[test]
    fn typed_usage_aggregate_preserves_totals() {
        let request = finished_request();
        let billable_usage = request.usage.as_ref().map(|usage| {
            usage.canonical_usage_buckets(crate::usage::CacheAccountingConvention::SEPARATE)
        });
        let mut aggregate = RequestUsageAggregate::default();

        aggregate.record_request(&request, billable_usage.as_ref());

        assert_eq!(aggregate.requests, 1);
        assert_eq!(aggregate.input_tokens, 10);
        assert_eq!(aggregate.output_tokens, 2);
        assert_eq!(aggregate.total_tokens, 12);
        assert_eq!(aggregate.average_duration_ms(), 100);
    }

    #[test]
    fn cache_write_is_not_counted_again_as_ordinary_input() {
        let mut request = finished_request();
        request.usage = Some(UsageMetrics {
            input_tokens: 1_000,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 200,
            ..UsageMetrics::default()
        });
        let billable_usage = request.usage.as_ref().map(|usage| {
            usage
                .canonical_usage_buckets(crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT)
        });
        let mut aggregate = RequestUsageAggregate::default();

        aggregate.record_request(&request, billable_usage.as_ref());

        assert_eq!(aggregate.input_tokens, 700);
        assert_eq!(aggregate.cache_read_input_tokens, 100);
        assert_eq!(aggregate.cache_creation_input_tokens, 200);
        assert_eq!(aggregate.total_tokens, 1_000);
    }

    #[test]
    fn frozen_convention_controls_identical_service_usage() {
        let mut request = finished_request();
        request.usage = Some(UsageMetrics {
            input_tokens: 1_000,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 200,
            ..UsageMetrics::default()
        });
        let usage = request.usage.as_ref().expect("usage");
        let included = usage
            .canonical_usage_buckets(crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT);
        let separate =
            usage.canonical_usage_buckets(crate::usage::CacheAccountingConvention::SEPARATE);
        let mut included_aggregate = RequestUsageAggregate::default();
        let mut separate_aggregate = RequestUsageAggregate::default();

        included_aggregate.record_request(&request, Some(&included));
        separate_aggregate.record_request(&request, Some(&separate));

        assert_eq!(included_aggregate.total_tokens, 1_000);
        assert_eq!(separate_aggregate.total_tokens, 1_300);
    }
}
