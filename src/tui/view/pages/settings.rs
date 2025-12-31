use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::config::RetryStrategy;
use crate::tui::model::{Palette, Snapshot, now_ms};
use crate::tui::state::UiState;

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
            Span::styled("5m top config: ", Style::default().fg(p.muted)),
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
        Span::styled("config: ", Style::default().fg(p.muted)),
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
        let strategy = match retry.strategy {
            RetryStrategy::Failover => "failover",
            RetryStrategy::SameUpstream => "same_upstream",
        };
        lines.push(Line::from(vec![
            Span::styled("retry: ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "strategy={} attempts={} backoff={}..{} jitter={} cooldown(cf_chal={}s cf_to={}s transport={}s)",
                    strategy,
                    retry.max_attempts,
                    retry.backoff_ms,
                    retry.backoff_max_ms,
                    retry.jitter_ms,
                    retry.cloudflare_challenge_cooldown_secs,
                    retry.cloudflare_timeout_cooldown_secs,
                    retry.transport_cooldown_secs
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  on_status: ", Style::default().fg(p.muted)),
            Span::styled(retry.on_status.clone(), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "常用快捷键", "Common keys"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(crate::tui::i18n::pick(
        ui.language,
        if ui.service_name == "codex" {
            "  1-6 切页  ? 帮助  q 退出  L 语言  (Configs: i 详情  Stats: y 导出/复制  Settings: R 重载配置  O 覆盖导入(二次确认))"
        } else {
            "  1-6 切页  ? 帮助  q 退出  L 语言  (Configs: i 详情  Stats: y 导出/复制)"
        },
        if ui.service_name == "codex" {
            "  1-6 pages  ? help  q quit  L language  (Configs: i details  Stats: y export/copy  Settings: R reload  O overwrite(confirm))"
        } else {
            "  1-6 pages  ? help  q quit  L language  (Configs: i details  Stats: y export/copy)"
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
