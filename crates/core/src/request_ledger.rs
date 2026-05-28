use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::local_log_store::{LogRetention, repair_log};
pub use crate::logging::request_log_path;
use crate::logging::request_log_retention;
use crate::pricing::{CostAdjustments, estimate_request_cost_from_operator_catalog_for_service};
use crate::state::{
    FinishedRequest, RequestObservability, RouteDecisionProvenance, SessionIdentitySource,
};
use crate::usage::{CacheInputAccounting, UsageMetrics};

#[derive(Debug, Clone, PartialEq)]
pub struct RequestLogLine {
    raw: String,
    value: Option<JsonValue>,
}

impl RequestLogLine {
    pub fn from_raw(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let value = serde_json::from_str::<JsonValue>(&raw).ok();
        Self { raw, value }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn value(&self) -> Option<&JsonValue> {
        self.value.as_ref()
    }

    pub fn is_valid_json(&self) -> bool {
        self.value.is_some()
    }

    pub fn display_lines(&self) -> Vec<String> {
        self.value
            .as_ref()
            .map(format_request_log_record_lines)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestLogFilters {
    pub session: Option<String>,
    pub model: Option<String>,
    pub station: Option<String>,
    pub provider: Option<String>,
    pub path: Option<String>,
    pub status_min: Option<u64>,
    pub status_max: Option<u64>,
    pub fast: bool,
    pub retried: bool,
}

impl RequestLogFilters {
    pub fn is_empty(&self) -> bool {
        self.session.is_none()
            && self.model.is_none()
            && self.station.is_none()
            && self.provider.is_none()
            && self.path.is_none()
            && self.status_min.is_none()
            && self.status_max.is_none()
            && !self.fast
            && !self.retried
    }

    pub fn matches(&self, record: &JsonValue) -> bool {
        if let Some(expected) = self.session.as_deref()
            && !field_contains(str_field(record, "session_id"), expected)
        {
            return false;
        }
        if let Some(expected) = self.model.as_deref()
            && !field_contains(request_model(record).as_deref(), expected)
        {
            return false;
        }
        if let Some(expected) = self.station.as_deref()
            && !field_contains(Some(station_name(record)), expected)
        {
            return false;
        }
        if let Some(expected) = self.provider.as_deref()
            && !field_contains(str_field(record, "provider_id"), expected)
        {
            return false;
        }
        if let Some(expected) = self.path.as_deref()
            && !field_contains(str_field(record, "path"), expected)
        {
            return false;
        }
        if let Some(min) = self.status_min
            && u64_field(record, "status_code").unwrap_or(0) < min
        {
            return false;
        }
        if let Some(max) = self.status_max
            && u64_field(record, "status_code").unwrap_or(0) > max
        {
            return false;
        }
        if self.fast && !request_is_fast(record) {
            return false;
        }
        if self.retried && !request_was_retried(record) {
            return false;
        }
        true
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
    pub fn record(
        &mut self,
        duration_ms: u64,
        usage: Option<&UsageMetrics>,
        accounting: CacheInputAccounting,
    ) {
        self.requests = self.requests.saturating_add(1);
        self.duration_ms_total = self.duration_ms_total.saturating_add(duration_ms);
        let Some(usage) = usage else {
            return;
        };

        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens.max(0));
        self.output_tokens = self
            .output_tokens
            .saturating_add(usage.output_tokens.max(0));
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(reasoning_tokens(usage));
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_tokens_total());
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(cache_creation_tokens(usage));
        self.total_tokens = self
            .total_tokens
            .saturating_add(total_tokens(usage, accounting));
    }

    pub fn average_duration_ms(&self) -> u64 {
        self.duration_ms_total
            .checked_div(self.requests)
            .unwrap_or(0)
    }

    pub fn summary_line(&self, station_name: &str) -> String {
        format!(
            "{} | {} | {} | {} | {} | {} | {} | {} | {}",
            station_name,
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

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequestUsageSummaryGroup {
    #[default]
    Station,
    Provider,
    Model,
    Session,
}

impl RequestUsageSummaryGroup {
    pub fn column_name(self) -> &'static str {
        match self {
            Self::Station => "station_name",
            Self::Provider => "provider_id",
            Self::Model => "model",
            Self::Session => "session_id",
        }
    }

    fn key(self, record: &JsonValue) -> String {
        match self {
            Self::Station => station_name(record).to_string(),
            Self::Provider => str_field(record, "provider_id").unwrap_or("-").to_string(),
            Self::Model => request_model(record).unwrap_or_else(|| "-".to_string()),
            Self::Session => str_field(record, "session_id").unwrap_or("-").to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RequestUsageSummaryRow {
    pub group_value: String,
    pub aggregate: RequestUsageAggregate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLedgerStore {
    path: PathBuf,
    retention: LogRetention,
}

impl Default for RequestLedgerStore {
    fn default() -> Self {
        Self::new(request_log_path())
    }
}

impl RequestLedgerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            retention: request_log_retention(),
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self::new(path.as_ref().to_path_buf())
    }

    #[cfg(test)]
    fn with_retention(path: impl Into<PathBuf>, retention: LogRetention) -> Self {
        Self {
            path: path.into(),
            retention,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    fn repair_before_read(&self) {
        repair_log(&self.path, self.retention);
    }

    fn open_after_repair(&self) -> std::io::Result<Option<File>> {
        self.repair_before_read();
        match File::open(&self.path) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn read_lines(&self) -> std::io::Result<Vec<RequestLogLine>> {
        let Some(file) = self.open_after_repair()? else {
            return Ok(Vec::new());
        };
        let reader = BufReader::new(file);
        Ok(reader
            .lines()
            .map_while(Result::ok)
            .map(RequestLogLine::from_raw)
            .collect())
    }

    pub fn tail_lines(&self, limit: usize) -> std::io::Result<Vec<RequestLogLine>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(file) = self.open_after_repair()? else {
            return Ok(Vec::new());
        };
        let reader = BufReader::new(file);
        let mut ring = VecDeque::with_capacity(limit);
        for line in reader.lines().map_while(Result::ok) {
            if ring.len() == limit {
                ring.pop_front();
            }
            ring.push_back(RequestLogLine::from_raw(line));
        }
        Ok(ring.into_iter().collect())
    }

    pub fn tail_finished_requests(&self, limit: usize) -> std::io::Result<Vec<FinishedRequest>> {
        let lines = self.tail_lines(limit)?;
        let mut requests = lines
            .iter()
            .filter_map(|line| {
                line.value()
                    .and_then(finished_request_from_request_log_record)
            })
            .collect::<Vec<_>>();
        requests.reverse();
        Ok(requests)
    }

    pub fn find_lines(
        &self,
        filters: &RequestLogFilters,
        limit: usize,
    ) -> std::io::Result<Vec<RequestLogLine>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(file) = self.open_after_repair()? else {
            return Ok(Vec::new());
        };
        let reader = BufReader::new(file);
        let mut ring = VecDeque::with_capacity(limit);
        for line in reader.lines().map_while(Result::ok) {
            let line = RequestLogLine::from_raw(line);
            if !line.value().is_some_and(|record| filters.matches(record)) {
                continue;
            }
            if ring.len() == limit {
                ring.pop_front();
            }
            ring.push_back(line);
        }
        Ok(ring.into_iter().rev().collect())
    }

    pub fn find_finished_requests(
        &self,
        filters: &RequestLogFilters,
        limit: usize,
    ) -> std::io::Result<Vec<FinishedRequest>> {
        let lines = self.find_lines(filters, limit)?;
        Ok(lines
            .iter()
            .filter_map(|line| {
                line.value()
                    .and_then(finished_request_from_request_log_record)
            })
            .collect())
    }

    pub fn summarize(
        &self,
        group: RequestUsageSummaryGroup,
        filters: &RequestLogFilters,
        limit: usize,
    ) -> std::io::Result<Vec<RequestUsageSummaryRow>> {
        let lines = self.read_lines()?;
        Ok(summarize_request_log_lines(
            lines.iter(),
            group,
            filters,
            limit,
        ))
    }
}

pub fn read_request_log_lines(path: &Path) -> std::io::Result<Vec<RequestLogLine>> {
    RequestLedgerStore::from_path(path).read_lines()
}

pub fn tail_request_log(path: &Path, limit: usize) -> std::io::Result<Vec<RequestLogLine>> {
    RequestLedgerStore::from_path(path).tail_lines(limit)
}

pub fn tail_finished_requests_from_log(
    path: &Path,
    limit: usize,
) -> std::io::Result<Vec<FinishedRequest>> {
    RequestLedgerStore::from_path(path).tail_finished_requests(limit)
}

pub fn find_finished_requests_from_log(
    path: &Path,
    filters: &RequestLogFilters,
    limit: usize,
) -> std::io::Result<Vec<FinishedRequest>> {
    RequestLedgerStore::from_path(path).find_finished_requests(filters, limit)
}

pub fn summarize_request_log(
    path: &Path,
    group: RequestUsageSummaryGroup,
    filters: &RequestLogFilters,
    limit: usize,
) -> std::io::Result<Vec<RequestUsageSummaryRow>> {
    RequestLedgerStore::from_path(path).summarize(group, filters, limit)
}

pub fn find_request_log(
    path: &Path,
    filters: &RequestLogFilters,
    limit: usize,
) -> std::io::Result<Vec<RequestLogLine>> {
    RequestLedgerStore::from_path(path).find_lines(filters, limit)
}

fn summarize_request_log_lines<'a>(
    lines: impl IntoIterator<Item = &'a RequestLogLine>,
    group: RequestUsageSummaryGroup,
    filters: &RequestLogFilters,
    limit: usize,
) -> Vec<RequestUsageSummaryRow> {
    let mut aggregate: HashMap<String, RequestUsageAggregate> = HashMap::new();
    for line in lines {
        let Some(record) = line.value() else {
            continue;
        };
        if !filters.matches(record) {
            continue;
        }
        let group_value = group.key(record);
        let duration_ms = u64_field(record, "duration_ms").unwrap_or(0);
        let service = str_field(record, "service").unwrap_or("-");
        let usage = usage_metrics(record);
        let entry = aggregate.entry(group_value).or_default();
        entry.record(
            duration_ms,
            usage.as_ref(),
            CacheInputAccounting::for_service(service),
        );
    }

    let mut items: Vec<RequestUsageSummaryRow> = aggregate
        .into_iter()
        .map(|(group_value, aggregate)| RequestUsageSummaryRow {
            group_value,
            aggregate,
        })
        .collect();
    items.sort_by(|a, b| {
        b.aggregate
            .total_tokens
            .cmp(&a.aggregate.total_tokens)
            .then_with(|| a.group_value.cmp(&b.group_value))
    });
    items.into_iter().take(limit).collect()
}

pub fn format_request_log_record_lines(record: &JsonValue) -> Vec<String> {
    let ts = i64_field(record, "timestamp_ms").unwrap_or(0);
    let service = str_field(record, "service").unwrap_or("-");
    let method = str_field(record, "method").unwrap_or("-");
    let path = str_field(record, "path").unwrap_or("-");
    let status = u64_field(record, "status_code").unwrap_or(0);
    let provider = str_field(record, "provider_id").unwrap_or("-");
    let endpoint = provider_endpoint_display(record);
    let station = station_name(record);
    let model = request_model(record).unwrap_or_else(|| "-".to_string());
    let tier = service_tier_display(record);

    let mut lines = vec![format!(
        "[{}] {} {} {} status={} endpoint={} provider={} station={} model={} tier={}",
        ts, service, method, path, status, endpoint, provider, station, model, tier
    )];

    let duration_ms = u64_field(record, "duration_ms").unwrap_or(0);
    let ttfb_ms = u64_field(record, "ttfb_ms");
    let usage = usage_metrics(record);
    let finished_request = finished_request_from_request_log_record(record);
    let observability = finished_request
        .as_ref()
        .map(|request| request.observability_view());
    let effective_ttfb_ms = observability
        .as_ref()
        .and_then(|observability| observability.ttfb_ms)
        .or(ttfb_ms);
    let speed = observability
        .as_ref()
        .and_then(|observability| observability.output_tokens_per_second)
        .or_else(|| {
            usage
                .as_ref()
                .and_then(|usage| output_tokens_per_second(usage, duration_ms, effective_ttfb_ms))
        });
    let cost = request_cost_display(service, model.as_str(), usage.as_ref());

    lines.push(format!(
        "    timing duration={} ttfb={} output_speed={} cost={}",
        format_ms(duration_ms),
        format_optional_ms(effective_ttfb_ms),
        format_optional_speed(speed),
        cost
    ));

    if let Some(usage) = usage.as_ref() {
        lines.push(format!(
            "    tokens input={} output={} cache_read={} cache_create={} reasoning={} total={}",
            usage.input_tokens.max(0),
            usage.output_tokens.max(0),
            usage.cache_read_tokens_total(),
            cache_creation_tokens(usage),
            reasoning_tokens(usage),
            total_tokens(usage, CacheInputAccounting::for_service(service))
        ));
    } else {
        lines.push("    tokens -".to_string());
    }

    lines
}

pub fn request_log_record_model(record: &JsonValue) -> Option<String> {
    request_model(record)
}

pub fn request_log_record_station(record: &JsonValue) -> &str {
    station_name(record)
}

pub fn request_log_record_is_fast(record: &JsonValue) -> bool {
    request_is_fast(record)
}

pub fn request_log_record_was_retried(record: &JsonValue) -> bool {
    request_was_retried(record)
}

pub fn finished_request_from_request_log_record(record: &JsonValue) -> Option<FinishedRequest> {
    let timestamp_ms = u64_field(record, "timestamp_ms").unwrap_or(0);
    let request_id = u64_field(record, "request_id").unwrap_or(timestamp_ms);
    let status_code = u64_field(record, "status_code")
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or(0);
    let duration_ms = u64_field(record, "duration_ms").unwrap_or(0);
    let usage = usage_metrics(record);
    let model = request_model(record);
    let service = str_field(record, "service").unwrap_or("-");
    let cost = estimate_request_cost_from_operator_catalog_for_service(
        model.as_deref(),
        usage.as_ref(),
        CostAdjustments::default(),
        service,
    );
    let retry = record
        .get("retry")
        .and_then(|retry| serde_json::from_value(retry.clone()).ok());
    let route_decision = record.get("route_decision").and_then(|route_decision| {
        serde_json::from_value::<RouteDecisionProvenance>(route_decision.clone()).ok()
    });
    let session_identity_source = record
        .get("session_identity_source")
        .and_then(|source| serde_json::from_value::<SessionIdentitySource>(source.clone()).ok());

    let mut request = FinishedRequest {
        id: request_id,
        trace_id: str_field(record, "trace_id").map(ToOwned::to_owned),
        session_id: str_field(record, "session_id").map(ToOwned::to_owned),
        session_identity_source,
        client_name: str_field(record, "client_name").map(ToOwned::to_owned),
        client_addr: str_field(record, "client_addr").map(ToOwned::to_owned),
        cwd: str_field(record, "cwd").map(ToOwned::to_owned),
        model,
        reasoning_effort: str_field(record, "reasoning_effort").map(ToOwned::to_owned),
        service_tier: service_tier_value(record),
        station_name: non_dash(station_name(record)).map(ToOwned::to_owned),
        provider_id: str_field(record, "provider_id").map(ToOwned::to_owned),
        upstream_base_url: str_field(record, "upstream_base_url").map(ToOwned::to_owned),
        route_decision,
        usage,
        cost,
        retry,
        observability: RequestObservability::default(),
        service: service.to_string(),
        method: str_field(record, "method").unwrap_or("-").to_string(),
        path: str_field(record, "path").unwrap_or("-").to_string(),
        status_code,
        duration_ms,
        ttfb_ms: u64_field(record, "ttfb_ms"),
        streaming: record
            .get("streaming")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        ended_at_ms: timestamp_ms,
    };
    request.refresh_observability();
    Some(request)
}

fn field_contains(value: Option<&str>, expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return true;
    }
    value
        .map(|value| value.to_ascii_lowercase().contains(&expected))
        .unwrap_or(false)
}

fn request_is_fast(record: &JsonValue) -> bool {
    record
        .get("observability")
        .and_then(|observability| observability.get("fast_mode"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || service_tier_display(record).eq_ignore_ascii_case("priority(fast)")
}

fn request_was_retried(record: &JsonValue) -> bool {
    if record
        .get("observability")
        .and_then(|observability| observability.get("retried"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if record
        .get("observability")
        .and_then(|observability| observability.get("attempt_count"))
        .and_then(|value| value.as_u64())
        .is_some_and(|attempts| attempts > 1)
    {
        return true;
    }
    if record
        .get("retry")
        .and_then(|retry| retry.get("route_attempts"))
        .and_then(|attempts| attempts.as_array())
        .is_some_and(|attempts| attempts.len() > 1)
    {
        return true;
    }
    if record
        .get("retry")
        .and_then(|retry| retry.get("attempts"))
        .and_then(|attempts| attempts.as_u64())
        .is_some_and(|attempts| attempts > 1)
    {
        return true;
    }
    record
        .get("retry")
        .and_then(|retry| retry.get("upstream_chain"))
        .and_then(|attempts| attempts.as_array())
        .is_some_and(|attempts| attempts.len() > 1)
}

fn usage_metrics(record: &JsonValue) -> Option<UsageMetrics> {
    record
        .get("usage")
        .and_then(|usage| serde_json::from_value::<UsageMetrics>(usage.clone()).ok())
}

fn request_cost_display(service: &str, model: &str, usage: Option<&UsageMetrics>) -> String {
    let model = model.trim();
    if model.is_empty() || model == "-" {
        return "-".to_string();
    }
    let cost = estimate_request_cost_from_operator_catalog_for_service(
        Some(model),
        usage,
        CostAdjustments::default(),
        service,
    );
    cost.display_total_with_confidence()
}

fn output_tokens_per_second(
    usage: &UsageMetrics,
    duration_ms: u64,
    ttfb_ms: Option<u64>,
) -> Option<f64> {
    const OUTPUT_RATE_SANITY_CEIL: f64 = 5_000.0;
    let output_tokens = usage.output_tokens.max(0);
    if output_tokens == 0 || duration_ms == 0 {
        return None;
    }
    let generation_ms = match ttfb_ms {
        Some(ttfb) if ttfb > 0 && ttfb < duration_ms => duration_ms.saturating_sub(ttfb),
        _ => duration_ms,
    };
    if generation_ms == 0 {
        return None;
    }
    let rate = output_tokens as f64 / (generation_ms as f64 / 1000.0);
    if generation_ms.saturating_mul(10) < duration_ms && rate > OUTPUT_RATE_SANITY_CEIL {
        return Some(output_tokens as f64 / (duration_ms as f64 / 1000.0));
    }
    Some(rate)
}

fn request_model(record: &JsonValue) -> Option<String> {
    str_field(record, "model")
        .map(ToOwned::to_owned)
        .or_else(|| nested_str(record, &["route_decision", "effective_model", "value"]))
        .or_else(|| model_from_route_attempts(record))
        .or_else(|| model_from_legacy_retry_chain(record))
}

fn model_from_route_attempts(record: &JsonValue) -> Option<String> {
    record
        .get("retry")
        .and_then(|retry| retry.get("route_attempts"))
        .and_then(|attempts| attempts.as_array())
        .and_then(|attempts| {
            attempts
                .iter()
                .rev()
                .filter(|attempt| {
                    !attempt
                        .get("skipped")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                })
                .find_map(|attempt| str_field(attempt, "model").map(ToOwned::to_owned))
        })
}

fn model_from_legacy_retry_chain(record: &JsonValue) -> Option<String> {
    record
        .get("retry")
        .and_then(|retry| retry.get("upstream_chain"))
        .and_then(|chain| chain.as_array())
        .and_then(|chain| {
            chain
                .iter()
                .rev()
                .filter_map(|entry| entry.as_str())
                .find_map(|entry| raw_kv_field(entry, "model"))
        })
}

fn raw_kv_field(raw: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    raw.split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .map(|value| value.trim().trim_matches(',').to_string())
        .filter(|value| !value.is_empty() && value != "-")
}

fn service_tier_display(record: &JsonValue) -> String {
    let tier = service_tier_value(record).unwrap_or_else(|| "-".to_string());
    if tier.eq_ignore_ascii_case("priority") {
        "priority(fast)".to_string()
    } else {
        tier
    }
}

fn service_tier_value(record: &JsonValue) -> Option<String> {
    record.get("service_tier").and_then(|tier| {
        tier.as_str()
            .map(ToOwned::to_owned)
            .or_else(|| service_tier_log_value(tier))
    })
}

fn service_tier_log_value(tier: &JsonValue) -> Option<String> {
    ["actual", "effective", "requested"]
        .iter()
        .find_map(|key| str_field(tier, key).map(ToOwned::to_owned))
}

fn station_name(record: &JsonValue) -> &str {
    str_field(record, "station_name")
        .or_else(|| str_field(record, "config_name"))
        .unwrap_or("-")
}

fn provider_endpoint_display(record: &JsonValue) -> String {
    str_field(record, "provider_endpoint_key")
        .map(ToOwned::to_owned)
        .or_else(|| {
            let provider = str_field(record, "provider_id")?;
            let endpoint = str_field(record, "endpoint_id")?;
            Some(format!("{provider}.{endpoint}"))
        })
        .unwrap_or_else(|| "-".to_string())
}

fn non_dash(value: &str) -> Option<&str> {
    (value != "-").then_some(value)
}

fn str_field<'a>(record: &'a JsonValue, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn nested_str(record: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = record;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn i64_field(record: &JsonValue, key: &str) -> Option<i64> {
    record.get(key).and_then(|value| value.as_i64())
}

fn u64_field(record: &JsonValue, key: &str) -> Option<u64> {
    record.get(key).and_then(|value| value.as_u64())
}

fn reasoning_tokens(usage: &UsageMetrics) -> i64 {
    usage.reasoning_output_tokens_total().max(0)
}

fn cache_creation_tokens(usage: &UsageMetrics) -> i64 {
    usage.cache_creation_tokens_total().max(0)
}

fn total_tokens(usage: &UsageMetrics, accounting: CacheInputAccounting) -> i64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        let breakdown = usage.cache_usage_breakdown(accounting);
        breakdown
            .effective_input_tokens
            .saturating_add(usage.output_tokens.max(0))
            .saturating_add(breakdown.cache_read_input_tokens)
            .saturating_add(breakdown.cache_creation_input_tokens)
    }
}

fn format_ms(value: u64) -> String {
    format!("{value}ms")
}

fn format_optional_ms(value: Option<u64>) -> String {
    value.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_optional_speed(value: Option<f64>) -> String {
    value
        .map(|speed| format!("{speed:.2} tok/s"))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_lines_include_route_model_fast_cache_and_speed() {
        let record = json!({
            "timestamp_ms": 123,
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 2000,
            "ttfb_ms": 500,
            "provider_id": "relay",
            "endpoint_id": "default",
            "provider_endpoint_key": "codex/relay/default",
            "service_tier": { "effective": "priority" },
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 30,
                "cache_read_input_tokens": 10,
                "cache_creation_5m_input_tokens": 5,
                "reasoning_output_tokens": 7,
                "total_tokens": 1045
            },
            "retry": {
                "route_attempts": [
                    { "decision": "completed", "model": "gpt-5" }
                ]
            }
        });

        let lines = format_request_log_record_lines(&record);

        assert!(lines[0].contains("endpoint=codex/relay/default"));
        assert!(lines[0].contains("station=-"));
        assert!(lines[0].contains("model=gpt-5"));
        assert!(lines[0].contains("tier=priority(fast)"));
        assert!(lines[1].contains("output_speed=20.00 tok/s"));
        assert!(lines[1].contains("cost="));
        assert!(lines[2].contains("cache_read=10"));
        assert!(lines[2].contains("cache_create=5"));
        assert!(lines[2].contains("reasoning=7"));
    }

    #[test]
    fn display_lines_use_corrected_ttfb_for_attempt_relative_stream_logs() {
        let record = json!({
            "timestamp_ms": 123,
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 10000,
            "ttfb_ms": 1210,
            "usage": {
                "output_tokens": 100,
                "total_tokens": 100
            },
            "retry": {
                "attempts": 2,
                "upstream_chain": [],
                "route_attempts": [
                    {
                        "attempt_index": 0,
                        "decision": "failed_status",
                        "status_code": 429,
                        "upstream_headers_ms": 50,
                        "duration_ms": 500,
                        "raw": "failed"
                    },
                    {
                        "attempt_index": 1,
                        "decision": "completed",
                        "status_code": 200,
                        "upstream_headers_ms": 1200,
                        "duration_ms": 2200,
                        "raw": "completed"
                    }
                ]
            }
        });

        let lines = format_request_log_record_lines(&record);

        assert!(lines[1].contains("ttfb=2210ms"));
        assert!(lines[1].contains("output_speed=12.84 tok/s"));
    }

    #[test]
    fn request_model_reads_legacy_retry_chain_model() {
        let record = json!({
            "retry": {
                "upstream_chain": [
                    "main:https://relay.example/v1 (idx=0) status=200 class=- model=gpt-5.4-mini"
                ]
            }
        });

        assert_eq!(request_model(&record).as_deref(), Some("gpt-5.4-mini"));
    }

    #[test]
    fn request_model_prefers_top_level_model_from_current_request_log_schema() {
        let record = json!({
            "model": "gpt-5",
            "route_decision": {
                "effective_model": { "value": "relay-gpt-5", "source": "station_mapping" }
            },
            "retry": {
                "route_attempts": [
                    { "decision": "completed", "model": "legacy-gpt-5" }
                ]
            }
        });

        assert_eq!(request_model(&record).as_deref(), Some("gpt-5"));
    }

    #[test]
    fn request_model_reads_route_decision_when_top_level_model_is_missing() {
        let record = json!({
            "route_decision": {
                "effective_model": { "value": "relay-gpt-5", "source": "station_mapping" }
            }
        });

        assert_eq!(request_model(&record).as_deref(), Some("relay-gpt-5"));
    }

    #[test]
    fn usage_aggregate_summary_includes_cache_and_average_duration() {
        let mut aggregate = RequestUsageAggregate::default();
        aggregate.record(
            200,
            Some(&UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 3,
                cache_creation_1h_input_tokens: 2,
                reasoning_output_tokens: 4,
                ..UsageMetrics::default()
            }),
            CacheInputAccounting::default(),
        );
        aggregate.record(400, None, CacheInputAccounting::default());

        assert_eq!(
            aggregate.summary_line("main"),
            "main | 2 | 10 | 5 | 3 | 2 | 4 | 20 | 300"
        );
    }

    #[test]
    fn filters_match_route_model_fast_retry_and_status() {
        let record = json!({
            "path": "/v1/responses",
            "session_id": "sid-abc",
            "station_name": "main-station",
            "provider_id": "relay-one",
            "status_code": 429,
            "service_tier": { "actual": "priority" },
            "observability": {
                "attempt_count": 2,
                "retried": true
            },
            "retry": {
                "route_attempts": [
                    { "decision": "failed_status", "model": "gpt-5.4-high" },
                    { "decision": "completed", "model": "gpt-5.4-high" }
                ]
            }
        });

        let filters = RequestLogFilters {
            session: Some("abc".to_string()),
            model: Some("5.4".to_string()),
            station: Some("main".to_string()),
            provider: Some("relay".to_string()),
            path: Some("responses".to_string()),
            status_min: Some(400),
            status_max: Some(499),
            fast: true,
            retried: true,
        };

        assert!(filters.matches(&record));
    }

    #[test]
    fn filters_reject_nonmatching_model() {
        let record = json!({
            "model": "gpt-5.4-high",
            "status_code": 200
        });
        let filters = RequestLogFilters {
            model: Some("mini".to_string()),
            ..RequestLogFilters::default()
        };

        assert!(!filters.matches(&record));
    }

    #[test]
    fn filters_match_compact_request_path() {
        let compact = json!({
            "path": "/responses/compact",
            "status_code": 200
        });
        let ordinary = json!({
            "path": "/responses",
            "status_code": 200
        });
        let filters = RequestLogFilters {
            path: Some("responses/compact".to_string()),
            ..RequestLogFilters::default()
        };

        assert!(filters.matches(&compact));
        assert!(!filters.matches(&ordinary));
    }

    #[test]
    fn tail_keeps_invalid_raw_lines_but_summary_ignores_them() {
        let lines = [
            RequestLogLine::from_raw(
                r#"{"station_name":"a","duration_ms":100,"usage":{"total_tokens":7}}"#,
            ),
            RequestLogLine::from_raw("not-json"),
            RequestLogLine::from_raw(
                r#"{"station_name":"b","duration_ms":200,"usage":{"total_tokens":3}}"#,
            ),
        ];

        assert!(!lines[1].is_valid_json());
        let rows = summarize_request_log_lines(
            lines.iter(),
            RequestUsageSummaryGroup::Station,
            &RequestLogFilters::default(),
            10,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].group_value, "a");
        assert_eq!(rows[0].aggregate.total_tokens, 7);
        assert_eq!(rows[1].group_value, "b");
        assert_eq!(rows[1].aggregate.total_tokens, 3);
    }

    fn temp_request_log_path(test_name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "codex-helper-request-ledger-{test_name}-{}-{}.jsonl",
            std::process::id(),
            crate::logging::now_ms()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn write_request_log_lines(path: &Path, lines: &[&str]) {
        std::fs::write(path, lines.join("\n")).expect("write request log lines");
    }

    #[test]
    fn request_ledger_store_tails_recent_lines_without_losing_raw_invalid_entries() {
        let path = temp_request_log_path("tail-lines");
        write_request_log_lines(
            &path,
            &[
                r#"{"path":"/one","status_code":200}"#,
                "not-json",
                r#"{"path":"/three","status_code":201}"#,
            ],
        );

        let store = RequestLedgerStore::from_path(&path);
        let lines = store.tail_lines(2).expect("tail request log");

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].raw(), "not-json");
        assert!(!lines[0].is_valid_json());
        assert_eq!(
            lines[1]
                .value()
                .and_then(|record| str_field(record, "path")),
            Some("/three")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn request_ledger_store_repairs_oversized_active_log_before_reading() {
        let path = temp_request_log_path("read-repair");
        std::fs::write(&path, vec![b'x'; 32]).expect("seed oversized request log");
        let store = RequestLedgerStore::with_retention(&path, LogRetention::new(16, 1));

        let lines = store.tail_lines(10).expect("tail repaired request log");

        assert!(lines.is_empty());
        assert!(
            !path.exists(),
            "oversized active request log should be rotated away before reading"
        );
        assert!(
            crate::local_log_store::collect_rotated_logs(&path).is_empty(),
            "oversized rotated request log should be pruned by retention budget"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn request_ledger_store_find_lines_returns_newest_matching_records() {
        let path = temp_request_log_path("find-lines");
        write_request_log_lines(
            &path,
            &[
                r#"{"path":"/v1/responses","provider_id":"a","status_code":200}"#,
                r#"{"path":"/v1/responses","provider_id":"b","status_code":429}"#,
                r#"{"path":"/v1/models","provider_id":"b","status_code":200}"#,
                r#"{"path":"/v1/responses","provider_id":"c","status_code":503}"#,
            ],
        );

        let store = RequestLedgerStore::from_path(&path);
        let lines = store
            .find_lines(
                &RequestLogFilters {
                    path: Some("responses".to_string()),
                    status_min: Some(400),
                    ..RequestLogFilters::default()
                },
                1,
            )
            .expect("find request log");

        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0]
                .value()
                .and_then(|record| str_field(record, "provider_id")),
            Some("c")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn summary_can_group_by_provider_model_or_session_with_filters() {
        let lines = [
            RequestLogLine::from_raw(
                r#"{"session_id":"sid-a","station_name":"s1","provider_id":"p1","model":"gpt-5","status_code":200,"usage":{"total_tokens":7}}"#,
            ),
            RequestLogLine::from_raw(
                r#"{"session_id":"sid-b","station_name":"s1","provider_id":"p1","model":"gpt-5.4","status_code":429,"usage":{"total_tokens":11}}"#,
            ),
            RequestLogLine::from_raw(
                r#"{"session_id":"sid-b","station_name":"s2","provider_id":"p2","model":"gpt-5.4","status_code":200,"usage":{"total_tokens":3}}"#,
            ),
        ];

        let provider_rows = summarize_request_log_lines(
            lines.iter(),
            RequestUsageSummaryGroup::Provider,
            &RequestLogFilters::default(),
            10,
        );
        assert_eq!(provider_rows[0].group_value, "p1");
        assert_eq!(provider_rows[0].aggregate.total_tokens, 18);
        assert_eq!(provider_rows[1].group_value, "p2");

        let model_rows = summarize_request_log_lines(
            lines.iter(),
            RequestUsageSummaryGroup::Model,
            &RequestLogFilters {
                status_min: Some(400),
                ..RequestLogFilters::default()
            },
            10,
        );
        assert_eq!(model_rows.len(), 1);
        assert_eq!(model_rows[0].group_value, "gpt-5.4");
        assert_eq!(model_rows[0].aggregate.total_tokens, 11);

        let session_rows = summarize_request_log_lines(
            lines.iter(),
            RequestUsageSummaryGroup::Session,
            &RequestLogFilters::default(),
            10,
        );
        assert_eq!(session_rows[0].group_value, "sid-b");
        assert_eq!(session_rows[0].aggregate.requests, 2);
    }

    #[test]
    fn request_log_record_projects_to_finished_request_for_ui_reuse() {
        let record = json!({
            "timestamp_ms": 1234,
            "request_id": 42,
            "trace_id": "codex-42",
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 1500,
            "ttfb_ms": 500,
            "station_name": "primary",
            "provider_id": "relay",
            "upstream_base_url": "https://relay.example/v1",
            "session_id": "sid-a",
            "reasoning_effort": "medium",
            "service_tier": { "actual": "priority" },
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "total_tokens": 150
            },
            "retry": {
                "attempts": 2,
                "upstream_chain": [
                    "primary:https://relay.example/v1 (idx=0) status=429 class=rate_limit model=gpt-5.4",
                    "primary:https://relay.example/v1 (idx=1) status=200 class=- model=gpt-5.4"
                ]
            }
        });

        let request =
            finished_request_from_request_log_record(&record).expect("finished request projection");

        assert_eq!(request.id, 42);
        assert_eq!(request.trace_id.as_deref(), Some("codex-42"));
        assert_eq!(request.session_id.as_deref(), Some("sid-a"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(request.service_tier.as_deref(), Some("priority"));
        assert!(request.is_fast_mode());
        assert_eq!(request.attempt_count(), 2);
        assert_eq!(request.output_tokens_per_second(), Some(50.0));
        assert_eq!(request.ended_at_ms, 1234);
    }
}
