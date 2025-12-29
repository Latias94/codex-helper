use std::collections::HashMap;
use std::time::Instant;

use ratatui::prelude::{Color, Style};

use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, ProxyState, SessionStats,
    UsageRollupView,
};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpstreamSummary {
    pub base_url: String,
    pub provider_id: Option<String>,
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
    pub(in crate::tui) last_provider_id: Option<String>,
    pub(in crate::tui) last_config_name: Option<String>,
    pub(in crate::tui) last_usage: Option<UsageMetrics>,
    pub(in crate::tui) total_usage: Option<UsageMetrics>,
    pub(in crate::tui) turns_total: Option<u64>,
    pub(in crate::tui) turns_with_usage: Option<u64>,
    pub(in crate::tui) override_effort: Option<String>,
    pub(in crate::tui) override_config_name: Option<String>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct Snapshot {
    pub(in crate::tui) rows: Vec<SessionRow>,
    pub(in crate::tui) recent: Vec<FinishedRequest>,
    pub(in crate::tui) overrides: HashMap<String, String>,
    pub(in crate::tui) config_overrides: HashMap<String, String>,
    pub(in crate::tui) global_override: Option<String>,
    pub(in crate::tui) config_meta_overrides: HashMap<String, (Option<bool>, Option<u8>)>,
    pub(in crate::tui) usage_rollup: UsageRollupView,
    pub(in crate::tui) config_health: HashMap<String, ConfigHealth>,
    pub(in crate::tui) health_checks: HashMap<String, HealthCheckStatus>,
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

pub(in crate::tui) fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path)
}

pub(in crate::tui) fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

pub(in crate::tui) fn short_sid(sid: &str, max: usize) -> String {
    if sid.chars().count() <= max {
        return sid.to_string();
    }
    let max = max.max(8);
    let head_len = ((max / 2).saturating_sub(1)).max(3);
    let tail_len = (max.saturating_sub(head_len + 1)).max(3);
    let head = sid.chars().take(head_len).collect::<String>();
    let tail = sid.chars().rev().take(tail_len).collect::<Vec<_>>();
    let tail = tail.into_iter().rev().collect::<String>();
    format!("{head}…{tail}")
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
    format!(
        "tok in/out/rsn/ttl: {}/{}/{}/{}",
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_tokens),
        tokens_short(usage.total_tokens)
    )
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

fn build_session_rows(
    active: Vec<ActiveRequest>,
    recent: &[FinishedRequest],
    overrides: &HashMap<String, String>,
    config_overrides: &HashMap<String, String>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionRow> {
    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionRow> = StdHashMap::new();

    for req in active {
        let key = req.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: req.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: Some(req.started_at_ms),
            active_last_method: Some(req.method.clone()),
            active_last_path: Some(req.path.clone()),
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: req.model.clone(),
            last_reasoning_effort: req.reasoning_effort.clone(),
            last_provider_id: req.provider_id.clone(),
            last_config_name: req.config_name.clone(),
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });

        entry.active_count += 1;
        entry.active_started_at_ms_min = Some(
            entry
                .active_started_at_ms_min
                .unwrap_or(req.started_at_ms)
                .min(req.started_at_ms),
        );
        entry.active_last_method = Some(req.method);
        entry.active_last_path = Some(req.path);
        if entry.cwd.is_none() {
            entry.cwd = req.cwd;
        }
        if let Some(effort) = req.reasoning_effort {
            entry.last_reasoning_effort = Some(effort);
        }
        if entry.last_model.is_none() {
            entry.last_model = req.model;
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = req.provider_id;
        }
        if entry.last_config_name.is_none() {
            entry.last_config_name = req.config_name;
        }
    }

    for r in recent {
        let key = r.session_id.clone();
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: r.cwd.clone(),
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: r.model.clone(),
            last_reasoning_effort: r.reasoning_effort.clone(),
            last_provider_id: r.provider_id.clone(),
            last_config_name: r.config_name.clone(),
            last_usage: r.usage.clone(),
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });

        let should_update = entry
            .last_ended_at_ms
            .map(|t| r.ended_at_ms >= t)
            .unwrap_or(true);
        if should_update {
            entry.last_status = Some(r.status_code);
            entry.last_duration_ms = Some(r.duration_ms);
            entry.last_ended_at_ms = Some(r.ended_at_ms);
            if r.reasoning_effort.is_some() {
                entry.last_reasoning_effort = r.reasoning_effort.clone();
            }
            if r.model.is_some() {
                entry.last_model = r.model.clone();
            }
            if r.provider_id.is_some() {
                entry.last_provider_id = r.provider_id.clone();
            }
            if r.config_name.is_some() {
                entry.last_config_name = r.config_name.clone();
            }
            if r.usage.is_some() {
                entry.last_usage = r.usage.clone();
            }
        }
        if entry.cwd.is_none() {
            entry.cwd = r.cwd.clone();
        }
    }

    for (sid, st) in stats.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: st.last_model.clone(),
            last_reasoning_effort: st.last_reasoning_effort.clone(),
            last_provider_id: st.last_provider_id.clone(),
            last_config_name: st.last_config_name.clone(),
            last_usage: st.last_usage.clone(),
            total_usage: Some(st.total_usage.clone()),
            turns_total: None,
            turns_with_usage: Some(st.turns_with_usage),
            override_effort: None,
            override_config_name: None,
        });
        entry.turns_total = Some(st.turns_total);
        if entry.last_model.is_none() {
            entry.last_model = st.last_model.clone();
        }
        if entry.last_reasoning_effort.is_none() {
            entry.last_reasoning_effort = st.last_reasoning_effort.clone();
        }
        if entry.last_provider_id.is_none() {
            entry.last_provider_id = st.last_provider_id.clone();
        }
        if entry.last_config_name.is_none() {
            entry.last_config_name = st.last_config_name.clone();
        }
        if entry.last_usage.is_none() {
            entry.last_usage = st.last_usage.clone();
        }
        if entry.total_usage.is_none() {
            entry.total_usage = Some(st.total_usage.clone());
        }
        if entry.turns_with_usage.is_none() {
            entry.turns_with_usage = Some(st.turns_with_usage);
        }
    }

    for (sid, eff) in overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
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
            last_provider_id: None,
            last_config_name: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });
        entry.override_effort = Some(eff.clone());
    }

    for (sid, cfg_name) in config_overrides.iter() {
        let key = Some(sid.clone());
        let entry = map.entry(key.clone()).or_insert_with(|| SessionRow {
            session_id: key,
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
            last_provider_id: None,
            last_config_name: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            override_effort: None,
            override_config_name: None,
        });
        entry.override_config_name = Some(cfg_name.clone());
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

pub(in crate::tui) async fn refresh_snapshot(
    state: &ProxyState,
    service_name: &str,
    stats_days: usize,
) -> Snapshot {
    let (
        active,
        recent,
        overrides,
        config_overrides,
        global_override,
        stats,
        config_meta,
        rollup,
        health,
        health_checks,
    ) = tokio::join!(
        state.list_active_requests(),
        state.list_recent_finished(200),
        state.list_session_effort_overrides(),
        state.list_session_config_overrides(),
        state.get_global_config_override(),
        state.list_session_stats(),
        state.get_config_meta_overrides(service_name),
        state.get_usage_rollup_view(service_name, 12, stats_days),
        state.get_config_health(service_name),
        state.list_health_checks(service_name),
    );

    let rows = build_session_rows(active, &recent, &overrides, &config_overrides, &stats);
    Snapshot {
        rows,
        recent,
        overrides,
        config_overrides,
        global_override,
        config_meta_overrides: config_meta,
        usage_rollup: rollup,
        config_health: health,
        health_checks,
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
