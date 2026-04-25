//! Tauri wiring: commands, event emit, window positioning.

use crate::daemon::DaemonCtx;
use crate::store::NotifState;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub fn emit_notif_new(app: &AppHandle, state: &NotifState) {
    let _ = app.emit("notif:new", state.clone());
    show_pill(app);
}

pub fn emit_notif_remove(app: &AppHandle, id: &str) {
    let _ = app.emit("notif:remove", id);
}

fn show_pill(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
        let _ = position_top_center(&win);
    }
}

pub fn hide_pill(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

fn position_top_center(win: &tauri::WebviewWindow) -> tauri::Result<()> {
    use tauri::{LogicalPosition, LogicalSize};
    let monitor = win.primary_monitor()?;
    if let Some(m) = monitor {
        let mw = m.size().width as f64 / m.scale_factor();
        let target_w = 500.0;
        let x = ((mw - target_w) / 2.0).max(0.0);
        win.set_position(LogicalPosition::new(x, 16.0))?;
        win.set_size(LogicalSize::new(target_w, 120.0))?;
    }
    Ok(())
}

#[tauri::command]
pub fn notif_list(ctx: tauri::State<'_, Arc<DaemonCtx>>) -> Vec<NotifState> {
    ctx.store.list()
}

#[tauri::command]
pub fn notif_dismiss(id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>) {
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

// Focus and Yes/No commands come in Task 11.
