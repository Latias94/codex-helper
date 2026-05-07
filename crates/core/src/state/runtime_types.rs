use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
pub struct UsageRollupView {
    pub since_start: UsageBucket,
    pub by_day: Vec<(i32, UsageBucket)>,
    pub by_config: Vec<(String, UsageBucket)>,
    pub by_config_day: HashMap<String, Vec<(i32, UsageBucket)>>,
    pub by_provider: Vec<(String, UsageBucket)>,
    pub by_provider_day: HashMap<String, Vec<(i32, UsageBucket)>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct UsageRollup {
    pub(super) since_start: UsageBucket,
    pub(super) by_day: HashMap<i32, UsageBucket>,
    pub(super) by_config: HashMap<String, UsageBucket>,
    pub(super) by_config_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_provider: HashMap<String, UsageBucket>,
    pub(super) by_provider_day: HashMap<String, HashMap<i32, UsageBucket>>,
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
