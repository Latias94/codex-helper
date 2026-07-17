use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::codex_integration::{CodexStartupReadinessIssue, CodexStartupReadinessSeverity};
use crate::proxy::{OperatorEndpointMode, OperatorRoutingCommand, OperatorSessionAffinityCommand};
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, shorten_middle};
use crate::tui::state::UiState;
use crate::tui::types::RoutingActionChoice;

use super::widgets::centered_rect;

mod help;
pub(super) use help::render_help_modal;
#[cfg(test)]
use help::{current_page_help_lines, help_quit_line_for_tests, help_text_for_tests};

mod provider_info;
pub(super) use provider_info::render_provider_info_modal;

pub(super) fn render_routing_actions_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &crate::tui::model::Snapshot,
) {
    let area = centered_rect(66, 52, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            match ui.language {
                Language::Zh => " 路由操作 ",
                Language::En => " Routing action ",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel));
    let selected_target = ui
        .selected_routing_candidate(snapshot)
        .map(|candidate| format!("{}.{}", candidate.provider_id, candidate.endpoint_id))
        .unwrap_or_else(|| "-".to_string());
    let mut lines = vec![
        Line::from(match ui.language {
            Language::Zh => format!("选中端点：{selected_target}"),
            Language::En => format!("Selected endpoint: {selected_target}"),
        }),
        Line::from(""),
    ];
    for (index, action) in RoutingActionChoice::ALL.iter().enumerate() {
        let label = match (ui.language, action) {
            (Language::Zh, RoutingActionChoice::PreferNewSessions) => "设为新会话偏好",
            (Language::En, RoutingActionChoice::PreferNewSessions) => "Prefer for new sessions",
            (Language::Zh, RoutingActionChoice::ClearNewSessionPreference) => "恢复自动调度",
            (Language::En, RoutingActionChoice::ClearNewSessionPreference) => {
                "Restore automatic scheduling"
            }
            (Language::Zh, RoutingActionChoice::EnableEndpoint) => "启用端点",
            (Language::En, RoutingActionChoice::EnableEndpoint) => "Enable endpoint",
            (Language::Zh, RoutingActionChoice::DrainEndpoint) => "排空端点",
            (Language::En, RoutingActionChoice::DrainEndpoint) => "Drain endpoint",
            (Language::Zh, RoutingActionChoice::DisableEndpoint) => "禁用端点",
            (Language::En, RoutingActionChoice::DisableEndpoint) => "Disable endpoint",
        };
        let selected = index == ui.routing_action_selected_idx;
        lines.push(Line::from(Span::styled(
            format!("{} {label}", if selected { ">" } else { " " }),
            Style::default()
                .fg(if selected { p.accent } else { p.text })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text)),
        area,
    );
}

pub(super) fn render_routing_confirmation_modal(f: &mut Frame<'_>, p: Palette, ui: &UiState) {
    let area = centered_rect(74, 48, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            match ui.language {
                Language::Zh => " 确认路由操作 ",
                Language::En => " Confirm routing action ",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.warn))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    match ui
        .routing_confirmation
        .as_ref()
        .map(|confirmation| &confirmation.command)
    {
        Some(OperatorRoutingCommand::SetNewSessionPreference {
            provider_id,
            endpoint_id,
        }) => {
            lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => format!("新会话优先使用 {provider_id}.{endpoint_id}"),
                    Language::En => {
                        format!("Prefer {provider_id}.{endpoint_id} for new sessions")
                    }
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(match ui.language {
                Language::Zh => {
                    "仅影响尚未建立 affinity 的会话；目标不可用时仍按配置策略回退。"
                }
                Language::En => {
                    "Only sessions without affinity are affected; unavailable targets still fall back through the configured policy."
                }
            }));
        }
        Some(OperatorRoutingCommand::ClearNewSessionPreference) => {
            lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => "恢复新会话自动调度",
                    Language::En => "Restore automatic scheduling for new sessions",
                },
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(match ui.language {
                Language::Zh => "已有会话 affinity 保持不变。",
                Language::En => "Existing session affinity remains unchanged.",
            }));
        }
        Some(OperatorRoutingCommand::SetEndpointMode {
            provider_id,
            endpoint_id,
            mode,
        }) => {
            let mode_label = match (ui.language, mode) {
                (Language::Zh, OperatorEndpointMode::Enabled) => "启用",
                (Language::En, OperatorEndpointMode::Enabled) => "Enable",
                (Language::Zh, OperatorEndpointMode::Draining) => "排空",
                (Language::En, OperatorEndpointMode::Draining) => "Drain",
                (Language::Zh, OperatorEndpointMode::Disabled) => "禁用",
                (Language::En, OperatorEndpointMode::Disabled) => "Disable",
            };
            lines.push(Line::from(Span::styled(
                format!("{mode_label} {provider_id}.{endpoint_id}"),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(match (ui.language, mode) {
                (Language::Zh, OperatorEndpointMode::Enabled) => {
                    "允许新会话和已有 affinity 使用此端点。"
                }
                (Language::En, OperatorEndpointMode::Enabled) => {
                    "Allow new sessions and existing affinity to use this endpoint."
                }
                (Language::Zh, OperatorEndpointMode::Draining) => {
                    "新会话将绕过此端点；已有 affinity 仍可继续。"
                }
                (Language::En, OperatorEndpointMode::Draining) => {
                    "New sessions bypass this endpoint; existing affinity may continue."
                }
                (Language::Zh, OperatorEndpointMode::Disabled) => {
                    "后续新会话和已有 affinity 都会被阻断；进行中的请求不被中断。"
                }
                (Language::En, OperatorEndpointMode::Disabled) => {
                    "Future new and affinity-bound requests are blocked; in-flight requests are not interrupted."
                }
            }));
        }
        None => {
            lines.push(Line::from(match ui.language {
                Language::Zh => "没有待确认的路由变更。",
                Language::En => "There is no routing change to confirm.",
            }));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::Zh => "Enter / y 确认    Esc / n 取消",
            Language::En => "Enter / y confirm    Esc / n cancel",
        },
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
    )));

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn render_session_affinity_actions_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &crate::tui::model::Snapshot,
) {
    let area = centered_rect(76, 68, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            match ui.language {
                Language::Zh => " 会话 affinity 高级操作 ",
                Language::En => " Advanced session affinity action ",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.accent))
        .style(Style::default().bg(p.panel));

    let selected_session = snapshot.rows.get(ui.selected_session_idx);
    let session_key = selected_session
        .and_then(|row| row.session_id.as_deref())
        .map(|value| shorten_middle(value, 42))
        .unwrap_or_else(|| "-".to_string());
    let current_target = selected_session
        .and_then(|row| row.route_affinity.as_ref())
        .map(|affinity| format!("{}.{}", affinity.provider_id, affinity.endpoint_id))
        .unwrap_or_else(|| "-".to_string());
    let mut lines = vec![
        Line::from(match ui.language {
            Language::Zh => format!("会话：{session_key}"),
            Language::En => format!("Session: {session_key}"),
        }),
        Line::from(match ui.language {
            Language::Zh => format!("当前绑定：{current_target}"),
            Language::En => format!("Current binding: {current_target}"),
        }),
        Line::from(""),
        Line::from(Span::styled(
            match ui.language {
                Language::Zh => "仅用于故障恢复：只允许空闲会话；服务端会再次校验。",
                Language::En => {
                    "Recovery only: the session must be idle; the daemon validates again."
                }
            },
            Style::default().fg(p.muted),
        )),
        Line::from(""),
    ];

    let clear_selected = ui.session_affinity_action_selected_idx == 0;
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::Zh => format!(
                "{} 清除 affinity（有状态请求可能被拒绝）",
                if clear_selected { ">" } else { " " }
            ),
            Language::En => format!(
                "{} Clear affinity (state-bound requests may be rejected)",
                if clear_selected { ">" } else { " " }
            ),
        },
        Style::default()
            .fg(if clear_selected { p.warn } else { p.text })
            .add_modifier(if clear_selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )));

    if let Some(routing) = snapshot.routing.as_ref()
        && routing.entry_strategy != crate::config::RouteStrategy::Conditional
    {
        let candidate_line_budget = usize::from(area.height.saturating_sub(10));
        let show_window_status = routing.candidates.len() > candidate_line_budget;
        let candidate_rows = candidate_line_budget
            .saturating_sub(usize::from(show_window_status))
            .max(1);
        let (start, end) = session_affinity_candidate_window(
            routing.candidates.len(),
            ui.session_affinity_action_selected_idx,
            candidate_rows,
        );
        for (index, candidate) in routing.candidates.iter().enumerate().take(end).skip(start) {
            let selected = ui.session_affinity_action_selected_idx == index + 1;
            let target = format!("{}.{}", candidate.provider_id, candidate.endpoint_id);
            let current = (target == current_target).then_some(match ui.language {
                Language::Zh => "（当前）",
                Language::En => " (current)",
            });
            lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => format!(
                        "{} 重新绑定到 {target}{}",
                        if selected { ">" } else { " " },
                        current.unwrap_or_default()
                    ),
                    Language::En => format!(
                        "{} Rebind to {target}{}",
                        if selected { ">" } else { " " },
                        current.unwrap_or_default()
                    ),
                },
                Style::default()
                    .fg(if selected { p.accent } else { p.text })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )));
        }
        if show_window_status {
            lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => format!(
                        "  候选端点 {}-{} / {}（↑/↓ 滚动）",
                        start + 1,
                        end,
                        routing.candidates.len()
                    ),
                    Language::En => format!(
                        "  Candidates {}-{} of {} (↑/↓ scroll)",
                        start + 1,
                        end,
                        routing.candidates.len()
                    ),
                },
                Style::default().fg(p.muted),
            )));
        }
    } else if snapshot
        .routing
        .as_ref()
        .is_some_and(|routing| routing.entry_strategy == crate::config::RouteStrategy::Conditional)
    {
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "条件路由只能清除 affinity；重新绑定需要具体请求上下文。",
                Language::En => {
                    "Conditional routes only allow Clear; rebind requires request context."
                }
            },
            Style::default().fg(p.muted),
        )));
    }

    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            match ui.language {
                Language::Zh => {
                    "跨端点 Rebind 仅限相同的显式 continuity domain，且目标当前必须可用。"
                }
                Language::En => {
                    "Cross-endpoint rebind requires the same explicit continuity domain and an available target."
                }
            },
            Style::default().fg(p.muted),
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

fn session_affinity_candidate_window(
    total: usize,
    selected_action_index: usize,
    max_rows: usize,
) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let rows = max_rows.max(1).min(total);
    let selected_candidate = selected_action_index
        .saturating_sub(1)
        .min(total.saturating_sub(1));
    let start = selected_candidate
        .saturating_sub(rows / 2)
        .min(total.saturating_sub(rows));
    (start, start + rows)
}

pub(super) fn render_session_affinity_confirmation_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &crate::tui::model::Snapshot,
) {
    let area = centered_rect(78, 56, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            match ui.language {
                Language::Zh => " 确认会话 affinity 操作 ",
                Language::En => " Confirm session affinity action ",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.warn))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if let Some(confirmation) = ui.session_affinity_confirmation.as_ref() {
        let session_key = shorten_middle(&confirmation.session_key, 48);
        lines.push(Line::from(match ui.language {
            Language::Zh => format!("会话：{session_key}"),
            Language::En => format!("Session: {session_key}"),
        }));
        lines.push(Line::from(""));
        match &confirmation.command {
            OperatorSessionAffinityCommand::Clear => {
                lines.push(Line::from(Span::styled(
                    match ui.language {
                        Language::Zh => "清除此会话的持久 affinity",
                        Language::En => "Clear this session's durable affinity",
                    },
                    Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(match ui.language {
                    Language::Zh => {
                        "后续 state-bound / hard 请求可能因缺少 affinity 而被拒绝。"
                    }
                    Language::En => {
                        "Later state-bound / hard requests may be rejected because affinity is missing."
                    }
                }));
                lines.push(Line::from(Span::styled(
                    match ui.language {
                        Language::Zh => "Clear 不选择替代端点；下一次合格请求会重新执行当前路由策略。",
                        Language::En => {
                            "Clear does not choose a replacement; the next eligible request reruns current routing policy."
                        }
                    },
                    Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(match ui.language {
                    Language::Zh => {
                        "若现有 WebSocket 重新选中另一端点，会先要求重连，不会写入旧上游。"
                    }
                    Language::En => {
                        "If an existing WebSocket selects another endpoint, it requires reconnect before any old-upstream write."
                    }
                }));
            }
            OperatorSessionAffinityCommand::Rebind {
                provider_id,
                endpoint_id,
            } => {
                let current_target = snapshot
                    .rows
                    .iter()
                    .find(|row| row.session_id.as_deref() == Some(&confirmation.session_key))
                    .and_then(|row| row.route_affinity.as_ref())
                    .map(|affinity| format!("{}.{}", affinity.provider_id, affinity.endpoint_id))
                    .unwrap_or_else(|| "-".to_string());
                let target = format!("{provider_id}.{endpoint_id}");
                lines.push(Line::from(Span::styled(
                    match ui.language {
                        Language::Zh => format!("将会话从 {current_target} 重新绑定到 {target}"),
                        Language::En => {
                            format!("Rebind the session from {current_target} to {target}")
                        }
                    },
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(match ui.language {
                    Language::Zh => {
                        "仅空闲会话可执行；跨端点时必须属于相同的显式 continuity domain。"
                    }
                    Language::En => {
                        "The session must be idle; cross-endpoint targets must share an explicit continuity domain."
                    }
                }));
                lines.push(Line::from(match ui.language {
                    Language::Zh => "目标还必须存在且当前可用；否则 daemon 会拒绝操作。",
                    Language::En => {
                        "The target must also exist and be currently available, or the daemon rejects the action."
                    }
                }));
            }
        }
    } else {
        lines.push(Line::from(match ui.language {
            Language::Zh => "没有待确认的会话 affinity 变更。",
            Language::En => "There is no session affinity change to confirm.",
        }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::Zh => "Enter / y 确认    Esc / n 取消",
            Language::En => "Enter / y confirm    Esc / n cancel",
        },
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
    )));

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}

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
        "Codex switch status is unavailable" => StartupIssueCopy {
            title: "Codex switch 状态不可用".to_string(),
            detail: issue.detail.clone(),
            action: "修改 Codex 配置前，请先检查 helper 自有的 switch journal。".to_string(),
        },
        "Codex is not switched to this proxy" => StartupIssueCopy {
            title: "Codex 尚未切换到当前代理".to_string(),
            detail: issue.detail.clone(),
            action: issue
                .action
                .replace(
                    "Run `codex-helper switch on",
                    "运行 `codex-helper switch on",
                )
                .replace(
                    " explicitly before starting a Codex client.",
                    "，然后再启动 Codex 客户端。",
                ),
        },
        "Codex switch requires recovery" => StartupIssueCopy {
            title: "Codex switch 需要恢复".to_string(),
            detail: issue.detail.clone(),
            action: "不要覆盖 Codex 配置；请先核对 journal 中记录的文件指纹。".to_string(),
        },
        "Codex switch operation is incomplete" => StartupIssueCopy {
            title: "Codex switch 操作未完成".to_string(),
            detail: issue.detail.clone(),
            action: issue
                .action
                .replace("Retry `codex-helper", "重试 `codex-helper")
                .replace(" or run", "，或运行"),
        },
        "Codex switch state is inconsistent" => StartupIssueCopy {
            title: "Codex switch 状态不一致".to_string(),
            detail: issue.detail.clone(),
            action: "运行 `codex-helper switch status`，核对配置后再继续。".to_string(),
        },
        "Codex points to a different helper endpoint" => StartupIssueCopy {
            title: "Codex 指向另一个 helper 端点".to_string(),
            detail: issue.detail.clone(),
            action: issue
                .action
                .replace("Run `codex-helper", "运行 `codex-helper")
                .replace(", then", "，然后")
                .replace(", or serve", "，或在")
                .replace(" at the configured endpoint.", " 配置的端点上启动服务。"),
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

    let inner = block.inner(area);
    let max_scroll = transcript_max_scroll(&lines, inner.width, inner.height);
    ui.session_transcript_scroll = ui.session_transcript_scroll.min(max_scroll);

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .scroll((ui.session_transcript_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

fn transcript_max_scroll(lines: &[Line<'_>], text_width: u16, viewport_height: u16) -> u16 {
    if text_width == 0 || viewport_height == 0 {
        return 0;
    }
    wrapped_visual_line_count(lines, usize::from(text_width))
        .saturating_sub(usize::from(viewport_height))
        .min(usize::from(u16::MAX)) as u16
}

fn wrapped_visual_line_count(lines: &[Line<'_>], text_width: usize) -> usize {
    if text_width == 0 {
        return 0;
    }

    lines
        .iter()
        .map(|line| {
            let width = line.width();
            if width == 0 {
                1
            } else {
                width.div_ceil(text_width)
            }
        })
        .sum()
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

#[cfg(test)]
mod tests;
