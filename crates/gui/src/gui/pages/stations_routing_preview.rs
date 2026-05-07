use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct StationsRoutingPreview {
    source: String,
    mode: String,
    eligible: Vec<String>,
    skipped: Vec<String>,
    retry_boundary: String,
    session_pin_note: Option<String>,
}

pub(super) fn render_stations_routing_preview(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    configured_active_station: Option<&str>,
) {
    let preview = build_stations_routing_preview(
        ctx.lang,
        &snapshot.stations,
        snapshot.global_station_override.as_deref(),
        configured_active_station,
        snapshot.session_station_overrides.len(),
        snapshot.resolved_retry.as_ref(),
        runtime_maps,
    );

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "Routing preview", "Routing preview"));
        ui.small(pick(
            ctx.lang,
            "按当前运行态预览新请求的 station 候选顺序；具体会话仍会先应用 session pin / profile binding。",
            "Preview the station candidate order for new requests under the current runtime state; concrete sessions still apply session pins and profile bindings first.",
        ));
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "来源", "Source"),
                preview.source
            ));
            ui.label(format!("{}: {}", pick(ctx.lang, "模式", "Mode"), preview.mode));
        });
        ui.small(preview.retry_boundary);
        if let Some(note) = preview.session_pin_note {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), note);
        }

        ui.add_space(4.0);
        ui.label(pick(ctx.lang, "候选顺序", "Candidate order"));
        if preview.eligible.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(ctx.lang, "<无可用候选>", "<no eligible candidates>"),
            );
        } else {
            for (index, item) in preview.eligible.iter().enumerate() {
                ui.small(format!("{}. {item}", index + 1));
            }
        }

        if !preview.skipped.is_empty() {
            ui.add_space(4.0);
            ui.label(pick(ctx.lang, "跳过原因", "Skipped"));
            for item in &preview.skipped {
                ui.small(item);
            }
        }
    });
}

fn build_stations_routing_preview(
    lang: Language,
    stations: &[StationOption],
    global_station_override: Option<&str>,
    configured_active_station: Option<&str>,
    session_pin_count: usize,
    retry: Option<&crate::config::ResolvedRetryConfig>,
    runtime_maps: &RuntimeStationMaps,
) -> StationsRoutingPreview {
    let mut preview =
        if let Some(global_pin) = global_station_override.and_then(non_empty_trimmed_str) {
            build_pinned_routing_preview(lang, stations, global_pin, runtime_maps)
        } else {
            build_auto_routing_preview(lang, stations, configured_active_station, runtime_maps)
        };

    preview.retry_boundary = retry_boundary_text(lang, retry);
    preview.session_pin_note = session_pin_note(lang, session_pin_count);
    preview
}

fn build_pinned_routing_preview(
    lang: Language,
    stations: &[StationOption],
    global_pin: &str,
    runtime_maps: &RuntimeStationMaps,
) -> StationsRoutingPreview {
    let mut eligible = Vec::new();
    let mut skipped = Vec::new();

    match stations.iter().find(|station| station.name == global_pin) {
        Some(station) => {
            let reasons = pinned_skip_reasons(lang, station, runtime_maps);
            if reasons.is_empty() {
                eligible.push(format!(
                    "{} {}",
                    station_label(station, None, runtime_maps),
                    pick(
                        lang,
                        "(global pin；draining/half-open 仍允许被固定路由使用)",
                        "(global pin; draining/half-open remain usable for pinned routing)",
                    )
                ));
            } else {
                skipped.push(format!("{}: {}", station.name, reasons.join(", ")));
            }
        }
        None => skipped.push(match lang {
            Language::Zh => format!("global pin 目标 {global_pin} 不在当前 station 列表中"),
            Language::En => {
                format!("global pin target {global_pin} is not in the current station list")
            }
        }),
    }

    StationsRoutingPreview {
        source: match lang {
            Language::Zh => format!("global pin={global_pin}"),
            Language::En => format!("global pin={global_pin}"),
        },
        mode: pick(lang, "pinned station", "pinned station").to_string(),
        eligible,
        skipped,
        retry_boundary: String::new(),
        session_pin_note: None,
    }
}

fn build_auto_routing_preview(
    lang: Language,
    stations: &[StationOption],
    configured_active_station: Option<&str>,
    runtime_maps: &RuntimeStationMaps,
) -> StationsRoutingPreview {
    let configured_active_station = configured_active_station.and_then(non_empty_trimmed_str);
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for station in stations {
        let reasons =
            automatic_skip_reasons(lang, station, configured_active_station, runtime_maps);
        if reasons.is_empty() {
            candidates.push(station.clone());
        } else {
            skipped.push(format!("{}: {}", station.name, reasons.join(", ")));
        }
    }

    let mut levels = candidates
        .iter()
        .map(|station| station.level.clamp(1, 10))
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    let has_multi_level = levels.len() > 1;

    if has_multi_level {
        candidates.sort_by(|a, b| {
            a.level
                .clamp(1, 10)
                .cmp(&b.level.clamp(1, 10))
                .then_with(|| {
                    station_is_configured_active(b, configured_active_station)
                        .cmp(&station_is_configured_active(a, configured_active_station))
                })
                .then_with(|| a.name.cmp(&b.name))
        });
    } else {
        candidates.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(active) = configured_active_station
            && let Some(pos) = candidates.iter().position(|station| station.name == active)
        {
            let item = candidates.remove(pos);
            candidates.insert(0, item);
        }
    }

    let eligible = candidates
        .iter()
        .map(|station| station_label(station, configured_active_station, runtime_maps))
        .collect::<Vec<_>>();
    let mode = if has_multi_level {
        pick(lang, "auto / level fallback", "auto / level fallback")
    } else {
        pick(
            lang,
            "auto / single-level fallback",
            "auto / single-level fallback",
        )
    };
    let source = configured_active_station
        .map(|active| match lang {
            Language::Zh => format!("configured active_station={active}"),
            Language::En => format!("configured active_station={active}"),
        })
        .unwrap_or_else(|| {
            pick(
                lang,
                "auto / no configured active",
                "auto / no configured active",
            )
            .to_string()
        });

    StationsRoutingPreview {
        source,
        mode: mode.to_string(),
        eligible,
        skipped,
        retry_boundary: String::new(),
        session_pin_note: None,
    }
}

fn automatic_skip_reasons(
    lang: Language,
    station: &StationOption,
    configured_active_station: Option<&str>,
    runtime_maps: &RuntimeStationMaps,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !station.enabled && !station_is_configured_active(station, configured_active_station) {
        reasons.push(pick(lang, "disabled", "disabled").to_string());
    }
    if station.runtime_state != RuntimeConfigState::Normal {
        reasons.push(match lang {
            Language::Zh => format!(
                "state={} 不参与自动路由",
                runtime_config_state_label(lang, station.runtime_state)
            ),
            Language::En => format!(
                "state={} is not eligible for automatic routing",
                runtime_config_state_label(lang, station.runtime_state)
            ),
        });
    }
    if station_upstream_count(runtime_maps, station.name.as_str()) == Some(0) {
        reasons.push(pick(lang, "no routable upstreams", "no routable upstreams").to_string());
    }
    reasons
}

fn pinned_skip_reasons(
    lang: Language,
    station: &StationOption,
    runtime_maps: &RuntimeStationMaps,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if station.runtime_state == RuntimeConfigState::BreakerOpen {
        reasons.push(
            pick(
                lang,
                "breaker_open blocks pinned routing",
                "breaker_open blocks pinned routing",
            )
            .to_string(),
        );
    }
    if station_upstream_count(runtime_maps, station.name.as_str()) == Some(0) {
        reasons.push(pick(lang, "no routable upstreams", "no routable upstreams").to_string());
    }
    reasons
}

fn retry_boundary_text(
    lang: Language,
    retry: Option<&crate::config::ResolvedRetryConfig>,
) -> String {
    let Some(retry) = retry else {
        return pick(
            lang,
            "retry: resolved policy 暂不可见；跨站边界未知。",
            "retry: resolved policy is not visible yet; cross-station boundaries are unknown.",
        )
        .to_string();
    };

    let provider_failover =
        retry.provider.strategy == RetryStrategy::Failover && retry.provider.max_attempts > 1;
    if retry.allow_cross_station_before_first_output && provider_failover {
        return match lang {
            Language::Zh => format!(
                "retry: provider failover x{}；首包前可按候选顺序跨 station，首包后固定在当前 station。",
                retry.provider.max_attempts
            ),
            Language::En => format!(
                "retry: provider failover x{}; may cross stations in candidate order before first output, then stays on the current station.",
                retry.provider.max_attempts
            ),
        };
    }

    if retry.provider.max_attempts > 1 {
        return match lang {
            Language::Zh => format!(
                "retry: provider {} x{}；当前策略不允许首包前跨 station，失败会先留在已选 station 内。",
                retry_strategy_label(retry.provider.strategy),
                retry.provider.max_attempts
            ),
            Language::En => format!(
                "retry: provider {} x{}; cross-station failover before first output is disabled, so failures stay inside the selected station first.",
                retry_strategy_label(retry.provider.strategy),
                retry.provider.max_attempts
            ),
        };
    }

    pick(
        lang,
        "retry: provider 只有一次尝试；自动切换主要依赖下一次请求重新选路。",
        "retry: provider has one attempt; automatic switching mainly happens when the next request is routed.",
    )
    .to_string()
}

fn session_pin_note(lang: Language, session_pin_count: usize) -> Option<String> {
    (session_pin_count > 0).then(|| match lang {
        Language::Zh => format!(
            "{session_pin_count} 个会话有 station pin；这些会话会先使用自己的 pin，再看 global/auto 策略。"
        ),
        Language::En => format!(
            "{session_pin_count} sessions have station pins; those sessions use their own pins before global/auto policy."
        ),
    })
}

fn station_label(
    station: &StationOption,
    configured_active_station: Option<&str>,
    runtime_maps: &RuntimeStationMaps,
) -> String {
    let mut parts = vec![format!("L{}", station.level.clamp(1, 10))];
    if station_is_configured_active(station, configured_active_station) {
        parts.push("active".to_string());
    }
    if let Some(upstreams) = station_upstream_count(runtime_maps, station.name.as_str()) {
        parts.push(format!("upstreams={upstreams}"));
    }
    if station.runtime_state != RuntimeConfigState::Normal {
        parts.push(format!(
            "state={}",
            runtime_config_state_label(Language::En, station.runtime_state)
        ));
    }

    match station
        .alias
        .as_deref()
        .filter(|alias| !alias.trim().is_empty())
    {
        Some(alias) => format!("{} ({alias}) [{}]", station.name, parts.join(", ")),
        None => format!("{} [{}]", station.name, parts.join(", ")),
    }
}

fn station_upstream_count(runtime_maps: &RuntimeStationMaps, station_name: &str) -> Option<usize> {
    runtime_maps
        .lb_view
        .get(station_name)
        .map(|view| view.upstreams.len())
}

fn station_is_configured_active(
    station: &StationOption,
    configured_active_station: Option<&str>,
) -> bool {
    configured_active_station == Some(station.name.as_str())
}

fn non_empty_trimmed_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(
        name: &str,
        enabled: bool,
        level: u8,
        runtime_state: RuntimeConfigState,
    ) -> StationOption {
        StationOption {
            name: name.to_string(),
            alias: None,
            enabled,
            level,
            configured_enabled: enabled,
            configured_level: level,
            runtime_enabled_override: None,
            runtime_level_override: None,
            runtime_state,
            runtime_state_override: None,
            capabilities: StationCapabilitySummary::default(),
        }
    }

    fn runtime_maps(upstream_counts: &[(&str, usize)]) -> RuntimeStationMaps {
        RuntimeStationMaps {
            lb_view: upstream_counts
                .iter()
                .map(|(name, count)| {
                    (
                        (*name).to_string(),
                        LbConfigView {
                            last_good_index: None,
                            upstreams: vec![Default::default(); *count],
                        },
                    )
                })
                .collect(),
            ..RuntimeStationMaps::default()
        }
    }

    #[test]
    fn auto_routing_preview_puts_single_level_active_first_and_skips_blocked() {
        let stations = vec![
            station("alpha", true, 1, RuntimeConfigState::Normal),
            station("beta", true, 1, RuntimeConfigState::Normal),
            station("disabled", false, 1, RuntimeConfigState::Normal),
            station("drain", true, 1, RuntimeConfigState::Draining),
        ];
        let maps = runtime_maps(&[("alpha", 1), ("beta", 1), ("disabled", 1), ("drain", 1)]);

        let preview = build_stations_routing_preview(
            Language::En,
            &stations,
            None,
            Some("beta"),
            0,
            Some(&RetryProfileName::Balanced.defaults()),
            &maps,
        );

        assert_eq!(preview.mode, "auto / single-level fallback");
        assert!(preview.eligible[0].starts_with("beta"));
        assert!(preview.eligible[1].starts_with("alpha"));
        assert!(
            preview
                .skipped
                .iter()
                .any(|item| item.contains("disabled: disabled"))
        );
        assert!(
            preview
                .skipped
                .iter()
                .any(|item| item.contains("drain") && item.contains("automatic routing"))
        );
    }

    #[test]
    fn auto_routing_preview_sorts_multi_level_before_active_tiebreak() {
        let stations = vec![
            station("alpha", true, 2, RuntimeConfigState::Normal),
            station("beta", true, 1, RuntimeConfigState::Normal),
            station("zeta", true, 2, RuntimeConfigState::Normal),
        ];
        let maps = runtime_maps(&[("alpha", 1), ("beta", 1), ("zeta", 1)]);

        let preview = build_stations_routing_preview(
            Language::En,
            &stations,
            None,
            Some("zeta"),
            0,
            None,
            &maps,
        );

        assert_eq!(preview.mode, "auto / level fallback");
        assert!(preview.eligible[0].starts_with("beta"));
        assert!(preview.eligible[1].starts_with("zeta"));
        assert!(preview.eligible[2].starts_with("alpha"));
    }

    #[test]
    fn pinned_routing_preview_allows_draining_but_blocks_breaker_open() {
        let maps = runtime_maps(&[("drain", 1), ("breaker", 1)]);
        let draining = build_stations_routing_preview(
            Language::En,
            &[station("drain", false, 1, RuntimeConfigState::Draining)],
            Some("drain"),
            None,
            0,
            None,
            &maps,
        );
        assert_eq!(draining.mode, "pinned station");
        assert!(draining.eligible[0].contains("global pin"));

        let blocked = build_stations_routing_preview(
            Language::En,
            &[station("breaker", true, 1, RuntimeConfigState::BreakerOpen)],
            Some("breaker"),
            None,
            0,
            None,
            &maps,
        );
        assert!(blocked.eligible.is_empty());
        assert!(blocked.skipped[0].contains("breaker_open blocks pinned routing"));
    }

    #[test]
    fn retry_boundary_text_explains_before_and_after_first_output() {
        let preview = build_stations_routing_preview(
            Language::En,
            &[station("alpha", true, 1, RuntimeConfigState::Normal)],
            None,
            Some("alpha"),
            2,
            Some(&RetryProfileName::AggressiveFailover.defaults()),
            &runtime_maps(&[("alpha", 1)]),
        );

        assert!(preview.retry_boundary.contains("before first output"));
        assert!(
            preview
                .retry_boundary
                .contains("stays on the current station")
        );
        assert!(
            preview
                .session_pin_note
                .as_deref()
                .is_some_and(|note| note.contains("2 sessions"))
        );
    }
}
