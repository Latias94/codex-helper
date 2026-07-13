use serde::{Deserialize, Serialize};

use crate::pricing::UsdAmount;
use crate::runtime_identity::ProviderEndpointKey;

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
    pub observation_provider_id: String,
    pub provider_endpoint: ProviderEndpointKey,
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
    pub quota_resets_at_ms: Option<u64>,
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
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_windows: Vec<ProviderUsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_rate: Option<ProviderUsageRateSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_model_stats: Vec<ProviderUsageModelStat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage_alerts: Vec<ProviderUsageAlert>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderUsageRateSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_duration_ms: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<String>,
}

impl ProviderUsageRateSnapshot {
    pub fn is_empty(&self) -> bool {
        self.average_duration_ms.is_none() && self.rpm.is_none() && self.tpm.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderUsageWindow {
    pub period: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderUsageModelStat {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUsageAlertKind {
    #[serde(rename = "daily_usage_80")]
    DailyUsage80,
    #[serde(rename = "daily_usage_95")]
    DailyUsage95,
    LowBalance,
    SubscriptionExpiringSoon,
    SubscriptionExpired,
}

impl ProviderUsageAlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderUsageAlertKind::DailyUsage80 => "daily_usage_80",
            ProviderUsageAlertKind::DailyUsage95 => "daily_usage_95",
            ProviderUsageAlertKind::LowBalance => "low_balance",
            ProviderUsageAlertKind::SubscriptionExpiringSoon => "subscription_expiring_soon",
            ProviderUsageAlertKind::SubscriptionExpired => "subscription_expired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderUsageAlert {
    pub kind: ProviderUsageAlertKind,
    pub message: String,
}

impl Default for ProviderBalanceSnapshot {
    fn default() -> Self {
        Self {
            observation_provider_id: String::new(),
            provider_endpoint: ProviderEndpointKey::new("", "", ""),
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
            quota_resets_at_ms: None,
            unlimited_quota: None,
            total_used_usd: None,
            today_used_usd: None,
            total_requests: None,
            today_requests: None,
            total_tokens: None,
            today_tokens: None,
            subscription_expires_at: None,
            usage_windows: Vec::new(),
            usage_rate: None,
            usage_model_stats: Vec::new(),
            usage_alerts: Vec::new(),
            error: None,
        }
    }
}

impl ProviderBalanceSnapshot {
    pub fn new(
        observation_provider_id: impl Into<String>,
        provider_endpoint: ProviderEndpointKey,
        source: impl Into<String>,
        fetched_at_ms: u64,
        stale_after_ms: Option<u64>,
    ) -> Self {
        let mut snapshot = Self {
            observation_provider_id: observation_provider_id.into(),
            provider_endpoint,
            source: source.into(),
            fetched_at_ms,
            stale_after_ms,
            ..Self::default()
        };
        snapshot.refresh_status(fetched_at_ms);
        snapshot
    }

    /// Records a public diagnostic category without retaining provider-controlled error text.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        let error = error.into();
        self.error = Some(safe_provider_balance_error(&error).to_string());
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

    pub fn has_amount_data(&self) -> bool {
        self.total_balance_usd.is_some()
            || self.subscription_balance_usd.is_some()
            || self.paygo_balance_usd.is_some()
            || self.monthly_budget_usd.is_some()
            || self.monthly_spent_usd.is_some()
            || self.quota_period.is_some()
            || self.quota_remaining_usd.is_some()
            || self.quota_limit_usd.is_some()
            || self.quota_used_usd.is_some()
            || self.quota_resets_at_ms.is_some()
            || self.unlimited_quota == Some(true)
            || self.total_used_usd.is_some()
            || self.today_used_usd.is_some()
    }

    pub fn carry_forward_amount_data_from(&mut self, previous: &Self) {
        if self.has_amount_data() {
            return;
        }
        self.plan_name = previous.plan_name.clone();
        self.total_balance_usd = previous.total_balance_usd.clone();
        self.subscription_balance_usd = previous.subscription_balance_usd.clone();
        self.paygo_balance_usd = previous.paygo_balance_usd.clone();
        self.monthly_budget_usd = previous.monthly_budget_usd.clone();
        self.monthly_spent_usd = previous.monthly_spent_usd.clone();
        self.quota_period = previous.quota_period.clone();
        self.quota_remaining_usd = previous.quota_remaining_usd.clone();
        self.quota_limit_usd = previous.quota_limit_usd.clone();
        self.quota_used_usd = previous.quota_used_usd.clone();
        self.quota_resets_at_ms = previous.quota_resets_at_ms;
        self.unlimited_quota = previous.unlimited_quota;
        self.total_used_usd = previous.total_used_usd.clone();
        self.today_used_usd = previous.today_used_usd.clone();
        self.total_requests = previous.total_requests;
        self.today_requests = previous.today_requests;
        self.total_tokens = previous.total_tokens;
        self.today_tokens = previous.today_tokens;
        self.subscription_expires_at = previous.subscription_expires_at.clone();
        self.usage_windows = previous.usage_windows.clone();
        self.usage_rate = previous.usage_rate.clone();
        self.usage_model_stats = previous.usage_model_stats.clone();
        self.usage_alerts = previous.usage_alerts.clone();
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
        } else {
            if let Some(total) = self.total_balance_usd.as_deref() {
                parts.push(format!("total=${total}"));
            }
            if let Some(quota) = self.quota_summary() {
                parts.push(quota);
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
        if !self.usage_alerts.is_empty() {
            let alerts = self
                .usage_alerts
                .iter()
                .map(|alert| alert.kind.as_str())
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("alerts={alerts}"));
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
pub struct ProviderRoutingBalanceSummary {
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

impl ProviderRoutingBalanceSummary {
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

const ERROR_CLASSIFICATION_MAX_CHARS: usize = 2_048;

pub(crate) fn safe_provider_balance_error(error: &str) -> &'static str {
    let normalized = error
        .chars()
        .take(ERROR_CLASSIFICATION_MAX_CHARS)
        .collect::<String>()
        .to_ascii_lowercase();
    if contains_any(
        &normalized,
        &[
            "no usable token",
            "missing token",
            "missing credential",
            "token env",
            "credential unavailable",
        ],
    ) {
        "provider credential unavailable"
    } else if contains_any(
        &normalized,
        &[
            "authentication failed",
            "authorization failed",
            "authorization required",
            "invalid api key",
            "api key invalid",
            "api key is invalid",
            "api key rejected",
            "api key disabled",
            "api key inactive",
            "invalid apikey",
            "apikey invalid",
            "invalid bearer token",
            "bearer token invalid",
            "unauthorized",
            "forbidden",
            "invalid token",
            "token invalid",
            "login required",
            "http 401",
            "status 401",
            "http 403",
            "status 403",
            "密钥无效",
            "令牌无效",
        ],
    ) {
        "authentication failed"
    } else if contains_any(
        &normalized,
        &[
            "user inactive",
            "account inactive",
            "account is not active",
            "account unavailable",
            "account disabled",
            "user disabled",
            "账户未激活",
            "账号未激活",
            "用户未激活",
            "账户已禁用",
            "账号已禁用",
            "用户已禁用",
        ],
    ) {
        "provider account unavailable"
    } else if contains_any(
        &normalized,
        &[
            "insufficient balance",
            "balance insufficient",
            "insufficient quota",
            "quota exhausted",
            "quota exceeded",
            "no balance",
            "余额不足",
            "额度不足",
            "配额不足",
        ],
    ) {
        "provider quota exhausted"
    } else if contains_any(
        &normalized,
        &["http 429", "status 429", "too many requests", "rate limit"],
    ) {
        "provider rate limited"
    } else if contains_any(&normalized, &["timed out", "timeout", "deadline exceeded"]) {
        "provider request timed out"
    } else if contains_any(
        &normalized,
        &[
            "connection",
            "connect error",
            "error sending request",
            "network error",
            "dns error",
            "tls error",
            "certificate error",
            "socket error",
        ],
    ) {
        "provider connection failed"
    } else if contains_any(
        &normalized,
        &[
            "invalid provider response",
            "decode response",
            "deserialize response",
            "parse response",
            "invalid response",
            "malformed response",
            "unexpected response",
            "response body",
            "response read failed",
            "schema validation",
        ],
    ) {
        "invalid provider response"
    } else if contains_any(
        &normalized,
        &[
            "provider refresh temporarily unavailable",
            "temporarily suppressed",
            "refresh cooldown",
            "refresh cooling down",
        ],
    ) {
        "provider refresh temporarily unavailable"
    } else {
        "provider request failed"
    }
}

fn contains_any(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_balance_error_is_reduced_to_safe_authentication_category() {
        let snapshot = ProviderBalanceSnapshot::default().with_error(
            "HTTP 401: {\"message\":\"Authorization: Bearer sk-live-secret\\napi_key=key-secret\",\"url\":\"https://relay.example/v1?token=query-secret\"}",
        );

        assert_eq!(snapshot.error.as_deref(), Some("authentication failed"));
        let encoded = serde_json::to_string(&snapshot).expect("serialize provider balance");
        for secret in [
            "sk-live-secret",
            "key-secret",
            "query-secret",
            "Authorization",
            "Bearer",
            "api_key",
            "?token=",
            "{\\\"message\\\"",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}: {encoded}");
        }
        assert!(!encoded.chars().any(char::is_control), "{encoded:?}");
    }

    #[test]
    fn provider_balance_error_preserves_only_diagnostic_category() {
        for (error, expected) in [
            (
                "USER_INACTIVE: User account is not active",
                "provider account unavailable",
            ),
            (
                "insufficient balance for this request",
                "provider quota exhausted",
            ),
            ("HTTP 429 Too Many Requests", "provider rate limited"),
            (
                "request timed out while reading response",
                "provider request timed out",
            ),
            (
                "connection refused by upstream",
                "provider connection failed",
            ),
            (
                "failed to decode response body as JSON",
                "invalid provider response",
            ),
            (
                "no usable token; checked upstream auth",
                "provider credential unavailable",
            ),
            (
                "all balance probe kinds are temporarily suppressed",
                "provider refresh temporarily unavailable",
            ),
            (
                "opaque failure Authorization: Bearer sk-secret url=https://relay.example/v1?api_key=query-secret",
                "provider request failed",
            ),
        ] {
            let snapshot = ProviderBalanceSnapshot::default().with_error(error);
            assert_eq!(snapshot.error.as_deref(), Some(expected), "{error}");
        }
    }

    #[test]
    fn provider_balance_error_categories_are_idempotent() {
        for category in [
            "authentication failed",
            "provider account unavailable",
            "provider quota exhausted",
            "provider rate limited",
            "provider request timed out",
            "provider connection failed",
            "invalid provider response",
            "provider credential unavailable",
            "provider refresh temporarily unavailable",
            "provider request failed",
        ] {
            let snapshot = ProviderBalanceSnapshot::default().with_error(category);
            assert_eq!(snapshot.error.as_deref(), Some(category), "{category}");
        }
    }

    #[test]
    fn balance_snapshot_status_labels_are_stable() {
        assert_eq!(BalanceSnapshotStatus::Unknown.as_str(), "unknown");
        assert_eq!(BalanceSnapshotStatus::Ok.as_str(), "ok");
        assert_eq!(BalanceSnapshotStatus::Exhausted.as_str(), "exhausted");
        assert_eq!(BalanceSnapshotStatus::Stale.as_str(), "stale");
        assert_eq!(BalanceSnapshotStatus::Error.as_str(), "error");
    }

    #[test]
    fn provider_usage_alert_wire_codes_match_stable_codes() {
        for kind in [
            ProviderUsageAlertKind::DailyUsage80,
            ProviderUsageAlertKind::DailyUsage95,
            ProviderUsageAlertKind::LowBalance,
            ProviderUsageAlertKind::SubscriptionExpiringSoon,
            ProviderUsageAlertKind::SubscriptionExpired,
        ] {
            assert_eq!(
                serde_json::to_value(kind).expect("serialize provider usage alert kind"),
                serde_json::Value::String(kind.as_str().to_string())
            );
        }
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
    fn provider_balance_amount_summary_keeps_wallet_with_quota() {
        let snapshot = ProviderBalanceSnapshot {
            plan_name: Some("rightcode".to_string()),
            total_balance_usd: Some("3.25".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("7.5".to_string()),
            quota_limit_usd: Some("20".to_string()),
            quota_used_usd: Some("12.5".to_string()),
            ..Default::default()
        };

        assert_eq!(
            snapshot.amount_summary(),
            "plan=rightcode total=$3.25 daily quota left=$7.5 limit=$20 used=$12.5"
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

    #[test]
    fn provider_balance_amount_summary_includes_usage_alerts() {
        let snapshot = ProviderBalanceSnapshot {
            usage_alerts: vec![ProviderUsageAlert {
                kind: ProviderUsageAlertKind::LowBalance,
                message: "remaining balance is low".to_string(),
            }],
            ..Default::default()
        };

        assert_eq!(snapshot.amount_summary(), "alerts=low_balance");
    }
}
