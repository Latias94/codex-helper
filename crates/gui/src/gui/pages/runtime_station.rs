use super::config_document::parse_proxy_config_document;
use super::*;
use crate::gui::proxy_control::ProxyController;

#[derive(Debug, Default)]
pub(super) struct RuntimeStationMaps {
    pub(super) station_health: HashMap<String, StationHealth>,
    pub(super) health_checks: HashMap<String, HealthCheckStatus>,
    pub(super) lb_view: HashMap<String, LbConfigView>,
}

pub(super) fn runtime_station_maps(proxy: &ProxyController) -> RuntimeStationMaps {
    match proxy.kind() {
        ProxyModeKind::Running => proxy
            .running()
            .map(|running| RuntimeStationMaps {
                station_health: running.station_health.clone(),
                health_checks: running.health_checks.clone(),
                lb_view: running.lb_view.clone(),
            })
            .unwrap_or_default(),
        ProxyModeKind::Attached => proxy
            .attached()
            .map(|attached| RuntimeStationMaps {
                station_health: attached.station_health.clone(),
                health_checks: attached.health_checks.clone(),
                lb_view: attached.lb_view.clone(),
            })
            .unwrap_or_default(),
        _ => RuntimeStationMaps::default(),
    }
}

pub(super) fn current_runtime_active_station(proxy: &ProxyController) -> Option<String> {
    let snapshot = proxy.snapshot()?;
    snapshot
        .effective_active_station
        .or(snapshot.configured_active_station)
}

pub(super) fn refresh_config_editor_from_disk_if_running(ctx: &mut PageCtx<'_>) {
    if !matches!(ctx.proxy.kind(), ProxyModeKind::Running) {
        return;
    }
    let new_path = crate::config::config_file_path();
    if let Ok(text) = std::fs::read_to_string(&new_path) {
        *ctx.proxy_config_text = text.clone();
        if let Ok(parsed) = parse_proxy_config_document(&text) {
            ctx.view.config.working = Some(parsed);
        }
    }
}

pub(super) fn format_runtime_station_health_status(
    health: Option<&StationHealth>,
    status: Option<&HealthCheckStatus>,
) -> String {
    if let Some(status) = status {
        if !status.done {
            return if status.cancel_requested {
                format!("cancel {}/{}", status.completed, status.total.max(1))
            } else {
                format!("run {}/{}", status.completed, status.total.max(1))
            };
        }
        if status.canceled {
            return "canceled".to_string();
        }
    }

    let Some(health) = health else {
        return "-".to_string();
    };
    if health.upstreams.is_empty() {
        return format!("0/0 @{}", health.checked_at_ms);
    }
    let ok = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .count();
    let best_ms = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .filter_map(|upstream| upstream.latency_ms)
        .min();
    if ok > 0 {
        if let Some(latency_ms) = best_ms {
            format!("{ok}/{} {latency_ms}ms", health.upstreams.len())
        } else {
            format!("{ok}/{} ok", health.upstreams.len())
        }
    } else {
        let code = health
            .upstreams
            .iter()
            .filter_map(|upstream| upstream.status_code)
            .next();
        match code {
            Some(code) => format!("err {code}"),
            None => "err".to_string(),
        }
    }
}

pub(super) fn format_runtime_lb_summary(lb: Option<&LbConfigView>) -> String {
    let Some(lb) = lb else {
        return "-".to_string();
    };
    let cooldowns = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.cooldown_remaining_secs.is_some())
        .count();
    let exhausted = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.usage_exhausted)
        .count();
    let failures: u32 = lb
        .upstreams
        .iter()
        .map(|upstream| upstream.failure_count)
        .sum();

    if cooldowns == 0 && exhausted == 0 && failures == 0 {
        return "-".to_string();
    }

    format!("cd={cooldowns} fail={failures} quota={exhausted}")
}

pub(super) fn runtime_config_state_label(
    lang: Language,
    state: RuntimeConfigState,
) -> &'static str {
    match (lang, state) {
        (Language::Zh, RuntimeConfigState::Normal) => "normal",
        (Language::Zh, RuntimeConfigState::Draining) => "draining",
        (Language::Zh, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (Language::Zh, RuntimeConfigState::HalfOpen) => "half_open",
        (_, RuntimeConfigState::Normal) => "normal",
        (_, RuntimeConfigState::Draining) => "draining",
        (_, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (_, RuntimeConfigState::HalfOpen) => "half_open",
    }
}

fn capability_support_short_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "是",
        (Language::Zh, CapabilitySupport::Unsupported) => "否",
        (Language::Zh, CapabilitySupport::Unknown) => "?",
        (_, CapabilitySupport::Supported) => "yes",
        (_, CapabilitySupport::Unsupported) => "no",
        (_, CapabilitySupport::Unknown) => "?",
    }
}

pub(super) fn capability_support_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "支持",
        (Language::Zh, CapabilitySupport::Unsupported) => "不支持",
        (Language::Zh, CapabilitySupport::Unknown) => "未知",
        (_, CapabilitySupport::Supported) => "supported",
        (_, CapabilitySupport::Unsupported) => "unsupported",
        (_, CapabilitySupport::Unknown) => "unknown",
    }
}

pub(super) fn format_runtime_config_capability_label(
    lang: Language,
    capabilities: &StationCapabilitySummary,
) -> String {
    let model_label = match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => {
            format!("{}:any", pick(lang, "模型", "models"))
        }
        ModelCatalogKind::Declared => {
            format!(
                "{}:{}",
                pick(lang, "模型", "models"),
                capabilities.supported_models.len()
            )
        }
    };
    format!(
        "{model_label} | tier:{} | effort:{}",
        capability_support_short_label(lang, capabilities.supports_service_tier),
        capability_support_short_label(lang, capabilities.supports_reasoning_effort),
    )
}

pub(super) fn runtime_config_capability_hover_text(
    lang: Language,
    capabilities: &StationCapabilitySummary,
) -> String {
    let mut lines = Vec::new();
    match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => lines.push(
            pick(
                lang,
                "模型能力: 未显式声明，当前按 implicit any 处理",
                "Model support: not declared explicitly; current routing treats this station as implicit-any",
            )
            .to_string(),
        ),
        ModelCatalogKind::Declared => {
            if capabilities.supported_models.is_empty() {
                lines.push(
                    pick(
                        lang,
                        "模型能力: 已声明，但没有正向可用模型模式",
                        "Model support: declared, but no positive model patterns are available",
                    )
                    .to_string(),
                );
            } else {
                lines.push(format!(
                    "{}: {}",
                    pick(lang, "模型列表", "Models"),
                    capabilities.supported_models.join(", ")
                ));
            }
        }
    }
    lines.push(format!(
        "{}: {}",
        pick(lang, "Fast/Service tier", "Fast/Service tier"),
        capability_support_label(lang, capabilities.supports_service_tier)
    ));
    lines.push(format!(
        "{}: {}",
        pick(lang, "思考强度", "Reasoning effort"),
        capability_support_label(lang, capabilities.supports_reasoning_effort)
    ));
    lines.push(
        pick(
            lang,
            "来源: supported_models/model_mapping 与 upstream tags",
            "Source: supported_models/model_mapping plus upstream tags",
        )
        .to_string(),
    );
    lines.join("\n")
}

pub(super) fn format_runtime_station_source(lang: Language, cfg: &StationOption) -> String {
    let mut parts = Vec::new();
    if let Some(enabled) = cfg.runtime_enabled_override {
        parts.push(format!(
            "{}={}",
            pick(lang, "启用", "enabled"),
            if enabled { "rt" } else { "rt-off" }
        ));
    }
    if cfg.runtime_level_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "等级", "level")));
    }
    if cfg.runtime_state_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "状态", "state")));
    }
    if parts.is_empty() {
        pick(lang, "站点配置", "station config").to_string()
    } else {
        parts.join(", ")
    }
}

pub(super) fn station_options_from_gui_stations(
    stations: &[StationOption],
) -> Vec<(String, String)> {
    let mut out = stations
        .iter()
        .map(|c| {
            let label = match c.alias.as_deref() {
                Some(a) if !a.trim().is_empty() => format!("{} ({a})", c.name),
                _ => c.name.clone(),
            };
            (c.name.clone(), label, c.level.clamp(1, 10))
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    out.into_iter().map(|(n, l, _)| (n, l)).collect()
}
