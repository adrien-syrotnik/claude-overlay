//! Daemon: listens for hook payloads (TCP 57842) and parses them into NotifState.
//! Also exposes a WebSocket listener (port 57843) for VS Code extensions.

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
use tokio_tungstenite::tungstenite::Message;
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub const HOOK_PORT: u16 = 57842;
pub const WS_PORT: u16 = 57843;
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
    /// Claude Code's notification subkind: "permission_prompt", "idle_prompt", …
    /// Empty for non-Notification events. Permission prompts must always show
    /// the overlay even if the source terminal is foreground.
    #[serde(default)]
    pub notification_type: String,
    /// AskUserQuestion options. When non-empty, daemon holds the TCP socket
    /// open and replies with the user's choice once they click in the overlay.
    #[serde(default)]
    pub options: Vec<String>,
    /// Outermost shell PID under VS Code Remote-WSL (`terminal.processId`).
    /// Lets the extension match the exact terminal Claude is running in even
    /// when several terminals share the same cwd. 0 = unknown.
    #[serde(default)]
    pub shell_pid: u32,
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
}

impl DaemonCtx {
    pub fn new() -> Self {
        Self {
            store: Arc::new(NotifStore::new()),
            registry: Arc::new(Registry::new()),
            pending: new_pending(),
            pending_answers: Arc::new(Mutex::new(HashMap::new())),
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
    use crate::heuristic::YesNoFormat;
    let options = if p.options.is_empty() { None } else { Some(p.options) };
    let shell_pid = if p.shell_pid == 0 { None } else { Some(p.shell_pid) };
    let notification_type = if p.notification_type.is_empty() { None } else { Some(p.notification_type) };
    // Claude Code's permission_prompt fires its native "❯ 1. Yes / 2. No"
    // picker. The message has no [y/n] markers, so detect_yn_prompt returns
    // None — but we know from the type that this IS a yes/no, just rendered
    // by a numeric picker. Use Numeric format (sends "1\n" / Esc).
    let yesno_format = detect_yn_prompt(&p.message).or_else(|| {
        if notification_type.as_deref() == Some("permission_prompt") {
            Some(YesNoFormat::Numeric)
        } else {
            None
        }
    });
    NotifState {
        id: String::new(),
        event: parse_event(&p.event),
        source_type: parse_source(&p.source_type),
        source_basename: p.source_basename,
        cwd: p.cwd,
        message: p.message.clone(),
        yesno_format,
        options,
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
            // make a blocking decision: AskQuestion (with options) or
            // permission_prompt (Claude Code's "Claude needs your permission
            // to use X") — those need to be visible even if the user is
            // looking at the terminal.
            let has_options = state.options.is_some();
            let is_permission = state.notification_type.as_deref() == Some("permission_prompt");
            let must_show = has_options || is_permission;
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

            if has_options {
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
        for n in &notifs {
            // Interactive notifs require a click — never auto-dismiss them.
            if n.options.is_some() || n.yesno_format.is_some() { continue; }
            let needle = n.source_basename.to_lowercase();
            let class_ok = match n.source_type {
                SourceType::Wt => fg_class.eq_ignore_ascii_case(focus_win32::CLASS_WT),
                SourceType::Vscode => fg_class.eq_ignore_ascii_case(focus_win32::CLASS_VSCODE),
                SourceType::Unknown => false,
            };
            let title_ok = fg_title_lc.contains(&needle);
            if class_ok && title_ok {
                ctx.store.remove(&n.id);
                crate::tauri_app::emit_notif_remove(&app, &n.id);
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
