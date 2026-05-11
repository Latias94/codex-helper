use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::config::{ResolvedRetryConfig, ResolvedRetryLayerConfig, RetryStrategy};
use crate::healthcheck::{
    HEALTHCHECK_MAX_INFLIGHT_ENV, HEALTHCHECK_TIMEOUT_MS_ENV, HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV,
};
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_amount_brief_lang, balance_snapshot_status_label_lang, now_ms,
    shorten, shorten_middle,
};
use crate::tui::state::UiState;

fn retry_strategy_name(strategy: RetryStrategy) -> &'static str {
    match strategy {
        RetryStrategy::Failover => "failover",
        RetryStrategy::SameUpstream => "same_upstream",
    }
}

fn retry_trigger_summary(layer: &ResolvedRetryLayerConfig) -> String {
    let statuses = if layer.on_status.trim().is_empty() {
        "-".to_string()
    } else {
        layer.on_status.clone()
    };
    let classes = if layer.on_class.is_empty() {
        "-".to_string()
    } else {
        layer.on_class.join(",")
    };
    format!("status=[{statuses}] class=[{classes}]")
}

fn retry_layer_preview_lang(
    label: &str,
    layer: &ResolvedRetryLayerConfig,
    lang: Language,
) -> String {
    format!(
        "{label}: {}={} {}={} backoff={}..{}ms jitter={}ms retry_on={}",
        i18n::label(lang, "strategy"),
        retry_strategy_name(layer.strategy),
        i18n::label(lang, "attempts"),
        layer.max_attempts,
        layer.backoff_ms,
        layer.backoff_max_ms,
        layer.jitter_ms,
        retry_trigger_summary(layer)
    )
}

#[cfg(test)]
fn retry_policy_preview_lines(retry: &ResolvedRetryConfig) -> Vec<String> {
    retry_policy_preview_lines_lang(retry, Language::En)
}

fn retry_policy_preview_lines_lang(retry: &ResolvedRetryConfig, lang: Language) -> Vec<String> {
    let mut lines = vec![
        retry_layer_preview_lang("upstream", &retry.upstream, lang),
        retry_layer_preview_lang("route", &retry.route, lang),
    ];
    let boundary = match (lang, retry.allow_cross_station_before_first_output) {
        (Language::Zh, true) => "边界：首个输出前允许跨站点 failover；输出后保持已提交路由",
        (Language::Zh, false) => "边界：首个输出前禁止跨站点 failover；仅执行同站点/上游策略",
        (Language::En, true) => {
            "boundary: cross-station failover allowed before first output; after output stays on committed route"
        }
        (Language::En, false) => {
            "boundary: cross-station failover blocked before first output; same-station/upstream policy only"
        }
    };
    lines.push(boundary.to_string());
    let never_class = if retry.never_on_class.is_empty() {
        "-".to_string()
    } else {
        retry.never_on_class.join(",")
    };
    lines.push(match lang {
        Language::Zh => format!(
            "护栏：never_status=[{}] never_class=[{}]",
            retry.never_on_status, never_class
        ),
        Language::En => format!(
            "guardrails: never_status=[{}] never_class=[{}]",
            retry.never_on_status, never_class
        ),
    });
    lines.push(match lang {
        Language::Zh => format!(
            "冷却：transport={}s cf_challenge={}s cf_timeout={}s backoff_factor={} max={}s",
            retry.transport_cooldown_secs,
            retry.cloudflare_challenge_cooldown_secs,
            retry.cloudflare_timeout_cooldown_secs,
            retry.cooldown_backoff_factor,
            retry.cooldown_backoff_max_secs
        ),
        Language::En => format!(
            "cooldown: transport={}s cf_challenge={}s cf_timeout={}s backoff_factor={} max={}s",
            retry.transport_cooldown_secs,
            retry.cloudflare_challenge_cooldown_secs,
            retry.cloudflare_timeout_cooldown_secs,
            retry.cooldown_backoff_factor,
            retry.cooldown_backoff_max_secs
        ),
    });
    lines
}

fn pricing_catalog_preview_lines_lang(
    snapshot: &Snapshot,
    limit: usize,
    lang: Language,
) -> Vec<String> {
    let catalog = &snapshot.pricing_catalog;
    let mut lines = vec![format!(
        "source={}  models={}",
        catalog.source, catalog.model_count
    )];
    if catalog.models.is_empty() {
        lines.push(i18n::label(lang, "no price rows").to_string());
        return lines;
    }

    for row in prioritized_price_rows(snapshot, limit) {
        lines.push(format!(
            "{}  in={} out={} cr={} cc={}  {}/{}",
            shorten_middle(&price_model_label(row), 24),
            format_price(&row.input_per_1m_usd),
            format_price(&row.output_per_1m_usd),
            format_optional_price(row.cache_read_input_per_1m_usd.as_deref()),
            format_optional_price(row.cache_creation_input_per_1m_usd.as_deref()),
            confidence_label_lang(row.confidence, lang),
            row.source
        ));
    }

    lines
}

fn balance_overview_lines_lang(snapshot: &Snapshot, limit: usize, lang: Language) -> Vec<String> {
    let mut stations = summarize_station_balances(&snapshot.provider_balances);
    if stations.is_empty() {
        return vec![i18n::label(lang, "no balance/quota data").to_string()];
    }

    stations.sort_by(|left, right| {
        station_priority(left)
            .cmp(&station_priority(right))
            .then_with(|| left.station_name.cmp(&right.station_name))
    });

    let total_rows = stations
        .iter()
        .map(|station| station.total_rows)
        .sum::<usize>();
    let exhausted_rows = stations
        .iter()
        .map(|station| station.exhausted_rows)
        .sum::<usize>();
    let lazy_rows = stations
        .iter()
        .map(|station| station.lazy_rows)
        .sum::<usize>();
    let stale_rows = stations
        .iter()
        .map(|station| station.stale_rows)
        .sum::<usize>();
    let unknown_rows = stations
        .iter()
        .map(|station| station.unknown_rows + station.error_rows)
        .sum::<usize>();

    let mut lines = vec![format!(
        "{}={}  rows={}  {}={}  {}={}  {}={}  {}={}",
        i18n::label(lang, "stations"),
        stations.len(),
        total_rows,
        i18n::label(lang, "exhausted"),
        exhausted_rows,
        i18n::label(lang, "lazy"),
        lazy_rows,
        i18n::label(lang, "stale"),
        stale_rows,
        i18n::label(lang, "unknown"),
        unknown_rows
    )];
    for station in stations.into_iter().take(limit) {
        lines.push(format!(
            "{}  rows={} {}={} {}={} {}={} {}={} {}={}  {}",
            shorten_middle(&station.station_name, 20),
            station.total_rows,
            i18n::label(lang, "ok"),
            station.ok_rows,
            i18n::label(lang, "stale"),
            station.stale_rows,
            i18n::label(lang, "exhausted"),
            station.exhausted_rows,
            i18n::label(lang, "lazy"),
            station.lazy_rows,
            i18n::label(lang, "unknown"),
            station.unknown_rows + station.error_rows,
            station
                .primary
                .as_ref()
                .map(|snapshot| format_primary_balance_lang(snapshot, lang))
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    lines
}

fn prioritized_price_rows(
    snapshot: &Snapshot,
    limit: usize,
) -> Vec<&crate::pricing::ModelPriceView> {
    snapshot
        .pricing_catalog
        .prioritized_models(recent_model_order(snapshot), limit)
}

fn recent_model_order(snapshot: &Snapshot) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for request in &snapshot.recent {
        if let Some(model) = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }
    for row in &snapshot.rows {
        if let Some(model) = row
            .effective_model
            .as_ref()
            .map(|value| value.value.as_str())
            .or(row.last_model.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }

    let mut models = counts.into_iter().collect::<Vec<_>>();
    models.sort_by(|(left_model, left_count), (right_model, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_model.cmp(right_model))
    });
    models.into_iter().map(|(model, _)| model).collect()
}

fn price_model_label(row: &crate::pricing::ModelPriceView) -> String {
    match row.display_name.as_deref() {
        Some(display) if display != row.model_id => format!("{display} ({})", row.model_id),
        Some(display) => display.to_string(),
        None => row.model_id.clone(),
    }
}

fn format_price(value: &str) -> String {
    format!("${value}")
}

fn format_optional_price(value: Option<&str>) -> String {
    value.map(format_price).unwrap_or_else(|| "-".to_string())
}

fn confidence_label_lang(
    confidence: crate::pricing::CostConfidence,
    lang: Language,
) -> &'static str {
    match confidence {
        crate::pricing::CostConfidence::Unknown => i18n::label(lang, "unknown"),
        crate::pricing::CostConfidence::Partial => i18n::label(lang, "partial"),
        crate::pricing::CostConfidence::Estimated => i18n::label(lang, "estimated"),
        crate::pricing::CostConfidence::Exact => i18n::label(lang, "exact"),
    }
}

#[derive(Debug, Clone)]
struct StationBalanceSummary {
    station_name: String,
    total_rows: usize,
    ok_rows: usize,
    stale_rows: usize,
    exhausted_rows: usize,
    lazy_rows: usize,
    error_rows: usize,
    unknown_rows: usize,
    primary: Option<crate::state::ProviderBalanceSnapshot>,
}

fn summarize_station_balances(
    provider_balances: &HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
) -> Vec<StationBalanceSummary> {
    provider_balances
        .iter()
        .map(|(station_name, balances)| StationBalanceSummary {
            station_name: station_name.clone(),
            total_rows: balances.len(),
            ok_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Ok)
                .count(),
            stale_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Stale)
                .count(),
            exhausted_rows: balances
                .iter()
                .filter(|balance| {
                    balance.status == crate::state::BalanceSnapshotStatus::Exhausted
                        && !balance.routing_ignored_exhaustion()
                })
                .count(),
            lazy_rows: balances
                .iter()
                .filter(|balance| balance.routing_ignored_exhaustion())
                .count(),
            error_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Error)
                .count(),
            unknown_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Unknown)
                .count(),
            primary: balances.iter().cloned().min_by(balance_priority),
        })
        .collect()
}

fn balance_priority(
    left: &crate::state::ProviderBalanceSnapshot,
    right: &crate::state::ProviderBalanceSnapshot,
) -> std::cmp::Ordering {
    balance_snapshot_rank(left)
        .cmp(&balance_snapshot_rank(right))
        .then_with(|| left.upstream_index.cmp(&right.upstream_index))
        .then_with(|| left.provider_id.cmp(&right.provider_id))
        .then_with(|| left.fetched_at_ms.cmp(&right.fetched_at_ms))
}

fn balance_snapshot_rank(snapshot: &crate::state::ProviderBalanceSnapshot) -> u8 {
    if snapshot.status == crate::state::BalanceSnapshotStatus::Exhausted
        && !snapshot.routing_ignored_exhaustion()
    {
        return 0;
    }

    match snapshot.status {
        crate::state::BalanceSnapshotStatus::Stale
        | crate::state::BalanceSnapshotStatus::Exhausted => 1,
        crate::state::BalanceSnapshotStatus::Error
        | crate::state::BalanceSnapshotStatus::Unknown => 2,
        crate::state::BalanceSnapshotStatus::Ok => 3,
    }
}

fn station_priority(summary: &StationBalanceSummary) -> u8 {
    summary
        .primary
        .as_ref()
        .map(balance_snapshot_rank)
        .unwrap_or(5)
}

fn format_primary_balance_lang(
    snapshot: &crate::state::ProviderBalanceSnapshot,
    lang: Language,
) -> String {
    let mut line = format!(
        "{}  #{}  {}  {}",
        shorten_middle(&snapshot.provider_id, 20),
        snapshot
            .upstream_index
            .map(|idx| idx.to_string())
            .unwrap_or_else(|| "-".to_string()),
        balance_snapshot_status_label_lang(snapshot, lang),
        balance_amount_brief_lang(snapshot, lang).unwrap_or_else(|| snapshot.amount_summary())
    );
    if let Some(err) = snapshot.error.as_deref()
        && !err.trim().is_empty()
    {
        line.push_str(&format!(
            "  {}={}",
            i18n::label(lang, "lookup_failed"),
            shorten(err, 48)
        ));
    }
    line
}

pub(super) fn render_settings_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let now_epoch_ms = now_ms();
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::SETTINGS_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();

    let lang_name = i18n::language_name(ui.language);
    let refresh_env = std::env::var("CODEX_HELPER_TUI_REFRESH_MS").ok();
    let recent_max_env = std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX").ok();
    let health_timeout_env = std::env::var(HEALTHCHECK_TIMEOUT_MS_ENV).ok();
    let health_inflight_env = std::env::var(HEALTHCHECK_MAX_INFLIGHT_ENV).ok();
    let health_upstream_conc_env = std::env::var(HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV).ok();

    let effective_recent_max = recent_max_env
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_000)
        .clamp(200, 20_000);

    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;
    let ok_pct = |ok: usize, total: usize| -> String {
        if total == 0 {
            "-".to_string()
        } else {
            format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
        }
    };

    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::RUNTIME_OVERVIEW_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("5m ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s5.ok_2xx, s5.total),
                s5.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.err_429,
                s5.err_5xx,
                s5.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("1h ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s1.ok_2xx, s1.total),
                s1.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.err_429,
                s1.err_5xx,
                s1.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some((pid, n)) = s5.top_provider.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top provider: ", Style::default().fg(p.muted)),
            Span::styled(pid.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }
    if let Some((cfg, n)) = s5.top_config.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top station: ", Style::default().fg(p.muted)),
            Span::styled(cfg.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::BALANCE_OVERVIEW_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    for line in balance_overview_lines_lang(snapshot, 6, ui.language) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(p.muted)),
            Span::styled(shorten_middle(&line, 110), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PRICING_CATALOG_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    for line in pricing_catalog_preview_lines_lang(snapshot, 6, ui.language) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(p.muted)),
            Span::styled(line, Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::TUI_OPTIONS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(lang_name, Style::default().fg(p.text)),
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_TOGGLE_HINT),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::REFRESH_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{}ms", ui.refresh_ms), Style::default().fg(p.text)),
        Span::styled(
            format!(
                "  env CODEX_HELPER_TUI_REFRESH_MS={}",
                refresh_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::WINDOW_SAMPLES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("recent_finished_max={effective_recent_max}"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            format!(
                "  env CODEX_HELPER_RECENT_FINISHED_MAX={}",
                recent_max_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PROFILE_CONTROL_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::CONFIGURED_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.configured_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            i18n::text(ui.language, msg::PRESS_P_MANAGE),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::RUNTIME_OVERRIDE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_default_profile_override
                .as_deref()
                .unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            i18n::text(ui.language, msg::PRESS_CAPITAL_P_MANAGE),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::EFFECTIVE_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.effective_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
    ]));
    let profile_list = if ui.profile_options.is_empty() {
        i18n::text(ui.language, msg::NO_PROFILES).to_string()
    } else {
        shorten_middle(
            ui.profile_options
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
                .as_str(),
            110,
        )
    };
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::AVAILABLE_PROFILES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(profile_list, Style::default().fg(p.text)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::HEALTH_CHECK_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "timeout_ms={}  max_inflight={}  upstream_concurrency={}",
            health_timeout_env.as_deref().unwrap_or("-"),
            health_inflight_env.as_deref().unwrap_or("-"),
            health_upstream_conc_env.as_deref().unwrap_or("-"),
        ),
        Style::default().fg(p.muted),
    )]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PATHS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "config file")),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            crate::config::config_file_path().display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    let home = crate::config::proxy_home_dir();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "home"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(home.display().to_string(), Style::default().fg(p.text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "logs"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            home.join("logs").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "reports"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            home.join("reports").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::RUNTIME_CONFIG_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    let loaded = ui
        .last_runtime_config_loaded_at_ms
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    let mtime = ui
        .last_runtime_config_source_mtime_ms
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    lines.push(Line::from(vec![
        Span::styled("loaded_at_ms: ", Style::default().fg(p.muted)),
        Span::styled(loaded, Style::default().fg(p.text)),
        Span::styled("  mtime_ms: ", Style::default().fg(p.muted)),
        Span::styled(mtime, Style::default().fg(p.text)),
        Span::styled(
            i18n::text(ui.language, msg::PRESS_R_RELOAD),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some(retry) = ui.last_runtime_retry.as_ref() {
        lines.push(Line::from(vec![Span::styled(
            i18n::label(ui.language, "retry policy"),
            Style::default().fg(p.text),
        )]));
        for line in retry_policy_preview_lines_lang(retry, ui.language) {
            lines.push(Line::from(vec![
                Span::styled("  - ", Style::default().fg(p.muted)),
                Span::styled(line, Style::default().fg(p.muted)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::COMMON_KEYS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(i18n::text(
        ui.language,
        if ui.service_name == "codex" {
            msg::COMMON_KEYS_CODEX
        } else {
            msg::COMMON_KEYS_OTHER
        },
    )));

    lines.push(Line::from(""));
    let updated_ms = snapshot.refreshed_at.elapsed().as_millis();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "updated")),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{updated_ms}ms"), Style::default().fg(p.muted)),
        Span::raw("  "),
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "now")),
            Style::default().fg(p.muted),
        ),
        Span::styled(now_epoch_ms.to_string(), Style::default().fg(p.muted)),
    ]));

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_layer(strategy: RetryStrategy, attempts: u32) -> ResolvedRetryLayerConfig {
        ResolvedRetryLayerConfig {
            max_attempts: attempts,
            backoff_ms: 100,
            backoff_max_ms: 1_000,
            jitter_ms: 25,
            on_status: "429,500-599".to_string(),
            on_class: vec!["upstream_transport_error".to_string()],
            strategy,
        }
    }

    #[test]
    fn retry_policy_preview_lines_explain_layers_and_boundary() {
        let retry = ResolvedRetryConfig {
            upstream: retry_layer(RetryStrategy::SameUpstream, 2),
            route: retry_layer(RetryStrategy::Failover, 3),
            allow_cross_station_before_first_output: true,
            never_on_status: "400,401,403".to_string(),
            never_on_class: vec!["client_error_non_retryable".to_string()],
            cloudflare_challenge_cooldown_secs: 60,
            cloudflare_timeout_cooldown_secs: 30,
            transport_cooldown_secs: 45,
            cooldown_backoff_factor: 2,
            cooldown_backoff_max_secs: 900,
        };

        let lines = retry_policy_preview_lines(&retry);

        assert!(lines[0].contains("upstream: strategy=same_upstream attempts=2"));
        assert!(lines[1].contains("route: strategy=failover attempts=3"));
        assert!(lines[2].contains("cross-station failover allowed before first output"));
        assert!(lines[3].contains("never_status=[400,401,403]"));
        assert!(lines[4].contains("transport=45s"));
    }
}
