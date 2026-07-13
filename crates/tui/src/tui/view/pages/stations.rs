use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::dashboard_core::OperatorProviderCapacity;
use crate::tui::ProviderOption;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_amount_brief_lang, balance_snapshot_status_label_lang,
    balance_snapshot_status_style, operator_provider_policy_action_count,
    provider_balance_brief_lang, shorten, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::provider_control::{
    policy_action_control_details, policy_action_control_summary,
};

fn provider_capacity_summary(capacity: &OperatorProviderCapacity) -> Option<String> {
    if capacity.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    match (capacity.active, capacity.limit) {
        (Some(active), Some(limit)) => parts.push(format!("active={active}/{limit}")),
        (None, Some(limit)) => parts.push(format!("limit={limit}")),
        _ => {}
    }
    if let Some(configured) = capacity.configured_max_concurrent_requests {
        parts.push(format!("configured={configured}"));
    }
    if let Some(effective) = capacity.effective_max_concurrent_requests {
        parts.push(format!("effective={effective}"));
    }
    if capacity.inherited_from_provider == Some(true) {
        parts.push("inherited".to_string());
    }
    if capacity.saturated {
        parts.push("saturated".to_string());
    }

    (!parts.is_empty()).then(|| parts.join(" "))
}

fn yes_no(value: bool, lang: crate::tui::Language) -> &'static str {
    i18n::label(lang, if value { "yes" } else { "no" })
}

pub(super) fn render_stations_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let left_block = Block::default()
        .title(Span::styled(
            l("Providers"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("Name"),
        l("Cfg"),
        l("Eff"),
        l("Routable"),
        l("control"),
        l("Balance/Quota"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let rows = providers
        .iter()
        .map(|provider| {
            let policy_action_count = operator_provider_policy_action_count(provider);
            let control = if policy_action_count == 0 {
                "-".to_string()
            } else {
                policy_action_count.to_string()
            };
            let balance = provider_balance_brief_lang(
                &snapshot.provider_balances,
                provider.name.as_str(),
                18,
                lang,
            );
            let style = Style::default().fg(if provider.effective_enabled {
                p.text
            } else {
                p.muted
            });
            Row::new([
                provider.name.clone(),
                yes_no(provider.configured_enabled, lang).to_string(),
                yes_no(provider.effective_enabled, lang).to_string(),
                format!(
                    "{}/{}",
                    provider.routable_endpoints,
                    provider.endpoints.len()
                ),
                control,
                balance,
            ])
            .style(style)
            .height(1)
        })
        .collect::<Vec<_>>();

    let table_visible_rows = usize::from(left_block.inner(columns[0]).height.saturating_sub(1));
    ui.sync_stations_table_viewport(providers.len(), table_visible_rows);

    let table = Table::new(
        rows,
        [
            Constraint::Min(9),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(18),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.stations_table);

    let selected = providers.get(ui.selected_station_idx);
    let right_title = selected
        .map(|provider| format!("{}: {}", l("Provider details"), provider.name))
        .unwrap_or_else(|| l("Provider details").to_string());

    let mut lines = Vec::new();
    if let Some(provider) = selected {
        if let Some(alias) = provider
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
        {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("alias")), Style::default().fg(p.muted)),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("configured: ", Style::default().fg(p.muted)),
            Span::styled(
                yes_no(provider.configured_enabled, lang),
                Style::default().fg(if provider.configured_enabled {
                    p.good
                } else {
                    p.warn
                }),
            ),
            Span::raw("   "),
            Span::styled("effective: ", Style::default().fg(p.muted)),
            Span::styled(
                yes_no(provider.effective_enabled, lang),
                Style::default().fg(if provider.effective_enabled {
                    p.good
                } else {
                    p.warn
                }),
            ),
            Span::raw("   "),
            Span::styled("routable: ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}/{}",
                    provider.routable_endpoints,
                    provider.endpoints.len()
                ),
                Style::default().fg(if provider.routable_endpoints > 0 {
                    p.accent
                } else {
                    p.warn
                }),
            ),
        ]));
        if let Some(capacity) = provider_capacity_summary(&provider.capacity) {
            lines.push(Line::from(vec![
                Span::styled("capacity: ", Style::default().fg(p.muted)),
                Span::styled(capacity, Style::default().fg(p.text)),
            ]));
        }

        let provider_control_lines = provider
            .endpoints
            .iter()
            .flat_map(|endpoint| {
                endpoint
                    .policy_actions
                    .iter()
                    .map(move |action| policy_action_control_summary(endpoint, action, 40))
            })
            .collect::<Vec<_>>();
        if !provider_control_lines.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                l("Provider control"),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]));
            for line in provider_control_lines.iter().take(8) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(shorten_middle(line, 96), Style::default().fg(p.warn)),
                ]));
            }
            if provider_control_lines.len() > 8 {
                lines.push(Line::from(Span::styled(
                    format!("... +{} more", provider_control_lines.len() - 8),
                    Style::default().fg(p.muted),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Balance / quota"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(balances) = snapshot.provider_balances.get(provider.name.as_str()) {
            if balances.is_empty() {
                lines.push(Line::from(Span::styled(
                    i18n::text(lang, msg::NONE_PARENS),
                    Style::default().fg(p.muted),
                )));
            } else {
                for balance in balances.iter().take(12) {
                    let endpoint_id = shorten_middle(&balance.provider_endpoint.endpoint_id, 12);
                    lines.push(Line::from(vec![
                        Span::styled(format!("{endpoint_id}: "), Style::default().fg(p.muted)),
                        Span::styled(
                            shorten_middle(&balance.observation_provider_id, 20),
                            Style::default().fg(p.text),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            balance_snapshot_status_label_lang(balance, lang),
                            balance_snapshot_status_style(p, balance),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            shorten_middle(
                                &balance_amount_brief_lang(balance, lang)
                                    .unwrap_or_else(|| balance.amount_summary()),
                                56,
                            ),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                    if let Some(error) = balance
                        .error
                        .as_deref()
                        .map(str::trim)
                        .filter(|error| !error.is_empty())
                    {
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            Span::styled(
                                format!("{}: {}", l("balance lookup failed"), shorten(error, 56)),
                                Style::default().fg(p.muted),
                            ),
                        ]));
                    }
                }
                if balances.len() > 12 {
                    lines.push(Line::from(Span::styled(
                        format!("... +{} more", balances.len() - 12),
                        Style::default().fg(p.muted),
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Endpoints"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if provider.endpoints.is_empty() {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        } else {
            for (index, endpoint) in provider.endpoints.iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{index:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(endpoint.name.clone(), Style::default().fg(p.text)),
                    Span::raw("  "),
                    Span::styled(
                        shorten_middle(endpoint.origin.as_deref().unwrap_or("-"), 64),
                        Style::default().fg(p.muted),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
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
                if let Some(capacity) = provider_capacity_summary(&endpoint.capacity) {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!("capacity: {capacity}"),
                            Style::default().fg(if endpoint.capacity.saturated {
                                p.warn
                            } else {
                                p.muted
                            }),
                        ),
                    ]));
                }
                for action in &endpoint.policy_actions {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!("control: {}", policy_action_control_details(action)),
                            Style::default().fg(p.warn),
                        ),
                    ]));
                }
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            l("No providers available."),
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            right_title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
