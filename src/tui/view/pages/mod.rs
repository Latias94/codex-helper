use ratatui::Frame;
use ratatui::layout::{Margin, Rect};

use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot};
use crate::tui::state::UiState;
use crate::tui::types::Page;

mod configs;
mod dashboard;
mod requests;
mod sessions;
mod settings;

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

    match ui.page {
        Page::Dashboard => dashboard::render_dashboard(f, p, ui, snapshot, providers, area),
        Page::Configs => configs::render_configs_page(f, p, ui, snapshot, providers, area),
        Page::Sessions => sessions::render_sessions_page(f, p, ui, snapshot, area),
        Page::Requests => requests::render_requests_page(f, p, ui, snapshot, area),
        Page::Stats => super::stats::render_stats_page(f, p, ui, snapshot, providers, area),
        Page::Settings => settings::render_settings_page(f, p, ui, snapshot, area),
    }
}
