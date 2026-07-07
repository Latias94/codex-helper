use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::policy_actions::PolicyActionProjection;
use crate::pricing::{CostBreakdown, CostSummary};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageBucket {
    pub requests_total: u64,
    pub requests_error: u64,
    pub duration_ms_total: u64,
    pub requests_with_usage: u64,
    pub duration_ms_with_usage_total: u64,
    pub generation_ms_total: u64,
    pub ttfb_ms_total: u64,
    pub ttfb_samples: u64,
    pub usage: UsageMetrics,
    #[serde(default, skip_serializing_if = "CostSummary::is_empty")]
    pub cost: CostSummary,
}

impl UsageBucket {
    pub fn add_assign(&mut self, other: &Self) {
        self.requests_total = self.requests_total.saturating_add(other.requests_total);
        self.requests_error = self.requests_error.saturating_add(other.requests_error);
        self.duration_ms_total = self
            .duration_ms_total
            .saturating_add(other.duration_ms_total);
        self.requests_with_usage = self
            .requests_with_usage
            .saturating_add(other.requests_with_usage);
        self.duration_ms_with_usage_total = self
            .duration_ms_with_usage_total
            .saturating_add(other.duration_ms_with_usage_total);
        self.generation_ms_total = self
            .generation_ms_total
            .saturating_add(other.generation_ms_total);
        self.ttfb_ms_total = self.ttfb_ms_total.saturating_add(other.ttfb_ms_total);
        self.ttfb_samples = self.ttfb_samples.saturating_add(other.ttfb_samples);
        self.usage.add_assign(&other.usage);
        self.cost.add_assign(&other.cost);
    }

    pub(super) fn record(
        &mut self,
        status_code: u16,
        duration_ms: u64,
        usage: Option<&UsageMetrics>,
        cost: Option<&CostBreakdown>,
        ttfb_ms: Option<u64>,
    ) {
        self.requests_total = self.requests_total.saturating_add(1);
        if status_code >= 400 {
            self.requests_error = self.requests_error.saturating_add(1);
        }
        self.duration_ms_total = self.duration_ms_total.saturating_add(duration_ms);
        if let Some(u) = usage {
            self.usage.add_assign(u);
            if let Some(cost) = cost {
                self.cost.record_usage_cost(cost);
            } else {
                self.cost.record_usage_cost(&CostBreakdown::unknown());
            }
            self.requests_with_usage = self.requests_with_usage.saturating_add(1);
            self.duration_ms_with_usage_total = self
                .duration_ms_with_usage_total
                .saturating_add(duration_ms);

            let gen_ms = match ttfb_ms {
                Some(ttfb) if ttfb > 0 && ttfb < duration_ms => duration_ms.saturating_sub(ttfb),
                _ => duration_ms,
            };
            self.generation_ms_total = self.generation_ms_total.saturating_add(gen_ms);
            if let Some(ttfb) = ttfb_ms.filter(|v| *v > 0) {
                self.ttfb_ms_total = self.ttfb_ms_total.saturating_add(ttfb);
                self.ttfb_samples = self.ttfb_samples.saturating_add(1);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageRollupCoverage {
    pub requested_days: usize,
    pub all_loaded: bool,
    pub loaded_first_day: Option<i32>,
    pub loaded_last_day: Option<i32>,
    pub loaded_days_with_data: usize,
    pub loaded_requests: u64,
    pub window_first_day: Option<i32>,
    pub window_last_day: Option<i32>,
    pub window_days_with_data: usize,
    pub window_requests: u64,
    pub window_exceeds_loaded_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageRollupView {
    pub loaded: UsageBucket,
    pub window: UsageBucket,
    pub coverage: UsageRollupCoverage,
    pub by_day: Vec<(i32, UsageBucket)>,
    pub by_config: Vec<(String, UsageBucket)>,
    pub by_config_day: HashMap<String, Vec<(i32, UsageBucket)>>,
    pub by_provider: Vec<(String, UsageBucket)>,
    pub by_provider_day: HashMap<String, Vec<(i32, UsageBucket)>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageDayHourRow {
    pub hour: u8,
    pub bucket: UsageBucket,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageDayDimensionRow {
    pub name: String,
    pub bucket: UsageBucket,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageDayCoverage {
    pub source: String,
    pub loaded_first_ms: Option<u64>,
    pub loaded_last_ms: Option<u64>,
    pub loaded_requests: u64,
    pub scanned_lines: usize,
    pub max_lines: usize,
    pub max_bytes: usize,
    pub bytes_truncated: bool,
    pub lines_truncated: bool,
    pub day_may_be_partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageRetryGateReasonRow {
    pub reason: String,
    pub active: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageRetryGateSummary {
    pub active: u64,
    pub active_cooldowns: u64,
    pub max_remaining_secs: Option<u64>,
    pub reasons: Vec<UsageRetryGateReasonRow>,
}

impl UsageRetryGateSummary {
    pub fn from_policy_actions(actions: &[PolicyActionProjection]) -> Self {
        let mut reasons = HashMap::<String, u64>::new();
        let mut active = 0_u64;
        let mut active_cooldowns = 0_u64;
        let mut max_remaining_secs = None::<u64>;

        for action in actions {
            active = active.saturating_add(1);
            if action.active_cooldown {
                active_cooldowns = active_cooldowns.saturating_add(1);
            }
            if let Some(remaining) = action.cooldown_remaining_secs {
                max_remaining_secs = Some(max_remaining_secs.unwrap_or(0).max(remaining));
            }
            let reason = action
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("-")
                .to_string();
            let count = reasons.entry(reason).or_default();
            *count = count.saturating_add(1);
        }

        let mut reasons = reasons
            .into_iter()
            .map(|(reason, active)| UsageRetryGateReasonRow { reason, active })
            .collect::<Vec<_>>();
        reasons.sort_by(|left, right| {
            right
                .active
                .cmp(&left.active)
                .then_with(|| left.reason.cmp(&right.reason))
        });
        reasons.truncate(5);

        Self {
            active,
            active_cooldowns,
            max_remaining_secs,
            reasons,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageDayView {
    pub day: i32,
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub generated_at_ms: u64,
    pub summary: UsageBucket,
    #[serde(default)]
    pub hourly: Vec<UsageDayHourRow>,
    #[serde(default)]
    pub provider_rows: Vec<UsageDayDimensionRow>,
    #[serde(default)]
    pub station_rows: Vec<UsageDayDimensionRow>,
    #[serde(default)]
    pub model_rows: Vec<UsageDayDimensionRow>,
    #[serde(default)]
    pub session_rows: Vec<UsageDayDimensionRow>,
    #[serde(default)]
    pub project_rows: Vec<UsageDayDimensionRow>,
    #[serde(default)]
    pub retry_gate: UsageRetryGateSummary,
    #[serde(default)]
    pub coverage: UsageDayCoverage,
}

#[derive(Debug, Clone, Default)]
pub(super) struct UsageRollup {
    pub(super) loaded: UsageBucket,
    pub(super) loaded_first_ms: Option<u64>,
    pub(super) loaded_last_ms: Option<u64>,
    pub(super) coverage_source: String,
    pub(super) replay_scanned_lines: usize,
    pub(super) replay_max_lines: usize,
    pub(super) replay_max_bytes: usize,
    pub(super) replay_bytes_truncated: bool,
    pub(super) replay_lines_truncated: bool,
    pub(super) recorded_requests: HashMap<String, i32>,
    pub(super) by_day: HashMap<i32, UsageBucket>,
    pub(super) by_hour: HashMap<i32, Vec<UsageBucket>>,
    pub(super) by_config: HashMap<String, UsageBucket>,
    pub(super) by_config_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_provider: HashMap<String, UsageBucket>,
    pub(super) by_provider_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_model: HashMap<String, UsageBucket>,
    pub(super) by_model_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_session: HashMap<String, UsageBucket>,
    pub(super) by_session_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_project: HashMap<String, UsageBucket>,
    pub(super) by_project_day: HashMap<String, HashMap<i32, UsageBucket>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UpstreamHealth {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passive: Option<PassiveUpstreamHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StationHealth {
    pub checked_at_ms: u64,
    #[serde(default)]
    pub upstreams: Vec<UpstreamHealth>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PassiveHealthState {
    Healthy,
    Degraded,
    Failing,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PassiveUpstreamHealth {
    pub score: u8,
    pub state: PassiveHealthState,
    pub observed_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub recent_successes: u32,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub recent_failures: u32,
}

fn u32_is_zero(value: &u32) -> bool {
    *value == 0
}

impl PassiveUpstreamHealth {
    const MAX_RECENT_BUCKET: u32 = 6;

    pub(super) fn record_success(&mut self, now_ms: u64, status_code: Option<u16>) {
        self.observed_at_ms = now_ms;
        self.last_success_at_ms = Some(now_ms);
        self.last_status_code = status_code;
        self.last_error_class = None;
        self.last_error = None;
        self.consecutive_failures = 0;
        self.recent_successes = self
            .recent_successes
            .saturating_add(1)
            .min(Self::MAX_RECENT_BUCKET);
        self.recent_failures = self.recent_failures.saturating_sub(1);
        self.refresh_score();
    }

    pub(super) fn record_failure(
        &mut self,
        now_ms: u64,
        status_code: Option<u16>,
        error_class: Option<String>,
        error: Option<String>,
    ) {
        self.observed_at_ms = now_ms;
        self.last_failure_at_ms = Some(now_ms);
        self.last_status_code = status_code;
        self.last_error_class = error_class;
        self.last_error = error;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.recent_failures = self
            .recent_failures
            .saturating_add(1)
            .min(Self::MAX_RECENT_BUCKET);
        self.recent_successes = self.recent_successes.saturating_sub(1);
        self.refresh_score();
    }

    fn refresh_score(&mut self) {
        let mut score = 100_i32;
        score -= (self.recent_failures.min(4) as i32) * 15;

        if self.consecutive_failures > 0 {
            let consecutive_penalty =
                10 + (self.consecutive_failures.saturating_sub(1).min(3) as i32) * 10;
            score -= consecutive_penalty;
        }

        let failure_is_latest = match (self.last_failure_at_ms, self.last_success_at_ms) {
            (Some(failure), Some(success)) => failure >= success,
            (Some(_), None) => true,
            _ => false,
        };
        if failure_is_latest {
            score -= self.failure_severity();
        }

        let score = score.clamp(0, 100) as u8;
        self.score = score;
        self.state = if score >= 80 {
            PassiveHealthState::Healthy
        } else if score >= 45 {
            PassiveHealthState::Degraded
        } else {
            PassiveHealthState::Failing
        };
    }

    fn failure_severity(&self) -> i32 {
        match self.last_error_class.as_deref() {
            Some("upstream_transport_error")
            | Some("upstream_transport_error_final")
            | Some("upstream_body_read_error")
            | Some("upstream_body_read_error_final")
            | Some("upstream_stream_error")
            | Some("upstream_response_body_too_large")
            | Some("cloudflare_timeout") => 25,
            Some("cloudflare_challenge") => 20,
            Some("client_error_non_retryable") => 15,
            Some(_) => 20,
            None => match self.last_status_code {
                Some(401 | 403) => 35,
                Some(429) => 20,
                Some(500..=599) => 25,
                Some(400..=499) => 15,
                _ => 20,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LbUpstreamView {
    pub failure_count: u32,
    pub cooldown_remaining_secs: Option<u64>,
    pub usage_exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LbConfigView {
    pub last_good_index: Option<usize>,
    pub upstreams: Vec<LbUpstreamView>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConfigState {
    #[default]
    Normal,
    Draining,
    BreakerOpen,
    HalfOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct HealthCheckStatus {
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub total: u32,
    pub completed: u32,
    pub ok: u32,
    pub err: u32,
    pub cancel_requested: bool,
    pub canceled: bool,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ConfigMetaOverride {
    pub(super) enabled: Option<bool>,
    pub(super) level: Option<u8>,
    pub(super) state: Option<RuntimeConfigState>,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeDefaultProfileOverride {
    pub(super) profile_name: String,
    #[allow(dead_code)]
    pub(super) updated_at_ms: u64,
}

pub(super) fn merge_station_health(
    mut active: HashMap<String, StationHealth>,
    passive: HashMap<String, HashMap<String, PassiveUpstreamHealth>>,
) -> HashMap<String, StationHealth> {
    for (station_name, passive_upstreams) in passive {
        let max_observed_at_ms = passive_upstreams
            .values()
            .map(|health| health.observed_at_ms)
            .max()
            .unwrap_or(0);
        let entry = active.entry(station_name).or_insert_with(|| StationHealth {
            checked_at_ms: max_observed_at_ms,
            upstreams: Vec::new(),
        });
        entry.checked_at_ms = entry.checked_at_ms.max(max_observed_at_ms);

        for (base_url, passive_health) in passive_upstreams {
            if let Some(upstream) = entry
                .upstreams
                .iter_mut()
                .find(|upstream| upstream.base_url == base_url)
            {
                upstream.passive = Some(passive_health);
            } else {
                entry.upstreams.push(UpstreamHealth {
                    base_url,
                    passive: Some(passive_health),
                    ..Default::default()
                });
            }
        }
    }

    active
}
