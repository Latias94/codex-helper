use eframe::egui;

use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::history_observed_summary::observed_session_row_from_snapshot;
use super::*;

fn render_observed_source_explanation_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &super::SessionRow,
) {
    console_section(
        ui,
        pick(lang, "来源解释", "Source explanation"),
        ConsoleTone::Neutral,
        |ui| {
            super::render_effective_route_explanation_grid(
                ui,
                lang,
                row,
                "history_observed_route_explanation_grid",
            );
        },
    );
}

pub(super) fn render_observed_session_placeholder(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    summary: &SessionSummary,
) {
    let lang = ctx.lang;
    ui.colored_label(
        egui::Color32::from_rgb(200, 120, 40),
        pick(
            lang,
            "当前会话只有共享观测摘要，没有可直接读取的 host-local transcript 文件。",
            "This session currently has only shared observed metadata; no host-local transcript file is available to read directly.",
        ),
    );
    ui.small(pick(
        lang,
        "你仍然可以在这里查看 session 标识、cwd 和最近路由摘要；需要更完整控制时请回到 Sessions / Requests。",
        "You can still inspect session identity, cwd, and recent route summary here; return to Sessions / Requests for broader control data.",
    ));
    ui.add_space(6.0);

    if let Some(summary_line) = summary.first_user_message.as_deref() {
        let mut text = summary_line.to_string();
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .desired_rows(4)
                .font(egui::TextStyle::Monospace)
                .interactive(false),
        );
    } else {
        ui.label(pick(lang, "（无可用摘要）", "(no summary available)"));
    }

    if let Some(snapshot) = ctx.proxy.snapshot()
        && let Some(row) = observed_session_row_from_snapshot(&snapshot, summary.id.as_str())
    {
        ui.add_space(8.0);
        super::render_observed_route_snapshot_card(ui, lang, &row);
        ui.add_space(8.0);
        render_observed_source_explanation_card(ui, lang, &row);
        if row.last_route_decision.is_some() {
            ui.add_space(8.0);
            super::render_last_route_decision_card(ui, lang, &row);
        }
    } else {
        ui.add_space(8.0);
        console_note(
            ui,
            pick(
                lang,
                "当前运行态里没有这条 session 的最新控制面快照，因此这里只能展示摘要文本。",
                "No current control-plane snapshot is available for this session, so only the summary text can be shown here.",
            ),
        );
    }
}
