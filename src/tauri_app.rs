//! Tauri wiring: commands, event emit, window positioning.

use crate::daemon::DaemonCtx;
use crate::focus_win32;
use crate::store::{NotifState, SourceType};
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

#[tauri::command]
pub fn notif_focus(id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>) {
    let Some(n) = ctx.store.get(&id) else { return; };
    let needle = n.source_basename.to_lowercase();
    let class = match n.source_type {
        SourceType::Wt => Some(focus_win32::CLASS_WT),
        SourceType::Vscode => Some(focus_win32::CLASS_VSCODE),
        SourceType::Unknown => None,
    };
    if let Some(hwnd) = focus_win32::find_window_by_title(class, &needle) {
        let _ = focus_win32::focus_hwnd(hwnd);
    }
    // Dismiss the notif after Focus.
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

fn do_send(answer: &str, id: &str, app: &AppHandle, ctx: &DaemonCtx) {
    let Some(n) = ctx.store.get(id) else { return; };
    let Some(fmt) = n.yesno_format else { return; };
    let text = match answer { "yes" => fmt.yes_text(), _ => fmt.no_text() };
    let needle = n.source_basename.to_lowercase();
    let class = match n.source_type {
        SourceType::Wt => Some(focus_win32::CLASS_WT),
        SourceType::Vscode => Some(focus_win32::CLASS_VSCODE),
        SourceType::Unknown => None,
    };
    let sent_ok = if let Some(hwnd) = focus_win32::find_window_by_title(class, &needle) {
        focus_win32::send_keys_safe(hwnd, text).is_ok()
    } else { false };
    if sent_ok {
        ctx.store.remove(id);
        emit_notif_remove(app, id);
        if ctx.store.len() == 0 { hide_pill(app); }
    } else {
        // Error path: emit a visual error event; user retries manually.
        let _ = app.emit("notif:error", serde_json::json!({"id": id, "reason": "focus_lost"}));
    }
}

#[tauri::command]
pub fn notif_send_yes(id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>) {
    do_send("yes", &id, &app, &ctx);
}

#[tauri::command]
pub fn notif_send_no(id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>) {
    do_send("no", &id, &app, &ctx);
}
