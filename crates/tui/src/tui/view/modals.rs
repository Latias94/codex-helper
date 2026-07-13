use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::codex_integration::{CodexStartupReadinessIssue, CodexStartupReadinessSeverity};
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, shorten_middle};
use crate::tui::state::UiState;

use super::widgets::centered_rect;

mod help;
pub(super) use help::render_help_modal;
#[cfg(test)]
use help::{current_page_help_lines, help_quit_line_for_tests, help_text_for_tests};

mod provider_info;
pub(super) use provider_info::render_provider_info_modal;

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
