use eframe::egui;

use super::*;

pub(super) fn render_history_header(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    remote_attached: bool,
    shared_observed_history_available: bool,
    attached_host_local_history_advertised: bool,
) {
    ui.heading(pick(ctx.lang, "历史会话", "History"));
    ui.label(pick(
        ctx.lang,
        "读取 Codex 的本地 sessions（~/.codex/sessions）。",
        "Reads local Codex sessions (~/.codex/sessions).",
    ));
    if !remote_attached {
        return;
    }

    ui.add_space(6.0);
    ui.group(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            if shared_observed_history_available {
                pick(
                    ctx.lang,
                    "当前附着的是远端代理。本页优先读取这台设备自己的 ~/.codex/sessions；若本机 history 为空，会退到共享观测摘要，但那仍不代表远端 host 的原始 transcript 文件。",
                    "A remote proxy is attached. This page prefers this device's ~/.codex/sessions; if local history is empty it falls back to shared observed summaries, which still do not represent the remote host's raw transcript files.",
                )
            } else {
                pick(
                    ctx.lang,
                    "当前附着的是远端代理。本页仍只会读取这台设备自己的 ~/.codex/sessions；当前附着目标未声明共享 history 观测能力，因此无法退到共享观测摘要。",
                    "A remote proxy is attached. This page still reads only this device's ~/.codex/sessions; the attached target does not advertise shared history observability, so observed fallback is unavailable.",
                )
            },
        );
        ui.small(if shared_observed_history_available {
            pick(
                ctx.lang,
                "当本机 history 为空时，本页会退到共享观测摘要；更完整的 session / route / request 观测仍建议看 Sessions 或 Requests。",
                "When local history is empty, this page falls back to shared observed summaries; use Sessions or Requests for fuller session/route/request observability.",
            )
        } else {
            pick(
                ctx.lang,
                "当前模式下 host-local transcript / cwd 仍不会直接映射到远端机器；如需更完整的共享观测，请查看 Sessions 或 Requests。",
                "In this mode, host-local transcript/cwd access still does not map to the remote machine; use Sessions or Requests for fuller shared observability.",
            )
        });
        if let Some(att) = ctx.proxy.attached()
            && let Some(warning) = remote_local_only_warning_message(
                att.admin_base_url.as_str(),
                &att.host_local_capabilities,
                ctx.lang,
                &[
                    pick(ctx.lang, "resume", "resume"),
                    pick(ctx.lang, "open file", "open file"),
                    pick(ctx.lang, "transcript", "transcript"),
                ],
            )
        {
            ui.small(warning);
        }
        if attached_host_local_history_advertised {
            ui.small(pick(
                ctx.lang,
                "附着目标声明其代理主机本地具备 session history 能力，但那不会自动映射为当前设备可读的 transcript 文件。",
                "The attached target advertises host-local session history on its own machine, but that does not automatically map to transcript files readable from this device.",
            ));
        }
        ui.horizontal(|ui| {
            if ui.button(pick(ctx.lang, "转到会话", "Go to Sessions")).clicked() {
                ctx.view.requested_page = Some(Page::Sessions);
            }
            if ui.button(pick(ctx.lang, "转到请求", "Go to Requests")).clicked() {
                ctx.view.requested_page = Some(Page::Requests);
            }
        });
    });
}
