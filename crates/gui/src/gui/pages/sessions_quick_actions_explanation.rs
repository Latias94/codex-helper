use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::*;

pub(super) fn render_source_explanation_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    has_session_cards: bool,
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
                "sessions_effective_route_explanation_grid",
            );
            if !has_session_cards {
                ui.add_space(6.0);
                console_note(
                    ui,
                    pick(
                        lang,
                        "当前附着数据来自旧接口回退，这里的来源解释是 best effort 推导。",
                        "Current attach data came from legacy fallback endpoints, so this explanation is best effort.",
                    ),
                );
            }
        },
    );
}
