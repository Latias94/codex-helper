use std::collections::HashMap;

use super::model::tokens_short;
use super::state::UiState;
use super::types::StatsFocus;
use crate::state::UsageBucket;
use crate::usage::UsageMetrics;

#[derive(Debug, Clone)]
pub(in crate::tui) enum StatsTarget {
    Station(String),
    Provider(String),
}

fn sum_buckets(buckets: &[(i32, UsageBucket)]) -> UsageBucket {
    let mut out = UsageBucket::default();
    for (_, b) in buckets {
        out.add_assign(b);
    }
    out
}

#[derive(Debug, Clone, Default)]
struct RecentBreakdown {
    total: u64,
    err: u64,
    class_2xx: u64,
    class_3xx: u64,
    class_4xx: u64,
    class_5xx: u64,
    top_status: Vec<(u16, u64)>,
    top_models_by_tokens: Vec<(String, (u64, i64))>,
    top_paths_by_tokens: Vec<(String, (u64, u64, i64))>,
}

fn compute_recent_breakdown(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
    target: &StatsTarget,
) -> RecentBreakdown {
    let mut by_model: HashMap<String, (u64, i64)> = HashMap::new();
    let mut by_path: HashMap<String, (u64, u64, i64)> = HashMap::new();
    let mut by_status: HashMap<u16, u64> = HashMap::new();
    let mut out = RecentBreakdown::default();

    for r in &snapshot.recent {
        let matches = match target {
            StatsTarget::Station(name) => r.station_name.as_deref() == Some(name.as_str()),
            StatsTarget::Provider(name) => r.provider_id.as_deref() == Some(name.as_str()),
        };
        if !matches {
            continue;
        }
        if ui.stats_errors_only && r.status_code < 400 {
            continue;
        }

        out.total += 1;
        if r.status_code >= 400 {
            out.err += 1;
        }
        match r.status_code {
            200..=299 => out.class_2xx += 1,
            300..=399 => out.class_3xx += 1,
            400..=499 => out.class_4xx += 1,
            _ => out.class_5xx += 1,
        }
        *by_status.entry(r.status_code).or_insert(0) += 1;

        let model = r.model.as_deref().unwrap_or("-");
        let tokens = r.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
        by_model
            .entry(model.to_string())
            .and_modify(|(c, t)| {
                *c = c.saturating_add(1);
                *t = t.saturating_add(tokens);
            })
            .or_insert((1, tokens));

        by_path
            .entry(r.path.clone())
            .and_modify(|(c, e, t)| {
                *c = c.saturating_add(1);
                if r.status_code >= 400 {
                    *e = e.saturating_add(1);
                }
                *t = t.saturating_add(tokens);
            })
            .or_insert((1, if r.status_code >= 400 { 1 } else { 0 }, tokens));
    }

    let mut status_items = by_status.into_iter().collect::<Vec<_>>();
    status_items.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    status_items.truncate(10);
    out.top_status = status_items;

    let mut model_items = by_model.into_iter().collect::<Vec<_>>();
    model_items.sort_by_key(|(_, (_, tok))| std::cmp::Reverse(*tok));
    model_items.truncate(10);
    out.top_models_by_tokens = model_items;

    let mut path_items = by_path.into_iter().collect::<Vec<_>>();
    path_items.sort_by_key(|(_, (_, _, tok))| std::cmp::Reverse(*tok));
    path_items.truncate(10);
    out.top_paths_by_tokens = path_items;

    out
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    format!("{}ms", total_ms / n)
}

fn fmt_usage_line(u: &UsageMetrics) -> String {
    let mut line = format!(
        "tokens in/out/rsn/ttl: {}/{}/{}/{}",
        tokens_short(u.input_tokens),
        tokens_short(u.output_tokens),
        tokens_short(u.reasoning_output_tokens_total()),
        tokens_short(u.total_tokens)
    );
    if u.has_cache_tokens() {
        line.push_str(&format!(
            " cache cached/read/create: {}/{}/{}",
            tokens_short(u.cached_input_tokens),
            tokens_short(u.cache_read_input_tokens),
            tokens_short(u.cache_creation_tokens_total())
        ));
    }
    line
}

pub(in crate::tui) fn selected_stats_target(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
) -> Option<StatsTarget> {
    match ui.stats_focus {
        StatsFocus::Stations => snapshot
            .usage_rollup
            .by_config
            .get(ui.selected_stats_station_idx)
            .map(|(k, _)| StatsTarget::Station(k.clone())),
        StatsFocus::Providers => snapshot
            .usage_rollup
            .by_provider
            .get(ui.selected_stats_provider_idx)
            .map(|(k, _)| StatsTarget::Provider(k.clone())),
    }
}

pub(in crate::tui) fn build_stats_report(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
    now_ms: u64,
) -> Option<String> {
    let target = selected_stats_target(ui, snapshot)?;

    let window_series = match &target {
        StatsTarget::Station(name) => snapshot
            .usage_rollup
            .by_config_day
            .get(name)
            .cloned()
            .unwrap_or_default(),
        StatsTarget::Provider(name) => snapshot
            .usage_rollup
            .by_provider_day
            .get(name)
            .cloned()
            .unwrap_or_default(),
    };

    let window_bucket = sum_buckets(&window_series);
    let recent = compute_recent_breakdown(ui, snapshot, &target);

    let (kind, name) = match &target {
        StatsTarget::Station(n) => ("station", n.as_str()),
        StatsTarget::Provider(n) => ("provider", n.as_str()),
    };

    let mut out = String::new();
    out.push_str("codex-helper TUI Stats report\n");
    out.push_str(&format!("generated_at_ms: {now_ms}\n"));
    out.push_str(&format!("service: {}\n", ui.service_name));
    out.push_str(&format!("target: {kind} {name}\n"));
    let window_label = match ui.stats_days {
        0 => "loaded".to_string(),
        1 => "today".to_string(),
        n => format!("{n}d"),
    };
    out.push_str(&format!("window: {window_label}\n"));
    out.push_str(&format!(
        "loaded_requests: {}  loaded_days_with_data: {}\n",
        snapshot.usage_rollup.coverage.loaded_requests,
        snapshot.usage_rollup.coverage.loaded_days_with_data
    ));
    if snapshot.usage_rollup.coverage.window_exceeds_loaded_start {
        out.push_str("coverage_warning: selected window starts before loaded log data\n");
    }
    out.push_str(&format!(
        "recent_filter: errors_only={}\n",
        ui.stats_errors_only
    ));
    out.push('\n');

    out.push_str("[window rollup]\n");
    out.push_str(&format!(
        "requests: {} (errors {} / {})  avg {}\n",
        window_bucket.requests_total,
        window_bucket.requests_error,
        fmt_pct(window_bucket.requests_error, window_bucket.requests_total),
        fmt_avg_ms(
            window_bucket.duration_ms_total,
            window_bucket.requests_total
        ),
    ));
    out.push_str(&format!("{}\n", fmt_usage_line(&window_bucket.usage)));
    out.push('\n');

    out.push_str("[recent sample]\n");
    out.push_str(&format!(
        "requests: {}  errors: {}  2xx/3xx/4xx/5xx: {}/{}/{}/{}\n",
        recent.total,
        recent.err,
        recent.class_2xx,
        recent.class_3xx,
        recent.class_4xx,
        recent.class_5xx
    ));
    if !recent.top_status.is_empty() {
        out.push_str("top_status:\n");
        for (s, c) in &recent.top_status {
            out.push_str(&format!("  - {s}: {c}\n"));
        }
    }
    if !recent.top_models_by_tokens.is_empty() {
        out.push_str("top_models_by_tokens:\n");
        for (m, (c, tok)) in &recent.top_models_by_tokens {
            out.push_str(&format!("  - {}: {} req / {}\n", m, c, tokens_short(*tok)));
        }
    }
    if !recent.top_paths_by_tokens.is_empty() {
        out.push_str("top_paths_by_tokens:\n");
        for (path, (c, e, tok)) in &recent.top_paths_by_tokens {
            out.push_str(&format!(
                "  - {}: {} req (err {}) / {}\n",
                path,
                c,
                e,
                tokens_short(*tok)
            ));
        }
    }

    Some(out)
}
