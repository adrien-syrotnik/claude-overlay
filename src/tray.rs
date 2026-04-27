//! System tray icon — sits in the Win11 hidden tray area, shows a context menu
//! with Restart / Quit, and tells the user the daemon is running.

use anyhow::Result;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

pub fn install(app: &AppHandle) -> Result<()> {
    let restart_i = MenuItem::with_id(app, "tray_restart", "Restart daemon", true, None::<&str>)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let quit_i = MenuItem::with_id(app, "tray_quit", "Quit", true, None::<&str>)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let sep = PredefinedMenuItem::separator(app)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let menu = Menu::with_items(app, &[&restart_i, &sep, &quit_i])
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let icon = app
        .default_window_icon()
        .ok_or_else(|| anyhow::anyhow!("missing default window icon"))?
        .clone();

    TrayIconBuilder::with_id("claude-overlay-tray")
        .icon(icon)
        .tooltip("claude-overlay (running)")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray_quit" => {
                app.exit(0);
            }
            "tray_restart" => {
                app.restart();
            }
            _ => {}
        })
        .build(app)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    Ok(())
}
