use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::pricing::{CostConfidence, UsdAmount};
use crate::routing_explain::{RoutingExplainCandidate, RoutingExplainResponse};
use crate::state::{
    BalanceSnapshotStatus, FinishedRequest, ProviderBalanceSnapshot, UsageBucket, UsageRollupView,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum UsageBalanceStatus {
    #[default]
    Unknown,
    Ok,
    Unlimited,
    Exhausted,
    Stale,
    Error,
}

impl UsageBalanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UsageBalanceStatus::Unknown => "unknown",
            UsageBalanceStatus::Ok => "ok",
            UsageBalanceStatus::Unlimited => "unlimited",
            UsageBalanceStatus::Exhausted => "exhausted",
            UsageBalanceStatus::Stale => "stale",
            UsageBalanceStatus::Error => "error",
        }
    }

    pub fn is_attention(self) -> bool {
        matches!(
            self,
            UsageBalanceStatus::Unknown
                | UsageBalanceStatus::Exhausted
                | UsageBalanceStatus::Stale
                | UsageBalanceStatus::Error
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageBalanceStatusCounts {
    pub ok: usize,
    pub unlimited: usize,
    pub exhausted: usize,
    pub stale: usize,
    pub error: usize,
    pub unknown: usize,
}

impl UsageBalanceStatusCounts {
    pub fn total(&self) -> usize {
        self.ok
            .saturating_add(self.unlimited)
            .saturating_add(self.exhausted)
            .saturating_add(self.stale)
            .saturating_add(self.error)
            .saturating_add(self.unknown)
    }

    pub fn record(&mut self, status: UsageBalanceStatus) {
        match status {
            UsageBalanceStatus::Ok => self.ok += 1,
            UsageBalanceStatus::Unlimited => self.unlimited += 1,
            UsageBalanceStatus::Exhausted => self.exhausted += 1,
            UsageBalanceStatus::Stale => self.stale += 1,
            UsageBalanceStatus::Error => self.error += 1,
            UsageBalanceStatus::Unknown => self.unknown += 1,
        }
    }

    pub fn aggregate_status(&self) -> UsageBalanceStatus {
        if self.total() == 0 {
            UsageBalanceStatus::Unknown
        } else if self.error > 0 {
            UsageBalanceStatus::Error
        } else if self.exhausted > 0 {
            UsageBalanceStatus::Exhausted
        } else if self.stale > 0 {
            UsageBalanceStatus::Stale
        } else if self.unknown > 0 {
            UsageBalanceStatus::Unknown
        } else if self.unlimited > 0 {
            UsageBalanceStatus::Unlimited
        } else {
            UsageBalanceStatus::Ok
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageBalanceView {
    pub service_name: String,
    pub window_days: usize,
    pub generated_at_ms: u64,
    pub totals: UsageBalanceTotals,
    pub provider_rows: Vec<UsageBalanceProviderRow>,
    pub endpoint_rows: Vec<UsageBalanceEndpointRow>,
    pub routing_impacts: Vec<UsageBalanceRouteImpact>,
    pub refresh_status: UsageBalanceRefreshStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageBalanceTotals {
    pub requests_total: u64,
    pub requests_error: u64,
    pub success_per_mille: Option<u16>,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub cost_total_usd: Option<String>,
    pub cost_display: String,
    pub cost_confidence: CostConfidence,
    pub balance_status_counts: UsageBalanceStatusCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageBalanceProviderRow {
    pub provider_id: String,
    pub display_name: String,
    pub usage: UsageBucket,
    pub success_per_mille: Option<u16>,
    pub output_tokens_per_second: Option<u64>,
    pub avg_ttfb_ms: Option<u64>,
    pub cost_total_usd: Option<String>,
    pub cost_display: String,
    pub cost_confidence: CostConfidence,
    pub balance_status: UsageBalanceStatus,
    pub balance_counts: UsageBalanceStatusCounts,
    pub primary_balance: Option<UsageBalanceSnapshotSummary>,
    pub latest_balance_error: Option<String>,
    pub balance_age_ms: Option<u64>,
    pub routing: UsageBalanceRouteImpact,
    pub endpoint_count: usize,
    pub endpoints_with_balance: usize,
    pub recent_endpoint_requests: u64,
}

impl UsageBalanceProviderRow {
    pub fn needs_attention(&self) -> bool {
        self.balance_status.is_attention()
            || self
                .latest_balance_error
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self.usage.requests_error > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageBalanceEndpointRow {
    pub provider_id: String,
    pub endpoint_id: String,
    pub station_name: Option<String>,
    pub upstream_index: Option<usize>,
    pub base_url: Option<String>,
    pub usage: UsageBucket,
    pub balance_status: UsageBalanceStatus,
    pub balance: Option<UsageBalanceSnapshotSummary>,
    pub route_selected: bool,
    pub route_skip_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageBalanceSnapshotSummary {
    pub provider_id: String,
    pub station_name: Option<String>,
    pub upstream_index: Option<usize>,
    pub source: String,
    pub status: UsageBalanceStatus,
    pub amount_summary: String,
    pub fetched_at_ms: u64,
    pub stale_after_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub error: Option<String>,
    pub exhaustion_affects_routing: bool,
    pub routing_exhausted: bool,
    pub routing_ignored_exhaustion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageBalanceRouteImpact {
    pub provider_id: String,
    pub selected: bool,
    pub selected_endpoint_id: Option<String>,
    pub selected_provider_endpoint_key: Option<String>,
    pub candidate_count: usize,
    pub skip_reasons: Vec<String>,
    pub route_paths: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageBalanceRefreshStatus {
    pub refreshing: bool,
    pub total_snapshots: usize,
    pub latest_fetched_at_ms: Option<u64>,
    pub latest_error: Option<String>,
    pub last_message: Option<String>,
    pub last_error: Option<String>,
    pub status_counts: UsageBalanceStatusCounts,
}

#[derive(Debug, Clone, Default)]
pub struct UsageBalanceRefreshInput {
    pub refreshing: bool,
    pub last_message: Option<String>,
    pub last_error: Option<String>,
}

pub struct UsageBalanceBuildInput<'a> {
    pub service_name: &'a str,
    pub window_days: usize,
    pub generated_at_ms: u64,
    pub usage_rollup: &'a UsageRollupView,
    pub provider_balances: &'a HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub recent: &'a [FinishedRequest],
    pub routing_explain: Option<&'a RoutingExplainResponse>,
    pub refresh: UsageBalanceRefreshInput,
}

impl UsageBalanceView {
    pub fn build(input: UsageBalanceBuildInput<'_>) -> Self {
        let route_index = RouteIndex::from_explain(input.routing_explain);
        let mut endpoint_rows = build_endpoint_rows(
            input.recent,
            input.provider_balances,
            &route_index,
            input.generated_at_ms,
        );
        sort_endpoint_rows(&mut endpoint_rows);

        let provider_rows = build_provider_rows(&input, &route_index, &endpoint_rows);
        let totals = build_totals(
            input.usage_rollup,
            input.provider_balances,
            input.generated_at_ms,
        );
        let refresh_status = build_refresh_status(
            input.provider_balances,
            input.generated_at_ms,
            input.refresh,
        );
        let routing_impacts = route_index
            .provider_impacts
            .values()
            .cloned()
            .collect::<Vec<_>>();

        Self {
            service_name: input.service_name.to_string(),
            window_days: input.window_days,
            generated_at_ms: input.generated_at_ms,
            totals,
            provider_rows,
            endpoint_rows,
            routing_impacts,
            refresh_status,
            warnings: Vec::new(),
        }
    }
}

fn build_totals(
    rollup: &UsageRollupView,
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    now_ms: u64,
) -> UsageBalanceTotals {
    let window = &rollup.window;
    let mut balance_status_counts = UsageBalanceStatusCounts::default();
    for snapshot in provider_balances.values().flatten() {
        balance_status_counts.record(classify_snapshot(snapshot, now_ms));
    }

    UsageBalanceTotals {
        requests_total: window.requests_total,
        requests_error: window.requests_error,
        success_per_mille: per_mille(
            window.requests_total.saturating_sub(window.requests_error),
            window.requests_total,
        ),
        input_tokens: window.usage.input_tokens,
        cached_input_tokens: window.usage.cache_read_tokens_total(),
        output_tokens: window.usage.output_tokens,
        reasoning_output_tokens: window.usage.reasoning_output_tokens_total(),
        total_tokens: window.usage.total_tokens,
        cost_total_usd: window.cost.total_cost_usd.clone(),
        cost_display: window.cost.display_total(),
        cost_confidence: window.cost.confidence,
        balance_status_counts,
    }
}

fn build_provider_rows(
    input: &UsageBalanceBuildInput<'_>,
    route_index: &RouteIndex,
    endpoint_rows: &[UsageBalanceEndpointRow],
) -> Vec<UsageBalanceProviderRow> {
    let mut provider_ids = BTreeSet::new();
    for (provider_id, _) in &input.usage_rollup.by_provider {
        provider_ids.insert(provider_id.clone());
    }
    for (provider_id, _) in group_balances_by_provider(input.provider_balances) {
        provider_ids.insert(provider_id);
    }
    for request in input.recent {
        if let Some(provider_id) = request
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            provider_ids.insert(provider_id.to_string());
        }
    }
    for provider_id in route_index.provider_impacts.keys() {
        provider_ids.insert(provider_id.clone());
    }

    let usage_by_provider = input
        .usage_rollup
        .by_provider
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    let balances_by_provider = group_balances_by_provider(input.provider_balances);
    let mut endpoint_counts = BTreeMap::<String, (usize, usize, u64)>::new();
    for endpoint in endpoint_rows {
        let entry = endpoint_counts
            .entry(endpoint.provider_id.clone())
            .or_insert((0, 0, 0));
        entry.0 += 1;
        if endpoint.balance.is_some() {
            entry.1 += 1;
        }
        entry.2 = entry.2.saturating_add(endpoint.usage.requests_total);
    }

    let mut rows = provider_ids
        .into_iter()
        .map(|provider_id| {
            let usage = usage_by_provider
                .get(provider_id.as_str())
                .cloned()
                .unwrap_or_default();
            let balances = balances_by_provider
                .get(provider_id.as_str())
                .cloned()
                .unwrap_or_default();
            let balance_counts = balance_counts_for_snapshots(&balances, input.generated_at_ms);
            let primary_balance = primary_balance_snapshot(&balances, input.generated_at_ms)
                .map(|snapshot| summarize_snapshot(snapshot, input.generated_at_ms));
            let latest_balance_error = latest_balance_error(&balances);
            let balance_age_ms = primary_balance.as_ref().and_then(|summary| summary.age_ms);
            let balance_status = balance_counts.aggregate_status();
            let routing = route_index
                .provider_impacts
                .get(provider_id.as_str())
                .cloned()
                .unwrap_or_else(|| UsageBalanceRouteImpact {
                    provider_id: provider_id.clone(),
                    ..UsageBalanceRouteImpact::default()
                });
            let (endpoint_count, endpoints_with_balance, recent_endpoint_requests) =
                endpoint_counts
                    .get(provider_id.as_str())
                    .copied()
                    .unwrap_or_default();

            UsageBalanceProviderRow {
                display_name: provider_id.clone(),
                provider_id,
                success_per_mille: per_mille(
                    usage.requests_total.saturating_sub(usage.requests_error),
                    usage.requests_total,
                ),
                output_tokens_per_second: output_tokens_per_second(&usage),
                avg_ttfb_ms: (usage.ttfb_samples > 0)
                    .then(|| usage.ttfb_ms_total / usage.ttfb_samples),
                cost_total_usd: usage.cost.total_cost_usd.clone(),
                cost_display: usage.cost.display_total(),
                cost_confidence: usage.cost.confidence,
                usage,
                balance_status,
                balance_counts,
                primary_balance,
                latest_balance_error,
                balance_age_ms,
                routing,
                endpoint_count,
                endpoints_with_balance,
                recent_endpoint_requests,
            }
        })
        .collect::<Vec<_>>();

    sort_provider_rows(&mut rows);
    rows
}

fn build_refresh_status(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    now_ms: u64,
    input: UsageBalanceRefreshInput,
) -> UsageBalanceRefreshStatus {
    let mut out = UsageBalanceRefreshStatus {
        refreshing: input.refreshing,
        last_message: input.last_message,
        last_error: input.last_error,
        ..UsageBalanceRefreshStatus::default()
    };

    for snapshot in provider_balances.values().flatten() {
        out.total_snapshots += 1;
        out.latest_fetched_at_ms = Some(
            out.latest_fetched_at_ms
                .unwrap_or(0)
                .max(snapshot.fetched_at_ms),
        );
        let status = classify_snapshot(snapshot, now_ms);
        out.status_counts.record(status);
        if let Some(error) = snapshot
            .error
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            && out
                .latest_fetched_at_ms
                .is_none_or(|latest| snapshot.fetched_at_ms >= latest)
        {
            out.latest_error = Some(error.to_string());
        }
    }

    out
}

fn build_endpoint_rows(
    recent: &[FinishedRequest],
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    route_index: &RouteIndex,
    now_ms: u64,
) -> Vec<UsageBalanceEndpointRow> {
    let mut acc = BTreeMap::<EndpointKey, EndpointAccum>::new();

    for candidate in route_index.candidates.values() {
        let key = EndpointKey {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
        };
        let row = acc
            .entry(key.clone())
            .or_insert_with(|| EndpointAccum::new(key));
        row.base_url = non_empty(candidate.base_url.clone()).or(row.base_url.take());
        row.route_selected |= candidate.selected;
        row.route_skip_reasons
            .extend(candidate.skip_reasons.iter().cloned());
        if let Some((station_name, upstream_index)) = candidate.compatibility.as_ref() {
            row.station_name = Some(station_name.clone());
            row.upstream_index = Some(*upstream_index);
        }
    }

    for request in recent {
        let Some(provider_id) = request
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let base_url = request
            .upstream_base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let endpoint_id = base_url
            .and_then(|base_url| {
                route_index
                    .endpoint_by_provider_base_url
                    .get(&(provider_id.to_string(), base_url.to_string()))
                    .cloned()
            })
            .unwrap_or_else(|| {
                base_url
                    .map(endpoint_id_from_base_url)
                    .unwrap_or_else(|| "unknown_endpoint".to_string())
            });
        let key = EndpointKey {
            provider_id: provider_id.to_string(),
            endpoint_id,
        };
        let row = acc
            .entry(key.clone())
            .or_insert_with(|| EndpointAccum::new(key));
        if row.base_url.is_none() {
            row.base_url = base_url.map(ToOwned::to_owned);
        }
        record_finished_into_bucket(&mut row.usage, request);
    }

    for (station_name, snapshots) in provider_balances {
        for snapshot in snapshots {
            let provider_id = balance_provider_id(station_name, snapshot);
            let endpoint_id = snapshot
                .upstream_index
                .and_then(|idx| {
                    route_index
                        .endpoint_by_provider_station_upstream
                        .get(&(provider_id.clone(), station_name.clone(), idx))
                        .cloned()
                })
                .unwrap_or_else(|| {
                    snapshot
                        .upstream_index
                        .map(|idx| format!("upstream#{idx}"))
                        .or_else(|| non_empty(snapshot.source.clone()))
                        .unwrap_or_else(|| "balance".to_string())
                });
            let key = EndpointKey {
                provider_id: provider_id.clone(),
                endpoint_id,
            };
            let row = acc
                .entry(key.clone())
                .or_insert_with(|| EndpointAccum::new(key));
            row.station_name = snapshot
                .station_name
                .clone()
                .or_else(|| Some(station_name.clone()))
                .or(row.station_name.take());
            row.upstream_index = snapshot.upstream_index.or(row.upstream_index);
            let summary = summarize_snapshot(snapshot, now_ms);
            let replace = row.balance.as_ref().is_none_or(|existing| {
                snapshot_display_rank(summary.status) < snapshot_display_rank(existing.status)
            });
            if replace {
                row.balance = Some(summary);
            }
        }
    }

    acc.into_values().map(EndpointAccum::finish).collect()
}

fn record_finished_into_bucket(bucket: &mut UsageBucket, request: &FinishedRequest) {
    bucket.requests_total = bucket.requests_total.saturating_add(1);
    if request.status_code >= 400 {
        bucket.requests_error = bucket.requests_error.saturating_add(1);
    }
    bucket.duration_ms_total = bucket.duration_ms_total.saturating_add(request.duration_ms);

    let Some(usage) = request.usage.as_ref() else {
        return;
    };

    bucket.usage.add_assign(usage);
    bucket.cost.record_usage_cost(&request.cost);
    bucket.requests_with_usage = bucket.requests_with_usage.saturating_add(1);
    bucket.duration_ms_with_usage_total = bucket
        .duration_ms_with_usage_total
        .saturating_add(request.duration_ms);
    let generation_ms = match request.ttfb_ms {
        Some(ttfb) if ttfb > 0 && ttfb < request.duration_ms => {
            request.duration_ms.saturating_sub(ttfb)
        }
        _ => request.duration_ms,
    };
    bucket.generation_ms_total = bucket.generation_ms_total.saturating_add(generation_ms);
    if let Some(ttfb) = request.ttfb_ms.filter(|value| *value > 0) {
        bucket.ttfb_ms_total = bucket.ttfb_ms_total.saturating_add(ttfb);
        bucket.ttfb_samples = bucket.ttfb_samples.saturating_add(1);
    }
}

fn group_balances_by_provider(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
) -> BTreeMap<String, Vec<&ProviderBalanceSnapshot>> {
    let mut out = BTreeMap::<String, Vec<&ProviderBalanceSnapshot>>::new();
    for (station_name, snapshots) in provider_balances {
        for snapshot in snapshots {
            out.entry(balance_provider_id(station_name, snapshot))
                .or_default()
                .push(snapshot);
        }
    }
    out
}

fn balance_provider_id(station_name: &str, snapshot: &ProviderBalanceSnapshot) -> String {
    if snapshot.provider_id.trim().is_empty() {
        station_name.to_string()
    } else {
        snapshot.provider_id.trim().to_string()
    }
}

fn balance_counts_for_snapshots(
    snapshots: &[&ProviderBalanceSnapshot],
    now_ms: u64,
) -> UsageBalanceStatusCounts {
    let mut counts = UsageBalanceStatusCounts::default();
    for snapshot in snapshots {
        counts.record(classify_snapshot(snapshot, now_ms));
    }
    counts
}

fn classify_snapshot(snapshot: &ProviderBalanceSnapshot, now_ms: u64) -> UsageBalanceStatus {
    match snapshot.status_at(now_ms) {
        BalanceSnapshotStatus::Ok if snapshot.unlimited_quota == Some(true) => {
            UsageBalanceStatus::Unlimited
        }
        BalanceSnapshotStatus::Ok => UsageBalanceStatus::Ok,
        BalanceSnapshotStatus::Exhausted => UsageBalanceStatus::Exhausted,
        BalanceSnapshotStatus::Stale => UsageBalanceStatus::Stale,
        BalanceSnapshotStatus::Error => UsageBalanceStatus::Error,
        BalanceSnapshotStatus::Unknown => UsageBalanceStatus::Unknown,
    }
}

fn summarize_snapshot(
    snapshot: &ProviderBalanceSnapshot,
    now_ms: u64,
) -> UsageBalanceSnapshotSummary {
    UsageBalanceSnapshotSummary {
        provider_id: snapshot.provider_id.clone(),
        station_name: snapshot.station_name.clone(),
        upstream_index: snapshot.upstream_index,
        source: snapshot.source.clone(),
        status: classify_snapshot(snapshot, now_ms),
        amount_summary: snapshot.amount_summary(),
        fetched_at_ms: snapshot.fetched_at_ms,
        stale_after_ms: snapshot.stale_after_ms,
        age_ms: (now_ms >= snapshot.fetched_at_ms).then(|| now_ms - snapshot.fetched_at_ms),
        error: snapshot.error.clone(),
        exhaustion_affects_routing: snapshot.exhaustion_affects_routing,
        routing_exhausted: snapshot.exhaustion_affects_routing
            && snapshot.status_at(now_ms) == BalanceSnapshotStatus::Exhausted,
        routing_ignored_exhaustion: !snapshot.exhaustion_affects_routing
            && snapshot.status_at(now_ms) == BalanceSnapshotStatus::Exhausted,
    }
}

fn primary_balance_snapshot<'a>(
    snapshots: &'a [&ProviderBalanceSnapshot],
    now_ms: u64,
) -> Option<&'a ProviderBalanceSnapshot> {
    snapshots.iter().copied().min_by(|left, right| {
        snapshot_display_rank(classify_snapshot(left, now_ms))
            .cmp(&snapshot_display_rank(classify_snapshot(right, now_ms)))
            .then_with(|| left.upstream_index.cmp(&right.upstream_index))
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    })
}

fn snapshot_display_rank(status: UsageBalanceStatus) -> u8 {
    match status {
        UsageBalanceStatus::Ok | UsageBalanceStatus::Unlimited => 0,
        UsageBalanceStatus::Stale => 1,
        UsageBalanceStatus::Unknown | UsageBalanceStatus::Error => 2,
        UsageBalanceStatus::Exhausted => 3,
    }
}

fn latest_balance_error(snapshots: &[&ProviderBalanceSnapshot]) -> Option<String> {
    snapshots
        .iter()
        .filter_map(|snapshot| {
            snapshot
                .error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|error| (snapshot.fetched_at_ms, error.to_string()))
        })
        .max_by_key(|(fetched_at_ms, _)| *fetched_at_ms)
        .map(|(_, error)| error)
}

fn per_mille(num: u64, den: u64) -> Option<u16> {
    (den > 0).then(|| ((num.saturating_mul(1000) / den).min(1000)) as u16)
}

fn output_tokens_per_second(bucket: &UsageBucket) -> Option<u64> {
    let output = bucket.usage.output_tokens.max(0) as u64;
    if output == 0 || bucket.generation_ms_total == 0 {
        return None;
    }
    Some(output.saturating_mul(1000) / bucket.generation_ms_total)
}

fn sort_provider_rows(rows: &mut [UsageBalanceProviderRow]) {
    rows.sort_by(|left, right| {
        provider_selected_rank(right)
            .cmp(&provider_selected_rank(left))
            .then_with(|| {
                provider_attention_rank(left.balance_status)
                    .cmp(&provider_attention_rank(right.balance_status))
            })
            .then_with(|| provider_cost_sort_value(right).cmp(&provider_cost_sort_value(left)))
            .then_with(|| right.usage.requests_total.cmp(&left.usage.requests_total))
            .then_with(|| left.provider_id.cmp(&right.provider_id))
    });
}

fn provider_selected_rank(row: &UsageBalanceProviderRow) -> u8 {
    u8::from(row.routing.selected)
}

fn provider_attention_rank(status: UsageBalanceStatus) -> u8 {
    match status {
        UsageBalanceStatus::Error => 0,
        UsageBalanceStatus::Exhausted => 1,
        UsageBalanceStatus::Stale => 2,
        UsageBalanceStatus::Unknown => 3,
        UsageBalanceStatus::Unlimited => 4,
        UsageBalanceStatus::Ok => 5,
    }
}

fn provider_cost_sort_value(row: &UsageBalanceProviderRow) -> i128 {
    row.cost_total_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
        .map(UsdAmount::femto_usd)
        .unwrap_or(0)
}

fn sort_endpoint_rows(rows: &mut [UsageBalanceEndpointRow]) {
    rows.sort_by(|left, right| {
        u8::from(right.route_selected)
            .cmp(&u8::from(left.route_selected))
            .then_with(|| {
                provider_attention_rank(left.balance_status)
                    .cmp(&provider_attention_rank(right.balance_status))
            })
            .then_with(|| right.usage.requests_total.cmp(&left.usage.requests_total))
            .then_with(|| left.provider_id.cmp(&right.provider_id))
            .then_with(|| left.endpoint_id.cmp(&right.endpoint_id))
    });
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EndpointKey {
    provider_id: String,
    endpoint_id: String,
}

#[derive(Debug, Clone)]
struct EndpointAccum {
    key: EndpointKey,
    station_name: Option<String>,
    upstream_index: Option<usize>,
    base_url: Option<String>,
    usage: UsageBucket,
    balance: Option<UsageBalanceSnapshotSummary>,
    route_selected: bool,
    route_skip_reasons: BTreeSet<String>,
}

impl EndpointAccum {
    fn new(key: EndpointKey) -> Self {
        Self {
            key,
            station_name: None,
            upstream_index: None,
            base_url: None,
            usage: UsageBucket::default(),
            balance: None,
            route_selected: false,
            route_skip_reasons: BTreeSet::new(),
        }
    }

    fn finish(self) -> UsageBalanceEndpointRow {
        let balance_status = self
            .balance
            .as_ref()
            .map(|balance| balance.status)
            .unwrap_or(UsageBalanceStatus::Unknown);
        UsageBalanceEndpointRow {
            provider_id: self.key.provider_id,
            endpoint_id: self.key.endpoint_id,
            station_name: self.station_name,
            upstream_index: self.upstream_index,
            base_url: self.base_url,
            usage: self.usage,
            balance_status,
            balance: self.balance,
            route_selected: self.route_selected,
            route_skip_reasons: self.route_skip_reasons.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone)]
struct RouteCandidateView {
    provider_id: String,
    endpoint_id: String,
    base_url: String,
    selected: bool,
    skip_reasons: Vec<String>,
    compatibility: Option<(String, usize)>,
}

#[derive(Debug, Clone, Default)]
struct RouteIndex {
    candidates: BTreeMap<String, RouteCandidateView>,
    provider_impacts: BTreeMap<String, UsageBalanceRouteImpact>,
    endpoint_by_provider_base_url: BTreeMap<(String, String), String>,
    endpoint_by_provider_station_upstream: BTreeMap<(String, String, usize), String>,
}

impl RouteIndex {
    fn from_explain(explain: Option<&RoutingExplainResponse>) -> Self {
        let mut out = Self::default();
        let Some(explain) = explain else {
            return out;
        };

        for candidate in &explain.candidates {
            out.record_candidate(candidate);
        }
        for impact in out.provider_impacts.values_mut() {
            impact.skip_reasons.sort();
            impact.skip_reasons.dedup();
            impact.route_paths.sort();
            impact.route_paths.dedup();
        }
        out
    }

    fn record_candidate(&mut self, candidate: &RoutingExplainCandidate) {
        let skip_reasons = candidate
            .skip_reasons
            .iter()
            .map(|reason| reason.code().to_string())
            .collect::<Vec<_>>();
        let compatibility = candidate.compatibility.as_ref().map(|compatibility| {
            (
                compatibility.station_name.clone(),
                compatibility.upstream_index,
            )
        });
        let view = RouteCandidateView {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            base_url: candidate.upstream_base_url.clone(),
            selected: candidate.selected,
            skip_reasons: skip_reasons.clone(),
            compatibility: compatibility.clone(),
        };

        self.endpoint_by_provider_base_url.insert(
            (
                candidate.provider_id.clone(),
                candidate.upstream_base_url.clone(),
            ),
            candidate.endpoint_id.clone(),
        );
        if let Some((station_name, upstream_index)) = compatibility {
            self.endpoint_by_provider_station_upstream.insert(
                (candidate.provider_id.clone(), station_name, upstream_index),
                candidate.endpoint_id.clone(),
            );
        }

        let impact = self
            .provider_impacts
            .entry(candidate.provider_id.clone())
            .or_insert_with(|| UsageBalanceRouteImpact {
                provider_id: candidate.provider_id.clone(),
                ..UsageBalanceRouteImpact::default()
            });
        impact.candidate_count += 1;
        impact.route_paths.push(candidate.route_path.clone());
        impact.skip_reasons.extend(skip_reasons);
        if candidate.selected {
            impact.selected = true;
            impact.selected_endpoint_id = Some(candidate.endpoint_id.clone());
            impact.selected_provider_endpoint_key = Some(candidate.provider_endpoint_key.clone());
        }

        self.candidates
            .insert(candidate.provider_endpoint_key.clone(), view);
    }
}

fn endpoint_id_from_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    after_scheme
        .split('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("endpoint")
        .to_string()
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::CostBreakdown;
    use crate::routing_explain::{
        RoutingExplainCandidate, RoutingExplainResponse, RoutingExplainSkipReason,
    };
    use crate::usage::UsageMetrics;

    fn build_view(
        usage_rollup: &UsageRollupView,
        provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
        recent: &[FinishedRequest],
        routing_explain: Option<&RoutingExplainResponse>,
    ) -> UsageBalanceView {
        UsageBalanceView::build(UsageBalanceBuildInput {
            service_name: "codex",
            window_days: 7,
            generated_at_ms: 1_000,
            usage_rollup,
            provider_balances,
            recent,
            routing_explain,
            refresh: UsageBalanceRefreshInput::default(),
        })
    }

    #[test]
    fn balance_status_counts_keep_unknown_stale_exhausted_error_and_unlimited_distinct() {
        let mut balances = HashMap::new();
        balances.insert(
            "station".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    provider_id: "ok".to_string(),
                    exhausted: Some(false),
                    total_balance_usd: Some("3".to_string()),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_id: "unlimited".to_string(),
                    exhausted: Some(false),
                    unlimited_quota: Some(true),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_id: "stale".to_string(),
                    exhausted: Some(false),
                    stale_after_ms: Some(950),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_id: "exhausted".to_string(),
                    exhausted: Some(true),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_id: "error".to_string(),
                    error: Some("lookup failed".to_string()),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_id: "unknown".to_string(),
                    fetched_at_ms: 900,
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        );

        let view = build_view(&UsageRollupView::default(), &balances, &[], None);

        assert_eq!(view.totals.balance_status_counts.ok, 1);
        assert_eq!(view.totals.balance_status_counts.unlimited, 1);
        assert_eq!(view.totals.balance_status_counts.stale, 1);
        assert_eq!(view.totals.balance_status_counts.exhausted, 1);
        assert_eq!(view.totals.balance_status_counts.error, 1);
        assert_eq!(view.totals.balance_status_counts.unknown, 1);
        assert_eq!(
            view.provider_rows
                .iter()
                .find(|row| row.provider_id == "unlimited")
                .map(|row| row.balance_status),
            Some(UsageBalanceStatus::Unlimited)
        );
        assert_eq!(
            view.provider_rows
                .iter()
                .find(|row| row.provider_id == "unknown")
                .map(|row| row.balance_status),
            Some(UsageBalanceStatus::Unknown)
        );
    }

    #[test]
    fn provider_rows_prefer_route_selection_then_attention_and_usage() {
        let rollup = UsageRollupView {
            by_provider: vec![
                (
                    "cheap".to_string(),
                    UsageBucket {
                        requests_total: 2,
                        ..UsageBucket::default()
                    },
                ),
                (
                    "selected".to_string(),
                    UsageBucket {
                        requests_total: 1,
                        ..UsageBucket::default()
                    },
                ),
            ],
            ..UsageRollupView::default()
        };
        let explain = RoutingExplainResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            runtime_loaded_at_ms: None,
            request_model: None,
            session_id: None,
            request_context: Default::default(),
            selected_route: None,
            candidates: vec![RoutingExplainCandidate {
                provider_id: "selected".to_string(),
                provider_alias: None,
                endpoint_id: "default".to_string(),
                provider_endpoint_key: "codex:selected:default".to_string(),
                route_path: vec!["main".to_string(), "selected".to_string()],
                preference_group: 0,
                compatibility: None,
                upstream_base_url: "https://selected.example/v1".to_string(),
                selected: true,
                skip_reasons: Vec::new(),
            }],
            affinity_policy: "off".to_string(),
            affinity: None,
            conditional_routes: Vec::new(),
        };

        let view = build_view(&rollup, &HashMap::new(), &[], Some(&explain));

        assert_eq!(view.provider_rows[0].provider_id, "selected");
        assert!(view.provider_rows[0].routing.selected);
    }

    #[test]
    fn endpoint_rows_merge_recent_usage_balance_and_route_skip_reasons() {
        let recent = vec![FinishedRequest {
            id: 1,
            trace_id: None,
            session_id: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-test".to_string()),
            reasoning_effort: None,
            service_tier: None,
            station_name: Some("station-a".to_string()),
            provider_id: Some("right".to_string()),
            upstream_base_url: Some("https://right.example/v1".to_string()),
            route_decision: None,
            usage: Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                ..UsageMetrics::default()
            }),
            cost: CostBreakdown::unknown(),
            retry: None,
            observability: Default::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 429,
            duration_ms: 120,
            ttfb_ms: Some(20),
            streaming: false,
            ended_at_ms: 950,
        }];
        let mut balances = HashMap::new();
        balances.insert(
            "station-a".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "right".to_string(),
                station_name: Some("station-a".to_string()),
                upstream_index: Some(0),
                exhausted: Some(true),
                fetched_at_ms: 940,
                ..ProviderBalanceSnapshot::default()
            }],
        );
        let explain = RoutingExplainResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            runtime_loaded_at_ms: None,
            request_model: None,
            session_id: None,
            request_context: Default::default(),
            selected_route: None,
            candidates: vec![RoutingExplainCandidate {
                provider_id: "right".to_string(),
                provider_alias: None,
                endpoint_id: "default".to_string(),
                provider_endpoint_key: "codex:right:default".to_string(),
                route_path: vec!["main".to_string(), "right".to_string()],
                preference_group: 0,
                compatibility: Some(crate::routing_explain::RoutingExplainCompatibility {
                    station_name: "station-a".to_string(),
                    upstream_index: 0,
                }),
                upstream_base_url: "https://right.example/v1".to_string(),
                selected: false,
                skip_reasons: vec![RoutingExplainSkipReason::UsageExhausted],
            }],
            affinity_policy: "off".to_string(),
            affinity: None,
            conditional_routes: Vec::new(),
        };

        let view = build_view(
            &UsageRollupView::default(),
            &balances,
            &recent,
            Some(&explain),
        );
        let endpoint = view
            .endpoint_rows
            .iter()
            .find(|row| row.provider_id == "right")
            .expect("right endpoint");

        assert_eq!(endpoint.endpoint_id, "default");
        assert_eq!(endpoint.usage.requests_total, 1);
        assert_eq!(endpoint.usage.requests_error, 1);
        assert_eq!(endpoint.balance_status, UsageBalanceStatus::Exhausted);
        assert_eq!(endpoint.route_skip_reasons, vec!["usage_exhausted"]);
    }
}
