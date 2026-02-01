#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = codex_helper_gui::run() {
        eprintln!("codex-helper-gui failed: {e}");
        std::process::exit(1);
    }
}
