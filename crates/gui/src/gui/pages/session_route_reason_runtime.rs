use super::session_route_fields::effective_route_field_label;
use super::session_route_state::EffectiveRouteField;
use super::*;

pub(super) fn runtime_fallback_explanation(
    row: &SessionRow,
    field: EffectiveRouteField,
    value: &ResolvedRouteValue,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => match row.last_station_name() {
            Some(last_station) if last_station == value.value => pick(
                lang,
                "当前没有 session pin、global pin 或 profile 默认，沿用最近观测到的站点。",
                "No session pin, global pin, or profile default applies, so the station falls back to the most recently observed value.",
            )
            .to_string(),
            Some(last_station) => format!(
                "{} {}；{} {}。",
                pick(
                    lang,
                    "当前没有 session pin、global pin 或 profile 默认，运行态把站点回填为",
                    "No session pin, global pin, or profile default applies, so runtime filled the station as",
                ),
                value.value,
                pick(
                    lang,
                    "最近观测到的站点仍是",
                    "while the most recently observed station is still",
                ),
                last_station
            ),
            None => format!(
                "{} {}。",
                pick(
                    lang,
                    "当前没有更明确的站点来源，运行态回填为",
                    "No more explicit station source is available, so runtime filled it as",
                ),
                value.value
            ),
        },
        EffectiveRouteField::Upstream => {
            let effective_station = row.effective_station_name();
            match (
                effective_station,
                row.last_station_name(),
                row.last_upstream_base_url.as_deref(),
            ) {
                (Some(station), Some(last_station), Some(last_upstream))
                    if station == last_station && last_upstream == value.value =>
                {
                    format!(
                        "{} {}，{} {}。",
                        pick(
                            lang,
                            "当前生效站点与最近观测一致，沿用该站点最近命中的 upstream",
                            "The effective station matches the last observed station, so the upstream falls back to the most recently observed target",
                        ),
                        value.value,
                        pick(lang, "所属站点", "for station"),
                        station
                    )
                }
                (Some(station), _, _) => format!(
                    "{} {}，{} {}。",
                    pick(
                        lang,
                        "当前站点可在运行态唯一补全 upstream",
                        "The current station can be completed to a single upstream at runtime",
                    ),
                    value.value,
                    pick(lang, "所属站点", "for station"),
                    station
                ),
                _ => format!(
                    "{} {}。",
                    pick(
                        lang,
                        "运行态补全了当前 upstream",
                        "Runtime completed the current upstream as",
                    ),
                    value.value
                ),
            }
        }
        _ => format!(
            "{} {}，{}。",
            pick(
                lang,
                "当前没有更高优先级的覆盖或默认值，沿用最近观测到的",
                "No higher-priority override or default applies, so the field falls back to the most recently observed",
            ),
            effective_route_field_label(field, lang),
            value.value
        ),
    }
}

pub(super) fn unresolved_effective_route_reason(
    row: &SessionRow,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Station => pick(
            lang,
            "当前没有 session pin、global pin、profile 默认，也没有最近可用的站点记录。",
            "There is no session pin, global pin, profile default, or recent station observation to resolve the current station.",
        )
        .to_string(),
        EffectiveRouteField::Upstream => {
            let effective_station = row.effective_station_name();
            match (effective_station, row.last_station_name()) {
                (Some(station), Some(last_station))
                    if station != last_station && row.last_upstream_base_url.is_some() =>
                {
                    format!(
                        "{} {}，{} {}，{}。",
                        pick(
                            lang,
                            "当前生效站点已经切到",
                            "The effective station has already switched to",
                        ),
                        station,
                        pick(
                            lang,
                            "但最近观测到的 upstream 仍属于站点",
                            "but the most recently observed upstream still belongs to station",
                        ),
                        last_station,
                        pick(
                            lang,
                            "所以不能直接把它当成当前 upstream",
                            "so it cannot be treated as the current upstream",
                        )
                    )
                }
                (Some(station), _) => format!(
                    "{} {}，{}。",
                    pick(lang, "当前站点是", "The current station is"),
                    station,
                    pick(
                        lang,
                        "但缺少最近 upstream 观测或唯一映射，因此暂时无法解释 upstream",
                        "but there is no recent upstream observation or unique mapping, so the upstream cannot be explained yet",
                    )
                ),
                (None, _) => pick(
                    lang,
                    "当前连 effective station 都还没有判定，因此无法解释 upstream。",
                    "The effective station itself is still unresolved, so the upstream cannot be explained.",
                )
                .to_string(),
            }
        }
        _ => format!(
            "{} {}。",
            pick(
                lang,
                "当前既没有覆盖、profile 默认，也没有最近请求值，无法判定",
                "There is no override, profile default, or recent request value to resolve",
            ),
            effective_route_field_label(field, lang)
        ),
    }
}
