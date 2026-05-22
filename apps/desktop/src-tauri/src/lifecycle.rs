use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Listener, Manager, Runtime, WebviewWindow, Window};

use crate::error::{CommandError, DesktopError};

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "codex-helper-main-tray";
const MENU_SHOW_WINDOW: &str = "show-window";
const MENU_HIDE_TO_TRAY: &str = "hide-to-tray";
const MENU_QUIT_APP: &str = "quit-app";
const CLOSE_REQUESTED_EVENT: &str = "codex-helper://close-requested";
const WINDOW_READY_EVENT: &str = "codex-helper://window-ready";

#[derive(Debug, Default)]
pub(crate) struct DesktopLifecycleState {
    quit_requested: AtomicBool,
}

#[allow(dead_code)]
pub(crate) struct DesktopTrayIcon<R: Runtime>(TrayIcon<R>);

impl DesktopLifecycleState {
    pub(crate) fn request_quit(&self) {
        self.quit_requested.store(true, Ordering::SeqCst);
    }

    pub(crate) fn quit_requested(&self) -> bool {
        self.quit_requested.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowCloseDecision {
    HideToTray,
    AllowClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppExitRuntimeEffect {
    LeaveRuntimeRunning,
}

pub(crate) fn decide_window_close(quit_requested: bool) -> WindowCloseDecision {
    if quit_requested {
        WindowCloseDecision::AllowClose
    } else {
        WindowCloseDecision::HideToTray
    }
}

pub(crate) fn normal_app_exit_runtime_effect() -> AppExitRuntimeEffect {
    AppExitRuntimeEffect::LeaveRuntimeRunning
}

pub(crate) fn second_instance_launch_runtime_effect() -> AppExitRuntimeEffect {
    AppExitRuntimeEffect::LeaveRuntimeRunning
}

pub(crate) fn setup_tray<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW_WINDOW, "Show Window", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, MENU_HIDE_TO_TRAY, "Hide to Tray", true, None::<&str>)?;
    let quit = MenuItem::with_id(
        app,
        MENU_QUIT_APP,
        "Quit App (Proxy Keeps Running)",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &hide, &separator, &quit])?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID);
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }

    let tray_icon = tray
        .tooltip("codex-helper local proxy control center")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            handle_tray_menu_event(app, event.id().as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            if should_show_window_for_tray_event(&event) {
                let _ = show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(DesktopTrayIcon(tray_icon));

    Ok(())
}

pub(crate) fn setup_main_window_lifecycle<R: Runtime>(app: &mut App<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        register_main_window_close_handler(window);
    }
    Ok(())
}

pub(crate) fn handle_window_event<R: Runtime>(window: &Window<R>, event: &tauri::WindowEvent) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        let lifecycle = window.state::<DesktopLifecycleState>();
        if decide_window_close(lifecycle.quit_requested()) == WindowCloseDecision::HideToTray {
            api.prevent_close();
            if let Err(err) = window.hide() {
                eprintln!("failed to hide main window to tray: {err}");
            }
        }
    }
}

pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Err(window_error("main window is not available"));
    };
    window
        .show()
        .map_err(|err| window_error(format!("failed to show main window: {err}")))?;
    window
        .unminimize()
        .map_err(|err| window_error(format!("failed to restore main window: {err}")))?;
    window
        .set_focus()
        .map_err(|err| window_error(format!("failed to focus main window: {err}")))?;
    Ok(())
}

pub(crate) fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Err(window_error("main window is not available"));
    };
    window
        .hide()
        .map_err(|err| window_error(format!("failed to hide main window: {err}")))?;
    Ok(())
}

pub(crate) fn minimize_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Err(window_error("main window is not available"));
    };
    window
        .minimize()
        .map_err(|err| window_error(format!("failed to minimize main window: {err}")))?;
    Ok(())
}

pub(crate) fn toggle_main_window_maximized<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(), CommandError> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Err(window_error("main window is not available"));
    };
    let is_maximized = window
        .is_maximized()
        .map_err(|err| window_error(format!("failed to read window maximized state: {err}")))?;
    if is_maximized {
        window
            .unmaximize()
            .map_err(|err| window_error(format!("failed to unmaximize main window: {err}")))?;
    } else {
        window
            .maximize()
            .map_err(|err| window_error(format!("failed to maximize main window: {err}")))?;
    }
    Ok(())
}

pub(crate) fn quit_app<R: Runtime>(app: &AppHandle<R>) {
    let lifecycle = app.state::<DesktopLifecycleState>();
    lifecycle.request_quit();
    debug_assert_eq!(
        normal_app_exit_runtime_effect(),
        AppExitRuntimeEffect::LeaveRuntimeRunning
    );
    app.exit(0);
}

pub(crate) fn handle_second_instance_launch<R: Runtime>(app: &AppHandle<R>) {
    debug_assert_eq!(
        second_instance_launch_runtime_effect(),
        AppExitRuntimeEffect::LeaveRuntimeRunning
    );
    if let Err(err) = show_main_window(app) {
        eprintln!(
            "failed to focus existing main window after second launch: {}",
            err.message
        );
    }
}

fn register_main_window_close_handler<R: Runtime>(window: WebviewWindow<R>) {
    let app = window.app_handle().clone();
    let window_for_ready = window.clone();
    window.listen(WINDOW_READY_EVENT, move |_| {
        register_main_window_close_handler(window_for_ready.clone());
    });

    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            let lifecycle = app.state::<DesktopLifecycleState>();
            let decision = decide_window_close(lifecycle.quit_requested());
            let _ = app.emit(
                CLOSE_REQUESTED_EVENT,
                close_decision_label(decision).to_string(),
            );
            if decision == WindowCloseDecision::HideToTray {
                api.prevent_close();
                let _ = hide_main_window(&app);
            }
        }
    });
}

fn handle_tray_menu_event<R: Runtime>(app: &AppHandle<R>, menu_id: &str) {
    match menu_id {
        MENU_SHOW_WINDOW => {
            let _ = show_main_window(app);
        }
        MENU_HIDE_TO_TRAY => {
            let _ = hide_main_window(app);
        }
        MENU_QUIT_APP => quit_app(app),
        _ => {}
    }
}

fn should_show_window_for_tray_event(event: &TrayIconEvent) -> bool {
    match event {
        TrayIconEvent::Click {
            button,
            button_state,
            ..
        } => *button == MouseButton::Left && *button_state == MouseButtonState::Up,
        TrayIconEvent::DoubleClick { button, .. } => *button == MouseButton::Left,
        _ => false,
    }
}

fn close_decision_label(decision: WindowCloseDecision) -> &'static str {
    match decision {
        WindowCloseDecision::HideToTray => "hide-to-tray",
        WindowCloseDecision::AllowClose => "allow-close",
    }
}

fn window_error(message: impl Into<String>) -> CommandError {
    DesktopError::Lifecycle(message.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_request_hides_to_tray_until_safe_quit_is_requested() {
        assert_eq!(decide_window_close(false), WindowCloseDecision::HideToTray);
        assert_eq!(decide_window_close(true), WindowCloseDecision::AllowClose);
    }

    #[test]
    fn normal_app_exit_never_stops_proxy_runtime() {
        assert_eq!(
            normal_app_exit_runtime_effect(),
            AppExitRuntimeEffect::LeaveRuntimeRunning
        );
    }

    #[test]
    fn second_instance_launch_never_stops_proxy_runtime() {
        assert_eq!(
            second_instance_launch_runtime_effect(),
            AppExitRuntimeEffect::LeaveRuntimeRunning
        );
    }

    #[test]
    fn lifecycle_state_records_explicit_quit_request() {
        let state = DesktopLifecycleState::default();

        assert!(!state.quit_requested());
        state.request_quit();
        assert!(state.quit_requested());
    }
}
