//! Registry of connected VS Code extension instances.
//!
//! Each VS Code window runs its own Extension Host, so there are N live
//! WebSocket connections to the daemon. The registry lets the daemon find
//! the right extension to forward a FOCUS / SEND_TEXT / IS_ACTIVE_TERMINAL
//! command to, given a notif's cwd / vscode_ipc_hook / source_type.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize)]
pub struct TerminalInfo {
    pub name: String,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
}

pub struct ExtensionConnection {
    pub ext_id: String,
    pub vscode_ipc_hook: String,
    pub workspace_folders: Vec<String>,
    pub vscode_pid: Option<u32>,
    pub window_focused: bool,
    pub terminals: Vec<TerminalInfo>,
    pub last_focus_change: Instant,
    /// Channel to push outgoing messages to the extension's WebSocket task.
    pub tx: UnboundedSender<String>,
}

pub struct Registry {
    inner: Mutex<HashMap<String, ExtensionConnection>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    pub fn insert(&self, ext: ExtensionConnection) {
        self.inner.lock().unwrap().insert(ext.ext_id.clone(), ext);
    }

    pub fn remove(&self, ext_id: &str) {
        self.inner.lock().unwrap().remove(ext_id);
    }

    pub fn update_terminals(&self, ext_id: &str, terminals: Vec<TerminalInfo>) {
        if let Some(e) = self.inner.lock().unwrap().get_mut(ext_id) {
            e.terminals = terminals;
        }
    }

    pub fn update_focus(&self, ext_id: &str, focused: bool) {
        if let Some(e) = self.inner.lock().unwrap().get_mut(ext_id) {
            e.window_focused = focused;
            if focused { e.last_focus_change = Instant::now(); }
        }
    }

    /// Find extension by exact IPC hook match (best identifier).
    pub fn find_by_ipc_hook(&self, hook: &str) -> Option<String> {
        self.inner.lock().unwrap()
            .values()
            .find(|e| e.vscode_ipc_hook == hook)
            .map(|e| e.ext_id.clone())
    }

    /// Find extension(s) having a terminal with the given cwd. If multiple,
    /// return the one whose window was most recently focused.
    pub fn find_by_terminal_cwd(&self, cwd: &str) -> Option<String> {
        let guard = self.inner.lock().unwrap();
        let candidates: Vec<_> = guard
            .values()
            .filter(|e| e.terminals.iter().any(|t| t.cwd.as_deref() == Some(cwd)))
            .collect();
        candidates
            .iter()
            .max_by_key(|e| e.last_focus_change)
            .map(|e| e.ext_id.clone())
    }

    pub fn get_tx(&self, ext_id: &str) -> Option<UnboundedSender<String>> {
        self.inner.lock().unwrap().get(ext_id).map(|e| e.tx.clone())
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn fake_ext(id: &str, hook: &str) -> ExtensionConnection {
        let (tx, _rx) = mpsc::unbounded_channel();
        ExtensionConnection {
            ext_id: id.into(),
            vscode_ipc_hook: hook.into(),
            workspace_folders: vec![],
            vscode_pid: None,
            window_focused: false,
            terminals: vec![],
            last_focus_change: Instant::now(),
            tx,
        }
    }

    #[test]
    fn insert_and_len() {
        let r = Registry::new();
        r.insert(fake_ext("a", "/sock/a"));
        r.insert(fake_ext("b", "/sock/b"));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn find_by_ipc_hook_exact() {
        let r = Registry::new();
        r.insert(fake_ext("a", "/sock/a"));
        r.insert(fake_ext("b", "/sock/b"));
        assert_eq!(r.find_by_ipc_hook("/sock/b").as_deref(), Some("b"));
        assert_eq!(r.find_by_ipc_hook("/sock/c"), None);
    }

    #[test]
    fn find_by_cwd_single_match() {
        let r = Registry::new();
        let mut a = fake_ext("a", "/sock/a");
        a.terminals = vec![TerminalInfo { name: "t1".into(), cwd: Some("/sample".into()), pid: None }];
        r.insert(a);
        assert_eq!(r.find_by_terminal_cwd("/sample").as_deref(), Some("a"));
    }

    #[test]
    fn find_by_cwd_prefers_most_recently_focused() {
        let r = Registry::new();
        let mut a = fake_ext("a", "/sock/a");
        a.terminals = vec![TerminalInfo { name: "t1".into(), cwd: Some("/sample".into()), pid: None }];
        a.last_focus_change = Instant::now() - std::time::Duration::from_secs(10);
        r.insert(a);
        let mut b = fake_ext("b", "/sock/b");
        b.terminals = vec![TerminalInfo { name: "t2".into(), cwd: Some("/sample".into()), pid: None }];
        b.last_focus_change = Instant::now();
        r.insert(b);
        assert_eq!(r.find_by_terminal_cwd("/sample").as_deref(), Some("b"));
    }

    #[test]
    fn update_terminals_replaces_list() {
        let r = Registry::new();
        r.insert(fake_ext("a", "/sock/a"));
        r.update_terminals("a", vec![
            TerminalInfo { name: "t1".into(), cwd: Some("/x".into()), pid: None },
        ]);
        assert_eq!(r.find_by_terminal_cwd("/x").as_deref(), Some("a"));
    }

    #[test]
    fn remove_drops_from_registry() {
        let r = Registry::new();
        r.insert(fake_ext("a", "/sock/a"));
        r.remove("a");
        assert_eq!(r.len(), 0);
        assert_eq!(r.find_by_ipc_hook("/sock/a"), None);
    }
}
