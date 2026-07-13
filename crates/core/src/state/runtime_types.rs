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
    pub by_provider_endpoint: Vec<(String, UsageBucket)>,
    pub by_provider_endpoint_day: HashMap<String, Vec<(i32, UsageBucket)>>,
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
    pub provider_endpoint_rows: Vec<UsageDayDimensionRow>,
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
    pub(super) terminal_range_by_day: HashMap<i32, (u64, u64)>,
    pub(super) recorded_requests: HashMap<String, i32>,
    pub(super) by_day: HashMap<i32, UsageBucket>,
    pub(super) by_hour: HashMap<i32, Vec<UsageBucket>>,
    pub(super) by_provider_endpoint: HashMap<String, UsageBucket>,
    pub(super) by_provider_endpoint_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_provider: HashMap<String, UsageBucket>,
    pub(super) by_provider_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_model: HashMap<String, UsageBucket>,
    pub(super) by_model_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_session: HashMap<String, UsageBucket>,
    pub(super) by_session_day: HashMap<String, HashMap<i32, UsageBucket>>,
    pub(super) by_project: HashMap<String, UsageBucket>,
    pub(super) by_project_day: HashMap<String, HashMap<i32, UsageBucket>>,
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
