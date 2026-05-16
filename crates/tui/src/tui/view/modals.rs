use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

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
