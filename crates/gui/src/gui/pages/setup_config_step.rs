use super::view_state::{SetupConfigInitLoad, SetupViewState};
use super::*;
use std::sync::mpsc::TryRecvError;

fn cancel_setup_config_init(state: &mut SetupViewState) {
    if let Some(load) = state.config_init_load.take() {
        load.join.abort();
    }
}

pub(super) fn poll_setup_config_init(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.setup.config_init_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.setup.config_init_load = None;
                return;
            }

            ctx.view.setup.config_init_load = None;
            match res {
                Ok((path, text)) => {
                    *ctx.proxy_settings_text = text;
                    *ctx.last_info = Some(format!(
                        "{}: {}",
                        pick(ctx.lang, "已写入设置", "Wrote settings"),
                        path.display()
                    ));
                    *ctx.last_error = None;
                }
                Err(err) => {
                    *ctx.last_error = Some(format!("init settings failed: {err}"));
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.setup.config_init_load = None;
        }
    }
}

fn start_setup_config_init(ctx: &mut PageCtx<'_>) {
    cancel_setup_config_init(&mut ctx.view.setup);
    ctx.view.setup.config_init_seq = ctx.view.setup.config_init_seq.saturating_add(1);
    let seq = ctx.view.setup.config_init_seq;
    let import = ctx.view.setup.import_codex_on_init;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = async {
            let path = crate::config::init_config_toml(false, import).await?;
            let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            Ok::<_, anyhow::Error>((path, text))
        }
        .await;
        let _ = tx.send((seq, result));
    });
    ctx.view.setup.config_init_load = Some(SetupConfigInitLoad { seq, rx, join });
}

pub(super) fn render_setup_config_step(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let cfg_path = ctx.proxy_settings_path.to_path_buf();
    let cfg_exists = cfg_path.exists() && !ctx.proxy_settings_text.trim().is_empty();

    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "1) 生成/导入配置",
            "1) Create/import proxy settings",
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "设置文件", "Settings file"),
            cfg_path.display()
        ));

        if cfg_exists {
            ui.colored_label(
                egui::Color32::from_rgb(60, 160, 90),
                pick(ctx.lang, "已就绪", "Ready"),
            );
            if ui
                .button(pick(ctx.lang, "打开设置文件", "Open settings file"))
                .clicked()
                && let Err(e) = open_in_file_manager(&cfg_path, true)
            {
                *ctx.last_error = Some(format!("open settings failed: {e}"));
            }
            if ui
                .button(pick(ctx.lang, "前往代理设置页", "Go to Proxy Settings"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::ProxySettings);
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "未检测到有效配置（建议先创建）",
                    "Settings file not found (create one first)",
                ),
            );
            ui.checkbox(
                &mut ctx.view.setup.import_codex_on_init,
                pick(
                    ctx.lang,
                    "自动从 ~/.codex/config.toml + auth.json 导入 Codex upstream",
                    "Auto-import Codex upstreams from ~/.codex/config.toml + auth.json",
                ),
            );
            if ctx.view.setup.config_init_load.is_some() {
                ui.label(pick(
                    ctx.lang,
                    "正在创建设置文件...",
                    "Creating settings file...",
                ));
            }

            if ui
                .add_enabled(
                    ctx.view.setup.config_init_load.is_none(),
                    egui::Button::new(pick(
                        ctx.lang,
                        "创建设置文件（config.toml）",
                        "Create settings file (config.toml)",
                    )),
                )
                .clicked()
            {
                start_setup_config_init(ctx);
            }
        }
    });
}
