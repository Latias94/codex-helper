use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct RoutingCandidate {
    pub name: String,
    pub alias: Option<String>,
    pub level: u8,
    pub enabled: bool,
    pub active: bool,
    pub upstreams: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceRoutingExplanation {
    #[serde(rename = "active_station")]
    pub active_station: Option<String>,
    pub mode: &'static str,
    #[serde(rename = "eligible_stations")]
    pub eligible_stations: Vec<RoutingCandidate>,
    #[serde(rename = "fallback_station")]
    pub fallback_station: Option<RoutingCandidate>,
}

fn routing_candidate(
    name: &str,
    svc: &ServiceConfig,
    active_name: Option<&str>,
) -> RoutingCandidate {
    RoutingCandidate {
        name: name.to_string(),
        alias: svc.alias.clone(),
        level: svc.level.clamp(1, 10),
        enabled: svc.enabled,
        active: active_name.is_some_and(|active| active == name),
        upstreams: svc.upstreams.len(),
    }
}

fn active_or_first_station(mgr: &ServiceConfigManager) -> Option<(String, &ServiceConfig)> {
    if let Some(active_name) = mgr.active.as_deref()
        && let Some(svc) = mgr.station(active_name)
    {
        return Some((active_name.to_string(), svc));
    }

    mgr.stations()
        .iter()
        .min_by_key(|(name, _)| *name)
        .map(|(name, svc)| (name.clone(), svc))
}

pub fn explain_service_routing(mgr: &ServiceConfigManager) -> ServiceRoutingExplanation {
    let active_name = mgr.active.as_deref();
    let mut eligible = mgr
        .stations()
        .iter()
        .filter(|(name, svc)| {
            !svc.upstreams.is_empty()
                && (svc.enabled || active_name.is_some_and(|active| active == name.as_str()))
        })
        .map(|(name, svc)| routing_candidate(name, svc, active_name))
        .collect::<Vec<_>>();

    let has_multi_level = {
        let mut levels = eligible
            .iter()
            .map(|candidate| candidate.level)
            .collect::<Vec<_>>();
        levels.sort_unstable();
        levels.dedup();
        levels.len() > 1
    };

    if !has_multi_level {
        eligible.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(active) = active_name
            && let Some(pos) = eligible
                .iter()
                .position(|candidate| candidate.name == active)
        {
            let item = eligible.remove(pos);
            eligible.insert(0, item);
        }

        if !eligible.is_empty() {
            return ServiceRoutingExplanation {
                active_station: mgr.active.clone(),
                mode: "single_level_multi",
                eligible_stations: eligible,
                fallback_station: None,
            };
        }

        return ServiceRoutingExplanation {
            active_station: mgr.active.clone(),
            mode: if active_or_first_station(mgr).is_some() {
                "single_level_fallback_active_station"
            } else {
                "single_level_empty"
            },
            eligible_stations: Vec::new(),
            fallback_station: active_or_first_station(mgr)
                .map(|(name, svc)| routing_candidate(&name, svc, active_name)),
        };
    }

    eligible.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| b.active.cmp(&a.active))
            .then_with(|| a.name.cmp(&b.name))
    });

    if !eligible.is_empty() {
        return ServiceRoutingExplanation {
            active_station: mgr.active.clone(),
            mode: "multi_level",
            eligible_stations: eligible,
            fallback_station: None,
        };
    }

    ServiceRoutingExplanation {
        active_station: mgr.active.clone(),
        mode: if active_or_first_station(mgr).is_some() {
            "multi_level_fallback_active_station"
        } else {
            "multi_level_empty"
        },
        eligible_stations: Vec::new(),
        fallback_station: active_or_first_station(mgr)
            .map(|(name, svc)| routing_candidate(&name, svc, active_name)),
    }
}
