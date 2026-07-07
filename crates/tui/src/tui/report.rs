use super::Language;
use super::i18n;
use super::model::tokens_short;
use super::state::UiState;
use super::types::StatsFocus;
use crate::state::{UsageBucket, UsageDayDimensionRow, UsageDayView};

#[derive(Debug, Clone)]
pub(in crate::tui) enum StatsTarget {
    Station(String, UsageBucket),
    Provider(String, UsageBucket),
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    total_ms
        .checked_div(n)
        .map(|avg| format!("{avg}ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn selected_stats_target_from_view(ui: &UiState, usage: &UsageDayView) -> Option<StatsTarget> {
    match ui.stats_focus {
        StatsFocus::Stations => usage
            .station_rows
            .get(ui.selected_stats_station_idx)
            .map(|row| StatsTarget::Station(row.name.clone(), row.bucket.clone())),
        StatsFocus::Providers => usage
            .provider_rows
            .get(ui.selected_stats_provider_idx)
            .map(|row| StatsTarget::Provider(row.name.clone(), row.bucket.clone())),
    }
}

pub(in crate::tui) fn build_stats_report(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
    now_ms: u64,
) -> Option<String> {
    let usage = &snapshot.usage_day;
    let target = selected_stats_target_from_view(ui, usage)?;
    let (kind, name, target_bucket) = match &target {
        StatsTarget::Station(name, bucket) => (i18n::label(ui.language, "station"), name, bucket),
        StatsTarget::Provider(name, bucket) => (i18n::label(ui.language, "provider"), name, bucket),
    };
    let l = |text| i18n::label(ui.language, text);

    let mut out = String::new();
    out.push_str(match ui.language {
        Language::Zh => "codex-helper TUI 今日用量报告\n",
        Language::En => "codex-helper TUI Daily Usage report\n",
    });
    out.push_str(&format!("generated_at_ms: {now_ms}\n"));
    out.push_str(&format!("service: {}\n", ui.service_name));
    out.push_str(&format!("day: {} ({})\n", usage.label, usage.day));
    out.push_str(&format!("{}: {kind} {name}\n", l("target")));
    out.push('\n');

    out.push_str(match ui.language {
        Language::Zh => "[今日汇总]\n",
        Language::En => "[day summary]\n",
    });
    append_bucket(&mut out, ui.language, &usage.summary, "total");
    append_bucket(&mut out, ui.language, target_bucket, name);
    out.push('\n');

    out.push_str(match ui.language {
        Language::Zh => "[覆盖范围]\n",
        Language::En => "[coverage]\n",
    });
    out.push_str(&format!(
        "source={} loaded_requests={} first_ms={} last_ms={}\n",
        usage.coverage.source,
        usage.coverage.loaded_requests,
        usage
            .coverage
            .loaded_first_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        usage
            .coverage
            .loaded_last_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    out.push_str(&format!(
        "partial={} reason={}\n",
        usage.coverage.day_may_be_partial,
        usage.coverage.partial_reason.as_deref().unwrap_or("-")
    ));
    if usage.coverage.scanned_lines > 0 {
        out.push_str(&format!(
            "scan_lines={} max_lines={} max_bytes={} bytes_truncated={} lines_truncated={}\n",
            usage.coverage.scanned_lines,
            usage.coverage.max_lines,
            usage.coverage.max_bytes,
            usage.coverage.bytes_truncated,
            usage.coverage.lines_truncated
        ));
    }
    out.push('\n');

    out.push_str(match ui.language {
        Language::Zh => "[Retry Gate]\n",
        Language::En => "[retry gate]\n",
    });
    out.push_str(&format!(
        "active={} cooldown={} max_remaining_secs={}\n",
        usage.retry_gate.active,
        usage.retry_gate.active_cooldowns,
        usage
            .retry_gate
            .max_remaining_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    for row in &usage.retry_gate.reasons {
        out.push_str(&format!("  - {}: {}\n", row.reason, row.active));
    }
    out.push('\n');

    append_dimension(&mut out, ui.language, "providers", &usage.provider_rows);
    append_dimension(&mut out, ui.language, "stations", &usage.station_rows);
    append_dimension(&mut out, ui.language, "models", &usage.model_rows);
    append_dimension(&mut out, ui.language, "sessions", &usage.session_rows);
    append_dimension(&mut out, ui.language, "projects", &usage.project_rows);

    Some(out)
}

fn append_bucket(out: &mut String, lang: Language, bucket: &UsageBucket, label: &str) {
    let l = |text| i18n::label(lang, text);
    out.push_str(&format!(
        "{label}: {}={} {}={} {}={} {}={} {}={} {}\n",
        l("requests"),
        bucket.requests_total,
        l("errors"),
        bucket.requests_error,
        l("error rate"),
        fmt_pct(bucket.requests_error, bucket.requests_total),
        l("avg"),
        fmt_avg_ms(bucket.duration_ms_total, bucket.requests_total),
        l("tokens"),
        tokens_short(bucket.usage.total_tokens),
        cost_text(bucket)
    ));
}

fn cost_text(bucket: &UsageBucket) -> String {
    format!(
        "cost={} priced={} unpriced={}",
        bucket.cost.display_total(),
        bucket.cost.priced_requests,
        bucket.cost.unpriced_requests
    )
}

fn append_dimension(
    out: &mut String,
    lang: Language,
    title: &'static str,
    rows: &[UsageDayDimensionRow],
) {
    let display_title = match (lang, title) {
        (Language::Zh, "providers") => "providers",
        (Language::Zh, "stations") => "stations",
        (Language::Zh, "models") => "models",
        (Language::Zh, "sessions") => "sessions",
        (Language::Zh, "projects") => "projects",
        _ => title,
    };
    out.push_str(&format!("[{display_title}]\n"));
    if rows.is_empty() {
        out.push_str("  -\n");
        return;
    }
    for row in rows.iter().take(30) {
        out.push_str(&format!(
            "  - {}: req={} err={} tok={} {}\n",
            row.name,
            row.bucket.requests_total,
            row.bucket.requests_error,
            tokens_short(row.bucket.usage.total_tokens),
            cost_text(&row.bucket)
        ));
    }
}
