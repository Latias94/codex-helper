use super::*;

pub(super) struct ProfileCardItem {
    pub(super) name: String,
    pub(super) summary: String,
    pub(super) station: String,
    pub(super) model: String,
    pub(super) reasoning_effort: String,
    pub(super) service_tier: String,
    pub(super) is_default: bool,
    pub(super) is_selected: bool,
}

pub(super) fn build_profile_card_item(
    name: &str,
    profile: &crate::config::ServiceControlProfile,
    is_default: bool,
    is_selected: bool,
) -> ProfileCardItem {
    let option = ControlProfileOption {
        name: name.to_string(),
        extends: profile.extends.clone(),
        station: profile.station.clone(),
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
        is_default,
    };

    ProfileCardItem {
        name: name.to_string(),
        summary: format_profile_summary(&option),
        station: option.station.unwrap_or_else(|| "auto".to_string()),
        model: option.model.unwrap_or_else(|| "auto".to_string()),
        reasoning_effort: option
            .reasoning_effort
            .unwrap_or_else(|| "auto".to_string()),
        service_tier: option.service_tier.unwrap_or_else(|| "auto".to_string()),
        is_default,
        is_selected,
    }
}

pub(super) fn render_profile_card_list<F>(
    ui: &mut egui::Ui,
    lang: Language,
    id_salt: &str,
    empty_label: &str,
    items: &[ProfileCardItem],
    mut on_select: F,
) where
    F: FnMut(&str),
{
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(320.0)
        .show(ui, |ui| {
            if items.is_empty() {
                ui.label(empty_label);
                return;
            }

            for item in items {
                let stroke_color = if item.is_selected {
                    egui::Color32::from_rgb(76, 114, 176)
                } else {
                    egui::Color32::from_rgb(190, 190, 190)
                };
                let fill_color = if item.is_selected {
                    egui::Color32::from_rgb(243, 247, 255)
                } else {
                    egui::Color32::from_rgb(250, 250, 250)
                };
                let response = egui::Frame::group(ui.style())
                    .fill(fill_color)
                    .stroke(egui::Stroke::new(
                        if item.is_selected { 1.5 } else { 1.0 },
                        stroke_color,
                    ))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(item.name.as_str()).strong());
                            if item.is_default {
                                render_profile_badge(
                                    ui,
                                    pick(lang, "default", "default"),
                                    egui::Color32::from_rgb(86, 122, 62),
                                );
                            }
                            if item.is_selected {
                                render_profile_badge(
                                    ui,
                                    pick(lang, "当前", "selected"),
                                    egui::Color32::from_rgb(76, 114, 176),
                                );
                            }
                        });
                        ui.small(item.summary.as_str());
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            render_profile_badge(
                                ui,
                                format!("station {}", item.station),
                                egui::Color32::from_rgb(86, 122, 62),
                            );
                            render_profile_badge(
                                ui,
                                format!("model {}", item.model),
                                egui::Color32::from_rgb(76, 114, 176),
                            );
                            render_profile_badge(
                                ui,
                                format!("effort {}", item.reasoning_effort),
                                egui::Color32::from_rgb(176, 122, 76),
                            );
                            render_profile_badge(
                                ui,
                                format!("tier {}", item.service_tier),
                                egui::Color32::from_rgb(122, 90, 166),
                            );
                        });
                    })
                    .response
                    .interact(egui::Sense::click());

                if response.clicked() {
                    on_select(item.name.as_str());
                }
                ui.add_space(6.0);
            }
        });
}

fn render_profile_badge(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) {
    let text = text.into();
    ui.label(
        egui::RichText::new(text)
            .small()
            .color(color)
            .background_color(color.gamma_multiply(0.10)),
    );
}
