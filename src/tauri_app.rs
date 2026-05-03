//! Tauri wiring: commands, event emit, window positioning.

use crate::daemon::DaemonCtx;
use crate::focus_win32;
use crate::store::{NotifState, SourceType};
use crate::vscode_client::send_command;
use serde_json::json;
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
        // Re-assert always-on-top — Tauri v2 on Win11 sometimes drops this after show/set_size.
        let _ = win.set_always_on_top(true);
    }
}

/// Called once at startup. Position the window then hide it — only show on notifs.
pub fn init_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = position_top_center(&win);
        let _ = win.set_always_on_top(true);
        let _ = win.hide();
    }
}

pub fn hide_pill(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

fn position_top_center(win: &tauri::WebviewWindow) -> tauri::Result<()> {
    position_top_center_with_height(win, 80.0)
}

fn position_top_center_with_height(win: &tauri::WebviewWindow, height: f64) -> tauri::Result<()> {
    use tauri::{LogicalPosition, LogicalSize};
    let monitor = win.primary_monitor()?;
    if let Some(m) = monitor {
        let mw = m.size().width as f64 / m.scale_factor();
        let target_w = 720.0;
        let x = ((mw - target_w) / 2.0).max(0.0);
        win.set_position(LogicalPosition::new(x, 0.0))?;
        win.set_size(LogicalSize::new(target_w, height))?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_overlay_height(rows: u32, dense_rows: u32, popover_open: bool, app: AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let header = 36.0;
        let row = 40.0;
        let dense_row = 64.0;
        let padding = 16.0;
        let popover_extra = if popover_open { 200.0 } else { 0.0 };
        let h = header
            + ((rows.saturating_sub(dense_rows)) as f64) * row
            + (dense_rows as f64) * dense_row
            + popover_extra
            + padding;
        let _ = position_top_center_with_height(&win, h.max(60.0));
        let _ = win.set_always_on_top(true);
    }
}

#[tauri::command]
pub fn notif_list(ctx: tauri::State<'_, Arc<DaemonCtx>>) -> Vec<NotifState> {
    ctx.store.list()
}

#[tauri::command]
pub fn notif_dismiss(id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>) {
    // If an AskQuestion is pending, drop the oneshot — the hook side will see
    // empty answer and Claude Code falls back to its native AskUserQuestion UI.
    if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&id) {
        let _ = tx.send(String::new());
    }
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

#[tauri::command]
pub fn notif_answer(
    id: String, answer: String,
    app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) {
    if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&id) {
        let _ = tx.send(answer);
    }
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

#[tauri::command]
pub async fn notif_focus(
    id: String,
    app: AppHandle,
    ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    let Some(n) = ctx.store.get(&id) else { return Ok(()); };
    let mut vscode_ok = false;
    if n.source_type == SourceType::Vscode {
        if let Some(ext_id) = &n.target_ext_id {
            let res = send_command(
                &ctx.registry, &ctx.pending, ext_id,
                json!({"type": "FOCUS", "cwd": n.cwd, "pid": n.shell_pid.unwrap_or(0)}),
                500,
            ).await;
            vscode_ok = res.as_ref().map(|v| v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)).unwrap_or(false);
        }
    }
    // VSCode's t.show() reveals the terminal panel in the host window, but does
    // NOT bring the OS window to the foreground if it's behind another. Always
    // do a Win32 SetForegroundWindow on the source window — additive, harmless
    // when the window is already foreground.
    let needle = n.source_basename.to_lowercase();
    let class = match n.source_type {
        SourceType::Wt => Some(focus_win32::CLASS_WT),
        SourceType::Vscode => Some(focus_win32::CLASS_VSCODE),
        SourceType::Unknown => None,
    };
    if let Some(hwnd) = focus_win32::find_window_by_title(class, &needle) {
        let _ = focus_win32::focus_hwnd(hwnd);
    }
    let _ = vscode_ok;
    if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&id) {
        let _ = tx.send(String::new());
    }
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
    Ok(())
}

async fn do_send_async(answer: &str, id: &str, app: &AppHandle, ctx: &DaemonCtx) {
    use crate::input_spec::{Delivery, InputSpec};
    let Some(n) = ctx.store.get(id) else { return; };
    // Only YesNo with Keystroke delivery needs SendInput. Other variants are
    // answered via BlockResponse on the held-open hook stdin path (notif_answer
    // / notif_answer_multi / notif_text).
    let fmt = match &n.input {
        InputSpec::YesNo { format, delivery: Delivery::Keystroke } => *format,
        _ => return,
    };
    let text = match answer { "yes" => fmt.yes_text(), _ => fmt.no_text() };

    let mut ok = false;
    if n.source_type == SourceType::Vscode {
        if let Some(ext_id) = &n.target_ext_id {
            let res = send_command(
                &ctx.registry, &ctx.pending, ext_id,
                json!({"type": "SEND_TEXT", "cwd": n.cwd, "pid": n.shell_pid.unwrap_or(0), "text": text}),
                500,
            ).await;
            ok = res.as_ref().map(|v| v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false)).unwrap_or(false);
        }
    }
    if !ok {
        let needle = n.source_basename.to_lowercase();
        let class = match n.source_type {
            SourceType::Wt => Some(focus_win32::CLASS_WT),
            SourceType::Vscode => Some(focus_win32::CLASS_VSCODE),
            SourceType::Unknown => None,
        };
        if let Some(hwnd) = focus_win32::find_window_by_title(class, &needle) {
            ok = focus_win32::send_keys_safe(hwnd, text).is_ok();
        }
    }

    if ok {
        ctx.store.remove(id);
        emit_notif_remove(app, id);
        if ctx.store.len() == 0 { hide_pill(app); }
    } else {
        let _ = app.emit("notif:error", serde_json::json!({"id": id, "reason": "focus_lost"}));
    }
}

#[tauri::command]
pub async fn notif_send_yes(
    id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    do_send_async("yes", &id, &app, &ctx).await;
    Ok(())
}

#[tauri::command]
pub async fn notif_send_no(
    id: String, app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    do_send_async("no", &id, &app, &ctx).await;
    Ok(())
}

#[tauri::command]
pub async fn notif_yes_no(
    id: String, choice: bool,
    app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    do_send_async(if choice { "yes" } else { "no" }, &id, &app, &ctx).await;
    Ok(())
}

#[tauri::command]
pub fn notif_answer_multi(
    id: String, answers: Vec<String>,
    app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) {
    let joined = answers.join(", ");
    if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&id) {
        let _ = tx.send(joined);
    }
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

#[tauri::command]
pub fn notif_text(
    id: String, text: String,
    app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) {
    if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&id) {
        let _ = tx.send(text);
    }
    ctx.store.remove(&id);
    emit_notif_remove(&app, &id);
    if ctx.store.len() == 0 { hide_pill(&app); }
}
