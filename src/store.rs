//! In-memory store of live notifications displayed in the overlay.

use crate::input_spec::YesNoFormat;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Vscode,
    Wt,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    Notification,
    Stop,
    AskQuestion,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotifState {
    pub id: String,
    pub event: HookEvent,
    pub source_type: SourceType,
    pub source_basename: String,
    pub cwd: String,
    pub message: String,
    pub yesno_format: Option<YesNoFormat>,
    /// AskUserQuestion options. When set, UI renders one button per option and
    /// the daemon waits for the user's choice on a held-open TCP connection.
    pub options: Option<Vec<String>>,
    pub target_ext_id: Option<String>,
    pub vscode_ipc_hook: Option<String>,
    pub wt_session: Option<String>,
    /// Outermost shell PID for the terminal Claude runs in. Used by the VS Code
    /// extension to disambiguate when multiple terminals share a cwd.
    pub shell_pid: Option<u32>,
    /// "permission_prompt" / "idle_prompt" / None. Drives whether the overlay
    /// must show even when the source terminal is foreground.
    pub notification_type: Option<String>,
    #[serde(skip)]
    pub created_at: Instant,
}


pub struct NotifStore {
    inner: Mutex<Vec<NotifState>>,
}

impl NotifStore {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Vec::new()) }
    }

    pub fn add(&self, mut state: NotifState) -> String {
        state.id = Uuid::new_v4().to_string();
        state.created_at = Instant::now();
        let id = state.id.clone();
        self.inner.lock().unwrap().push(state);
        id
    }

    /// Remove any existing entries with the same cwd, then add the new state.
    /// Returns (new_id, removed_ids) so the caller can emit notif:remove for the displaced rows.
    pub fn add_dedup_by_cwd(&self, mut state: NotifState) -> (String, Vec<String>) {
        let mut v = self.inner.lock().unwrap();
        let removed: Vec<String> = v.iter()
            .filter(|n| n.cwd == state.cwd)
            .map(|n| n.id.clone())
            .collect();
        v.retain(|n| n.cwd != state.cwd);
        state.id = Uuid::new_v4().to_string();
        state.created_at = Instant::now();
        let id = state.id.clone();
        v.push(state);
        (id, removed)
    }

    pub fn remove(&self, id: &str) -> Option<NotifState> {
        let mut v = self.inner.lock().unwrap();
        let pos = v.iter().position(|n| n.id == id)?;
        Some(v.remove(pos))
    }

    pub fn get(&self, id: &str) -> Option<NotifState> {
        self.inner.lock().unwrap().iter().find(|n| n.id == id).cloned()
    }

    pub fn list(&self) -> Vec<NotifState> {
        self.inner.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(msg: &str) -> NotifState {
        NotifState {
            id: String::new(),
            event: HookEvent::Notification,
            source_type: SourceType::Wt,
            source_basename: "myproject".into(),
            cwd: "/home/user/code/myproject".into(),
            message: msg.into(),
            yesno_format: None,
            options: None,
            target_ext_id: None,
            vscode_ipc_hook: None,
            wt_session: None,
            shell_pid: None,
            notification_type: None,
            created_at: Instant::now(),
        }
    }

    #[test]
    fn add_assigns_id_and_returns_it() {
        let s = NotifStore::new();
        let id = s.add(sample("hi"));
        assert!(!id.is_empty());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn get_finds_by_id() {
        let s = NotifStore::new();
        let id = s.add(sample("hi"));
        assert_eq!(s.get(&id).unwrap().message, "hi");
    }

    #[test]
    fn remove_returns_and_drops() {
        let s = NotifStore::new();
        let id = s.add(sample("hi"));
        assert!(s.remove(&id).is_some());
        assert_eq!(s.len(), 0);
        assert!(s.remove(&id).is_none());
    }

    #[test]
    fn list_returns_all() {
        let s = NotifStore::new();
        s.add(sample("a"));
        s.add(sample("b"));
        assert_eq!(s.list().len(), 2);
    }

    #[test]
    fn add_generates_unique_ids() {
        let s = NotifStore::new();
        let id1 = s.add(sample("a"));
        let id2 = s.add(sample("b"));
        assert_ne!(id1, id2);
    }
}
