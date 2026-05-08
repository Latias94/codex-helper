use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use ratatui::prelude::{Color, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::ProxyConfig;
use crate::dashboard_core::WindowStats;
pub(in crate::tui) use crate::dashboard_core::window_stats::compute_window_stats;
use crate::pricing::ModelPriceCatalogSnapshot;
use crate::state::{
    FinishedRequest, HealthCheckStatus, LbConfigView, ProviderBalanceSnapshot, ProxyState,
    ResolvedRouteValue, SessionIdentityCard, SessionObservationScope, StationHealth,
    UsageRollupView,
};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpstreamSummary {
    pub base_url: String,
    pub provider_id: Option<String>,
    pub auth: String,
    pub tags: Vec<(String, String)>,
    pub supported_models: Vec<String>,
    pub model_mapping: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderOption {
    pub name: String,
    pub alias: Option<String>,
    pub enabled: bool,
    pub level: u8,
    pub active: bool,
    pub upstreams: Vec<UpstreamSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct SessionRow {
    pub(in crate::tui) session_id: Option<String>,
    pub(in crate::tui) observation_scope: SessionObservationScope,
    pub(in crate::tui) host_local_transcript_path: Option<String>,
    pub(in crate::tui) last_client_name: Option<String>,
    pub(in crate::tui) last_client_addr: Option<String>,
    pub(in crate::tui) cwd: Option<String>,
    pub(in crate::tui) active_count: usize,
    pub(in crate::tui) active_started_at_ms_min: Option<u64>,
    pub(in crate::tui) active_last_method: Option<String>,
    pub(in crate::tui) active_last_path: Option<String>,
    pub(in crate::tui) last_status: Option<u16>,
    pub(in crate::tui) last_duration_ms: Option<u64>,
    pub(in crate::tui) last_ended_at_ms: Option<u64>,
    pub(in crate::tui) last_model: Option<String>,
    pub(in crate::tui) last_reasoning_effort: Option<String>,
    pub(in crate::tui) last_service_tier: Option<String>,
    pub(in crate::tui) last_provider_id: Option<String>,
    pub(in crate::tui) last_station_name: Option<String>,
    pub(in crate::tui) last_upstream_base_url: Option<String>,
    pub(in crate::tui) last_usage: Option<UsageMetrics>,
    pub(in crate::tui) total_usage: Option<UsageMetrics>,
    pub(in crate::tui) turns_total: Option<u64>,
    pub(in crate::tui) turns_with_usage: Option<u64>,
    pub(in crate::tui) binding_profile_name: Option<String>,
    pub(in crate::tui) binding_continuity_mode: Option<crate::state::SessionContinuityMode>,
    pub(in crate::tui) effective_model: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_reasoning_effort: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_service_tier: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_station: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_upstream_base_url: Option<ResolvedRouteValue>,
    pub(in crate::tui) override_model: Option<String>,
    pub(in crate::tui) override_effort: Option<String>,
    pub(in crate::tui) override_station_name: Option<String>,
    pub(in crate::tui) override_service_tier: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct Snapshot {
    pub(in crate::tui) rows: Vec<SessionRow>,
    pub(in crate::tui) recent: Vec<FinishedRequest>,
    pub(in crate::tui) model_overrides: HashMap<String, String>,
    pub(in crate::tui) overrides: HashMap<String, String>,
    pub(in crate::tui) station_overrides: HashMap<String, String>,
    pub(in crate::tui) service_tier_overrides: HashMap<String, String>,
    pub(in crate::tui) global_station_override: Option<String>,
    pub(in crate::tui) station_meta_overrides: HashMap<String, (Option<bool>, Option<u8>)>,
    pub(in crate::tui) usage_rollup: UsageRollupView,
    pub(in crate::tui) provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub(in crate::tui) station_health: HashMap<String, StationHealth>,
    pub(in crate::tui) health_checks: HashMap<String, HealthCheckStatus>,
    pub(in crate::tui) lb_view: HashMap<String, LbConfigView>,
    pub(in crate::tui) stats_5m: WindowStats,
    pub(in crate::tui) stats_1h: WindowStats,
    pub(in crate::tui) pricing_catalog: ModelPriceCatalogSnapshot,
    pub(in crate::tui) refreshed_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::tui) struct Palette {
    pub(in crate::tui) bg: Color,
    pub(in crate::tui) panel: Color,
    pub(in crate::tui) border: Color,
    pub(in crate::tui) text: Color,
    pub(in crate::tui) muted: Color,
    pub(in crate::tui) accent: Color,
    pub(in crate::tui) focus: Color,
    pub(in crate::tui) good: Color,
    pub(in crate::tui) warn: Color,
    pub(in crate::tui) bad: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(14, 17, 22),
            panel: Color::Rgb(18, 22, 28),
            border: Color::Rgb(54, 62, 74),
            text: Color::Rgb(224, 228, 234),
            muted: Color::Rgb(144, 154, 164),
            accent: Color::Rgb(88, 166, 255),
            focus: Color::Rgb(121, 192, 255),
            good: Color::Rgb(63, 185, 80),
            warn: Color::Rgb(210, 153, 34),
            bad: Color::Rgb(248, 81, 73),
        }
    }
}

pub(in crate::tui) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(in crate::tui) const CODEX_RECENT_WINDOWS: [(u64, &str); 6] = [
    (30 * 60, "30m"),
    (60 * 60, "1h"),
    (3 * 60 * 60, "3h"),
    (8 * 60 * 60, "8h"),
    (12 * 60 * 60, "12h"),
    (24 * 60 * 60, "24h"),
];

pub(in crate::tui) fn codex_recent_window_label(idx: usize) -> &'static str {
    CODEX_RECENT_WINDOWS[idx.min(CODEX_RECENT_WINDOWS.len() - 1)].1
}

pub(in crate::tui) fn codex_recent_window_threshold_ms(now_ms: u64, idx: usize) -> u64 {
    let secs = CODEX_RECENT_WINDOWS[idx.min(CODEX_RECENT_WINDOWS.len() - 1)].0;
    now_ms.saturating_sub(secs.saturating_mul(1000))
}

pub(in crate::tui) fn basename(path: &str) -> &str {
    let path = path.trim_end_matches(['/', '\\']);
    let slash = path.rfind('/');
    let backslash = path.rfind('\\');
    let idx = match (slash, backslash) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    if let Some(i) = idx {
        &path[i.saturating_add(1)..]
    } else {
        path
    }
}

pub(in crate::tui) fn shorten(s: &str, max: usize) -> String {
    shorten_head(s, max)
}

pub(in crate::tui) fn shorten_middle(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let remaining = max.saturating_sub(1);
    let head_w = remaining / 2;
    let tail_w = remaining.saturating_sub(head_w);
    let head = prefix_by_width(s, head_w);
    let tail = suffix_by_width(s, tail_w);
    format!("{head}…{tail}")
}

fn shorten_head(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let head = prefix_by_width(s, max.saturating_sub(1));
    format!("{head}…")
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn prefix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(w) > max_width {
            break;
        }
        width = width.saturating_add(w);
        end = i.saturating_add(ch.len_utf8());
    }
    &s[..end]
}

fn suffix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut width = 0usize;
    let mut start = s.len();
    for (i, ch) in s.char_indices().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(w) > max_width {
            break;
        }
        width = width.saturating_add(w);
        start = i;
    }
    &s[start..]
}

pub(in crate::tui) fn short_sid(sid: &str, max: usize) -> String {
    // Prefer head truncation (end ellipsis) over middle truncation so the string stays readable
    // and copy/paste friendly in terminals.
    shorten_head(sid, max)
}

pub fn build_provider_options(
    cfg: &crate::config::ProxyConfig,
    service_name: &str,
) -> Vec<ProviderOption> {
    let upstream_summary = |u: &crate::config::UpstreamConfig| -> UpstreamSummary {
        let auth = if let Some(env) = u.auth.auth_token_env.as_deref()
            && !env.trim().is_empty()
        {
            format!("bearer env {env}")
        } else if u
            .auth
            .auth_token
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            "bearer inline".to_string()
        } else if let Some(env) = u.auth.api_key_env.as_deref()
            && !env.trim().is_empty()
        {
            format!("x-api-key env {env}")
        } else if u
            .auth
            .api_key
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            "x-api-key inline".to_string()
        } else {
            "-".to_string()
        };

        let mut tags = u
            .tags
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        tags.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut supported_models = u.supported_models.keys().cloned().collect::<Vec<_>>();
        supported_models.sort();

        let mut model_mapping = u
            .model_mapping
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        model_mapping.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        UpstreamSummary {
            base_url: u.base_url.clone(),
            provider_id: u.tags.get("provider_id").cloned(),
            auth,
            tags,
            supported_models,
            model_mapping,
        }
    };

    let mut providers: Vec<ProviderOption> = match service_name {
        "claude" => cfg
            .claude
            .stations()
            .iter()
            .map(|(name, svc)| ProviderOption {
                name: name.clone(),
                alias: svc.alias.clone(),
                enabled: svc.enabled,
                level: svc.level.clamp(1, 10),
                active: cfg.claude.active.as_deref() == Some(name.as_str()),
                upstreams: svc.upstreams.iter().map(upstream_summary).collect(),
            })
            .collect(),
        _ => cfg
            .codex
            .stations()
            .iter()
            .map(|(name, svc)| ProviderOption {
                name: name.clone(),
                alias: svc.alias.clone(),
                enabled: svc.enabled,
                level: svc.level.clamp(1, 10),
                active: cfg.codex.active.as_deref() == Some(name.as_str()),
                upstreams: svc.upstreams.iter().map(upstream_summary).collect(),
            })
            .collect(),
    };
    providers.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
    providers
}

fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

pub(in crate::tui) fn format_age(now_ms: u64, ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return "-".to_string();
    };
    if now_ms <= ts {
        return "0s".to_string();
    }
    let mut secs = (now_ms - ts) / 1000;
    let days = secs / 86400;
    secs %= 86400;
    let hours = secs / 3600;
    secs %= 3600;
    let mins = secs / 60;
    secs %= 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

pub(in crate::tui) fn tokens_short(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1_000_000.0 {
        format!("{:.1}m", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{:.0}", n)
    }
}

pub(in crate::tui) fn usage_line(usage: &UsageMetrics) -> String {
    let mut line = format!(
        "tok in/out/rsn/ttl: {}/{}/{}/{}",
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_output_tokens_total()),
        tokens_short(usage.total_tokens)
    );
    if usage.has_cache_tokens() {
        line.push_str(&format!(
            " cache cached/read/create: {}/{}/{}",
            tokens_short(usage.cached_input_tokens),
            tokens_short(usage.cache_read_input_tokens),
            tokens_short(usage.cache_creation_tokens_total())
        ));
    }
    line
}

pub(in crate::tui) fn status_style(p: Palette, status: Option<u16>) -> Style {
    match status {
        Some(s) if (200..300).contains(&s) => Style::default().fg(p.good),
        Some(s) if (300..400).contains(&s) => Style::default().fg(p.accent),
        Some(s) if (400..500).contains(&s) => Style::default().fg(p.warn),
        Some(_) => Style::default().fg(p.bad),
        None => Style::default().fg(p.muted),
    }
}

fn build_session_rows_from_cards(cards: &[SessionIdentityCard]) -> Vec<SessionRow> {
    let mut rows = cards
        .iter()
        .filter_map(|card| {
            let session_id = card.session_id.clone()?;
            Some(SessionRow {
                session_id: Some(session_id),
                observation_scope: card.observation_scope,
                host_local_transcript_path: card.host_local_transcript_path.clone(),
                last_client_name: card.last_client_name.clone(),
                last_client_addr: card.last_client_addr.clone(),
                cwd: card.cwd.clone(),
                active_count: card.active_count as usize,
                active_started_at_ms_min: card.active_started_at_ms_min,
                active_last_method: None,
                active_last_path: None,
                last_status: card.last_status,
                last_duration_ms: card.last_duration_ms,
                last_ended_at_ms: card.last_ended_at_ms,
                last_model: card.last_model.clone(),
                last_reasoning_effort: card.last_reasoning_effort.clone(),
                last_service_tier: card.last_service_tier.clone(),
                last_provider_id: card.last_provider_id.clone(),
                last_station_name: card.last_station_name.clone(),
                last_upstream_base_url: card.last_upstream_base_url.clone(),
                last_usage: card.last_usage.clone(),
                total_usage: card.total_usage.clone(),
                turns_total: card.turns_total,
                turns_with_usage: card.turns_with_usage,
                binding_profile_name: card.binding_profile_name.clone(),
                binding_continuity_mode: card.binding_continuity_mode,
                effective_model: card.effective_model.clone(),
                effective_reasoning_effort: card.effective_reasoning_effort.clone(),
                effective_service_tier: card.effective_service_tier.clone(),
                effective_station: card.effective_station.clone(),
                effective_upstream_base_url: card.effective_upstream_base_url.clone(),
                override_model: card.override_model.clone(),
                override_effort: card.override_effort.clone(),
                override_station_name: card.override_station_name.clone(),
                override_service_tier: card.override_service_tier.clone(),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

pub(in crate::tui) fn session_row_has_any_override(row: &SessionRow) -> bool {
    row.override_model.is_some()
        || row.override_effort.is_some()
        || row.override_station_name.is_some()
        || row.override_service_tier.is_some()
}

pub(in crate::tui) fn format_observed_client_identity(
    client_name: Option<&str>,
    client_addr: Option<&str>,
) -> Option<String> {
    match (
        client_name.map(str::trim).filter(|value| !value.is_empty()),
        client_addr.map(str::trim).filter(|value| !value.is_empty()),
    ) {
        (Some(name), Some(addr)) => Some(format!("{name} @ {addr}")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(addr)) => Some(addr.to_string()),
        (None, None) => None,
    }
}

pub(in crate::tui) fn session_observation_scope_label(
    scope: SessionObservationScope,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => "observed only",
        SessionObservationScope::HostLocalEnriched => "host-local enriched",
    }
}

pub(in crate::tui) fn session_transcript_host_status(row: &SessionRow) -> String {
    if row.host_local_transcript_path.is_some() {
        "linked under ~/.codex/sessions".to_string()
    } else {
        "no host-local transcript detected".to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct SessionControlPosture {
    pub(in crate::tui) headline: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) color: Color,
}

pub(in crate::tui) fn session_override_fields(row: &SessionRow) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if row.override_model.is_some() {
        fields.push("model");
    }
    if row.override_effort.is_some() {
        fields.push("effort");
    }
    if row.override_station_name.is_some() {
        fields.push("station");
    }
    if row.override_service_tier.is_some() {
        fields.push("service_tier");
    }
    fields
}

pub(in crate::tui) fn session_control_posture(
    row: &SessionRow,
    global_station: Option<&str>,
) -> SessionControlPosture {
    let override_fields = session_override_fields(row);
    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        if override_fields.is_empty() {
            let mode = row
                .binding_continuity_mode
                .map(|mode| format!("{mode:?}").to_ascii_lowercase())
                .unwrap_or_else(|| "default_profile".to_string());
            return SessionControlPosture {
                headline: format!("bound to profile {profile_name} ({mode})"),
                detail:
                    "This session keeps its stored binding until another profile or override rewrites it."
                        .to_string(),
                color: Color::Rgb(63, 185, 80),
            };
        }

        return SessionControlPosture {
            headline: format!("profile {profile_name} with session overrides"),
            detail: format!(
                "Session overrides on {} currently take priority over the bound profile.",
                override_fields.join(", ")
            ),
            color: Color::Rgb(88, 166, 255),
        };
    }

    if !override_fields.is_empty() {
        return SessionControlPosture {
            headline: "session-controlled route".to_string(),
            detail: format!(
                "This session is currently pinned by overrides on {}.",
                override_fields.join(", ")
            ),
            color: Color::Rgb(88, 166, 255),
        };
    }

    if let Some(station) = global_station.filter(|station| !station.trim().is_empty()) {
        return SessionControlPosture {
            headline: format!(
                "no binding; global station {station} may still influence fallback"
            ),
            detail:
                "Without a stored profile or session override, runtime/global routing explains the effective route."
                    .to_string(),
            color: Color::Rgb(210, 153, 34),
        };
    }

    SessionControlPosture {
        headline: "no stored binding or session override".to_string(),
        detail:
            "Effective route comes from request payloads, station defaults, and runtime fallback."
                .to_string(),
        color: Color::Rgb(144, 154, 164),
    }
}

pub(in crate::tui) async fn refresh_snapshot(
    state: &ProxyState,
    cfg: Arc<ProxyConfig>,
    service_name: &str,
    stats_days: usize,
) -> Snapshot {
    let (mut snap, config_meta) = tokio::join!(
        crate::dashboard_core::build_dashboard_snapshot(state, service_name, 2_000, stats_days),
        state.get_station_meta_overrides(service_name),
    );
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    crate::state::enrich_session_identity_cards_with_runtime(&mut snap.session_cards, mgr);

    let global_station_override = snap.effective_global_station_override().map(str::to_owned);
    let station_health = snap.effective_station_health().clone();
    let rows = build_session_rows_from_cards(&snap.session_cards);
    Snapshot {
        rows,
        recent: snap.recent,
        model_overrides: snap.session_model_overrides,
        overrides: snap.session_effort_overrides,
        station_overrides: snap.session_station_overrides,
        service_tier_overrides: snap.session_service_tier_overrides,
        global_station_override,
        station_meta_overrides: config_meta,
        usage_rollup: snap.usage_rollup,
        provider_balances: snap.provider_balances,
        station_health,
        health_checks: snap.health_checks,
        lb_view: snap.lb_view,
        stats_5m: snap.stats_5m,
        stats_1h: snap.stats_1h,
        pricing_catalog: crate::pricing::operator_model_price_catalog_snapshot(),
        refreshed_at: Instant::now(),
    }
}

pub(in crate::tui) fn filtered_requests_len(
    snapshot: &Snapshot,
    selected_session_idx: usize,
) -> usize {
    let selected_sid = snapshot
        .rows
        .get(selected_session_idx)
        .and_then(|r| r.session_id.as_deref());
    snapshot
        .recent
        .iter()
        .filter(|r| match (selected_sid, r.session_id.as_deref()) {
            (Some(sid), Some(rid)) => sid == rid,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(60)
        .count()
}

pub(in crate::tui) fn find_session_idx(snapshot: &Snapshot, sid: &str) -> Option<usize> {
    snapshot
        .rows
        .iter()
        .position(|row| row.session_id.as_deref() == Some(sid))
}

pub(in crate::tui) fn request_page_focus_session_id(
    snapshot: &Snapshot,
    explicit_focus: Option<&str>,
    selected_session_idx: usize,
) -> Option<String> {
    explicit_focus.map(ToOwned::to_owned).or_else(|| {
        snapshot
            .rows
            .get(selected_session_idx)
            .and_then(|row| row.session_id.clone())
    })
}

pub(in crate::tui) fn request_matches_page_filters(
    request: &FinishedRequest,
    errors_only: bool,
    scope_session: bool,
    focused_sid: Option<&str>,
) -> bool {
    if errors_only && request.status_code < 400 {
        return false;
    }
    if !scope_session {
        return true;
    }

    match (focused_sid, request.session_id.as_deref()) {
        (Some(sid), Some(request_sid)) => sid == request_sid,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

pub(in crate::tui) fn filtered_request_page_len(
    snapshot: &Snapshot,
    explicit_focus: Option<&str>,
    selected_session_idx: usize,
    errors_only: bool,
    scope_session: bool,
) -> usize {
    let focused_sid = request_page_focus_session_id(snapshot, explicit_focus, selected_session_idx);
    snapshot
        .recent
        .iter()
        .filter(|request| {
            request_matches_page_filters(
                request,
                errors_only,
                scope_session,
                focused_sid.as_deref(),
            )
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::FinishedRequest;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn basename_handles_unix_and_windows_paths() {
        assert_eq!(basename("/a/b/c"), "c");
        assert_eq!(basename("/a/b/c/"), "c");
        assert_eq!(basename(r"C:\a\b\c"), "c");
        assert_eq!(basename(r"C:\a\b\c\"), "c");
    }

    #[test]
    fn shorten_respects_display_width_cjk() {
        let s = "你好世界";
        let out = shorten(s, 5);
        assert_eq!(out, "你好…");
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 5);
    }

    #[test]
    fn shorten_middle_keeps_both_ends() {
        let s = "abcdef";
        assert_eq!(shorten_middle(s, 5), "ab…ef");
    }

    #[test]
    fn request_page_focus_session_prefers_explicit_focus() {
        let snapshot = Snapshot {
            rows: vec![SessionRow {
                session_id: Some("sid-selected".to_string()),
                observation_scope: SessionObservationScope::ObservedOnly,
                host_local_transcript_path: None,
                last_client_name: None,
                last_client_addr: None,
                cwd: None,
                active_count: 0,
                active_started_at_ms_min: None,
                active_last_method: None,
                active_last_path: None,
                last_status: None,
                last_duration_ms: None,
                last_ended_at_ms: None,
                last_model: None,
                last_reasoning_effort: None,
                last_service_tier: None,
                last_provider_id: None,
                last_station_name: None,
                last_upstream_base_url: None,
                last_usage: None,
                total_usage: None,
                turns_total: None,
                turns_with_usage: None,
                binding_profile_name: None,
                binding_continuity_mode: None,
                effective_model: None,
                effective_reasoning_effort: None,
                effective_service_tier: None,
                effective_station: None,
                effective_upstream_base_url: None,
                override_model: None,
                override_effort: None,
                override_station_name: None,
                override_service_tier: None,
            }],
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            station_meta_overrides: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: Instant::now(),
        };

        let focused = request_page_focus_session_id(&snapshot, Some("sid-explicit"), 0);

        assert_eq!(focused.as_deref(), Some("sid-explicit"));
    }

    #[test]
    fn filtered_request_page_len_uses_explicit_focus() {
        let snapshot = Snapshot {
            rows: vec![SessionRow {
                session_id: Some("sid-selected".to_string()),
                observation_scope: SessionObservationScope::ObservedOnly,
                host_local_transcript_path: None,
                last_client_name: None,
                last_client_addr: None,
                cwd: None,
                active_count: 0,
                active_started_at_ms_min: None,
                active_last_method: None,
                active_last_path: None,
                last_status: None,
                last_duration_ms: None,
                last_ended_at_ms: None,
                last_model: None,
                last_reasoning_effort: None,
                last_service_tier: None,
                last_provider_id: None,
                last_station_name: None,
                last_upstream_base_url: None,
                last_usage: None,
                total_usage: None,
                turns_total: None,
                turns_with_usage: None,
                binding_profile_name: None,
                binding_continuity_mode: None,
                effective_model: None,
                effective_reasoning_effort: None,
                effective_service_tier: None,
                effective_station: None,
                effective_upstream_base_url: None,
                override_model: None,
                override_effort: None,
                override_station_name: None,
                override_service_tier: None,
            }],
            recent: vec![
                FinishedRequest {
                    id: 1,
                    trace_id: Some("codex-1".to_string()),
                    session_id: Some("sid-selected".to_string()),
                    client_name: None,
                    client_addr: None,
                    cwd: None,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    station_name: None,
                    provider_id: None,
                    upstream_base_url: None,
                    route_decision: None,
                    usage: None,
                    cost: crate::pricing::CostBreakdown::default(),
                    retry: None,
                    observability: crate::state::RequestObservability::default(),
                    service: "codex".to_string(),
                    method: "POST".to_string(),
                    path: "/v1/responses".to_string(),
                    status_code: 200,
                    duration_ms: 120,
                    ttfb_ms: None,
                    streaming: false,
                    ended_at_ms: 1,
                },
                FinishedRequest {
                    id: 2,
                    trace_id: Some("codex-2".to_string()),
                    session_id: Some("sid-explicit".to_string()),
                    client_name: None,
                    client_addr: None,
                    cwd: None,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    station_name: None,
                    provider_id: None,
                    upstream_base_url: None,
                    route_decision: None,
                    usage: None,
                    cost: crate::pricing::CostBreakdown::default(),
                    retry: None,
                    observability: crate::state::RequestObservability::default(),
                    service: "codex".to_string(),
                    method: "POST".to_string(),
                    path: "/v1/responses".to_string(),
                    status_code: 200,
                    duration_ms: 120,
                    ttfb_ms: None,
                    streaming: false,
                    ended_at_ms: 2,
                },
            ],
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            station_meta_overrides: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: Instant::now(),
        };

        let count = filtered_request_page_len(&snapshot, Some("sid-explicit"), 0, false, true);

        assert_eq!(count, 1);
    }

    #[test]
    fn build_session_rows_from_cards_skips_sessionless_cards() {
        let rows = build_session_rows_from_cards(&[
            SessionIdentityCard {
                session_id: None,
                ..SessionIdentityCard::default()
            },
            SessionIdentityCard {
                session_id: Some("sid-1".to_string()),
                ..SessionIdentityCard::default()
            },
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sid-1"));
    }

    #[test]
    fn session_control_posture_reports_profile_overrides() {
        let row = SessionRow {
            session_id: Some("sid-1".to_string()),
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_station_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: Some("fast".to_string()),
            binding_continuity_mode: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station: None,
            effective_upstream_base_url: None,
            override_model: Some("gpt-5.4".to_string()),
            override_effort: None,
            override_station_name: None,
            override_service_tier: None,
        };

        let posture = session_control_posture(&row, None);

        assert!(posture.headline.contains("profile fast"));
        assert!(posture.detail.contains("model"));
    }
}
