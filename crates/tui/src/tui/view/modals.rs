use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::dashboard_core::ControlProfileOption;
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, compute_window_stats, now_ms,
    provider_balance_compact_lang, provider_tags_brief, routing_context_balance_rank, shorten,
    shorten_middle, station_balance_brief_lang, station_primary_balance_snapshot,
};
use crate::tui::state::UiState;
use crate::tui::types::{EffortChoice, Overlay, Page, ServiceTierChoice};

use super::widgets::centered_rect;

fn help_heading(text: impl Into<String>, p: Palette) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.into(),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )])
}

fn help_current_page_title(lang: Language, page: Page, is_route_graph: bool) -> &'static str {
    match (lang, page, is_route_graph) {
        (Language::Zh, Page::Dashboard, _) => "当前页面：总览",
        (Language::Zh, Page::Stations, true) => "当前页面：路由",
        (Language::Zh, Page::Stations, false) => "当前页面：站点",
        (Language::Zh, Page::Sessions, _) => "当前页面：会话",
        (Language::Zh, Page::Requests, _) => "当前页面：请求",
        (Language::Zh, Page::Stats, _) => "当前页面：提供商",
        (Language::Zh, Page::Settings, _) => "当前页面：设置",
        (Language::Zh, Page::History, _) => "当前页面：历史",
        (Language::Zh, Page::Recent, _) => "当前页面：最近",
        (Language::En, Page::Dashboard, _) => "Current page: Dashboard",
        (Language::En, Page::Stations, true) => "Current page: Routing",
        (Language::En, Page::Stations, false) => "Current page: Stations",
        (Language::En, Page::Sessions, _) => "Current page: Sessions",
        (Language::En, Page::Requests, _) => "Current page: Requests",
        (Language::En, Page::Stats, _) => "Current page: Providers",
        (Language::En, Page::Settings, _) => "Current page: Settings",
        (Language::En, Page::History, _) => "Current page: History",
        (Language::En, Page::Recent, _) => "Current page: Recent",
    }
}

fn current_page_help_lines(
    lang: Language,
    page: Page,
    is_route_graph: bool,
    is_codex_service: bool,
    p: Palette,
) -> Vec<Line<'static>> {
    let mut lines = vec![help_heading(
        help_current_page_title(lang, page, is_route_graph),
        p,
    )];

    let entries = match (lang, page, is_route_graph, is_codex_service) {
        (Language::Zh, Page::Dashboard, true, _) => vec![
            "  Tab        切换会话/请求焦点",
            "  b/M/f      会话 profile、model、fast/service tier 覆盖",
            "  Enter      打开 effort 菜单；l/m/h/X 快速设置；x 清除",
            "  p/P        打开会话/全局 route target 编辑",
            "  O/H o/h    从会话或请求面板跳到关联页面",
        ],
        (Language::Zh, Page::Dashboard, false, _) => vec![
            "  Tab        切换会话/请求焦点",
            "  b/M/f      会话 profile、model、fast/service tier 覆盖",
            "  Enter      打开 effort 菜单；l/m/h/X 快速设置；x 清除",
            "  p/P        设置会话站点覆盖 / 全局站点 pin",
            "  O/H o/h    从会话或请求面板跳到关联页面",
        ],
        (Language::Zh, Page::Stations, true, _) => vec![
            "  r/Enter    打开 routing 编辑器",
            "  g          刷新路由预览与余额",
            "  e/f/s      启停、包月优先、耗尽策略",
            "  1/2/0      设置 monthly/paygo/unknown billing tag",
            "  Backspace  清除全局 route target；o/O 设置或清除会话 route target",
            "  []/u/d     调整 provider 顺序",
        ],
        (Language::Zh, Page::Stations, false, _) => vec![
            "  Enter      设置全局站点 pin；Backspace 清除",
            "  o/O        设置或清除当前会话站点覆盖",
            "  i          查看站点详情",
            "  h/H        检查当前/全部站点；c/C 取消检查",
        ],
        (Language::Zh, Page::Sessions, _, _) => vec![
            "  b/M/f      会话 profile、model、fast/service tier 覆盖",
            "  R          重置当前会话 manual overrides",
            "  a/e/v      活跃、错误、覆盖筛选；r 重置筛选",
            "  t          打开全屏对话记录",
            "  o/H        跳到 Requests / History",
        ],
        (Language::Zh, Page::Requests, _, _) => vec![
            "  e          仅看错误",
            "  s          切换当前会话 / 全部请求",
            "  x          清除显式 session 聚焦",
            "  o/h        跳到 Sessions / History",
        ],
        (Language::Zh, Page::Stats, _, _) => vec![
            "  Tab        切换站点汇总 / provider 用量",
            "  a          仅看余额或刷新需要关注的 provider",
            "  g          刷新 provider 余额",
            "  d          切换 today / 7d / 30d / loaded 窗口",
            "  PgUp/PgDn  滚动 provider endpoint 详情；y 复制并导出报告",
        ],
        (Language::Zh, Page::Settings, _, true) => vec![
            "  p/P        管理配置默认 profile / 运行时默认 profile",
            "  R          重载运行时配置",
            "  O          从 ~/.codex 覆盖导入站点，需要二次确认",
        ],
        (Language::Zh, Page::Settings, _, false) => vec![
            "  p/P        管理配置默认 profile / 运行时默认 profile",
            "  R          重载运行时配置",
        ],
        (Language::Zh, Page::History, _, _) => vec![
            "  r          刷新历史会话列表",
            "  t/Enter    打开全屏对话记录",
            "  s/f        跳到 Sessions / Requests",
        ],
        (Language::Zh, Page::Recent, _, _) => vec![
            "  [ / ]      切换时间窗口",
            "  Enter/y    复制选中项 / 复制可见列表",
            "  t          打开全屏对话记录",
            "  s/f/h      跳到 Sessions / Requests / History",
        ],
        (Language::En, Page::Dashboard, true, _) => vec![
            "  Tab        switch Sessions / Requests focus",
            "  b/M/f      session profile, model, fast/service tier overrides",
            "  Enter      open effort menu; l/m/h/X quick set; x clear",
            "  p/P        open session/global route target editor",
            "  O/H o/h    jump from session or request panels",
        ],
        (Language::En, Page::Dashboard, false, _) => vec![
            "  Tab        switch Sessions / Requests focus",
            "  b/M/f      session profile, model, fast/service tier overrides",
            "  Enter      open effort menu; l/m/h/X quick set; x clear",
            "  p/P        set session station override / global station pin",
            "  O/H o/h    jump from session or request panels",
        ],
        (Language::En, Page::Stations, true, _) => vec![
            "  r/Enter    open routing editor",
            "  g          refresh routing preview and balances",
            "  e/f/s      enable, monthly-first, exhausted action",
            "  1/2/0      set monthly/paygo/unknown billing tag",
            "  Backspace  clear global route target; o/O set or clear session route target",
            "  []/u/d     reorder providers",
        ],
        (Language::En, Page::Stations, false, _) => vec![
            "  Enter      set global station pin; Backspace clears it",
            "  o/O        set or clear current session station override",
            "  i          open station details",
            "  h/H        check selected/all stations; c/C cancel checks",
        ],
        (Language::En, Page::Sessions, _, _) => vec![
            "  b/M/f      session profile, model, fast/service tier overrides",
            "  R          reset current session manual overrides",
            "  a/e/v      active, error, override filters; r resets filters",
            "  t          open full-screen transcript",
            "  o/H        jump to Requests / History",
        ],
        (Language::En, Page::Requests, _, _) => vec![
            "  e          toggle errors-only",
            "  s          switch current session / all requests",
            "  x          clear explicit session focus",
            "  o/h        jump to Sessions / History",
        ],
        (Language::En, Page::Stats, _, _) => vec![
            "  Tab        switch station rollup / provider usage",
            "  a          show providers needing balance or refresh attention",
            "  g          refresh provider balances",
            "  d          cycle today / 7d / 30d / loaded window",
            "  PgUp/PgDn  scroll provider endpoint details; y copies and exports a report",
        ],
        (Language::En, Page::Settings, _, true) => vec![
            "  p/P        manage configured default profile / runtime default profile",
            "  R          reload runtime config",
            "  O          overwrite-import stations from ~/.codex, with confirmation",
        ],
        (Language::En, Page::Settings, _, false) => vec![
            "  p/P        manage configured default profile / runtime default profile",
            "  R          reload runtime config",
        ],
        (Language::En, Page::History, _, _) => vec![
            "  r          refresh history session list",
            "  t/Enter    open full-screen transcript",
            "  s/f        jump to Sessions / Requests",
        ],
        (Language::En, Page::Recent, _, _) => vec![
            "  [ / ]      switch time window",
            "  Enter/y    copy selected item / visible list",
            "  t          open full-screen transcript",
            "  s/f/h      jump to Sessions / Requests / History",
        ],
    };

    lines.extend(entries.into_iter().map(Line::from));
    lines.push(Line::from(""));
    lines
}

#[cfg(test)]
fn help_text_for_tests(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn profile_option_to_service_profile(
    profile: &ControlProfileOption,
) -> crate::config::ServiceControlProfile {
    crate::config::ServiceControlProfile {
        extends: profile.extends.clone(),
        station: None,
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
    }
}

fn resolve_profile_from_options(
    profile_name: &str,
    profiles: &[ControlProfileOption],
) -> anyhow::Result<crate::config::ServiceControlProfile> {
    let profile_catalog = profiles
        .iter()
        .map(|profile| {
            (
                profile.name.clone(),
                profile_option_to_service_profile(profile),
            )
        })
        .collect::<BTreeMap<_, _>>();
    crate::config::resolve_service_profile_from_catalog(&profile_catalog, profile_name)
}

fn format_profile_route_summary(profile: &crate::config::ServiceControlProfile) -> String {
    format!(
        "model={}  reasoning={}  tier={}",
        profile.model.as_deref().unwrap_or("<auto>"),
        profile.reasoning_effort.as_deref().unwrap_or("<auto>"),
        profile.service_tier.as_deref().unwrap_or("<auto>"),
    )
}

fn profile_declared_summary(profile: &ControlProfileOption, lang: Language) -> String {
    let mut parts = Vec::new();
    if let Some(extends) = profile.extends.as_deref() {
        parts.push(format!("extends={extends}"));
    }
    parts.push(format!(
        "model={}",
        profile.model.as_deref().unwrap_or("<auto>")
    ));
    parts.push(format!(
        "reasoning={}",
        profile.reasoning_effort.as_deref().unwrap_or("<auto>")
    ));
    parts.push(format!(
        "tier={}",
        profile.service_tier.as_deref().unwrap_or("<auto>")
    ));
    format!(
        "{} {}",
        i18n::text(lang, msg::DECLARED_LABEL),
        shorten_middle(parts.join("  ").as_str(), 72)
    )
}

fn profile_resolved_summary(
    profile_name: &str,
    profiles: &[ControlProfileOption],
    lang: Language,
) -> (String, bool) {
    match resolve_profile_from_options(profile_name, profiles) {
        Ok(profile) => (
            format!(
                "{} {}",
                i18n::text(lang, msg::RESOLVED_LABEL),
                shorten_middle(format_profile_route_summary(&profile).as_str(), 72)
            ),
            false,
        ),
        Err(err) => (
            format!(
                "{} {}",
                i18n::text(lang, msg::RESOLVE_FAILED_LABEL),
                shorten_middle(err.to_string().as_str(), 72)
            ),
            true,
        ),
    }
}

pub(super) fn render_station_info_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
) {
    let area = centered_rect(84, 84, f.area());
    f.render_widget(Clear, area);

    let selected_session = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.as_deref())
        .unwrap_or("-");
    let session_override = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.override_station_name.as_deref());
    let global_station_override = snapshot.global_station_override.as_deref();

    let selected = providers.get(ui.selected_station_idx);
    let title = if let Some(cfg) = selected {
        let level = cfg.level.clamp(1, 10);
        format!(
            "{}: {} (L{})",
            i18n::text(ui.language, msg::OVERLAY_STATION_DETAILS),
            cfg.name,
            level
        )
    } else {
        i18n::text(ui.language, msg::OVERLAY_STATION_DETAILS).to_string()
    };

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::SESSION_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(selected_session.to_string(), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled(
            i18n::text(ui.language, msg::PINNED_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            if let Some(s) = session_override {
                format!("session={s}")
            } else if let Some(g) = global_station_override {
                format!("global={g}")
            } else {
                "-".to_string()
            },
            Style::default().fg(
                if session_override.is_some() || global_station_override.is_some() {
                    p.accent
                } else {
                    p.muted
                },
            ),
        ),
        Span::raw("   "),
        Span::styled(
            i18n::text(ui.language, msg::KEYS_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            i18n::text(ui.language, msg::FOOTER_STATION_INFO),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(cfg) = selected {
        let now = now_ms();

        let stats_5m_cfg = compute_window_stats(&snapshot.recent, now, 5 * 60_000, |r| {
            r.station_name.as_deref() == Some(cfg.name.as_str())
        });
        let stats_1h_cfg = compute_window_stats(&snapshot.recent, now, 60 * 60_000, |r| {
            r.station_name.as_deref() == Some(cfg.name.as_str())
        });

        let fmt_ok_pct = |ok: usize, total: usize| -> String {
            if total == 0 {
                "-".to_string()
            } else {
                format!("{:>2}%", ((ok as f64) * 100.0 / (total as f64)).round())
            }
        };
        let fmt_ms = |ms: Option<u64>| -> String {
            ms.map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".to_string())
        };
        let fmt_attempts = |a: Option<f64>| -> String {
            a.map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "-".to_string())
        };
        let fmt_rate_pct = |r: Option<f64>| -> String {
            r.map(|v| format!("{:.0}%", v * 100.0))
                .unwrap_or_else(|| "-".to_string())
        };

        let (enabled_ovr, level_ovr) = snapshot
            .station_meta_overrides
            .get(cfg.name.as_str())
            .copied()
            .unwrap_or((None, None));
        let enabled = enabled_ovr.unwrap_or(cfg.enabled);
        let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);

        if let Some(alias) = cfg.alias.as_deref()
            && !alias.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled(
                    i18n::text(ui.language, msg::ALIAS_LABEL),
                    Style::default().fg(p.muted),
                ),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(
                i18n::text(ui.language, msg::STATUS_LABEL),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                i18n::text(
                    ui.language,
                    if enabled {
                        msg::ENABLED_LABEL
                    } else {
                        msg::DISABLED_LABEL
                    },
                ),
                Style::default().fg(if enabled { p.good } else { p.warn }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("L{level}"),
                Style::default().fg(if level_ovr.is_some() {
                    p.accent
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                if cfg.active { "active" } else { "" },
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));
        lines.push(Line::from(""));

        lines.push(Line::from(vec![Span::styled(
            i18n::text(ui.language, msg::RUNTIME_HEALTH_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![
            Span::styled("5m ", Style::default().fg(p.muted)),
            Span::styled(
                i18n::text(ui.language, msg::OK_PREFIX),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                fmt_ok_pct(stats_5m_cfg.ok_2xx, stats_5m_cfg.total),
                Style::default().fg(
                    if stats_5m_cfg.total > 0 && stats_5m_cfg.ok_2xx == stats_5m_cfg.total {
                        p.good
                    } else {
                        p.muted
                    },
                ),
            ),
            Span::raw("  "),
            Span::styled("p95 ", Style::default().fg(p.muted)),
            Span::styled(fmt_ms(stats_5m_cfg.p95_ms), Style::default().fg(p.muted)),
            Span::raw("  "),
            Span::styled("att ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_attempts(stats_5m_cfg.avg_attempts),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("r ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_rate_pct(stats_5m_cfg.retry_rate),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("429 ", Style::default().fg(p.muted)),
            Span::styled(
                stats_5m_cfg.err_429.to_string(),
                Style::default().fg(if stats_5m_cfg.err_429 > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled("5xx ", Style::default().fg(p.muted)),
            Span::styled(
                stats_5m_cfg.err_5xx.to_string(),
                Style::default().fg(if stats_5m_cfg.err_5xx > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("n={}", stats_5m_cfg.total),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("1h ", Style::default().fg(p.muted)),
            Span::styled(
                i18n::text(ui.language, msg::OK_PREFIX),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                fmt_ok_pct(stats_1h_cfg.ok_2xx, stats_1h_cfg.total),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("p95 ", Style::default().fg(p.muted)),
            Span::styled(fmt_ms(stats_1h_cfg.p95_ms), Style::default().fg(p.muted)),
            Span::raw("  "),
            Span::styled("att ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_attempts(stats_1h_cfg.avg_attempts),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("r ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_rate_pct(stats_1h_cfg.retry_rate),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("429 ", Style::default().fg(p.muted)),
            Span::styled(
                stats_1h_cfg.err_429.to_string(),
                Style::default().fg(if stats_1h_cfg.err_429 > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled("5xx ", Style::default().fg(p.muted)),
            Span::styled(
                stats_1h_cfg.err_5xx.to_string(),
                Style::default().fg(if stats_1h_cfg.err_5xx > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("n={}", stats_1h_cfg.total),
                Style::default().fg(p.muted),
            ),
        ]));
        if let Some((pid, cnt)) = stats_5m_cfg.top_provider.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("5m top ", Style::default().fg(p.muted)),
                Span::styled(pid.to_string(), Style::default().fg(p.text)),
                Span::styled(format!("  n={cnt}"), Style::default().fg(p.muted)),
            ]));
        }
        lines.push(Line::from(""));

        lines.push(Line::from(vec![Span::styled(
            i18n::text(ui.language, msg::UPSTREAMS_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));

        let health = snapshot.station_health.get(cfg.name.as_str());
        let lb = snapshot.lb_view.get(cfg.name.as_str());

        let (rt5_by_upstream, rt1_by_upstream) = {
            use std::collections::HashMap;

            #[derive(Default)]
            struct Rt {
                total: usize,
                ok: usize,
                err_429: usize,
                err_5xx: usize,
                ok_lat_ms: Vec<u64>,
                attempts_sum: u64,
                retry_cnt: u64,
            }

            fn add(map: &mut HashMap<String, Rt>, r: &crate::state::FinishedRequest) {
                let Some(url) = r.upstream_base_url.as_deref() else {
                    return;
                };
                if url.trim().is_empty() {
                    return;
                }
                let e = map.entry(url.to_string()).or_default();
                e.total += 1;
                let attempts = r.attempt_count();
                e.attempts_sum = e.attempts_sum.saturating_add(attempts as u64);
                if attempts > 1 {
                    e.retry_cnt = e.retry_cnt.saturating_add(1);
                }
                if r.status_code == 429 {
                    e.err_429 += 1;
                } else if (500..600).contains(&r.status_code) {
                    e.err_5xx += 1;
                }
                if (200..300).contains(&r.status_code) {
                    e.ok += 1;
                    e.ok_lat_ms.push(r.duration_ms);
                }
            }

            let mut m5: HashMap<String, Rt> = HashMap::new();
            let mut m1: HashMap<String, Rt> = HashMap::new();
            let cutoff_5 = now.saturating_sub(5 * 60_000);
            let cutoff_1 = now.saturating_sub(60 * 60_000);
            for r in snapshot.recent.iter() {
                if r.station_name.as_deref() != Some(cfg.name.as_str()) {
                    continue;
                }
                if r.ended_at_ms >= cutoff_5 {
                    add(&mut m5, r);
                }
                if r.ended_at_ms >= cutoff_1 {
                    add(&mut m1, r);
                }
            }
            (m5, m1)
        };

        if cfg.upstreams.is_empty() {
            lines.push(Line::from(Span::styled(
                i18n::text(ui.language, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        } else {
            for (idx, up) in cfg.upstreams.iter().enumerate() {
                let pid = up.provider_id.as_deref().unwrap_or("-");
                let auth = up.auth.as_str();

                let (ok, status_code, latency_ms, err) = health
                    .and_then(|h| h.upstreams.iter().find(|u| u.base_url == up.base_url))
                    .map(|u| (u.ok, u.status_code, u.latency_ms, u.error.as_deref()))
                    .unwrap_or((None, None, None, None));

                let health_text = if let Some(ok) = ok {
                    if ok {
                        format!(
                            "ok {} {}",
                            status_code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            latency_ms
                                .map(|m| format!("{m}ms"))
                                .unwrap_or_else(|| "-".to_string())
                        )
                    } else {
                        format!(
                            "err {}",
                            status_code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "-".to_string())
                        )
                    }
                } else {
                    i18n::text(ui.language, msg::NOT_CHECKED).to_string()
                };

                let lb_text = lb
                    .and_then(|v| v.upstreams.get(idx))
                    .map(|u| {
                        let mut parts = Vec::new();
                        if lb.and_then(|v| v.last_good_index) == Some(idx) {
                            parts.push("last_good".to_string());
                        }
                        if u.failure_count > 0 {
                            parts.push(format!("fail={}", u.failure_count));
                        }
                        if let Some(secs) = u.cooldown_remaining_secs {
                            parts.push(format!("cooldown={secs}s"));
                        }
                        if u.usage_exhausted {
                            parts.push("exhausted".to_string());
                        }
                        if parts.is_empty() {
                            "-".to_string()
                        } else {
                            parts.join(" ")
                        }
                    })
                    .unwrap_or_else(|| "-".to_string());

                let models_text = if up.supported_models.is_empty() && up.model_mapping.is_empty() {
                    i18n::text(ui.language, msg::MODELS_ALL).to_string()
                } else {
                    let allow = up.supported_models.len();
                    let map = up.model_mapping.len();
                    match ui.language {
                        Language::Zh => format!(
                            "{}：{} {allow} / {} {map}",
                            i18n::label(ui.language, "models"),
                            i18n::label(ui.language, "allow"),
                            i18n::label(ui.language, "map")
                        ),
                        Language::En => format!("models: allow {allow} / map {map}"),
                    }
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(pid.to_string(), Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(
                        shorten_middle(&up.base_url, 100),
                        Style::default().fg(p.text),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        format!("{}: ", i18n::label(ui.language, "auth")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(auth.to_string(), Style::default().fg(p.text)),
                    Span::raw("   "),
                    Span::styled(models_text, Style::default().fg(p.muted)),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        format!("{}: ", i18n::label(ui.language, "health")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(
                        health_text,
                        Style::default().fg(if ok == Some(true) { p.good } else { p.warn }),
                    ),
                    Span::raw("   "),
                    Span::styled(
                        format!("{}: ", i18n::label(ui.language, "lb")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(lb_text, Style::default().fg(p.muted)),
                ]));

                let runtime_line = {
                    fn pct(ok: usize, total: usize) -> String {
                        if total == 0 {
                            "-".to_string()
                        } else {
                            format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
                        }
                    }
                    fn p95(mut v: Vec<u64>) -> Option<u64> {
                        if v.is_empty() {
                            return None;
                        }
                        let n = v.len();
                        let idx =
                            ((0.95 * (n.saturating_sub(1) as f64)).ceil() as usize).min(n - 1);
                        let (_, nth, _) = v.select_nth_unstable(idx);
                        Some(*nth)
                    }
                    fn att(sum: u64, total: usize) -> String {
                        if total == 0 {
                            "-".to_string()
                        } else {
                            format!("{:.1}", sum as f64 / total as f64)
                        }
                    }

                    let rt5 = rt5_by_upstream.get(&up.base_url);
                    let rt1 = rt1_by_upstream.get(&up.base_url);

                    let s5 = rt5
                        .map(|x| {
                            let p95_ms = p95(x.ok_lat_ms.clone())
                                .map(|v| format!("{v}ms"))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "5m ok{} p95={} att{} 429={} 5xx={}",
                                pct(x.ok, x.total),
                                p95_ms,
                                att(x.attempts_sum, x.total),
                                x.err_429,
                                x.err_5xx
                            )
                        })
                        .unwrap_or_else(|| "5m -".to_string());
                    let s1 = rt1
                        .map(|x| {
                            let p95_ms = p95(x.ok_lat_ms.clone())
                                .map(|v| format!("{v}ms"))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "1h ok{} p95={} att{} 429={} 5xx={}",
                                pct(x.ok, x.total),
                                p95_ms,
                                att(x.attempts_sum, x.total),
                                x.err_429,
                                x.err_5xx
                            )
                        })
                        .unwrap_or_else(|| "1h -".to_string());
                    format!("{s5} | {s1}")
                };
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        format!("{}: ", i18n::label(ui.language, "rt")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(runtime_line, Style::default().fg(p.muted)),
                ]));

                if !up.tags.is_empty() {
                    let tags = up
                        .tags
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!("{}: ", i18n::label(ui.language, "tags")),
                            Style::default().fg(p.muted),
                        ),
                        Span::styled(shorten(&tags, 120), Style::default().fg(p.muted)),
                    ]));
                }

                if !up.supported_models.is_empty() {
                    let samples = up
                        .supported_models
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut s = samples.join(", ");
                    if up.supported_models.len() > samples.len() {
                        s.push_str(", …");
                    }
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!("{}: ", i18n::label(ui.language, "allow")),
                            Style::default().fg(p.muted),
                        ),
                        Span::styled(s, Style::default().fg(p.muted)),
                    ]));
                }
                if !up.model_mapping.is_empty() {
                    let samples = up
                        .model_mapping
                        .iter()
                        .take(6)
                        .map(|(k, v)| format!("{k}->{v}"))
                        .collect::<Vec<_>>();
                    let mut s = samples.join(", ");
                    if up.model_mapping.len() > samples.len() {
                        s.push_str(", …");
                    }
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!("{}: ", i18n::label(ui.language, "map")),
                            Style::default().fg(p.muted),
                        ),
                        Span::styled(s, Style::default().fg(p.muted)),
                    ]));
                }

                if let Some(e) = err
                    && !e.trim().is_empty()
                {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(shorten(e, 140), Style::default().fg(p.muted)),
                    ]));
                }
                lines.push(Line::from(""));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::NO_STATION_SELECTED),
            Style::default().fg(p.muted),
        )));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height);
    ui.station_info_scroll = ui
        .station_info_scroll
        .min(max_scroll.min(u16::MAX as usize) as u16);

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false })
        .scroll((ui.station_info_scroll, 0));
    f.render_widget(content, area);
}

pub(super) fn render_help_modal(f: &mut Frame<'_>, p: Palette, ui: &UiState) {
    let lang = ui.language;
    let is_route_graph = ui.uses_route_graph_routing();
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(lang, msg::OVERLAY_HELP_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines =
        current_page_help_lines(lang, ui.page, is_route_graph, ui.service_name == "codex", p);
    lines.extend(if lang == crate::tui::Language::Zh {
        vec![
            Line::from(vec![Span::styled(
                "导航",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  ↑/↓, j/k   移动选中项"),
            Line::from("  1-8        切换页面"),
            Line::from(
                "            1 总览  2 站点/路由  3 会话  4 请求  5 提供商  6 设置  7 历史  8 最近",
            ),
            Line::from("  L          切换语言（中/英，自动落盘）"),
            Line::from("  6 设置     查看运行态与关键配置入口"),
            Line::from(
                "  设置页      p 管理配置默认 profile；P 管理运行时默认 profile；R 重载配置；O 覆盖导入 ~/.codex（仅 codex）",
            ),
            Line::from("  Tab        切换焦点（总览页）"),
            Line::from(
                "  总览页     b 打开 profile 菜单；M 打开 model 菜单；f 打开 fast / service tier 菜单；R 重置当前会话 manual overrides；O/H 从会话面板跳到 Requests/History；o/h 从请求面板跳到 Sessions/History",
            ),
            Line::from(""),
            Line::from(vec![Span::styled(
                "推理强度",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      打开 effort 菜单（会话列表）"),
            Line::from("  l/m/h/X    设置 low/medium/high/xhigh"),
            Line::from("  x          清除 effort 覆盖"),
            Line::from(if is_route_graph {
                "  R          重置当前会话 model/route_target/effort/service_tier 覆盖"
            } else {
                "  R          重置当前会话 model/station/effort/service_tier 覆盖"
            }),
            Line::from(""),
            Line::from(vec![Span::styled(
                "模型覆盖",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  M          打开 model 菜单（Dashboard/Sessions）"),
            Line::from("  clear      清除当前会话 model 覆盖"),
            Line::from("  Custom...  输入任意 model 名称"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Fast / Service Tier",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  f          打开 fast / service tier 菜单（Dashboard/Sessions）"),
            Line::from("  priority   通常对应 fast mode"),
            Line::from("  Custom...  输入任意 service_tier"),
            Line::from(""),
            Line::from(vec![Span::styled(
                if is_route_graph {
                    "Route target 覆盖"
                } else {
                    "Provider 覆盖"
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from(if is_route_graph {
                "  p/P        打开 route graph 编辑器（provider 选择由 routing policy 管理）"
            } else {
                "  p          会话级 provider 覆盖（固定）"
            }),
            Line::from(if is_route_graph {
                "  r          在 Routing 页打开 routing 编辑器"
            } else {
                "  P          全局站点 pin（运行时）"
            }),
            Line::from("  b          打开 session profile 菜单（Dashboard/Sessions）"),
            Line::from("  Clear binding  清除当前会话已存储的 profile 绑定（保留其他会话覆盖）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                if is_route_graph {
                    "路由页（Routing）"
                } else {
                    "站点页（Stations）"
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from(if is_route_graph {
                "  Enter/r    打开 routing 编辑器（策略/顺序/tags/启停）"
            } else {
                "  Enter      设置为全局 pin"
            }),
            Line::from(if is_route_graph {
                "  Backspace  清除全局 route target"
            } else {
                "  Backspace  清除全局 pin（自动）"
            }),
            Line::from(if is_route_graph {
                "  o          设置会话 route target 为选中 provider"
            } else {
                "  o          设置会话 override 为当前站点"
            }),
            Line::from(if is_route_graph {
                "  O          清除会话 route target"
            } else {
                "  O          清除会话 override"
            }),
            Line::from("  i          查看 Provider 详情（可滚动）"),
            Line::from("  h/H        运行健康检查（当前/全部）"),
            Line::from("  c/C        取消健康检查（当前/全部）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "请求页（Requests）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  s          scope：全部 vs 当前会话"),
            Line::from("  x          清除显式 session 聚焦"),
            Line::from("  o/h        打开到 Sessions / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "会话页（Sessions）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  a          仅看活跃（active-only）"),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  v          仅看覆盖（overrides-only）"),
            Line::from("  r          重置筛选"),
            Line::from("  M          打开 model 菜单"),
            Line::from("  f          打开 fast / service tier 菜单"),
            Line::from("  R          重置当前会话 manual overrides"),
            Line::from("  t          对话记录（全屏）"),
            Line::from("  o/H        打开到 Requests / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "历史页（History）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  r          刷新历史会话列表"),
            Line::from("  t/Enter    打开对话记录（全屏）"),
            Line::from("  s/f        打开到 Sessions / Requests"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "最近页（Recent）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  [ / ]      切换时间窗口"),
            Line::from("  Enter / y  复制选中 / 复制可见列表"),
            Line::from("  t          打开对话记录（全屏）"),
            Line::from("  s/f/h      打开到 Sessions / Requests / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "提供商页（Providers）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        切换焦点（station vs provider）"),
            Line::from("  d          切换窗口（today/7d/30d/loaded）"),
            Line::from("  a          provider 仅看余额/错误关注项"),
            Line::from("  e          recent 仅看错误"),
            Line::from("  PgUp/PgDn  provider 详情滚动"),
            Line::from("  g          刷新余额"),
            Line::from("  y          复制 + 导出报告（当前选中项）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "退出",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  q          退出并触发 shutdown"),
            Line::from("  Esc/?      关闭帮助"),
        ]
    } else {
        vec![
            Line::from(vec![Span::styled(
                "Navigation",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        switch focus (Dashboard)"),
            Line::from("  ↑/↓, j/k   move selection"),
            Line::from("  1-8        switch page"),
            Line::from(
                "            1 Dashboard  2 Stations/Routing  3 Sessions  4 Requests  5 Providers  6 Settings  7 History  8 Recent",
            ),
            Line::from("  L          toggle language (zh/en, persisted)"),
            Line::from("  6 Settings show runtime + station overview"),
            Line::from(
                "  Settings   p manage configured default profile; P manage runtime default profile; R reload settings; O overwrite-import ~/.codex (codex only)",
            ),
            Line::from(
                "  Dashboard  b opens profile menu; M opens model menu; f opens fast / service tier menu; R resets current session manual overrides; O/H jump from Sessions panel to Requests/History; o/h jump from Requests panel to Sessions/History",
            ),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Effort",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      open effort menu (on Sessions)"),
            Line::from("  l/m/h/X    set low/medium/high/xhigh"),
            Line::from("  x          clear effort override"),
            Line::from(if is_route_graph {
                "  R          reset session model/route_target/effort/service_tier overrides"
            } else {
                "  R          reset session model/station/effort/service_tier overrides"
            }),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Model override",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  M          open model menu (Dashboard/Sessions)"),
            Line::from("  clear      clear the session model override"),
            Line::from("  Custom...  enter any model name"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Fast / Service tier",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  f          open fast / service tier menu (Dashboard/Sessions)"),
            Line::from("  priority   usually maps to fast mode"),
            Line::from("  Custom...  enter any service_tier"),
            Line::from(""),
            Line::from(vec![Span::styled(
                if is_route_graph {
                    "Route target override"
                } else {
                    "Provider override"
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from(if is_route_graph {
                "  p/P        open route graph editor (provider choice is routing policy)"
            } else {
                "  p          session provider override (pinned)"
            }),
            Line::from(if is_route_graph {
                "  r          open routing editor on the Routing page"
            } else {
                "  P          global station pin (runtime)"
            }),
            Line::from("  b          open session profile menu (Dashboard/Sessions)"),
            Line::from(
                "  Clear binding  clear the stored session profile binding and keep other session overrides",
            ),
            Line::from(""),
            Line::from(vec![Span::styled(
                if is_route_graph {
                    "Routing page"
                } else {
                    "Stations page"
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from(if is_route_graph {
                "  Enter/r    open routing editor (policy/order/tags/enable)"
            } else {
                "  Enter      set global pin"
            }),
            Line::from(if is_route_graph {
                "  Backspace  clear global route target"
            } else {
                "  Backspace  clear global pin (auto)"
            }),
            Line::from(if is_route_graph {
                "  o          set session route target to selected provider"
            } else {
                "  o          set session override to selected station"
            }),
            Line::from(if is_route_graph {
                "  O          clear session route target"
            } else {
                "  O          clear session override"
            }),
            Line::from("  i          open provider details (scrollable)"),
            Line::from("  h/H        run health checks (selected/all)"),
            Line::from("  c/C        cancel health checks (selected/all)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Requests page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  e          toggle errors-only filter"),
            Line::from("  s          toggle scope (all vs selected session)"),
            Line::from("  x          clear explicit session focus"),
            Line::from("  o/h        open in Sessions / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Sessions page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  a          toggle active-only"),
            Line::from("  e          toggle errors-only"),
            Line::from("  v          toggle overrides-only"),
            Line::from("  r          reset filters"),
            Line::from("  M          open model menu"),
            Line::from("  f          open fast / service tier menu"),
            Line::from("  R          reset current session manual overrides"),
            Line::from("  t          transcript (full-screen)"),
            Line::from("  o/H        open in Requests / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "History page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  r          refresh history session list"),
            Line::from("  t/Enter    open transcript (full-screen)"),
            Line::from("  s/f        open in Sessions / Requests"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Recent page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  [ / ]      switch time window"),
            Line::from("  Enter / y  copy selected / copy visible list"),
            Line::from("  t          open transcript (full-screen)"),
            Line::from("  s/f/h      open in Sessions / Requests / History"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Providers page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        switch focus (station vs provider)"),
            Line::from("  d          cycle time window (today/7d/30d/loaded)"),
            Line::from("  a          provider attention-only balance/error rows"),
            Line::from("  e          toggle errors-only (recent breakdown)"),
            Line::from("  PgUp/PgDn  scroll provider details"),
            Line::from("  g          refresh balances"),
            Line::from("  y          copy + export report (selected item)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Quit",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  q          quit and request shutdown"),
            Line::from("  Esc/?      close this modal"),
        ]
    });

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

pub(super) fn render_session_transcript_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    // Use a full-screen "page-like" overlay so users can mouse-select/copy without
    // accidentally including other panels in the selection rectangle.
    let area = f.area();
    f.render_widget(Clear, area);

    let sid = ui.session_transcript_sid.as_deref().unwrap_or("-");
    let mode = match ui.session_transcript_tail {
        Some(n) => format!("{} {n}", l("tail")),
        None => l("all").to_string(),
    };
    let title = format!(
        "{}: {}  [{mode}]",
        i18n::text(lang, msg::OVERLAY_SESSION_TRANSCRIPT),
        sid
    );

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{}: ", l("sid")), Style::default().fg(p.muted)),
        Span::styled(sid.to_string(), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled(
            i18n::text(lang, msg::KEYS_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            i18n::text(lang, msg::FOOTER_SESSION_TRANSCRIPT),
            Style::default().fg(p.muted),
        ),
    ]));

    if let Some(meta) = ui.session_transcript_meta.as_ref() {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("meta")), Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(meta.id.as_str(), 44),
                Style::default().fg(p.text),
            ),
            Span::raw("   "),
            Span::styled(format!("{}: ", l("cwd")), Style::default().fg(p.muted)),
            Span::styled(
                meta.cwd
                    .as_deref()
                    .map(|s| shorten_middle(s, 60))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
        ]));
    }

    if let Some(file) = ui.session_transcript_file.as_deref() {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("file")), Style::default().fg(p.muted)),
            Span::styled(shorten_middle(file, 120), Style::default().fg(p.muted)),
        ]));
    }

    if let Some(err) = ui.session_transcript_error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("{}: {err}", l("error")),
            Style::default().fg(p.bad),
        )));
    }

    lines.push(Line::from(""));

    if ui.session_transcript_messages.is_empty() {
        lines.push(Line::from(Span::styled(
            i18n::text(lang, msg::NO_TRANSCRIPT_MESSAGES),
            Style::default().fg(p.muted),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("messages")), Style::default().fg(p.muted)),
            Span::styled(
                ui.session_transcript_messages.len().to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(""));
        for msg in ui.session_transcript_messages.iter() {
            let role_style = if msg.role.eq_ignore_ascii_case("Assistant") {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.text).add_modifier(Modifier::BOLD)
            };
            let head = if let Some(ts) = msg.timestamp.as_deref() {
                format!("[{}] {}", ts, msg.role)
            } else {
                msg.role.clone()
            };

            lines.push(Line::from(Span::styled(head, role_style)));
            for line in msg.text.lines() {
                lines.push(Line::from(Span::raw(format!("  {line}"))));
            }
            lines.push(Line::from(""));
        }
    }

    let inner_h = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_h).min(u16::MAX as usize) as u16;
    ui.session_transcript_scroll = ui.session_transcript_scroll.min(max_scroll);

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .scroll((ui.session_transcript_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

pub(super) fn render_effort_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(50, 55, f.area());
    f.render_widget(Clear, area);
    let focused = ui.overlay == Overlay::EffortMenu;
    let block = Block::default()
        .title(Span::styled(
            i18n::label(ui.language, "Set reasoning effort"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { p.focus } else { p.border }))
        .style(Style::default().bg(p.panel));

    let choices = [
        EffortChoice::Clear,
        EffortChoice::Low,
        EffortChoice::Medium,
        EffortChoice::High,
        EffortChoice::XHigh,
    ];
    let items = choices
        .iter()
        .map(|c| ListItem::new(Line::from(c.label(ui.language))))
        .collect::<Vec<_>>();

    ui.menu_list.select(Some(
        ui.effort_menu_idx.min(choices.len().saturating_sub(1)),
    ));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

pub(super) fn render_model_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(68, 64, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_SET_SESSION_MODEL),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut items = Vec::with_capacity(ui.session_model_options.len().saturating_add(1));
    items.push(ListItem::new(Text::from(vec![
        Line::from(i18n::text(ui.language, msg::CLEAR_MODEL_OVERRIDE)),
        Line::from(Span::styled(
            i18n::text(ui.language, msg::RESTORE_DEFAULT_ROUTING),
            Style::default().fg(p.muted),
        )),
    ])));
    items.extend(ui.session_model_options.iter().map(|model| {
        ListItem::new(Text::from(vec![
            Line::from(shorten_middle(model, 56)),
            Line::from(Span::styled(
                i18n::text(ui.language, msg::APPLY_SESSION_MODEL_OVERRIDE),
                Style::default().fg(p.muted),
            )),
        ]))
    }));
    items.push(ListItem::new(Text::from(vec![
        Line::from(i18n::text(ui.language, msg::CUSTOM_MODEL)),
        Line::from(Span::styled(
            i18n::text(ui.language, msg::CUSTOM_MODEL_HELP),
            Style::default().fg(p.muted),
        )),
    ])));

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.model_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

pub(super) fn render_model_input_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(72, 36, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_INPUT_SESSION_MODEL),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let current = ui.session_model_input.trim();
    let current = if current.is_empty() {
        "<empty>"
    } else {
        current
    };
    let hint = ui.session_model_input_hint.as_deref().unwrap_or("-");

    let lines = vec![
        Line::from(vec![
            Span::styled(
                i18n::text(ui.language, msg::CURRENT_INPUT_LABEL),
                Style::default().fg(p.muted),
            ),
            Span::styled(current.to_string(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(
                i18n::text(ui.language, msg::SESSION_MODEL_HINT_LABEL),
                Style::default().fg(p.muted),
            ),
            Span::styled(shorten_middle(hint, 56), Style::default().fg(p.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            i18n::text(ui.language, msg::MODEL_INPUT_HELP),
            Style::default().fg(p.muted),
        )),
    ];

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

pub(super) fn render_service_tier_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(58, 52, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_SET_SERVICE_TIER),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let choices = [
        ServiceTierChoice::Clear,
        ServiceTierChoice::Default,
        ServiceTierChoice::Priority,
        ServiceTierChoice::Flex,
    ];
    let items = choices
        .iter()
        .map(|choice| {
            let detail = match choice {
                ServiceTierChoice::Clear => {
                    i18n::text(ui.language, msg::CLEAR_SERVICE_TIER_OVERRIDE)
                }
                ServiceTierChoice::Default => {
                    i18n::text(ui.language, msg::USE_DEFAULT_SERVICE_TIER)
                }
                ServiceTierChoice::Priority => {
                    i18n::text(ui.language, msg::USE_PRIORITY_SERVICE_TIER)
                }
                ServiceTierChoice::Flex => i18n::text(ui.language, msg::USE_FLEX_SERVICE_TIER),
            };
            ListItem::new(Text::from(vec![
                Line::from(choice.label(ui.language)),
                Line::from(Span::styled(detail, Style::default().fg(p.muted))),
            ]))
        })
        .chain(std::iter::once(ListItem::new(Text::from(vec![
            Line::from(i18n::text(ui.language, msg::CUSTOM_SERVICE_TIER)),
            Line::from(Span::styled(
                i18n::text(ui.language, msg::CUSTOM_SERVICE_TIER_HELP),
                Style::default().fg(p.muted),
            )),
        ]))))
        .collect::<Vec<_>>();

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.service_tier_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

pub(super) fn render_service_tier_input_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(72, 36, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_INPUT_SERVICE_TIER),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let current = ui.session_service_tier_input.trim();
    let current = if current.is_empty() {
        "<empty>"
    } else {
        current
    };
    let hint = ui.session_service_tier_input_hint.as_deref().unwrap_or("-");

    let lines = vec![
        Line::from(vec![
            Span::styled(
                i18n::text(ui.language, msg::CURRENT_INPUT_LABEL),
                Style::default().fg(p.muted),
            ),
            Span::styled(current.to_string(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(
                i18n::text(ui.language, msg::SESSION_TIER_HINT_LABEL),
                Style::default().fg(p.muted),
            ),
            Span::styled(shorten_middle(hint, 56), Style::default().fg(p.accent)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            i18n::text(ui.language, msg::SERVICE_TIER_INPUT_HELP),
            Style::default().fg(p.muted),
        )),
    ];

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

pub(super) fn render_profile_modal_v2(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(82, 72, f.area());
    f.render_widget(Clear, area);
    let (title, clear_title, clear_detail) = match ui.overlay {
        Overlay::ProfileMenuDefaultRuntime => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_RUNTIME_PROFILE),
            i18n::text(ui.language, msg::CLEAR_RUNTIME_PROFILE),
            i18n::text(ui.language, msg::CLEAR_RUNTIME_PROFILE_HELP),
        ),
        Overlay::ProfileMenuDefaultPersisted => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_CONFIGURED_PROFILE),
            i18n::text(ui.language, msg::CLEAR_CONFIGURED_PROFILE),
            i18n::text(ui.language, msg::CLEAR_CONFIGURED_PROFILE_HELP),
        ),
        _ => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_SESSION_PROFILE),
            i18n::text(ui.language, msg::CLEAR_SESSION_PROFILE_BINDING),
            i18n::text(ui.language, msg::CLEAR_SESSION_PROFILE_BINDING_HELP),
        ),
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut items = Vec::with_capacity(ui.profile_options.len().saturating_add(1));
    items.push(ListItem::new(Text::from(vec![
        Line::from(clear_title),
        Line::from(Span::styled(clear_detail, Style::default().fg(p.muted))),
    ])));
    items.extend(ui.profile_options.iter().map(|profile| {
        let mut label = profile.name.clone();
        let is_configured_default =
            ui.configured_default_profile.as_deref() == Some(profile.name.as_str());
        let is_runtime_override =
            ui.runtime_default_profile_override.as_deref() == Some(profile.name.as_str());
        let is_effective_default =
            ui.effective_default_profile.as_deref() == Some(profile.name.as_str());
        match ui.overlay {
            Overlay::ProfileMenuDefaultRuntime => {
                if is_runtime_override {
                    label.push_str(" *runtime");
                } else if is_effective_default {
                    label.push_str(" *effective");
                }
            }
            Overlay::ProfileMenuDefaultPersisted => {
                if is_configured_default && is_effective_default {
                    label.push_str(" *configured/effective");
                } else if is_configured_default {
                    label.push_str(" *configured");
                } else if is_effective_default {
                    label.push_str(" *effective");
                }
            }
            _ => {
                if profile.is_default {
                    label.push_str(" *default");
                }
            }
        }
        let declared = profile_declared_summary(profile, ui.language);
        let (resolved, resolve_failed) =
            profile_resolved_summary(profile.name.as_str(), &ui.profile_options, ui.language);
        ListItem::new(Text::from(vec![
            Line::from(label),
            Line::from(Span::styled(declared, Style::default().fg(p.muted))),
            Line::from(Span::styled(
                resolved,
                Style::default().fg(if resolve_failed { p.bad } else { p.accent }),
            )),
        ]))
    }));

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.profile_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

pub(super) fn render_provider_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    title: &str,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let area = centered_rect(60, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));
    let inner_width = usize::from(block.inner(area).width);
    let balance_prefix = format!("{}: ", l("balance/quota"));
    let tags_prefix = format!("{}: ", l("tags"));
    let balance_width = modal_value_width(inner_width, &balance_prefix);
    let tags_width = modal_value_width(inner_width, &tags_prefix);

    let mut items = Vec::with_capacity(providers.len() + 1);
    items.push(ListItem::new(Line::from(format!(
        "({})",
        l("Clear override")
    ))));
    for pvd in providers {
        let mut label = format!("L{} {}", pvd.level.clamp(1, 10), pvd.name);
        if pvd.active {
            label.push_str(" *");
        }
        if !pvd.enabled {
            label.push_str(" [off]");
        }
        if let Some(alias) = pvd.alias.as_deref()
            && !alias.trim().is_empty()
            && alias != pvd.name
        {
            label.push_str(&format!(" ({alias})"));
        }
        let balance = station_balance_brief_lang(
            &snapshot.provider_balances,
            pvd.name.as_str(),
            balance_width,
            lang,
        );
        let balance_style = if pvd.enabled {
            station_primary_balance_snapshot(&snapshot.provider_balances, pvd.name.as_str())
                .map(|snapshot| balance_snapshot_status_style(p, snapshot))
                .unwrap_or_else(|| Style::default().fg(p.muted))
        } else {
            Style::default().fg(p.muted)
        };
        let tags = provider_tags_brief(pvd, tags_width).unwrap_or_else(|| "-".to_string());
        let style = Style::default().fg(if pvd.enabled { p.text } else { p.muted });
        items.push(
            ListItem::new(Text::from(vec![
                Line::from(Span::styled(label, style)),
                Line::from(vec![
                    Span::styled(balance_prefix.clone(), Style::default().fg(p.muted)),
                    Span::styled(balance, balance_style),
                ]),
                Line::from(vec![
                    Span::styled(tags_prefix.clone(), Style::default().fg(p.muted)),
                    Span::styled(tags, Style::default().fg(p.muted)),
                ]),
                Line::from(vec![Span::styled(
                    format!("upstreams={}", pvd.upstreams.len()),
                    Style::default().fg(p.muted),
                )]),
            ]))
            .style(style),
        );
    }

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.provider_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

fn routing_policy_label(policy: crate::config::RoutingPolicyV4) -> &'static str {
    match policy {
        crate::config::RoutingPolicyV4::ManualSticky => "manual-sticky",
        crate::config::RoutingPolicyV4::OrderedFailover => "ordered-failover",
        crate::config::RoutingPolicyV4::TagPreferred => "tag-preferred",
        crate::config::RoutingPolicyV4::Conditional => "conditional",
    }
}

fn routing_exhausted_label(action: crate::config::RoutingExhaustedActionV4) -> &'static str {
    match action {
        crate::config::RoutingExhaustedActionV4::Continue => "continue",
        crate::config::RoutingExhaustedActionV4::Stop => "stop",
    }
}

fn routing_tags_label(tags: &BTreeMap<String, String>, max_width: usize) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    let parts = tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    shorten_middle(&parts.join(" "), max_width)
}

fn routing_prefer_tags_label(filters: &[BTreeMap<String, String>], max_width: usize) -> String {
    if filters.is_empty() {
        return "-".to_string();
    }
    let parts = filters
        .iter()
        .map(|tags| routing_tags_label(tags, max_width))
        .collect::<Vec<_>>();
    shorten_middle(&parts.join(" OR "), max_width)
}

fn modal_value_width(inner_width: usize, prefix: &str) -> usize {
    inner_width
        .saturating_sub(UnicodeWidthStr::width(prefix))
        .saturating_sub(2)
        .clamp(24, 72)
}

fn routing_provider_balance_line<'a>(
    snapshot: &'a Snapshot,
    provider_name: &str,
    lang: Language,
) -> Option<(&'a crate::state::ProviderBalanceSnapshot, String)> {
    let mut matches = snapshot
        .provider_balances
        .iter()
        .flat_map(|(key, balances)| {
            balances.iter().filter_map(move |balance| {
                if balance.provider_id == provider_name
                    || (balance.provider_id.trim().is_empty() && key == provider_name)
                {
                    Some((
                        routing_context_balance_rank(key, balance, provider_name),
                        balance,
                    ))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.upstream_index.cmp(&right.1.upstream_index))
            .then_with(|| right.1.fetched_at_ms.cmp(&left.1.fetched_at_ms))
    });
    let (_, balance) = matches.into_iter().next()?;
    Some((balance, provider_balance_compact_lang(balance, 38, lang)))
}

pub(super) fn render_routing_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let area = centered_rect(76, 78, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            l("Routing"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let Some(spec) = ui.routing_spec.as_ref() else {
        let text = Text::from(vec![
            Line::from(l("routing spec not loaded")),
            Line::from(Span::styled(
                match lang {
                    Language::Zh => "g 刷新   Esc 关闭",
                    Language::En => "g refresh   Esc close",
                },
                Style::default().fg(p.muted),
            )),
        ]);
        f.render_widget(
            Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(p.text))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let order = {
        let mut order = if spec.order.is_empty() {
            spec.providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect::<Vec<_>>()
        } else {
            spec.order.clone()
        };
        for provider in &spec.providers {
            if !order.iter().any(|name| name == &provider.name) {
                order.push(provider.name.clone());
            }
        }
        order
    };
    let provider_by_name = spec
        .providers
        .iter()
        .map(|provider| (provider.name.as_str(), provider))
        .collect::<BTreeMap<_, _>>();

    let mut items = Vec::new();
    items.push(ListItem::new(Text::from(vec![
        Line::from(vec![
            Span::styled(format!("{}: ", l("policy")), Style::default().fg(p.muted)),
            Span::styled(routing_policy_label(spec.policy), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("target")), Style::default().fg(p.muted)),
            Span::styled(
                spec.target.as_deref().unwrap_or("-"),
                Style::default().fg(if spec.target.is_some() { p.accent } else { p.muted }),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("on_exhausted")), Style::default().fg(p.muted)),
            Span::styled(routing_exhausted_label(spec.on_exhausted), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(format!("{}: ", l("prefer_tags")), Style::default().fg(p.muted)),
            Span::styled(
                routing_prefer_tags_label(&spec.prefer_tags, 72),
                Style::default().fg(if spec.prefer_tags.is_empty() {
                    p.muted
                } else {
                    p.accent
                }),
            ),
        ]),
        Line::from(Span::styled(
            match lang {
                Language::Zh => {
                    "Enter pin  a 顺序  f monthly 优先  e 启用/禁用  s stop/continue  [/]/u/d 排序  1 monthly  2 paygo  0 清除 billing  g 刷新  Esc 关闭"
                }
                Language::En => {
                    "Enter pin  a ordered  f monthly-first  e enable/disable  s stop/continue  [/]/u/d reorder  1 monthly  2 paygo  0 clear billing  g refresh  Esc close"
                }
            },
            Style::default().fg(p.muted),
        )),
    ])));

    for (idx, name) in order.iter().enumerate() {
        let provider = provider_by_name.get(name.as_str()).copied();
        let enabled = provider.map(|provider| provider.enabled).unwrap_or(false);
        let tags = provider
            .map(|provider| routing_tags_label(&provider.tags, 42))
            .unwrap_or_else(|| "-".to_string());
        let alias = provider
            .and_then(|provider| provider.alias.as_deref())
            .filter(|alias| !alias.trim().is_empty() && *alias != name)
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        let marker = if spec.target.as_deref() == Some(name.as_str()) {
            "PIN"
        } else if matches!(spec.policy, crate::config::RoutingPolicyV4::TagPreferred)
            && provider.is_some_and(|provider| {
                spec.prefer_tags.iter().any(|filter| {
                    !filter.is_empty()
                        && filter
                            .iter()
                            .all(|(key, value)| provider.tags.get(key) == Some(value))
                })
            })
        {
            "PREF"
        } else {
            "    "
        };
        let (balance_style, balance_text) =
            if let Some((balance, text)) = routing_provider_balance_line(snapshot, name, lang) {
                (balance_snapshot_status_style(p, balance), text)
            } else {
                (Style::default().fg(p.muted), "-".to_string())
            };

        let mut title_style = Style::default().fg(if enabled { p.text } else { p.muted });
        if spec.target.as_deref() == Some(name.as_str()) {
            title_style = title_style.fg(p.accent).add_modifier(Modifier::BOLD);
        }
        items.push(
            ListItem::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                    Span::styled(marker, Style::default().fg(p.accent)),
                    Span::raw("  "),
                    Span::styled(format!("{name}{alias}"), title_style),
                    if enabled {
                        Span::raw("")
                    } else {
                        Span::styled(" [off]", Style::default().fg(p.warn))
                    },
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("     {}: ", l("balance/quota")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(balance_text, balance_style),
                ]),
                Line::from(vec![
                    Span::styled(format!("{}: ", l("tags")), Style::default().fg(p.muted)),
                    Span::styled(tags, Style::default().fg(p.muted)),
                ]),
            ]))
            .style(Style::default().fg(if enabled { p.text } else { p.muted })),
        );
    }

    let selected = if order.is_empty() {
        0
    } else {
        ui.routing_menu_idx.min(order.len().saturating_sub(1)) + 1
    };
    ui.menu_list.select(Some(selected));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

#[cfg(test)]
mod tests {
    use super::{
        current_page_help_lines, help_text_for_tests, profile_declared_summary,
        profile_resolved_summary,
    };
    use crate::dashboard_core::ControlProfileOption;
    use crate::tui::Language;
    use crate::tui::model::Palette;
    use crate::tui::types::Page;

    fn make_profile(name: &str) -> ControlProfileOption {
        ControlProfileOption {
            name: name.to_string(),
            extends: None,
            station: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            fast_mode: false,
            is_default: false,
        }
    }

    #[test]
    fn profile_declared_summary_includes_extends_and_auto_defaults() {
        let mut profile = make_profile("fast");
        profile.extends = Some("base".to_string());
        profile.reasoning_effort = Some("low".to_string());

        let summary = profile_declared_summary(&profile, Language::En);

        assert!(summary.contains("declared:"));
        assert!(summary.contains("extends=base"));
        assert!(summary.contains("reasoning=low"));
        assert!(summary.contains("tier=<auto>"));
    }

    #[test]
    fn profile_resolved_summary_uses_inherited_values() {
        let mut base = make_profile("base");
        base.model = Some("gpt-5.4".to_string());
        base.service_tier = Some("priority".to_string());

        let mut fast = make_profile("fast");
        fast.extends = Some("base".to_string());
        fast.reasoning_effort = Some("low".to_string());

        let (summary, failed) = profile_resolved_summary("fast", &[base, fast], Language::En);

        assert!(!failed);
        assert!(summary.contains("resolved:"));
        assert!(summary.contains("model=gpt-5.4"));
        assert!(summary.contains("reasoning=low"));
        assert!(summary.contains("tier=priority"));
    }

    #[test]
    fn profile_resolved_summary_reports_cycle_error() {
        let mut alpha = make_profile("alpha");
        alpha.extends = Some("beta".to_string());

        let mut beta = make_profile("beta");
        beta.extends = Some("alpha".to_string());

        let (summary, failed) = profile_resolved_summary("alpha", &[alpha, beta], Language::En);

        assert!(failed);
        assert!(summary.contains("resolve failed:"));
        assert!(summary.contains("profile inheritance cycle"));
    }

    #[test]
    fn routing_provider_balance_line_falls_back_for_legacy_snapshot_keys() {
        let snapshot = crate::tui::model::Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: std::collections::HashMap::new(),
            overrides: std::collections::HashMap::new(),
            station_overrides: std::collections::HashMap::new(),
            route_target_overrides: std::collections::HashMap::new(),
            service_tier_overrides: std::collections::HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: std::collections::HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: std::collections::HashMap::from([(
                "input".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: String::new(),
                    status: crate::state::BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("9.00".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            )]),
            station_health: std::collections::HashMap::new(),
            health_checks: std::collections::HashMap::new(),
            lb_view: std::collections::HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: std::time::Instant::now(),
        };

        let (_, text) = super::routing_provider_balance_line(&snapshot, "input", Language::En)
            .expect("legacy snapshot should still resolve");

        assert!(text.contains("$9.00"), "{text}");
    }

    #[test]
    fn routing_provider_balance_line_prefers_routing_context() {
        let snapshot = crate::tui::model::Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: std::collections::HashMap::new(),
            overrides: std::collections::HashMap::new(),
            station_overrides: std::collections::HashMap::new(),
            route_target_overrides: std::collections::HashMap::new(),
            service_tier_overrides: std::collections::HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: std::collections::HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: std::collections::HashMap::from([
                (
                    "input6".to_string(),
                    vec![crate::state::ProviderBalanceSnapshot {
                        provider_id: "input6".to_string(),
                        station_name: Some("input6".to_string()),
                        upstream_index: Some(0),
                        status: crate::state::BalanceSnapshotStatus::Ok,
                        total_balance_usd: Some("99.00".to_string()),
                        fetched_at_ms: 2_000,
                        ..crate::state::ProviderBalanceSnapshot::default()
                    }],
                ),
                (
                    "routing".to_string(),
                    vec![crate::state::ProviderBalanceSnapshot {
                        provider_id: "input6".to_string(),
                        station_name: Some("routing".to_string()),
                        upstream_index: Some(6),
                        status: crate::state::BalanceSnapshotStatus::Exhausted,
                        exhausted: Some(true),
                        exhaustion_affects_routing: false,
                        quota_period: Some("daily".to_string()),
                        quota_remaining_usd: Some("0".to_string()),
                        quota_limit_usd: Some("300".to_string()),
                        fetched_at_ms: 1_000,
                        ..crate::state::ProviderBalanceSnapshot::default()
                    }],
                ),
            ]),
            station_health: std::collections::HashMap::new(),
            health_checks: std::collections::HashMap::new(),
            lb_view: std::collections::HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
            refreshed_at: std::time::Instant::now(),
        };

        let (balance, text) =
            super::routing_provider_balance_line(&snapshot, "input6", Language::En)
                .expect("routing snapshot should resolve");

        assert_eq!(balance.station_name.as_deref(), Some("routing"));
        assert!(text.contains("$0") && text.contains("$300.00"), "{text}");
        assert!(!text.contains("$99.00"), "{text}");
    }

    #[test]
    fn current_page_help_includes_hidden_routing_actions() {
        let lines =
            current_page_help_lines(Language::En, Page::Stations, true, true, Palette::default());
        let text = help_text_for_tests(&lines);

        assert!(text.contains("Current page: Routing"), "{text}");
        assert!(text.contains("1/2/0"), "{text}");
        assert!(text.contains("Backspace"), "{text}");
        assert!(text.contains("[]/u/d"), "{text}");
    }

    #[test]
    fn current_page_help_includes_usage_detail_actions() {
        let lines =
            current_page_help_lines(Language::En, Page::Stats, true, true, Palette::default());
        let text = help_text_for_tests(&lines);

        assert!(text.contains("Current page: Providers"), "{text}");
        assert!(text.contains("PgUp/PgDn"), "{text}");
        assert!(text.contains("refresh provider balances"), "{text}");
    }
}
