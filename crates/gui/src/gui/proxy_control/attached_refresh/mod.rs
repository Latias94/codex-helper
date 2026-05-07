use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use anyhow::bail;
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::config::{
    PersistedProviderSpec, PersistedStationProviderRef, PersistedStationSpec, ResolvedRetryConfig,
    RetryConfig,
};
use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ApiV1Snapshot, ControlProfileOption,
    HostLocalControlPlaneCapabilities, OperatorHealthSummary, OperatorRetrySummary,
    OperatorRuntimeSummary, OperatorSummaryCounts, OperatorSummaryLinks, ProviderOption,
    RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption, WindowStats,
};
use crate::pricing::{ModelPriceCatalogSnapshot, bundled_model_price_catalog_snapshot};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ProviderBalanceSnapshot,
    SessionIdentityCard, SessionManualOverrides, SessionStats, StationHealth, UsageRollupView,
};

use super::attached_discovery::{attached_management_candidates, resolve_api_v1_surface};
use super::{AttachedStatus, ProxyController, ProxyMode, send_admin_request};

mod fetch;
mod state_apply;

use fetch::refresh_from_base;
use state_apply::apply_refresh_result;

impl ProxyController {
    pub fn refresh_attached_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        let refresh_every = refresh_every.max(Duration::from_secs(1));
        let base_candidates = match &mut self.mode {
            ProxyMode::Attached(att) => {
                if let Some(last_refresh) = att.last_refresh
                    && last_refresh.elapsed() < refresh_every
                {
                    return;
                }
                att.last_refresh = Some(Instant::now());
                attached_management_candidates(att)
            }
            _ => return,
        };

        let client = self.http_client.clone();
        let fut = async move {
            let req_timeout = Duration::from_millis(800);
            let mut last_err: Option<anyhow::Error> = None;
            for base in base_candidates {
                match refresh_from_base(&client, &base, req_timeout).await {
                    Ok(result) => return Ok::<_, anyhow::Error>(result),
                    Err(err) => last_err = Some(err),
                }
            }

            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("attach refresh failed")))
        };

        match rt.block_on(fut) {
            Ok(result) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    apply_refresh_result(att, result);
                }
            }
            Err(err) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    att.last_error = Some(err.to_string());
                }
            }
        }
    }
}
