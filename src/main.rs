mod autostart;
mod daemon;
mod focus_win32;
mod heuristic;
mod registry;
mod store;
mod tauri_app;
mod vscode_client;

use anyhow::Result;
use std::sync::Arc;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  claude-overlay.exe --daemon");
    eprintln!("  claude-overlay.exe --stdin");
    eprintln!("  claude-overlay.exe --install-autostart");
    eprintln!("  claude-overlay.exe --uninstall-autostart");
    eprintln!("  claude-overlay.exe --status");
    std::process::exit(2);
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| usage());

    match mode {
        "--daemon" => run_daemon(),
        "--stdin" => run_client_stdin(),
        "--install-autostart" => {
            let exe = std::env::current_exe()?.to_string_lossy().to_string();
            autostart::install(&exe)?; println!("installed: {}", exe); Ok(())
        }
        "--uninstall-autostart" => { autostart::uninstall()?; println!("uninstalled"); Ok(()) }
        "--status" => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                match tokio::net::TcpStream::connect(("127.0.0.1", daemon::HOOK_PORT)).await {
                    Ok(_) => { println!("daemon is running"); }
                    Err(_) => { println!("daemon is NOT running"); }
                }
            });
            Ok(())
        }
        _ => usage(),
    }
}

fn run_daemon() -> Result<()> {
    let _mutex = match daemon::acquire_mutex()? {
        Some(h) => h,
        None => { eprintln!("another daemon instance is already running"); std::process::exit(0); }
    };
    let ctx = Arc::new(daemon::DaemonCtx::new());

    let rt = tokio::runtime::Runtime::new()?;
    let handle = rt.handle().clone();

    tauri::Builder::default()
        .manage(ctx.clone())
        .invoke_handler(tauri::generate_handler![
            tauri_app::notif_list,
            tauri_app::notif_dismiss,
            tauri_app::notif_focus,
            tauri_app::notif_send_yes,
            tauri_app::notif_send_no,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let ctx_hook = ctx.clone();
            let app_hook = app_handle.clone();
            handle.spawn(async move {
                let store = ctx_hook.store.clone();
                let app = app_hook;
                let _ = daemon::run_hook_listener_with_app(ctx_hook, store, app).await;
            });
            let ctx_ws = ctx.clone();
            handle.spawn(async move {
                let _ = daemon::run_ws_listener(ctx_ws).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

fn run_client_stdin() -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = String::new();
        tokio::io::stdin().read_to_string(&mut buf).await?;
        let buf = buf.trim().to_string();
        match tokio::net::TcpStream::connect(("127.0.0.1", daemon::HOOK_PORT)).await {
            Ok(mut s) => {
                s.write_all(buf.as_bytes()).await?;
                s.write_all(b"\n").await?;
                let mut resp = String::new();
                tokio::io::AsyncBufReadExt::read_line(
                    &mut tokio::io::BufReader::new(&mut s), &mut resp,
                ).await?;
                eprintln!("daemon response: {}", resp.trim());
                Ok::<(), anyhow::Error>(())
            }
            Err(_) => {
                // For v1 we don't auto-upgrade to daemon here — rely on auto-start.
                // If that's insufficient, spawn a detached --daemon process.
                eprintln!("daemon not running; spawn --daemon separately");
                Ok(())
            }
        }
    })?;
    Ok(())
}
