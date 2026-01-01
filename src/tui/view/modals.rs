use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, compute_window_stats, now_ms, short_sid, shorten, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::types::{EffortChoice, Overlay};

use super::widgets::centered_rect;

pub(super) fn render_config_info_modal(
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
        .and_then(|r| r.override_config_name.as_deref());
    let global_override = snapshot.global_override.as_deref();

    let selected = providers.get(ui.selected_config_idx);
    let title = if let Some(cfg) = selected {
        let level = cfg.level.clamp(1, 10);
        format!(
            "{}: {} (L{})",
            crate::tui::i18n::pick(ui.language, "配置详情", "Config details"),
            cfg.name,
            level
        )
    } else {
        crate::tui::i18n::pick(ui.language, "配置详情", "Config details").to_string()
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
            crate::tui::i18n::pick(ui.language, "会话：", "session: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(short_sid(selected_session, 28), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "固定：", "pinned: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            if let Some(s) = session_override {
                format!("session={s}")
            } else if let Some(g) = global_override {
                format!("global={g}")
            } else {
                "-".to_string()
            },
            Style::default().fg(if session_override.is_some() || global_override.is_some() {
                p.accent
            } else {
                p.muted
            }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "按键：", "keys: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言",
                "↑/↓ scroll  PgUp/PgDn page  Esc close  L language",
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(cfg) = selected {
        let now = now_ms();

        let stats_5m_cfg = compute_window_stats(&snapshot.recent, now, 5 * 60_000, |r| {
            r.config_name.as_deref() == Some(cfg.name.as_str())
        });
        let stats_1h_cfg = compute_window_stats(&snapshot.recent, now, 60 * 60_000, |r| {
            r.config_name.as_deref() == Some(cfg.name.as_str())
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
            .config_meta_overrides
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
                    crate::tui::i18n::pick(ui.language, "别名：", "alias: "),
                    Style::default().fg(p.muted),
                ),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(
                crate::tui::i18n::pick(ui.language, "状态：", "status: "),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                crate::tui::i18n::pick(
                    ui.language,
                    if enabled { "启用" } else { "禁用" },
                    if enabled { "enabled" } else { "disabled" },
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
                crate::tui::i18n::pick(
                    ui.language,
                    if cfg.active { "active" } else { "" },
                    if cfg.active { "active" } else { "" },
                ),
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));
        lines.push(Line::from(""));

        lines.push(Line::from(vec![Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "运行态（可用性/体验）",
                "Runtime (availability/UX)",
            ),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![
            Span::styled("5m ", Style::default().fg(p.muted)),
            Span::styled(
                crate::tui::i18n::pick(ui.language, "成功 ", "ok "),
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
                crate::tui::i18n::pick(ui.language, "成功 ", "ok "),
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
            crate::tui::i18n::pick(ui.language, "上游（Providers）", "Upstreams (providers)"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));

        let health = snapshot.config_health.get(cfg.name.as_str());
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
                let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
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
                if r.config_name.as_deref() != Some(cfg.name.as_str()) {
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
                crate::tui::i18n::pick(ui.language, "（无）", "(none)"),
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
                    crate::tui::i18n::pick(ui.language, "未检查", "not checked").to_string()
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
                    crate::tui::i18n::pick(ui.language, "模型：全部", "models: all").to_string()
                } else {
                    let allow = up.supported_models.len();
                    let map = up.model_mapping.len();
                    crate::tui::i18n::pick(
                        ui.language,
                        &format!("模型：allow {allow} / map {map}"),
                        &format!("models: allow {allow} / map {map}"),
                    )
                    .to_string()
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
                    Span::styled("auth: ", Style::default().fg(p.muted)),
                    Span::styled(auth.to_string(), Style::default().fg(p.text)),
                    Span::raw("   "),
                    Span::styled(models_text, Style::default().fg(p.muted)),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled("health: ", Style::default().fg(p.muted)),
                    Span::styled(
                        health_text,
                        Style::default().fg(if ok == Some(true) { p.good } else { p.warn }),
                    ),
                    Span::raw("   "),
                    Span::styled("lb: ", Style::default().fg(p.muted)),
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
                    Span::styled("rt: ", Style::default().fg(p.muted)),
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
                        Span::styled("tags: ", Style::default().fg(p.muted)),
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
                        Span::styled("allow: ", Style::default().fg(p.muted)),
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
                        Span::styled("map: ", Style::default().fg(p.muted)),
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
            crate::tui::i18n::pick(ui.language, "未选中任何配置。", "No config selected."),
            Style::default().fg(p.muted),
        )));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height);
    ui.config_info_scroll = ui
        .config_info_scroll
        .min(max_scroll.min(u16::MAX as usize) as u16);

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false })
        .scroll((ui.config_info_scroll, 0));
    f.render_widget(content, area);
}

pub(super) fn render_help_modal(f: &mut Frame<'_>, p: Palette, lang: crate::tui::Language) {
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            crate::tui::i18n::pick(lang, "帮助", "Help"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let lines = if lang == crate::tui::Language::Zh {
        vec![
            Line::from(vec![Span::styled(
                "导航",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  ↑/↓, j/k   移动选中项"),
            Line::from("  1-6        切换页面"),
            Line::from("            1 总览  2 配置  3 会话  4 请求  5 统计  6 设置"),
            Line::from("  L          切换语言（中/英，自动落盘）"),
            Line::from("  Tab        切换焦点（总览页）"),
            Line::from("  6 设置     查看运行态与关键配置入口"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "推理强度",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      打开 effort 菜单（会话列表）"),
            Line::from("  l/m/h/X    设置 low/medium/high/xhigh"),
            Line::from("  x          清除 effort 覆盖"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Provider 覆盖",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  p          会话级 provider 覆盖（固定）"),
            Line::from("  P          全局 active provider（首选，可 failover）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "配置页（Configs）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      设置为全局 active config（同 level 可 failover）"),
            Line::from("  Backspace  清除 active（自动）"),
            Line::from("  o          设置会话 override 为当前 config"),
            Line::from("  O          清除会话 override"),
            Line::from("  i          查看 Provider 详情（可滚动）"),
            Line::from("  t          切换 enabled（热更新 + 落盘）"),
            Line::from("  +/-        调整 level（热更新 + 落盘）"),
            Line::from("  h/H        运行健康检查（当前/全部）"),
            Line::from("  c/C        取消健康检查（当前/全部）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "请求页（Requests）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  s          scope：全部 vs 当前会话"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "会话页（Sessions）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  a          仅看活跃（active-only）"),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  v          仅看覆盖（overrides-only）"),
            Line::from("  r          重置筛选"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "统计页（Stats）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        切换焦点（config vs provider）"),
            Line::from("  d          切换窗口（7/21/60 天）"),
            Line::from("  e          recent 仅看错误"),
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
            Line::from("  1-6        switch page"),
            Line::from(
                "            1 Dashboard  2 Configs  3 Sessions  4 Requests  5 Stats  6 Settings",
            ),
            Line::from("  L          toggle language (zh/en, persisted)"),
            Line::from("  6 Settings show runtime + config overview"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Effort",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      open effort menu (on Sessions)"),
            Line::from("  l/m/h/X    set low/medium/high/xhigh"),
            Line::from("  x          clear effort override"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Provider override",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  p          session provider override (pinned)"),
            Line::from("  P          global active provider (preferred, failover enabled)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Configs page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      set active config (same-level failover enabled)"),
            Line::from("  Backspace  clear active (auto)"),
            Line::from("  o          set session override to selected config"),
            Line::from("  O          clear session override"),
            Line::from("  i          open provider details (scrollable)"),
            Line::from("  t          toggle enabled (hot reload + saved)"),
            Line::from("  +/-        adjust level (hot reload + saved)"),
            Line::from("  h/H        run health checks (selected/all)"),
            Line::from("  c/C        cancel health checks (selected/all)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Requests page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  e          toggle errors-only filter"),
            Line::from("  s          toggle scope (all vs selected session)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Sessions page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  a          toggle active-only"),
            Line::from("  e          toggle errors-only"),
            Line::from("  v          toggle overrides-only"),
            Line::from("  r          reset filters"),
            Line::from("  t          view transcript (Codex)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Stats page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        switch focus (config vs provider)"),
            Line::from("  d          cycle time window (7/21/60 days)"),
            Line::from("  e          toggle errors-only (recent breakdown)"),
            Line::from("  y          copy + export report (selected item)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Quit",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  q          quit and request shutdown"),
            Line::from("  Esc/?      close this modal"),
        ]
    };

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

pub(super) fn render_session_transcript_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(92, 90, f.area());
    f.render_widget(Clear, area);

    let sid = ui.selected_session_id.as_deref().unwrap_or("-");
    let title = format!(
        "{}: {}",
        crate::tui::i18n::pick(ui.language, "会话对话记录", "Session transcript"),
        short_sid(sid, 28)
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
        Span::styled("sid: ", Style::default().fg(p.muted)),
        Span::styled(short_sid(sid, 36), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "按键：", "keys: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "↑/↓ 滚动  PgUp/PgDn 翻页  g/G 顶/底  t/Esc 关闭  L 语言",
                "↑/↓ scroll  PgUp/PgDn page  g/G top/bottom  t/Esc close  L language",
            ),
            Style::default().fg(p.muted),
        ),
    ]));

    if let Some(meta) = ui.session_transcript_meta.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("meta: ", Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(meta.id.as_str(), 44),
                Style::default().fg(p.text),
            ),
            Span::raw("   "),
            Span::styled("cwd: ", Style::default().fg(p.muted)),
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
            Span::styled("file: ", Style::default().fg(p.muted)),
            Span::styled(shorten_middle(file, 120), Style::default().fg(p.muted)),
        ]));
    }

    if let Some(err) = ui.session_transcript_error.as_deref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("error: {err}"),
            Style::default().fg(p.bad),
        )));
    }

    lines.push(Line::from(""));

    if ui.session_transcript_messages.is_empty() {
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "未找到可展示的对话消息（可能该会话不在 ~/.codex/sessions，或格式发生变化）。",
                "No transcript messages found (session file missing or format changed).",
            ),
            Style::default().fg(p.muted),
        )));
    } else {
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
            "Set reasoning effort",
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
        .map(|c| ListItem::new(Line::from(c.label())))
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

pub(super) fn render_provider_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    providers: &[ProviderOption],
    title: &str,
) {
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

    let mut items = Vec::with_capacity(providers.len() + 1);
    items.push(ListItem::new(Line::from("(Clear override)")));
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
        let style = Style::default().fg(if pvd.enabled { p.text } else { p.muted });
        items.push(ListItem::new(Line::from(label)).style(style));
    }

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.provider_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}
