use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::model::{Palette, Snapshot};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay, Page, page_index, page_titles};

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
    let overrides_effort = snapshot.overrides.len();
    let overrides_cfg = snapshot.config_overrides.len();
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

    let global_cfg = snapshot
        .global_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let focus = match ui.focus {
        Focus::Sessions => crate::tui::i18n::pick(ui.language, "会话", "Sessions"),
        Focus::Requests => crate::tui::i18n::pick(ui.language, "请求", "Requests"),
        Focus::Configs => crate::tui::i18n::pick(ui.language, "配置", "Configs"),
    };
    let title = Line::from(vec![
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
    ]);

    let last_req = snapshot.recent.first();
    let last_provider = last_req
        .and_then(|r| r.provider_id.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_config = last_req
        .and_then(|r| r.config_name.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_attempts = last_req
        .and_then(|r| r.retry.as_ref())
        .map(|x| x.attempts)
        .unwrap_or(1);

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

    let subtitle = Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "活跃 ", "active "),
            Style::default().fg(p.muted),
        ),
        Span::styled(active_total.to_string(), Style::default().fg(p.good)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "错误(80) ", "errors(80) "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            recent_err.to_string(),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled("5m ", Style::default().fg(p.muted)),
        Span::styled(
            fmt_ok_pct(s5.ok_2xx, s5.total),
            Style::default().fg(if s5.total > 0 && s5.ok_2xx == s5.total {
                p.good
            } else {
                p.muted
            }),
        ),
        Span::raw(" "),
        Span::styled("p95 ", Style::default().fg(p.muted)),
        Span::styled(fmt_ms(s5.p95_ms), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("att ", Style::default().fg(p.muted)),
        Span::styled(fmt_attempts(s5.avg_attempts), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("429 ", Style::default().fg(p.muted)),
        Span::styled(
            s5.err_429.to_string(),
            Style::default().fg(if s5.err_429 > 0 { p.warn } else { p.muted }),
        ),
        Span::raw(" "),
        Span::styled("5xx ", Style::default().fg(p.muted)),
        Span::styled(
            s5.err_5xx.to_string(),
            Style::default().fg(if s5.err_5xx > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled("1h ", Style::default().fg(p.muted)),
        Span::styled(
            fmt_ok_pct(s1.ok_2xx, s1.total),
            Style::default().fg(p.muted),
        ),
        Span::raw(" "),
        Span::styled("p95 ", Style::default().fg(p.muted)),
        Span::styled(fmt_ms(s1.p95_ms), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("429 ", Style::default().fg(p.muted)),
        Span::styled(
            s1.err_429.to_string(),
            Style::default().fg(if s1.err_429 > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "当前 ", "cur "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("{last_provider}/{last_config}×{last_attempts}"),
            Style::default().fg(p.accent),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "健康检查 ", "hc "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            if hc_running > 0 {
                if ui.language == crate::tui::Language::Zh {
                    format!("运行:{hc_running} 取消:{hc_canceling}")
                } else {
                    format!("run:{hc_running} cancel:{hc_canceling}")
                }
            } else {
                "-".to_string()
            },
            Style::default().fg(if hc_running > 0 { p.accent } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "覆盖 ", "overrides "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("{overrides_effort}/{overrides_cfg}"),
            Style::default().fg(p.muted),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "覆盖(全局) ", "override(global) "),
            Style::default().fg(p.muted),
        ),
        Span::styled(global_cfg.to_string(), Style::default().fg(p.accent)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "刷新 ", "updated "),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{updated}ms"), Style::default().fg(p.muted)),
    ]);

    let tabs = ratatui::widgets::Tabs::new(
        page_titles(ui.language)
            .iter()
            .map(|t| Line::from(*t))
            .collect::<Vec<_>>(),
    )
    .select(page_index(ui.page))
    .style(Style::default().fg(p.muted))
    .highlight_style(Style::default().fg(p.text).add_modifier(Modifier::BOLD))
    .divider(Span::raw("  "));

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(p.border));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(Text::from(title)), chunks[0]);
    f.render_widget(Paragraph::new(Text::from(subtitle)), chunks[1]);
    f.render_widget(tabs, chunks[2]);
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
                "1-6 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 或 j/k 移动  Enter effort  l/m/h/X 设置  x 清除  p 会话配置  P 全局配置  ? 帮助",
                "1-6 pages  q quit  L language  Tab focus  ↑/↓ or j/k move  Enter effort  l/m/h/X set  x clear  p session cfg  P global cfg  ? help",
            ),
            Page::Configs => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  ↑/↓ 选择  i 详情  t 切换 enabled  +/- level  h 检查  H 全部检查  c 取消  C 全部取消  Enter 设为 active  Backspace 自动  o 会话 override  O 清除  ? 帮助",
                "1-6 pages  q quit  L language  ↑/↓ select  i details  t toggle enabled  +/- level  h check  H check all  c cancel  C cancel all  Enter set active  Backspace auto  o session override  O clear  ? help",
            ),
            Page::Requests => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  ↑/↓ 选择  e 仅看错误  s scope(会话/全部)  ? 帮助",
                "1-6 pages  q quit  L language  ↑/↓ select  e errors_only  s scope(session/all)  ? help",
            ),
            Page::Sessions => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  ↑/↓ 选择  a 仅看活跃  e 仅看错误  v 仅看覆盖  r 重置  ? 帮助",
                "1-6 pages  q quit  L language  ↑/↓ select  a active_only  e errors_only  v overrides_only  r reset  ? help",
            ),
            Page::Stats => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  Tab 焦点(config/provider)  ↑/↓ 选择  d 天数(7/21/60)  e 仅看错误(recent)  y 复制+导出报告  ? 帮助",
                "1-6 pages  q quit  L language  Tab focus(config/provider)  ↑/↓ select  d days(7/21/60)  e errors_only(recent)  y copy+export report  ? help",
            ),
            Page::Settings => crate::tui::i18n::pick(
                ui.language,
                if ui.service_name == "codex" {
                    "1-6 页面  q 退出  L 语言  R 重载配置  O 覆盖导入(~/.codex，二次确认)  ? 帮助"
                } else {
                    "1-6 页面  q 退出  L 语言  R 重载配置  ? 帮助"
                },
                if ui.service_name == "codex" {
                    "1-6 pages  q quit  L language  R reload  O overwrite(~/.codex, confirm)  ? help"
                } else {
                    "1-6 pages  q quit  L language  R reload  ? help"
                },
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
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter 应用  Esc 取消",
            "↑/↓ select  Enter apply  Esc cancel",
        ),
        Overlay::ConfigInfo => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言",
            "↑/↓ scroll  PgUp/PgDn page  Esc close  L language",
        ),
    };
    let right = ui.toast.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");

    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled(right, Style::default().fg(p.accent)),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(p.border));
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(Text::from(line)).wrap(Wrap { trim: true }),
        area,
    );
}
