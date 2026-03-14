use super::*;

pub(super) fn station_mapping_explanation(
    row: &SessionRow,
    resolved: &ResolvedRouteValue,
    lang: Language,
) -> String {
    let requested_model = row.last_model.as_deref().unwrap_or("-");
    let station = row
        .effective_station_name()
        .or(row.last_station_name())
        .unwrap_or("-");
    let upstream = row.last_upstream_base_url.as_deref().unwrap_or("-");
    format!(
        "{} {}，{} {} / {} {}，{} {}。",
        pick(
            lang,
            "最近请求提交的模型是",
            "The most recent request submitted model",
        ),
        requested_model,
        pick(lang, "但站点", "but station"),
        station,
        pick(lang, "upstream", "upstream"),
        upstream,
        pick(
            lang,
            "的 model mapping 将实际模型改写为",
            "rewrote the effective model through model mapping to",
        ),
        resolved.value
    )
}
