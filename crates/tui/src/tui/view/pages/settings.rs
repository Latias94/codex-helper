use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use codex_helper_core::codex_switch::{self, CodexSwitchPhase, CodexSwitchStatus};

use crate::config::RetryProfileName;
use crate::dashboard_core::{OperatorReadStatus, OperatorRetrySummary};
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, Snapshot, shorten, shorten_middle};
use crate::tui::state::UiState;

fn retry_profile_name(profile: Option<RetryProfileName>) -> &'static str {
    match profile {
        Some(RetryProfileName::Balanced) => "balanced",
        Some(RetryProfileName::SameUpstream) => "same-upstream",
        Some(RetryProfileName::AggressiveFailover) => "aggressive-failover",
        Some(RetryProfileName::CostPrimary) => "cost-primary",
        None => "default",
    }
}

fn read_status_label(status: OperatorReadStatus, language: Language) -> &'static str {
    match (status, language) {
        (OperatorReadStatus::Ready, Language::Zh) => "就绪",
        (OperatorReadStatus::Stale, Language::Zh) => "陈旧",
        (OperatorReadStatus::Disconnected, Language::Zh) => "离线",
        (OperatorReadStatus::AuthRequired, Language::Zh) => "需要认证",
        (OperatorReadStatus::Ready, Language::En) => "ready",
        (OperatorReadStatus::Stale, Language::En) => "stale",
        (OperatorReadStatus::Disconnected, Language::En) => "offline",
        (OperatorReadStatus::AuthRequired, Language::En) => "auth required",
    }
}

fn codex_switch_status_lines(
    p: Palette,
    language: Language,
    status: &CodexSwitchStatus,
) -> Vec<Line<'static>> {
    let yes_no = |value| match (language, value) {
        (Language::Zh, true) => "是",
        (Language::Zh, false) => "否",
        (Language::En, true) => "yes",
        (Language::En, false) => "no",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("  phase: ", Style::default().fg(p.muted)),
        Span::styled(
            status.phase.as_str(),
            Style::default().fg(if status.phase == CodexSwitchPhase::RecoveryRequired {
                p.warn
            } else {
                p.text
            }),
        ),
        Span::styled("  enabled: ", Style::default().fg(p.muted)),
        Span::styled(yes_no(status.enabled), Style::default().fg(p.text)),
        Span::styled("  managed: ", Style::default().fg(p.muted)),
        Span::styled(yes_no(status.managed), Style::default().fg(p.text)),
    ])];
    lines.push(Line::from(vec![
        Span::styled("  base_url: ", Style::default().fg(p.muted)),
        Span::styled(
            status.base_url.as_deref().unwrap_or("-").to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    if let Some(reason) = status.recovery_reason.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("  recovery: ", Style::default().fg(p.warn)),
            Span::styled(reason.to_string(), Style::default().fg(p.warn)),
        ]));
    }
    lines
}

fn retry_lines(retry: &OperatorRetrySummary, language: Language) -> Vec<Line<'static>> {
    let configured = match language {
        Language::Zh => format!(
            "  配置：profile={} upstream_attempts={} provider_attempts={}",
            retry_profile_name(retry.configured_profile),
            retry.upstream_max_attempts,
            retry.provider_max_attempts
        ),
        Language::En => format!(
            "  configured: profile={} upstream_attempts={} provider_attempts={}",
            retry_profile_name(retry.configured_profile),
            retry.upstream_max_attempts,
            retry.provider_max_attempts
        ),
    };
    let observed = match language {
        Language::Zh => format!(
            "  近期观测：retried={} same_provider={} cross_provider={} fast={}",
            retry.recent_retried_requests,
            retry.recent_same_provider_retries,
            retry.recent_cross_provider_failovers,
            retry.recent_fast_mode_requests
        ),
        Language::En => format!(
            "  observed: retried={} same_provider={} cross_provider={} fast={}",
            retry.recent_retried_requests,
            retry.recent_same_provider_retries,
            retry.recent_cross_provider_failovers,
            retry.recent_fast_mode_requests
        ),
    };
    vec![Line::from(configured), Line::from(observed)]
}

pub(super) fn render_settings_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    _snapshot: &Snapshot,
    area: Rect,
) {
    let mut lines = Vec::new();
    let status = ui.operator_read_model.as_ref().map(|model| model.status);

    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => "只读运行态",
            Language::En => "Read-only runtime",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            match ui.language {
                Language::Zh => "  连接：",
                Language::En => "  connection: ",
            },
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_connection.label(ui.language),
            Style::default().fg(p.text),
        ),
        Span::styled(
            match ui.language {
                Language::Zh => "  bundle：",
                Language::En => "  bundle: ",
            },
            Style::default().fg(p.muted),
        ),
        Span::styled(
            status
                .map(|status| read_status_label(status, ui.language))
                .unwrap_or("-"),
            Style::default().fg(match status {
                Some(OperatorReadStatus::Ready) => p.good,
                Some(OperatorReadStatus::Stale) => p.warn,
                Some(OperatorReadStatus::Disconnected | OperatorReadStatus::AuthRequired) => p.bad,
                None => p.muted,
            }),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  loaded_at_ms: ", Style::default().fg(p.muted)),
        Span::styled(
            ui.last_runtime_config_loaded_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            Style::default().fg(p.text),
        ),
        Span::styled("  source_mtime_ms: ", Style::default().fg(p.muted)),
        Span::styled(
            ui.last_runtime_config_source_mtime_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            Style::default().fg(p.text),
        ),
    ]));
    if let Some(error) = ui.runtime_status_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("  {}", shorten(error, 120)),
            Style::default().fg(p.warn),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::Zh => "重试摘要",
            Language::En => "Retry summary",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    if let Some(retry) = ui.last_retry_summary.as_ref() {
        lines.extend(retry_lines(retry, ui.language));
    } else {
        lines.push(Line::from(Span::styled(
            "  -",
            Style::default().fg(p.muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        i18n::text(ui.language, msg::PROFILE_CONTROL_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::CONFIGURED_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.configured_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            i18n::text(ui.language, msg::EFFECTIVE_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.effective_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
    ]));
    let profiles = if ui.profile_options.is_empty() {
        i18n::text(ui.language, msg::NO_PROFILES).to_string()
    } else {
        shorten_middle(
            &ui.profile_options
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            110,
        )
    };
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::AVAILABLE_PROFILES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(profiles, Style::default().fg(p.text)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        i18n::text(ui.language, msg::TUI_OPTIONS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            i18n::language_name(ui.language),
            Style::default().fg(p.text),
        ),
        Span::styled(
            match ui.language {
                Language::Zh => "  （L 仅切换当前 TUI 会话）",
                Language::En => "  (L changes this TUI session only)",
            },
            Style::default().fg(p.muted),
        ),
    ]));

    if ui.service_name == "codex" {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "Codex 本地 Switch",
                Language::En => "Codex Local Switch",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        match codex_switch::inspect() {
            Ok(status) => lines.extend(codex_switch_status_lines(p, ui.language, &status)),
            Err(error) => lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => format!("  读取状态失败：{error}"),
                    Language::En => format!("  read status failed: {error}"),
                },
                Style::default().fg(p.warn),
            ))),
        }
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "  n/o 显式开启/关闭本地 switch；已有 Codex app 需重启后生效。",
                Language::En => {
                    "  n/o explicitly switch the local Codex target on/off; restart existing Codex apps to apply it."
                }
            },
            Style::default().fg(p.muted),
        )));
    }

    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::PAGE_SETTINGS),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::{read_status_label, retry_profile_name};
    use crate::config::RetryProfileName;
    use crate::dashboard_core::OperatorReadStatus;
    use crate::tui::Language;

    #[test]
    fn settings_labels_preserve_explicit_read_states() {
        assert_eq!(
            read_status_label(OperatorReadStatus::Ready, Language::En),
            "ready"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::Stale, Language::En),
            "stale"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::Disconnected, Language::En),
            "offline"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::AuthRequired, Language::En),
            "auth required"
        );
    }

    #[test]
    fn settings_retry_summary_uses_the_typed_profile() {
        assert_eq!(
            retry_profile_name(Some(RetryProfileName::AggressiveFailover)),
            "aggressive-failover"
        );
    }
}
