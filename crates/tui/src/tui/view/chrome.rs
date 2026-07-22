use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, Snapshot, now_ms, request_attempt_count, shorten_middle};
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

fn route_summary_full(provider: &str, endpoint: &str, attempts: impl std::fmt::Display) -> String {
    format!("{provider} -> {endpoint} x{attempts}")
}

fn route_summary_medium(
    provider: &str,
    endpoint: &str,
    attempts: impl std::fmt::Display,
) -> String {
    format!(
        "{} -> {} x{attempts}",
        shorten_middle(provider, 18),
        shorten_middle(endpoint, 18)
    )
}

fn route_summary_compact(endpoint: &str, attempts: impl std::fmt::Display) -> String {
    format!("{} x{attempts}", shorten_middle(endpoint, 18))
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

    let help_part = parts
        .iter()
        .copied()
        .find(|part| part.starts_with('?') || part.contains("help") || part.contains("帮助"));

    let mut first_parts = Vec::new();
    let mut split_at = parts.len();
    for (idx, part) in parts.iter().enumerate() {
        if footer_parts_fit(&first_parts, part, max_width) || first_parts.is_empty() {
            first_parts.push(*part);
        } else {
            split_at = idx;
            break;
        }
    }

    let mut second_parts = Vec::new();
    let mut hidden_parts = false;
    if split_at < parts.len() {
        for part in &parts[split_at..] {
            if footer_parts_fit(&second_parts, part, max_width) || second_parts.is_empty() {
                second_parts.push(*part);
            } else {
                hidden_parts = true;
                break;
            }
        }
    }

    if hidden_parts
        && let Some(help_part) = help_part
        && !first_parts.contains(&help_part)
        && !second_parts.contains(&help_part)
    {
        while !second_parts.is_empty() && !footer_parts_fit(&second_parts, help_part, max_width) {
            second_parts.pop();
        }
        if footer_parts_fit(&second_parts, help_part, max_width) || second_parts.is_empty() {
            second_parts.push(help_part);
        }
    }

    (first_parts.join("  "), second_parts.join("  "))
}

fn footer_parts_fit(parts: &[&str], next: &str, max_width: usize) -> bool {
    let current_width = parts
        .iter()
        .map(|part| UnicodeWidthStr::width(*part))
        .sum::<usize>();
    let separators = parts.len().saturating_mul(2);
    current_width
        .saturating_add(separators)
        .saturating_add(UnicodeWidthStr::width(next))
        <= max_width
}

fn settings_footer_help_text(ui: &UiState) -> String {
    let mut parts = match (ui.language, ui.runtime_connection.is_attached()) {
        (crate::tui::Language::Zh, false) => vec!["1-9/0 页面", "q 退出"],
        (crate::tui::Language::En, false) => vec!["1-9/0 pages", "q quit"],
        (crate::tui::Language::Zh, true) => vec!["q 只退出控制台"],
        (crate::tui::Language::En, true) => vec!["q exit console only"],
    };
    parts.push(match ui.language {
        crate::tui::Language::Zh => "↑/↓/Pg/Home/End 滚动",
        crate::tui::Language::En => "↑/↓/Pg/Home/End scroll",
    });
    if ui.can_mutate_default_profile() {
        parts.push("p/P profiles");
    }
    if ui.can_reload_runtime() {
        parts.push(match ui.language {
            crate::tui::Language::Zh => "R 重载",
            crate::tui::Language::En => "R reload",
        });
    }
    if ui.can_inspect_relay_capabilities() {
        parts.push("C inspect");
    }
    if ui.can_run_relay_live_smoke() {
        parts.push("X/Y smoke");
    }
    if ui.allows_local_codex_switch() {
        parts.extend(["n/o switch", "B/I/F/V/D presets"]);
    }
    if !ui.can_mutate_default_profile()
        && !ui.can_reload_runtime()
        && !ui.can_inspect_relay_capabilities()
        && !ui.can_run_relay_live_smoke()
        && !ui.allows_local_codex_switch()
    {
        parts.push(match ui.runtime_connection.is_remote_observer() {
            true => "remote read-only",
            false => "currently read-only",
        });
    }
    parts.push("L language");
    parts.push("? help");
    parts.join("  ")
}

fn sessions_footer_help_text(ui: &UiState) -> String {
    let mut parts = match (ui.language, ui.runtime_connection.is_attached()) {
        (crate::tui::Language::Zh, false) => vec!["1-9/0 页面", "q 退出"],
        (crate::tui::Language::En, false) => vec!["1-9/0 pages", "q quit"],
        (crate::tui::Language::Zh, true) => vec!["q 只退出控制台"],
        (crate::tui::Language::En, true) => vec!["q exit console only"],
    };
    parts.push(match ui.language {
        crate::tui::Language::Zh => "↑/↓ 会话",
        crate::tui::Language::En => "↑/↓ session",
    });
    parts.push(match ui.language {
        crate::tui::Language::Zh => "Pg 详情",
        crate::tui::Language::En => "Pg details",
    });
    if ui.can_mutate_session_affinity() {
        parts.push("A affinity");
    }
    parts.push("a/e/v filters");
    if ui.can_mutate_session_binding() {
        parts.extend([
            "b/M/E/f binding",
            "Enter effort",
            "x clear effort",
            "R reset",
        ]);
    }
    parts.push(match ui.language {
        crate::tui::Language::Zh => "o 请求",
        crate::tui::Language::En => "o requests",
    });
    if ui.can_bridge_runtime_sessions_to_local_codex() {
        parts.push(match ui.language {
            crate::tui::Language::Zh => "t 记录",
            crate::tui::Language::En => "t transcript",
        });
    }
    if !ui.can_mutate_session_affinity() && !ui.can_mutate_session_binding() {
        parts.push(match (ui.language, ui.runtime_connection) {
            (
                crate::tui::Language::Zh,
                crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ) => "远程只读",
            (
                crate::tui::Language::En,
                crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ) => "remote read-only",
            (crate::tui::Language::Zh, crate::tui::state::RuntimeConnectionKind::LocalAttached) => {
                "本机只读"
            }
            (crate::tui::Language::En, crate::tui::state::RuntimeConnectionKind::LocalAttached) => {
                "local read-only"
            }
            (crate::tui::Language::Zh, _) => "当前只读",
            (crate::tui::Language::En, _) => "currently read-only",
        });
    }
    parts.push("? help");
    parts.join("  ")
}

fn requests_footer_help_text(ui: &UiState) -> String {
    let mut parts = match (ui.language, ui.runtime_connection.is_attached()) {
        (crate::tui::Language::Zh, false) => vec!["1-9/0 页面", "q 退出"],
        (crate::tui::Language::En, false) => vec!["1-9/0 pages", "q quit"],
        (crate::tui::Language::Zh, true) => vec!["q 只退出控制台"],
        (crate::tui::Language::En, true) => vec!["q exit console only"],
    };
    parts.push(match ui.language {
        crate::tui::Language::Zh => "↑/↓ 请求",
        crate::tui::Language::En => "↑/↓ request",
    });
    parts.push(match ui.language {
        crate::tui::Language::Zh => "Pg 详情",
        crate::tui::Language::En => "Pg details",
    });
    parts.push("e/c/s filters");
    parts.push(match ui.language {
        crate::tui::Language::Zh => "x 清焦点",
        crate::tui::Language::En => "x clear focus",
    });
    parts.push(match ui.language {
        crate::tui::Language::Zh => "o 会话",
        crate::tui::Language::En => "o session",
    });
    if ui.can_bridge_runtime_sessions_to_local_codex() {
        parts.push(match ui.language {
            crate::tui::Language::Zh => "h 历史",
            crate::tui::Language::En => "h history",
        });
    }
    parts.push("? help");
    parts.join("  ")
}

fn dashboard_footer_help_text(ui: &UiState) -> String {
    let mut parts = match (ui.language, ui.runtime_connection.is_attached()) {
        (crate::tui::Language::Zh, false) => vec!["1-9/0 页面", "q 退出"],
        (crate::tui::Language::En, false) => vec!["1-9/0 pages", "q quit"],
        (crate::tui::Language::Zh, true) => vec!["q 只退出控制台"],
        (crate::tui::Language::En, true) => vec!["q exit console only"],
    };
    parts.push("Tab focus");
    parts.push(match ui.language {
        crate::tui::Language::Zh => "Pg/Home/End 详情",
        crate::tui::Language::En => "Pg/Home/End details",
    });
    match ui.focus {
        Focus::Sessions => {
            parts.push(match ui.language {
                crate::tui::Language::Zh => "↑/↓ 会话",
                crate::tui::Language::En => "↑/↓ session",
            });
            parts.push(match ui.language {
                crate::tui::Language::Zh => "O 请求",
                crate::tui::Language::En => "O requests",
            });
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                parts.push(match ui.language {
                    crate::tui::Language::Zh => "H 历史",
                    crate::tui::Language::En => "H history",
                });
            }
            if ui.can_mutate_session_binding() {
                parts.extend(["b/M/E/f binding", "Enter effort", "x clear effort"]);
            }
        }
        Focus::Requests => {
            parts.push(match ui.language {
                crate::tui::Language::Zh => "↑/↓ 请求",
                crate::tui::Language::En => "↑/↓ request",
            });
            parts.push(match ui.language {
                crate::tui::Language::Zh => "o 会话",
                crate::tui::Language::En => "o session",
            });
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                parts.push(match ui.language {
                    crate::tui::Language::Zh => "h 历史",
                    crate::tui::Language::En => "h history",
                });
            }
        }
        Focus::Providers => {}
    }
    parts.push("? help");
    parts.join("  ")
}

fn stats_footer_help_text(ui: &UiState) -> String {
    let mut parts = match (ui.language, ui.runtime_connection.is_attached()) {
        (crate::tui::Language::Zh, false) => vec!["1-9/0 页面", "q 退出"],
        (crate::tui::Language::En, false) => vec!["1-9/0 pages", "q quit"],
        (crate::tui::Language::Zh, true) => vec!["q 只退出控制台"],
        (crate::tui::Language::En, true) => vec!["q exit console only"],
    };
    parts.push(match ui.language {
        crate::tui::Language::Zh => "Tab 额度视图",
        crate::tui::Language::En => "Tab quota view",
    });
    parts.push(match ui.language {
        crate::tui::Language::Zh => "↑/↓ 移动",
        crate::tui::Language::En => "↑/↓ move",
    });
    parts.push(match (ui.language, ui.can_refresh_provider_balances()) {
        (crate::tui::Language::Zh, true) => "g 刷新余额",
        (crate::tui::Language::En, true) => "g refresh balances",
        (crate::tui::Language::Zh, false) => "g 刷新快照",
        (crate::tui::Language::En, false) => "g refresh snapshot",
    });
    parts.push(match ui.language {
        crate::tui::Language::Zh => "y 导出",
        crate::tui::Language::En => "y export",
    });
    parts.push("? help");
    parts.join("  ")
}

fn routing_footer_help_text(ui: &UiState, base: &str) -> String {
    if !ui.routing_detail_available {
        return base.to_string();
    }
    let pane_hint = match ui.language {
        crate::tui::Language::Zh => "Tab 候选/详情",
        crate::tui::Language::En => "Tab list/details",
    };
    format!("{base}  {pane_hint}")
}

fn footer_help_text(ui: &UiState) -> String {
    if ui.overlay == Overlay::None && ui.page == Page::Settings {
        return settings_footer_help_text(ui);
    }
    if ui.overlay == Overlay::None && ui.page == Page::Sessions {
        return sessions_footer_help_text(ui);
    }
    if ui.overlay == Overlay::None && ui.page == Page::Requests {
        return requests_footer_help_text(ui);
    }
    if ui.overlay == Overlay::None && ui.page == Page::Dashboard {
        return dashboard_footer_help_text(ui);
    }
    if ui.overlay == Overlay::None && ui.page == Page::Stats {
        return stats_footer_help_text(ui);
    }

    if !ui.runtime_connection.is_attached()
        && ui.overlay == Overlay::None
        && ui.page == Page::Routing
    {
        let base = match (
            ui.language,
            ui.can_mutate_routing(),
            ui.can_refresh_provider_balances(),
        ) {
            (crate::tui::Language::Zh, true, true) => {
                "1-9/0 页面  q 退出  ↑/↓/Pg 端点  Enter 操作  a 自动  m 模式  g 刷新  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, true, true) => {
                "1-9/0 pages  q quit  ↑/↓/Pg endpoint  Enter actions  a auto  m mode  g refresh  i details  ? help"
            }
            (crate::tui::Language::Zh, true, false) => {
                "1-9/0 页面  q 退出  ↑/↓/Pg 端点  Enter 操作  a 自动  m 模式  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, true, false) => {
                "1-9/0 pages  q quit  ↑/↓/Pg endpoint  Enter actions  a auto  m mode  i details  ? help"
            }
            (crate::tui::Language::Zh, false, true) => {
                "1-9/0 页面  q 退出  ↑/↓/Pg 端点  g 刷新  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, false, true) => {
                "1-9/0 pages  q quit  ↑/↓/Pg endpoint  g refresh  i details  ? help"
            }
            (crate::tui::Language::Zh, false, false) => {
                "1-9/0 页面  q 退出  ↑/↓ 端点  i 详情  当前只读  ? 帮助"
            }
            (crate::tui::Language::En, false, false) => {
                "1-9/0 pages  q quit  ↑/↓ endpoint  i details  currently read-only  ? help"
            }
        };
        return routing_footer_help_text(ui, base);
    }

    if ui.runtime_connection.is_attached() && ui.overlay == Overlay::None {
        let base = match (
            ui.language,
            ui.page,
            ui.can_mutate_routing(),
            ui.can_refresh_provider_balances(),
            ui.runtime_connection.is_remote_observer(),
            ui.can_bridge_runtime_sessions_to_local_codex(),
        ) {
            (crate::tui::Language::Zh, Page::Routing, true, true, _, _) => {
                "q 只退出控制台  ↑/↓/Pg 端点  Enter 操作  a 自动  m 模式  g 刷新  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, Page::Routing, true, true, _, _) => {
                "q exit console only  ↑/↓/Pg endpoint  Enter actions  a auto  m mode  g refresh  i details  ? help"
            }
            (crate::tui::Language::Zh, Page::Routing, true, false, _, _) => {
                "q 只退出控制台  ↑/↓/Pg 端点  Enter 操作  a 自动  m 模式  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, Page::Routing, true, false, _, _) => {
                "q exit console only  ↑/↓/Pg endpoint  Enter actions  a auto  m mode  i details  ? help"
            }
            (crate::tui::Language::Zh, Page::Routing, false, true, _, _) => {
                "q 只退出控制台  ↑/↓/Pg 端点  g 刷新  i 详情  ? 帮助"
            }
            (crate::tui::Language::En, Page::Routing, false, true, _, _) => {
                "q exit console only  ↑/↓/Pg endpoint  g refresh  i details  ? help"
            }
            (crate::tui::Language::Zh, Page::Routing, false, false, true, _) => {
                "q 只退出控制台  ↑/↓ 端点  i 详情  远程只读  ? 帮助"
            }
            (crate::tui::Language::En, Page::Routing, false, false, true, _) => {
                "q exit console only  ↑/↓ endpoint  i details  remote read-only  ? help"
            }
            (crate::tui::Language::Zh, Page::Routing, false, false, false, _) => {
                "q 只退出控制台  ↑/↓ 端点  i 详情  本机只读  ? 帮助"
            }
            (crate::tui::Language::En, Page::Routing, false, false, false, _) => {
                "q exit console only  ↑/↓ endpoint  i details  local read-only  ? help"
            }
            (crate::tui::Language::Zh, Page::Dashboard, _, _, _, true) => {
                "1-9/0 页面  q 只退出控制台  Tab 焦点  ↑/↓ 移动  O/H o/h 关联页  ? 帮助"
            }
            (crate::tui::Language::En, Page::Dashboard, _, _, _, true) => {
                "1-9/0 pages  q exit console only  Tab focus  ↑/↓ move  O/H o/h related pages  ? help"
            }
            (crate::tui::Language::Zh, Page::Dashboard, _, _, _, false) => {
                "1-9/0 页面  q 只退出控制台  Tab 焦点  ↑/↓ 移动  O/o 请求/会话  ? 帮助"
            }
            (crate::tui::Language::En, Page::Dashboard, _, _, _, false) => {
                "1-9/0 pages  q exit console only  Tab focus  ↑/↓ move  O/o requests/sessions  ? help"
            }
            (crate::tui::Language::Zh, Page::Requests, _, _, _, true) => {
                "q 只退出控制台  ↑/↓ 请求  e/c/s 筛选  x 清焦点  o 会话  h 历史  ? 帮助"
            }
            (crate::tui::Language::En, Page::Requests, _, _, _, true) => {
                "q exit console only  ↑/↓ request  e/c/s filters  x clear focus  o session  h history  ? help"
            }
            (crate::tui::Language::Zh, Page::Requests, _, _, _, false) => {
                "q 只退出控制台  ↑/↓ 请求  e/c/s 筛选  x 清焦点  o 会话  ? 帮助"
            }
            (crate::tui::Language::En, Page::Requests, _, _, _, false) => {
                "q exit console only  ↑/↓ request  e/c/s filters  x clear focus  o session  ? help"
            }
            (crate::tui::Language::Zh, Page::Stats, _, _, _, _) => {
                "q 只退出控制台  Tab 额度视图  ↑/↓ 移动  g 刷新  y 导出  ? 帮助"
            }
            (crate::tui::Language::En, Page::Stats, _, _, _, _) => {
                "q exit console only  Tab quota view  ↑/↓ move  g refresh  y export  ? help"
            }
            (crate::tui::Language::Zh, Page::Settings, _, _, _, _) => {
                if ui.allows_local_codex_switch() {
                    "q 只退出控制台  n/o Codex switch  L 语言  ? 帮助"
                } else {
                    "q 只退出控制台  L 语言  当前只读  ? 帮助"
                }
            }
            (crate::tui::Language::En, Page::Settings, _, _, _, _) => {
                if ui.allows_local_codex_switch() {
                    "q exit console only  n/o Codex switch  L language  ? help"
                } else {
                    "q exit console only  L language  read-only  ? help"
                }
            }
            (crate::tui::Language::Zh, Page::History, _, _, _, true) => {
                "q 只退出控制台  ↑/↓ 选择  Pg 详情  r 刷新  t/Enter 记录  s/f 会话/请求  ? 帮助"
            }
            (crate::tui::Language::En, Page::History, _, _, _, true) => {
                "q exit console only  ↑/↓ select  Pg details  r refresh  t/Enter transcript  s/f session/requests  ? help"
            }
            (crate::tui::Language::Zh, Page::History, _, _, _, false) => {
                "q 只退出控制台  ↑/↓ 选择  Pg 详情  r 刷新  t/Enter 本机记录  ? 帮助"
            }
            (crate::tui::Language::En, Page::History, _, _, _, false) => {
                "q exit console only  ↑/↓ select  Pg details  r refresh  t/Enter local transcript  ? help"
            }
            (crate::tui::Language::Zh, Page::Recent, _, _, _, true) => {
                "q 只退出控制台  ↑/↓ 选择  Pg 详情  [] 时间  Enter/y 复制  t 记录  s/f/h 跳转  ? 帮助"
            }
            (crate::tui::Language::En, Page::Recent, _, _, _, true) => {
                "q exit console only  ↑/↓ select  Pg details  [] window  Enter/y copy  t transcript  s/f/h navigate  ? help"
            }
            (crate::tui::Language::Zh, Page::Recent, _, _, _, false) => {
                "q 只退出控制台  ↑/↓ 选择  Pg 详情  [] 时间  Enter/y 复制  t 记录  h 历史  ? 帮助"
            }
            (crate::tui::Language::En, Page::Recent, _, _, _, false) => {
                "q exit console only  ↑/↓ select  Pg details  [] window  Enter/y copy  t transcript  h history  ? help"
            }
            (crate::tui::Language::Zh, Page::Fleet, _, _, _, _) => {
                "q 只退出控制台  Tab 焦点  ↑/↓ 移动  r 刷新  t Tree/Flat  ? 帮助"
            }
            (crate::tui::Language::En, Page::Fleet, _, _, _, _) => {
                "q exit console only  Tab focus  ↑/↓ move  r refresh  t Tree/Flat  ? help"
            }
            (crate::tui::Language::Zh, Page::ServiceStatus, _, _, _, _) => {
                "q 只退出控制台  ↑/↓/Pg 探针  Tab 详情  r 刷新  ? 帮助"
            }
            (crate::tui::Language::En, Page::ServiceStatus, _, _, _, _) => {
                "q exit console only  ↑/↓/Pg probes  Tab details  r refresh  ? help"
            }
            (crate::tui::Language::Zh, Page::Sessions, _, _, _, _) => {
                "q 只退出控制台  ↑/↓ 会话  a/e 筛选  ? 帮助"
            }
            (crate::tui::Language::En, Page::Sessions, _, _, _, _) => {
                "q exit console only  ↑/↓ session  a/e filters  ? help"
            }
        };
        return if ui.page == Page::Routing {
            routing_footer_help_text(ui, base)
        } else {
            base.to_string()
        };
    }

    match ui.overlay {
        Overlay::None => match ui.page {
            Page::Dashboard => i18n::text(ui.language, msg::FOOTER_DASHBOARD),
            Page::Routing => i18n::text(ui.language, msg::FOOTER_ROUTING),
            Page::Requests => i18n::text(ui.language, msg::FOOTER_REQUESTS),
            Page::Sessions => i18n::text(ui.language, msg::FOOTER_SESSIONS),
            Page::Stats => i18n::text(ui.language, msg::FOOTER_STATS),
            Page::Settings if ui.allows_local_codex_switch() => {
                i18n::text(ui.language, msg::FOOTER_SETTINGS_CODEX)
            }
            Page::Settings => i18n::text(ui.language, msg::FOOTER_SETTINGS_OTHER),
            Page::History => i18n::text(ui.language, msg::FOOTER_HISTORY),
            Page::Recent => i18n::text(ui.language, msg::FOOTER_RECENT),
            Page::Fleet => i18n::text(ui.language, msg::FOOTER_FLEET),
            Page::ServiceStatus => i18n::text(ui.language, msg::FOOTER_SERVICE_STATUS),
        },
        Overlay::Help => i18n::text(ui.language, msg::FOOTER_HELP),
        Overlay::ProviderInfo => i18n::text(ui.language, msg::FOOTER_PROVIDER_INFO),
        Overlay::SessionTranscript => i18n::text(ui.language, msg::FOOTER_SESSION_TRANSCRIPT),
        Overlay::StartupAlert => i18n::text(ui.language, msg::FOOTER_STARTUP_GUARDRAIL),
        Overlay::RoutingActions => match ui.language {
            crate::tui::Language::Zh => "↑/↓ 选择  Enter 继续  Esc 取消",
            crate::tui::Language::En => "↑/↓ select  Enter continue  Esc cancel",
        },
        Overlay::RoutingConfirmation => match ui.language {
            crate::tui::Language::Zh => "Enter / y 确认  Esc / n 取消",
            crate::tui::Language::En => "Enter / y confirm  Esc / n cancel",
        },
        Overlay::SessionAffinityActions => match ui.language {
            crate::tui::Language::Zh => "↑/↓ 选择  Enter 继续  Esc 取消",
            crate::tui::Language::En => "↑/↓ select  Enter continue  Esc cancel",
        },
        Overlay::SessionAffinityConfirmation => match ui.language {
            crate::tui::Language::Zh => "Enter / y 确认  Esc / n 取消",
            crate::tui::Language::En => "Enter / y confirm  Esc / n cancel",
        },
        Overlay::SessionProfileMenu
        | Overlay::SessionModelMenu
        | Overlay::SessionEffortMenu
        | Overlay::SessionServiceTierMenu
        | Overlay::ConfiguredDefaultProfileMenu
        | Overlay::RuntimeDefaultProfileMenu => match ui.language {
            crate::tui::Language::Zh => "↑/↓ 选择  Enter 应用  Esc 取消",
            crate::tui::Language::En => "↑/↓ select  Enter apply  Esc cancel",
        },
        Overlay::SessionBindingInput => match ui.language {
            crate::tui::Language::Zh => "输入值  Enter 应用  Esc 返回",
            crate::tui::Language::En => "type value  Enter apply  Esc back",
        },
    }
    .to_string()
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

fn compact_page_key(idx: usize) -> String {
    if idx == 9 {
        "0".to_string()
    } else {
        (idx + 1).to_string()
    }
}

fn header_tabs_line(p: Palette, ui: &UiState, max_width: u16) -> Line<'static> {
    let selected = page_index(ui.page);
    let titles = page_titles(ui.language);

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
            compact_page_key(idx)
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
    let updated = snapshot_age_ms(ui, snapshot, now_ms());
    let focus = match ui.page {
        Page::Fleet => i18n::label(ui.language, "fleet view"),
        Page::ServiceStatus => i18n::label(ui.language, "service status"),
        _ => match ui.focus {
            Focus::Sessions => i18n::text(ui.language, msg::FOCUS_SESSIONS),
            Focus::Requests => i18n::text(ui.language, msg::FOCUS_REQUESTS),
            Focus::Providers => i18n::text(ui.language, msg::FOCUS_PROVIDERS),
        },
    };
    let connection = ui.runtime_connection.label(ui.language);
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
            Span::styled(connection.to_string(), Style::default().fg(p.muted)),
            Span::raw("  "),
            Span::styled(
                format!("{}{focus}", i18n::text(ui.language, msg::FOCUS_LABEL)),
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
            Span::styled(connection.to_string(), Style::default().fg(p.muted)),
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
    let last_endpoint = last_req
        .and_then(|r| r.endpoint_id.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_attempts = last_req.map(request_attempt_count).unwrap_or(1);

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

    let route_full = route_summary_full(last_provider, last_endpoint, last_attempts);
    let route_medium = route_summary_medium(last_provider, last_endpoint, last_attempts);
    let route_compact = route_summary_compact(last_endpoint, last_attempts);
    let mut subtitle_spans = Vec::new();
    if inner.width >= 150 {
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ACTIVE_SHORT),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ERRORS_SHORT),
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
            i18n::text(ui.language, msg::STATUS_CURRENT_SHORT),
            route_full,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        push_header_sep(&mut subtitle_spans, false);
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_UPDATED_SHORT),
            format!("{updated}ms"),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
    } else if inner.width >= 96 {
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ACTIVE_TINY),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ERRORS_TINY),
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
            i18n::text(ui.language, msg::STATUS_CURRENT_SHORT),
            route_medium,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_UPDATED_TINY),
            format!("{updated}ms"),
            Style::default().fg(p.muted),
            Style::default().fg(p.muted),
        );
    } else {
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ACTIVE_TINY),
            active_total.to_string(),
            Style::default().fg(p.muted),
            Style::default().fg(p.good),
        );
        push_header_sep(&mut subtitle_spans, true);
        push_header_metric(
            &mut subtitle_spans,
            i18n::text(ui.language, msg::STATUS_ERRORS_TINY),
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
            i18n::text(ui.language, msg::STATUS_CURRENT_SHORT),
            route_compact,
            Style::default().fg(p.muted),
            Style::default().fg(p.accent),
        );
        if inner.width >= 70 {
            push_header_sep(&mut subtitle_spans, true);
            push_header_metric(
                &mut subtitle_spans,
                i18n::text(ui.language, msg::STATUS_UPDATED_TINY),
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

fn snapshot_age_ms(ui: &UiState, snapshot: &Snapshot, current_time_ms: u64) -> u128 {
    ui.operator_read_model
        .as_ref()
        .filter(|model| {
            matches!(
                model.status,
                crate::dashboard_core::OperatorReadStatus::Ready
                    | crate::dashboard_core::OperatorReadStatus::Stale
            ) && model.captured_at_ms > 0
        })
        .map(|model| u128::from(current_time_ms.saturating_sub(model.captured_at_ms)))
        .unwrap_or_else(|| snapshot.refreshed_at.elapsed().as_millis())
}

pub(super) fn render_footer(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let now = std::time::Instant::now();
    if let Some((_, ts)) = ui.toast.as_ref()
        && now.duration_since(*ts) > Duration::from_secs(3)
    {
        ui.toast = None;
    }

    let left = footer_help_text(ui);
    let right = ui.toast.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");

    let (first, second) = split_footer_help(&left, area.width);
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
    fn stale_operator_snapshot_keeps_original_capture_age() {
        let ui = UiState {
            operator_read_model: Some(crate::dashboard_core::OperatorReadModel {
                api_version: 1,
                service_name: "codex".to_string(),
                status: crate::dashboard_core::OperatorReadStatus::Stale,
                captured_at_ms: 5_000,
                revisions: None,
                data: None,
                issue: Some(crate::dashboard_core::OperatorReadIssue::RefreshFailed),
            }),
            ..Default::default()
        };

        assert_eq!(snapshot_age_ms(&ui, &Snapshot::default(), 12_000), 7_000);
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
        let (first, second) = split_footer_help(
            "1-9 pages  q quit  L language  Tab focus  Enter actions",
            26,
        );

        assert!(UnicodeWidthStr::width(first.as_str()) <= 26);
        assert!(!second.is_empty());
        assert!(second.contains("Tab focus"));
    }

    #[test]
    fn split_footer_help_keeps_lines_bounded_and_help_discoverable() {
        let text = i18n::text(crate::tui::Language::En, msg::FOOTER_STATS);
        let (first, second) = split_footer_help(text, 38);

        assert!(UnicodeWidthStr::width(first.as_str()) <= 38, "{first}");
        assert!(UnicodeWidthStr::width(second.as_str()) <= 38, "{second}");
        assert!(
            first.contains("? help") || second.contains("? help"),
            "{first}\n{second}"
        );
    }

    #[test]
    fn footer_help_text_advertises_routing_operator_actions() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("endpoint"), "{text}");
        assert!(text.contains("i details"), "{text}");
        assert!(text.contains("Enter actions"), "{text}");
        assert!(text.contains("a auto"), "{text}");
        assert!(text.contains("g refresh"), "{text}");
    }

    #[test]
    fn wide_routing_footer_advertises_independent_detail_focus() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            routing_detail_available: true,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("Tab list/details"), "{text}");
    }

    #[test]
    fn dashboard_footer_uses_unambiguous_navigation_keys_for_each_focus() {
        let sessions = UiState {
            page: Page::Dashboard,
            focus: Focus::Sessions,
            language: crate::tui::Language::En,
            ..Default::default()
        };
        let requests = UiState {
            page: Page::Dashboard,
            focus: Focus::Requests,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let sessions = footer_help_text(&sessions);
        assert!(sessions.contains("O requests"), "{sessions}");
        assert!(sessions.contains("H history"), "{sessions}");
        assert!(!sessions.contains("o session"), "{sessions}");
        assert!(!sessions.contains("h history"), "{sessions}");

        let requests = footer_help_text(&requests);
        assert!(requests.contains("o session"), "{requests}");
        assert!(requests.contains("h history"), "{requests}");
        assert!(!requests.contains("O requests"), "{requests}");
    }

    #[test]
    fn integrated_routing_footer_does_not_advertise_blocked_actions() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            operator_read_model: Some(crate::dashboard_core::OperatorReadModel {
                api_version: 1,
                service_name: "codex".to_string(),
                status: crate::dashboard_core::OperatorReadStatus::Stale,
                captured_at_ms: 1,
                revisions: None,
                data: None,
                issue: Some(crate::dashboard_core::OperatorReadIssue::RefreshFailed),
            }),
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("currently read-only"), "{text}");
        assert!(!text.contains("Enter actions"), "{text}");
        assert!(!text.contains("g refresh"), "{text}");
    }

    #[test]
    fn remote_routing_footer_is_explicitly_read_only() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("endpoint"), "{text}");
        assert!(text.contains("remote read-only"), "{text}");
        assert!(!text.contains("Enter actions"), "{text}");
        assert!(!text.contains("g refresh"), "{text}");
    }

    #[test]
    fn local_attached_routing_footer_advertises_only_balance_refresh_capability() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            operator_action_capabilities: crate::dashboard_core::OperatorActionCapabilities {
                refresh_provider_balances: true,
                mutate_routing: false,
                mutate_session_affinity: false,
                mutate_session_binding: false,
                reload_runtime: false,
                mutate_default_profile: false,
                inspect_relay_capabilities: false,
                run_relay_live_smoke: false,
            },
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("g refresh"), "{text}");
        assert!(!text.contains("Enter actions"), "{text}");
        assert!(!text.contains("a auto"), "{text}");
        assert!(!text.contains("read-only"), "{text}");
    }

    #[test]
    fn local_attached_routing_footer_advertises_all_operator_capabilities() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            operator_action_capabilities: crate::dashboard_core::OperatorActionCapabilities {
                refresh_provider_balances: true,
                mutate_routing: true,
                mutate_session_affinity: true,
                mutate_session_binding: true,
                reload_runtime: false,
                mutate_default_profile: false,
                inspect_relay_capabilities: false,
                run_relay_live_smoke: false,
            },
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("Enter actions"), "{text}");
        assert!(text.contains("a auto"), "{text}");
        assert!(text.contains("g refresh"), "{text}");
        assert!(!text.contains("read-only"), "{text}");
    }

    #[test]
    fn local_attached_routing_footer_advertises_only_routing_mutation_capability() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            operator_action_capabilities: crate::dashboard_core::OperatorActionCapabilities {
                refresh_provider_balances: false,
                mutate_routing: true,
                mutate_session_affinity: false,
                mutate_session_binding: false,
                reload_runtime: false,
                mutate_default_profile: false,
                inspect_relay_capabilities: false,
                run_relay_live_smoke: false,
            },
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("Enter actions"), "{text}");
        assert!(text.contains("a auto"), "{text}");
        assert!(!text.contains("g refresh"), "{text}");
        assert!(!text.contains("read-only"), "{text}");
    }

    #[test]
    fn local_attached_routing_footer_does_not_claim_remote_read_only() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("local read-only"), "{text}");
        assert!(!text.contains("remote read-only"), "{text}");
    }

    #[test]
    fn integrated_sessions_footer_advertises_affinity_actions() {
        let ui = UiState {
            page: Page::Sessions,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("A affinity"), "{text}");
        assert!(text.contains("Enter effort"), "{text}");
        assert!(text.contains("a/e/v filters"), "{text}");
        assert!(text.contains("b/M/E/f binding"), "{text}");
        assert!(text.contains("R reset"), "{text}");
        assert!(!text.contains("read-only"), "{text}");
    }

    #[test]
    fn settings_footer_follows_runtime_capabilities() {
        let integrated = UiState {
            page: Page::Settings,
            language: crate::tui::Language::En,
            ..Default::default()
        };
        let remote = UiState {
            page: Page::Settings,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };
        let local_attached = UiState {
            page: Page::Settings,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            operator_action_capabilities: crate::dashboard_core::OperatorActionCapabilities {
                reload_runtime: true,
                inspect_relay_capabilities: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let integrated = footer_help_text(&integrated);
        assert!(integrated.contains("p/P profiles"), "{integrated}");
        assert!(integrated.contains("R reload"), "{integrated}");
        assert!(integrated.contains("C inspect"), "{integrated}");
        assert!(integrated.contains("X/Y smoke"), "{integrated}");
        assert!(integrated.contains("B/I/F/V/D presets"), "{integrated}");

        let remote = footer_help_text(&remote);
        assert!(remote.contains("remote read-only"), "{remote}");
        for blocked in [
            "p/P profiles",
            "R reload",
            "C inspect",
            "X/Y smoke",
            "presets",
        ] {
            assert!(
                !remote.contains(blocked),
                "unexpected {blocked:?} in {remote}"
            );
        }

        let local_attached = footer_help_text(&local_attached);
        assert!(local_attached.contains("R reload"), "{local_attached}");
        assert!(local_attached.contains("C inspect"), "{local_attached}");
        assert!(local_attached.contains("n/o switch"), "{local_attached}");
        assert!(!local_attached.contains("p/P profiles"), "{local_attached}");
        assert!(!local_attached.contains("X/Y smoke"), "{local_attached}");
    }

    #[test]
    fn attached_sessions_footer_distinguishes_local_and_remote_read_only() {
        let local = UiState {
            page: Page::Sessions,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            ..Default::default()
        };
        let remote = UiState {
            page: Page::Sessions,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        let local_text = footer_help_text(&local);
        let remote_text = footer_help_text(&remote);

        assert!(local_text.contains("local read-only"), "{local_text}");
        assert!(!local_text.contains("A affinity"), "{local_text}");
        assert!(local_text.contains("transcript"), "{local_text}");
        assert!(remote_text.contains("remote read-only"), "{remote_text}");
        assert!(!remote_text.contains("A affinity"), "{remote_text}");
        assert!(!remote_text.contains("transcript"), "{remote_text}");
        assert!(remote_text.contains("o requests"), "{remote_text}");
    }

    #[test]
    fn attached_page_footers_advertise_the_shared_controls_they_handle() {
        let remote = |page| UiState {
            page,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ..Default::default()
        };

        let requests = footer_help_text(&remote(Page::Requests));
        assert!(requests.contains("e/c/s filters"), "{requests}");
        assert!(requests.contains("x clear focus"), "{requests}");
        assert!(requests.contains("o session"), "{requests}");
        assert!(!requests.contains("h history"), "{requests}");

        let history = footer_help_text(&remote(Page::History));
        assert!(history.contains("Pg details"), "{history}");
        assert!(history.contains("r refresh"), "{history}");
        assert!(history.contains("local transcript"), "{history}");
        assert!(!history.contains("s/f"), "{history}");

        let recent = footer_help_text(&remote(Page::Recent));
        assert!(recent.contains("Pg details"), "{recent}");
        assert!(recent.contains("Enter/y copy"), "{recent}");
        assert!(recent.contains("h history"), "{recent}");
        assert!(!recent.contains("s/f/h"), "{recent}");

        let stats = footer_help_text(&remote(Page::Stats));
        assert!(stats.contains("g refresh"), "{stats}");
        assert!(stats.contains("y export"), "{stats}");

        let fleet = footer_help_text(&remote(Page::Fleet));
        assert!(fleet.contains("Tab focus"), "{fleet}");
        assert!(fleet.contains("r refresh"), "{fleet}");
        assert!(fleet.contains("t Tree/Flat"), "{fleet}");

        let local_history = footer_help_text(&UiState {
            page: Page::History,
            language: crate::tui::Language::En,
            runtime_connection: crate::tui::state::RuntimeConnectionKind::LocalAttached,
            local_operator_transport_available: true,
            ..Default::default()
        });
        assert!(
            local_history.contains("s/f session/requests"),
            "{local_history}"
        );
    }

    #[test]
    fn route_summary_full_uses_directional_separator() {
        assert_eq!(
            route_summary_full("provider-a", "endpoint-b", 3),
            "provider-a -> endpoint-b x3"
        );
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
        assert!(line_text(&line).contains("7 Settings"));
    }

    #[test]
    fn header_tabs_line_uses_canonical_routing_label() {
        let ui = UiState {
            page: Page::Routing,
            language: crate::tui::Language::En,
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
        assert!(line_text(&line).starts_with("9"));
    }

    #[test]
    fn footer_help_text_mentions_fleet_page() {
        let ui = UiState {
            page: Page::Fleet,
            language: crate::tui::Language::En,
            ..Default::default()
        };

        let text = footer_help_text(&ui);

        assert!(text.contains("1-9/0 pages"), "{text}");
        assert!(text.contains("Tab focus"), "{text}");
        assert!(text.contains("r refresh"), "{text}");
        assert!(text.contains("t Tree/Flat"), "{text}");
        assert!(text.contains("? help"), "{text}");
    }
}
