use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Url;
use tokio::sync::{OnceCell, Semaphore};

use crate::config::{UpstreamConfig, storage::load_config};
use crate::healthcheck::{
    HEALTHCHECK_MAX_INFLIGHT_ENV, HEALTHCHECK_TIMEOUT_MS_ENV, HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV,
};
use crate::state::{ProxyState, StationHealth, UpstreamHealth};
use crate::tui::model::now_ms;

fn shorten_err(err: &str, max: usize) -> String {
    if err.chars().count() <= max {
        return err.to_string();
    }
    err.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

fn health_check_timeout() -> Duration {
    let ms = std::env::var(HEALTHCHECK_TIMEOUT_MS_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_500)
        .clamp(300, 20_000);
    Duration::from_millis(ms)
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

fn health_check_station_semaphore() -> &'static OnceCell<Arc<Semaphore>> {
    static SEM: OnceCell<Arc<Semaphore>> = OnceCell::const_new();
    &SEM
}

async fn station_permit() -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
    let sem = health_check_station_semaphore()
        .get_or_init(|| async { Arc::new(Semaphore::new(health_check_max_inflight_stations())) })
        .await;
    sem.clone().acquire_owned().await
}

fn health_check_url(base_url: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base_url)?;
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
            out.error = Some(shorten_err(&format!("invalid base_url: {e}"), 140));
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

pub(super) async fn load_upstreams_for_station(
    service_name: &str,
    station_name: &str,
) -> anyhow::Result<Vec<UpstreamConfig>> {
    let cfg = load_config().await?;
    let mgr = if service_name == "claude" {
        &cfg.claude
    } else {
        &cfg.codex
    };
    let Some(svc) = mgr.station(station_name) else {
        anyhow::bail!("station '{station_name}' not found");
    };
    Ok(svc.upstreams.clone())
}

pub(super) async fn begin_station_health_check(
    state: &ProxyState,
    service_name: &'static str,
    station_name: &str,
    upstream_count: usize,
) -> bool {
    let now = now_ms();
    if !state
        .try_begin_station_health_check(service_name, station_name, upstream_count, now)
        .await
    {
        return false;
    }

    state
        .record_station_health(
            service_name,
            station_name.to_string(),
            StationHealth {
                checked_at_ms: now,
                upstreams: Vec::new(),
            },
        )
        .await;
    true
}

async fn run_health_check_for_station(
    state: Arc<ProxyState>,
    service_name: &'static str,
    station_name: String,
    upstreams: Vec<UpstreamConfig>,
) {
    let timeout = health_check_timeout();
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
    let mut futs = FuturesUnordered::new();
    for upstream in upstreams {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        futs.push(async move {
            let _permit = sem.acquire().await;
            probe_upstream(&client, &upstream).await
        });
    }

    let mut canceled = false;
    while let Some(up) = futs.next().await {
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

pub(super) fn spawn_station_health_check(
    state: Arc<ProxyState>,
    service_name: &'static str,
    station_name: String,
    upstreams: Vec<UpstreamConfig>,
) {
    tokio::spawn(async move {
        let _permit = station_permit().await;
        run_health_check_for_station(state, service_name, station_name, upstreams).await;
    });
}

pub(super) fn spawn_all_station_health_checks(
    state: Arc<ProxyState>,
    service_name: &'static str,
    stations: Vec<String>,
) {
    tokio::spawn(async move {
        let cfg = match load_config().await {
            Ok(c) => c,
            Err(err) => {
                let now = now_ms();
                for station_name in stations {
                    state
                        .try_begin_station_health_check(service_name, &station_name, 1, now)
                        .await;
                    state
                        .record_station_health_check_result(
                            service_name,
                            &station_name,
                            now,
                            UpstreamHealth {
                                base_url: "<load_config>".to_string(),
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
                }
                return;
            }
        };

        let mgr = if service_name == "claude" {
            &cfg.claude
        } else {
            &cfg.codex
        };
        for station_name in stations {
            let Some(svc) = mgr.station(&station_name) else {
                continue;
            };
            let upstreams = svc.upstreams.clone();
            if !begin_station_health_check(&state, service_name, &station_name, upstreams.len())
                .await
            {
                continue;
            }
            spawn_station_health_check(Arc::clone(&state), service_name, station_name, upstreams);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    });
}
