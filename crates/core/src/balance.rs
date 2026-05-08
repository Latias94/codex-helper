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
            exhaustion_affects_routing: true,
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

    pub fn routing_exhausted(&self) -> bool {
        self.exhaustion_affects_routing && self.status == BalanceSnapshotStatus::Exhausted
    }

    fn has_amount_data(&self) -> bool {
        self.total_balance_usd.is_some()
            || self.subscription_balance_usd.is_some()
            || self.paygo_balance_usd.is_some()
            || self.monthly_budget_usd.is_some()
            || self.monthly_spent_usd.is_some()
    }

    pub fn amount_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(total) = self.total_balance_usd.as_deref() {
            parts.push(format!("total=${total}"));
        }
        if let Some(budget) = self.monthly_budget_usd.as_deref() {
            parts.push(format!("budget=${budget}"));
        }
        if let Some(spent) = self.monthly_spent_usd.as_deref() {
            parts.push(format!("spent=${spent}"));
        }
        if let Some(sub) = self.subscription_balance_usd.as_deref() {
            parts.push(format!("sub=${sub}"));
        }
        if let Some(paygo) = self.paygo_balance_usd.as_deref() {
            parts.push(format!("paygo=${paygo}"));
        }
        if parts.is_empty() {
            "-".to_string()
        } else {
            parts.join(" ")
        }
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
            total_balance_usd: Some("3.5".to_string()),
            monthly_budget_usd: Some("5".to_string()),
            monthly_spent_usd: Some("1.25".to_string()),
            subscription_balance_usd: Some("2".to_string()),
            paygo_balance_usd: Some("1.5".to_string()),
            ..Default::default()
        };

        assert_eq!(
            snapshot.amount_summary(),
            "total=$3.5 budget=$5 spent=$1.25 sub=$2 paygo=$1.5"
        );
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
    }
}
