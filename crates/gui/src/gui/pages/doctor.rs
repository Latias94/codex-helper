use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "诊断", "Doctor"));
    ui.label(pick(
        ctx.lang,
        "用于排查：配置是否可读、env 是否缺失、Codex CLI 配置/认证文件是否存在、自动导入链路是否可用、日志与用量提供商配置是否正常。",
        "Helps diagnose: config readability, missing env vars, Codex CLI config/auth presence, auto-import viability, logs and usage providers.",
    ));
    ui.separator();

    let lang = match ctx.lang {
        Language::En => DoctorLang::En,
        _ => DoctorLang::Zh,
    };

    ui.horizontal(|ui| {
        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            ctx.view.doctor.report = None;
            ctx.view.doctor.last_error = None;
            ctx.view.doctor.loaded_at_ms = None;
        }

        if ui
            .button(pick(ctx.lang, "复制 JSON", "Copy JSON"))
            .clicked()
        {
            if let Some(r) = ctx.view.doctor.report.as_ref() {
                let text = serde_json::to_string_pretty(r)
                    .unwrap_or_else(|_| "{\"checks\":[]}".to_string());
                ui.ctx().copy_text(text);
                *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
            } else {
                *ctx.last_error =
                    Some(pick(ctx.lang, "尚未加载报告", "Report not loaded").to_string());
            }
        }

        if ui
            .button(pick(ctx.lang, "打开设置文件", "Open settings file"))
            .clicked()
        {
            let path = crate::config::config_file_path();
            if let Err(e) = open_in_file_manager(&path, true) {
                *ctx.last_error = Some(format!("open settings failed: {e}"));
            }
        }

        if ui
            .button(pick(ctx.lang, "打开日志目录", "Open logs folder"))
            .clicked()
        {
            let dir = crate::config::proxy_home_dir().join("logs");
            if let Err(e) = open_in_file_manager(&dir, false) {
                *ctx.last_error = Some(format!("open logs failed: {e}"));
            }
        }
    });

    if ctx.view.doctor.report.is_none() && ctx.view.doctor.last_error.is_none() {
        let report = ctx.rt.block_on(crate::doctor::run_doctor(lang));
        ctx.view.doctor.loaded_at_ms = Some(now_ms());
        ctx.view.doctor.report = Some(report);
    }

    let Some(report) = ctx.view.doctor.report.as_ref() else {
        if let Some(err) = ctx.view.doctor.last_error.as_deref() {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        } else {
            ui.label(pick(ctx.lang, "暂无报告", "No report"));
        }
        return;
    };

    fn status_color(st: DoctorStatus) -> egui::Color32 {
        match st {
            DoctorStatus::Ok => egui::Color32::from_rgb(60, 160, 90),
            DoctorStatus::Info => egui::Color32::from_rgb(80, 160, 200),
            DoctorStatus::Warn => egui::Color32::from_rgb(200, 120, 40),
            DoctorStatus::Fail => egui::Color32::from_rgb(200, 60, 60),
        }
    }

    let mut ok = 0usize;
    let mut info = 0usize;
    let mut warn = 0usize;
    let mut fail = 0usize;
    for c in &report.checks {
        match c.status {
            DoctorStatus::Ok => ok += 1,
            DoctorStatus::Info => info += 1,
            DoctorStatus::Warn => warn += 1,
            DoctorStatus::Fail => fail += 1,
        }
    }

    ui.label(format!(
        "{}: OK {ok} | INFO {info} | WARN {warn} | FAIL {fail}",
        pick(ctx.lang, "汇总", "Summary")
    ));
    if let Some(ts) = ctx.view.doctor.loaded_at_ms {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "加载时间(ms)", "Loaded at (ms)"),
            ts
        ));
    }

    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("doctor_report_scroll")
        .show(ui, |ui| {
            for c in &report.checks {
                ui.horizontal(|ui| {
                    let label = match c.status {
                        DoctorStatus::Ok => "OK",
                        DoctorStatus::Info => "INFO",
                        DoctorStatus::Warn => "WARN",
                        DoctorStatus::Fail => "FAIL",
                    };
                    ui.colored_label(status_color(c.status), label);
                    ui.label(c.id);
                });
                ui.label(&c.message);
                ui.separator();
            }
        });
}
