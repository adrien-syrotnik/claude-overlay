mod autostart;
mod daemon;
mod heuristic;
mod registry;
mod store;
mod vscode_client;

use anyhow::Result;
use std::sync::Arc;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  claude-overlay.exe --daemon              Start daemon (bind mutex + ports)");
    eprintln!("  claude-overlay.exe --stdin               Read JSON from stdin, send to daemon (spawn daemon if absent)");
    eprintln!("  claude-overlay.exe --install-autostart   Install HKCU Registry Run key");
    eprintln!("  claude-overlay.exe --uninstall-autostart Remove HKCU Registry Run key");
    eprintln!("  claude-overlay.exe --status              Check if daemon is running");
    std::process::exit(2);
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or_else(|| usage());

    match mode {
        "--daemon" => run_daemon().await,
        "--stdin" => run_client_stdin().await,
        "--install-autostart" => {
            let exe = std::env::current_exe()?.to_string_lossy().to_string();
            autostart::install(&exe)?;
            println!("installed: {}", exe);
            Ok(())
        }
        "--uninstall-autostart" => { autostart::uninstall()?; println!("uninstalled"); Ok(()) }
        "--status" => run_status().await,
        _ => usage(),
    }
}

async fn run_daemon() -> Result<()> {
    let _mutex = match daemon::acquire_mutex()? {
        Some(h) => h,
        None => {
            eprintln!("another daemon instance is already running");
            std::process::exit(0);
        }
    };
    let ctx = Arc::new(daemon::DaemonCtx::new());
    let ctx_ws = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = daemon::run_ws_listener(ctx_ws).await {
            eprintln!("ws listener error: {:?}", e);
        }
    });
    daemon::run_hook_listener(ctx, |_id| {}).await?;
    Ok(())
}

async fn run_client_stdin() -> Result<()> {
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
                &mut tokio::io::BufReader::new(&mut s),
                &mut resp,
            ).await?;
            eprintln!("daemon response: {}", resp.trim());
            Ok(())
        }
        Err(_) => {
            eprintln!("daemon not up, becoming daemon ourselves");
            let _mutex = daemon::acquire_mutex()?.expect("mutex race");
            let ctx = Arc::new(daemon::DaemonCtx::new());
            let payload: daemon::HookPayload = serde_json::from_str(&buf)?;
            let state = daemon::payload_to_state(payload);
            ctx.store.add(state);
            daemon::run_hook_listener(ctx, |_id| {}).await?;
            Ok(())
        }
    }
}

async fn run_status() -> Result<()> {
    match tokio::net::TcpStream::connect(("127.0.0.1", daemon::HOOK_PORT)).await {
        Ok(_) => { println!("daemon is running"); Ok(()) }
        Err(_) => { println!("daemon is NOT running"); Ok(()) }
    }
}
