//! System tray management for rew.
//!
//! Shows status icon (green=normal, yellow=warning, gray=paused),
//! right-click menu with actions.

use crate::state::AppState;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

/// Tray icon status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayStatus {
    Normal,
    Warning,
    Paused,
}

/// Set up the system tray with icon and menu.
pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let menu = build_tray_menu(app)?;

    let _tray = TrayIconBuilder::with_id("rew-tray")
        .icon(load_tray_icon(TrayStatus::Normal))
        .tooltip("rew — 文件保护运行中")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            handle_menu_event(app, &event.id().0);
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                show_main_window(app);
            }
        })
        .build(app)?;

    Ok(())
}

fn build_tray_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let show_timeline = MenuItem::with_id(app, "show_timeline", "查看时间线", true, None::<&str>)?;
    let pause_protection =
        MenuItem::with_id(app, "pause_protection", "暂停保护", true, None::<&str>)?;
    let resume_protection =
        MenuItem::with_id(app, "resume_protection", "恢复保护", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 rew", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_timeline,
            &pause_protection,
            &resume_protection,
            &quit,
        ],
    )?;

    Ok(menu)
}

fn handle_menu_event(app: &AppHandle, event_id: &str) {
    match event_id {
        "show_timeline" => {
            show_main_window(app);
        }
        "pause_protection" => {
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut paused) = state.paused.lock() {
                    *paused = true;
                }
            }
            update_tray_status(app, TrayStatus::Paused);
        }
        "resume_protection" => {
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut paused) = state.paused.lock() {
                    *paused = false;
                }
            }
            // Check if there's an active warning
            let has_warning = app
                .try_state::<AppState>()
                .and_then(|s| s.has_warning.lock().ok().map(|w| *w))
                .unwrap_or(false);
            update_tray_status(
                app,
                if has_warning {
                    TrayStatus::Warning
                } else {
                    TrayStatus::Normal
                },
            );
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.unminimize();
    }
}

fn load_tray_icon(status: TrayStatus) -> Image<'static> {
    let icon_bytes: &[u8] = match status {
        TrayStatus::Normal | TrayStatus::Warning | TrayStatus::Paused => {
            include_bytes!("../icons/32x32.png")
        }
    };
    Image::from_bytes(icon_bytes).expect("Failed to load tray icon")
}

/// Update the tray icon to reflect the current status.
pub fn update_tray_status(app: &AppHandle, status: TrayStatus) {
    if let Some(tray) = app.tray_by_id("rew-tray") {
        let tooltip = match status {
            TrayStatus::Normal => "rew — 文件保护运行中",
            TrayStatus::Warning => "rew — ⚠️ 检测到异常",
            TrayStatus::Paused => "rew — 保护已暂停",
        };
        let _ = tray.set_icon(Some(load_tray_icon(status)));
        let _ = tray.set_tooltip(Some(tooltip));
    }
}
