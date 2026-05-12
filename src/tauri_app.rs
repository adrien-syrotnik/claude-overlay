//! Tauri wiring: commands, event emit, window positioning.

use crate::daemon::DaemonCtx;
use crate::focus_win32;
use crate::input_spec::Choice;
use crate::store::{NotifState, SourceType};
use crate::vscode_client::send_command;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopoverData {
    pub notif_id: String,
    pub items: Vec<Choice>,
    pub multi_select: bool,
    pub allow_other: bool,
}

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

/// Hide the popover window and clear its state. Call from any notif lifecycle
/// transition that ends the parent row (dismiss, answer, focus).
fn force_close_popover(app: &AppHandle, ctx: &DaemonCtx) {
    if let Some(pop) = app.get_webview_window("popover") {
        let _ = pop.hide();
    }
    *ctx.popover_data.lock().unwrap() = None;
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
pub fn set_overlay_height_px(height: f64, app: AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = position_top_center_with_height(&win, height.max(40.0));
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
    force_close_popover(&app, &ctx);
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
    force_close_popover(&app, &ctx);
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
    force_close_popover(&app, &ctx);
    if ctx.store.len() == 0 { hide_pill(&app); }
    Ok(())
}

/// Build an ordered list of needle candidates for `find_window_by_title`,
/// starting with the most-specific (the terminal's basename) and climbing up
/// the cwd path. Stops at generic shared roots ("home", "users", "code",
/// "src", "mnt", ...) so we never produce a needle that would match unrelated
/// VS Code/Terminal windows. Caps at 4 candidates.
fn derive_title_candidates(cwd: &str, source_basename: &str) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "home", "users", "code", "src", "documents", "dev", "projects",
        "workspace", "workspaces", "repos", "tmp", "var", "etc", "mnt",
    ];
    let mut out: Vec<String> = Vec::new();
    if !source_basename.is_empty() {
        out.push(source_basename.to_lowercase());
    }
    for ancestor in std::path::Path::new(cwd).ancestors() {
        let Some(name) = ancestor.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let lc = name.to_lowercase();
        if GENERIC.contains(&lc.as_str()) { break; }
        // Skip a "user profile" name like /home/<user> or /Users/<user> —
        // matching by username would route to any window owned by them.
        let parent_lc = ancestor.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        if matches!(parent_lc.as_deref(), Some("home") | Some("users")) { break; }
        if !out.contains(&lc) {
            out.push(lc);
            if out.len() >= 4 { break; }
        }
    }
    out
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
        let class = match n.source_type {
            SourceType::Wt => Some(focus_win32::CLASS_WT),
            SourceType::Vscode => Some(focus_win32::CLASS_VSCODE),
            SourceType::Unknown => None,
        };
        // Try `source_basename` first (matches when terminal cwd == workspace
        // root) then climb the cwd ancestors. VS Code shows the workspace
        // folder name in the title, which can be several levels up from a
        // `cd subdir`-ed terminal — e.g. cwd `/home/x/proj/sub` with workspace
        // `proj` only matches if we also try "proj" as a needle.
        for needle in derive_title_candidates(&n.cwd, &n.source_basename) {
            if let Some(hwnd) = focus_win32::find_window_by_title(class, &needle) {
                if focus_win32::send_keys_safe(hwnd, text).is_ok() {
                    ok = true;
                    break;
                }
            }
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
    force_close_popover(&app, &ctx);
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
    force_close_popover(&app, &ctx);
    if ctx.store.len() == 0 { hide_pill(&app); }
}

#[tauri::command]
pub fn open_popover(
    notif_id: String,
    items: Vec<Choice>,
    multi_select: bool,
    allow_other: bool,
    anchor_x: f64,
    anchor_y: f64,
    anchor_height: f64,
    app: AppHandle,
    ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    use tauri::{LogicalPosition, LogicalSize};
    let main = app.get_webview_window("overlay").ok_or("no main window")?;
    let pop = app.get_webview_window("popover").ok_or("no popover window")?;

    let main_pos = main.outer_position().map_err(|e| e.to_string())?;
    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let main_x = main_pos.x as f64 / scale;
    let main_y = main_pos.y as f64 / scale;

    let screen_x = main_x + anchor_x;
    let screen_y = main_y + anchor_y + anchor_height + 4.0;

    let data = PopoverData {
        notif_id, items, multi_select, allow_other,
    };

    *ctx.popover_data.lock().unwrap() = Some(data.clone());

    pop.set_position(LogicalPosition::new(screen_x, screen_y))
        .map_err(|e| e.to_string())?;
    pop.set_size(LogicalSize::new(288.0, 100.0))
        .map_err(|e| e.to_string())?;
    pop.show().map_err(|e| e.to_string())?;
    pop.set_always_on_top(true).map_err(|e| e.to_string())?;
    let _ = pop.set_focus();

    let _ = app.emit("popover:show", data);
    Ok(())
}

#[tauri::command]
pub fn close_popover(
    app: AppHandle,
    ctx: tauri::State<'_, Arc<DaemonCtx>>,
) {
    if let Some(pop) = app.get_webview_window("popover") {
        let _ = pop.hide();
    }
    *ctx.popover_data.lock().unwrap() = None;
}

#[tauri::command]
pub fn get_popover_data(ctx: tauri::State<'_, Arc<DaemonCtx>>) -> Option<PopoverData> {
    ctx.popover_data.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_popover_height_px(height: f64, app: AppHandle) {
    use tauri::LogicalSize;
    if let Some(pop) = app.get_webview_window("popover") {
        let _ = pop.set_size(LogicalSize::new(288.0, height.max(40.0)));
    }
}
