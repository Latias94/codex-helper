use serde::{Deserialize, Serialize};

use crate::pricing::UsdAmount;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BalanceSnapshotStatus {
    #[default]
    Unknown,
    Ok,
    Exhausted,
    Stale,
    Error,
}

impl BalanceSnapshotStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BalanceSnapshotStatus::Unknown => "unknown",
            BalanceSnapshotStatus::Ok => "ok",
            BalanceSnapshotStatus::Exhausted => "exhausted",
            BalanceSnapshotStatus::Stale => "stale",
            BalanceSnapshotStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBalanceSnapshot {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_index: Option<usize>,
    pub source: String,
    pub fetched_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_ms: Option<u64>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub status: BalanceSnapshotStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausted: Option<bool>,
    #[serde(
        default = "default_exhaustion_affects_routing",
        skip_serializing_if = "bool_is_true"
    )]
    pub exhaustion_affects_routing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paygo_balance_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_budget_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_spent_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_period: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_remaining_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_limit_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited_quota: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for ProviderBalanceSnapshot {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            station_name: None,
            upstream_index: None,
            source: String::new(),
            fetched_at_ms: 0,
            stale_after_ms: None,
            stale: false,
            status: BalanceSnapshotStatus::Unknown,
            exhausted: None,
            exhaustion_affects_routing: true,
            plan_name: None,
            total_balance_usd: None,
            subscription_balance_usd: None,
            paygo_balance_usd: None,
            monthly_budget_usd: None,
            monthly_spent_usd: None,
            quota_period: None,
            quota_remaining_usd: None,
            quota_limit_usd: None,
            quota_used_usd: None,
            unlimited_quota: None,
            total_used_usd: None,
            today_used_usd: None,
            total_requests: None,
            today_requests: None,
            total_tokens: None,
            today_tokens: None,
            error: None,
        }
    }
}

impl ProviderBalanceSnapshot {
    pub fn new(
        provider_id: impl Into<String>,
        station_name: impl Into<String>,
        upstream_index: usize,
        source: impl Into<String>,
        fetched_at_ms: u64,
        stale_after_ms: Option<u64>,
    ) -> Self {
        let mut snapshot = Self {
            provider_id: provider_id.into(),
            station_name: Some(station_name.into()),
            upstream_index: Some(upstream_index),
            source: source.into(),
            fetched_at_ms,
            stale_after_ms,
            ..Self::default()
        };
        snapshot.refresh_status(fetched_at_ms);
        snapshot
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self.exhausted = None;
        self.refresh_status(self.fetched_at_ms);
        self
    }

    pub fn refresh_status(&mut self, now_ms: u64) {
        self.stale = self.stale_at(now_ms);
        self.status = self.status_at(now_ms);
    }

    pub fn stale_at(&self, now_ms: u64) -> bool {
        self.stale_after_ms
            .is_some_and(|stale_after_ms| now_ms > stale_after_ms)
    }

    pub fn status_at(&self, now_ms: u64) -> BalanceSnapshotStatus {
        let stale = self.stale_at(now_ms);
        if self
            .error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            BalanceSnapshotStatus::Error
        } else if self.exhausted == Some(true) {
            BalanceSnapshotStatus::Exhausted
        } else if stale {
            BalanceSnapshotStatus::Stale
        } else if self.exhausted == Some(false) || self.has_amount_data() {
            BalanceSnapshotStatus::Ok
        } else {
            BalanceSnapshotStatus::Unknown
        }
    }

    pub fn routing_exhausted(&self) -> bool {
        self.exhaustion_affects_routing && self.status == BalanceSnapshotStatus::Exhausted
    }

    pub fn routing_ignored_exhaustion(&self) -> bool {
        self.status == BalanceSnapshotStatus::Exhausted && !self.exhaustion_affects_routing
    }

    fn has_amount_data(&self) -> bool {
        self.total_balance_usd.is_some()
            || self.subscription_balance_usd.is_some()
            || self.paygo_balance_usd.is_some()
            || self.monthly_budget_usd.is_some()
            || self.monthly_spent_usd.is_some()
            || self.quota_period.is_some()
            || self.quota_remaining_usd.is_some()
            || self.quota_limit_usd.is_some()
            || self.quota_used_usd.is_some()
            || self.unlimited_quota == Some(true)
            || self.total_used_usd.is_some()
            || self.today_used_usd.is_some()
    }

    pub fn amount_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(plan) = self.plan_name.as_deref()
            && !plan.trim().is_empty()
        {
            parts.push(format!("plan={plan}"));
        }
        if self.unlimited_quota == Some(true) {
            parts.push("unlimited".to_string());
        } else if let Some(quota) = self.quota_summary() {
            parts.push(quota);
        } else {
            if let Some(total) = self.total_balance_usd.as_deref() {
                parts.push(format!("total=${total}"));
            }
            match (
                self.monthly_budget_usd.as_deref(),
                self.monthly_spent_usd.as_deref(),
            ) {
                (Some(budget), Some(spent)) => {
                    if let Some(left) = left_from_budget_and_spent(budget, spent) {
                        parts.push(format!("left=${left} budget=${budget} spent=${spent}"));
                    } else {
                        parts.push(format!("budget=${budget} spent=${spent}"));
                    }
                }
                (Some(budget), None) => parts.push(format!("budget=${budget}")),
                (None, Some(spent)) => parts.push(format!("used=${spent}")),
                (None, None) => {}
            }
            if let Some(used) = self.total_used_usd.as_deref() {
                parts.push(format!("used=${used}"));
            }
            if let Some(today) = self.today_used_usd.as_deref() {
                parts.push(format!("today=${today}"));
            }
            if let Some(sub) = self.subscription_balance_usd.as_deref() {
                parts.push(format!("sub=${sub}"));
            }
            if let Some(paygo) = self.paygo_balance_usd.as_deref() {
                parts.push(format!("paygo=${paygo}"));
            }
        }
        if let Some(requests) = self.total_requests {
            parts.push(format!("req={requests}"));
        }
        if let Some(tokens) = self.total_tokens {
            parts.push(format!("tok={tokens}"));
        }
        if parts.is_empty() {
            "-".to_string()
        } else {
            parts.join(" ")
        }
    }

    fn quota_summary(&self) -> Option<String> {
        let period = self
            .quota_period
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let remaining = self
            .quota_remaining_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let limit = self
            .quota_limit_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let used = self
            .quota_used_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if remaining.is_none() && limit.is_none() && used.is_none() {
            return None;
        }

        let mut parts = Vec::new();
        let quota_label = match period {
            Some("quota") | None => "quota".to_string(),
            Some(period) => format!("{period} quota"),
        };
        parts.push(quota_label);

        match (remaining, limit, used) {
            (Some(remaining), Some(limit), Some(used)) => {
                parts.push(format!("left=${remaining} limit=${limit} used=${used}"))
            }
            (Some(remaining), Some(limit), None) => {
                parts.push(format!("left=${remaining} limit=${limit}"))
            }
            (Some(remaining), None, Some(used)) => {
                parts.push(format!("left=${remaining} used=${used}"))
            }
            (Some(remaining), None, None) => parts.push(format!("left=${remaining}")),
            (None, Some(limit), Some(used)) => parts.push(format!("used=${used} limit=${limit}")),
            (None, Some(limit), None) => parts.push(format!("limit=${limit}")),
            (None, None, Some(used)) => parts.push(format!("used=${used}")),
            (None, None, None) => {}
        }

        Some(parts.join(" "))
    }
}

fn left_from_budget_and_spent(budget: &str, spent: &str) -> Option<String> {
    let budget = UsdAmount::from_decimal_str(budget)?;
    let spent = UsdAmount::from_decimal_str(spent)?;
    Some(budget.saturating_sub(spent).format_usd())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StationRoutingBalanceSummary {
    pub snapshots: usize,
    #[serde(default)]
    pub ok: usize,
    #[serde(default)]
    pub exhausted: usize,
    #[serde(default)]
    pub stale: usize,
    #[serde(default)]
    pub error: usize,
    #[serde(default)]
    pub unknown: usize,
    #[serde(default)]
    pub routing_snapshots: usize,
    #[serde(default)]
    pub routing_exhausted: usize,
    #[serde(default)]
    pub routing_ignored_exhausted: usize,
}

impl StationRoutingBalanceSummary {
    pub fn from_snapshots(snapshots: Option<&[ProviderBalanceSnapshot]>) -> Self {
        let mut out = Self::default();
        let Some(snapshots) = snapshots else {
            return out;
        };

        for snapshot in snapshots {
            out.record(snapshot, snapshot.status);
        }
        out
    }

    pub fn from_snapshot_iter_at<'a>(
        snapshots: impl IntoIterator<Item = &'a ProviderBalanceSnapshot>,
        now_ms: u64,
    ) -> Self {
        let mut out = Self::default();
        for snapshot in snapshots {
            out.record(snapshot, snapshot.status_at(now_ms));
        }
        out
    }

    fn record(&mut self, snapshot: &ProviderBalanceSnapshot, status: BalanceSnapshotStatus) {
        self.snapshots += 1;
        match status {
            BalanceSnapshotStatus::Ok => self.ok += 1,
            BalanceSnapshotStatus::Exhausted => self.exhausted += 1,
            BalanceSnapshotStatus::Stale => self.stale += 1,
            BalanceSnapshotStatus::Error => self.error += 1,
            BalanceSnapshotStatus::Unknown => self.unknown += 1,
        }
        if snapshot.exhaustion_affects_routing {
            self.routing_snapshots += 1;
            if status == BalanceSnapshotStatus::Exhausted {
                self.routing_exhausted += 1;
            }
        } else if status == BalanceSnapshotStatus::Exhausted {
            self.routing_ignored_exhausted += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots == 0
    }
}

fn default_exhaustion_affects_routing() -> bool {
    true
}

fn bool_is_true(value: &bool) -> bool {
    *value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_snapshot_status_labels_are_stable() {
        assert_eq!(BalanceSnapshotStatus::Unknown.as_str(), "unknown");
        assert_eq!(BalanceSnapshotStatus::Ok.as_str(), "ok");
        assert_eq!(BalanceSnapshotStatus::Exhausted.as_str(), "exhausted");
        assert_eq!(BalanceSnapshotStatus::Stale.as_str(), "stale");
        assert_eq!(BalanceSnapshotStatus::Error.as_str(), "error");
    }

    #[test]
    fn provider_balance_amount_summary_formats_known_amounts() {
        let snapshot = ProviderBalanceSnapshot {
            plan_name: Some("monthly".to_string()),
            total_balance_usd: Some("3.5".to_string()),
            monthly_budget_usd: Some("5".to_string()),
            monthly_spent_usd: Some("1.25".to_string()),
            total_used_usd: Some("7".to_string()),
            today_used_usd: Some("0.5".to_string()),
            subscription_balance_usd: Some("2".to_string()),
            paygo_balance_usd: Some("1.5".to_string()),
            total_requests: Some(42),
            total_tokens: Some(1234),
            ..Default::default()
        };

        assert_eq!(
            snapshot.amount_summary(),
            "plan=monthly total=$3.5 left=$3.75 budget=$5 spent=$1.25 used=$7 today=$0.5 sub=$2 paygo=$1.5 req=42 tok=1234"
        );
    }

    #[test]
    fn provider_balance_amount_summary_prioritizes_unlimited_quota() {
        let snapshot = ProviderBalanceSnapshot {
            plan_name: Some("cx".to_string()),
            unlimited_quota: Some(true),
            quota_used_usd: Some("106065.94".to_string()),
            ..Default::default()
        };

        assert_eq!(snapshot.amount_summary(), "plan=cx unlimited");
    }

    #[test]
    fn routing_exhausted_respects_snapshot_routing_flag() {
        let mut snapshot = ProviderBalanceSnapshot {
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            ..Default::default()
        };
        snapshot.refresh_status(100);

        assert_eq!(snapshot.status, BalanceSnapshotStatus::Exhausted);
        assert!(!snapshot.routing_exhausted());
        assert!(snapshot.routing_ignored_exhaustion());
    }
}
