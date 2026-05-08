use serde::{Deserialize, Serialize};

use crate::config::{ResolvedRetryConfig, RetryStrategy};
use crate::state::{
    BalanceSnapshotStatus, LbConfigView, ProviderBalanceSnapshot, RuntimeConfigState,
};

use super::types::StationOption;

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
}

impl StationRoutingBalanceSummary {
    pub fn from_snapshots(snapshots: Option<&[ProviderBalanceSnapshot]>) -> Self {
        let mut out = Self::default();
        let Some(snapshots) = snapshots else {
            return out;
        };

        out.snapshots = snapshots.len();
        for snapshot in snapshots {
            match snapshot.status {
                BalanceSnapshotStatus::Ok => out.ok += 1,
                BalanceSnapshotStatus::Exhausted => out.exhausted += 1,
                BalanceSnapshotStatus::Stale => out.stale += 1,
                BalanceSnapshotStatus::Error => out.error += 1,
                BalanceSnapshotStatus::Unknown => out.unknown += 1,
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StationRoutingCandidate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub level: u8,
    pub enabled: bool,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstreams: Option<usize>,
    #[serde(default)]
    pub runtime_state: RuntimeConfigState,
    #[serde(default)]
    pub has_cooldown: bool,
    #[serde(default)]
    pub any_usage_exhausted: bool,
    #[serde(default)]
    pub all_usage_exhausted: bool,
    #[serde(
        default,
        skip_serializing_if = "StationRoutingBalanceSummary::is_empty"
    )]
    pub balance: StationRoutingBalanceSummary,
}

impl StationRoutingCandidate {
    pub fn from_station_option(
        station: &StationOption,
        configured_active_station: Option<&str>,
        lb: Option<&LbConfigView>,
        balances: Option<&[ProviderBalanceSnapshot]>,
    ) -> Self {
        let upstreams = lb.map(|view| view.upstreams.len());
        let has_cooldown = lb.is_some_and(|view| {
            view.upstreams
                .iter()
                .any(|upstream| upstream.cooldown_remaining_secs.is_some())
        });
        let any_usage_exhausted = lb.is_some_and(|view| {
            view.upstreams
                .iter()
                .any(|upstream| upstream.usage_exhausted)
        });
        let all_usage_exhausted = lb.is_some_and(|view| {
            !view.upstreams.is_empty()
                && view
                    .upstreams
                    .iter()
                    .all(|upstream| upstream.usage_exhausted)
        });

        Self {
            name: station.name.clone(),
            alias: station.alias.clone(),
            level: station.level.clamp(1, 10),
            enabled: station.enabled,
            active: configured_active_station == Some(station.name.as_str()),
            upstreams,
            runtime_state: station.runtime_state,
            has_cooldown,
            any_usage_exhausted,
            all_usage_exhausted,
            balance: StationRoutingBalanceSummary::from_snapshots(balances),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StationRoutingSource {
    SessionPin(String),
    GlobalPin(String),
    ConfiguredActiveStation(String),
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StationRoutingMode {
    PinnedStation,
    AutoLevelFallback,
    AutoSingleLevelFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StationRoutingSkipReason {
    Disabled,
    RuntimeState(RuntimeConfigState),
    NoRoutableUpstreams,
    MissingPinnedTarget,
    BreakerOpenBlocksPinned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StationRoutingSkipped {
    pub station_name: String,
    pub reasons: Vec<StationRoutingSkipReason>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StationRetryBoundary {
    Unknown,
    CrossStationBeforeFirstOutput {
        provider_max_attempts: u32,
    },
    CurrentStationFirst {
        provider_strategy: RetryStrategy,
        provider_max_attempts: u32,
    },
    NextRequestOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StationRoutingPosture {
    pub source: StationRoutingSource,
    pub mode: StationRoutingMode,
    pub eligible_candidates: Vec<StationRoutingCandidate>,
    pub skipped: Vec<StationRoutingSkipped>,
    pub retry_boundary: StationRetryBoundary,
    #[serde(default)]
    pub session_pin_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct StationRoutingPostureInput<'a> {
    pub stations: &'a [StationRoutingCandidate],
    pub session_station_override: Option<&'a str>,
    pub global_station_override: Option<&'a str>,
    pub configured_active_station: Option<&'a str>,
    pub session_pin_count: usize,
    pub retry: Option<&'a ResolvedRetryConfig>,
}

pub fn build_station_routing_posture(
    input: StationRoutingPostureInput<'_>,
) -> StationRoutingPosture {
    let retry_boundary = retry_boundary_from_config(input.retry);

    if let Some(session_pin) = non_empty_trimmed(input.session_station_override) {
        let mut posture = build_pinned_station_posture(
            input.stations,
            StationRoutingSource::SessionPin(session_pin.to_string()),
            session_pin,
        );
        posture.retry_boundary = retry_boundary;
        posture.session_pin_count = input.session_pin_count;
        return posture;
    }

    if let Some(global_pin) = non_empty_trimmed(input.global_station_override) {
        let mut posture = build_pinned_station_posture(
            input.stations,
            StationRoutingSource::GlobalPin(global_pin.to_string()),
            global_pin,
        );
        posture.retry_boundary = retry_boundary;
        posture.session_pin_count = input.session_pin_count;
        return posture;
    }

    let mut posture = build_auto_station_posture(
        input.stations,
        non_empty_trimmed(input.configured_active_station),
    );
    posture.retry_boundary = retry_boundary;
    posture.session_pin_count = input.session_pin_count;
    posture
}

fn build_pinned_station_posture(
    stations: &[StationRoutingCandidate],
    source: StationRoutingSource,
    pinned_station: &str,
) -> StationRoutingPosture {
    let mut eligible_candidates = Vec::new();
    let mut skipped = Vec::new();

    match stations
        .iter()
        .find(|station| station.name == pinned_station)
    {
        Some(station) => {
            let reasons = pinned_skip_reasons(station);
            if reasons.is_empty() {
                eligible_candidates.push(station.clone());
            } else {
                skipped.push(StationRoutingSkipped {
                    station_name: station.name.clone(),
                    reasons,
                });
            }
        }
        None => skipped.push(StationRoutingSkipped {
            station_name: pinned_station.to_string(),
            reasons: vec![StationRoutingSkipReason::MissingPinnedTarget],
        }),
    }

    StationRoutingPosture {
        source,
        mode: StationRoutingMode::PinnedStation,
        eligible_candidates,
        skipped,
        retry_boundary: StationRetryBoundary::Unknown,
        session_pin_count: 0,
    }
}

fn build_auto_station_posture(
    stations: &[StationRoutingCandidate],
    configured_active_station: Option<&str>,
) -> StationRoutingPosture {
    let source = configured_active_station
        .map(|station| StationRoutingSource::ConfiguredActiveStation(station.to_string()))
        .unwrap_or(StationRoutingSource::Auto);

    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    for station in stations {
        let reasons = automatic_skip_reasons(station);
        if reasons.is_empty() {
            candidates.push(station.clone());
        } else {
            skipped.push(StationRoutingSkipped {
                station_name: station.name.clone(),
                reasons,
            });
        }
    }

    let mut levels = candidates
        .iter()
        .map(|station| station.level.clamp(1, 10))
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    let has_multi_level = levels.len() > 1;

    if has_multi_level {
        candidates.sort_by(|a, b| {
            a.level
                .clamp(1, 10)
                .cmp(&b.level.clamp(1, 10))
                .then_with(|| b.active.cmp(&a.active))
                .then_with(|| a.name.cmp(&b.name))
        });
    } else {
        candidates.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(pos) = candidates.iter().position(|station| station.active) {
            let station = candidates.remove(pos);
            candidates.insert(0, station);
        }
    }

    StationRoutingPosture {
        source,
        mode: if has_multi_level {
            StationRoutingMode::AutoLevelFallback
        } else {
            StationRoutingMode::AutoSingleLevelFallback
        },
        eligible_candidates: candidates,
        skipped,
        retry_boundary: StationRetryBoundary::Unknown,
        session_pin_count: 0,
    }
}

fn automatic_skip_reasons(station: &StationRoutingCandidate) -> Vec<StationRoutingSkipReason> {
    let mut reasons = Vec::new();
    if !station.enabled && !station.active {
        reasons.push(StationRoutingSkipReason::Disabled);
    }
    if station.runtime_state != RuntimeConfigState::Normal {
        reasons.push(StationRoutingSkipReason::RuntimeState(
            station.runtime_state,
        ));
    }
    if station.upstreams == Some(0) {
        reasons.push(StationRoutingSkipReason::NoRoutableUpstreams);
    }
    reasons
}

fn pinned_skip_reasons(station: &StationRoutingCandidate) -> Vec<StationRoutingSkipReason> {
    let mut reasons = Vec::new();
    if station.runtime_state == RuntimeConfigState::BreakerOpen {
        reasons.push(StationRoutingSkipReason::BreakerOpenBlocksPinned);
    }
    if station.upstreams == Some(0) {
        reasons.push(StationRoutingSkipReason::NoRoutableUpstreams);
    }
    reasons
}

fn retry_boundary_from_config(retry: Option<&ResolvedRetryConfig>) -> StationRetryBoundary {
    let Some(retry) = retry else {
        return StationRetryBoundary::Unknown;
    };

    let provider_failover =
        retry.route.strategy == RetryStrategy::Failover && retry.route.max_attempts > 1;
    if retry.allow_cross_station_before_first_output && provider_failover {
        return StationRetryBoundary::CrossStationBeforeFirstOutput {
            provider_max_attempts: retry.route.max_attempts,
        };
    }

    if retry.route.max_attempts > 1 {
        return StationRetryBoundary::CurrentStationFirst {
            provider_strategy: retry.route.strategy,
            provider_max_attempts: retry.route.max_attempts,
        };
    }

    StationRetryBoundary::NextRequestOnly
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(
        name: &str,
        enabled: bool,
        level: u8,
        active: bool,
        upstreams: usize,
    ) -> StationRoutingCandidate {
        StationRoutingCandidate {
            name: name.to_string(),
            alias: None,
            level,
            enabled,
            active,
            upstreams: Some(upstreams),
            runtime_state: RuntimeConfigState::Normal,
            has_cooldown: false,
            any_usage_exhausted: false,
            all_usage_exhausted: false,
            balance: StationRoutingBalanceSummary::default(),
        }
    }

    #[test]
    fn auto_posture_puts_single_level_active_first_and_skips_blocked() {
        let mut drain = station("drain", true, 1, false, 1);
        drain.runtime_state = RuntimeConfigState::Draining;
        let stations = vec![
            station("alpha", true, 1, false, 1),
            station("beta", true, 1, true, 1),
            station("disabled", false, 1, false, 1),
            drain,
        ];

        let posture = build_station_routing_posture(StationRoutingPostureInput {
            stations: &stations,
            session_station_override: None,
            global_station_override: None,
            configured_active_station: Some("beta"),
            session_pin_count: 0,
            retry: Some(&crate::config::RetryProfileName::Balanced.defaults()),
        });

        assert_eq!(posture.mode, StationRoutingMode::AutoSingleLevelFallback);
        assert_eq!(posture.eligible_candidates[0].name, "beta");
        assert_eq!(posture.eligible_candidates[1].name, "alpha");
        assert!(posture.skipped.iter().any(|item| {
            item.station_name == "disabled"
                && item.reasons == vec![StationRoutingSkipReason::Disabled]
        }));
        assert!(posture.skipped.iter().any(|item| {
            item.station_name == "drain"
                && item.reasons
                    == vec![StationRoutingSkipReason::RuntimeState(
                        RuntimeConfigState::Draining,
                    )]
        }));
    }

    #[test]
    fn auto_posture_sorts_multi_level_before_active_tiebreak() {
        let stations = vec![
            station("alpha", true, 2, false, 1),
            station("beta", true, 1, false, 1),
            station("zeta", true, 2, true, 1),
        ];

        let posture = build_station_routing_posture(StationRoutingPostureInput {
            stations: &stations,
            session_station_override: None,
            global_station_override: None,
            configured_active_station: Some("zeta"),
            session_pin_count: 0,
            retry: None,
        });

        assert_eq!(posture.mode, StationRoutingMode::AutoLevelFallback);
        assert_eq!(posture.eligible_candidates[0].name, "beta");
        assert_eq!(posture.eligible_candidates[1].name, "zeta");
        assert_eq!(posture.eligible_candidates[2].name, "alpha");
    }

    #[test]
    fn pinned_posture_allows_draining_but_blocks_breaker_open() {
        let mut draining = station("drain", false, 1, false, 1);
        draining.runtime_state = RuntimeConfigState::Draining;
        let stations = vec![draining];

        let posture = build_station_routing_posture(StationRoutingPostureInput {
            stations: &stations,
            session_station_override: None,
            global_station_override: Some("drain"),
            configured_active_station: None,
            session_pin_count: 0,
            retry: None,
        });

        assert_eq!(posture.mode, StationRoutingMode::PinnedStation);
        assert_eq!(posture.eligible_candidates[0].name, "drain");

        let mut breaker = station("breaker", true, 1, false, 1);
        breaker.runtime_state = RuntimeConfigState::BreakerOpen;
        let blocked = build_station_routing_posture(StationRoutingPostureInput {
            stations: &[breaker],
            session_station_override: None,
            global_station_override: Some("breaker"),
            configured_active_station: None,
            session_pin_count: 0,
            retry: None,
        });

        assert!(blocked.eligible_candidates.is_empty());
        assert_eq!(
            blocked.skipped[0].reasons,
            vec![StationRoutingSkipReason::BreakerOpenBlocksPinned]
        );
    }

    #[test]
    fn retry_boundary_explains_before_and_after_first_output() {
        let retry = crate::config::RetryProfileName::AggressiveFailover.defaults();
        let stations = vec![station("alpha", true, 1, true, 1)];

        let posture = build_station_routing_posture(StationRoutingPostureInput {
            stations: &stations,
            session_station_override: None,
            global_station_override: None,
            configured_active_station: Some("alpha"),
            session_pin_count: 2,
            retry: Some(&retry),
        });

        assert_eq!(
            posture.retry_boundary,
            StationRetryBoundary::CrossStationBeforeFirstOutput {
                provider_max_attempts: 3
            }
        );
        assert_eq!(posture.session_pin_count, 2);
    }

    #[test]
    fn station_option_adapter_preserves_lb_warning_facts() {
        let station = StationOption {
            name: "alpha".to_string(),
            alias: Some("Alpha".to_string()),
            enabled: true,
            level: 12,
            configured_enabled: true,
            configured_level: 12,
            runtime_enabled_override: None,
            runtime_level_override: None,
            runtime_state: RuntimeConfigState::Normal,
            runtime_state_override: None,
            capabilities: Default::default(),
        };
        let lb = LbConfigView {
            last_good_index: Some(0),
            upstreams: vec![crate::state::LbUpstreamView {
                failure_count: 0,
                cooldown_remaining_secs: Some(30),
                usage_exhausted: true,
            }],
        };

        let candidate =
            StationRoutingCandidate::from_station_option(&station, Some("alpha"), Some(&lb), None);

        assert_eq!(candidate.level, 10);
        assert!(candidate.active);
        assert_eq!(candidate.upstreams, Some(1));
        assert!(candidate.has_cooldown);
        assert!(candidate.any_usage_exhausted);
        assert!(candidate.all_usage_exhausted);
    }

    #[test]
    fn station_option_adapter_preserves_balance_warning_facts() {
        let station = StationOption {
            name: "alpha".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            configured_enabled: true,
            configured_level: 1,
            runtime_enabled_override: None,
            runtime_level_override: None,
            runtime_state: RuntimeConfigState::Normal,
            runtime_state_override: None,
            capabilities: Default::default(),
        };
        let balances = vec![
            ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Ok,
                ..ProviderBalanceSnapshot::default()
            },
            ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                ..ProviderBalanceSnapshot::default()
            },
            ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Stale,
                ..ProviderBalanceSnapshot::default()
            },
            ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Error,
                ..ProviderBalanceSnapshot::default()
            },
        ];

        let candidate = StationRoutingCandidate::from_station_option(
            &station,
            Some("alpha"),
            None,
            Some(&balances),
        );

        assert_eq!(candidate.balance.snapshots, 4);
        assert_eq!(candidate.balance.ok, 1);
        assert_eq!(candidate.balance.exhausted, 1);
        assert_eq!(candidate.balance.stale, 1);
        assert_eq!(candidate.balance.error, 1);
    }
}
