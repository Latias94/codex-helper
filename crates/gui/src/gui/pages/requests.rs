use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "\u{8bf7}\u{6c42}", "Requests"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "\u{5f53}\u{524d}\u{672a}\u{8fd0}\u{884c}\u{4ee3}\u{7406}\u{ff0c}\u{4e5f}\u{672a}\u{9644}\u{7740}\u{5230}\u{73b0}\u{6709}\u{4ee3}\u{7406}\u{3002}\u{8bf7}\u{5728}\u{201c}\u{603b}\u{89c8}\u{201d}\u{91cc}\u{542f}\u{52a8}\u{6216}\u{9644}\u{7740}\u{540e}\u{518d}\u{67e5}\u{770b}\u{8bf7}\u{6c42}\u{3002}",
            "No proxy is running or attached. Start or attach on Overview to view requests.",
        ));
        return;
    };

    let last_error = snapshot.last_error.clone();
    let recent = snapshot.recent.clone();

    if let Some(err) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    let selected_sid = ctx
        .view
        .requests
        .focused_session_id
        .clone()
        .or_else(|| ctx.view.sessions.selected_session_id.clone());
    let selected_sid_ref = selected_sid.as_deref();

    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.requests.scope_session,
            pick(
                ctx.lang,
                "\u{8ddf}\u{968f}\u{6240}\u{9009}\u{4f1a}\u{8bdd}",
                "Scope to selected session",
            ),
        );
        ui.checkbox(
            &mut ctx.view.requests.errors_only,
            pick(ctx.lang, "\u{4ec5}\u{9519}\u{8bef}", "Errors only"),
        );
        if ui
            .button(pick(ctx.lang, "\u{5237}\u{65b0}", "Refresh"))
            .clicked()
        {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
        }
    });

    if ctx.view.requests.scope_session {
        ui.horizontal_wrapped(|ui| {
            if let Some(sid) = selected_sid_ref {
                ui.small(format!("session: {sid}"));
                if ctx.view.requests.focused_session_id.is_some() {
                    ui.small(pick(
                        ctx.lang,
                        "\u{ff08}\u{663e}\u{5f0f}\u{805a}\u{7126}\u{ff09}",
                        "(explicit focus)",
                    ));
                    if ui
                        .button(pick(
                            ctx.lang,
                            "\u{6539}\u{4e3a}\u{8ddf}\u{968f} Sessions",
                            "Follow Sessions instead",
                        ))
                        .clicked()
                    {
                        ctx.view.requests.focused_session_id = None;
                    }
                } else {
                    ui.small(pick(
                        ctx.lang,
                        "\u{ff08}\u{8ddf}\u{968f} Sessions\u{ff09}",
                        "(following Sessions)",
                    ));
                }
            } else {
                ui.small(pick(
                    ctx.lang,
                    "\u{5f53}\u{524d}\u{6ca1}\u{6709}\u{53ef}\u{7528}\u{4e8e}\u{9650}\u{5b9a}\u{7684} session_id\u{ff1b}\u{663e}\u{793a}\u{5168}\u{90e8}\u{8bf7}\u{6c42}\u{3002}",
                    "No session_id is available for scoping right now; all requests remain visible.",
                ));
            }
        });
    }

    ui.add_space(6.0);

    let filtered = recent
        .iter()
        .filter(|r| {
            if ctx.view.requests.errors_only && r.status_code < 400 {
                return false;
            }
            if ctx.view.requests.scope_session {
                match (selected_sid_ref, r.session_id.as_deref()) {
                    (Some(sid), Some(rid)) => sid == rid,
                    (Some(_), None) => false,
                    (None, _) => true,
                }
            } else {
                true
            }
        })
        .take(600)
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        ctx.view.requests.selected_idx = 0;
    } else {
        ctx.view.requests.selected_idx = ctx
            .view
            .requests
            .selected_idx
            .min(filtered.len().saturating_sub(1));
    }

    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "\u{5217}\u{8868}", "List"));
        cols[0].add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("requests_list_scroll")
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                let now = now_ms();
                for (pos, r) in filtered.iter().enumerate() {
                    let selected = pos == ctx.view.requests.selected_idx;
                    let age = format_age(now, Some(r.ended_at_ms));
                    let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
                    let model = r.model.as_deref().unwrap_or("-");
                    let cfg = r.station_name.as_deref().unwrap_or("-");
                    let pid = r.provider_id.as_deref().unwrap_or("-");
                    let path = shorten_middle(&r.path, 60);
                    let label = format!(
                        "{age}  st={}  {}ms  att={}  {}  {}  {}  {}",
                        r.status_code,
                        r.duration_ms,
                        attempts,
                        shorten(model, 18),
                        shorten(cfg, 14),
                        shorten(pid, 10),
                        path
                    );
                    if ui.selectable_label(selected, label).clicked() {
                        ctx.view.requests.selected_idx = pos;
                    }
                }
            });

        cols[1].heading(pick(ctx.lang, "\u{8be6}\u{60c5}", "Details"));
        cols[1].add_space(4.0);

        let Some(r) = filtered.get(ctx.view.requests.selected_idx).copied() else {
            cols[1].label(if ctx.view.requests.scope_session {
                if let Some(sid) = selected_sid_ref {
                    format!(
                        "{} {sid}",
                        pick(
                            ctx.lang,
                            "\u{5f53}\u{524d}\u{6ca1}\u{6709}\u{5339}\u{914d}\u{8fd9}\u{4e2a} session \u{7684}\u{8bf7}\u{6c42}\u{ff1a}",
                            "No requests currently match session:",
                        )
                    )
                } else {
                    pick(
                        ctx.lang,
                        "\u{5f53}\u{524d}\u{6ca1}\u{6709}\u{53ef}\u{5339}\u{914d}\u{7684}\u{8bf7}\u{6c42}\u{3002}",
                        "No requests match the current filters.",
                    )
                    .to_string()
                }
            } else {
                pick(
                    ctx.lang,
                    "\u{65e0}\u{8bf7}\u{6c42}\u{6570}\u{636e}\u{3002}",
                    "No requests match current filters.",
                )
                .to_string()
            });
            return;
        };

        cols[1].label(format!("id: {}", r.id));
        cols[1].label(format!("service: {}", r.service));
        cols[1].label(format!("method: {}", r.method));
        cols[1].label(format!("path: {}", r.path));
        cols[1].label(format!("status: {}", r.status_code));
        cols[1].label(format!("duration: {} ms", r.duration_ms));
        if let Some(ttfb) = r.ttfb_ms.filter(|v| *v > 0) {
            cols[1].label(format!("ttfb: {ttfb} ms"));
        }

        if let Some(sid) = r.session_id.as_deref() {
            cols[1].label(format!("session: {sid}"));
        }
        if let Some(cwd) = r.cwd.as_deref() {
            cols[1].label(format!("cwd: {cwd}"));
        }

        if let Some(sid) = r.session_id.as_deref() {
            cols[1].horizontal_wrapped(|ui| {
                if ui
                    .button(pick(
                        ctx.lang,
                        "\u{9650}\u{5b9a}\u{5230}\u{6b64} session",
                        "Focus this session",
                    ))
                    .clicked()
                {
                    prepare_select_requests_for_session(&mut ctx.view.requests, sid.to_string());
                }

                if ui
                    .button(pick(
                        ctx.lang,
                        "\u{5728} Sessions \u{67e5}\u{770b}",
                        "Open in Sessions",
                    ))
                    .clicked()
                {
                    focus_session_in_sessions(&mut ctx.view.sessions, sid.to_string());
                    prepare_select_requests_for_session(&mut ctx.view.requests, sid.to_string());
                    ctx.view.requested_page = Some(Page::Sessions);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "\u{5df2}\u{5207}\u{5230} Sessions \u{5e76}\u{5b9a}\u{4f4d}\u{5230}\u{5f53}\u{524d} session",
                            "Opened in Sessions and focused the current session",
                        )
                        .to_string(),
                    );
                }

                if ui
                    .button(pick(
                        ctx.lang,
                        "\u{5728} History \u{67e5}\u{770b}",
                        "Open in History",
                    ))
                    .clicked()
                {
                    match ctx
                        .rt
                        .block_on(crate::sessions::find_codex_session_file_by_id(sid))
                    {
                        Ok(path) => {
                            if let Some(summary) =
                                request_history_summary_from_request(r, path.clone(), ctx.lang)
                            {
                                history::prepare_select_session_from_external(
                                    &mut ctx.view.history,
                                    summary,
                                    history::ExternalHistoryOrigin::Requests,
                                );
                                ctx.view.requested_page = Some(Page::History);
                                *ctx.last_info = Some(
                                    if path.is_some() {
                                        pick(
                                            ctx.lang,
                                            "\u{5df2}\u{5207}\u{5230} History\u{ff08}\u{672c}\u{5730} transcript\u{ff09}",
                                            "Opened in History (local transcript)",
                                        )
                                    } else {
                                        pick(
                                            ctx.lang,
                                            "\u{5df2}\u{5207}\u{5230} History\u{ff08}\u{5171}\u{4eab}\u{89c2}\u{6d4b}\u{6458}\u{8981}\u{ff09}",
                                            "Opened in History (observed summary)",
                                        )
                                    }
                                    .to_string(),
                                );
                            }
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("find session file failed: {e}"));
                        }
                    }
                }
            });
        }

        cols[1].label(format!("model: {}", r.model.as_deref().unwrap_or("-")));
        cols[1].label(format!(
            "effort: {}",
            r.reasoning_effort.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "service_tier: {}",
            r.service_tier.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "station: {}",
            r.station_name.as_deref().unwrap_or("-")
        ));
        cols[1].label(format!(
            "provider: {}",
            r.provider_id.as_deref().unwrap_or("-")
        ));
        if let Some(u) = r.upstream_base_url.as_deref() {
            cols[1].label(format!("upstream: {}", shorten_middle(u, 80)));
        }

        if let Some(u) = r.usage.as_ref().filter(|u| u.total_tokens > 0) {
            cols[1].label(format!("usage: {}", usage_line(u)));

            let ttfb_ms = r.ttfb_ms.unwrap_or(0);
            let gen_ms = if ttfb_ms > 0 && ttfb_ms < r.duration_ms {
                r.duration_ms.saturating_sub(ttfb_ms)
            } else {
                r.duration_ms
            };
            if gen_ms > 0 && u.output_tokens > 0 {
                let out_tok_s = (u.output_tokens as f64) / (gen_ms as f64 / 1000.0);
                if out_tok_s.is_finite() && out_tok_s > 0.0 {
                    cols[1].label(format!("out_tok/s: {:.1}", out_tok_s));
                }
            }
        }

        cols[1].separator();
        cols[1].label(pick(
            ctx.lang,
            "\u{91cd}\u{8bd5} / \u{8def}\u{7531}\u{94fe}",
            "Retry / route chain",
        ));
        if let Some(retry) = r.retry.as_ref() {
            cols[1].label(format!("attempts: {}", retry.attempts));
            let max = 12usize;
            for (idx, entry) in retry.upstream_chain.iter().take(max).enumerate() {
                cols[1].label(format!("{:>2}. {}", idx + 1, shorten_middle(entry, 120)));
            }
            if retry.upstream_chain.len() > max {
                cols[1].label(format!(
                    "\u{2026} +{} more",
                    retry.upstream_chain.len() - max
                ));
            }
        } else {
            cols[1].label(pick(
                ctx.lang,
                "\u{ff08}\u{65e0}\u{91cd}\u{8bd5}\u{ff09}",
                "(no retries)",
            ));
        }
    });
}
