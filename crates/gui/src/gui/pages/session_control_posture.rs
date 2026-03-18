use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionControlTone {
    Positive,
    Neutral,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionControlPosture {
    pub(super) headline: String,
    pub(super) detail: String,
    pub(super) tone: SessionControlTone,
}

pub(super) fn session_has_manual_overrides(row: &SessionRow) -> bool {
    row.override_model.is_some()
        || row.override_station_name().is_some()
        || row.override_effort.is_some()
        || row.override_service_tier.is_some()
}

pub(super) fn session_override_field_labels(row: &SessionRow, lang: Language) -> Vec<String> {
    let mut fields = Vec::new();
    if row.override_station_name().is_some() {
        fields.push(pick(lang, "station", "station").to_string());
    }
    if row.override_model.is_some() {
        fields.push("model".to_string());
    }
    if row.override_effort.is_some() {
        fields.push(pick(lang, "reasoning", "reasoning").to_string());
    }
    if row.override_service_tier.is_some() {
        fields.push("service_tier".to_string());
    }
    fields
}

pub(super) fn session_manual_override_summary(row: &SessionRow, lang: Language) -> String {
    format!(
        "station={}, model={}, reasoning={}, service_tier={}",
        row.override_station_name().unwrap_or("-"),
        row.override_model.as_deref().unwrap_or("-"),
        row.override_effort.as_deref().unwrap_or("-"),
        format_service_tier_display(row.override_service_tier.as_deref(), lang, "-"),
    )
}

pub(super) fn session_binding_mode_label(
    mode: Option<SessionContinuityMode>,
    lang: Language,
) -> String {
    match mode {
        Some(SessionContinuityMode::DefaultProfile) => {
            pick(lang, "default_profile 继承", "default_profile inherited").to_string()
        }
        Some(SessionContinuityMode::ManualProfile) => {
            pick(lang, "手动应用", "manual apply").to_string()
        }
        None => pick(lang, "<无>", "<none>").to_string(),
    }
}

pub(super) fn session_control_posture(
    row: &SessionRow,
    profiles: &[ControlProfileOption],
    lang: Language,
) -> SessionControlPosture {
    if row.session_id.is_none() {
        return SessionControlPosture {
            headline: pick(
                lang,
                "这是没有 session_id 的聚合观测条目",
                "This is an aggregated entry without a session_id",
            )
            .to_string(),
            detail: pick(
                lang,
                "它只能展示观测到的请求路由，不能建立真正的 session binding 或单会话 overrides。",
                "It can only show observed request routing, and cannot own a real session binding or per-session overrides.",
            )
            .to_string(),
            tone: SessionControlTone::Warning,
        };
    }

    let override_fields = session_override_field_labels(row, lang);
    let override_summary = override_fields.join(", ");
    let bound_profile_exists = row
        .binding_profile_name
        .as_deref()
        .is_some_and(|name| profiles.iter().any(|profile| profile.name.as_str() == name));

    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        if bound_profile_exists {
            let headline = match row.binding_continuity_mode {
                Some(SessionContinuityMode::ManualProfile) => format!(
                    "{} {profile_name}",
                    pick(
                        lang,
                        "当前由 profile 手动绑定:",
                        "Currently manually bound to profile:"
                    )
                ),
                _ => format!(
                    "{} {profile_name}",
                    pick(
                        lang,
                        "当前由 profile 继承绑定:",
                        "Currently inherited from profile:",
                    )
                ),
            };
            let detail = if override_fields.is_empty() {
                match row.binding_continuity_mode {
                    Some(SessionContinuityMode::ManualProfile) => pick(
                        lang,
                        "这是显式 apply 到该 session 的 binding；除非重新应用别的 profile 或设置 session overrides，它会继续沿用。",
                        "This binding was explicitly applied to the session; it keeps applying until another profile is reapplied or session overrides replace part of it.",
                    )
                    .to_string(),
                    _ => pick(
                        lang,
                        "这是会话创建或恢复时继承的 binding；切换“新会话默认 profile”不会自动改写它。",
                        "This binding was inherited when the session was created or restored; switching the new-session default profile does not rewrite it automatically.",
                    )
                    .to_string(),
                }
            } else {
                format!(
                    "{} {}。{}",
                    pick(
                        lang,
                        "当前还有 session overrides 覆盖这些字段:",
                        "This session also has overrides on:",
                    ),
                    override_summary,
                    pick(
                        lang,
                        "这些字段优先于 binding / profile 默认。",
                        "Those fields take priority over the binding and profile defaults.",
                    )
                )
            };
            return SessionControlPosture {
                headline,
                detail,
                tone: SessionControlTone::Positive,
            };
        }

        return SessionControlPosture {
            headline: format!(
                "{} {profile_name}",
                pick(
                    lang,
                    "当前仍绑定到已缺失的 profile:",
                    "Still bound to a missing profile:",
                )
            ),
            detail: if override_fields.is_empty() {
                pick(
                    lang,
                    "当前工作台里已经找不到这个 profile；effective route 只能依赖 binding 快照和运行态结果继续解释。",
                    "The current workspace no longer contains this profile; the effective route can only be explained from the stored binding snapshot and runtime results.",
                )
                .to_string()
            } else {
                format!(
                    "{} {}。{}",
                    pick(
                        lang,
                        "当前工作台里已经找不到这个 profile；另外还有 session overrides:",
                        "The current workspace no longer contains this profile; there are also session overrides on:",
                    ),
                    override_summary,
                    pick(
                        lang,
                        "这些字段仍会覆盖 binding 快照里的默认值。",
                        "Those fields still override the defaults stored in the binding snapshot.",
                    )
                )
            },
            tone: SessionControlTone::Warning,
        };
    }

    if !override_fields.is_empty() {
        return SessionControlPosture {
            headline: pick(
                lang,
                "当前没有 profile binding，靠 session overrides 控制",
                "There is no profile binding; this session is controlled by session overrides",
            )
            .to_string(),
            detail: format!(
                "{} {}。{}",
                pick(
                    lang,
                    "当前 session 显式覆盖了",
                    "This session explicitly overrides",
                ),
                override_summary,
                pick(
                    lang,
                    "这些字段优先于 profile 默认、global pin 和请求默认。",
                    "Those fields take priority over profile defaults, the global pin, and request defaults.",
                )
            ),
            tone: SessionControlTone::Neutral,
        };
    }

    if let Some(station) = row
        .effective_station()
        .filter(|value| value.source == RouteValueSource::GlobalOverride)
        .map(|value| value.value.as_str())
    {
        return SessionControlPosture {
            headline: format!(
                "{} {station}",
                pick(
                    lang,
                    "当前没有 profile binding，站点跟随全局 pin:",
                    "There is no profile binding; station follows the global pin:",
                )
            ),
            detail: pick(
                lang,
                "如果全局 pin 切换，这个 session 的 effective station 也会一起变化。",
                "If the global pin changes, this session's effective station changes with it.",
            )
            .to_string(),
            tone: SessionControlTone::Neutral,
        };
    }

    SessionControlPosture {
        headline: pick(
            lang,
            "当前没有固定 binding",
            "This session has no fixed binding",
        )
        .to_string(),
        detail: pick(
            lang,
            "effective route 主要由最近请求值、active/default station 与运行态回填共同决定。",
            "The effective route is currently determined mostly by recent request values, the active/default station, and runtime fallback.",
        )
        .to_string(),
        tone: SessionControlTone::Neutral,
    }
}

pub(super) fn session_control_tone_color(tone: SessionControlTone) -> egui::Color32 {
    match tone {
        SessionControlTone::Positive => egui::Color32::from_rgb(60, 160, 90),
        SessionControlTone::Neutral => egui::Color32::from_rgb(120, 120, 120),
        SessionControlTone::Warning => egui::Color32::from_rgb(200, 120, 40),
    }
}
