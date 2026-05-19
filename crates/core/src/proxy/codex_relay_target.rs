use axum::http::StatusCode;

use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamConfig};

use super::ProxyControlError;

#[derive(Debug, Clone, Copy)]
pub(super) struct CodexRelayTargetSelection<'a> {
    pub(super) station_name: Option<&'a str>,
    pub(super) upstream_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct SelectedCodexRelayTarget {
    pub(super) station_name: String,
    pub(super) upstream_index: usize,
    pub(super) upstream: UpstreamConfig,
}

pub(super) fn select_codex_relay_target(
    mgr: &ServiceConfigManager,
    selection: CodexRelayTargetSelection<'_>,
) -> Result<SelectedCodexRelayTarget, ProxyControlError> {
    let station_name = selection
        .station_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| mgr.active.clone())
        .or_else(|| stable_first_station_name(mgr))
        .ok_or_else(|| {
            ProxyControlError::new(StatusCode::BAD_REQUEST, "no codex station is configured")
        })?;
    let station = mgr.station(&station_name).ok_or_else(|| {
        ProxyControlError::new(
            StatusCode::NOT_FOUND,
            format!("station '{station_name}' not found"),
        )
    })?;
    let upstream_index = selection.upstream_index.unwrap_or(0);
    let upstream = station
        .upstreams
        .get(upstream_index)
        .cloned()
        .ok_or_else(|| {
            let (status, message) = upstream_not_found(&station_name, station, upstream_index);
            ProxyControlError::new(status, message)
        })?;
    Ok(SelectedCodexRelayTarget {
        station_name,
        upstream_index,
        upstream,
    })
}

fn stable_first_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.stations().keys().min().cloned()
}

fn upstream_not_found(
    station_name: &str,
    station: &ServiceConfig,
    upstream_index: usize,
) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        format!(
            "upstream index {upstream_index} not found for station '{}' ({} upstreams configured)",
            station_name,
            station.upstreams.len()
        ),
    )
}
