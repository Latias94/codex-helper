use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_rank, provider_balance_brief_lang,
    provider_balance_compact_lang, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::centered_rect;

pub(in crate::tui::view) fn render_provider_info_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
) {
    let area = centered_rect(76, 78, f.area());
    f.render_widget(Clear, area);
    let selected = providers.get(ui.selected_provider_idx);
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
        let label = |zh: &'static str, en: &'static str| match ui.language {
            crate::tui::Language::Zh => zh,
            crate::tui::Language::En => en,
        };
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
        if let Some(endpoint_id) = ui.provider_info_endpoint_id.as_deref()
            && let Some(endpoint) = provider
                .endpoints
                .iter()
                .find(|endpoint| endpoint.name == endpoint_id)
        {
            let candidate = snapshot.routing.as_ref().and_then(|routing| {
                routing.candidates.iter().find(|candidate| {
                    candidate.provider_id == provider.name && candidate.endpoint_id == endpoint_id
                })
            });
            let balance = snapshot
                .provider_balances
                .get(provider.name.as_str())
                .and_then(|balances| {
                    balances
                        .iter()
                        .filter(|balance| balance.provider_endpoint.endpoint_id == endpoint_id)
                        .min_by_key(|balance| balance_snapshot_rank(balance))
                });
            let capacity = match (endpoint.capacity.active, endpoint.capacity.limit) {
                (Some(active), Some(limit)) => format!("{active}/{limit}"),
                (None, Some(limit)) => format!("-/{limit}"),
                _ => endpoint
                    .capacity
                    .effective_max_concurrent_requests
                    .map(|limit| format!("-/{limit}"))
                    .unwrap_or_else(|| "-".to_string()),
            };
            lines.push(Line::from(Span::styled(
                label("当前路由端点", "Selected route endpoint"),
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::styled("  target: ", Style::default().fg(p.muted)),
                Span::styled(
                    format!("{}.{}", provider.name, endpoint.name),
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(format!(
                "  {}={}  {}={}  priority={}  {}={capacity}",
                label("顺序", "order"),
                candidate
                    .map(|candidate| candidate.route_order.saturating_add(1).to_string())
                    .unwrap_or_else(|| "-".to_string()),
                label("偏好组", "group"),
                candidate
                    .map(|candidate| candidate.preference_group.saturating_add(1).to_string())
                    .unwrap_or_else(|| "-".to_string()),
                endpoint.priority,
                label("并发", "capacity"),
            )));
            lines.push(Line::from(format!(
                "  configured={}  effective={}  routable={}  state={:?}",
                endpoint.configured_enabled,
                endpoint.effective_enabled,
                endpoint.routable,
                endpoint.runtime_state
            )));
            for action in endpoint.policy_actions.iter().take(8) {
                let cooldown = action
                    .cooldown_remaining_secs
                    .map(|seconds| format!(" ({seconds}s)"))
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("  {}={}{}", label("控制", "control"), action.code, cooldown),
                    Style::default().fg(if action.active_cooldown {
                        p.warn
                    } else {
                        p.muted
                    }),
                )));
            }
            lines.push(Line::from(format!(
                "  {}/{}: {}",
                label("余额", "balance"),
                label("额度", "quota"),
                balance
                    .map(|balance| provider_balance_compact_lang(balance, 84, ui.language))
                    .unwrap_or_else(|| "-".to_string())
            )));
            lines.push(Line::from(format!(
                "  route path: {}",
                candidate
                    .map(|candidate| candidate.route_path.join(" -> "))
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| "-".to_string())
            )));
            lines.push(Line::from(""));
        }
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
                let selected_endpoint =
                    ui.provider_info_endpoint_id.as_deref() == Some(endpoint.name.as_str());
                let endpoint_style = if selected_endpoint {
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.text)
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} {index:>2}. ", if selected_endpoint { ">" } else { " " }),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(endpoint.name.clone(), endpoint_style),
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
            .scroll((ui.provider_info_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}
