use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::Palette;
use crate::tui::state::UiState;
use crate::tui::types::Page;
use crate::tui::view::widgets::{centered_rect, max_wrapped_vertical_scroll};

fn help_heading(text: impl Into<String>, p: Palette) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    ))
}

fn help_current_page_title(lang: Language, page: Page) -> &'static str {
    match (lang, page) {
        (Language::Zh, Page::Dashboard) => "当前页面：总览",
        (Language::Zh, Page::Routing) => "当前页面：路由",
        (Language::Zh, Page::Sessions) => "当前页面：会话",
        (Language::Zh, Page::Requests) => "当前页面：请求",
        (Language::Zh, Page::Stats) => "当前页面：用量",
        (Language::Zh, Page::Settings) => "当前页面：设置",
        (Language::Zh, Page::History) => "当前页面：历史",
        (Language::Zh, Page::Recent) => "当前页面：最近",
        (Language::Zh, Page::Fleet) => "当前页面：Fleet",
        (Language::Zh, Page::ServiceStatus) => "当前页面：服务状态",
        (Language::En, Page::Dashboard) => "Current page: Dashboard",
        (Language::En, Page::Routing) => "Current page: Routing",
        (Language::En, Page::Sessions) => "Current page: Sessions",
        (Language::En, Page::Requests) => "Current page: Requests",
        (Language::En, Page::Stats) => "Current page: Usage",
        (Language::En, Page::Settings) => "Current page: Settings",
        (Language::En, Page::History) => "Current page: History",
        (Language::En, Page::Recent) => "Current page: Recent",
        (Language::En, Page::Fleet) => "Current page: Fleet",
        (Language::En, Page::ServiceStatus) => "Current page: Service Status",
    }
}

fn settings_help_entries(ui: &UiState, language: Language) -> Vec<&'static str> {
    let mut entries = vec![match language {
        Language::Zh => "  ↑/↓ PgUp/PgDn  滚动设置；Home/End 顶部/底部",
        Language::En => "  ↑/↓ PgUp/PgDn  scroll settings; Home/End top/bottom",
    }];
    if ui.can_mutate_default_profile() {
        entries.push(match language {
            Language::Zh => "  p/P        配置默认 profile / 运行时默认 profile",
            Language::En => "  p/P        configured profile / runtime profile",
        });
    }
    if ui.can_reload_runtime() {
        entries.push(match language {
            Language::Zh => "  R          重载运行时配置",
            Language::En => "  R          reload runtime configuration",
        });
    }
    if ui.can_inspect_relay_capabilities() {
        entries.push(match language {
            Language::Zh => "  C          运行 relay capability 诊断",
            Language::En => "  C          run relay capability diagnostics",
        });
    }
    if ui.can_run_relay_live_smoke() {
        entries.push(match language {
            Language::Zh => "  X/Y        二次确认 compact / compact+image live smoke",
            Language::En => "  X/Y        confirm compact / compact+image live smoke twice",
        });
    }
    if ui.allows_local_codex_switch() {
        entries.extend(match language {
            Language::Zh => [
                "  n/o        显式开启/关闭 Codex 本地 switch",
                "  B/I/F/V/D  切换 ChatGPT/Imagegen/Official/Official Imagegen/Default preset",
            ],
            Language::En => [
                "  n/o        explicitly switch the local Codex target on/off",
                "  B/I/F/V/D  select ChatGPT/Imagegen/Official/Official Imagegen/Default preset",
            ],
        });
    }
    if entries.len() == 1 {
        entries.push(match language {
            Language::Zh => "  当前仅可查看只读 operator bundle",
            Language::En => "  current operator bundle is read-only",
        });
    }
    entries
}

pub(super) fn current_page_help_lines(ui: &UiState, p: Palette) -> Vec<Line<'static>> {
    let lang = ui.language;
    let page = ui.page;
    let local_codex_switch = ui.allows_local_codex_switch();
    let mut lines = vec![help_heading(help_current_page_title(lang, page), p)];
    let entries = match (lang, page, local_codex_switch) {
        (Language::Zh, Page::Dashboard, _) => {
            let mut entries = vec![
                "  Tab        切换会话/请求焦点",
                "  ↑/↓        移动当前选择",
                "  PgUp/PgDn/Home/End 滚动会话详情",
            ];
            match ui.focus {
                crate::tui::types::Focus::Sessions => {
                    entries.push("  O          从会话跳到关联 Requests");
                    if ui.can_bridge_runtime_sessions_to_local_codex() {
                        entries.push("  H          从会话跳到 History");
                    }
                    if ui.can_mutate_session_binding() {
                        entries.extend([
                            "  b/M/E/f    profile / model / effort / fast 与 service tier",
                            "  Enter/x    打开 effort 菜单 / 清除 effort（兼容键）",
                            "  l/m/h/X    设置 low / medium / high / xhigh effort",
                            "  R          重置当前会话的手动控制",
                        ]);
                    }
                }
                crate::tui::types::Focus::Requests => {
                    entries.push("  o          从请求跳到关联 Sessions");
                    if ui.can_bridge_runtime_sessions_to_local_codex() {
                        entries.push("  h          从请求跳到 History");
                    }
                }
                crate::tui::types::Focus::Providers => {}
            }
            entries
        }
        (Language::Zh, Page::Routing, _) => {
            let mut entries = vec![
                "  ↑/↓ PgUp/PgDn  浏览候选端点或当前详情",
                "  Home/End   跳到当前面板首端或末端",
                "  p          定位当前新会话偏好",
            ];
            if ui.routing_detail_available {
                entries.insert(0, "  Tab        切换候选端点与详情焦点");
            }
            if ui.can_mutate_routing() {
                entries.extend([
                    "  Enter      打开新会话偏好与端点状态菜单",
                    "  a/Backspace 清除新会话偏好并恢复自动调度",
                    "  m          打开端点 Enabled/Draining/Disabled 菜单",
                ]);
            }
            if ui.can_refresh_provider_balances() {
                entries.push("  g          强制全量刷新余额/额度");
            }
            entries.extend([
                "  i          查看当前提供商与端点详情",
                "  Order/Group/Pri = 路由顺序 / 偏好组 / 端点优先级",
            ]);
            entries
        }
        (Language::Zh, Page::Sessions, _) => {
            let mut entries = vec![
                "  ↑/↓        选择会话；PgUp/PgDn/Home/End 滚动详情",
                "  a/e/v      活跃、错误、手动控制筛选；r 重置筛选",
            ];
            if ui.can_mutate_session_affinity() {
                entries.push("  A          打开空闲会话的 affinity 高级操作");
            }
            if ui.can_mutate_session_binding() {
                entries.extend([
                    "  b/M/E/f    profile / model / effort / fast 与 service tier",
                    "  Enter      打开 effort 菜单（v0.20.3 兼容键）",
                    "  l/m/h/X    快速设置 low / medium / high / xhigh effort",
                    "  x          清除 effort（兼容键）",
                    "  R          重置当前会话的手动控制",
                ]);
            }
            entries.push("  o          跳到关联 Requests");
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.extend(["  t          打开全屏对话记录", "  H          跳到 History"]);
            }
            entries
        }
        (Language::Zh, Page::Requests, _) => {
            let mut entries = vec![
                "  ↑/↓        选择请求；PgUp/PgDn/Home/End 滚动详情",
                "  e/c/s      错误、控制证据与会话范围筛选",
                "  x          清除显式 session 聚焦",
                "  o          跳到关联 Sessions",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  h          跳到 History");
            }
            entries
        }
        (Language::Zh, Page::Stats, _) => vec![
            "  Tab        切换额度池 / 项目 / 提供商 / 端点",
            "  ↑/↓        移动当前视图选择",
            if ui.can_refresh_provider_balances() {
                "  g          强制刷新全部余额/额度并读取新快照"
            } else {
                "  g          仅刷新观察快照；不会请求上游余额"
            },
            "  y          导出并复制选中报告",
        ],
        (Language::Zh, Page::Settings, _) => settings_help_entries(ui, Language::Zh),
        (Language::Zh, Page::History, _) => {
            let mut entries = vec![
                "  ↑/↓        选择会话；PgUp/PgDn/Home/End 滚动详情",
                "  r          刷新历史会话列表",
                "  t/Enter    打开全屏对话记录",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  s/f        跳到 Sessions / Requests");
            }
            entries
        }
        (Language::Zh, Page::Recent, _) => {
            let mut entries = vec![
                "  ↑/↓        选择会话；PgUp/PgDn/Home/End 滚动详情",
                "  [ / ]      切换时间窗口",
                "  Enter/y    复制选中项 / 复制可见列表",
                "  t/h        打开记录 / 跳到 History",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  s/f        跳到 Sessions / Requests");
            }
            entries
        }
        (Language::Zh, Page::Fleet, _) => vec![
            "  Tab        切换节点 / 工作单元焦点",
            "  r          刷新快照；t 切换 Tree / Flat",
        ],
        (Language::Zh, Page::ServiceStatus, _) => vec![
            "  ↑/↓ PgUp/PgDn  移动探针；Home/End 跳到首尾",
            "  Tab        切换探针列表 / 当前详情焦点",
            "  ↑/↓ PgUp/PgDn  在详情焦点中滚动；Home/End 跳到首尾",
            "  r          读取最新只读快照",
        ],
        (Language::En, Page::Dashboard, _) => {
            let mut entries = vec![
                "  Tab        switch Sessions / Requests focus",
                "  ↑/↓        move the active selection",
                "  PgUp/PgDn/Home/End scroll session details",
            ];
            match ui.focus {
                crate::tui::types::Focus::Sessions => {
                    entries.push("  O          jump from a session to related Requests");
                    if ui.can_bridge_runtime_sessions_to_local_codex() {
                        entries.push("  H          jump from a session to History");
                    }
                    if ui.can_mutate_session_binding() {
                        entries.extend([
                            "  b/M/E/f    profile / model / effort / fast and service tier",
                            "  Enter/x    open effort menu / clear effort (compatibility keys)",
                            "  l/m/h/X    set low / medium / high / xhigh effort",
                            "  R          reset the selected session's manual controls",
                        ]);
                    }
                }
                crate::tui::types::Focus::Requests => {
                    entries.push("  o          jump from a request to related Sessions");
                    if ui.can_bridge_runtime_sessions_to_local_codex() {
                        entries.push("  h          jump from a request to History");
                    }
                }
                crate::tui::types::Focus::Providers => {}
            }
            entries
        }
        (Language::En, Page::Routing, _) => {
            let mut entries = vec![
                "  ↑/↓ PgUp/PgDn  page endpoint candidates or focused details",
                "  Home/End   jump to the first or last item in the focused pane",
                "  p          locate the preferred new-session target",
            ];
            if ui.routing_detail_available {
                entries.insert(0, "  Tab        switch endpoint-list/detail focus");
            }
            if ui.can_mutate_routing() {
                entries.extend([
                    "  Enter      open new-session preference and endpoint actions",
                    "  a/Backspace clear the new-session preference and restore auto",
                    "  m          open endpoint Enabled/Draining/Disabled actions",
                ]);
            }
            if ui.can_refresh_provider_balances() {
                entries.push("  g          force-refresh all balances and quotas");
            }
            entries.extend([
                "  i          inspect the current provider and endpoint",
                "  Order/Group/Pri = route order / preference group / endpoint priority",
            ]);
            entries
        }
        (Language::En, Page::Sessions, _) => {
            let mut entries = vec![
                "  ↑/↓        select a session; PgUp/PgDn/Home/End scroll details",
                "  a/e/v      active, error, and manual-control filters; r resets",
            ];
            if ui.can_mutate_session_affinity() {
                entries.push("  A          open advanced affinity actions for an idle session");
            }
            if ui.can_mutate_session_binding() {
                entries.extend([
                    "  b/M/E/f    profile / model / effort / fast and service tier",
                    "  Enter      open the effort menu (v0.20.3 compatibility key)",
                    "  l/m/h/X    quickly set low / medium / high / xhigh effort",
                    "  x          clear effort (compatibility key)",
                    "  R          reset the selected session's manual controls",
                ]);
            }
            entries.push("  o          jump to related Requests");
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.extend([
                    "  t          open the full-screen transcript",
                    "  H          jump to History",
                ]);
            }
            entries
        }
        (Language::En, Page::Requests, _) => {
            let mut entries = vec![
                "  ↑/↓        select a request; PgUp/PgDn/Home/End scroll details",
                "  e/c/s      error, control-evidence, and session-scope filters",
                "  x          clear explicit session focus",
                "  o          jump to related Sessions",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  h          jump to History");
            }
            entries
        }
        (Language::En, Page::Stats, _) => vec![
            "  Tab        switch pool / project / provider / endpoint",
            "  ↑/↓        move the active view selection",
            if ui.can_refresh_provider_balances() {
                "  g          force-refresh all balances/quotas and read the new snapshot"
            } else {
                "  g          refresh the observer snapshot only; upstream balances stay unchanged"
            },
            "  y          export and copy the selected report",
        ],
        (Language::En, Page::Settings, _) => settings_help_entries(ui, Language::En),
        (Language::En, Page::History, _) => {
            let mut entries = vec![
                "  ↑/↓        select a session; PgUp/PgDn/Home/End scroll details",
                "  r          refresh the history session list",
                "  t/Enter    open the full-screen transcript",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  s/f        jump to Sessions / Requests");
            }
            entries
        }
        (Language::En, Page::Recent, _) => {
            let mut entries = vec![
                "  ↑/↓        select a session; PgUp/PgDn/Home/End scroll details",
                "  [ / ]      switch the time window",
                "  Enter/y    copy the selected item / visible list",
                "  t/h        open a transcript / jump to History",
            ];
            if ui.can_bridge_runtime_sessions_to_local_codex() {
                entries.push("  s/f        jump to Sessions / Requests");
            }
            entries
        }
        (Language::En, Page::Fleet, _) => vec![
            "  Tab        switch nodes / work units focus",
            "  r          refresh the snapshot; t toggles Tree / Flat",
        ],
        (Language::En, Page::ServiceStatus, _) => {
            vec![
                "  ↑/↓ PgUp/PgDn  move probes; Home/End jumps to either edge",
                "  Tab        switch probe-list / selected-details focus",
                "  ↑/↓ PgUp/PgDn  scroll focused details; Home/End jumps to either edge",
                "  r          read the latest service status snapshot",
            ]
        }
    };
    lines.extend(entries.into_iter().map(Line::from));
    lines.push(Line::from(""));
    lines
}

#[cfg(test)]
pub(super) fn help_text_for_tests(lines: &[Line<'_>]) -> String {
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

fn help_quit_line(lang: Language, attached: bool) -> &'static str {
    match (lang, attached) {
        (Language::Zh, true) => "  q          只退出附着控制台；不停止 resident proxy",
        (Language::Zh, false) => "  q          退出并触发 shutdown",
        (Language::En, true) => {
            "  q          exit attached console only; keep resident proxy running"
        }
        (Language::En, false) => "  q          quit and request shutdown",
    }
}

fn language_help_line(ui: &UiState) -> &'static str {
    match (ui.language, !ui.runtime_connection.is_remote_observer()) {
        (Language::Zh, true) => "  L          切换语言并保存到 config.toml",
        (Language::En, true) => "  L          change language and save it to config.toml",
        (Language::Zh, false) => "  L          仅切换当前 TUI 会话语言",
        (Language::En, false) => "  L          language (current TUI session only)",
    }
}

#[cfg(test)]
pub(super) fn help_quit_line_for_tests(lang: Language, attached: bool) -> &'static str {
    help_quit_line(lang, attached)
}

#[cfg(test)]
pub(super) fn language_help_line_for_tests(ui: &UiState) -> &'static str {
    language_help_line(ui)
}

pub(in crate::tui::view) fn render_help_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let terminal = f.area();
    let area = if terminal.width < 100 || terminal.height < 30 {
        centered_rect(94, 90, terminal)
    } else {
        centered_rect(72, 72, terminal)
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_HELP_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));
    let mut lines = current_page_help_lines(ui, p);
    lines.push(help_heading(
        match ui.language {
            Language::Zh => "通用",
            Language::En => "General",
        },
        p,
    ));
    lines.extend([
        Line::from(match ui.language {
            Language::Zh => "  1-9/0      切换页面",
            Language::En => "  1-9/0      pages",
        }),
        Line::from(language_help_line(ui)),
        Line::from(match ui.language {
            Language::Zh => "  ? / Esc    打开 / 关闭帮助",
            Language::En => "  ? / Esc    open / close help",
        }),
        Line::from(help_quit_line(
            ui.language,
            ui.runtime_connection.is_attached(),
        )),
    ]);
    let inner = block.inner(area);
    let max_scroll = max_wrapped_vertical_scroll(&lines, inner.width, inner.height);
    ui.help_scroll = ui.help_scroll.min(max_scroll);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .scroll((ui.help_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
    if max_scroll > 0 {
        let mut scrollbar =
            ScrollbarState::new(usize::from(max_scroll) + 1).position(usize::from(ui.help_scroll));
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(widget, area, &mut scrollbar);
    }
}
