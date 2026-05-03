# Generic input routing & adaptive UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refacto le routing des inputs en `InputSpec` enum, fix le bug `AskUserQuestion`, ajoute le rendu adaptatif (inline ↔ popover, checkboxes, text input) et enrichit les `permission_prompt` avec la commande extraite du transcript.

**Architecture:** Nouveau module `src/input_spec.rs` qui centralise les types d'input. `NotifState` perd `yesno_format`/`options` au profit d'un seul champ `input: InputSpec`. Le hook bash construit le sous-objet `input_spec` (parsing JSON path + transcript enrichment). UI dispatche sur `state.input.kind` avec règle de bascule inline/popover (`≤3 options ET max(label) ≤ 18 chars`).

**Tech Stack:** Rust + Tauri v2, serde JSON, bash + jq, vanilla JS/CSS, Win32 SendInput.

**Spec source:** `docs/superpowers/specs/2026-05-03-generic-input-routing-design.md`

---

## Pre-flight

- [ ] **Step 0.1: Create branch (optional)**

```bash
cd /home/adrie/code/claude-overlay
git checkout -b feat/input-spec-refacto
```

Si tu préfères rester sur main (cohérent avec l'historique du projet), skip ça.

- [ ] **Step 0.2: Verify clean working tree**

```bash
git status --short
```
Expected: aucun fichier modifié non committé en dehors de ceux déjà présents (la spec est committée).

---

## Task 1: Create `src/input_spec.rs` with types + serialization tests

**Files:**
- Create: `/home/adrie/code/claude-overlay/src/input_spec.rs`
- Modify: `/home/adrie/code/claude-overlay/src/main.rs` (add `mod input_spec;`)

- [ ] **Step 1.1: Add module declaration to `src/main.rs`**

Open `src/main.rs` and find the existing `mod` declarations. Add after the last one:

```rust
mod input_spec;
```

- [ ] **Step 1.2: Create `src/input_spec.rs` with the type skeleton**

```rust
//! Input specification for overlay rows. Describes what kind of input UI
//! the user needs to provide and how the answer is delivered back.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum YesNoFormat {
    /// Short form: "y" / "n"
    YN,
    /// Long form: "yes" / "no"
    YesNo,
    /// Claude Code's native picker — "1\n" / Esc.
    Numeric,
}

impl YesNoFormat {
    pub fn yes_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "y\n",
            YesNoFormat::YesNo => "yes\n",
            YesNoFormat::Numeric => "1\n",
        }
    }
    pub fn no_text(&self) -> &'static str {
        match self {
            YesNoFormat::YN => "n\n",
            YesNoFormat::YesNo => "no\n",
            YesNoFormat::Numeric => "\x1b",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// SendInput keystrokes to the source terminal window.
    Keystroke,
    /// Hook holds its stdout open; we send `{"answer": "..."}` JSON line back.
    BlockResponse,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputSpec {
    None,
    YesNo {
        format: YesNoFormat,
        delivery: Delivery,
    },
    SingleChoice {
        options: Vec<Choice>,
        allow_other: bool,
        delivery: Delivery,
    },
    MultiChoice {
        options: Vec<Choice>,
        allow_other: bool,
        delivery: Delivery,
    },
    TextInput {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        delivery: Delivery,
    },
}

impl Default for InputSpec {
    fn default() -> Self {
        InputSpec::None
    }
}
```

- [ ] **Step 1.3: Add unit tests at the bottom of `src/input_spec.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yes_no_serializes_with_format_and_delivery() {
        let spec = InputSpec::YesNo {
            format: YesNoFormat::Numeric,
            delivery: Delivery::Keystroke,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "yes_no", "format": "numeric", "delivery": "keystroke"}));
    }

    #[test]
    fn single_choice_serializes_options() {
        let spec = InputSpec::SingleChoice {
            options: vec![
                Choice { label: "A".into(), description: None },
                Choice { label: "B".into(), description: Some("desc".into()) },
            ],
            allow_other: true,
            delivery: Delivery::BlockResponse,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({
            "kind": "single_choice",
            "options": [
                {"label": "A"},
                {"label": "B", "description": "desc"},
            ],
            "allow_other": true,
            "delivery": "block_response",
        }));
    }

    #[test]
    fn none_serializes_as_kind_only() {
        let spec = InputSpec::None;
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "none"}));
    }

    #[test]
    fn text_input_omits_placeholder_when_none() {
        let spec = InputSpec::TextInput {
            placeholder: None,
            delivery: Delivery::BlockResponse,
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v, json!({"kind": "text_input", "delivery": "block_response"}));
    }

    #[test]
    fn yes_no_format_keystrokes() {
        assert_eq!(YesNoFormat::YN.yes_text(), "y\n");
        assert_eq!(YesNoFormat::Numeric.no_text(), "\x1b");
    }
}
```

- [ ] **Step 1.4: Run tests to confirm they pass**

```bash
cd /home/adrie/code/claude-overlay
cargo test --lib input_spec --no-fail-fast
```
Expected: 5 passed.

- [ ] **Step 1.5: Commit**

```bash
git add src/main.rs src/input_spec.rs
git commit -m "feat: add InputSpec module with YesNo/Single/Multi/TextInput variants"
```

---

## Task 2: Move `YesNoFormat` ownership; keep `detect_yn_prompt` in heuristic.rs

`YesNoFormat` was originally defined in `src/heuristic.rs` with a custom `Serialize` impl in `src/store.rs`. Task 1 added a copy to `input_spec.rs` — now make `input_spec` the single source of truth and update `heuristic.rs` to reference it.

**Files:**
- Modify: `/home/adrie/code/claude-overlay/src/heuristic.rs`
- Modify: `/home/adrie/code/claude-overlay/src/store.rs`

- [ ] **Step 2.1: Update `src/heuristic.rs` — delete the local `YesNoFormat` and import from `input_spec`**

Replace the top of `src/heuristic.rs` (lines 1-34) with:

```rust
//! Heuristic detection of y/N-style confirmation prompts in free-form messages.

use crate::input_spec::YesNoFormat;
use regex::Regex;
use once_cell::sync::Lazy;
```

Then delete the `pub enum YesNoFormat` block AND its `impl YesNoFormat` block (formerly lines 6-34). Keep `detect_yn_prompt` intact (lines 36-51).

- [ ] **Step 2.2: Remove the now-orphaned custom `Serialize` impl from `src/store.rs`**

Find this block in `src/store.rs` (lines 50-59) and delete it:

```rust
// Make YesNoFormat serializable since it's used inside NotifState.
impl Serialize for YesNoFormat {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            YesNoFormat::YN => "y_n",
            YesNoFormat::YesNo => "yes_no",
            YesNoFormat::Numeric => "numeric",
        })
    }
}
```

Also remove the now-unused import line `use crate::heuristic::YesNoFormat;` at the top of `store.rs` (will be re-added in Task 3 via `input_spec`).

- [ ] **Step 2.3: Compile to confirm no orphan references**

```bash
cargo check --tests
```
Expected: errors only in `store.rs` referring to `yesno_format` (Task 3 fixes those). NO errors in `heuristic.rs` or `input_spec.rs`.

If `heuristic.rs` errors with "cannot find type `YesNoFormat`", verify Step 2.1 was applied correctly.

- [ ] **Step 2.4: Run heuristic tests**

```bash
cargo test --lib heuristic
```
Expected: 11 passed (the existing detect_yn_prompt tests still work).

- [ ] **Step 2.5: Commit**

```bash
git add src/heuristic.rs src/store.rs
git commit -m "refactor: relocate YesNoFormat to input_spec module"
```

---

## Task 3: Refactor `NotifState` to use `InputSpec`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/src/store.rs`

- [ ] **Step 3.1: Update `NotifState` struct**

Replace the `yesno_format` and `options` fields (currently around lines 33-37) with a single `input` field. The struct becomes:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct NotifState {
    pub id: String,
    pub event: HookEvent,
    pub source_type: SourceType,
    pub source_basename: String,
    pub cwd: String,
    pub message: String,
    /// What input UI the row needs (none / yes-no / single / multi / text).
    /// Replaces the old `yesno_format` + `options` pair.
    pub input: InputSpec,
    pub target_ext_id: Option<String>,
    pub vscode_ipc_hook: Option<String>,
    pub wt_session: Option<String>,
    pub shell_pid: Option<u32>,
    pub notification_type: Option<String>,
    #[serde(skip)]
    pub created_at: Instant,
}
```

Add the import at the top of `store.rs` (after the existing `use` lines):

```rust
use crate::input_spec::InputSpec;
```

- [ ] **Step 3.2: Update the `sample` test helper**

Find the `fn sample(msg: &str) -> NotifState` in the `#[cfg(test)] mod tests` block of `src/store.rs`. Replace the `yesno_format: None,` and `options: None,` lines with:

```rust
            input: InputSpec::None,
```

- [ ] **Step 3.3: Run store tests**

```bash
cargo test --lib store
```
Expected: 5 passed.

- [ ] **Step 3.4: Commit (don't compile the whole project yet — Task 4 does that)**

```bash
git add src/store.rs
git commit -m "refactor(store): replace yesno_format+options with InputSpec"
```

---

## Task 4: Update `HookPayload` + `payload_to_state`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/src/daemon.rs`

- [ ] **Step 4.1: Update `HookPayload` struct (around lines 44-69)**

Replace the existing `pub struct HookPayload` with:

```rust
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
```

Note: this requires `InputSpec` to also be `Deserialize`. Add `Deserialize` to the derive on `InputSpec`, `Choice`, `Delivery`, `YesNoFormat` in `src/input_spec.rs`:

```rust
use serde::{Deserialize, Serialize};
```

Then change every `#[derive(Debug, Clone, Copy, Serialize)]` and `#[derive(Debug, Clone, Serialize)]` in `input_spec.rs` to `#[derive(Debug, Clone, Copy, Deserialize, Serialize)]` / `#[derive(Debug, Clone, Deserialize, Serialize)]` as applicable.

- [ ] **Step 4.2: Add a deserialization unit test in `src/input_spec.rs`**

Inside the existing `mod tests`, add:

```rust
    #[test]
    fn deserialize_single_choice_from_hook_json() {
        let raw = json!({
            "kind": "single_choice",
            "options": [{"label": "A"}, {"label": "B", "description": "second"}],
            "allow_other": false,
            "delivery": "block_response",
        });
        let spec: InputSpec = serde_json::from_value(raw).unwrap();
        match spec {
            InputSpec::SingleChoice { options, allow_other, delivery: _ } => {
                assert_eq!(options.len(), 2);
                assert_eq!(options[1].description.as_deref(), Some("second"));
                assert!(!allow_other);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn deserialize_yes_no_from_hook_json() {
        let raw = json!({"kind": "yes_no", "format": "yn", "delivery": "keystroke"});
        let spec: InputSpec = serde_json::from_value(raw).unwrap();
        assert!(matches!(spec, InputSpec::YesNo { format: YesNoFormat::YN, delivery: Delivery::Keystroke }));
    }
```

- [ ] **Step 4.3: Rewrite `payload_to_state` (around lines 111-144)**

Replace the entire function with:

```rust
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
```

Make sure to remove the `use crate::heuristic::YesNoFormat;` line that's no longer needed (the new code imports `YesNoFormat` from `input_spec`).

- [ ] **Step 4.4: Add a `payload_to_state` unit test (in `src/daemon.rs`)**

Append at the end of `daemon.rs`:

```rust
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
```

- [ ] **Step 4.5: Run the new tests**

```bash
cargo test --lib daemon::tests --no-fail-fast
cargo test --lib input_spec
```
Expected: all pass.

- [ ] **Step 4.6: Commit**

```bash
git add src/daemon.rs src/input_spec.rs
git commit -m "feat(daemon): consume input_spec from hook payload + tests"
```

---

## Task 5: Update Tauri handlers — `notif_yes_no` + `notif_answer_multi` + `notif_text`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/src/tauri_app.rs`
- Modify: `/home/adrie/code/claude-overlay/src/main.rs` (register new commands in the invoke handler)

- [ ] **Step 5.1: Replace `do_send_async` to dispatch on `InputSpec::YesNo`**

Find `async fn do_send_async` in `tauri_app.rs` (around line 144). Replace the body up to the `let text = …` decision so it reads:

```rust
async fn do_send_async(answer: &str, id: &str, app: &AppHandle, ctx: &DaemonCtx) {
    use crate::input_spec::{Delivery, InputSpec};
    let Some(n) = ctx.store.get(id) else { return; };
    // Only YesNo-Keystroke needs SendInput. Other variants are answered via
    // BlockResponse on the held-open hook stdin.
    let (fmt, _delivery) = match &n.input {
        InputSpec::YesNo { format, delivery: Delivery::Keystroke } => (*format, Delivery::Keystroke),
        _ => return,
    };
    let text = match answer { "yes" => fmt.yes_text(), _ => fmt.no_text() };
```

The rest of the function (the SendInput / VS Code SEND_TEXT path, the `if ok { … }`) stays unchanged.

- [ ] **Step 5.2: Add `notif_yes_no` (single command replacing yes/no pair)**

Below the existing `notif_send_no` (around line 195), keep the old commands for compatibility for now, but **also** add this new dispatcher we'll wire from JS in Task 9:

```rust
#[tauri::command]
pub async fn notif_yes_no(
    id: String, choice: bool,
    app: AppHandle, ctx: tauri::State<'_, Arc<DaemonCtx>>,
) -> Result<(), String> {
    do_send_async(if choice { "yes" } else { "no" }, &id, &app, &ctx).await;
    Ok(())
}
```

(Keep `notif_send_yes` / `notif_send_no` in place so we can ship Task 5 without breaking the current UI; Task 9 removes them when the new UI lands.)

- [ ] **Step 5.3: Add `notif_answer_multi` for multi-choice**

```rust
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
    if ctx.store.len() == 0 { hide_pill(&app); }
}
```

- [ ] **Step 5.4: Add `notif_text` for text-input variant**

```rust
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
    if ctx.store.len() == 0 { hide_pill(&app); }
}
```

- [ ] **Step 5.5: Register new commands in `main.rs`**

Find `tauri::generate_handler!` in `src/main.rs` and add the three new entries to the list:

```rust
tauri::generate_handler![
    crate::tauri_app::set_overlay_height,
    crate::tauri_app::notif_list,
    crate::tauri_app::notif_dismiss,
    crate::tauri_app::notif_answer,
    crate::tauri_app::notif_focus,
    crate::tauri_app::notif_send_yes,
    crate::tauri_app::notif_send_no,
    crate::tauri_app::notif_yes_no,        // NEW
    crate::tauri_app::notif_answer_multi,  // NEW
    crate::tauri_app::notif_text,          // NEW
]
```

(Adjust path/syntax to match what's already there — the names `notif_send_yes`/`notif_send_no` should already be present.)

- [ ] **Step 5.6: Compile + run all tests**

```bash
cargo check
cargo test --lib
```
Expected: clean compile + all unit tests pass.

- [ ] **Step 5.7: Commit**

```bash
git add src/tauri_app.rs src/main.rs
git commit -m "feat(tauri): notif_yes_no + notif_answer_multi + notif_text handlers"
```

---

## Task 6: Bump overlay width + extend `set_overlay_height` for popover

**Files:**
- Modify: `/home/adrie/code/claude-overlay/tauri.conf.json`
- Modify: `/home/adrie/code/claude-overlay/src/tauri_app.rs`

- [ ] **Step 6.1: Find current width in `tauri.conf.json`**

```bash
grep -n '"width"' tauri.conf.json
```

- [ ] **Step 6.2: Update width 500 → 720**

In `tauri.conf.json`, change `"width": 500` to `"width": 720` (only the overlay window — leave any other window untouched).

- [ ] **Step 6.3: Verify `position_top_center_with_height` accommodates the new width**

```bash
grep -n "position_top_center_with_height\|width" src/tauri_app.rs | head
```

If the function reads the current window width from the OS, no change needed. If it has a hardcoded 500, change it to 720 too.

- [ ] **Step 6.4: Extend `set_overlay_height` signature**

Replace the existing fn (around line 62):

```rust
#[tauri::command]
pub fn set_overlay_height(rows: u32, dense_rows: u32, popover_open: bool, app: AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let header = 36.0;
        let row = 40.0;
        let dense_row = 64.0;
        let padding = 16.0;
        let popover_extra = if popover_open { 200.0 } else { 0.0 };
        let h = header
            + ((rows.saturating_sub(dense_rows)) as f64) * row
            + (dense_rows as f64) * dense_row
            + popover_extra
            + padding;
        let _ = position_top_center_with_height(&win, h.max(60.0));
        let _ = win.set_always_on_top(true);
    }
}
```

JS callers will pass `dense_rows: 0, popover_open: false` for the simple case until Task 9.

- [ ] **Step 6.5: Update existing JS caller in `ui/app.js` to match the new signature**

Around the existing call `invoke('set_overlay_height', { rows: visibleRowCount });`, change to:

```js
invoke('set_overlay_height', { rows: visibleRowCount, denseRows: 0, popoverOpen: false });
```

(Leave a TODO comment: `// TODO Task 9: pass real denseRows and popoverOpen.`)

- [ ] **Step 6.6: Build and confirm no UI smoke regression**

```bash
cargo check
```
Expected: clean.

(We can't fully test the resize until we run Tauri end-to-end at Task 11.)

- [ ] **Step 6.7: Commit**

```bash
git add tauri.conf.json src/tauri_app.rs ui/app.js
git commit -m "feat: bump overlay width to 720 + dense/popover hooks in set_overlay_height"
```

---

## Task 7: Hook — fix AskUserQuestion `jq` paths + emit `input_spec`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/hooks/claude-overlay-notify.sh`
- Create: `/home/adrie/code/claude-overlay/tests/hook_normalization.sh`

- [ ] **Step 7.1: Open the hook script and locate the `PreToolUse` branch**

In `hooks/claude-overlay-notify.sh` find the block starting with `if [[ "$EVENT" == "PreToolUse" ]]; then` (around line 55). Replace the entire block (down to `exit 0; fi` of that branch) with:

```bash
if [[ "$EVENT" == "PreToolUse" ]]; then
  TOOL=$(jq -r '.tool_name // empty' <<<"$PAYLOAD")
  if [[ "$TOOL" != "AskUserQuestion" ]]; then exit 0; fi

  # New AskUserQuestion API: tool_input.questions is a 1-4 array. We support
  # only questions[0] in this iteration; multi-question support is a TODO.
  QUESTION=$(jq -r '.tool_input.questions[0].question // empty' <<<"$PAYLOAD")
  MULTI=$(jq -r   '.tool_input.questions[0].multiSelect // false' <<<"$PAYLOAD")
  OPTIONS_JSON=$(jq -c '
    [(.tool_input.questions[0].options // [])[] |
      if type == "string" then {label: ., description: null}
      else {label: (.label // empty), description: (.description // null)} end] |
    map(select(.label | length > 0))
  ' <<<"$PAYLOAD")

  if [[ -z "$QUESTION" || -z "$OPTIONS_JSON" || "$OPTIONS_JSON" == "[]" ]]; then
    log "AskUserQuestion: empty question or options, skipping (payload schema?)"
    exit 0
  fi

  KIND="single_choice"
  if [[ "$MULTI" == "true" ]]; then KIND="multi_choice"; fi

  INPUT_SPEC=$(jq -nc \
    --arg kind "$KIND" \
    --argjson options "$OPTIONS_JSON" \
    '{kind: $kind, options: $options, allow_other: true, delivery: "block_response"}')

  ASK_PAYLOAD=$(jq -nc \
    --arg event "AskQuestion" \
    --arg cwd "$CWD" \
    --arg message "$QUESTION" \
    --arg source_type "$SOURCE" \
    --arg source_basename "$BASENAME" \
    --arg wt_session "${WT_SESSION:-}" \
    --arg vscode_ipc_hook "${VSCODE_IPC_HOOK_CLI:-}" \
    --arg vscode_pid "${VSCODE_PID:-}" \
    --argjson shell_pid "${SHELL_PID:-0}" \
    --argjson timestamp_ms "$(date +%s%3N)" \
    --argjson input_spec "$INPUT_SPEC" \
    '{event: $event, cwd: $cwd, message: $message, source_type: $source_type, source_basename: $source_basename, wt_session: $wt_session, vscode_ipc_hook: $vscode_ipc_hook, vscode_pid: $vscode_pid, shell_pid: $shell_pid, timestamp_ms: $timestamp_ms, input_spec: $input_spec, notification_type: ""}')

  RESP=$(echo "$ASK_PAYLOAD" | claude-overlay.exe --stdin-ask 2>/dev/null || true)
  ANSWER=$(jq -r '.answer // empty' <<<"$RESP" 2>/dev/null || echo "")
  if [[ -n "$ANSWER" ]]; then
    jq -nc --arg r "$ANSWER" '{decision:"block",reason:$r}'
  fi
  exit 0
fi
```

- [ ] **Step 7.2: Create `tests/hook_normalization.sh` for stub testing**

```bash
mkdir -p /home/adrie/code/claude-overlay/tests
```

Then create `tests/hook_normalization.sh`:

```bash
#!/usr/bin/env bash
# Verifies that the hook produces correct input_spec JSON for known payload
# shapes. Intercepts the call to claude-overlay.exe by stubbing it.
set -euo pipefail

cd "$(dirname "$0")/.."

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Stub claude-overlay.exe to capture stdin
cat > "$TMPDIR/claude-overlay.exe" <<'EOF'
#!/usr/bin/env bash
cat > "$TMPDIR/captured.json"
echo '{"answer":""}'
EOF
chmod +x "$TMPDIR/claude-overlay.exe"
export PATH="$TMPDIR:$PATH"
export TMPDIR
export TERM_PROGRAM=vscode

fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

# Test 1: AskUserQuestion with 2 string options
PAYLOAD='{
  "session_id": "x", "transcript_path": "/tmp/none", "cwd": "/proj",
  "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
  "tool_input": {"questions": [{"question": "Pick one?", "header": "T", "multiSelect": false, "options": ["A", "B"]}]}
}'
echo "$PAYLOAD" | bash hooks/claude-overlay-notify.sh > /dev/null || true
[ -f "$TMPDIR/captured.json" ] || fail "no payload captured"

KIND=$(jq -r '.input_spec.kind' < "$TMPDIR/captured.json")
[ "$KIND" = "single_choice" ] || fail "expected single_choice, got $KIND"
LABEL_A=$(jq -r '.input_spec.options[0].label' < "$TMPDIR/captured.json")
[ "$LABEL_A" = "A" ] || fail "expected A, got $LABEL_A"
ALLOW_OTHER=$(jq -r '.input_spec.allow_other' < "$TMPDIR/captured.json")
[ "$ALLOW_OTHER" = "true" ] || fail "expected allow_other=true"
pass "single_choice with string options"

rm "$TMPDIR/captured.json"

# Test 2: multiSelect=true with object options
PAYLOAD='{
  "session_id": "x", "transcript_path": "/tmp/none", "cwd": "/proj",
  "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
  "tool_input": {"questions": [{"question": "Multi?", "header": "M", "multiSelect": true, "options": [{"label":"X","description":"x desc"},{"label":"Y"}]}]}
}'
echo "$PAYLOAD" | bash hooks/claude-overlay-notify.sh > /dev/null || true
KIND=$(jq -r '.input_spec.kind' < "$TMPDIR/captured.json")
[ "$KIND" = "multi_choice" ] || fail "expected multi_choice, got $KIND"
DESC=$(jq -r '.input_spec.options[0].description' < "$TMPDIR/captured.json")
[ "$DESC" = "x desc" ] || fail "expected description 'x desc', got '$DESC'"
pass "multi_choice with object options"

# Test 3: non-AskUserQuestion tool short-circuits
PAYLOAD='{"hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": "/x"}'
rm -f "$TMPDIR/captured.json"
echo "$PAYLOAD" | bash hooks/claude-overlay-notify.sh > /dev/null || true
[ ! -f "$TMPDIR/captured.json" ] || fail "Bash should not have called claude-overlay.exe"
pass "non-AskUserQuestion PreToolUse exits silently"

echo "All hook normalization tests passed."
```

Make it executable:

```bash
chmod +x tests/hook_normalization.sh
```

- [ ] **Step 7.3: Run the test**

```bash
bash tests/hook_normalization.sh
```
Expected:
```
PASS: single_choice with string options
PASS: multi_choice with object options
PASS: non-AskUserQuestion PreToolUse exits silently
All hook normalization tests passed.
```

If you get failures, the most likely culprit is `jq` version differences. Check `jq --version` (the script assumes ≥1.6).

- [ ] **Step 7.4: Commit**

```bash
git add hooks/claude-overlay-notify.sh tests/hook_normalization.sh
git commit -m "fix(hook): correct AskUserQuestion jq path + emit input_spec"
```

---

## Task 8: Hook — transcript enrichment for `permission_prompt`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/hooks/claude-overlay-notify.sh`
- Modify: `/home/adrie/code/claude-overlay/tests/hook_normalization.sh`

- [ ] **Step 8.1: Add `extract_pending_tool` helper to the hook**

Just below the existing `get_shell_pid()` function in the hook, add:

```bash
# Parse $TRANSCRIPT_PATH (JSONL) and return the most recent assistant tool_use
# for Bash/Edit/Write/Read as a one-line JSON object {name, input}, or empty
# string. Tolerates a flush race (file exists but trailing line incomplete).
extract_pending_tool() {
  local transcript="$1"
  [ -z "$transcript" ] && { echo ""; return; }
  [ -f "$transcript" ] || { echo ""; return; }
  tail -n 50 "$transcript" 2>/dev/null \
    | jq -c 'select(.type=="assistant") |
             .message.content[]? |
             select(.type=="tool_use" and (.name=="Bash" or .name=="Edit" or .name=="Write" or .name=="Read")) |
             {name: .name, input: .input}' 2>/dev/null \
    | tail -n 1
}
```

- [ ] **Step 8.2: Use it in the `Notification` branch**

Find the section just before `if [[ "$EVENT" == "Stop" && -z "$MESSAGE" ]]; then` and add the enrichment for `permission_prompt`:

```bash
TRANSCRIPT_PATH=$(jq -r '.transcript_path // empty' <<<"$PAYLOAD")
if [[ "$EVENT" == "Notification" && "$NOTIFICATION_TYPE" == "permission_prompt" ]]; then
  PENDING=$(extract_pending_tool "$TRANSCRIPT_PATH")
  if [[ -n "$PENDING" ]]; then
    PEND_TOOL=$(jq -r '.name' <<<"$PENDING")
    case "$PEND_TOOL" in
      Bash)
        CMD=$(jq -r '.input.command // empty' <<<"$PENDING")
        [ -n "$CMD" ] && MESSAGE="Bash: $CMD"
        ;;
      Edit)
        FP=$(jq -r '.input.file_path // empty' <<<"$PENDING")
        [ -n "$FP" ] && MESSAGE="Edit: $FP"
        ;;
      Write)
        FP=$(jq -r '.input.file_path // empty' <<<"$PENDING")
        [ -n "$FP" ] && MESSAGE="Write: $FP"
        ;;
      Read)
        FP=$(jq -r '.input.file_path // empty' <<<"$PENDING")
        [ -n "$FP" ] && MESSAGE="Read: $FP"
        ;;
    esac
    log "permission_prompt enrichment: tool=$PEND_TOOL message='$MESSAGE'"
  else
    log "permission_prompt: no recent tool_use in transcript, keeping brut message"
  fi
fi
```

- [ ] **Step 8.3: Add a test for permission_prompt enrichment**

Append to `tests/hook_normalization.sh` BEFORE the final `echo "All hook normalization tests passed."`:

```bash
# Test 4: permission_prompt enrichment with Bash tool_use in transcript
TRANSCRIPT="$TMPDIR/transcript.jsonl"
cat > "$TRANSCRIPT" <<'EOF'
{"type":"user","message":{"role":"user","content":"go"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"rm -rf /tmp/foo"}}]}}
EOF

PAYLOAD=$(jq -nc \
  --arg cwd "/proj" \
  --arg transcript "$TRANSCRIPT" \
  '{cwd: $cwd, transcript_path: $transcript, hook_event_name: "Notification", message: "Claude needs your permission to use Bash", notification_type: "permission_prompt"}')

rm -f "$TMPDIR/captured.json"
echo "$PAYLOAD" | bash hooks/claude-overlay-notify.sh > /dev/null || true

# In Notification branch, the hook background-forks claude-overlay.exe via `&`.
# Wait briefly for the captured file to appear.
for _ in 1 2 3 4 5; do
  [ -f "$TMPDIR/captured.json" ] && break
  sleep 0.2
done

[ -f "$TMPDIR/captured.json" ] || fail "no payload captured for permission_prompt"
ENRICHED_MSG=$(jq -r '.message' < "$TMPDIR/captured.json")
[ "$ENRICHED_MSG" = "Bash: rm -rf /tmp/foo" ] || fail "expected enriched 'Bash: rm -rf /tmp/foo', got '$ENRICHED_MSG'"
pass "permission_prompt enriched with Bash command from transcript"

# Test 5: permission_prompt with no transcript falls back to brut message
PAYLOAD=$(jq -nc \
  --arg cwd "/proj" \
  '{cwd: $cwd, transcript_path: "/nonexistent", hook_event_name: "Notification", message: "Claude needs your permission to use Edit", notification_type: "permission_prompt"}')

rm -f "$TMPDIR/captured.json"
echo "$PAYLOAD" | bash hooks/claude-overlay-notify.sh > /dev/null || true
for _ in 1 2 3 4 5; do
  [ -f "$TMPDIR/captured.json" ] && break
  sleep 0.2
done
[ -f "$TMPDIR/captured.json" ] || fail "no payload captured for fallback"
FALLBACK_MSG=$(jq -r '.message' < "$TMPDIR/captured.json")
[ "$FALLBACK_MSG" = "Claude needs your permission to use Edit" ] || fail "expected brut message fallback, got '$FALLBACK_MSG'"
pass "missing transcript falls back to brut message"
```

- [ ] **Step 8.4: Run the tests**

```bash
bash tests/hook_normalization.sh
```
Expected: 5 PASS lines.

- [ ] **Step 8.5: Commit**

```bash
git add hooks/claude-overlay-notify.sh tests/hook_normalization.sh
git commit -m "feat(hook): enrich permission_prompt with Bash/Edit/Write/Read context"
```

---

## Task 9: Rewrite `ui/app.js` — dispatch on `state.input.kind`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/ui/app.js`

- [ ] **Step 9.1: Replace the entire `mkRow` function and its callers**

Open `ui/app.js`. Replace the whole `function mkRow(state) { … }` block with:

```js
const POPOVER_OPTIONS_THRESHOLD = 3;
const POPOVER_LABEL_THRESHOLD = 18;

function shouldUsePopover(options) {
  if (options.length > POPOVER_OPTIONS_THRESHOLD) return true;
  return options.some(o => (o.label || '').length > POPOVER_LABEL_THRESHOLD);
}

function mkButton(label, opts = {}) {
  const b = document.createElement('button');
  b.className = 'btn' + (opts.accent ? ' btn-accent' : '') + (opts.icon ? ' btn-icon' : '');
  b.textContent = label;
  if (opts.title) b.title = opts.title;
  if (opts.onClick) b.onclick = opts.onClick;
  return b;
}

function mkFocusBtn(state) {
  return mkButton('Focus', { onClick: () => invoke('notif_focus', { id: state.id }) });
}

function mkDismissBtn(state) {
  return mkButton('×', { icon: true, onClick: () => invoke('notif_dismiss', { id: state.id }) });
}

function renderNone(state, group) {
  group.append(mkFocusBtn(state), mkDismissBtn(state));
}

function renderYesNo(state, group) {
  const isPerm = state.notification_type === 'permission_prompt';
  const yes = mkButton(isPerm ? 'Allow' : 'Yes', { accent: true,
    onClick: () => invoke('notif_yes_no', { id: state.id, choice: true }) });
  const no = mkButton(isPerm ? 'Deny' : 'No',
    { onClick: () => invoke('notif_yes_no', { id: state.id, choice: false }) });
  group.append(yes, no, mkDismissBtn(state));
}

function renderSingleChoice(state, group, row) {
  const { options, allow_other } = state.input;
  if (shouldUsePopover(options)) {
    const trigger = mkButton('Choose ⌄', { accent: true,
      onClick: (e) => openSinglePopover(state, options, allow_other, e.currentTarget) });
    group.append(trigger);
  } else {
    options.forEach(opt => {
      const b = mkButton(opt.label, { accent: true, title: opt.description || opt.label,
        onClick: () => invoke('notif_answer', { id: state.id, answer: opt.label }) });
      group.append(b);
    });
    if (allow_other) {
      group.append(mkButton('Other…', { onClick: () => switchToText(row, state) }));
    }
  }
  group.append(mkDismissBtn(state));
}

function renderMultiChoice(state, group, row) {
  const { options, allow_other } = state.input;
  const selected = new Set();
  if (shouldUsePopover(options)) {
    const trigger = mkButton('Select… ⌄', { accent: true,
      onClick: (e) => openMultiPopover(state, options, allow_other, e.currentTarget, selected) });
    group.append(trigger);
  } else {
    const list = document.createElement('span');
    list.className = 'checkbox-list';
    options.forEach(opt => {
      const lbl = document.createElement('label');
      lbl.className = 'cb';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.onchange = () => cb.checked ? selected.add(opt.label) : selected.delete(opt.label);
      lbl.append(cb, document.createTextNode(opt.label));
      lbl.title = opt.description || opt.label;
      list.append(lbl);
    });
    group.append(list);
    if (allow_other) {
      group.append(mkButton('Other…', { onClick: () => switchToText(row, state) }));
    }
    group.append(mkButton('Submit', { accent: true,
      onClick: () => invoke('notif_answer_multi', { id: state.id, answers: Array.from(selected) }) }));
  }
  group.append(mkDismissBtn(state));
}

function renderTextInput(state, group, row) {
  const input = document.createElement('input');
  input.type = 'text';
  input.className = 'text-input';
  input.placeholder = (state.input && state.input.placeholder) || 'Type your answer…';
  input.onkeydown = (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      submitText(state.id, input.value);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      invoke('notif_dismiss', { id: state.id });
    }
  };
  group.append(input);
  group.append(mkButton('Submit', { accent: true, onClick: () => submitText(state.id, input.value) }));
  group.append(mkDismissBtn(state));
  setTimeout(() => input.focus(), 0);
}

function submitText(id, text) {
  if (text.length === 0) return;
  invoke('notif_text', { id, text });
}

function switchToText(row, state) {
  // Ephemeral text-input mode: remove existing button group, replace with text input.
  const oldGroup = row.querySelector('.btn-group');
  if (oldGroup) oldGroup.remove();
  const newGroup = document.createElement('span');
  newGroup.className = 'btn-group';
  const fakeState = Object.assign({}, state, { input: { kind: 'text_input', placeholder: 'Other…' } });
  renderTextInput(fakeState, newGroup, row);
  row.appendChild(newGroup);
}

let activePopover = null;
function closeActivePopover() {
  if (activePopover) {
    activePopover.remove();
    activePopover = null;
    document.removeEventListener('click', popoverOutsideHandler, true);
    invoke('set_overlay_height', { rows: states.size, denseRows: 0, popoverOpen: false });
  }
}
function popoverOutsideHandler(e) {
  if (activePopover && !activePopover.contains(e.target)) closeActivePopover();
}
function mkPopover(triggerEl) {
  closeActivePopover();
  const pop = document.createElement('div');
  pop.className = 'popover';
  const r = triggerEl.getBoundingClientRect();
  pop.style.left = `${r.left}px`;
  pop.style.top = `${r.bottom + 4}px`;
  document.body.appendChild(pop);
  activePopover = pop;
  setTimeout(() => document.addEventListener('click', popoverOutsideHandler, true), 0);
  invoke('set_overlay_height', { rows: states.size, denseRows: 0, popoverOpen: true });
  return pop;
}

function openSinglePopover(state, options, allowOther, triggerEl) {
  const pop = mkPopover(triggerEl);
  options.forEach(opt => {
    const item = document.createElement('button');
    item.className = 'popover-item';
    item.textContent = opt.label;
    if (opt.description) {
      const d = document.createElement('span');
      d.className = 'popover-desc';
      d.textContent = opt.description;
      item.appendChild(d);
    }
    item.onclick = () => {
      closeActivePopover();
      invoke('notif_answer', { id: state.id, answer: opt.label });
    };
    pop.appendChild(item);
  });
  if (allowOther) {
    const o = document.createElement('button');
    o.className = 'popover-item popover-other';
    o.textContent = 'Other…';
    o.onclick = () => {
      closeActivePopover();
      const row = document.querySelector(`.notif-row[data-id="${state.id}"]`);
      if (row) switchToText(row, state);
    };
    pop.appendChild(o);
  }
}

function openMultiPopover(state, options, allowOther, triggerEl, selected) {
  const pop = mkPopover(triggerEl);
  options.forEach(opt => {
    const item = document.createElement('label');
    item.className = 'popover-item popover-checkbox';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = selected.has(opt.label);
    cb.onchange = () => cb.checked ? selected.add(opt.label) : selected.delete(opt.label);
    item.append(cb, document.createTextNode(' ' + opt.label));
    if (opt.description) {
      const d = document.createElement('span');
      d.className = 'popover-desc';
      d.textContent = opt.description;
      item.appendChild(d);
    }
    pop.appendChild(item);
  });
  const submit = document.createElement('button');
  submit.className = 'popover-item popover-submit';
  submit.textContent = 'Submit';
  submit.onclick = () => {
    closeActivePopover();
    invoke('notif_answer_multi', { id: state.id, answers: Array.from(selected) });
  };
  pop.appendChild(submit);
}

function applyDenseClass(row, state) {
  const msgLen = (state.message || '').length;
  const optsLen = (state.input && state.input.options || [])
    .reduce((s, o) => s + (o.label || '').length, 0);
  if (msgLen + optsLen > 80) row.classList.add('dense');
}

function mkRow(state) {
  const li = document.createElement('li');
  li.className = 'notif-row';
  li.dataset.id = state.id;

  const dot = document.createElement('span');
  dot.className = `status-dot ${state.event === 'Stop' ? 'stop' : 'notification'}`;
  li.appendChild(dot);

  const bn = document.createElement('span');
  bn.className = 'basename'; bn.textContent = state.source_basename;
  li.appendChild(bn);

  const sep = document.createElement('span');
  sep.className = 'separator'; sep.textContent = '›';
  li.appendChild(sep);

  const msg = document.createElement('span');
  msg.className = 'message'; msg.textContent = state.message; msg.title = state.message;
  li.appendChild(msg);

  const group = document.createElement('span');
  group.className = 'btn-group';

  const kind = state.input && state.input.kind || 'none';
  switch (kind) {
    case 'yes_no':        renderYesNo(state, group); break;
    case 'single_choice': renderSingleChoice(state, group, li); break;
    case 'multi_choice':  renderMultiChoice(state, group, li); break;
    case 'text_input':    renderTextInput(state, group, li); break;
    default:              renderNone(state, group);
  }
  li.appendChild(group);
  applyDenseClass(li, state);
  return li;
}
```

- [ ] **Step 9.2: Update the `reconcile` function to count dense rows for `set_overlay_height`**

Find the existing call `invoke('set_overlay_height', { rows: visibleRowCount, denseRows: 0, popoverOpen: false });` (added in Task 6) and replace with:

```js
const denseRows = visible.filter(s => {
  const msgLen = (s.message || '').length;
  const optsLen = (s.input && s.input.options || []).reduce((acc, o) => acc + (o.label || '').length, 0);
  return msgLen + optsLen > 80;
}).length;
invoke('set_overlay_height', { rows: visibleRowCount, denseRows, popoverOpen: !!activePopover });
```

- [ ] **Step 9.3: Smoke-check the JS by serving locally (optional)**

We can't run the UI without Tauri. Visually skim the file for syntax errors:

```bash
node --check ui/app.js
```
Expected: no output (success).

- [ ] **Step 9.4: Commit**

```bash
git add ui/app.js
git commit -m "feat(ui): mkRow dispatches on input.kind with popover/text-input support"
```

---

## Task 10: CSS — `.dense`, `.popover`, `.checkbox-list`, `.text-input`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/ui/style.css`

- [ ] **Step 10.1: Append new style rules at the end of `ui/style.css`**

```css
/* Dense fallback: tighter typography when a row has lots of content */
.notif-row.dense { padding: 4px 4px; gap: 6px; }
.notif-row.dense .message {
  font-size: 12px;
  white-space: normal;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* Inline checkbox list (multi-choice short) */
.checkbox-list { display: inline-flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.checkbox-list .cb { display: inline-flex; gap: 4px; align-items: center; font-size: 12px; cursor: pointer; }
.checkbox-list .cb input { accent-color: #4aa0ff; }

/* Text input field */
.btn-group .text-input {
  background: rgba(255,255,255,0.08);
  color: #f1f1f3;
  border: 1px solid rgba(255,255,255,0.12);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 12px;
  min-width: 200px;
  outline: none;
}
.btn-group .text-input:focus {
  border-color: #4aa0ff;
  background: rgba(255,255,255,0.12);
}

/* Popover anchored under the trigger button */
.popover {
  position: fixed;
  background: #18181c;
  color: #f1f1f3;
  border: 1px solid rgba(255,255,255,0.1);
  border-radius: 12px;
  box-shadow: 0 12px 32px rgba(0,0,0,0.55);
  padding: 4px;
  min-width: 240px;
  max-height: 320px;
  overflow-y: auto;
  z-index: 1000;
  animation: popIn 120ms ease-out;
}
@keyframes popIn { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: translateY(0); } }
.popover-item {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  width: 100%;
  background: transparent;
  color: #f1f1f3;
  border: 0;
  padding: 8px 10px;
  border-radius: 8px;
  font-size: 13px;
  cursor: pointer;
  text-align: left;
}
.popover-item:hover { background: rgba(255,255,255,0.08); }
.popover-checkbox { flex-direction: row; align-items: center; gap: 8px; }
.popover-checkbox input { accent-color: #4aa0ff; }
.popover-desc { font-size: 11px; color: rgba(255,255,255,0.55); }
.popover-other { border-top: 1px solid rgba(255,255,255,0.08); margin-top: 4px; padding-top: 10px; font-style: italic; }
.popover-submit { border-top: 1px solid rgba(255,255,255,0.08); margin-top: 4px; padding-top: 10px; background: #4aa0ff; }
.popover-submit:hover { background: #5aa8ff; }
```

- [ ] **Step 10.2: Commit**

```bash
git add ui/style.css
git commit -m "feat(ui): styles for dense rows, checkboxes, popover, text input"
```

---

## Task 11: Build, deploy, smoke-test the AskUserQuestion fix

**Files:**
- (no source changes — this task validates the build chain end-to-end)

- [ ] **Step 11.1: Build the Tauri app on the Windows side**

The codebase is checked out twice (WSL + Windows). The actual Tauri build runs on Windows. From WSL:

```bash
rsync -a --delete /home/adrie/code/claude-overlay/ /mnt/c/Users/adrie/code/claude-overlay/ \
  --exclude target --exclude node_modules --exclude .git
```

Then run from PowerShell (the user can do this with `! powershell.exe -c "..."` if executing inline, or directly in a Windows terminal):

```powershell
cd C:\Users\adrie\code\claude-overlay
cargo tauri build --no-bundle
```

Expected: `Built application at: C:\Users\adrie\code\claude-overlay\target\release\claude-overlay.exe` (or similar path).

- [ ] **Step 11.2: Deploy the new binary**

```powershell
Copy-Item C:\Users\adrie\code\claude-overlay\target\release\claude-overlay.exe `
  C:\Users\adrie\.local\bin\claude-overlay.exe -Force
```

- [ ] **Step 11.3: Restart the daemon**

```powershell
Stop-Process -Name claude-overlay -Force -ErrorAction SilentlyContinue
Start-Process C:\Users\adrie\.local\bin\claude-overlay.exe -ArgumentList "--daemon"
```

(Or whatever the user's normal startup mechanism is — tray icon, Run dialog, etc.)

- [ ] **Step 11.4: Verify the daemon is up**

```powershell
Get-Process claude-overlay
```
Expected: one process listed.

- [ ] **Step 11.5: Trigger an `AskUserQuestion`**

Tell the user: "From a Claude Code session, ask me a question via `AskUserQuestion` (any 2-3 option question). Watch the overlay."

Expected:
- Overlay row appears with `basename › <question text>`
- 2-3 buttons matching the option labels
- `[Other…]` button visible (because `allow_other: true`)
- `[×]` close button at the right

- [ ] **Step 11.6: Click an option and confirm Claude Code receives it**

Click the first option. Expected:
- Overlay row disappears
- Claude Code conversation continues with that option as the answer (visible in the next assistant turn)

- [ ] **Step 11.7: Trigger a 4-option AskUserQuestion to test the popover**

Same as 11.5 but ask for 4 options. Expected:
- Single `[Choose ⌄]` button appears (popover trigger)
- Click → popover appears below the trigger with all 4 options + `Other…` at the bottom
- Click outside closes the popover without sending
- Click an option closes the popover AND sends the answer

- [ ] **Step 11.8: Trigger a permission_prompt for Bash and verify enrichment**

Get Claude Code to need permission for a Bash command (`rm -f /tmp/test.txt` typically triggers the user's OUTSIDE hook). Expected:
- Overlay row's message text reads `Bash: rm -f /tmp/test.txt` instead of the brut "Claude needs your permission to use Bash"
- `[Allow]` `[Deny]` `[×]` buttons present
- Click `[Allow]` → bash runs

- [ ] **Step 11.9: If Step 11.8 fails (no enrichment)**

Most likely cause: the `transcript_path` in the payload is empty, or `extract_pending_tool` doesn't find a recent `tool_use`. Check:

```bash
tail -20 /tmp/claude-overlay-hook.log
```

The hook now logs `permission_prompt enrichment: tool=… message='…'` or `permission_prompt: no recent tool_use in transcript, keeping brut message`. Diagnose from there.

- [ ] **Step 11.10: Run the offline test suite one more time**

```bash
cargo test --lib --no-fail-fast
bash tests/hook_normalization.sh
```
Expected: everything green.

- [ ] **Step 11.11: Commit any small fixes from smoke testing**

If steps 11.5–11.9 surfaced bugs, fix and commit:

```bash
git add <touched-files>
git commit -m "fix: <specific bug from smoke test>"
```

---

## Task 12: E2E acceptance walkthrough

Walk through the full acceptance checklist from the spec (§13). Each line either passes (mark `[x]`) or generates a follow-up bug.

- [ ] **Step 12.1: AskUserQuestion 2-3 inline buttons** — see 11.5–11.6.
- [ ] **Step 12.2: AskUserQuestion 4-option popover** — see 11.7.
- [ ] **Step 12.3: 3 options where one label > 18 chars triggers popover (hybride règle)**

Trigger an `AskUserQuestion` like: question with options `["Yes", "No", "Run the command and skip safety check please"]`. Expected: popover (because of the long label).

- [ ] **Step 12.4: Multi-choice short → inline checkboxes**

Trigger an `AskUserQuestion` with `multiSelect: true` and 3 short options. Expected: inline checkbox list + `[Submit][Other…][×]` row. Tick 2, click Submit. Claude receives `"A, B"`-style joined string.

- [ ] **Step 12.5: Multi-choice long → popover**

Same with 5 options. Expected: `[Select… ⌄]` trigger; popover with checkboxes + Submit at bottom.

- [ ] **Step 12.6: Other → text input transition**

Click `[Other…]`. Expected: row's button group is replaced by a focused text input + Submit + ×. Type something, press Enter. Claude receives the typed string.

- [ ] **Step 12.7: permission_prompt enrichment for Bash / Edit / Write / Read**

For each tool, get Claude to need permission and verify the row message is `Bash: …` / `Edit: …` / `Write: …` / `Read: …`.

- [ ] **Step 12.8: Stop / generic Notification compactness**

Trigger a normal Stop event. Expected: `[Focus][×]` only, NO `.dense` class added.

- [ ] **Step 12.9: Width is fixed at 720**

```powershell
# Quick visual: position the overlay vs a known 720px reference (or take a screenshot and measure).
```

- [ ] **Step 12.10: Focus + IsIconic regression**

Ensure focusing a maximized VS Code window does NOT un-maximize it (regression check on Bug 3).

- [ ] **Step 12.11: Final commit + tag**

```bash
git log --oneline -20
git tag -a v0.2.0 -m "v0.2.0 — generic input routing, AskUserQuestion fix, transcript enrichment"
```

---

## Task 13: Cleanup — remove deprecated `notif_send_yes` / `notif_send_no`

**Files:**
- Modify: `/home/adrie/code/claude-overlay/src/tauri_app.rs`
- Modify: `/home/adrie/code/claude-overlay/src/main.rs`

- [ ] **Step 13.1: Confirm `app.js` no longer calls `notif_send_yes` / `notif_send_no`**

```bash
grep -n "notif_send_yes\|notif_send_no" ui/app.js
```
Expected: no matches.

- [ ] **Step 13.2: Remove the two commands from `tauri_app.rs`**

Delete the two `#[tauri::command] pub async fn notif_send_yes/no` blocks (lines 181-195).

- [ ] **Step 13.3: Remove from `main.rs` `generate_handler!`**

Remove the `crate::tauri_app::notif_send_yes,` and `crate::tauri_app::notif_send_no,` lines.

- [ ] **Step 13.4: Compile + run all tests**

```bash
cargo check
cargo test --lib
bash tests/hook_normalization.sh
```
Expected: clean.

- [ ] **Step 13.5: Commit**

```bash
git add src/tauri_app.rs src/main.rs
git commit -m "chore: drop notif_send_yes/no in favor of notif_yes_no"
```

---

## Self-Review Checklist (run after writing the plan)

**Spec coverage:**
- §1 Goals 1 (AskUserQuestion fix) → Tasks 7, 11
- §1 Goals 2 (refacto extensibility) → Tasks 1–5
- §1 Goals 3 (adaptive UI) → Tasks 9, 10, 12
- §1 Goals 4 (permission_prompt enrichment) → Task 8
- §1 Goals 5 (720px fixed + dense fallback) → Tasks 6, 10
- §4 Data model → Tasks 1, 3
- §5 Hook normalization → Tasks 7, 8
- §6 Daemon → Task 4, 5
- §7 UI rendering rules → Task 9
- §8 Hauteur dynamique → Task 6
- §9 Keystrokes (BlockResponse-only for non-YesNo) → Task 5 (do_send_async early-return)
- §10 Error handling → Task 8 (fallback brut message), Task 4 (input_spec absent → None)
- §11 Testing → unit tests in Tasks 1, 4 + hook_normalization.sh in 7, 8
- §12 Files touchés → all 12 files cited in Tasks
- §13 Acceptance → Task 12
- §14 Out of scope → respected (no multi-question, no preview, no plugin, no macOS)

**Placeholder scan:** none.

**Type consistency:**
- `InputSpec`, `Choice`, `Delivery`, `YesNoFormat` — names consistent across tasks.
- Tauri commands `notif_yes_no(id, choice)`, `notif_answer_multi(id, answers)`, `notif_text(id, text)` — match between Rust signatures (Task 5) and JS calls (Task 9).
- `set_overlay_height(rows, dense_rows, popover_open)` — signature agrees between Rust (Task 6) and JS callers (Task 9).

All checks clean.
