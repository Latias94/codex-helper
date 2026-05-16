use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::Palette;
use crate::tui::state::UiState;
use crate::tui::types::Page;
use crate::tui::view::widgets::centered_rect;
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

pub(super) fn current_page_help_lines(
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

pub(in crate::tui::view) fn render_help_modal(f: &mut Frame<'_>, p: Palette, ui: &UiState) {
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
