use ratatui::widgets::{ListState, TableState};

use super::model::{Snapshot, filtered_requests_len};
use super::types::{Focus, Overlay, Page};

#[derive(Debug)]
pub(in crate::tui) struct UiState {
    pub(in crate::tui) page: Page,
    pub(in crate::tui) focus: Focus,
    pub(in crate::tui) overlay: Overlay,
    pub(in crate::tui) selected_session_idx: usize,
    pub(in crate::tui) selected_session_id: Option<String>,
    pub(in crate::tui) selected_request_idx: usize,
    pub(in crate::tui) effort_menu_idx: usize,
    pub(in crate::tui) provider_menu_idx: usize,
    pub(in crate::tui) toast: Option<(String, std::time::Instant)>,
    pub(in crate::tui) should_exit: bool,
    pub(in crate::tui) sessions_table: TableState,
    pub(in crate::tui) requests_table: TableState,
    pub(in crate::tui) menu_list: ListState,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            page: Page::Dashboard,
            focus: Focus::Sessions,
            overlay: Overlay::None,
            selected_session_idx: 0,
            selected_session_id: None,
            selected_request_idx: 0,
            effort_menu_idx: 0,
            provider_menu_idx: 0,
            toast: None,
            should_exit: false,
            sessions_table: TableState::default(),
            requests_table: TableState::default(),
            menu_list: ListState::default(),
        }
    }
}

impl UiState {
    pub(in crate::tui) fn clamp_selection(&mut self, snapshot: &Snapshot) {
        if snapshot.rows.is_empty() {
            self.selected_session_idx = 0;
            self.selected_session_id = None;
            self.sessions_table.select(None);

            self.selected_request_idx = 0;
            self.requests_table.select(None);
            return;
        }

        if let Some(sid) = self.selected_session_id.clone()
            && let Some(idx) = snapshot
                .rows
                .iter()
                .position(|r| r.session_id.as_deref() == Some(sid.as_str()))
        {
            self.selected_session_idx = idx;
        } else {
            self.selected_session_idx = self.selected_session_idx.min(snapshot.rows.len() - 1);
            self.selected_session_id = snapshot.rows[self.selected_session_idx].session_id.clone();
        }
        self.sessions_table.select(Some(self.selected_session_idx));

        let req_len = filtered_requests_len(snapshot, self.selected_session_idx);
        if req_len == 0 {
            self.selected_request_idx = 0;
            self.requests_table.select(None);
        } else {
            self.selected_request_idx = self.selected_request_idx.min(req_len - 1);
            self.requests_table.select(Some(self.selected_request_idx));
        }
    }
}

pub(in crate::tui) fn adjust_table_selection(
    table: &mut TableState,
    delta: i32,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        table.select(None);
        return None;
    }
    let cur = table.selected().unwrap_or(0);
    let next = if delta.is_negative() {
        cur.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (cur + delta as usize).min(len - 1)
    };
    table.select(Some(next));
    Some(next)
}
