# Generic input routing & adaptive UI — design spec

**Date:** 2026-05-03
**Topic:** refacto du routing pour gérer dynamiquement single-choice / multi-choice / text-input / yes-no, fix du bug `AskUserQuestion`, et enrichissement des `permission_prompt` avec la commande extraite du transcript.

---

## 1. Goals

1. Fix le bug actuel `AskUserQuestion` (les options ne s'affichent pas → l'utilisateur voit juste "Allow/Focus" alors qu'il devrait voir les choix).
2. Refacto pour qu'ajouter un nouveau type d'input soit une opération localisée (un nouveau variant d'enum + un branchement UI), pas une cascade d'`if/else`.
3. UI adaptative qui utilise des **boutons inline** quand c'est court et un **dropdown popover** quand c'est long.
4. Enrichir les `permission_prompt` avec la commande Bash / le path Edit/Write/Read pour qu'on prenne la décision en voyant le contexte.
5. Largeur fixe **720 px**, typographie dense en fallback quand le contenu pousse fort.

## 2. Non-goals

- Pas de système de plugin / config externe pour router des tools custom (YAGNI, on fera une PR séparée si quelqu'un demande).
- Pas de portage macOS / Linux dans ce chantier.
- Pas de support pour les pickers multi-select natifs du terminal (Claude Code n'en émet pas dans le hook actuel — `AskUserQuestion` passe par PreToolUse qu'on intercepte directement).
- Pas de redimensionnement dynamique de la fenêtre (toujours 720 px).

## 3. Architecture

```
┌─────────────────┐   stdin JSON   ┌──────────────────┐
│ hooks/notify.sh │ ─────────────> │ daemon (Rust)    │
│  (bash)         │                │                  │
│  - normalize    │                │  payload_to_state│
│  - extract      │                │  ↓               │
│    transcript   │                │  InputSpec       │
└─────────────────┘                │  ↓               │
                                   │  NotifStore      │
                                   │  ↓ Tauri event   │
                                   │  notif:new       │
                                   └──────────────────┘
                                            │
                                            ▼
                                   ┌──────────────────┐
                                   │ UI (app.js)      │
                                   │  render(state):  │
                                   │   switch         │
                                   │   state.input    │
                                   │   .kind          │
                                   │  → button | popo │
                                   │    ver | input   │
                                   └──────────────────┘
                                            │
                                            ▼ user click
                                   ┌──────────────────┐
                                   │ daemon.notif_*   │
                                   │  Delivery::      │
                                   │   Keystroke      │
                                   │   ── SendInput   │
                                   │   BlockResponse  │
                                   │   ── stdout JSON │
                                   └──────────────────┘
```

## 4. Data model (`src/input_spec.rs` — nouveau fichier)

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputSpec {
    None,
    YesNo {
        format: YesNoFormat,             // YN, YesNo, Numeric (Claude native picker)
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
        delivery: Delivery,             // toujours BlockResponse en pratique
    },
    TextInput {
        placeholder: Option<String>,
        delivery: Delivery,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Keystroke,        // SendInput dans le terminal
    BlockResponse,    // hook tient sa stdout, on répond {decision:"block",reason:...}
}
```

`NotifState` dans `src/store.rs`:

```rust
pub struct NotifState {
    pub id: String,
    pub event: HookEvent,
    pub source_type: SourceType,
    pub source_basename: String,
    pub cwd: String,
    pub message: String,
    pub input: InputSpec,                    // NOUVEAU — remplace yesno_format + options
    pub target_ext_id: Option<String>,
    pub vscode_ipc_hook: Option<String>,
    pub wt_session: Option<String>,
    pub shell_pid: Option<u32>,
    pub notification_type: Option<String>,
    pub created_at: Instant,
}
```

Champs supprimés : `yesno_format`, `options`.

## 5. Hook normalization (`hooks/claude-overlay-notify.sh`)

Le hook continue de produire un JSON envoyé sur la stdin du daemon. Il prépare une **payload normalisée** avec un sous-objet `input_spec` que le daemon parse directement (pas de re-derivation côté Rust).

### 5.1 Branche `PreToolUse` + `tool_name == "AskUserQuestion"`

**Fix du bug :** lire `tool_input.questions[0]` (pluriel + index 0). Si plusieurs questions (1-4 supportés par l'API), on traite uniquement la première dans cette PR — on documente le TODO pour multi-question (le frontend pourra empiler N rows si besoin, mais c'est out-of-scope).

```bash
QUESTION=$(jq -r '.tool_input.questions[0].question // empty' <<<"$PAYLOAD")
HEADER=$(jq -r   '.tool_input.questions[0].header   // empty' <<<"$PAYLOAD")
MULTI=$(jq -r    '.tool_input.questions[0].multiSelect // false' <<<"$PAYLOAD")
OPTIONS_JSON=$(jq -c '
  [(.tool_input.questions[0].options // [])[] |
    {label: (.label // .), description: (.description // null)}]
' <<<"$PAYLOAD")
```

Construit un `input_spec`:

```jsonc
{
  "kind": "single_choice",       // ou "multi_choice" si MULTI=true
  "options": [{ "label": "...", "description": "..." }, ...],
  "allow_other": true,           // AskUserQuestion ajoute toujours "Other"
  "delivery": "block_response"
}
```

Reste comme avant : envoi via `claude-overlay.exe --stdin-ask`, blocking, parse réponse, émet `{decision:"block",reason:ANSWER}`.

### 5.2 Branche `Notification` avec `notification_type == "permission_prompt"`

Avant d'envoyer la payload, parse `transcript_path` pour récupérer la commande qu'on est en train d'autoriser:

```bash
extract_pending_tool() {
  local transcript="$1"
  [ -f "$transcript" ] || { echo ""; return; }
  # tail+jq : dernier message assistant qui contient un tool_use Bash/Edit/Write/Read
  tail -n 50 "$transcript" \
    | jq -r 'select(.type=="assistant") |
             .message.content[]? |
             select(.type=="tool_use" and (.name=="Bash" or .name=="Edit" or .name=="Write" or .name=="Read")) |
             {name: .name, input: .input}' 2>/dev/null \
    | tail -n 1
}

PENDING=$(extract_pending_tool "$TRANSCRIPT_PATH")
if [ -n "$PENDING" ]; then
  TOOL_NAME=$(jq -r '.name'             <<<"$PENDING")
  case "$TOOL_NAME" in
    Bash)  CONTEXT=$(jq -r '.input.command'             <<<"$PENDING") ;;
    Edit)  CONTEXT=$(jq -r '"Edit: \(.input.file_path)"' <<<"$PENDING") ;;
    Write) CONTEXT=$(jq -r '"Write: \(.input.file_path)"'<<<"$PENDING") ;;
    Read)  CONTEXT=$(jq -r '"Read: \(.input.file_path)"' <<<"$PENDING") ;;
  esac
  MESSAGE="$TOOL_NAME: $CONTEXT"   # remplace "Claude needs your permission to use Bash"
fi
```

Fallback si `extract_pending_tool` retourne vide (race avec le flush JSONL): on garde le `message` brut. Pas d'erreur, juste un peu moins d'info.

L'`input_spec` dans ce cas reste `{kind: "yes_no", format: "numeric", delivery: "keystroke"}` (le picker natif de Claude est piloté par chiffres + Esc).

### 5.3 Branche par défaut (Notification générique, Stop)

`input_spec = {"kind": "none"}` → l'UI affiche juste le message + bouton Focus + ×.

## 6. Daemon (`src/daemon.rs`)

`payload_to_state` :
- Désérialise le champ `input_spec` du JSON entrant directement dans un `InputSpec` (serde `tag = "kind"`).
- Si absent ou parse échoue → `InputSpec::None`.
- Le reste de la fonction inchangé (routing terminal, dedup par cwd, etc.).

Handlers Tauri impactés (`src/tauri_app.rs`) :
- `notif_send_yes` / `notif_send_no` deviennent un seul `notif_yes_no(id, choice: bool)` qui dispatch sur `state.input` :
  - `InputSpec::YesNo { format, delivery: Keystroke }` → `send_keys_safe(hwnd, format.yes_text() / no_text())`
  - `InputSpec::YesNo { delivery: BlockResponse }` → renvoie via le canal `oneshot` au hook bloqué (utile si un jour on a un YesNo via PreToolUse)
- `notif_answer(id, answer: String)` → BlockResponse, envoie `{decision:"block",reason:answer}`
- `notif_answer_multi(id, answers: Vec<String>)` → BlockResponse, joint les labels avec `", "` et renvoie comme `answer`
- `notif_text(id, text: String)` → BlockResponse, renvoie `text` comme `answer`

Tous les handlers `notif_*` sont maintenant **rejected** si l'`InputSpec` ne match pas (genre `notif_text` sur un `YesNo` → retourne erreur). Évite les bugs silencieux.

## 7. UI rendering (`ui/app.js` + `ui/style.css`)

### 7.1 Règle de bascule inline → popover

```js
function shouldUsePopover(options) {
  if (options.length > 3) return true;
  return options.some(o => o.label.length > 18);
}
```

Appliqué aux variants `single_choice` et `multi_choice`.

### 7.2 Layout par variant

| `kind`            | Inline                                   | Popover                                  |
|-------------------|------------------------------------------|------------------------------------------|
| `none`            | `[Focus] [×]`                            | n/a                                      |
| `yes_no`          | `[Allow] [Deny] [×]` ou `[Yes] [No] [×]` | n/a                                      |
| `single_choice`   | `[Opt A] [Opt B] [Opt C] [×]`            | `[Choose ⌄] [×]` ouvre une popover liste |
| `multi_choice`    | `☐ A  ☐ B  [Submit] [×]`                 | `[Select… ⌄] [×]` ouvre liste à cocher   |
| `text_input`      | `<input>` + `[Submit] [×]`               | n/a                                      |

Le bouton `[Other]` (allow_other) apparaît dans inline et popover ; click → bascule la row en mode `text_input` éphémère.

### 7.3 CSS — typographie dense fallback

Quand `state.input.kind !== 'none'` ET `message.length + sum(option.label.length) > 80`, on applique la classe `.notif-row.dense`:
- font-size message: 13 → 12 px
- padding row: 6px → 4px
- gap: 10px → 6px
- max-height message: wrap sur 2 lignes (`-webkit-line-clamp: 2`)

Largeur fenêtre Tauri: **720 px fixe** (modifié dans `tauri.conf.json` + `position_top_center_with_height`).

### 7.4 Popover

DOM: une `<div class="popover">` insérée dans le `body` (pas dans la row, pour éviter clip par overflow).
Position: ancrée au bouton trigger (calc via `getBoundingClientRect` + offset down 4px).
Fermeture: click outside (listener `document.addEventListener('click', closeIfOutside)` ajouté à l'ouverture, retiré à la fermeture).

## 8. Hauteur dynamique

`set_overlay_height(rows)` reste mais devient `set_overlay_height(rows, has_dense, popover_open)`:
- header: 36 px
- row standard: 40 px
- row dense (multi-line): 64 px
- popover ouverte: +200 px (max 6 options visibles, scroll au-delà)
- padding: 16 px

## 9. Keystrokes

`InputSpec::YesNo` reste comme aujourd'hui (déjà fait). Pour les autres variants, **delivery = BlockResponse uniquement** (parce que tous les autres cas viennent de `AskUserQuestion` qui passe par PreToolUse synchrone).

Si un jour un cas exotique apparaît (pas dans ce sprint), on rajoute une variante de `KeystrokeStrategy` à `Delivery::Keystroke`.

## 10. Error handling

| Erreur                                          | Comportement                                                |
|-------------------------------------------------|-------------------------------------------------------------|
| `transcript_path` n'existe pas                  | Fallback message brut, pas d'erreur visible                 |
| `jq` parse échoue dans `extract_pending_tool`   | Fallback message brut, log dans `/tmp/claude-overlay-hook.log` |
| `input_spec` absent du payload                  | `InputSpec::None`                                           |
| Handler Tauri appelé avec mauvais variant       | Retourne `Err(anyhow!("input mismatch"))`, l'UI flash rouge |
| Popover ne se ferme pas (listener leak)         | Test E2E couvre l'ouverture/fermeture                       |
| Multi-question (1-4) dans `AskUserQuestion`     | TODO documenté, on traite seulement `questions[0]`          |

## 11. Testing

- **Unit Rust :**
  - `input_spec.rs` — sérialisation des variants vers JSON attendu par l'UI
  - `daemon::payload_to_state` — chaque variant `kind` → bon `InputSpec`
  - `tauri_app::notif_*` — refus si variant mismatch
- **Bash :** un script `tests/hook_normalization.sh` qui pipe des payloads stub dans le hook et vérifie le JSON sortant
- **UI manuel :** checklist E2E à la fin (cf. §13)
- **Pas de test E2E automatisé** dans cette PR (Tauri test harness pas encore set up — out of scope).

## 12. Files touchés

| Fichier                                | Action      | Pourquoi                                              |
|----------------------------------------|-------------|-------------------------------------------------------|
| `src/input_spec.rs`                    | NEW         | Type `InputSpec`, `Choice`, `Delivery`                |
| `src/main.rs`                          | edit        | `mod input_spec;`                                     |
| `src/store.rs`                         | edit        | `NotifState.input`, virer `yesno_format`/`options`    |
| `src/heuristic.rs`                     | edit        | `YesNoFormat` reste mais bouge dans `input_spec.rs`   |
| `src/daemon.rs`                        | edit        | `payload_to_state` produit `InputSpec`                |
| `src/tauri_app.rs`                     | edit        | Handlers refacto + `set_overlay_height` étendu        |
| `hooks/claude-overlay-notify.sh`       | edit        | Fix `jq` path AskUserQuestion + `extract_pending_tool` + `input_spec` field |
| `ui/app.js`                            | rewrite     | `mkRow()` switch sur `state.input.kind`               |
| `ui/style.css`                         | edit        | Classes `.dense`, `.popover`, `.checkbox-list`        |
| `ui/index.html`                        | edit        | Container popover                                     |
| `tauri.conf.json`                      | edit        | Width 500 → 720                                       |
| `tests/hook_normalization.sh`          | NEW         | Stubs hook, vérifie sortie JSON                       |

## 13. Acceptance checklist (E2E manuel)

- [ ] **Bug AskUserQuestion fix** : `AskUserQuestion(question, options=[A,B,C])` → 3 boutons inline `[A][B][C]` + `[Other][×]`. Click `A` → réponse `A` injectée dans Claude Code, conversation continue.
- [ ] **Single choice + 4 options** → bouton `[Choose ⌄]` qui ouvre popover. Click une option → ferme + envoie. Click outside → ferme sans envoyer.
- [ ] **Single choice + 3 options dont une de 25 chars** → popover (règle hybride).
- [ ] **Multi-choice** : checkboxes inline + `[Submit]` ; multi long → popover avec checkboxes + bouton Submit en bas.
- [ ] **Other → text input** : transition smooth (pas de remount), submit envoie le texte typed.
- [ ] **permission_prompt enrichi** :
  - `Bash` → row affiche `Bash: rm -rf /tmp/foo`
  - `Edit` → `Edit: /path/to/file.rs`
  - Tool inconnu (genre `WebSearch`) → fallback message brut.
- [ ] **Stop / Notification générique** → row simple `[Focus][×]`, pas de classe `.dense`.
- [ ] **Largeur** : fenêtre toujours 720 px, pas de redimensionnement.
- [ ] **Bug regress focus + resize maximisé** : reste fixé (IsIconic guard inchangé).

## 14. Out of scope (TODO follow-ups)

- Multi-question (`AskUserQuestion` avec `questions: [Q1, Q2, ...]` 2-4 questions empilées) → row par question.
- Preview field d'`AskUserQuestion` (markdown side-by-side) → trop gros, autre PR.
- Plugin/config externe pour router des tools custom → seulement si demandé.
- Portage macOS/Linux → roadmap séparée.
