use std::collections::HashMap;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use reqwest::Url;
use serde::Deserialize;

use crate::state::ProviderBalanceSnapshot;
use crate::usage_providers::UsageProviderRefreshSummary;

use super::types::{
    ProviderBalanceRefreshResult, ProviderBalanceRefreshStatus, ProviderBalanceRefreshTask,
};
use super::{ProxyController, ProxyMode, send_admin_request};

const PROVIDER_BALANCE_REFRESH_PATH: &str = "/__codex_helper/api/v1/providers/balances/refresh";

#[derive(Debug, Deserialize)]
struct ProviderBalanceRefreshResponse {
    refresh: UsageProviderRefreshSummary,
    provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
}

fn build_provider_balance_refresh_url(
    base: &str,
    station_name: Option<&str>,
    provider_id: Option<&str>,
) -> anyhow::Result<String> {
    let mut url = Url::parse(base)
        .with_context(|| format!("invalid admin base url: {base}"))?
        .join(PROVIDER_BALANCE_REFRESH_PATH)
        .context("invalid provider balance refresh endpoint")?;

    {
        let mut pairs = url.query_pairs_mut();
        if let Some(station_name) = station_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            pairs.append_pair("station_name", station_name);
        }
        if let Some(provider_id) = provider_id.map(str::trim).filter(|value| !value.is_empty()) {
            pairs.append_pair("provider_id", provider_id);
        }
    }

    Ok(url.to_string())
}

fn format_balance_refresh_summary(summary: &UsageProviderRefreshSummary) -> String {
    if summary.deduplicated > 0 {
        return "already refreshing".to_string();
    }

    format!(
        "attempted={} refreshed={} failed={} missing_token={} auto={}/{}",
        summary.attempted,
        summary.refreshed,
        summary.failed,
        summary.missing_token,
        summary.auto_refreshed,
        summary.auto_attempted
    )
}

impl ProxyController {
    pub fn supports_provider_balance_refresh(&self) -> bool {
        match &self.mode {
            ProxyMode::Running(_) => true,
            ProxyMode::Attached(att) => {
                att.api_version == Some(1) && att.supports_provider_balance_refresh_api
            }
            _ => false,
        }
    }

    pub fn provider_balance_refresh_status(&self) -> &ProviderBalanceRefreshStatus {
        &self.provider_balance_refresh_status
    }

    pub fn request_provider_balance_refresh(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
        provider_id: Option<String>,
    ) -> anyhow::Result<bool> {
        self.poll_provider_balance_refresh();
        if self.provider_balance_refresh.is_some() {
            return Ok(false);
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => format!("http://127.0.0.1:{}", r.admin_port),
            ProxyMode::Attached(att)
                if att.api_version == Some(1) && att.supports_provider_balance_refresh_api =>
            {
                att.admin_base_url.clone()
            }
            ProxyMode::Attached(_) => {
                bail!("attached proxy does not expose provider balance refresh")
            }
            _ => bail!("proxy is not running"),
        };
        let url = build_provider_balance_refresh_url(
            &base,
            station_name.as_deref(),
            provider_id.as_deref(),
        )?;

        let client = self.http_client.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let join = rt.spawn(async move {
            let result = async move {
                let response =
                    send_admin_request(client.post(url).timeout(Duration::from_secs(15))).await?;
                let parsed = response
                    .json::<ProviderBalanceRefreshResponse>()
                    .await
                    .context("decode provider balance refresh response")?;
                Ok::<_, anyhow::Error>(ProviderBalanceRefreshResult {
                    refresh: parsed.refresh,
                    provider_balances: parsed.provider_balances,
                })
            }
            .await;
            let _ = tx.send(result);
        });

        self.provider_balance_refresh = Some(ProviderBalanceRefreshTask { rx, join });
        self.provider_balance_refresh_status.refreshing = true;
        self.provider_balance_refresh_status.last_started_at = Some(Instant::now());
        self.provider_balance_refresh_status.last_message = Some("refreshing".to_string());
        self.provider_balance_refresh_status.last_error = None;
        Ok(true)
    }

    pub fn poll_provider_balance_refresh(&mut self) {
        let outcome = match self.provider_balance_refresh.as_ref() {
            Some(task) => match task.rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "provider balance refresh task disconnected"
                ))),
            },
            None => None,
        };

        let Some(outcome) = outcome else {
            return;
        };
        if let Some(task) = self.provider_balance_refresh.take() {
            task.join.abort();
        }

        self.provider_balance_refresh_status.refreshing = false;
        self.provider_balance_refresh_status.last_finished_at = Some(Instant::now());
        match outcome {
            Ok(result) => {
                match &mut self.mode {
                    ProxyMode::Running(r) => r.provider_balances = result.provider_balances,
                    ProxyMode::Attached(att) => att.provider_balances = result.provider_balances,
                    _ => {}
                }
                self.provider_balance_refresh_status.last_message =
                    Some(format_balance_refresh_summary(&result.refresh));
                self.provider_balance_refresh_status.last_error = None;
            }
            Err(err) => {
                self.provider_balance_refresh_status.last_message = None;
                self.provider_balance_refresh_status.last_error = Some(err.to_string());
            }
        }
    }

    pub(super) fn clear_provider_balance_refresh(&mut self) {
        if let Some(task) = self.provider_balance_refresh.take() {
            task.join.abort();
        }
        self.provider_balance_refresh_status.refreshing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_refresh_url_keeps_filters_encoded() {
        let url = build_provider_balance_refresh_url(
            "http://127.0.0.1:4321",
            Some("monthly pool"),
            Some("provider/a"),
        )
        .expect("url");

        assert!(url.starts_with(
            "http://127.0.0.1:4321/__codex_helper/api/v1/providers/balances/refresh?"
        ));
        assert!(url.contains("station_name=monthly+pool"));
        assert!(url.contains("provider_id=provider%2Fa"));
    }

    #[test]
    fn balance_refresh_summary_reports_deduplicated_refresh() {
        let summary = UsageProviderRefreshSummary {
            deduplicated: 1,
            ..UsageProviderRefreshSummary::default()
        };

        assert_eq!(
            format_balance_refresh_summary(&summary),
            "already refreshing"
        );
    }
}
