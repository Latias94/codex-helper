use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::bail;
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::config::{ResolvedRetryConfig, RetryConfig};
use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ApiV1Snapshot, ControlProfileOption,
    OperatorSummaryLinks, ProviderOption, StationOption, WindowStats,
};
use crate::pricing::{ModelPriceCatalogSnapshot, bundled_model_price_catalog_snapshot};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, SessionManualOverrides, SessionStats,
    StationHealth, UsageRollupView,
};

use super::attached_discovery::{attached_management_candidates, resolve_api_v1_surface};
use super::types::AttachedRefreshResult;
use super::{AttachedStatus, ProxyController, ProxyMode, send_admin_request};

mod fetch;
mod state_apply;

use fetch::refresh_from_base;
pub(super) use state_apply::apply_refresh_result as apply_attached_refresh_result;

pub(super) async fn fetch_attached_refresh(
    client: Client,
    base_candidates: Vec<String>,
) -> anyhow::Result<AttachedRefreshResult> {
    let req_timeout = Duration::from_millis(800);
    let mut last_err: Option<anyhow::Error> = None;
    for base in base_candidates {
        match refresh_from_base(&client, &base, req_timeout).await {
            Ok(result) => return Ok(result),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("attach refresh failed")))
}

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

        match rt.block_on(fetch_attached_refresh(
            self.http_client.clone(),
            base_candidates,
        )) {
            Ok(result) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    apply_attached_refresh_result(att, result);
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
