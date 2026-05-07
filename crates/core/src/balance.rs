use serde::{Deserialize, Serialize};

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
            total_balance_usd: None,
            subscription_balance_usd: None,
            paygo_balance_usd: None,
            monthly_budget_usd: None,
            monthly_spent_usd: None,
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
        self.stale = self
            .stale_after_ms
            .is_some_and(|stale_after_ms| now_ms > stale_after_ms);
        self.status = if self
            .error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            BalanceSnapshotStatus::Error
        } else if self.exhausted == Some(true) {
            BalanceSnapshotStatus::Exhausted
        } else if self.stale {
            BalanceSnapshotStatus::Stale
        } else if self.exhausted == Some(false) || self.has_amount_data() {
            BalanceSnapshotStatus::Ok
        } else {
            BalanceSnapshotStatus::Unknown
        };
    }

    fn has_amount_data(&self) -> bool {
        self.total_balance_usd.is_some()
            || self.subscription_balance_usd.is_some()
            || self.paygo_balance_usd.is_some()
            || self.monthly_budget_usd.is_some()
            || self.monthly_spent_usd.is_some()
    }
}
