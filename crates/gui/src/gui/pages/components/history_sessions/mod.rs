use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::history::HistoryScope;
use super::super::{
    PageCtx, WtItemSkipReason, basename, build_wt_items_from_session_summaries, format_age,
    history_workdir_from_cwd, now_ms, open_wt_items, session_summary_sort_key_ms, short_sid,
    shorten, shorten_middle, workdir_status_from_summary,
};

mod all_by_date;
mod session_panels;

pub(in super::super) use all_by_date::{
    render_all_by_date_dates_panel, render_all_by_date_sessions_panel,
};
pub(in super::super) use session_panels::{
    render_sessions_panel_horizontal, render_sessions_panel_vertical,
};
