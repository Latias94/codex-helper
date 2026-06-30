use ratatui::Frame;
use ratatui::layout::{Margin, Rect};

use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot};
use crate::tui::state::UiState;
use crate::tui::types::Page;

mod dashboard;
mod fleet;
mod history;
mod recent;
mod requests;
mod service_status;
mod sessions;
mod settings;
mod stations;

pub(super) fn render_body(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let area = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    ui.sync_rendered_page_state(snapshot);

    match ui.page {
        Page::Dashboard => dashboard::render_dashboard(f, p, ui, snapshot, providers, area),
        Page::Stations => stations::render_stations_page(f, p, ui, snapshot, providers, area),
        Page::Sessions => sessions::render_sessions_page(f, p, ui, snapshot, area),
        Page::Requests => requests::render_requests_page(f, p, ui, snapshot, area),
        Page::Stats => super::stats::render_stats_page(f, p, ui, snapshot, providers, area),
        Page::Settings => settings::render_settings_page(f, p, ui, snapshot, area),
        Page::History => history::render_history_page(f, p, ui, area),
        Page::Recent => recent::render_recent_page(f, p, ui, area),
        Page::Fleet => fleet::render_fleet_page(f, p, ui, area),
        Page::ServiceStatus => service_status::render_service_status_page(f, p, ui, snapshot, area),
    }
}
