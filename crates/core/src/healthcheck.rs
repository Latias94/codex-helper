use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::Context;
use reqwest::Url;
use tokio::sync::Semaphore;

use crate::config::UpstreamConfig;
use crate::state::{ProxyState, UpstreamHealth};

pub const HEALTHCHECK_TIMEOUT_MS_ENV: &str = "CODEX_HELPER_HEALTHCHECK_TIMEOUT_MS";
pub const HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV: &str =
    "CODEX_HELPER_HEALTHCHECK_UPSTREAM_CONCURRENCY";
pub const HEALTHCHECK_MAX_INFLIGHT_ENV: &str = "CODEX_HELPER_HEALTHCHECK_MAX_INFLIGHT";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn shorten_err(err: &str, max: usize) -> String {
    if err.chars().count() <= max {
        return err.to_string();
    }
    err.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

fn health_check_timeout_ms() -> u64 {
    std::env::var(HEALTHCHECK_TIMEOUT_MS_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_500)
        .clamp(300, 20_000)
}

fn health_check_upstream_concurrency() -> usize {
    std::env::var(HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4)
        .min(32)
}

fn health_check_max_inflight_stations() -> usize {
    std::env::var(HEALTHCHECK_MAX_INFLIGHT_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2)
        .min(16)
}

fn health_check_station_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(health_check_max_inflight_stations())))
}

fn health_check_url(base_url: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base_url).with_context(|| format!("invalid base_url: {base_url}"))?;
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url.join("models")?)
}

async fn probe_upstream(client: &reqwest::Client, upstream: &UpstreamConfig) -> UpstreamHealth {
    let mut out = UpstreamHealth {
        base_url: upstream.base_url.clone(),
        ..UpstreamHealth::default()
    };

    let url = match health_check_url(&upstream.base_url) {
        Ok(u) => u,
        Err(e) => {
            out.ok = Some(false);
            out.error = Some(shorten_err(&e.to_string(), 140));
            return out;
        }
    };

    let start = Instant::now();
    let mut req = client.get(url).header("Accept", "application/json");
    if let Some(token) = upstream.auth.resolve_auth_token() {
        req = req.header("Authorization", format!("Bearer {}", token));
    } else if let Some(key) = upstream.auth.resolve_api_key() {
        req = req.header("X-API-Key", key);
    }

    match req.send().await {
        Ok(resp) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.status_code = Some(resp.status().as_u16());
            out.ok = Some(resp.status().is_success());
            if !resp.status().is_success() {
                out.error = Some(shorten_err(&format!("HTTP {}", resp.status()), 140));
            }
        }
        Err(e) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.ok = Some(false);
            out.error = Some(shorten_err(&e.to_string(), 140));
        }
    }

    out
}

pub async fn run_health_check_for_station(
    state: Arc<ProxyState>,
    service_name: &'static str,
    station_name: String,
    upstreams: Vec<UpstreamConfig>,
) {
    let _permit = health_check_station_semaphore().acquire().await;

    let timeout = std::time::Duration::from_millis(health_check_timeout_ms());
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let now = now_ms();
            state
                .record_station_health_check_result(
                    service_name,
                    &station_name,
                    now,
                    UpstreamHealth {
                        base_url: "<client>".to_string(),
                        ok: Some(false),
                        status_code: None,
                        latency_ms: None,
                        error: Some(shorten_err(&err.to_string(), 140)),
                        passive: None,
                    },
                )
                .await;
            state
                .finish_station_health_check(service_name, &station_name, now, false)
                .await;
            return;
        }
    };

    let upstream_conc = health_check_upstream_concurrency();
    let sem = Arc::new(Semaphore::new(upstream_conc));

    let mut futs = futures_util::stream::FuturesUnordered::new();
    for upstream in upstreams {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        futs.push(async move {
            let _permit = sem.acquire().await;
            probe_upstream(&client, &upstream).await
        });
    }

    let mut canceled = false;
    while let Some(up) = futures_util::StreamExt::next(&mut futs).await {
        let now = now_ms();
        state
            .record_station_health_check_result(service_name, &station_name, now, up)
            .await;
        if state
            .is_station_health_check_cancel_requested(service_name, &station_name)
            .await
        {
            canceled = true;
            break;
        }
    }

    let now = now_ms();
    state
        .finish_station_health_check(service_name, &station_name, now, canceled)
        .await;
}
