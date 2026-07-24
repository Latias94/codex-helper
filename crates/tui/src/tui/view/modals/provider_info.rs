use ratatui::Frame;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use crate::state::ProviderBalanceSnapshot;
use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, format_age, has_current_upstream_usage_report, now_ms,
    provider_balance_brief_lang, provider_balance_compact_lang, provider_endpoint_balance_snapshot,
    provider_endpoint_current_usage_report_snapshot, provider_usage_alert_label_lang,
    provider_usage_rate_summary_lang, provider_usage_source_label_lang,
    provider_usage_window_summary_lang, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::{centered_rect, max_wrapped_vertical_scroll};

fn push_upstream_usage_report_lines(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    lang: crate::tui::Language,
    balance: &ProviderBalanceSnapshot,
) {
    if !has_current_upstream_usage_report(balance) {
        return;
    }

    let label = |zh: &'static str, en: &'static str| match lang {
        crate::tui::Language::Zh => zh,
        crate::tui::Language::En => en,
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        label("上游用量报告", "Upstream usage report"),
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(format!(
        "  source={}  age={}",
        provider_usage_source_label_lang(&balance.source, lang),
        format_age(now_ms(), Some(balance.fetched_at_ms))
    )));
    if let Some(rate) = provider_usage_rate_summary_lang(balance, lang) {
        lines.push(Line::from(format!(
            "  {}={rate}",
            label("接口遥测", "telemetry")
        )));
    }
    for window in balance.usage_windows.iter().take(3) {
        lines.push(Line::from(format!(
            "  {}",
            provider_usage_window_summary_lang(window, lang)
        )));
    }
    if !balance.usage_alerts.is_empty() {
        let alerts = balance
            .usage_alerts
            .iter()
            .map(|alert| provider_usage_alert_label_lang(alert.kind, lang))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(Span::styled(
            format!("  {}={alerts}", label("告警", "alerts")),
            Style::default().fg(p.warn),
        )));
    }
    for model in balance.usage_model_stats.iter().take(4) {
        let mut fields = Vec::new();
        if let Some(requests) = model.request_count {
            fields.push(format!("req={requests}"));
        }
        if let Some(tokens) = model.total_tokens {
            fields.push(format!("tokens={tokens}"));
        }
        if let Some(cost) = model.total_cost_usd.as_deref() {
            fields.push(format!("cost=${cost}"));
        }
        if !fields.is_empty() {
            lines.push(Line::from(format!(
                "  {} {} {}",
                label("模型", "model"),
                model.model,
                fields.join(" ")
            )));
        }
    }
}

pub(in crate::tui::view) fn render_provider_info_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
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
        lines.push(Line::from(vec![
            Span::styled("credential readiness: ", Style::default().fg(p.muted)),
            Span::styled(
                provider
                    .credential_readiness
                    .map(|readiness| readiness.as_str())
                    .unwrap_or("unreported"),
                Style::default().fg(
                    if provider.credential_readiness.is_some_and(|readiness| {
                        readiness == crate::credentials::CredentialAggregateReadiness::Ready
                    }) {
                        p.good
                    } else {
                        p.warn
                    },
                ),
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
            let balance = provider_endpoint_balance_snapshot(
                &snapshot.provider_balances,
                provider.name.as_str(),
                endpoint_id,
            );
            let usage_report = provider_endpoint_current_usage_report_snapshot(
                &snapshot.provider_balances,
                provider.name.as_str(),
                endpoint_id,
            );
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
            lines.push(Line::from(format!(
                "  {}={}",
                label("凭据", "credential"),
                endpoint
                    .credential_readiness
                    .map(|readiness| readiness.as_str())
                    .unwrap_or("unreported")
            )));
            for detail in &endpoint.credential_details {
                let kind = detail.kind.map(|kind| kind.as_str()).unwrap_or("upstream");
                let source = detail.source_kind.as_deref().unwrap_or("unreported");
                let reference = detail.reference.as_deref().unwrap_or("-");
                let cause = detail
                    .stale_cause
                    .map(|cause| format!(" cause={}", cause.as_str()))
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!(
                        "    {kind}: {} source={source} ref={reference}{cause}",
                        detail.code.as_str()
                    ),
                    Style::default().fg(if detail.code.is_routable() {
                        p.muted
                    } else {
                        p.warn
                    }),
                )));
            }
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
            if let Some(balance) = usage_report {
                push_upstream_usage_report_lines(&mut lines, p, ui.language, balance);
            }
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
                            "priority={} configured={} effective={} routable={} state={:?} credential={}",
                            endpoint.priority,
                            endpoint.configured_enabled,
                            endpoint.effective_enabled,
                            endpoint.routable,
                            endpoint.runtime_state,
                            endpoint
                                .credential_readiness
                                .map(|readiness| readiness.as_str())
                                .unwrap_or("unreported")
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

    let inner = block.inner(area);
    let max_scroll = max_wrapped_vertical_scroll(&lines, inner.width, inner.height);
    ui.provider_info_scroll = ui.provider_info_scroll.min(max_scroll);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .scroll((ui.provider_info_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
    if max_scroll > 0 {
        let mut scrollbar = ScrollbarState::new(usize::from(max_scroll) + 1)
            .position(usize::from(ui.provider_info_scroll));
        let widget =
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(p.focus));
        f.render_stateful_widget(widget, area, &mut scrollbar);
    }
}
