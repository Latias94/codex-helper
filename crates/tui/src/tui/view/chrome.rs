use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::model::{Palette, Snapshot, shorten_middle};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay, Page, page_index, page_titles};

fn push_header_sep(spans: &mut Vec<Span<'static>>, compact: bool) {
    spans.push(Span::raw(if compact { "  " } else { "   " }));
}

fn push_header_metric(
    spans: &mut Vec<Span<'static>>,
    label: impl Into<String>,
    value: impl Into<String>,
    label_style: Style,
    value_style: Style,
) {
    spans.push(Span::styled(label.into(), label_style));
    spans.push(Span::styled(value.into(), value_style));
}

fn text_prefix_by_width(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > max_width {
            break;
        }
        out.push(ch);
        width = width.saturating_add(ch_width);
    }
    out
}

fn truncate_text_to_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    format!(
        "{}…",
        text_prefix_by_width(text, max_width.saturating_sub(1))
    )
}

fn fit_spans_to_width(spans: Vec<Span<'static>>, max_width: u16) -> Line<'static> {
    let max_width = usize::from(max_width);
    if max_width == 0 {
        return Line::from("");
    }

    let mut used = 0usize;
    let mut fitted = Vec::new();
    for span in spans {
        let width = UnicodeWidthStr::width(span.content.as_ref());
        if used.saturating_add(width) <= max_width {
            used = used.saturating_add(width);
            fitted.push(span);
            continue;
        }

        let remaining = max_width.saturating_sub(used);
        if remaining > 0 {
            let style = span.style;
            fitted.push(Span::styled(
                truncate_text_to_width(span.content.as_ref(), remaining),
                style,
            ));
        }
        break;
    }
    Line::from(fitted)
}

fn split_footer_help(text: &str, max_width: u16) -> (String, String) {
    let max_width = usize::from(max_width);
    if max_width == 0 {
        return (String::new(), String::new());
    }

    let parts = text
        .split("  ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return (String::new(), String::new());
    }

    let mut first = String::new();
    let mut split_at = parts.len();
    for (idx, part) in parts.iter().enumerate() {
        let candidate = if first.is_empty() {
            (*part).to_string()
        } else {
            format!("{first}  {part}")
        };
        if UnicodeWidthStr::width(candidate.as_str()) <= max_width || first.is_empty() {
            first = candidate;
        } else {
            split_at = idx;
            break;
        }
    }

    let second = if split_at < parts.len() {
        parts[split_at..].join("  ")
    } else {
        String::new()
    };
    (first, second)
}

fn fit_line_to_width(line: Line<'static>, max_width: u16) -> Line<'static> {
    fit_spans_to_width(line.spans, max_width)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn tab_style(p: Palette, selected: bool) -> Style {
    if selected {
        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.muted)
    }
}

fn header_tabs_line(p: Palette, ui: &UiState, max_width: u16) -> Line<'static> {
    let selected = page_index(ui.page);
    let titles = page_titles(ui.language, ui.uses_v3_routing());

    let mut full = Vec::new();
    for (idx, title) in titles.iter().enumerate() {
        if idx > 0 {
            full.push(Span::raw("  "));
        }
        full.push(Span::styled(
            (*title).to_string(),
            tab_style(p, idx == selected),
        ));
    }
    if spans_width(&full) <= usize::from(max_width) {
        return Line::from(full);
    }

    let mut compact = Vec::new();
    for (idx, title) in titles.iter().enumerate() {
        if idx > 0 {
            compact.push(Span::raw(" "));
        }
        let label = if idx == selected {
            (*title).to_string()
        } else {
            (idx + 1).to_string()
        };
        compact.push(Span::styled(label, tab_style(p, idx == selected)));
    }
    if spans_width(&compact) <= usize::from(max_width) {
        return Line::from(compact);
    }

    fit_spans_to_width(
        vec![Span::styled(
            titles[selected].to_string(),
            tab_style(p, true),
        )],
        max_width,
    )
}

pub(super) fn render_header(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    service_name: &'static str,
    port: u16,
    area: Rect,
) {
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let inner = content_area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let active_total = snapshot.rows.iter().map(|r| r.active_count).sum::<usize>();
    let recent_err = snapshot
        .recent
        .iter()
        .take(80)
        .filter(|r| r.status_code >= 400)
        .count();
    let updated = snapshot.refreshed_at.elapsed().as_millis();
    let overrides_model = snapshot.model_overrides.len();
    let overrides_effort = snapshot.overrides.len();
    let overrides_station = snapshot.station_overrides.len();
    let overrides_tier = snapshot.service_tier_overrides.len();
    let (hc_running, hc_canceling) = {
        let mut running = 0usize;
        let mut canceling = 0usize;
        for st in snapshot.health_checks.values() {
            if !st.done {
                running += 1;
                if st.cancel_requested {
                    canceling += 1;
                }
            }
        }
        (running, canceling)
    };

    let global_station = snapshot
        .global_station_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let focus = match ui.focus {
        Focus::Sessions => crate::tui::i18n::pick(ui.language, "会话", "Sessions"),
        Focus::Requests => crate::tui::i18n::pick(ui.language, "请求", "Requests"),
        Focus::Stations if ui.uses_v3_routing() => {
            crate::tui::i18n::pick(ui.language, "路由", "Routing")
        }
        Focus::Stations => crate::tui::i18n::pick(ui.language, "站点", "Stations"),
    };
    let title = if inner.width >= 72 {
        Line::from(vec![
            Span::styled(
                "codex-helper",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{service_name}:{port}"),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}{focus}",
                    crate::tui::i18n::pick(ui.language, "焦点：", "focus: ")
                ),
                Style::default().fg(p.muted),
            ),
        ])
    } else if inner.width >= 46 {
        Line::from(vec![
            Span::styled(
                "codex-helper",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{service_name}:{port}"),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled(focus.to_string(), Style::default().fg(p.muted)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "codex-helper",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(port.to_string(), Style::default().fg(p.muted)),
        ])
    };

    let last_req = snapshot.recent.first();
    let last_provider = last_req
        .and_then(|r| r.provider_id.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_station = last_req
        .and_then(|r| r.station_name.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_attempts = last_req.map(|r| r.attempt_count()).unwrap_or(1);

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

    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;

    let hc_text = if hc_running > 0 {
        if ui.language == crate::tui::Language::Zh {
            format!("运行:{hc_running} 取消:{hc_canceling}")
        } else {
            format!("run:{hc_running} cancel:{hc_canceling}")
        }
    } else {
        "-".to_string()
    };
    let route_full = format!("{last_provider}/{last_station}×{last_attempts}");
    let route_medium = format!(
        "{}/{}×{last_attempts}",
        shorten_middle(last_provider, 18),
        shorten_middle(last_station, 18)
    );
    let route_compact = format!("{}×{last_attempts}", shorten_middle(last_station, 18));
    let overrides_total = overrides_model
        .saturating_add(overrides_effort)
        .saturating_add(overrides_station)
        .saturating_add(overrides_tier);

    let mut subtitle_spans = Vec::new();
    if inner.width >= 150 {
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "活跃 ", "active "),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "错误(80) ", "errors(80) "),
            recent_err.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            "5m ",
            fmt_ok_pct(s5.ok_2xx, s5.total),
            Style::default().fg(p.muted),
            Style::default().fg(if s5.total > 0 && s5.ok_2xx == s5.total {
                p.good
            } else {
                p.muted
            }),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "p95 ",
            fmt_ms(s5.p95_ms),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "att ",
            fmt_attempts(s5.avg_attempts),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "429 ",
            s5.err_429.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if s5.err_429 > 0 { p.warn } else { p.muted }),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "5xx ",
            s5.err_5xx.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if s5.err_5xx > 0 { p.warn } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            "1h ",
            fmt_ok_pct(s1.ok_2xx, s1.total),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "p95 ",
            fmt_ms(s1.p95_ms),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        subtitle_spans.push(Span::raw(" "));
        push_header_metric(
            &mut subtitle_spans,
            "429 ",
            s1.err_429.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if s1.err_429 > 0 { p.warn } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "当前 ", "cur "),
            route_full,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "健康检查 ", "hc "),
            hc_text,
            Style::default().fg(p.muted),
            Style::default().fg(if hc_running > 0 { p.accent } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "覆盖(M/E/C/T) ", "overrides(M/E/C/T) "),
            format!("{overrides_model}/{overrides_effort}/{overrides_station}/{overrides_tier}"),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "覆盖(全局站点) ", "override(global station) "),
            global_station.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "刷新 ", "updated "),
            format!("{updated}ms"),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
    } else if inner.width >= 96 {
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "活 ", "act "),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "错 ", "err "),
            recent_err.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            "5m ",
            format!("{} {}", fmt_ok_pct(s5.ok_2xx, s5.total), fmt_ms(s5.p95_ms)),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "当前 ", "cur "),
            route_medium,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        if hc_running > 0 {
            push_header_sep(&mut subtitle_spans, true);
            push_header_metric(
                &mut subtitle_spans,
                "hc ",
                hc_text,
                Style::default().fg(p.muted),
                Style::default().fg(p.accent),
            );
        }
        if overrides_total > 0 {
            push_header_sep(&mut subtitle_spans, true);
            push_header_metric(
                &mut subtitle_spans,
                "ovr ",
                overrides_total.to_string(),
                Style::default().fg(p.muted),
                Style::default().fg(p.muted),
            );
        }
        if global_station != "-" {
            push_header_sep(&mut subtitle_spans, true);
            push_header_metric(
                &mut subtitle_spans,
                "global ",
                shorten_middle(global_station, 18),
                Style::default().fg(p.muted),
                Style::default().fg(p.accent),
            );
        }
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "刷 ", "upd "),
            format!("{updated}ms"),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
    } else {
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "活 ", "act "),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "错 ", "err "),
            recent_err.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            "5m ",
            fmt_ok_pct(s5.ok_2xx, s5.total),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            crate::tui::i18n::pick(ui.language, "当前 ", "cur "),
            route_compact,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        if inner.width >= 70 {
            push_header_sep(&mut subtitle_spans, true);
            push_header_metric(
                &mut subtitle_spans,
                crate::tui::i18n::pick(ui.language, "刷 ", "upd "),
                format!("{updated}ms"),
                Style::default().fg(p.muted),
                Style::default().fg(p.muted),
            );
        }
    }

    let title = fit_line_to_width(title, inner.width);
    let subtitle = fit_spans_to_width(subtitle_spans, inner.width);

    let tabs = header_tabs_line(p, ui, inner.width);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(p.border));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(Text::from(title)), chunks[0]);
    f.render_widget(Paragraph::new(Text::from(subtitle)), chunks[1]);
    f.render_widget(Paragraph::new(Text::from(tabs)), chunks[2]);
}

pub(super) fn render_footer(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let now = std::time::Instant::now();
    if let Some((_, ts)) = ui.toast.as_ref()
        && now.duration_since(*ts) > Duration::from_secs(3)
    {
        ui.toast = None;
    }

    let left = match ui.overlay {
        Overlay::None => match ui.page {
            Page::Dashboard => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 或 j/k 移动  b profile绑定  M model  f fast/tier  R 重置覆盖  Enter effort  l/m/h/X 设置  x 清除  p 会话站点  P 全局 pin  O/H(会话) o/h(请求) 跳转  ? 帮助",
                "1-8 pages  q quit  L language  Tab focus  ↑/↓ or j/k move  b profile binding  M model  f fast/tier  R reset overrides  Enter effort  l/m/h/X set  x clear  p session station  P global pin  O/H(session) o/h(request) jump  ? help",
            ),
            Page::Stations if ui.uses_v3_routing() => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ provider  r/Enter routing编辑  e 启停  f 包月优先  1/2/0 billing  s 耗尽策略  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ provider  r/Enter routing editor  e enable  f monthly-first  1/2/0 billing  s exhausted action  ? help",
            ),
            Page::Stations => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ 选择  r routing  i 详情  Enter 全局 pin  Backspace 清除  o 会话站点 override  O 清除  h/H 检查  c/C 取消  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ select  r routing  i details  Enter global pin  Backspace clear  o session station override  O clear  h/H check  c/C cancel  ? help",
            ),
            Page::Requests => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ 选择  e 仅看错误  s scope(会话/全部)  x 清除聚焦  o 打开到 Sessions  h 打开到 History  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ select  e errors_only  s scope(session/all)  x clear focus  o open Sessions  h open History  ? help",
            ),
            Page::Sessions => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ 选择  b profile绑定  M model  f fast/tier  R 重置覆盖  a 仅看活跃  e 仅看错误  v 仅看覆盖  r 重置  t 对话记录  o 打开到 Requests  H 打开到 History  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ select  b profile binding  M model  f fast/tier  R reset overrides  a active_only  e errors_only  v overrides_only  r reset  t transcript  o open Requests  H open History  ? help",
            ),
            Page::Stats => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  Tab 焦点(station/provider)  ↑/↓ 选择  d 天数(7/21/60)  e 仅看错误(recent)  y 复制+导出报告  ? 帮助",
                "1-8 pages  q quit  L language  Tab focus(station/provider)  ↑/↓ select  d days(7/21/60)  e errors_only(recent)  y copy+export report  ? help",
            ),
            Page::Settings => crate::tui::i18n::pick(
                ui.language,
                if ui.service_name == "codex" {
                    "1-8 页面  q 退出  L 语言  p 配置默认profile  P 运行时默认profile  R 重载配置  O 覆盖导入(~/.codex，二次确认)  ? 帮助"
                } else {
                    "1-8 页面  q 退出  L 语言  p 配置默认profile  P 运行时默认profile  R 重载配置  ? 帮助"
                },
                if ui.service_name == "codex" {
                    "1-8 pages  q quit  L language  p configured-default-profile  P runtime-default-profile  R reload  O overwrite(~/.codex, confirm)  ? help"
                } else {
                    "1-8 pages  q quit  L language  p configured-default-profile  P runtime-default-profile  R reload  ? help"
                },
            ),
            Page::History => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ 选择  r 刷新  t/Enter 对话记录  s 打开到 Sessions  f 打开到 Requests  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ select  r refresh  t/Enter transcript  s open Sessions  f open Requests  ? help",
            ),
            Page::Recent => crate::tui::i18n::pick(
                ui.language,
                "1-8 页面  q 退出  L 语言  ↑/↓ 选择  [] 切换时间  r 刷新  Enter 复制选中  y 复制全部(可见)  t transcript  s/f/h 跳转  ? 帮助",
                "1-8 pages  q quit  L language  ↑/↓ select  [] window  r refresh  Enter copy selected  y copy all(visible)  t transcript  s/f/h navigate  ? help",
            ),
        },
        Overlay::Help => crate::tui::i18n::pick(
            ui.language,
            "Esc 关闭帮助  L 语言",
            "Esc close help  L language",
        ),
        Overlay::EffortMenu => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter 应用  Esc 取消",
            "↑/↓ select  Enter apply  Esc cancel",
        ),
        Overlay::ModelMenuSession => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择 model  Enter 应用  Esc 取消",
            "↑/↓ select model  Enter apply  Esc cancel",
        ),
        Overlay::ModelInputSession => crate::tui::i18n::pick(
            ui.language,
            "输入 model  Enter 应用  Esc 返回菜单  Backspace 删除  Delete/Ctrl+U 清空",
            "type model  Enter apply  Esc back to menu  Backspace delete  Delete/Ctrl+U clear",
        ),
        Overlay::ServiceTierMenuSession => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择 service tier  Enter 应用  Esc 取消",
            "↑/↓ select service tier  Enter apply  Esc cancel",
        ),
        Overlay::ServiceTierInputSession => crate::tui::i18n::pick(
            ui.language,
            "输入 service_tier  Enter 应用  Esc 返回菜单  Backspace 删除  Delete/Ctrl+U 清空",
            "type service_tier  Enter apply  Esc back to menu  Backspace delete  Delete/Ctrl+U clear",
        ),
        Overlay::ProfileMenuSession => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择 profile 操作  Enter 应用/清除绑定  Esc 取消",
            "↑/↓ select profile action  Enter apply/clear binding  Esc cancel",
        ),
        Overlay::ProfileMenuDefaultRuntime => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择运行时默认 profile  Enter 应用/清除覆盖  Esc 取消",
            "↑/↓ select runtime default profile  Enter apply/clear override  Esc cancel",
        ),
        Overlay::ProfileMenuDefaultPersisted => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择配置默认 profile  Enter 应用/清除默认值  Esc 取消",
            "↑/↓ select configured default profile  Enter apply/clear default  Esc cancel",
        ),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter 应用  Esc 取消",
            "↑/↓ select  Enter apply  Esc cancel",
        ),
        Overlay::RoutingMenu => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter pin  a 顺序  f 包月优先  e 启停  s 耗尽策略  []/u/d 重排  1/2/0 billing tag  g 刷新  Esc 关闭",
            "↑/↓ select  Enter pin  a ordered  f monthly-first  e enable  s exhausted action  []/u/d reorder  1/2/0 billing tag  g refresh  Esc close",
        ),
        Overlay::StationInfo => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言",
            "↑/↓ scroll  PgUp/PgDn page  Esc close  L language",
        ),
        Overlay::SessionTranscript => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 滚动  PgUp/PgDn 翻页  g/G 顶/底  A 全量/尾部  y 复制  t/Esc 关闭  L 语言",
            "↑/↓ scroll  PgUp/PgDn page  g/G top/bottom  A all/tail  y copy  t/Esc close  L language",
        ),
    };
    let right = ui.toast.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");

    let (first, second) = split_footer_help(left, area.width);
    let first_line = fit_spans_to_width(
        vec![Span::styled(first, Style::default().fg(p.muted))],
        area.width,
    );
    let mut second_spans = Vec::new();
    if !right.is_empty() {
        second_spans.push(Span::styled(
            right.to_string(),
            Style::default().fg(p.accent),
        ));
        if !second.is_empty() {
            second_spans.push(Span::raw("  "));
        }
    }
    if !second.is_empty() {
        second_spans.push(Span::styled(second, Style::default().fg(p.muted)));
    }
    if second_spans.is_empty() {
        second_spans.push(Span::raw(""));
    }
    let second_line = fit_spans_to_width(second_spans, area.width);
    f.render_widget(
        Paragraph::new(Text::from(vec![first_line, second_line])),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_width(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn fit_spans_to_width_truncates_overlong_header_line() {
        let line = fit_spans_to_width(
            vec![
                Span::raw("active 12   "),
                Span::styled("current very-long-provider-name", Style::default()),
            ],
            18,
        );

        assert!(line_width(&line) <= 18);
        assert_eq!(line.spans.last().unwrap().content.as_ref(), "curre…");
    }

    #[test]
    fn fit_spans_to_width_handles_cjk_display_width() {
        let line = fit_spans_to_width(vec![Span::raw("状态 运行中 provider")], 8);

        assert!(line_width(&line) <= 8);
        assert_eq!(line.spans[0].content.as_ref(), "状态 运…");
    }

    #[test]
    fn split_footer_help_uses_second_line_for_overflow() {
        let (first, second) =
            split_footer_help("1-8 pages  q quit  L language  Tab focus  P global pin", 26);

        assert!(UnicodeWidthStr::width(first.as_str()) <= 26);
        assert!(!second.is_empty());
        assert!(second.contains("Tab focus"));
    }

    #[test]
    fn header_tabs_line_fits_available_width() {
        let ui = UiState {
            page: Page::Settings,
            language: crate::tui::Language::En,
            ..Default::default()
        };
        let line = header_tabs_line(Palette::default(), &ui, 24);

        assert!(line_width(&line) <= 24);
    }

    #[test]
    fn header_tabs_line_keeps_selected_page_visible_when_compact() {
        let ui = UiState {
            page: Page::Settings,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let line = header_tabs_line(Palette::default(), &ui, 24);

        assert!(line_width(&line) <= 24);
        assert!(line_text(&line).contains("6 Settings"));
    }

    #[test]
    fn header_tabs_line_uses_routing_label_for_v3() {
        let ui = UiState {
            page: Page::Stations,
            language: crate::tui::Language::En,
            config_version: Some(3),
            ..Default::default()
        };

        let line = header_tabs_line(Palette::default(), &ui, 80);

        assert!(line_text(&line).contains("2 Routing"));
    }

    #[test]
    fn header_tabs_line_falls_back_to_selected_page_for_tiny_width() {
        let ui = UiState {
            page: Page::Recent,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let line = header_tabs_line(Palette::default(), &ui, 8);

        assert!(line_width(&line) <= 8);
        assert!(line_text(&line).starts_with("8"));
    }
}
