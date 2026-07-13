use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{Palette, Snapshot, provider_balance_brief_lang, shorten_middle};
use crate::tui::state::UiState;
use crate::tui::view::widgets::centered_rect;

pub(in crate::tui::view) fn render_station_info_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
) {
    let area = centered_rect(76, 78, f.area());
    f.render_widget(Clear, area);
    let selected = providers.get(ui.selected_station_idx);
    let title = selected
        .map(|provider| {
            format!(
                "{}: {}",
                i18n::label(ui.language, "Provider details"),
                provider.name
            )
        })
        .unwrap_or_else(|| i18n::label(ui.language, "Provider details").to_string());
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if let Some(provider) = selected {
        lines.push(Line::from(vec![
            Span::styled("provider: ", Style::default().fg(p.muted)),
            Span::styled(provider.name.clone(), Style::default().fg(p.text)),
            Span::styled("  configured: ", Style::default().fg(p.muted)),
            Span::styled(
                provider.configured_enabled.to_string(),
                Style::default().fg(if provider.configured_enabled {
                    p.good
                } else {
                    p.warn
                }),
            ),
            Span::styled("  effective: ", Style::default().fg(p.muted)),
            Span::styled(
                provider.effective_enabled.to_string(),
                Style::default().fg(if provider.effective_enabled {
                    p.good
                } else {
                    p.warn
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("alias: ", Style::default().fg(p.muted)),
            Span::styled(
                provider.alias.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
            Span::styled("  routable endpoints: ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}/{}",
                    provider.routable_endpoints,
                    provider.endpoints.len()
                ),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("balance/quota: ", Style::default().fg(p.muted)),
            Span::styled(
                provider_balance_brief_lang(
                    &snapshot.provider_balances,
                    &provider.name,
                    88,
                    ui.language,
                ),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            i18n::label(ui.language, "Endpoints"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        if provider.endpoints.is_empty() {
            lines.push(Line::from(Span::styled(
                "  -",
                Style::default().fg(p.muted),
            )));
        } else {
            for (index, endpoint) in provider.endpoints.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {index:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(endpoint.name.clone(), Style::default().fg(p.text)),
                    Span::raw("  "),
                    Span::styled(
                        shorten_middle(endpoint.origin.as_deref().unwrap_or("-"), 72),
                        Style::default().fg(p.muted),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(
                        format!(
                            "priority={} configured={} effective={} routable={} state={:?}",
                            endpoint.priority,
                            endpoint.configured_enabled,
                            endpoint.effective_enabled,
                            endpoint.routable,
                            endpoint.runtime_state
                        )
                        .to_ascii_lowercase(),
                        Style::default().fg(p.muted),
                    ),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            i18n::label(ui.language, "No provider selected."),
            Style::default().fg(p.muted),
        )));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .scroll((ui.station_info_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}
