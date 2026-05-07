use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::config::{ResolvedRetryConfig, ResolvedRetryLayerConfig, RetryStrategy};
use crate::tui::model::{Palette, Snapshot, now_ms, shorten_middle};
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

fn retry_layer_preview(label: &str, layer: &ResolvedRetryLayerConfig) -> String {
    format!(
        "{label}: strategy={} attempts={} backoff={}..{}ms jitter={}ms retry_on={}",
        retry_strategy_name(layer.strategy),
        layer.max_attempts,
        layer.backoff_ms,
        layer.backoff_max_ms,
        layer.jitter_ms,
        retry_trigger_summary(layer)
    )
}

fn retry_policy_preview_lines(retry: &ResolvedRetryConfig) -> Vec<String> {
    let mut lines = vec![
        retry_layer_preview("upstream", &retry.upstream),
        retry_layer_preview("provider", &retry.provider),
    ];
    let boundary = if retry.allow_cross_station_before_first_output {
        "boundary: cross-station failover allowed before first output; after output stays on committed route"
    } else {
        "boundary: cross-station failover blocked before first output; same-station/upstream policy only"
    };
    lines.push(boundary.to_string());
    let never_class = if retry.never_on_class.is_empty() {
        "-".to_string()
    } else {
        retry.never_on_class.join(",")
    };
    lines.push(format!(
        "guardrails: never_status=[{}] never_class=[{}]",
        retry.never_on_status, never_class
    ));
    lines.push(format!(
        "cooldown: transport={}s cf_challenge={}s cf_timeout={}s backoff_factor={} max={}s",
        retry.transport_cooldown_secs,
        retry.cloudflare_challenge_cooldown_secs,
        retry.cloudflare_timeout_cooldown_secs,
        retry.cooldown_backoff_factor,
        retry.cooldown_backoff_max_secs
    ));
    lines
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
            crate::tui::i18n::pick(ui.language, "设置", "Settings"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();

    let lang_name = match ui.language {
        crate::tui::Language::Zh => "中文",
        crate::tui::Language::En => "English",
    };
    let refresh_env = std::env::var("CODEX_HELPER_TUI_REFRESH_MS").ok();
    let recent_max_env = std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX").ok();
    let health_timeout_env = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_TIMEOUT_MS").ok();
    let health_inflight_env = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_MAX_INFLIGHT").ok();
    let health_upstream_conc_env =
        std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_UPSTREAM_CONCURRENCY").ok();

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
        crate::tui::i18n::pick(ui.language, "运行态概览", "Runtime overview"),
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
        crate::tui::i18n::pick(ui.language, "TUI 选项", "TUI options"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "语言：", "language: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(lang_name, Style::default().fg(p.text)),
        Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "  (按 L 切换，并落盘到 ui.language)",
                "  (press L to toggle and persist to ui.language)",
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "刷新间隔：", "refresh: "),
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
            crate::tui::i18n::pick(ui.language, "窗口采样：", "window samples: "),
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
        crate::tui::i18n::pick(ui.language, "Profile 控制", "Profile control"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "配置默认：", "configured default: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.configured_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "  (按 p 管理)", "  (press p to manage)"),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "运行时覆盖：", "runtime override: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_default_profile_override
                .as_deref()
                .unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "  (按 P 管理)", "  (press P to manage)"),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "当前生效：", "effective default: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.effective_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
    ]));
    let profile_list = if ui.profile_options.is_empty() {
        crate::tui::i18n::pick(ui.language, "<no profiles>", "<no profiles>").to_string()
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
            crate::tui::i18n::pick(ui.language, "可用 profile：", "available profiles: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(profile_list, Style::default().fg(p.text)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "Health Check", "Health Check"),
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
        crate::tui::i18n::pick(ui.language, "路径", "Paths"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("config file: ", Style::default().fg(p.muted)),
        Span::styled(
            crate::config::config_file_path().display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    let home = crate::config::proxy_home_dir();
    lines.push(Line::from(vec![
        Span::styled("home:   ", Style::default().fg(p.muted)),
        Span::styled(home.display().to_string(), Style::default().fg(p.text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("logs:   ", Style::default().fg(p.muted)),
        Span::styled(
            home.join("logs").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("reports:", Style::default().fg(p.muted)),
        Span::styled(
            home.join("reports").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "运行态配置", "Runtime config"),
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
            crate::tui::i18n::pick(ui.language, "  (按 R 立即重载)", "  (press R to reload)"),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some(retry) = ui.last_runtime_retry.as_ref() {
        lines.push(Line::from(vec![Span::styled(
            "retry policy:",
            Style::default().fg(p.text),
        )]));
        for line in retry_policy_preview_lines(retry) {
            lines.push(Line::from(vec![
                Span::styled("  - ", Style::default().fg(p.muted)),
                Span::styled(line, Style::default().fg(p.muted)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "常用快捷键", "Common keys"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(crate::tui::i18n::pick(
        ui.language,
        if ui.service_name == "codex" {
            "  1-8 切页  ? 帮助  q 退出  L 语言  (Stations: i 详情  Stats: y 导出/复制  Settings: R 重载配置  O 覆盖导入(二次确认))"
        } else {
            "  1-8 切页  ? 帮助  q 退出  L 语言  (Stations: i 详情  Stats: y 导出/复制)"
        },
        if ui.service_name == "codex" {
            "  1-8 pages  ? help  q quit  L language  (Stations: i details  Stats: y export/copy  Settings: R reload  O overwrite(confirm))"
        } else {
            "  1-8 pages  ? help  q quit  L language  (Stations: i details  Stats: y export/copy)"
        },
    )));

    lines.push(Line::from(""));
    let updated_ms = snapshot.refreshed_at.elapsed().as_millis();
    lines.push(Line::from(vec![
        Span::styled("updated: ", Style::default().fg(p.muted)),
        Span::styled(format!("{updated_ms}ms"), Style::default().fg(p.muted)),
        Span::raw("  "),
        Span::styled("now: ", Style::default().fg(p.muted)),
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
            provider: retry_layer(RetryStrategy::Failover, 3),
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
        assert!(lines[1].contains("provider: strategy=failover attempts=3"));
        assert!(lines[2].contains("cross-station failover allowed before first output"));
        assert!(lines[3].contains("never_status=[400,401,403]"));
        assert!(lines[4].contains("transport=45s"));
    }
}
