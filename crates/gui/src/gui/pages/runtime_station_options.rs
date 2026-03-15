use super::*;

pub(super) fn format_runtime_station_source(lang: Language, cfg: &StationOption) -> String {
    let mut parts = Vec::new();
    if let Some(enabled) = cfg.runtime_enabled_override {
        parts.push(format!(
            "{}={}",
            pick(lang, "启用", "enabled"),
            if enabled { "rt" } else { "rt-off" }
        ));
    }
    if cfg.runtime_level_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "等级", "level")));
    }
    if cfg.runtime_state_override.is_some() {
        parts.push(format!("{}=rt", pick(lang, "状态", "state")));
    }
    if parts.is_empty() {
        pick(lang, "站点配置", "station config").to_string()
    } else {
        parts.join(", ")
    }
}

pub(super) fn station_options_from_gui_stations(
    stations: &[StationOption],
) -> Vec<(String, String)> {
    let mut out = stations
        .iter()
        .map(|c| {
            let label = match c.alias.as_deref() {
                Some(a) if !a.trim().is_empty() => format!("{} ({a})", c.name),
                _ => c.name.clone(),
            };
            (c.name.clone(), label, c.level.clamp(1, 10))
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    out.into_iter().map(|(n, l, _)| (n, l)).collect()
}
