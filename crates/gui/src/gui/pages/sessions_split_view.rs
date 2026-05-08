use super::sessions_controller::{SessionPageActions, SessionRenderData};
use super::sessions_detail_controls::render_session_detail_controls;
use super::sessions_quick_actions::{render_session_quick_actions, render_source_explanation_card};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_sessions_split_view(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    render_data: &SessionRenderData,
    profiles: &[ControlProfileOption],
    global_station_override: Option<&str>,
    has_session_cards: bool,
    host_local_session_features: bool,
) -> SessionPageActions {
    let mut actions = SessionPageActions::default();

    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "列表", "List"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("sessions_list_scroll")
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                for (pos, row) in render_data.filtered_rows().enumerate() {
                    let selected = pos == render_data.selected_idx_in_filtered;
                    let response = render_session_list_entry(ui, row, selected, ctx.lang);
                    if response.clicked() {
                        ctx.view.sessions.selected_idx = pos;
                        ctx.view.sessions.selected_session_id = row.session_id.clone();
                    }
                    ui.add_space(4.0);
                }
            });

        cols[1].heading(pick(ctx.lang, "详情", "Details"));
        cols[1].add_space(4.0);

        let Some(row) = render_data.selected_row() else {
            cols[1].label(pick(ctx.lang, "无会话数据。", "No session data."));
            return;
        };

        cols[1].columns(2, |summary_cols| {
            render_session_identity_card(
                &mut summary_cols[0],
                ctx.lang,
                row,
                profiles,
                host_local_session_features,
            );
            super::render_session_route_snapshot_card(
                &mut summary_cols[1],
                ctx.lang,
                row,
                global_station_override,
            );
        });
        cols[1].add_space(8.0);
        render_source_explanation_card(&mut cols[1], ctx.lang, row, has_session_cards);
        cols[1].add_space(8.0);

        render_session_quick_actions(&mut cols[1], ctx, row, host_local_session_features);
        if !host_local_session_features {
            if let Some(att) = ctx.proxy.attached()
                && let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
                    ctx.lang,
                    &[pick(ctx.lang, "cwd", "cwd"), pick(ctx.lang, "transcript", "transcript")],
                )
            {
                cols[1].small(warning);
            } else {
                cols[1].small(pick(
                    ctx.lang,
                    "提示：远端附着时，cwd / transcript 入口会被禁用；请用 Sessions / Requests 查看共享观测数据。",
                    "Tip: in remote-attached mode, cwd/transcript entries are disabled; use Sessions / Requests for shared observed data.",
                ));
            }
        }

        if let Some(status) = row.last_status {
            cols[1].label(format!("status(last): {status}"));
        }
        if let Some(ms) = row.last_duration_ms {
            cols[1].label(format!("duration(last): {ms} ms"));
        }
        if let Some(usage) = row.last_usage.as_ref() {
            cols[1].label(format!("usage(last): {}", usage_line(usage)));
        }
        if let Some(usage) = row.total_usage.as_ref() {
            cols[1].label(format!("usage(total): {}", usage_line(usage)));
        }

        cols[1].separator();

        render_session_detail_controls(
            &mut cols[1],
            ctx,
            snapshot,
            row,
            profiles,
            global_station_override,
            &mut actions.apply_session_profile,
            &mut actions.clear_session_profile_binding,
            &mut actions.clear_session_manual_overrides,
        );
    });

    actions
}
