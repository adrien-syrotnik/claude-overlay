//! Daemon: listens for hook payloads (TCP 47842) and parses them into NotifState.
//! Also exposes a WebSocket listener (port 47843) for VS Code extensions.
//!
//! Ports MUST stay below 49152 (the Windows ephemeral/dynamic port range start):
//! anything in 49152-65535 can be silently reserved by WinNAT/Hyper-V/WSL2 at
//! any boot and become unbindable for us, with no visible owner in netstat.
//! See claude-overlay-stderr.log incident 2026-05-12.

use crate::focus_win32;
use crate::heuristic::detect_yn_prompt;
use crate::registry::{ExtensionConnection, Registry, TerminalInfo};
use crate::store::{HookEvent, NotifState, NotifStore, SourceType};
use crate::vscode_client::{new_pending, route_result, send_command, PendingMap};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tauri::Manager;
use tokio_tungstenite::tungstenite::Message;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub const HOOK_PORT: u16 = 47842;
pub const WS_PORT: u16 = 47843;
const MUTEX_NAME: &str = r"Global\claude-overlay-daemon";

/// Try to acquire the global named mutex. Returns Some(handle) if we are the
/// first instance, None if another daemon is already running.
pub fn acquire_mutex() -> Result<Option<HANDLE>> {
    unsafe {
        let handle = CreateMutexW(None, true, &HSTRING::from(MUTEX_NAME))
            .context("CreateMutexW failed")?;
        let err = windows::Win32::Foundation::GetLastError();
        if err == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            Ok(None)
        } else {
            Ok(Some(handle))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub event: String,
    pub cwd: String,
    pub message: String,
    pub source_type: String,
    pub source_basename: String,
    pub wt_session: String,
    pub vscode_ipc_hook: String,
    pub vscode_pid: String,
    pub timestamp_ms: u64,
    /// "permission_prompt", "idle_prompt", or empty.
    #[serde(default)]
    pub notification_type: String,
    /// Outermost shell PID under VS Code Remote-WSL. 0 = unknown.
    #[serde(default)]
    pub shell_pid: u32,
    /// New: input specification produced by the hook bash. Absent for
    /// `Stop`/generic `Notification`. Deserialized directly into `InputSpec`
    /// via the `kind` tag.
    #[serde(default)]
    pub input_spec: Option<crate::input_spec::InputSpec>,
}

/// Map from notif_id → oneshot sender for the user's chosen answer. Set on
/// payload arrival when `options` is non-empty; consumed by `notif_answer` /
/// `notif_dismiss` once the user clicks.
pub type PendingAnswers = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

pub struct DaemonCtx {
    pub store: Arc<NotifStore>,
    pub registry: Arc<Registry>,
    pub pending: PendingMap,
    pub pending_answers: PendingAnswers,
    pub popover_data: Mutex<Option<crate::tauri_app::PopoverData>>,
}

impl DaemonCtx {
    pub fn new() -> Self {
        Self {
            store: Arc::new(NotifStore::new()),
            registry: Arc::new(Registry::new()),
            pending: new_pending(),
            pending_answers: Arc::new(Mutex::new(HashMap::new())),
            popover_data: Mutex::new(None),
        }
    }
}

fn parse_event(s: &str) -> HookEvent {
    match s {
        "Notification" => HookEvent::Notification,
        "Stop" => HookEvent::Stop,
        "AskQuestion" => HookEvent::AskQuestion,
        _ => HookEvent::Notification,
    }
}

fn parse_source(s: &str) -> SourceType {
    match s {
        "vscode" => SourceType::Vscode,
        "wt" => SourceType::Wt,
        _ => SourceType::Unknown,
    }
}

/// Build NotifState from raw payload. Full matching/skip-foreground happens in daemon loop.
pub fn payload_to_state(p: HookPayload) -> NotifState {
    use crate::input_spec::{Delivery, InputSpec, YesNoFormat};
    let shell_pid = if p.shell_pid == 0 { None } else { Some(p.shell_pid) };
    let notification_type = if p.notification_type.is_empty() { None } else { Some(p.notification_type) };

    // Resolve InputSpec. Priority:
    // 1. Hook explicitly provided one (AskUserQuestion path) — use it.
    // 2. Permission_prompt — Claude Code's numeric picker (yes_no / Numeric).
    // 3. Free-form yes/no detected in the message text.
    // 4. None.
    let input = if let Some(spec) = p.input_spec {
        spec
    } else if notification_type.as_deref() == Some("permission_prompt") {
        InputSpec::YesNo {
            format: YesNoFormat::Numeric,
            delivery: Delivery::Keystroke,
        }
    } else if let Some(format) = detect_yn_prompt(&p.message) {
        InputSpec::YesNo { format, delivery: Delivery::Keystroke }
    } else {
        InputSpec::None
    };

    NotifState {
        id: String::new(),
        event: parse_event(&p.event),
        source_type: parse_source(&p.source_type),
        source_basename: p.source_basename,
        cwd: p.cwd,
        message: p.message.clone(),
        input,
        target_ext_id: None,
        vscode_ipc_hook: if p.vscode_ipc_hook.is_empty() { None } else { Some(p.vscode_ipc_hook) },
        wt_session: if p.wt_session.is_empty() { None } else { Some(p.wt_session) },
        shell_pid,
        notification_type,
        created_at: Instant::now(),
    }
}

/// Run the TCP listener for hook payloads. Each connection sends one JSON line,
/// we reply with `{"ok":true,"notif_id":"..."}` and emit a Tauri event.
pub async fn run_hook_listener_with_app(
    ctx: Arc<DaemonCtx>,
    store: Arc<NotifStore>,
    app: tauri::AppHandle,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", HOOK_PORT)).await
        .context("bind hook port failed")?;
    eprintln!("[daemon] hook listener on 127.0.0.1:{}", HOOK_PORT);

    loop {
        let (socket, _) = listener.accept().await?;
        let ctx = ctx.clone();
        let store = store.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() { return; }
            let payload: HookPayload = match serde_json::from_str(&line) {
                Ok(p) => p,
                Err(e) => {
                    let _ = reader.get_mut().write_all(
                        format!("{{\"ok\":false,\"error\":\"{}\"}}\n", e).as_bytes()
                    ).await;
                    return;
                }
            };
            let mut state = payload_to_state(payload);
            if state.source_type == SourceType::Vscode {
                // 1. Exact match by IPC hook (per-VS-Code-window identifier).
                if let Some(hook) = &state.vscode_ipc_hook {
                    if let Some(id) = ctx.registry.find_by_ipc_hook(hook) {
                        state.target_ext_id = Some(id);
                    }
                }
                // 2. Match by terminal shell PID (per-terminal identifier under
                //    Remote-WSL — picks the right one when multiple terminals
                //    share a cwd).
                if state.target_ext_id.is_none() {
                    if let Some(pid) = state.shell_pid {
                        if let Some(id) = ctx.registry.find_by_terminal_pid(pid) {
                            state.target_ext_id = Some(id);
                        }
                    }
                }
                // 3. Fallback: match by terminal cwd (most recently focused window).
                if state.target_ext_id.is_none() {
                    if let Some(id) = ctx.registry.find_by_terminal_cwd(&state.cwd) {
                        state.target_ext_id = Some(id);
                    }
                }
            }
            // Skip the overlay only for fire-and-forget notifs whose source
            // terminal is already foreground. NEVER skip when the user must
            // make a blocking decision: AskQuestion (with input) or
            // permission_prompt (Claude Code's "Claude needs your permission
            // to use X") — those need to be visible even if the user is
            // looking at the terminal.
            let needs_answer = !matches!(state.input, crate::input_spec::InputSpec::None);
            let is_permission = state.notification_type.as_deref() == Some("permission_prompt");
            let must_show = needs_answer || is_permission;
            if !must_show && is_terminal_foreground(&ctx, &state).await {
                let _ = reader.get_mut().write_all(
                    b"{\"ok\":true,\"displayed\":false,\"reason\":\"foreground_skip\"}\n"
                ).await;
                return;
            }
            let state_clone = state.clone();
            let (id, displaced) = store.add_dedup_by_cwd(state);
            let _ = reader.get_mut().write_all(
                format!("{{\"ok\":true,\"notif_id\":\"{}\"}}\n", id).as_bytes()
            ).await;
            for old_id in &displaced {
                crate::tauri_app::emit_notif_remove(&app, old_id);
                // If a displaced notif had a pending answer waiter, drop it
                // (the hook on the other end gets oneshot Err and exits empty).
                let dropped = ctx.pending_answers.lock().unwrap().remove(old_id);
                drop(dropped);
            }
            let mut with_id = state_clone;
            with_id.id = id.clone();
            crate::tauri_app::emit_notif_new(&app, &with_id);

            if needs_answer {
                // Register oneshot, await user click in overlay, write answer back.
                let (tx, rx) = oneshot::channel::<String>();
                {
                    let mut guard = ctx.pending_answers.lock().unwrap();
                    guard.insert(id.clone(), tx);
                }
                // 10-minute cap so a forgotten notif doesn't pin the hook forever.
                let answer = match tokio::time::timeout(
                    std::time::Duration::from_secs(600), rx,
                ).await {
                    Ok(Ok(a)) => a,
                    _ => String::new(),
                };
                let line = json!({"answer": answer}).to_string();
                let _ = reader.get_mut().write_all(line.as_bytes()).await;
                let _ = reader.get_mut().write_all(b"\n").await;
            }
            let _ = ctx;
        });
    }
}

pub async fn run_ws_listener(ctx: Arc<DaemonCtx>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", WS_PORT))
        .await
        .context("bind ws port failed")?;
    eprintln!("[daemon] ws listener on 127.0.0.1:{}", WS_PORT);

    loop {
        let (socket, _) = listener.accept().await?;
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_ws(ctx, socket).await {
                eprintln!("[ws] handler error: {:?}", e);
            }
        });
    }
}

async fn handle_ws(ctx: Arc<DaemonCtx>, stream: tokio::net::TcpStream) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await
        .context("ws accept failed")?;
    let (mut write, mut read) = ws_stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let mut ext_id: Option<String> = None;

    loop {
        tokio::select! {
            msg = read.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("REGISTER") => {
                        let id = v.get("ext_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
                        let hook = v.get("vscode_ipc_hook").and_then(|s| s.as_str()).unwrap_or("").to_string();
                        let pid = v.get("vscode_pid").and_then(|s| s.as_u64()).map(|x| x as u32);
                        let focused = v.get("window_focused").and_then(|b| b.as_bool()).unwrap_or(false);
                        let folders: Vec<String> = v.get("workspace_folders")
                            .and_then(|a| a.as_array())
                            .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                            .unwrap_or_default();
                        ctx.registry.insert(ExtensionConnection {
                            ext_id: id.clone(),
                            vscode_ipc_hook: hook,
                            workspace_folders: folders,
                            vscode_pid: pid,
                            window_focused: focused,
                            terminals: vec![],
                            last_focus_change: Instant::now(),
                            tx: tx.clone(),
                        });
                        ext_id = Some(id);
                    }
                    Some("TERMINALS_UPDATED") => {
                        if let Some(id) = &ext_id {
                            let terminals: Vec<TerminalInfo> = v.get("terminals")
                                .and_then(|a| a.as_array())
                                .map(|a| a.iter().filter_map(|t| {
                                    Some(TerminalInfo {
                                        name: t.get("name")?.as_str()?.to_string(),
                                        cwd: t.get("cwd").and_then(|c| c.as_str().map(|s| s.to_string())),
                                        pid: t.get("pid").and_then(|p| p.as_u64()).map(|x| x as u32),
                                    })
                                }).collect())
                                .unwrap_or_default();
                            ctx.registry.update_terminals(id, terminals);
                        }
                    }
                    Some("WINDOW_FOCUS_CHANGED") => {
                        if let Some(id) = &ext_id {
                            let focused = v.get("focused").and_then(|b| b.as_bool()).unwrap_or(false);
                            ctx.registry.update_focus(id, focused);
                        }
                    }
                    Some("COMMAND_RESULT") => {
                        if let Some(id) = v.get("cmd_id").and_then(|s| s.as_str()) {
                            route_result(&ctx.pending, id, v.clone()).await;
                        }
                    }
                    _ => {}
                }
            }
            out = rx.recv() => {
                let Some(text) = out else { break };
                if write.send(Message::Text(text)).await.is_err() { break; }
            }
        }
    }

    if let Some(id) = ext_id {
        ctx.registry.remove(&id);
    }
    Ok(())
}

/// Polls every 500ms while at least one notif is active. If the foreground window
/// matches a notif's source (class + basename in title), auto-dismiss it.
/// Interactive notifs (AskQuestion with options, or y/n prompts) are NEVER
/// auto-dismissed — the user must explicitly click an option.
pub async fn run_foreground_watcher(ctx: Arc<DaemonCtx>, app: tauri::AppHandle) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tick.tick().await;
        let notifs = ctx.store.list();
        if notifs.is_empty() { continue; }
        let (fg_class, fg_title) = focus_win32::foreground_info();
        if fg_class.is_empty() { continue; }
        let fg_title_lc = fg_title.to_lowercase();
        eprintln!("[fg_watcher] fg_class={:?} fg_title={:?} notifs={}", fg_class, fg_title, notifs.len());
        let mut popover_target_dismissed: Option<String> = None;
        for n in &notifs {
            let needle = n.source_basename.to_lowercase();
            let class_ok = match n.source_type {
                SourceType::Wt => fg_class.eq_ignore_ascii_case(focus_win32::CLASS_WT),
                SourceType::Vscode => fg_class.eq_ignore_ascii_case(focus_win32::CLASS_VSCODE),
                SourceType::Unknown => false,
            };
            let title_ok = fg_title_lc.contains(&needle);
            eprintln!("[fg_watcher] notif {} src={:?} needle={:?} class_ok={} title_ok={} age_ms={}",
                n.id, n.source_type, needle, class_ok, title_ok, n.created_at.elapsed().as_millis());
            if class_ok && title_ok {
                // Interactive notifs (YesNo / Single / Multi / Text) get
                // auto-dismissed when the source terminal regains focus —
                // semantic: "user came back, will answer in the native UI".
                // For AskQuestion the empty-answer fallback in the hook
                // triggers Claude Code's native AskUserQuestion UI; for YesNo
                // the user just types Allow/Deny in the terminal prompt.
                //
                // Grace period: skip dismissal until the notif is at least
                // 800ms old. Without this, if the source window is ALREADY
                // foreground at notif arrival (the common case in VS Code
                // since the chat panel and editor share the same HWND), the
                // overlay flashes briefly and vanishes before the user even
                // registers it. 800ms ≈ first-paint + reaction time.
                use crate::input_spec::InputSpec;
                let is_interactive = matches!(
                    n.input,
                    InputSpec::YesNo { .. }
                    | InputSpec::SingleChoice { .. }
                    | InputSpec::MultiChoice { .. }
                    | InputSpec::TextInput { .. }
                );
                if is_interactive
                    && n.created_at.elapsed() < std::time::Duration::from_millis(800)
                {
                    continue;
                }
                if let Some(tx) = ctx.pending_answers.lock().unwrap().remove(&n.id) {
                    let _ = tx.send(String::new());
                }
                ctx.store.remove(&n.id);
                crate::tauri_app::emit_notif_remove(&app, &n.id);
                popover_target_dismissed = Some(n.id.clone());
            }
        }
        // If the dropped notif had an open popover, close it too.
        if let Some(dismissed_id) = popover_target_dismissed {
            let close = {
                let mut data = ctx.popover_data.lock().unwrap();
                if let Some(d) = data.as_ref() {
                    if d.notif_id == dismissed_id { *data = None; true } else { false }
                } else { false }
            };
            if close {
                if let Some(pop) = app.get_webview_window("popover") {
                    let _ = pop.hide();
                }
            }
        }
        if ctx.store.len() == 0 {
            crate::tauri_app::hide_pill(&app);
        }
    }
}

async fn is_terminal_foreground(ctx: &DaemonCtx, state: &NotifState) -> bool {
    match state.source_type {
        SourceType::Wt => {
            let needle = state.source_basename.to_lowercase();
            if let Some(hwnd) = focus_win32::find_window_by_title(Some(focus_win32::CLASS_WT), &needle) {
                let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
                return fg == hwnd;
            }
            false
        }
        SourceType::Vscode => {
            let Some(ext_id) = &state.target_ext_id else { return false; };
            let res = send_command(
                &ctx.registry, &ctx.pending, ext_id,
                json!({"type": "IS_ACTIVE_TERMINAL", "cwd": state.cwd, "pid": state.shell_pid.unwrap_or(0)}),
                200,
            ).await;
            res.ok()
                .and_then(|v| v.get("active").and_then(|b| b.as_bool()))
                .unwrap_or(false)
        }
        SourceType::Unknown => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_spec::{Delivery, InputSpec, YesNoFormat};

    fn base_payload() -> HookPayload {
        HookPayload {
            event: "Notification".into(),
            cwd: "/x".into(),
            message: "".into(),
            source_type: "vscode".into(),
            source_basename: "x".into(),
            wt_session: "".into(),
            vscode_ipc_hook: "".into(),
            vscode_pid: "".into(),
            timestamp_ms: 0,
            notification_type: "".into(),
            shell_pid: 0,
            input_spec: None,
        }
    }

    #[test]
    fn permission_prompt_becomes_numeric_yes_no() {
        let mut p = base_payload();
        p.notification_type = "permission_prompt".into();
        p.message = "Claude needs your permission to use Bash".into();
        let s = payload_to_state(p);
        assert!(matches!(
            s.input,
            InputSpec::YesNo { format: YesNoFormat::Numeric, delivery: Delivery::Keystroke }
        ));
    }

    #[test]
    fn explicit_input_spec_overrides_heuristics() {
        let mut p = base_payload();
        p.event = "AskQuestion".into();
        p.input_spec = Some(InputSpec::SingleChoice {
            options: vec![],
            allow_other: false,
            delivery: Delivery::BlockResponse,
        });
        let s = payload_to_state(p);
        assert!(matches!(s.input, InputSpec::SingleChoice { .. }));
    }

    #[test]
    fn yn_marker_in_message_yields_yes_no_keystroke() {
        let mut p = base_payload();
        p.message = "Proceed? [y/N]".into();
        let s = payload_to_state(p);
        assert!(matches!(
            s.input,
            InputSpec::YesNo { format: YesNoFormat::YN, delivery: Delivery::Keystroke }
        ));
    }

    #[test]
    fn plain_notification_yields_none() {
        let p = base_payload();
        let s = payload_to_state(p);
        assert!(matches!(s.input, InputSpec::None));
    }
}
