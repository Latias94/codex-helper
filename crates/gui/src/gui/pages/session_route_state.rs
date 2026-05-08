use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectiveRouteField {
    Model,
    Station,
    Upstream,
    Effort,
    ServiceTier,
}

impl EffectiveRouteField {
    pub(super) const ALL: [Self; 5] = [
        Self::Model,
        Self::Station,
        Self::Upstream,
        Self::Effort,
        Self::ServiceTier,
    ];
}

pub(super) fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn format_observed_client_identity(
    client_name: Option<&str>,
    client_addr: Option<&str>,
) -> Option<String> {
    match (
        non_empty_trimmed(client_name),
        non_empty_trimmed(client_addr),
    ) {
        (Some(name), Some(addr)) => Some(format!("{name} @ {addr}")),
        (Some(name), None) => Some(name),
        (None, Some(addr)) => Some(addr),
        (None, None) => None,
    }
}

pub(super) fn session_observation_scope_short_label(
    lang: Language,
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "obs", "obs"),
        SessionObservationScope::HostLocalEnriched => pick(lang, "host", "host"),
    }
}

pub(super) fn session_observation_scope_label(
    lang: Language,
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => pick(lang, "仅共享观测", "Observed only"),
        SessionObservationScope::HostLocalEnriched => {
            pick(lang, "代理主机 enrich", "Host-local enriched")
        }
    }
}

pub(super) fn session_transcript_host_status_label(lang: Language, row: &SessionRow) -> String {
    if row.host_local_transcript_path.is_some() {
        pick(
            lang,
            "已在 ~/.codex/sessions 链接",
            "linked under ~/.codex/sessions",
        )
        .to_string()
    } else {
        pick(
            lang,
            "未检测到 host-local transcript",
            "no host-local transcript detected",
        )
        .to_string()
    }
}

pub(super) fn session_transcript_access_message(
    lang: Language,
    row: &SessionRow,
    host_local_session_features: bool,
) -> String {
    match (
        row.host_local_transcript_path.is_some(),
        host_local_session_features,
    ) {
        (true, true) => pick(
            lang,
            "这台设备可直接打开这个 host-local transcript。",
            "This device can open the linked host-local transcript directly.",
        )
        .to_string(),
        (true, false) => pick(
            lang,
            "代理主机已链接到 transcript，但当前附着设备不能直接访问代理主机的文件系统。",
            "The proxy host has a linked transcript, but this attached device cannot access the proxy host filesystem directly.",
        )
        .to_string(),
        (false, true) => pick(
            lang,
            "这台设备具备 host-local 能力，但当前未在 ~/.codex/sessions 下找到匹配文件。",
            "This device has host-local access, but no matching file was found under ~/.codex/sessions yet.",
        )
        .to_string(),
        (false, false) => pick(
            lang,
            "当前是远端附着视角；可控制该 session_id，但不能假设本机可读取代理主机的 transcript。",
            "This is a remote-attached view; the session_id is controllable, but local transcript access on the proxy host cannot be assumed here.",
        )
        .to_string(),
    }
}

pub(super) fn resolve_effective_observed_value(
    override_value: Option<&str>,
    observed_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = non_empty_trimmed(override_value) {
        return Some(ResolvedRouteValue {
            value,
            source: RouteValueSource::SessionOverride,
        });
    }
    non_empty_trimmed(observed_value).map(|value| ResolvedRouteValue {
        value,
        source: RouteValueSource::RequestPayload,
    })
}

pub(super) fn apply_effective_route_to_row(
    row: &mut SessionRow,
    global_station_override: Option<&str>,
) {
    row.effective_model =
        resolve_effective_observed_value(row.override_model.as_deref(), row.last_model.as_deref());
    row.effective_reasoning_effort = resolve_effective_observed_value(
        row.override_effort.as_deref(),
        row.last_reasoning_effort.as_deref(),
    );
    row.effective_service_tier = resolve_effective_observed_value(
        row.override_service_tier.as_deref(),
        row.last_service_tier.as_deref(),
    );
    row.effective_station_value =
        if let Some(value) = non_empty_trimmed(row.override_station_name()) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::SessionOverride,
            })
        } else if let Some(value) = non_empty_trimmed(global_station_override) {
            Some(ResolvedRouteValue {
                value,
                source: RouteValueSource::GlobalOverride,
            })
        } else {
            non_empty_trimmed(row.last_station_name()).map(|value| ResolvedRouteValue {
                value,
                source: RouteValueSource::RuntimeFallback,
            })
        };
    row.effective_upstream_base_url = match (
        row.effective_station(),
        non_empty_trimmed(row.last_station_name()),
        non_empty_trimmed(row.last_upstream_base_url.as_deref()),
    ) {
        (Some(config), Some(last_config), Some(upstream)) if config.value == last_config => {
            Some(ResolvedRouteValue {
                value: upstream,
                source: RouteValueSource::RuntimeFallback,
            })
        }
        _ => None,
    };
}
