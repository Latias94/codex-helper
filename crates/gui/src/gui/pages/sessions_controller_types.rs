use super::*;

#[derive(Debug, Default)]
pub(super) struct SessionPageActions {
    pub(super) apply_session_profile: Option<(String, String)>,
    pub(super) clear_session_profile_binding: Option<String>,
    pub(super) clear_session_manual_overrides: Option<String>,
}

#[derive(Debug)]
pub(super) struct SessionRenderData {
    pub(super) rows: Vec<SessionRow>,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) selected_idx_in_filtered: usize,
}

impl SessionRenderData {
    pub(super) fn filtered_rows(&self) -> impl Iterator<Item = &SessionRow> {
        self.filtered_indices
            .iter()
            .filter_map(|idx| self.rows.get(*idx))
    }

    pub(super) fn selected_row(&self) -> Option<&SessionRow> {
        self.filtered_indices
            .get(self.selected_idx_in_filtered)
            .and_then(|idx| self.rows.get(*idx))
    }
}
