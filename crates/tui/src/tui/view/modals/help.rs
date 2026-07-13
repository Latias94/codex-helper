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
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    ))
}

fn help_current_page_title(lang: Language, page: Page, routing: bool) -> &'static str {
    match (lang, page, routing) {
        (Language::Zh, Page::Dashboard, _) => "当前页面：总览",
        (Language::Zh, Page::Stations, true) => "当前页面：路由",
        (Language::Zh, Page::Stations, false) => "当前页面：站点",
        (Language::Zh, Page::Sessions, _) => "当前页面：会话",
        (Language::Zh, Page::Requests, _) => "当前页面：请求",
        (Language::Zh, Page::Stats, _) => "当前页面：用量",
        (Language::Zh, Page::Settings, _) => "当前页面：设置",
        (Language::Zh, Page::History, _) => "当前页面：历史",
        (Language::Zh, Page::Recent, _) => "当前页面：最近",
        (Language::Zh, Page::Fleet, _) => "当前页面：Fleet",
        (Language::Zh, Page::ServiceStatus, _) => "当前页面：服务状态",
        (Language::En, Page::Dashboard, _) => "Current page: Dashboard",
        (Language::En, Page::Stations, true) => "Current page: Routing",
        (Language::En, Page::Stations, false) => "Current page: Stations",
        (Language::En, Page::Sessions, _) => "Current page: Sessions",
        (Language::En, Page::Requests, _) => "Current page: Requests",
        (Language::En, Page::Stats, _) => "Current page: Usage",
        (Language::En, Page::Settings, _) => "Current page: Settings",
        (Language::En, Page::History, _) => "Current page: History",
        (Language::En, Page::Recent, _) => "Current page: Recent",
        (Language::En, Page::Fleet, _) => "Current page: Fleet",
        (Language::En, Page::ServiceStatus, _) => "Current page: Service Status",
    }
}

pub(super) fn current_page_help_lines(
    lang: Language,
    page: Page,
    routing: bool,
    codex: bool,
    p: Palette,
) -> Vec<Line<'static>> {
    let mut lines = vec![help_heading(
        help_current_page_title(lang, page, routing),
        p,
    )];
    let entries = match (lang, page, codex) {
        (Language::Zh, Page::Dashboard, _) => vec![
            "  Tab        切换会话/请求焦点",
            "  ↑/↓        移动当前选择",
            "  O/H o/h    跳到关联请求、会话或历史",
        ],
        (Language::Zh, Page::Stations, _) => vec![
            "  ↑/↓        移动 provider 选择",
            "  i          查看只读 provider 详情",
        ],
        (Language::Zh, Page::Sessions, _) => vec![
            "  a/e        活跃、错误筛选；r 重置筛选",
            "  t          打开全屏对话记录",
            "  o/H        跳到 Requests / History",
        ],
        (Language::Zh, Page::Requests, _) => vec![
            "  e/c/s      错误、控制证据与会话范围筛选",
            "  x          清除显式 session 聚焦",
            "  o/h        跳到 Sessions / History",
        ],
        (Language::Zh, Page::Stats, _) => vec![
            "  Tab        切换 endpoint / provider 今日排行",
            "  ↑/↓        移动当前排行选择",
            "  y          导出并复制选中报告",
        ],
        (Language::Zh, Page::Settings, true) => vec![
            "  n/o        显式开启/关闭 Codex 本地 switch",
            "  页面       展示 operator bundle、重试和 profile 只读事实",
        ],
        (Language::Zh, Page::Settings, false) => {
            vec!["  页面       展示 operator bundle、重试和 profile 只读事实"]
        }
        (Language::Zh, Page::History, _) => vec![
            "  r          刷新历史会话列表",
            "  t/Enter    打开全屏对话记录",
            "  s/f        跳到 Sessions / Requests",
        ],
        (Language::Zh, Page::Recent, _) => vec![
            "  [ / ]      切换时间窗口",
            "  Enter/y    复制选中项 / 复制可见列表",
            "  t/s/f/h    打开记录或跳到关联页面",
        ],
        (Language::Zh, Page::Fleet, _) => vec![
            "  Tab        切换节点 / 工作单元焦点",
            "  r          刷新快照；t 切换 Tree / Flat",
        ],
        (Language::Zh, Page::ServiceStatus, _) => vec!["  r          刷新只读服务状态快照"],
        (Language::En, Page::Dashboard, _) => vec![
            "  Tab        switch Sessions / Requests focus",
            "  ↑/↓        move the active selection",
            "  O/H o/h    jump to related requests, sessions, or history",
        ],
        (Language::En, Page::Stations, _) => vec![
            "  ↑/↓        move the provider selection",
            "  i          inspect read-only provider details",
        ],
        (Language::En, Page::Sessions, _) => vec![
            "  a/e        active and error filters; r resets",
            "  t          open the full-screen transcript",
            "  o/H        jump to Requests / History",
        ],
        (Language::En, Page::Requests, _) => vec![
            "  e/c/s      error, control-evidence, and session-scope filters",
            "  x          clear explicit session focus",
            "  o/h        jump to Sessions / History",
        ],
        (Language::En, Page::Stats, _) => vec![
            "  Tab        switch endpoint / provider day ranking",
            "  ↑/↓        move the active ranking selection",
            "  y          export and copy the selected report",
        ],
        (Language::En, Page::Settings, true) => vec![
            "  n/o        explicitly switch the local Codex target on/off",
            "  page       shows read-only operator bundle, retry, and profile facts",
        ],
        (Language::En, Page::Settings, false) => {
            vec!["  page       shows read-only operator bundle, retry, and profile facts"]
        }
        (Language::En, Page::History, _) => vec![
            "  r          refresh the history session list",
            "  t/Enter    open the full-screen transcript",
            "  s/f        jump to Sessions / Requests",
        ],
        (Language::En, Page::Recent, _) => vec![
            "  [ / ]      switch the time window",
            "  Enter/y    copy the selected item / visible list",
            "  t/s/f/h    open a transcript or jump to a related page",
        ],
        (Language::En, Page::Fleet, _) => vec![
            "  Tab        switch nodes / work units focus",
            "  r          refresh the snapshot; t toggles Tree / Flat",
        ],
        (Language::En, Page::ServiceStatus, _) => {
            vec!["  r          refresh the read-only service status snapshot"]
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
        (Language::Zh, false) => "  q          退出控制台；不请求停止 runtime",
        (Language::En, true) => {
            "  q          exit attached console only; keep resident proxy running"
        }
        (Language::En, false) => "  q          exit console and keep the runtime running",
    }
}

#[cfg(test)]
pub(super) fn help_quit_line_for_tests(lang: Language, attached: bool) -> &'static str {
    help_quit_line(lang, attached)
}

pub(in crate::tui::view) fn render_help_modal(f: &mut Frame<'_>, p: Palette, ui: &UiState) {
    let area = centered_rect(72, 72, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_HELP_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));
    let mut lines = current_page_help_lines(
        ui.language,
        ui.page,
        ui.uses_route_graph_routing(),
        ui.service_name == "codex",
        p,
    );
    lines.push(help_heading(
        match ui.language {
            Language::Zh => "通用",
            Language::En => "General",
        },
        p,
    ));
    lines.extend([
        Line::from("  1-9/0      pages"),
        Line::from("  L          language (current TUI session only)"),
        Line::from("  ? / Esc    open / close help"),
        Line::from(help_quit_line(
            ui.language,
            ui.runtime_connection.is_attached(),
        )),
    ]);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}
