use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, provider_balance_compact_lang,
    routing_context_balance_rank, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::centered_rect;
fn routing_policy_label(policy: crate::config::RoutingPolicyV4) -> &'static str {
    match policy {
        crate::config::RoutingPolicyV4::ManualSticky => "manual-sticky",
        crate::config::RoutingPolicyV4::OrderedFailover => "ordered-failover",
        crate::config::RoutingPolicyV4::TagPreferred => "tag-preferred",
        crate::config::RoutingPolicyV4::Conditional => "conditional",
    }
}

fn routing_exhausted_label(action: crate::config::RoutingExhaustedActionV4) -> &'static str {
    match action {
        crate::config::RoutingExhaustedActionV4::Continue => "continue",
        crate::config::RoutingExhaustedActionV4::Stop => "stop",
    }
}

fn routing_tags_label(tags: &BTreeMap<String, String>, max_width: usize) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    let parts = tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    shorten_middle(&parts.join(" "), max_width)
}

fn routing_prefer_tags_label(filters: &[BTreeMap<String, String>], max_width: usize) -> String {
    if filters.is_empty() {
        return "-".to_string();
    }
    let parts = filters
        .iter()
        .map(|tags| routing_tags_label(tags, max_width))
        .collect::<Vec<_>>();
    shorten_middle(&parts.join(" OR "), max_width)
}

pub(super) fn routing_provider_balance_line<'a>(
    snapshot: &'a Snapshot,
    provider_name: &str,
    lang: Language,
) -> Option<(&'a crate::state::ProviderBalanceSnapshot, String)> {
    let mut matches = snapshot
        .provider_balances
        .iter()
        .flat_map(|(key, balances)| {
            balances.iter().filter_map(move |balance| {
                if balance.provider_id == provider_name
                    || (balance.provider_id.trim().is_empty() && key == provider_name)
                {
                    Some((
                        routing_context_balance_rank(key, balance, provider_name),
                        balance,
                    ))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.upstream_index.cmp(&right.1.upstream_index))
            .then_with(|| right.1.fetched_at_ms.cmp(&left.1.fetched_at_ms))
    });
    let (_, balance) = matches.into_iter().next()?;
    Some((balance, provider_balance_compact_lang(balance, 38, lang)))
}

pub(in crate::tui::view) fn render_routing_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let area = centered_rect(76, 78, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            l("Routing"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let Some(spec) = ui.routing_spec.as_ref() else {
        let text = Text::from(vec![
            Line::from(l("routing spec not loaded")),
            Line::from(Span::styled(
                match lang {
                    Language::Zh => "g 刷新   Esc 关闭",
                    Language::En => "g refresh   Esc close",
                },
                Style::default().fg(p.muted),
            )),
        ]);
        f.render_widget(
            Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(p.text))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let order = {
        let mut order = if spec.order.is_empty() {
            spec.providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect::<Vec<_>>()
        } else {
            spec.order.clone()
        };
        for provider in &spec.providers {
            if !order.iter().any(|name| name == &provider.name) {
                order.push(provider.name.clone());
            }
        }
        order
    };
    let provider_by_name = spec
        .providers
        .iter()
        .map(|provider| (provider.name.as_str(), provider))
        .collect::<BTreeMap<_, _>>();

    let mut items = Vec::new();
    items.push(ListItem::new(Text::from(vec![
        Line::from(vec![
            Span::styled(format!("{}: ", l("policy")), Style::default().fg(p.muted)),
            Span::styled(routing_policy_label(spec.policy), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("target")), Style::default().fg(p.muted)),
            Span::styled(
                spec.target.as_deref().unwrap_or("-"),
                Style::default().fg(if spec.target.is_some() { p.accent } else { p.muted }),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("on_exhausted")), Style::default().fg(p.muted)),
            Span::styled(routing_exhausted_label(spec.on_exhausted), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(format!("{}: ", l("prefer_tags")), Style::default().fg(p.muted)),
            Span::styled(
                routing_prefer_tags_label(&spec.prefer_tags, 72),
                Style::default().fg(if spec.prefer_tags.is_empty() {
                    p.muted
                } else {
                    p.accent
                }),
            ),
        ]),
        Line::from(Span::styled(
            match lang {
                Language::Zh => {
                    "Enter pin  a 顺序  f monthly 优先  e 启用/禁用  s stop/continue  [/]/u/d 排序  1 monthly  2 paygo  0 清除 billing  g 刷新  Esc 关闭"
                }
                Language::En => {
                    "Enter pin  a ordered  f monthly-first  e enable/disable  s stop/continue  [/]/u/d reorder  1 monthly  2 paygo  0 clear billing  g refresh  Esc close"
                }
            },
            Style::default().fg(p.muted),
        )),
    ])));

    for (idx, name) in order.iter().enumerate() {
        let provider = provider_by_name.get(name.as_str()).copied();
        let enabled = provider.map(|provider| provider.enabled).unwrap_or(false);
        let tags = provider
            .map(|provider| routing_tags_label(&provider.tags, 42))
            .unwrap_or_else(|| "-".to_string());
        let alias = provider
            .and_then(|provider| provider.alias.as_deref())
            .filter(|alias| !alias.trim().is_empty() && *alias != name)
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        let marker = if spec.target.as_deref() == Some(name.as_str()) {
            "PIN"
        } else if matches!(spec.policy, crate::config::RoutingPolicyV4::TagPreferred)
            && provider.is_some_and(|provider| {
                spec.prefer_tags.iter().any(|filter| {
                    !filter.is_empty()
                        && filter
                            .iter()
                            .all(|(key, value)| provider.tags.get(key) == Some(value))
                })
            })
        {
            "PREF"
        } else {
            "    "
        };
        let (balance_style, balance_text) =
            if let Some((balance, text)) = routing_provider_balance_line(snapshot, name, lang) {
                (balance_snapshot_status_style(p, balance), text)
            } else {
                (Style::default().fg(p.muted), "-".to_string())
            };

        let mut title_style = Style::default().fg(if enabled { p.text } else { p.muted });
        if spec.target.as_deref() == Some(name.as_str()) {
            title_style = title_style.fg(p.accent).add_modifier(Modifier::BOLD);
        }
        items.push(
            ListItem::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                    Span::styled(marker, Style::default().fg(p.accent)),
                    Span::raw("  "),
                    Span::styled(format!("{name}{alias}"), title_style),
                    if enabled {
                        Span::raw("")
                    } else {
                        Span::styled(" [off]", Style::default().fg(p.warn))
                    },
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("     {}: ", l("balance/quota")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(balance_text, balance_style),
                ]),
                Line::from(vec![
                    Span::styled(format!("{}: ", l("tags")), Style::default().fg(p.muted)),
                    Span::styled(tags, Style::default().fg(p.muted)),
                ]),
            ]))
            .style(Style::default().fg(if enabled { p.text } else { p.muted })),
        );
    }

    let selected = if order.is_empty() {
        0
    } else {
        ui.routing_menu_idx.min(order.len().saturating_sub(1)) + 1
    };
    ui.menu_list.select(Some(selected));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}
