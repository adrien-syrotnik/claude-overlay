//! Daemon: listens for hook payloads (TCP 57842) and parses them into NotifState.
//! WebSocket listener for VS Code extensions (port 57843) comes in Task 7.

use crate::heuristic::detect_yn_prompt;
use crate::registry::Registry;
use crate::store::{HookEvent, NotifState, NotifStore, SourceType};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
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
}

impl DaemonCtx {
    pub fn new() -> Self {
        Self {
            store: Arc::new(NotifStore::new()),
            registry: Arc::new(Registry::new()),
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
/// we reply with `{"ok":true,"notif_id":"..."}` and close.
pub async fn run_hook_listener<F>(ctx: Arc<DaemonCtx>, mut on_notif: F) -> Result<()>
where
    F: FnMut(String) + Send + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", HOOK_PORT))
        .await
        .context("bind hook port failed")?;
    eprintln!("[daemon] hook listener on 127.0.0.1:{}", HOOK_PORT);
    let _ = &mut on_notif;

    loop {
        let (socket, _) = listener.accept().await?;
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            if reader.read_line(&mut line).await.is_err() { return; }
            let payload: HookPayload = match serde_json::from_str(&line) {
                Ok(p) => p,
                Err(e) => {
                    let _ = reader.get_mut()
                        .write_all(format!("{{\"ok\":false,\"error\":\"{}\"}}\n", e).as_bytes())
                        .await;
                    return;
                }
            };
            let state = payload_to_state(payload);
            let id = ctx.store.add(state);
            let _ = reader.get_mut()
                .write_all(format!("{{\"ok\":true,\"notif_id\":\"{}\"}}\n", id).as_bytes())
                .await;
            // on_notif is called from spawn but we need to guarantee thread-safety;
            // keep this simple: pass notif_id to caller via... we'll wire through in Task 10.
        });
    }
}
