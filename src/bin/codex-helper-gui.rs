#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    eprintln!(
        "warning: codex-helper-gui (egui) is deprecated; use the Tauri desktop \
         client for the Windows replacement path. The egui binary remains \
         available as a legacy fallback."
    );

    if let Err(e) = codex_helper_gui::run() {
        eprintln!("codex-helper-gui failed: {e}");
        std::process::exit(1);
    }
}
