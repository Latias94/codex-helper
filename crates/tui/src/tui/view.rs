use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::model::{Palette, ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;

mod chrome;
mod modals;
mod pages;
mod stats;
mod widgets;

pub(in crate::tui) fn render_app(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    service_name: &'static str,
    port: u16,
    providers: &[ProviderOption],
) {
    f.render_widget(widgets::BackgroundWidget { p }, f.area());

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    chrome::render_header(f, p, ui, snapshot, service_name, port, outer[0]);
    pages::render_body(f, p, ui, snapshot, providers, outer[1]);
    chrome::render_footer(f, p, ui, outer[2]);

    match ui.overlay {
        Overlay::None => {}
        Overlay::Help => modals::render_help_modal(f, p, ui.language),
        Overlay::StationInfo => modals::render_station_info_modal(f, p, ui, snapshot, providers),
        Overlay::EffortMenu => modals::render_effort_modal(f, p, ui),
        Overlay::ModelMenuSession => modals::render_model_modal(f, p, ui),
        Overlay::ModelInputSession => modals::render_model_input_modal(f, p, ui),
        Overlay::ServiceTierMenuSession => modals::render_service_tier_modal(f, p, ui),
        Overlay::ServiceTierInputSession => modals::render_service_tier_input_modal(f, p, ui),
        Overlay::ProfileMenuSession
        | Overlay::ProfileMenuDefaultRuntime
        | Overlay::ProfileMenuDefaultPersisted => modals::render_profile_modal_v2(f, p, ui),
        Overlay::SessionTranscript => modals::render_session_transcript_modal(f, p, ui),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            let title = match ui.overlay {
                Overlay::ProviderMenuSession => crate::tui::i18n::pick(
                    ui.language,
                    "会话 Provider 覆盖",
                    "Session provider override",
                ),
                Overlay::ProviderMenuGlobal => {
                    crate::tui::i18n::pick(ui.language, "全局站点 pin", "Global station pin")
                }
                _ => unreachable!(),
            };
            modals::render_provider_modal(f, p, ui, providers, title);
        }
    }
}
