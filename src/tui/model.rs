use std::collections::HashMap;
use std::time::Instant;

use ratatui::prelude::{Color, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, LbConfigView, ProxyState,
    SessionStats, UsageRollupView,
};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct WindowStats {
    pub(in crate::tui) total: usize,
    pub(in crate::tui) ok_2xx: usize,
    pub(in crate::tui) err_429: usize,
    pub(in crate::tui) err_4xx: usize,
    pub(in crate::tui) err_5xx: usize,
    pub(in crate::tui) p50_ms: Option<u64>,
    pub(in crate::tui) p95_ms: Option<u64>,
    pub(in crate::tui) avg_attempts: Option<f64>,
    pub(in crate::tui) retry_rate: Option<f64>,
    pub(in crate::tui) top_provider: Option<(String, usize)>,
    pub(in crate::tui) top_config: Option<(String, usize)>,
}

pub(in crate::tui) fn compute_window_stats<F>(
    recent: &[FinishedRequest],
    now_ms: u64,
    window_ms: u64,
    mut include: F,
) -> WindowStats
where
    F: FnMut(&FinishedRequest) -> bool,
{
    fn percentile(mut v: Vec<u64>, p: f64) -> Option<u64> {
        if v.is_empty() {
            return None;
        }
        let n = v.len();
        let idx = ((p * (n.saturating_sub(1) as f64)).ceil() as usize).min(n - 1);
        let (_, nth, _) = v.select_nth_unstable(idx);
        Some(*nth)
    }

    let cutoff = now_ms.saturating_sub(window_ms);
    let mut out = WindowStats::default();
    let mut ok_lat = Vec::new();
    let mut attempts_sum: u64 = 0;
    let mut retry_cnt: u64 = 0;

    let mut by_provider: HashMap<String, usize> = HashMap::new();
    let mut by_config: HashMap<String, usize> = HashMap::new();

    for r in recent.iter() {
        if r.ended_at_ms < cutoff {
            continue;
        }
        if !include(r) {
            continue;
        }
        out.total += 1;

        let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
        attempts_sum = attempts_sum.saturating_add(attempts as u64);
        if attempts > 1 {
            retry_cnt = retry_cnt.saturating_add(1);
        }

        if r.status_code == 429 {
            out.err_429 += 1;
        } else if (400..500).contains(&r.status_code) {
            out.err_4xx += 1;
        } else if (500..600).contains(&r.status_code) {
            out.err_5xx += 1;
        }

        if (200..300).contains(&r.status_code) {
            out.ok_2xx += 1;
            ok_lat.push(r.duration_ms);

            if let Some(pid) = r.provider_id.as_deref()
                && !pid.trim().is_empty()
            {
                *by_provider.entry(pid.to_string()).or_insert(0) += 1;
            }
            if let Some(cfg) = r.config_name.as_deref()
                && !cfg.trim().is_empty()
            {
                *by_config.entry(cfg.to_string()).or_insert(0) += 1;
            }
        }
    }

    out.p50_ms = percentile(ok_lat.clone(), 0.50);
    out.p95_ms = percentile(ok_lat, 0.95);
    if out.total > 0 {
        out.avg_attempts = Some(attempts_sum as f64 / out.total as f64);
        out.retry_rate = Some(retry_cnt as f64 / out.total as f64);
    }

    out.top_provider = by_provider.into_iter().max_by_key(|(_, v)| *v);
    out.top_config = by_config.into_iter().max_by_key(|(_, v)| *v);
    out
}

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
    pub(in crate::tui) lb_view: HashMap<String, LbConfigView>,
    pub(in crate::tui) stats_5m: WindowStats,
    pub(in crate::tui) stats_1h: WindowStats,
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
            .configs
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
            .configs
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
    let now = now_ms();
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
        lb_view,
    ) = tokio::join!(
        state.list_active_requests(),
        state.list_recent_finished(2_000),
        state.list_session_effort_overrides(),
        state.list_session_config_overrides(),
        state.get_global_config_override(),
        state.list_session_stats(),
        state.get_config_meta_overrides(service_name),
        state.get_usage_rollup_view(service_name, 12, stats_days),
        state.get_config_health(service_name),
        state.list_health_checks(service_name),
        state.get_lb_view(),
    );

    let rows = build_session_rows(active, &recent, &overrides, &config_overrides, &stats);
    let stats_5m = compute_window_stats(&recent, now, 5 * 60_000, |_| true);
    let stats_1h = compute_window_stats(&recent, now, 60 * 60_000, |_| true);
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
        lb_view,
        stats_5m,
        stats_1h,
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

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
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
}
