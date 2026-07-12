use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::pricing::{CostBreakdown, CostSummary, UsdAmount};
use crate::runtime_identity::ProviderEndpointKey;
use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
use crate::state::{AccountingPoolMembership, AccountingPriceCoverage, FinishedRequest};
use crate::usage::UsageMetrics;

pub const MINUTE_MS: u64 = 60_000;
pub const DAY_MS: u64 = 24 * 60 * MINUTE_MS;
pub const DEFAULT_ATTRIBUTION_RETENTION_MS: u64 = 35 * DAY_MS;
pub const DEFAULT_ATTRIBUTION_MAX_BUCKETS: usize = 200_000;
pub const DEFAULT_ATTRIBUTION_MAX_TRACES: usize = 500_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[serde(default)]
pub struct AttributionPoolKey {
    pub pool_key: String,
    pub revision: u64,
}

impl AttributionPoolKey {
    fn from_membership(membership: &AccountingPoolMembership) -> Option<Self> {
        membership.is_reconciliation_eligible().then(|| Self {
            pool_key: membership.pool_key.clone(),
            revision: membership.revision,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[serde(default)]
pub struct AttributionBucketKey {
    pub bucket_start_ms: u64,
    pub service_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint: Option<ProviderEndpointKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<AttributionPoolKey>,
    pub project: ProjectIdentity,
    pub price_coverage: AccountingPriceCoverage,
}

impl AttributionBucketKey {
    pub fn bucket_end_ms(&self) -> u64 {
        self.bucket_start_ms.saturating_add(MINUTE_MS)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct AttributionAggregate {
    pub requests: u64,
    pub usage: UsageMetrics,
    pub cost: CostSummary,
    pub first_ended_at_ms: Option<u64>,
    pub last_ended_at_ms: Option<u64>,
    pub append_failed_requests: u64,
    pub cost_overflow: bool,
    #[serde(skip)]
    checked_cost_femto_usd: i128,
}

impl AttributionAggregate {
    fn record(&mut self, request: &FinishedRequest) {
        self.requests = self.requests.saturating_add(1);
        if let Some(usage) = request.usage.as_ref() {
            self.usage.add_assign(usage);
        }
        match request.accounting.price_coverage {
            AccountingPriceCoverage::Captured | AccountingPriceCoverage::Reconstructed => {
                self.cost.record_usage_cost(&request.cost);
                if let Some(cost) = request.cost.total_cost_femto_usd().or_else(|| {
                    request
                        .cost
                        .total_cost_usd
                        .as_deref()
                        .and_then(UsdAmount::from_decimal_str)
                        .map(UsdAmount::femto_usd)
                }) {
                    self.record_cost_femto(cost);
                } else {
                    self.cost_overflow = true;
                }
            }
            AccountingPriceCoverage::InvalidCaptured
            | AccountingPriceCoverage::Unpriced
            | AccountingPriceCoverage::Unknown => {
                self.cost.record_usage_cost(&CostBreakdown::unknown());
            }
        }
        self.first_ended_at_ms = Some(
            self.first_ended_at_ms
                .map(|current| current.min(request.ended_at_ms))
                .unwrap_or(request.ended_at_ms),
        );
        self.last_ended_at_ms = Some(
            self.last_ended_at_ms
                .map(|current| current.max(request.ended_at_ms))
                .unwrap_or(request.ended_at_ms),
        );
    }

    fn record_cost_femto(&mut self, cost_femto_usd: i128) {
        if self.cost_overflow {
            return;
        }
        match self.checked_cost_femto_usd.checked_add(cost_femto_usd) {
            Some(total) => self.checked_cost_femto_usd = total,
            None => self.cost_overflow = true,
        }
    }

    pub fn checked_cost_femto_usd(&self) -> Option<i128> {
        (!self.cost_overflow).then_some(self.checked_cost_femto_usd)
    }

    #[cfg(test)]
    pub(crate) fn record_cost_femto_for_test(&mut self, cost_femto_usd: i128) {
        self.record_cost_femto(cost_femto_usd);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct AttributionCoverage {
    pub loaded_first_ms: Option<u64>,
    pub loaded_last_ms: Option<u64>,
    pub queried_first_ms: Option<u64>,
    pub queried_last_ms: Option<u64>,
    pub replay_in_progress: bool,
    pub replay_scanned_lines: usize,
    pub replay_max_lines: usize,
    pub replay_max_bytes: usize,
    pub bytes_truncated: bool,
    pub lines_truncated: bool,
    pub time_truncated: bool,
    pub count_truncated: bool,
    pub dedupe_truncated: bool,
    pub boundary_partial: bool,
    pub leading_boundary_partial: bool,
    pub trailing_boundary_partial: bool,
    pub cost_overflow: bool,
    pub duplicate_requests: u64,
    pub append_failed_requests: u64,
    pub partial_captured_price_requests: u64,
    pub reconstructed_price_requests: u64,
    pub invalid_captured_price_requests: u64,
    pub unpriced_requests: u64,
    pub unmatched_endpoint_requests: u64,
    pub unmatched_pool_requests: u64,
    pub unknown_project_requests: u64,
}

impl AttributionCoverage {
    pub fn complete_for_reconciliation(&self) -> bool {
        !self.replay_in_progress
            && !self.bytes_truncated
            && !self.lines_truncated
            && !self.time_truncated
            && !self.count_truncated
            && !self.dedupe_truncated
            && !self.leading_boundary_partial
            && !self.trailing_boundary_partial
            && self.append_failed_requests == 0
            && self.partial_captured_price_requests == 0
            && self.reconstructed_price_requests == 0
            && self.invalid_captured_price_requests == 0
            && self.unpriced_requests == 0
            && self.unmatched_endpoint_requests == 0
            && self.unmatched_pool_requests == 0
            && !self.cost_overflow
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct AttributionQuery {
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint: Option<ProviderEndpointKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_endpoints: Vec<ProviderEndpointKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<AttributionPoolKey>,
}

impl AttributionQuery {
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self {
            start_ms,
            end_ms,
            ..Self::default()
        }
    }

    pub fn for_service(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = Some(service_name.into());
        self
    }

    pub fn for_pool(mut self, pool: AttributionPoolKey) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn for_endpoints(
        mut self,
        endpoints: impl IntoIterator<Item = ProviderEndpointKey>,
    ) -> Self {
        self.provider_endpoints = endpoints.into_iter().collect();
        self.provider_endpoints.sort();
        self.provider_endpoints.dedup();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct AttributionBucket {
    pub key: AttributionBucketKey,
    pub aggregate: AttributionAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct AttributionQueryResult {
    pub start_ms: u64,
    pub end_ms: u64,
    pub covered_start_ms: u64,
    pub covered_end_ms: u64,
    pub rows: Vec<AttributionBucket>,
    pub coverage: AttributionCoverage,
}

impl AttributionQueryResult {
    pub fn checked_total_femto_usd(&self) -> Option<i128> {
        self.rows.iter().try_fold(0_i128, |total, row| {
            total.checked_add(row.aggregate.checked_cost_femto_usd()?)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttributionInsert {
    Inserted,
    Duplicate,
}

#[derive(Debug)]
pub(super) struct AttributionIndex {
    retention_ms: u64,
    max_buckets: usize,
    max_traces: usize,
    buckets: BTreeMap<AttributionBucketKey, AttributionAggregate>,
    seen_traces: HashMap<String, u64>,
    trace_order: VecDeque<(u64, String)>,
    append_failure_traces: HashSet<String>,
    duplicate_events: VecDeque<u64>,
    latest_ended_at_ms: Option<u64>,
    loaded_first_ms: Option<u64>,
    loaded_last_ms: Option<u64>,
    replay_tasks: usize,
    replay_scanned_lines: usize,
    replay_max_lines: usize,
    replay_max_bytes: usize,
    replay_bytes_truncated: bool,
    replay_lines_truncated: bool,
    time_truncated: bool,
    count_truncated: bool,
    dedupe_truncated: bool,
}

impl Default for AttributionIndex {
    fn default() -> Self {
        Self::new(
            DEFAULT_ATTRIBUTION_RETENTION_MS,
            DEFAULT_ATTRIBUTION_MAX_BUCKETS,
            DEFAULT_ATTRIBUTION_MAX_TRACES,
        )
    }
}

impl AttributionIndex {
    pub(super) fn new(retention_ms: u64, max_buckets: usize, max_traces: usize) -> Self {
        Self {
            retention_ms: retention_ms.max(DAY_MS),
            max_buckets: max_buckets.max(1),
            max_traces: max_traces.max(1),
            buckets: BTreeMap::new(),
            seen_traces: HashMap::new(),
            trace_order: VecDeque::new(),
            append_failure_traces: HashSet::new(),
            duplicate_events: VecDeque::new(),
            latest_ended_at_ms: None,
            loaded_first_ms: None,
            loaded_last_ms: None,
            replay_tasks: 0,
            replay_scanned_lines: 0,
            replay_max_lines: 0,
            replay_max_bytes: 0,
            replay_bytes_truncated: false,
            replay_lines_truncated: false,
            time_truncated: false,
            count_truncated: false,
            dedupe_truncated: false,
        }
    }

    pub(super) fn record(&mut self, request: &FinishedRequest) -> AttributionInsert {
        let trace_key = request_replay_key(request);
        if self.seen_traces.contains_key(&trace_key) {
            self.duplicate_events.push_back(request.ended_at_ms);
            self.prune(request.ended_at_ms);
            return AttributionInsert::Duplicate;
        }

        let key = bucket_key(request);
        self.buckets.entry(key).or_default().record(request);
        self.seen_traces
            .insert(trace_key.clone(), request.ended_at_ms);
        self.trace_order.push_back((request.ended_at_ms, trace_key));
        self.latest_ended_at_ms = Some(
            self.latest_ended_at_ms
                .map(|current| current.max(request.ended_at_ms))
                .unwrap_or(request.ended_at_ms),
        );
        self.prune(request.ended_at_ms);
        AttributionInsert::Inserted
    }

    pub(super) fn mark_append_failure(&mut self, request: &FinishedRequest) {
        let trace_key = request_replay_key(request);
        if !self.append_failure_traces.insert(trace_key) {
            return;
        }
        if let Some(aggregate) = self.buckets.get_mut(&bucket_key(request)) {
            aggregate.append_failed_requests = aggregate.append_failed_requests.saturating_add(1);
        }
    }

    pub(super) fn begin_replay(&mut self) {
        self.replay_tasks = self.replay_tasks.saturating_add(1);
    }

    pub(super) fn finish_replay(
        &mut self,
        scanned_lines: usize,
        max_lines: usize,
        max_bytes: usize,
        bytes_truncated: bool,
        lines_truncated: bool,
    ) {
        self.replay_tasks = self.replay_tasks.saturating_sub(1);
        self.replay_scanned_lines = self.replay_scanned_lines.saturating_add(scanned_lines);
        self.replay_max_lines = self.replay_max_lines.max(max_lines);
        self.replay_max_bytes = self.replay_max_bytes.max(max_bytes);
        self.replay_bytes_truncated |= bytes_truncated;
        self.replay_lines_truncated |= lines_truncated;
    }

    pub(super) fn query(&self, query: AttributionQuery) -> AttributionQueryResult {
        let leading_boundary_partial = !query.start_ms.is_multiple_of(MINUTE_MS);
        let trailing_boundary_partial = !query.end_ms.is_multiple_of(MINUTE_MS);
        let boundary_partial = leading_boundary_partial || trailing_boundary_partial;
        let full_start = ceil_to_minute(query.start_ms);
        let full_end = floor_to_minute(query.end_ms);
        let window_rows = self
            .buckets
            .iter()
            .filter(|(key, _)| {
                query.start_ms < query.end_ms
                    && full_start < full_end
                    && key.bucket_start_ms >= full_start
                    && key.bucket_end_ms() <= full_end
                    && query
                        .service_name
                        .as_ref()
                        .is_none_or(|service| &key.service_name == service)
            })
            .collect::<Vec<_>>();
        let coverage_rows = window_rows
            .iter()
            .copied()
            .filter(|(key, _)| key_is_coverage_related(key, &query))
            .collect::<Vec<_>>();
        let rows = window_rows
            .into_iter()
            .filter(|(key, _)| key_is_attributed_to_query(key, &query))
            .map(|(key, aggregate)| AttributionBucket {
                key: key.clone(),
                aggregate: aggregate.clone(),
            })
            .collect::<Vec<_>>();

        let queried_first_ms = rows
            .iter()
            .filter_map(|row| row.aggregate.first_ended_at_ms)
            .min();
        let queried_last_ms = rows
            .iter()
            .filter_map(|row| row.aggregate.last_ended_at_ms)
            .max();
        let mut coverage = AttributionCoverage {
            loaded_first_ms: self.loaded_first_ms,
            loaded_last_ms: self.loaded_last_ms,
            queried_first_ms,
            queried_last_ms,
            replay_in_progress: self.replay_tasks > 0,
            replay_scanned_lines: self.replay_scanned_lines,
            replay_max_lines: self.replay_max_lines,
            replay_max_bytes: self.replay_max_bytes,
            bytes_truncated: self.replay_bytes_truncated,
            lines_truncated: self.replay_lines_truncated,
            time_truncated: self.time_truncated
                && self
                    .loaded_first_ms
                    .is_none_or(|loaded_first| query.start_ms < loaded_first),
            count_truncated: self.count_truncated,
            dedupe_truncated: self.dedupe_truncated,
            boundary_partial,
            leading_boundary_partial,
            trailing_boundary_partial,
            duplicate_requests: self
                .duplicate_events
                .iter()
                .filter(|ended_at| **ended_at >= query.start_ms && **ended_at < query.end_ms)
                .count() as u64,
            ..AttributionCoverage::default()
        };
        for (key, aggregate) in coverage_rows {
            coverage.append_failed_requests = coverage
                .append_failed_requests
                .saturating_add(aggregate.append_failed_requests);
            coverage.cost_overflow |= aggregate.cost_overflow;
            match key.price_coverage {
                AccountingPriceCoverage::Reconstructed => {
                    coverage.reconstructed_price_requests = coverage
                        .reconstructed_price_requests
                        .saturating_add(aggregate.requests);
                }
                AccountingPriceCoverage::InvalidCaptured => {
                    coverage.invalid_captured_price_requests = coverage
                        .invalid_captured_price_requests
                        .saturating_add(aggregate.requests);
                }
                AccountingPriceCoverage::Unpriced | AccountingPriceCoverage::Unknown => {
                    coverage.unpriced_requests = coverage
                        .unpriced_requests
                        .saturating_add(aggregate.requests);
                }
                AccountingPriceCoverage::Captured => {
                    coverage.partial_captured_price_requests = coverage
                        .partial_captured_price_requests
                        .saturating_add(aggregate.cost.partial_requests);
                }
            }
            if key.provider_endpoint.is_none() {
                coverage.unmatched_endpoint_requests = coverage
                    .unmatched_endpoint_requests
                    .saturating_add(aggregate.requests);
            }
            if key.pool.is_none() {
                coverage.unmatched_pool_requests = coverage
                    .unmatched_pool_requests
                    .saturating_add(aggregate.requests);
            }
            if key.project.kind == ProjectIdentityKind::Unknown {
                coverage.unknown_project_requests = coverage
                    .unknown_project_requests
                    .saturating_add(aggregate.requests);
            }
        }

        AttributionQueryResult {
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            covered_start_ms: full_start,
            covered_end_ms: full_end,
            rows,
            coverage,
        }
    }

    fn prune(&mut self, observed_at_ms: u64) {
        let latest = self
            .latest_ended_at_ms
            .map(|current| current.max(observed_at_ms))
            .unwrap_or(observed_at_ms);
        let cutoff = latest.saturating_sub(self.retention_ms);

        while self
            .buckets
            .first_key_value()
            .is_some_and(|(key, _)| key.bucket_end_ms() <= cutoff)
        {
            self.buckets.pop_first();
            self.time_truncated = true;
        }
        while self.buckets.len() > self.max_buckets {
            self.buckets.pop_first();
            self.count_truncated = true;
        }

        while self
            .trace_order
            .front()
            .is_some_and(|(timestamp, _)| *timestamp < cutoff)
        {
            self.remove_oldest_trace(false);
        }
        while self.trace_order.len() > self.max_traces {
            self.remove_oldest_trace(true);
        }
        while self
            .duplicate_events
            .front()
            .is_some_and(|timestamp| *timestamp < cutoff)
        {
            self.duplicate_events.pop_front();
        }
        while self.duplicate_events.len() > self.max_traces {
            self.duplicate_events.pop_front();
            self.dedupe_truncated = true;
        }
        self.append_failure_traces
            .retain(|trace| self.seen_traces.contains_key(trace));
        self.refresh_loaded_bounds();
    }

    fn remove_oldest_trace(&mut self, count_truncated: bool) {
        let Some((timestamp, trace)) = self.trace_order.pop_front() else {
            return;
        };
        if self.seen_traces.get(&trace) == Some(&timestamp) {
            self.seen_traces.remove(&trace);
            self.append_failure_traces.remove(&trace);
        }
        self.dedupe_truncated |= count_truncated;
    }

    fn refresh_loaded_bounds(&mut self) {
        self.loaded_first_ms = self
            .buckets
            .values()
            .filter_map(|aggregate| aggregate.first_ended_at_ms)
            .min();
        self.loaded_last_ms = self
            .buckets
            .values()
            .filter_map(|aggregate| aggregate.last_ended_at_ms)
            .max();
    }
}

fn endpoint_is_selected(endpoint: Option<&ProviderEndpointKey>, query: &AttributionQuery) -> bool {
    if let Some(selected) = query.provider_endpoint.as_ref() {
        return endpoint == Some(selected);
    }
    query.provider_endpoints.is_empty()
        || endpoint.is_some_and(|endpoint| query.provider_endpoints.binary_search(endpoint).is_ok())
}

fn key_is_attributed_to_query(key: &AttributionBucketKey, query: &AttributionQuery) -> bool {
    if !endpoint_is_selected(key.provider_endpoint.as_ref(), query) {
        return false;
    }
    query
        .pool
        .as_ref()
        .is_none_or(|pool| key.pool.as_ref() == Some(pool))
}

fn key_is_coverage_related(key: &AttributionBucketKey, query: &AttributionQuery) -> bool {
    let endpoint_related = endpoint_is_selected(key.provider_endpoint.as_ref(), query)
        || key.provider_endpoint.is_none();
    let Some(pool) = query.pool.as_ref() else {
        return endpoint_related;
    };
    key.pool.as_ref() == Some(pool) || (key.pool.is_none() && endpoint_related)
}

pub(super) fn request_replay_key(request: &FinishedRequest) -> String {
    match request.trace_id.as_deref() {
        Some(trace) if crate::logging::is_versioned_request_trace_id(trace) => {
            format!("trace|{trace}")
        }
        Some(trace)
            if trace == crate::logging::legacy_request_trace_id(&request.service, request.id) =>
        {
            format!("legacy|{trace}|{}|{}", request.ended_at_ms, request.id)
        }
        Some(trace) => format!("trace|{trace}"),
        None => format!("fallback|{}|{}", request.ended_at_ms, request.id),
    }
}

fn bucket_key(request: &FinishedRequest) -> AttributionBucketKey {
    AttributionBucketKey {
        bucket_start_ms: floor_to_minute(request.ended_at_ms),
        service_name: request.service.clone(),
        provider_endpoint: request.accounting.provider_endpoint.clone(),
        pool: request
            .accounting
            .pool_membership
            .as_ref()
            .and_then(AttributionPoolKey::from_membership),
        project: request.accounting.project.clone(),
        price_coverage: request.accounting.price_coverage,
    }
}

fn floor_to_minute(timestamp_ms: u64) -> u64 {
    timestamp_ms / MINUTE_MS * MINUTE_MS
}

fn ceil_to_minute(timestamp_ms: u64) -> u64 {
    if timestamp_ms.is_multiple_of(MINUTE_MS) {
        timestamp_ms
    } else {
        floor_to_minute(timestamp_ms).saturating_add(MINUTE_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::CostConfidence;
    use crate::quota_pool::IdentityConfidence;
    use crate::runtime_identity::ProviderEndpointKey;
    use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
    use crate::state::{
        AccountingPoolMembership, AccountingPriceCoverage, FinishedRequest, RequestAccountingFacts,
        RequestObservability,
    };

    fn request(trace_id: &str, ended_at_ms: u64, project: &str) -> FinishedRequest {
        FinishedRequest {
            id: ended_at_ms,
            trace_id: Some(trace_id.to_string()),
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: Some(project.to_string()),
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: None,
            station_name: Some("relay".to_string()),
            provider_id: Some("input20".to_string()),
            upstream_base_url: Some("https://relay.example/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: serde_json::from_value(serde_json::json!({
                "input_cost_usd": "1",
                "total_cost_usd": "1",
                "confidence": CostConfidence::Estimated,
                "pricing_source": "test",
                "pricing_provider": "openai",
                "pricing_generation": "price-row-1",
                "effective_pricing_revision": "catalog-1"
            }))
            .expect("test cost"),
            accounting: RequestAccountingFacts {
                schema_version: 1,
                project: ProjectIdentity {
                    kind: ProjectIdentityKind::GitRoot,
                    path: Some(project.to_string()),
                },
                provider_endpoint: Some(ProviderEndpointKey::new("codex", "input20", "default")),
                pool_membership: Some(AccountingPoolMembership {
                    pool_key: "pool-a".to_string(),
                    revision: 3,
                    confidence: IdentityConfidence::High,
                    aggregation_eligible: true,
                }),
                price_coverage: AccountingPriceCoverage::Captured,
            },
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms,
        }
    }

    #[test]
    fn trace_replay_is_idempotent_and_keeps_captured_pool_revision() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        let record = request("trace-1", 120_001, "C:/src/repo");

        assert_eq!(index.record(&record), AttributionInsert::Inserted);
        assert_eq!(index.record(&record), AttributionInsert::Duplicate);

        let result = index.query(AttributionQuery::new(120_000, 180_000));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].aggregate.requests, 1);
        assert_eq!(
            result.rows[0]
                .key
                .pool
                .as_ref()
                .map(|pool| (pool.pool_key.as_str(), pool.revision)),
            Some(("pool-a", 3))
        );
        assert_eq!(result.coverage.duplicate_requests, 1);
    }

    #[test]
    fn legacy_trace_ids_only_dedupe_the_same_completed_event() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        let mut first = request("codex-1", 120_001, "C:/src/repo");
        first.id = 1;
        let mut after_restart = request("codex-1", 180_001, "C:/src/repo");
        after_restart.id = 1;

        assert_eq!(index.record(&first), AttributionInsert::Inserted);
        assert_eq!(index.record(&first), AttributionInsert::Duplicate);
        assert_eq!(index.record(&after_restart), AttributionInsert::Inserted);

        let result = index.query(AttributionQuery::new(120_000, 240_000));
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.aggregate.requests)
                .sum::<u64>(),
            2
        );
        assert_eq!(result.coverage.duplicate_requests, 1);
    }

    #[test]
    fn retention_and_count_bounds_lower_query_coverage() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 2, 3);
        index.record(&request("trace-old", 1, "C:/src/old"));
        index.record(&request("trace-a", 36 * DAY_MS, "C:/src/a"));
        index.record(&request("trace-b", 36 * DAY_MS + MINUTE_MS, "C:/src/b"));
        index.record(&request("trace-c", 36 * DAY_MS + 2 * MINUTE_MS, "C:/src/c"));

        let result = index.query(AttributionQuery::new(0, 37 * DAY_MS));
        assert!(result.coverage.time_truncated);
        assert!(result.coverage.count_truncated);
        assert!(result.rows.len() <= 2);
    }

    #[test]
    fn replay_and_append_failures_are_explicit_coverage() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        let record = request("trace-1", 120_001, "C:/src/repo");
        index.begin_replay();
        index.record(&record);
        index.mark_append_failure(&record);

        let during = index.query(AttributionQuery::new(120_000, 180_000));
        assert!(during.coverage.replay_in_progress);
        assert_eq!(during.coverage.append_failed_requests, 1);

        index.finish_replay(20, 100, 1_024, true, false);
        let after = index.query(AttributionQuery::new(120_000, 180_000));
        assert!(!after.coverage.replay_in_progress);
        assert!(after.coverage.bytes_truncated);
    }

    #[test]
    fn partial_minute_boundaries_are_excluded_instead_of_over_attributed() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        index.record(&request("trace-1", 120_001, "C:/src/repo"));

        let result = index.query(AttributionQuery::new(120_002, 180_000));

        assert!(result.rows.is_empty());
        assert!(result.coverage.leading_boundary_partial);
        assert!(!result.coverage.trailing_boundary_partial);
        assert!(!result.coverage.complete_for_reconciliation());
    }

    #[test]
    fn trailing_realtime_bucket_invalidates_exact_reconciliation() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        index.record(&request("trace-1", 120_001, "C:/src/repo"));

        let result = index.query(AttributionQuery::new(120_000, 180_001));

        assert_eq!(result.rows.len(), 1);
        assert!(!result.coverage.leading_boundary_partial);
        assert!(result.coverage.trailing_boundary_partial);
        assert!(!result.coverage.complete_for_reconciliation());
        assert_eq!(result.covered_end_ms, 180_000);
    }

    #[test]
    fn partial_captured_cost_keeps_known_amount_but_invalidates_reconciliation() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        let mut record = request("trace-partial", 120_001, "C:/src/repo");
        record.cost.confidence = CostConfidence::Partial;

        index.record(&record);
        let result = index.query(AttributionQuery::new(120_000, 180_000));

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].aggregate.cost.total_cost_usd.as_deref(),
            Some("1")
        );
        assert_eq!(result.rows[0].aggregate.cost.partial_requests, 1);
        assert_eq!(result.coverage.partial_captured_price_requests, 1);
        assert!(!result.coverage.complete_for_reconciliation());
    }

    #[test]
    fn selected_pool_coverage_includes_endpoint_only_and_unknown_endpoint_gaps() {
        let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
        index.record(&request("captured", 120_001, "C:/src/repo"));
        let mut endpoint_only = request("endpoint-only", 120_002, "C:/src/repo");
        endpoint_only.accounting.pool_membership = None;
        index.record(&endpoint_only);
        let mut unknown_endpoint = request("unknown-endpoint", 120_003, "C:/src/repo");
        unknown_endpoint.accounting.provider_endpoint = None;
        unknown_endpoint.accounting.pool_membership = None;
        index.record(&unknown_endpoint);

        let result = index.query(
            AttributionQuery::new(120_000, 180_000)
                .for_service("codex")
                .for_pool(AttributionPoolKey {
                    pool_key: "pool-a".to_string(),
                    revision: 3,
                })
                .for_endpoints([ProviderEndpointKey::new("codex", "input20", "default")]),
        );

        assert_eq!(
            result.rows.len(),
            1,
            "only captured pool rows are attributed"
        );
        assert_eq!(result.coverage.unmatched_pool_requests, 2);
        assert_eq!(result.coverage.unmatched_endpoint_requests, 1);
        assert!(!result.coverage.complete_for_reconciliation());
    }

    #[test]
    fn reconstructed_and_unpriced_rows_never_claim_complete_reconciliation() {
        for price_coverage in [
            AccountingPriceCoverage::Reconstructed,
            AccountingPriceCoverage::Unpriced,
        ] {
            let mut index = AttributionIndex::new(35 * DAY_MS, 100, 100);
            let mut record = request("trace-1", 120_001, "C:/src/repo");
            record.accounting.price_coverage = price_coverage;
            index.record(&record);

            let result = index.query(AttributionQuery::new(120_000, 180_000));
            assert!(!result.coverage.complete_for_reconciliation());
        }
    }

    #[test]
    fn checked_cost_sum_exposes_overflow_instead_of_saturating_for_reconciliation() {
        let mut aggregate = AttributionAggregate::default();

        aggregate.record_cost_femto_for_test(i128::MAX);
        aggregate.record_cost_femto_for_test(1);

        assert!(aggregate.cost_overflow);
        assert_eq!(aggregate.checked_cost_femto_usd(), None);
    }
}
