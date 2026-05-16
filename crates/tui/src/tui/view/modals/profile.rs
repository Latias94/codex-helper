use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem};

use crate::dashboard_core::ControlProfileOption;
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, shorten_middle};
use crate::tui::state::UiState;
use crate::tui::types::Overlay;
use crate::tui::view::widgets::centered_rect;
fn profile_option_to_service_profile(
    profile: &ControlProfileOption,
) -> crate::config::ServiceControlProfile {
    crate::config::ServiceControlProfile {
        extends: profile.extends.clone(),
        station: None,
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
    }
}

fn resolve_profile_from_options(
    profile_name: &str,
    profiles: &[ControlProfileOption],
) -> anyhow::Result<crate::config::ServiceControlProfile> {
    let profile_catalog = profiles
        .iter()
        .map(|profile| {
            (
                profile.name.clone(),
                profile_option_to_service_profile(profile),
            )
        })
        .collect::<BTreeMap<_, _>>();
    crate::config::resolve_service_profile_from_catalog(&profile_catalog, profile_name)
}

fn format_profile_route_summary(profile: &crate::config::ServiceControlProfile) -> String {
    format!(
        "model={}  reasoning={}  tier={}",
        profile.model.as_deref().unwrap_or("<auto>"),
        profile.reasoning_effort.as_deref().unwrap_or("<auto>"),
        profile.service_tier.as_deref().unwrap_or("<auto>"),
    )
}

pub(super) fn profile_declared_summary(profile: &ControlProfileOption, lang: Language) -> String {
    let mut parts = Vec::new();
    if let Some(extends) = profile.extends.as_deref() {
        parts.push(format!("extends={extends}"));
    }
    parts.push(format!(
        "model={}",
        profile.model.as_deref().unwrap_or("<auto>")
    ));
    parts.push(format!(
        "reasoning={}",
        profile.reasoning_effort.as_deref().unwrap_or("<auto>")
    ));
    parts.push(format!(
        "tier={}",
        profile.service_tier.as_deref().unwrap_or("<auto>")
    ));
    format!(
        "{} {}",
        i18n::text(lang, msg::DECLARED_LABEL),
        shorten_middle(parts.join("  ").as_str(), 72)
    )
}

pub(super) fn profile_resolved_summary(
    profile_name: &str,
    profiles: &[ControlProfileOption],
    lang: Language,
) -> (String, bool) {
    match resolve_profile_from_options(profile_name, profiles) {
        Ok(profile) => (
            format!(
                "{} {}",
                i18n::text(lang, msg::RESOLVED_LABEL),
                shorten_middle(format_profile_route_summary(&profile).as_str(), 72)
            ),
            false,
        ),
        Err(err) => (
            format!(
                "{} {}",
                i18n::text(lang, msg::RESOLVE_FAILED_LABEL),
                shorten_middle(err.to_string().as_str(), 72)
            ),
            true,
        ),
    }
}

pub(in crate::tui::view) fn render_profile_modal_v2(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
) {
    let area = centered_rect(82, 72, f.area());
    f.render_widget(Clear, area);
    let (title, clear_title, clear_detail) = match ui.overlay {
        Overlay::ProfileMenuDefaultRuntime => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_RUNTIME_PROFILE),
            i18n::text(ui.language, msg::CLEAR_RUNTIME_PROFILE),
            i18n::text(ui.language, msg::CLEAR_RUNTIME_PROFILE_HELP),
        ),
        Overlay::ProfileMenuDefaultPersisted => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_CONFIGURED_PROFILE),
            i18n::text(ui.language, msg::CLEAR_CONFIGURED_PROFILE),
            i18n::text(ui.language, msg::CLEAR_CONFIGURED_PROFILE_HELP),
        ),
        _ => (
            i18n::text(ui.language, msg::OVERLAY_MANAGE_SESSION_PROFILE),
            i18n::text(ui.language, msg::CLEAR_SESSION_PROFILE_BINDING),
            i18n::text(ui.language, msg::CLEAR_SESSION_PROFILE_BINDING_HELP),
        ),
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut items = Vec::with_capacity(ui.profile_options.len().saturating_add(1));
    items.push(ListItem::new(Text::from(vec![
        Line::from(clear_title),
        Line::from(Span::styled(clear_detail, Style::default().fg(p.muted))),
    ])));
    items.extend(ui.profile_options.iter().map(|profile| {
        let mut label = profile.name.clone();
        let is_configured_default =
            ui.configured_default_profile.as_deref() == Some(profile.name.as_str());
        let is_runtime_override =
            ui.runtime_default_profile_override.as_deref() == Some(profile.name.as_str());
        let is_effective_default =
            ui.effective_default_profile.as_deref() == Some(profile.name.as_str());
        match ui.overlay {
            Overlay::ProfileMenuDefaultRuntime => {
                if is_runtime_override {
                    label.push_str(" *runtime");
                } else if is_effective_default {
                    label.push_str(" *effective");
                }
            }
            Overlay::ProfileMenuDefaultPersisted => {
                if is_configured_default && is_effective_default {
                    label.push_str(" *configured/effective");
                } else if is_configured_default {
                    label.push_str(" *configured");
                } else if is_effective_default {
                    label.push_str(" *effective");
                }
            }
            _ => {
                if profile.is_default {
                    label.push_str(" *default");
                }
            }
        }
        let declared = profile_declared_summary(profile, ui.language);
        let (resolved, resolve_failed) =
            profile_resolved_summary(profile.name.as_str(), &ui.profile_options, ui.language);
        ListItem::new(Text::from(vec![
            Line::from(label),
            Line::from(Span::styled(declared, Style::default().fg(p.muted))),
            Line::from(Span::styled(
                resolved,
                Style::default().fg(if resolve_failed { p.bad } else { p.accent }),
            )),
        ]))
    }));

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.profile_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}
