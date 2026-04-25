//! Daemon: listens for hook payloads (TCP 57842) and parses them into NotifState.
//! Also exposes a WebSocket listener (port 57843) for VS Code extensions.

use crate::heuristic::detect_yn_prompt;
use crate::registry::{ExtensionConnection, Registry, TerminalInfo};
use crate::store::{HookEvent, NotifState, NotifStore, SourceType};
use crate::vscode_client::{new_pending, route_result, PendingMap};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
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
}

pub struct DaemonCtx {
    pub store: Arc<NotifStore>,
    pub registry: Arc<Registry>,
    pub pending: PendingMap,
}

impl DaemonCtx {
    pub fn new() -> Self {
        Self {
            store: Arc::new(NotifStore::new()),
            registry: Arc::new(Registry::new()),
            pending: new_pending(),
        }
    }
}

fn parse_event(s: &str) -> HookEvent {
    match s {
        "Notification" => HookEvent::Notification,
        "Stop" => HookEvent::Stop,
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
    NotifState {
        id: String::new(),
        event: parse_event(&p.event),
        source_type: parse_source(&p.source_type),
        source_basename: p.source_basename,
        cwd: p.cwd,
        message: p.message.clone(),
        yesno_format: detect_yn_prompt(&p.message),
        target_ext_id: None,
        vscode_ipc_hook: if p.vscode_ipc_hook.is_empty() { None } else { Some(p.vscode_ipc_hook) },
        wt_session: if p.wt_session.is_empty() { None } else { Some(p.wt_session) },
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
            let state = payload_to_state(payload);
            let state_clone = state.clone();
            let id = store.add(state);
            let _ = reader.get_mut().write_all(
                format!("{{\"ok\":true,\"notif_id\":\"{}\"}}\n", id).as_bytes()
            ).await;
            // Build an updated state with the id assigned for the emit.
            let mut with_id = state_clone;
            with_id.id = id.clone();
            crate::tauri_app::emit_notif_new(&app, &with_id);
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
