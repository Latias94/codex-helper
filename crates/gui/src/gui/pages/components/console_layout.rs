use std::hash::Hash;

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum ConsoleTone {
    Neutral,
    Accent,
    Positive,
    Warning,
}

impl ConsoleTone {
    fn stroke_color(self) -> egui::Color32 {
        match self {
            Self::Neutral => egui::Color32::from_rgb(110, 120, 135),
            Self::Accent => egui::Color32::from_rgb(60, 110, 170),
            Self::Positive => egui::Color32::from_rgb(50, 145, 90),
            Self::Warning => egui::Color32::from_rgb(200, 120, 40),
        }
    }

    fn fill_color(self) -> egui::Color32 {
        match self {
            Self::Neutral => egui::Color32::from_rgba_unmultiplied(120, 130, 145, 16),
            Self::Accent => egui::Color32::from_rgba_unmultiplied(70, 120, 190, 20),
            Self::Positive => egui::Color32::from_rgba_unmultiplied(55, 155, 95, 18),
            Self::Warning => egui::Color32::from_rgba_unmultiplied(215, 135, 45, 18),
        }
    }
}

pub(in super::super) fn console_section<R>(
    ui: &mut egui::Ui,
    title: impl Into<String>,
    tone: ConsoleTone,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let title = title.into();
    egui::Frame::group(ui.style())
        .fill(tone.fill_color())
        .stroke(egui::Stroke::new(1.0, tone.stroke_color()))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .color(tone.stroke_color()),
            );
            ui.add_space(6.0);
            add(ui)
        })
}

pub(in super::super) fn console_kv_grid<H: Hash>(
    ui: &mut egui::Ui,
    id_source: H,
    rows: &[(String, String)],
) {
    egui::Grid::new(id_source)
        .num_columns(2)
        .spacing([12.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.small(egui::RichText::new(label).strong());
                ui.monospace(value);
                ui.end_row();
            }
        });
}

pub(in super::super) fn console_note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.small(
        egui::RichText::new(text.into())
            .italics()
            .color(egui::Color32::from_rgb(130, 130, 130)),
    );
}
