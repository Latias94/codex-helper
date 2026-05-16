use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, compute_window_stats, now_ms,
    provider_tags_brief, shorten, shorten_middle, station_balance_brief_lang,
    station_primary_balance_snapshot,
};
use crate::tui::state::UiState;
use crate::tui::types::{EffortChoice, Overlay, ServiceTierChoice};

use super::widgets::centered_rect;

mod help;
pub(super) use help::render_help_modal;
#[cfg(test)]
use help::{current_page_help_lines, help_text_for_tests};

mod profile;
pub(super) use profile::render_profile_modal_v2;
#[cfg(test)]
use profile::{profile_declared_summary, profile_resolved_summary};

mod routing;
pub(super) use routing::render_routing_modal;
#[cfg(test)]
use routing::routing_provider_balance_line;

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

fn modal_value_width(inner_width: usize, prefix: &str) -> usize {
    inner_width
        .saturating_sub(UnicodeWidthStr::width(prefix))
        .saturating_sub(2)
        .clamp(24, 72)
}

#[cfg(test)]
mod tests;
