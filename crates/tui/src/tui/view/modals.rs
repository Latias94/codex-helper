use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::codex_integration::{CodexStartupReadinessIssue, CodexStartupReadinessSeverity};
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, provider_tags_brief, shorten_middle,
    station_balance_brief_lang, station_primary_balance_snapshot,
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

mod station_info;
pub(super) use station_info::render_station_info_modal;

struct StartupIssueCopy {
    title: String,
    detail: String,
    action: String,
}

fn startup_issue_copy(issue: &CodexStartupReadinessIssue, lang: Language) -> StartupIssueCopy {
    if lang == Language::En {
        return StartupIssueCopy {
            title: issue.title.clone(),
            detail: issue.detail.clone(),
            action: issue.action.clone(),
        };
    }

    match issue.title.as_str() {
        "Codex client config changed on startup" => StartupIssueCopy {
            title: "Codex 客户端配置已在启动时变更".to_string(),
            detail: "codex-helper 已为本地桥接更新 ~/.codex/config.toml 或 ~/.codex/auth.json。"
                .to_string(),
            action: "完整重启已打开的 Codex App、Codex TUI 或 codex exec 会话，让它重新读取客户端配置。"
                .to_string(),
        },
        "Codex local proxy patch failed" => StartupIssueCopy {
            title: "Codex 本地代理 patch 失败".to_string(),
            detail: issue.detail.clone(),
            action: "运行 `codex-helper switch status`，先修复 Codex 客户端配置问题。"
                .to_string(),
        },
        "Could not inspect Codex switch status" => StartupIssueCopy {
            title: "无法检查 Codex switch 状态".to_string(),
            detail: issue.detail.clone(),
            action: "在普通 shell 中运行 `codex-helper switch status` 检查客户端配置。"
                .to_string(),
        },
        "Could not inspect Codex remote-control status" => StartupIssueCopy {
            title: "无法检查 Codex 远程控制状态".to_string(),
            detail: issue.detail.clone(),
            action: "在普通 shell 中运行 `codex-helper switch remote-control status` 检查桌面端状态。"
                .to_string(),
        },
        "Codex is not using the local helper" => StartupIssueCopy {
            title: "Codex 尚未使用本地 helper".to_string(),
            detail: issue.detail.clone(),
            action: issue
                .action
                .replace("Run `codex-helper switch on", "运行 `codex-helper switch on")
                .replace(
                    "or restart codex-helper so the client patch can be applied.",
                    "或重启 codex-helper，让客户端 patch 生效。",
                ),
        },
        "Codex local proxy patch has no switch state" => StartupIssueCopy {
            title: "Codex 本地代理 patch 缺少 switch 状态".to_string(),
            detail: "Codex 已指向本地 helper，但 codex-helper 找不到用于恢复的元数据。"
                .to_string(),
            action: "执行 switch-off 操作前，请先检查 ~/.codex/config.toml。".to_string(),
        },
        "Codex local proxy port does not match this TUI" => StartupIssueCopy {
            title: "Codex 本地代理端口与当前 TUI 不一致".to_string(),
            detail: issue.detail.clone(),
            action: issue
                .action
                .replace("Run `codex-helper switch on", "运行 `codex-helper switch on")
                .replace(
                    "or restart this helper instance on the configured port.",
                    "或用配置端口重启当前 helper 实例。",
                ),
        },
        "Codex bridge mode does not match helper config" => StartupIssueCopy {
            title: "Codex 桥接模式与 helper 配置不一致".to_string(),
            detail: issue.detail.clone(),
            action: "运行 `codex-helper switch status`；如果刚切换过模式，请完整重启 Codex 客户端。"
                .to_string(),
        },
        "Official relay bridge can route a session across providers" => StartupIssueCopy {
            title: "官方 relay 桥接可能把同一会话路由到不同 provider".to_string(),
            detail: issue.detail.clone(),
            action: "多认证上游使用官方 relay 功能时，建议把 [codex.routing].affinity_policy 设为 \"fallback-sticky\" 或 \"hard\"，让 remote compaction 更接近官方体验。"
                .to_string(),
        },
        "Could not inspect codex-helper routing affinity" => StartupIssueCopy {
            title: "无法检查 codex-helper 路由粘性配置".to_string(),
            detail: issue.detail.clone(),
            action: "检查 ~/.codex-helper/config.toml，并为官方 relay 功能选择合适的 affinity_policy。"
                .to_string(),
        },
        "Removed remote_control config key is present" => StartupIssueCopy {
            title: "检测到已移除的 remote_control 配置项".to_string(),
            detail: issue.detail.clone(),
            action: "移除 remote_control，只保留 [features].remote_connections = true。"
                .to_string(),
        },
        "Codex App remote-control state is incomplete" => StartupIssueCopy {
            title: "Codex App 远程控制状态不完整".to_string(),
            detail: issue.detail.clone(),
            action: "运行 `codex-helper switch remote-control enable`，然后完整重启 Codex App。"
                .to_string(),
        },
        "Remote-control enablement is not confirmed in Codex logs" => StartupIssueCopy {
            title: "未在 Codex 日志中确认远程控制启用".to_string(),
            detail: "配置和 SQLite 状态看起来已启用，但没有找到 experimentalFeature/enablement/set 成功日志。"
                .to_string(),
            action: "完整重启 Codex App，然后运行 `codex-helper switch remote-control check-logs`。"
                .to_string(),
        },
        "Could not inspect Codex remote-control logs" => StartupIssueCopy {
            title: "无法检查 Codex 远程控制日志".to_string(),
            detail: issue.detail.clone(),
            action: "重启 Codex App 后，在普通 shell 中运行 `codex-helper switch remote-control check-logs`。"
                .to_string(),
        },
        _ => StartupIssueCopy {
            title: issue.title.clone(),
            detail: issue.detail.clone(),
            action: issue.action.clone(),
        },
    }
}

fn startup_severity_label(severity: CodexStartupReadinessSeverity, lang: Language) -> &'static str {
    match lang {
        Language::En => severity.label(),
        Language::Zh => match severity {
            CodexStartupReadinessSeverity::Info => "信息",
            CodexStartupReadinessSeverity::Warning => "警告",
        },
    }
}

fn startup_hidden_issue_line(hidden: usize, lang: Language) -> String {
    match lang {
        Language::En => {
            format!("+{hidden} more startup item(s); run `codex-helper switch status` for details.")
        }
        Language::Zh => {
            format!("还有 {hidden} 个启动检查项；运行 `codex-helper switch status` 查看详情。")
        }
    }
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

pub(super) fn render_startup_alert_modal(f: &mut Frame<'_>, p: Palette, ui: &UiState) {
    let area = centered_rect(76, 66, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::OVERLAY_STARTUP_GUARDRAIL),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::En => {
                "Review these Codex client state items before relying on this TUI session."
            }
            Language::Zh => "继续使用本次 TUI 会话前，请先检查这些 Codex 客户端状态项。",
        },
        Style::default().fg(p.muted),
    )));
    lines.push(Line::from(""));

    let Some(report) = ui.startup_readiness.as_ref() else {
        lines.push(Line::from(Span::styled(
            i18n::label(ui.language, "No startup issues are currently recorded."),
            Style::default().fg(p.text),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::FOOTER_STARTUP_GUARDRAIL),
            Style::default().fg(p.muted),
        )));
        let content = Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false });
        f.render_widget(content, area);
        return;
    };

    let max_visible = 5;
    for (idx, issue) in report.issues.iter().take(max_visible).enumerate() {
        let issue_copy = startup_issue_copy(issue, ui.language);
        let severity_style = match issue.severity {
            CodexStartupReadinessSeverity::Info => Style::default().fg(p.accent),
            CodexStartupReadinessSeverity::Warning => Style::default().fg(p.warn),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{}. [{}] ",
                    idx + 1,
                    startup_severity_label(issue.severity, ui.language)
                ),
                severity_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                issue_copy.title,
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            issue_copy.detail,
            Style::default().fg(p.text),
        )));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", i18n::label(ui.language, "next")),
                Style::default().fg(p.muted),
            ),
            Span::styled(issue_copy.action, Style::default().fg(p.accent)),
        ]));
        lines.push(Line::from(""));
    }

    let hidden = report.issues.len().saturating_sub(max_visible);
    if hidden > 0 {
        lines.push(Line::from(Span::styled(
            startup_hidden_issue_line(hidden, ui.language),
            Style::default().fg(p.muted),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        i18n::text(ui.language, msg::FOOTER_STARTUP_GUARDRAIL),
        Style::default().fg(p.muted),
    )));

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
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
