use super::session_route_fields::{
    binding_profile_reference, effective_route_field_label, effective_route_field_value,
    route_value_source_label, unresolved_route_source_label,
};
use super::session_route_state::EffectiveRouteField;
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectiveRouteExplanation {
    pub(super) value: String,
    pub(super) source_label: String,
    pub(super) reason: String,
}

pub(super) fn explain_effective_route_field(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> EffectiveRouteExplanation {
    let value = effective_route_field_value(row, field);
    let value_label = value
        .map(|resolved| resolved.value.clone())
        .unwrap_or_else(|| "-".to_string());
    let source_label = value
        .map(|resolved| route_value_source_label(resolved.source, lang).to_string())
        .unwrap_or_else(|| unresolved_route_source_label(lang).to_string());
    let field_label = effective_route_field_label(field, lang);

    let reason = match value {
        Some(resolved) => match resolved.source {
            RouteValueSource::SessionOverride => format!(
                "{} {}={}，{}。",
                pick(
                    lang,
                    "当前 session 显式覆盖了",
                    "The current session explicitly overrides",
                ),
                field_label,
                resolved.value,
                pick(
                    lang,
                    "因此它优先于其他来源生效",
                    "so it takes priority over every other source",
                )
            ),
            RouteValueSource::GlobalOverride => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前 session 没有单独覆盖，命中了全局运行时覆盖，当前目标为",
                    "The current session has no dedicated override and therefore follows the global runtime override to",
                ),
                resolved.value,
                pick(
                    lang,
                    "所以这里以全局结果为准",
                    "so the global choice is authoritative here",
                )
            ),
            RouteValueSource::ProfileDefault => format!(
                "{} {}，{} {}={}。",
                pick(
                    lang,
                    "当前 session 绑定到",
                    "The current session is bound to"
                ),
                binding_profile_reference(row, lang),
                pick(lang, "其默认", "whose default"),
                field_label,
                resolved.value
            ),
            RouteValueSource::RequestPayload => format!(
                "{} {}，{}。",
                pick(
                    lang,
                    "当前没有 session override 或 profile 默认，沿用最近请求体里的",
                    "There is no session override or profile default, so the field follows the latest request payload for",
                ),
                field_label,
                resolved.value
            ),
            RouteValueSource::StationMapping => {
                super::session_route_reason_sources::station_mapping_explanation(
                    row, resolved, lang,
                )
            }
            RouteValueSource::RuntimeFallback => {
                super::session_route_reason_runtime::runtime_fallback_explanation(
                    row, field, resolved, lang,
                )
            }
        },
        None => {
            super::session_route_reason_runtime::unresolved_effective_route_reason(row, field, lang)
        }
    };

    EffectiveRouteExplanation {
        value: value_label,
        source_label,
        reason,
    }
}
