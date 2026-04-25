//! Send commands to a specific VS Code extension instance and await its reply.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use crate::registry::Registry;

pub type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

pub fn new_pending() -> PendingMap {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Send a command to an extension and await its COMMAND_RESULT reply.
pub async fn send_command(
    registry: &Registry,
    pending: &PendingMap,
    ext_id: &str,
    command: Value,
    timeout_ms: u64,
) -> Result<Value> {
    let cmd_id = Uuid::new_v4().to_string();
    let mut msg = command;
    msg["cmd_id"] = json!(cmd_id);

    let tx = registry
        .get_tx(ext_id)
        .ok_or_else(|| anyhow!("extension not registered: {}", ext_id))?;

    let (resp_tx, resp_rx) = oneshot::channel();
    pending.lock().await.insert(cmd_id.clone(), resp_tx);

    tx.send(msg.to_string()).map_err(|e| anyhow!("send failed: {:?}", e))?;

    let result = timeout(Duration::from_millis(timeout_ms), resp_rx).await;
    pending.lock().await.remove(&cmd_id);
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(anyhow!("response channel closed")),
        Err(_) => Err(anyhow!("timeout {}ms", timeout_ms)),
    }
}

/// Called by the WS handler when a COMMAND_RESULT arrives. Routes it to
/// the oneshot sender waiting on that cmd_id, if any.
pub async fn route_result(pending: &PendingMap, cmd_id: &str, value: Value) {
    if let Some(tx) = pending.lock().await.remove(cmd_id) {
        let _ = tx.send(value);
    }
}
