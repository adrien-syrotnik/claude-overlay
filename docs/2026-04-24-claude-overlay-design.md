# Claude Code Overlay Notifications — Design Document

**Date** : 2026-04-24
**Status** : Draft awaiting user review
**Working directory** : `~/code/myproject` (par convention, le spec vit ici même si l'outil est orthogonal aux autres projets — il sert dans **toutes** les sessions Claude Code)
**Language** : FR (user) — identifiers techniques en anglais

---

## 1. Objectif

Remplacer le beep PowerShell actuel du hook `Notification` de Claude Code (configuré dans `~/.claude/settings.json`) par un **overlay custom always-on-top** style Dynamic Island iOS, affiché en haut-centre de l'écran Windows 11, interactif (boutons Focus / Yes / No / Dismiss), avec gestion **multi-notifications** et **multi-terminaux**.

L'outil doit gérer : Windows Terminal (plusieurs windows), VS Code embedded terminal **dans plusieurs windows VS Code simultanées**, terminaux Remote-WSL (cas dominant) et terminaux Windows natifs, terminaux cmd.exe standalone (fallback best-effort).

### Pourquoi pas BurntToast / toast Windows natif
Déjà exploré dans une session précédente. Verdict : le click sur un toast Windows relance `powershell.exe` qui ouvre un terminal vide qui se ferme immédiatement — UX cassée. Customiser l'AppId ou enregistrer un protocol handler est fragile. PS 7 (nécessaire pour `-ActivatedAction` de BurntToast) n'est pas dispo. Décision : on ship un binaire custom.

### Ce qui est dans v1
- Overlay Dynamic-Island-style (top-center, merge/grow animé multi-lignes)
- **Daemon auto-start au démarrage Windows** via Registry Run key (HKCU)
- Fallback auto-daemon : si le daemon est mort, le premier hook bash respawn un daemon
- Support **Windows Terminal** (Win32 API : title match + SetForegroundWindow + SendInput)
- Support **VS Code multi-window** via extension UI dédiée qui se connecte au daemon en WebSocket
- Match fin de la bonne window VS Code via `$VSCODE_IPC_HOOK_CLI` (identifiant unique par window côté Remote-WSL et Extension Host)
- Boutons : `Focus terminal`, `Yes` / `No` (conditionnels), `Dismiss`
- Détection heuristique du prompt y/N (variantes `[y/N]`, `(y/n)`, `[yes/no]`, `[Y/n]`, etc.) → boutons Yes/No visibles uniquement si match, envoient le format exact détecté (`y\n` vs `yes\n`)
- **Skip de la notif si le terminal source est déjà au foreground** au moment du fire
- Auto-close : 15s pour events `Notification`, 3s pour events `Stop`
- Beep PowerShell actuel **conservé** en parallèle (via le hook bash) pendant la période de validation

### Ce qui n'est PAS dans v1
- Nova integration (personnel d'abord, extraction si ça tient 1-2 semaines d'usage)
- Click sur la pill entière = focus (seuls les boutons réagissent)
- Son custom au-delà du beep existant
- File attente persistée (crash daemon = notifs vivantes perdues, acceptable)
- Écriture dans le TTY WSL pour WT (SendKeys pour l'instant, TTY en fallback v1.1 si SendKeys déçoit)
- Focus panneau terminal VS Code spécifique quand plusieurs panneaux coexistent dans la même window avec le même cwd (match par cwd → premier trouvé, acceptable)
- Supervisor robuste pour restart-on-crash du daemon (Task Scheduler, v1.1 si besoin)
- Claude Code lancé depuis PowerShell Windows natif (pas dans le workflow actuel)

### Critères de sortie v1
Voir section 12 (critères d'acceptation).

### Durée cible
**1 soirée** côté Rust/Tauri + **1-2h** côté extension VS Code + **30min** install/setup script. Shippable en ~1 soirée dense, sinon étalé sur 2 sessions.

---

## 2. Architecture globale

Trois composants coopérants, tous sur la machine locale Windows 11 :

```
┌──────────────────────────────────────────────────────────────────────────┐
│                             WSL2 (Ubuntu)                                 │
│                                                                           │
│  Claude Code (bash dans terminal VS Code Remote-WSL ou WT)                │
│     │                                                                     │
│     │  fire hook Notification / Stop avec payload JSON sur stdin          │
│     ▼                                                                     │
│  ~/.claude/hooks/claude-overlay-notify.sh                                 │
│     │  - lit stdin JSON                                                   │
│     │  - sniff $TERM_PROGRAM, $WT_SESSION,                                │
│     │           $VSCODE_IPC_HOOK_CLI, $VSCODE_PID                         │
│     │  - enrichit payload                                                 │
│     │  - invoke claude-overlay.exe (via interop PATH Windows)             │
│     │  - fire beep PowerShell en parallèle                                │
└─────┼─────────────────────────────────────────────────────────────────────┘
      │
      ▼  (process boundary WSL → Windows)
┌──────────────────────────────────────────────────────────────────────────┐
│                             Windows 11                                    │
│                                                                           │
│  claude-overlay.exe                                                       │
│     │                                                                     │
│     ├─ Mode "client" (invoqué par hook): TCP connect 127.0.0.1:57842,     │
│     │     envoie JSON, exit <20ms                                         │
│     │                                                                     │
│     └─ Mode "daemon" (démarré au login OU fallback si port refus):        │
│           bind 127.0.0.1:57842 (hook clients, JSON line-framed)           │
│           bind 127.0.0.1:57843 (extensions VS Code, WebSocket)            │
│           tray icon (Quit only)                                           │
│           webview Tauri cachée par défaut                                 │
│           registre extensions: Map<ext_id, ExtensionConnection>           │
│           store notifs: Vec<NotifState>                                   │
│                                                                           │
│  VS Code window #1 (ex: myproject en Remote-WSL)                            │
│     │                                                                     │
│     └─ extension "claude-overlay-focus" (UI extension, runs on Windows)   │
│           - au activate(): WebSocket connect 127.0.0.1:57843              │
│           - envoie REGISTER {ext_id, vscode_ipc_hook, workspace_folders}  │
│           - push TERMINALS_UPDATED, WINDOW_FOCUS_CHANGED events           │
│           - listen commands du daemon: FOCUS, SEND_TEXT, IS_ACTIVE        │
│                                                                           │
│  VS Code window #2 (ex: nova en Remote-WSL) — idem, autre WebSocket       │
│  VS Code window #3 (ex: projet Windows natif) — idem                      │
└──────────────────────────────────────────────────────────────────────────┘
```

### Pourquoi Tauri et pas egui/iced
Le rendu visuel Dynamic Island (coins arrondis anti-aliasés, `backdrop-filter: blur`, transitions de hauteur animées, fade-in slide-down) est **trivial en CSS** (~30 lignes) et **laborieux** en GUI libs Rust natives (shaders custom, tween manuel). Coût : binaire ~15-25MB, ~20MB RAM idle. Non-issue pour un outil perso utilisé plusieurs fois par jour.

### Pourquoi daemon auto-start Windows (et pas juste auto-daemon hook-spawned)
Dans un modèle où les extensions VS Code **se connectent au daemon** (inversion nécessaire pour multi-window), avoir le daemon déjà up au boot Windows garantit :
- Connexion WebSocket extension → daemon immédiate à l'activate de VS Code (pas de retry loop)
- Registre des extensions complet avant qu'une notif arrive
- Pas de cold-start 300-500ms sur la 1ère notif de la journée

L'**auto-daemon fallback** (hook bash respawn si daemon absent) reste pour robustesse : si le daemon crashe et n'est pas redémarré (pas de supervisor en v1), le prochain hook Claude Code le relance spontanément. Named mutex empêche les doublons.

### Pourquoi extensions → daemon (WebSocket) et pas daemon → extensions (HTTP)
Chaque VS Code window a son propre Extension Host → N instances de l'extension. Si chaque instance tentait de bind `127.0.0.1:57843` en HTTP server, conflit de port : seule la première gagne. En **inversant** la connexion (extension client WebSocket → daemon server), chaque instance peut se connecter indépendamment au daemon, qui maintient un registre global. Le daemon a une vue complète de toutes les windows VS Code actives.

### Pourquoi IPC TCP pour les hooks (port 57842)
Les deux bouts (client spawn par hook + daemon) tournent **côté Windows**. `127.0.0.1:57842` suffit, pas de traversée WSL. Named pipe Windows = plus de Rust ceremony sans gain. Port 57842 : choisi dans une plage non-enregistrée IANA, collision quasi nulle.

---

## 3. Composants détaillés

### 3.1 `claude-overlay.exe` — binaire Rust + Tauri

**Stack**
- Rust 2021, `cargo-tauri` v2.x
- Dépendances backend : `tokio` (async runtime), `tokio-tungstenite` (WebSocket server), `windows` crate (Win32 APIs : EnumWindows / SetForegroundWindow / GetForegroundWindow / SendInput / Registry manipulation), `serde` / `serde_json`, `tray-icon`, `regex`, `uuid`
- Frontend : HTML/CSS/vanilla JS (pas de framework, ~200 lignes total). CSS custom properties pour thème.

**Modes d'exécution (déterminés via flags CLI et état système)**

| Invocation | Comportement |
|---|---|
| `claude-overlay.exe --daemon` | Mode daemon forcé. Acquire named mutex `Global\claude-overlay-daemon`. Si mutex déjà tenu → exit immédiat (un autre daemon tourne). Sinon : bind 57842 + 57843, spawn webview + tray, enter event loop. Utilisé par le Registry Run key. |
| `claude-overlay.exe --stdin` (ou avec `<<payload>>` en arg) | Mode client : tente `TcpStream::connect("127.0.0.1:57842")`. Si OK → envoie JSON, reçoit ACK, exit. Si refus → devient daemon (bascule sur `--daemon` logic). Utilisé par le hook bash. |
| `claude-overlay.exe --install-autostart` | Écrit la Registry Run key HKCU : `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` valeur `ClaudeOverlay = "C:\...\claude-overlay.exe --daemon"`. Exit. Pas de droits admin requis. |
| `claude-overlay.exe --uninstall-autostart` | Supprime la clé. Exit. |
| `claude-overlay.exe --status` | Ping 127.0.0.1:57842. Affiche "daemon running (pid X, uptime Y)" ou "daemon not running". Utile debug. |

**Comportement daemon**
- Sur connexion TCP entrante (port 57842, hook client) : parse JSON, valide, applique la logique skip-if-foreground (§7), si OK crée une `NotifState` dans le store, emit `tauri.emit("notif:new")` au frontend, arme un timer auto-close.
- Sur connexion WebSocket entrante (port 57843, extension VS Code) : attend REGISTER message, stocke dans `extensions: HashMap<ext_id, ExtensionConnection>`. Keep-alive avec ping/pong WS.
- Sur déconnexion WebSocket : purge l'entrée du registre.
- Sur message WS entrant : update (TERMINALS_UPDATED, WINDOW_FOCUS_CHANGED) ou réponse à une commande précédente.
- Sur événement Tauri IPC depuis frontend (clic bouton) : dispatch action (§5).
- Sur quit tray : graceful shutdown — close listeners, disconnect extensions poliment, exit.

**Pas d'auto-shutdown idle en v1** : le daemon est censé rester up jusqu'au reboot Windows ou quit explicite. Ça simplifie le modèle mental ("si installé, toujours là") et aligne avec le Run key.

**Protocole JSON hook → daemon (TCP 57842, line-framed JSON)**
```json
{
  "event": "Notification" | "Stop",
  "cwd": "~/code/myproject",
  "message": "Claude is waiting for your input [y/N]",
  "source_type": "vscode" | "wt" | "unknown",
  "source_basename": "myproject",
  "wt_session": "uuid-or-empty",
  "vscode_ipc_hook": "/run/user/1000/vscode-ipc-xyz.sock-or-empty",
  "vscode_pid": "12345-or-empty",
  "timestamp_ms": 1713945678123
}
```
Réponse daemon : `{"ok": true, "notif_id": "abc123", "displayed": true}` ou `{"ok": true, "notif_id": "abc123", "displayed": false, "reason": "foreground_skip"}`.

**Protocole WebSocket daemon ↔ extension (port 57843, JSON messages)**

Extension → Daemon :
```json
// Au connect
{"type": "REGISTER", "ext_id": "uuid-v4", "vscode_ipc_hook": "/run/.../vscode-ipc-xyz.sock", "workspace_folders": ["~/code/myproject"], "vscode_pid": 12345, "window_focused": true}

// Sur changement
{"type": "TERMINALS_UPDATED", "terminals": [{"name": "bash", "cwd": "~/code/myproject", "pid": 45678}, ...]}
{"type": "WINDOW_FOCUS_CHANGED", "focused": true}

// Réponse à une commande
{"type": "COMMAND_RESULT", "cmd_id": "...", "ok": true, "data": {...}}
```

Daemon → Extension :
```json
{"type": "FOCUS", "cmd_id": "c1", "cwd": "~/code/myproject"}
{"type": "SEND_TEXT", "cmd_id": "c2", "cwd": "~/code/myproject", "text": "y\n"}
{"type": "IS_ACTIVE_TERMINAL", "cmd_id": "c3", "cwd": "~/code/myproject"}
{"type": "PING", "cmd_id": "c4"}
```

### 3.2 Hook bash — `~/.claude/hooks/claude-overlay-notify.sh`

Shell script pur (bash). Rôle : lire le payload Claude Code sur stdin, enrichir, invoquer le binaire Windows.

```bash
#!/usr/bin/env bash
set -euo pipefail

PAYLOAD=$(cat)  # stdin JSON de Claude Code

# Extraction champs source
CWD=$(jq -r '.cwd // empty' <<<"$PAYLOAD")
BASENAME=$(basename "$CWD")
MESSAGE=$(jq -r '.message // empty' <<<"$PAYLOAD")
EVENT=$(jq -r '.hook_event_name // empty' <<<"$PAYLOAD")

# Détection type de terminal source
if [[ "${TERM_PROGRAM:-}" == "vscode" ]]; then
  SOURCE="vscode"
elif [[ -n "${WT_SESSION:-}" ]]; then
  SOURCE="wt"
else
  SOURCE="unknown"
fi

# Enrichissement avec IDs fins (pour matching window VS Code unique)
ENRICHED=$(jq -n \
  --arg event "$EVENT" \
  --arg cwd "$CWD" \
  --arg message "$MESSAGE" \
  --arg source_type "$SOURCE" \
  --arg source_basename "$BASENAME" \
  --arg wt_session "${WT_SESSION:-}" \
  --arg vscode_ipc_hook "${VSCODE_IPC_HOOK_CLI:-}" \
  --arg vscode_pid "${VSCODE_PID:-}" \
  --argjson timestamp_ms "$(date +%s%3N)" \
  '{event: $event, cwd: $cwd, message: $message, source_type: $source_type, source_basename: $source_basename, wt_session: $wt_session, vscode_ipc_hook: $vscode_ipc_hook, vscode_pid: $vscode_pid, timestamp_ms: $timestamp_ms}')

# Invoke Windows binary (non-blocking)
echo "$ENRICHED" | claude-overlay.exe --stdin &
disown

# Beep PowerShell parallèle (période de validation)
powershell.exe -c "[System.Media.SystemSounds]::Beep.Play()" &
disown

exit 0
```

**Points à noter**
- `&` + `disown` pour que le hook ne bloque pas Claude Code (les hooks ont un timeout court, on veut retourner immédiatement).
- `--stdin` = le binaire lit le JSON depuis stdin (évite les problèmes de shell-escaping sur Windows args).
- `jq` est une dépendance supposée présente. Sinon, fallback `printf` manuel ou check `command -v jq` avec message d'erreur.
- Aucun appel réseau direct, aucun call bloquant côté bash.
- `VSCODE_IPC_HOOK_CLI` est set par VS Code dans chaque terminal intégré (Remote-WSL ou natif) et identifie de manière unique la window VS Code source.

### 3.3 Extension VS Code — `claude-overlay-focus`

**Stack** : TypeScript, `vsce` pour build `.vsix`, ~150 lignes total, dépendance `ws` (client WebSocket Node).

**Manifest — `package.json` clés importantes**
```jsonc
{
  "name": "claude-overlay-focus",
  "version": "0.1.0",
  "engines": { "vscode": "^1.85.0" },
  "activationEvents": ["onStartupFinished"],
  "extensionKind": ["ui"],  // ← CRITIQUE: tourne côté Windows, pas dans WSL remote host
  "main": "./out/extension.js",
  "contributes": {
    "commands": [
      { "command": "claudeOverlay.reconnect", "title": "Claude Overlay: Force reconnect to daemon" },
      { "command": "claudeOverlay.status", "title": "Claude Overlay: Show connection status" }
    ]
  },
  "dependencies": { "ws": "^8.16.0" }
}
```

**Pourquoi `extensionKind: ["ui"]`**
- L'extension tourne sur Windows (même quand VS Code est en Remote-WSL)
- Le WebSocket `127.0.0.1:57843` pointe vers le daemon Windows — pas de traversée WSL
- `vscode.window.terminals` est proxifié : l'extension UI voit TOUS les terminaux (Remote-WSL inclus), avec `creationOptions.cwd` reflétant le path remote (`/home/user/...`)
- `terminal.sendText()` forward correctement vers le pty remote via le tunnel RPC interne de VS Code

**Fallback si l'API proxy ne donne pas accès aux cwd Remote-WSL comme attendu** : on passe à `extensionKind: ["ui", "workspace"]`. La variante Workspace (dans WSL remote extension host) se connecte au daemon via `$(ip route show default | awk '{print $3}'):57843` (gateway Windows). Plus fragile, activé seulement si le test manuel révèle le besoin.

**Comportement**

```typescript
// Pseudocode extension.ts

import * as vscode from 'vscode'
import WebSocket from 'ws'

const DAEMON_URL = 'ws://127.0.0.1:57843'
const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 5000

let ws: WebSocket | null = null
let reconnectTimer: NodeJS.Timeout | null = null
let reconnectDelay = RECONNECT_BASE_MS
const extId = crypto.randomUUID()
const ipcHook = process.env.VSCODE_IPC_HOOK_CLI || ''

export function activate(context: vscode.ExtensionContext) {
  connect()

  // Push updates quand les terminaux changent
  context.subscriptions.push(
    vscode.window.onDidOpenTerminal(() => sendTerminalsUpdate()),
    vscode.window.onDidCloseTerminal(() => sendTerminalsUpdate()),
    vscode.window.onDidChangeWindowState(e =>
      send({ type: 'WINDOW_FOCUS_CHANGED', focused: e.focused })
    )
  )
}

function connect() {
  ws = new WebSocket(DAEMON_URL)

  ws.on('open', () => {
    reconnectDelay = RECONNECT_BASE_MS
    send({
      type: 'REGISTER',
      ext_id: extId,
      vscode_ipc_hook: ipcHook,
      workspace_folders: vscode.workspace.workspaceFolders?.map(f => f.uri.fsPath) ?? [],
      vscode_pid: process.pid,
      window_focused: vscode.window.state.focused,
    })
    sendTerminalsUpdate()
  })

  ws.on('message', handleCommand)

  ws.on('close', () => {
    ws = null
    scheduleReconnect()
  })

  ws.on('error', () => { /* ignore, will trigger close */ })
}

function scheduleReconnect() {
  if (reconnectTimer) return
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS)
    connect()
  }, reconnectDelay)
}

async function handleCommand(raw: WebSocket.RawData) {
  const msg = JSON.parse(raw.toString())
  if (msg.type === 'FOCUS') {
    const t = findTerminal(msg.cwd)
    t?.show(false)
    reply(msg.cmd_id, { ok: !!t })
  } else if (msg.type === 'SEND_TEXT') {
    const t = findTerminal(msg.cwd)
    if (t) { t.sendText(msg.text, false); reply(msg.cmd_id, { ok: true }) }
    else   { reply(msg.cmd_id, { ok: false, reason: 'terminal_not_found' }) }
  } else if (msg.type === 'IS_ACTIVE_TERMINAL') {
    const active = vscode.window.state.focused
      && vscode.window.activeTerminal?.creationOptions?.cwd === msg.cwd
    reply(msg.cmd_id, { ok: true, active })
  } else if (msg.type === 'PING') {
    reply(msg.cmd_id, { ok: true })
  }
}

function findTerminal(cwd: string): vscode.Terminal | undefined {
  return vscode.window.terminals.find(t => {
    const tcwd = (t.creationOptions as any)?.cwd
    return typeof tcwd === 'string' ? tcwd === cwd
           : tcwd?.fsPath === cwd
  })
}

function sendTerminalsUpdate() {
  send({
    type: 'TERMINALS_UPDATED',
    terminals: vscode.window.terminals.map(t => ({
      name: t.name,
      cwd: (t.creationOptions as any)?.cwd?.toString() ?? null,
      pid: (t as any).processId ?? null,
    })),
  })
}

function send(obj: any) { ws?.send(JSON.stringify(obj)) }
function reply(cmd_id: string, data: any) { send({ type: 'COMMAND_RESULT', cmd_id, ...data }) }

export function deactivate() { ws?.close() }
```

**Gestion du cycle de vie**
- `onStartupFinished` activation → connect au daemon au boot VS Code
- Reconnect avec backoff exponentiel (500ms → 1s → 2s → 4s → 5s cap) si daemon down
- Chaque connect régénère un `ext_id` frais (le daemon ne stocke pas d'état persistant entre disconnects)

**Edge cases**
- Plusieurs terminaux dans la même window avec le même cwd : `findTerminal` retourne le premier. Documenté comme limite v1.
- Daemon jamais démarré (pas d'auto-start installé, pas de Claude Code lancé) : l'extension retry à vie avec backoff 5s. Coût négligeable (1 connect tentative toutes les 5s).
- VS Code fermé pendant une notif : WS disconnect, daemon purge l'extension, si l'user clique Focus après → fallback "extension unreachable" affiché.

**Installation**
```
vsce package
code --install-extension claude-overlay-focus-0.1.0.vsix
```

---

## 4. Data flow complet — cycle de vie d'une notification

```
1. Claude Code (WSL) déclenche hook Notification / Stop
      │
      ▼
2. ~/.claude/hooks/claude-overlay-notify.sh
      │  enrichit payload avec source_type + VSCODE_IPC_HOOK_CLI + WT_SESSION
      │  fork beep PowerShell (parallèle)
      ▼
3. claude-overlay.exe --stdin (mode client)
      │  TCP connect 127.0.0.1:57842
      │    ├─ succès → envoie JSON, reçoit ACK, exit
      │    └─ refus (daemon down) → bascule en mode daemon, enchaîne étape 4
      ▼
4. Daemon reçoit la notif (TCP 57842)
      │
      │  a. Matching target VS Code (si source_type=vscode)
      │     - Match exact par vscode_ipc_hook dans registre extensions
      │         └─ trouvé → target_ext_id = X
      │     - Sinon : extensions ayant un terminal avec ce cwd
      │         └─ si unique → target_ext_id = Y
      │         └─ si plusieurs → prend la plus récemment focused
      │         └─ si aucune → dégrade en source_type=unknown
      │
      │  b. Check "skip si foreground ?"
      │     - source=vscode avec target_ext_id → WS send IS_ACTIVE_TERMINAL {cwd}
      │         └─ si active:true → drop notif, exit path
      │     - source=wt → GetForegroundWindow + class + title match
      │         └─ si match → drop
      │     - source=unknown → jamais skip
      │
      │  c. Parse heuristique y/N depuis message (regex multi-format)
      │     - détecte format: "y_n" | "yes_no" | null
      │     - set buttons_yes_no = (format !== null)
      │
      │  d. Crée NotifState {
      │       id, source_type, target_ext_id?, cwd, message, source_basename,
      │       format, buttons_yes_no, created_at, timeout_handle
      │     }
      │     Push dans store (Vec<NotifState>)
      │
      │  e. emit("notif:new", state) vers Tauri webview
      ▼
5. Frontend (webview)
      │  - reçoit event, insert <div.notif-row> avec slide-in animation
      │  - pill container ajuste sa hauteur (CSS transition)
      │  - si buttons_yes_no: affiche Yes/No en plus de Focus/×
      ▼
6. Utilisateur interagit (ou timer expire)
      │
      ├─ timer 15s (Notif) / 3s (Stop)  →  action=dismiss notif_id
      ├─ clic Focus                      →  action=focus notif_id
      ├─ clic Yes / No                   →  action=send_input notif_id, text="y"|"n"
      ├─ clic ×                          →  action=dismiss notif_id
      ▼
7. Frontend → backend Rust (Tauri invoke) → dispatch selon action (§5)
      │  après action réussie: emit("notif:remove", notif_id)
      ▼
8. Frontend retire la row (slide-out + fade), ajuste hauteur pill
   Si plus aucune notif vivante: fade-out complet de la pill après 200ms
```

---

## 5. Actions détaillées par source_type

### 5.1 `Focus terminal`

| source_type | Mécanisme |
|---|---|
| `vscode` avec `target_ext_id` | Daemon envoie WS `{type:"FOCUS", cwd}` à l'extension identifiée → extension fait `terminal.show(preserveFocus=false)`. Timeout réponse 500ms. Si KO → fallback title match `Code.exe` puis unknown. |
| `wt` | `EnumWindows` filtré sur classe `CASCADIA_HOSTING_WINDOW_CLASS`. Pour chaque HWND : `GetWindowText` → si contient `source_basename` (case-insensitive) → `SetForegroundWindow` + `ShowWindow SW_RESTORE`. Fallback : WT le plus récent. |
| `unknown` | Best-effort : `EnumWindows` all top-level matching basename, prend le plus récent. Sinon error inline dans la pill. |

### 5.2 `Yes` / `No` (envoi d'input)

**Payload à envoyer (selon format détecté)**
| format | Yes envoie | No envoie |
|---|---|---|
| `y_n` (prompt `[y/N]` ou `[Y/n]` ou `(y/n)`) | `y\n` | `n\n` |
| `yes_no` (prompt `[yes/no]` ou `(yes/no)`) | `yes\n` | `no\n` |
| ambigu (détecté mais pattern flou) | `yes\n` | `no\n` (versions longues = universelles) |

**Dispatch**
| source_type | Mécanisme |
|---|---|
| `vscode` avec `target_ext_id` | WS `{type:"SEND_TEXT", cwd, text}` → extension fait `terminal.sendText(text, addNewLine=false)`. **100% fiable**, pas de dépendance au focus. |
| `wt` | Séquence garde-foutée : (a) `SetForegroundWindow(hwnd_target)` ; (b) `Sleep(30ms)` propagation focus ; (c) `GetForegroundWindow()` check ; (d) si OK → `SendInput` texte + VK_RETURN ; (e) si KO → emit `notif:error` → affichage inline "focus lost, type manually" 3s. **~95% fiable**. |
| `unknown` | Même approche que `wt`. |

**Après envoi réussi** : la notif source est auto-dismiss (user a répondu).
**Après échec** (WT garde-fou) : la notif reste affichée, user peut retry ou Focus + taper manuellement.

### 5.3 `Dismiss` (×)

Retire la `NotifState` du store, emit `notif:remove`, la row fade-out, la pill rétracte sa hauteur. Si plus de notifs vivantes → pill fade-out complet.

### 5.4 Auto-close timer

- `event === "Notification"` : 15s
- `event === "Stop"` : 3s

**Stratégie** : timer par notif individuelle (chaque ligne expire indépendamment). Si feel gênant pendant l'impl (pill qui fluctue), bascule sur timer global pill-level (décision mid-implementation).

---

## 6. UI/UX — Dynamic Island pill

### Layout statique
- Position : top-center écran (monitor principal), offset 16px du haut, z-order always-on-top (`WS_EX_TOPMOST` via Tauri `alwaysOnTop: true`)
- Largeur : auto, min 360px, max 640px
- Hauteur : auto, 1 ligne = ~52px, chaque ligne additionnelle +44px
- Coins : `border-radius: 24px`
- Background : `rgba(22, 22, 26, 0.92)` + `backdrop-filter: blur(20px) saturate(180%)`
- Police : system UI (Segoe UI Variable sur Win11), light weight pour message, medium pour basename
- Bordure : 1px `rgba(255,255,255,0.08)` pour définition sur bg clair

### Contenu par ligne
```
[● vert/bleu]  myproject › Claude is waiting for your input [y/N]     [Yes] [No] [Focus] [×]
```
- Pastille statut : vert si `event=Stop`, bleu pulsant si `event=Notification`
- Prefix `basename ›` en medium
- Message en light, truncate à ~80 chars avec ellipsis + tooltip complet au hover
- Boutons : pill-shaped small, Yes/No = filled accent, Focus = ghost, × = icon only

### Animations
- Apparition pill : fade-in 150ms + slide-down 12px (ease-out)
- Nouvelle ligne dans pill existante : slide-in bottom + fade 200ms
- Pill grow/shrink hauteur : `transition: height 250ms cubic-bezier(0.2, 0, 0.2, 1)`
- Dismiss ligne : slide-out opposé + fade 180ms
- Fade-out pill entière (plus de notifs) : fade 200ms après délai 100ms

### États visuels spéciaux
- Envoi Yes/No en cours (WT) : boutons grisés avec spinner 300ms
- Erreur envoi (WT focus lost) : ligne flash rouge 200ms + message "couldn't focus, type manually" remplace boutons 3s puis restore
- Extension VS Code unreachable (target_ext_id introuvable) : erreur "VS Code extension offline, falling back to window focus"

---

## 7. Détection foreground (skip notif)

**Règle** : skip la notif si et seulement si le terminal source correspondant est foreground au moment du fire.

**Implémentation par source_type**

| source_type | Check foreground |
|---|---|
| `vscode` avec `target_ext_id` | WS `{type:"IS_ACTIVE_TERMINAL", cwd}` à l'extension. Réponse `{active: true}` ssi `window.state.focused && activeTerminal.creationOptions.cwd === cwd`. Timeout 200ms → considère false (affiche la notif). |
| `wt` | `GetForegroundWindow()` → HWND. `GetClassName(hwnd)` → si `== "CASCADIA_HOSTING_WINDOW_CLASS"` → `GetWindowText` → si contient `source_basename` → skip. |
| `unknown` | Jamais skip. |

**Side-effect assumé** : le beep parallèle (fire depuis le hook bash indépendamment du binaire overlay) **joue quand même** si skip. Ça reste un signal discret qui évite le silence total. Si gênant, on coupera en v1.1.

---

## 8. Packaging & installation

### Arborescence finale
```
C:\Users\adrie\.local\bin\
    claude-overlay.exe              # binaire Tauri release, single-file

~/.claude/hooks/
    claude-overlay-notify.sh        # hook bash (config settings.json pointe ici)

VS Code extensions (install locale):
    claude-overlay-focus-0.1.0.vsix
```

### Config `~/.claude/settings.json` — patch
```jsonc
{
  "hooks": {
    "Notification": [
      { "matcher": "", "hooks": [{ "type": "command", "command": "bash ~/.claude/hooks/claude-overlay-notify.sh" }] }
    ],
    "Stop": [
      { "matcher": "", "hooks": [{ "type": "command", "command": "bash ~/.claude/hooks/claude-overlay-notify.sh" }] }
    ]
  }
}
```

### Install procédure one-time

À documenter dans `docs/install.md` du repo de l'outil :

```powershell
# 1. Copier le binaire dans ~/.local/bin et l'ajouter au PATH Windows
mkdir "$env:USERPROFILE\.local\bin" -ea 0
copy claude-overlay.exe "$env:USERPROFILE\.local\bin\"
[Environment]::SetEnvironmentVariable(
  "PATH",
  $env:PATH + ";$env:USERPROFILE\.local\bin",
  "User"
)

# 2. Installer l'auto-start Windows (Registry Run key, HKCU, no admin)
& "$env:USERPROFILE\.local\bin\claude-overlay.exe" --install-autostart

# 3. Installer l'extension VS Code
code --install-extension claude-overlay-focus-0.1.0.vsix
```

```bash
# 4. Copier le hook bash dans WSL
cp hooks/claude-overlay-notify.sh ~/.claude/hooks/
chmod +x ~/.claude/hooks/claude-overlay-notify.sh

# 5. Patcher ~/.claude/settings.json pour pointer le hook (cf. section ci-dessus)

# 6. Démarrer le daemon manuellement une 1ère fois (il redémarre au boot ensuite)
claude-overlay.exe --daemon  # non-blocking, se détache
```

### Structure du repo source (recommandation)
Repo standalone `~/code/claude-overlay/` :
```
claude-overlay/
├─ Cargo.toml
├─ tauri.conf.json
├─ src/                           # Rust backend
│   ├─ main.rs                    # argument parsing, mode dispatch
│   ├─ daemon.rs                  # TCP 57842 + WS 57843 listeners + mutex
│   ├─ registry.rs                # Map<ext_id, ExtensionConnection>
│   ├─ store.rs                   # NotifState Vec
│   ├─ focus_win32.rs             # EnumWindows, SetForegroundWindow, SendInput
│   ├─ vscode_client.rs           # send WS commands + await responses
│   ├─ autostart.rs               # Registry Run key read/write
│   └─ heuristic.rs               # regex y/N detection + format extract
├─ ui/                            # Frontend webview
│   ├─ index.html
│   ├─ style.css
│   └─ app.js
├─ vscode-ext/                    # Extension VS Code
│   ├─ package.json
│   ├─ tsconfig.json
│   ├─ src/extension.ts
│   └─ README.md
├─ hooks/
│   └─ claude-overlay-notify.sh
└─ docs/
    ├─ install.md
    └─ 2026-04-24-claude-overlay-design.md  # copie du spec pour vie autonome
```

---

## 9. Configuration runtime

Aucune configuration externe requise en v1 — valeurs en dur :

| Valeur | Default | Description |
|---|---|---|
| Port daemon hooks | 57842 | TCP |
| Port daemon extensions | 57843 | WebSocket |
| Timeout auto-close Notification | 15000 ms | |
| Timeout auto-close Stop | 3000 ms | |
| Skip if foreground | enabled | |
| Beep parallèle | enabled | Via hook bash |
| Reconnect extension backoff | 500ms → 5s cap | Exponentiel |
| WS timeout commande | 500 ms | Focus / SendText |
| Foreground check timeout | 200 ms | IS_ACTIVE_TERMINAL |

**v1 : pas de config file.** Si besoin de tuning plus tard, on ajoutera `%APPDATA%\claude-overlay\config.toml`.

---

## 10. Tests

v1 = **tests manuels principaux**, unit tests ciblés sur la logique parseable.

### Unit tests Rust (automatisés)
- `heuristic.rs` : 10-15 cases sur regex y/N matching et format detection
  - `"[y/N]"` → `Some((true, YesNoFormat::YN))`
  - `"Save changes [Y/n]?"` → `Some((true, YesNoFormat::YN))`
  - `"Confirm? [yes/no]"` → `Some((true, YesNoFormat::YesNo))`
  - `"What's your name?"` → `None`
  - `"Add 'yes' to the list"` → acceptable faux-positif, documenter
- `store.rs` : state machine add / remove / expire
- `registry.rs` : register / deregister / find_by_ipc_hook / find_by_cwd

### Tests manuels — checklist de validation v1
À cocher avant "done" :

1. [ ] Daemon démarre au boot Windows (vérifier dans Task Manager `claude-overlay.exe` présent)
2. [ ] Notif unique depuis VS Code Remote-WSL : pill s'affiche, Focus ramène le bon terminal VS Code
3. [ ] Notif unique depuis VS Code fenêtre #2 (autre workspace, autre ipc_hook) : focus la **bonne** window, pas la 1ère
4. [ ] Notif unique depuis WT : title match correct, focus fonctionne
5. [ ] Notif unique depuis cmd.exe standalone (source=unknown) : fallback best-effort OK
6. [ ] Burst : 3 notifs en <2s depuis 3 VS Code windows différentes → pill grossit, 3 lignes distinctes
7. [ ] Yes button sur prompt `[y/N]` dans VS Code : envoie `y\n` dans le bon terminal de la bonne window
8. [ ] Yes button sur prompt `[yes/no]` dans WT : envoie `yes\n` via SendKeys, overlay dismiss
9. [ ] No button sur prompt `[Y/n]` dans VS Code Remote-WSL : envoie `n\n`, terminal Linux le reçoit
10. [ ] Message sans pattern y/N : boutons Yes/No absents, juste Focus et ×
11. [ ] Skip foreground VS Code : focus VS Code terminal cwd X, fire notif pour X → pas d'overlay (beep OK)
12. [ ] Skip foreground WT : focus WT window title myproject, fire notif cwd myproject → pas d'overlay
13. [ ] Auto-close Notification à 15s
14. [ ] Auto-close Stop à 3s
15. [ ] Kill daemon pendant notif : process exit, next notif respawn daemon via hook bash fallback
16. [ ] Tray icon Quit : ferme tout, aucun orphan
17. [ ] VS Code fermé + notif source=vscode : target_ext_id introuvable, fallback affiche message approprié
18. [ ] WT focus steal pendant Yes : error inline "couldn't focus, type manually"
19. [ ] Reboot Windows : daemon auto-restart, extensions se reconnectent après ouverture VS Code
20. [ ] `--install-autostart` puis `--uninstall-autostart` : clé Registry bien écrite puis supprimée
21. [ ] 50 notifs sur 30 min : pas de leak mémoire visible (check Task Manager), daemon stable
22. [ ] Terminal VS Code natif Windows (PowerShell) — Claude Code pas applicable directement, mais vérifier que `vscode.window.terminals` les liste correctement côté extension (pour éviter des crashes si type inattendu)

Pas de bench launch-time automatisé en v1 (mesure manuelle Measure-Command sur le path client).

---

## 11. Open questions & risques

### Open questions à trancher pendant l'implémentation
1. **Repo standalone vs sous-dossier d'un autre projet** : reco repo standalone (`~/code/claude-overlay/`). Le spec vit dans `other-project/docs/specs/` par convention user mais sera copié dans le repo outil pour vie autonome.
2. **Timer auto-close par-notif vs pill-global** : commence par-notif, bascule si feel gênant.
3. **Beep dans hook bash ou hook séparé** : dans hook bash (section 3.2). Plus simple, user commente la ligne après validation.
4. **`extensionKind: ["ui"]` suffit pour terminaux Remote-WSL** : à valider au test 2 de la checklist. Fallback `["ui", "workspace"]` documenté.
5. **Path du `.local\bin` dans PATH Windows** : via `SetEnvironmentVariable` PowerShell (method dans install.md). Alternative : ajouter manuellement via UI système. Non-critique.

### Risques
| Risque | Impact | Mitigation |
|---|---|---|
| SendKeys WT échoue par focus steal | User doit retaper la réponse | Garde-fou `GetForegroundWindow` + error inline. Acceptable v1. |
| Extension VS Code Kind UI ne voit pas les cwd Remote-WSL comme attendu | Focus/Yes/No cassés pour terminaux WSL | Test manuel précoce (checklist #2). Fallback `extensionKind: ["ui", "workspace"]`. |
| WebView2 runtime manquant | Daemon ne démarre pas | Win11 l'a pré-installé. Check boot + message clair si absent. |
| Race mutex 2 daemons | Instabilité | Named mutex `Global\claude-overlay-daemon`, perdant → exit. |
| Regex y/N faux-positif | Boutons affichés sur question libre mentionnant "yes" | Heuristique tolère le bruit, user ignore les boutons. Raffinement à l'usage. |
| Daemon crashe sans restart | Overlay inactif jusqu'au reboot ou prochain hook | Auto-daemon fallback hook-spawn le respawn. Pas de supervisor robuste v1 (acceptable). |
| Plusieurs terminaux VS Code même cwd même window | Focus ambigu, 1er trouvé | Documenté comme limite. Rare en pratique. |
| Port 57842/57843 conflit | Daemon ne peut pas bind | Improbable (ports choisis dans plage libre). Fallback discovery `%TEMP%\claude-overlay.port` → v1.1 si ça arrive. |
| Registry Run key nécessite user logon | Daemon pas actif avant login | Acceptable (Claude Code aussi nécessite user login). |
| Extension bloquée en retry loop si daemon jamais up | Log spam, CPU minime | Backoff cap 5s, donc 1 tentative toutes les 5s. Négligeable. |

---

## 12. Critères d'acceptation v1

v1 est "done" quand :
- ✅ Les 22 items de la checklist §10 sont cochés
- ✅ L'outil est installé dans le workflow quotidien (daemon auto-start activé, binaire dans PATH Windows, extension VS Code installée, hook bash en place, `settings.json` patché)
- ✅ User a fait au moins 3 sessions Claude Code complètes avec l'overlay actif sans bug bloquant
- ✅ Le spec est copié dans le repo standalone de l'outil
- ✅ Un `docs/install.md` permet une réinstall from-scratch en <10 min
- ✅ Les unit tests Rust (heuristic, store, registry) passent

---

## 13. Futur (post-v1)

Classés par priorité ressentie :

1. **Écriture TTY pour WT (v1.1)** si SendKeys déçoit (>10% d'échecs) : wrapper autour de `claude` CLI qui capture le `tty` et expose un endpoint pour écrire directement dans le slave pty. 100% fiable, plus complexe.
2. **Click sur la pill entière = Focus** (polish)
3. **Son custom configurable** (remplacer le beep)
4. **Config file** (`%APPDATA%\claude-overlay\config.toml`) pour ports, timeouts, skip_if_foreground
5. **Persistance queue** via fichier pour survivre à un crash daemon
6. **Nova integration** : extraction dans un module nova avec `SETUP.sh` qui install binaire + extension + hook + settings.json patch. À faire quand l'outil est stable.
7. **Focus panneau terminal VS Code spécifique** parmi plusieurs panneaux même window avec cwd identique : tracker l'instance terminal unique au moment du hook via un id injecté dans le terminal à sa création.
8. **Supervisor robuste** : Task Scheduler avec restart-on-crash au lieu de Registry Run key simple.
9. **Toast Windows fallback** si overlay désactivé (option en config).
10. **Claude Code lancé depuis PowerShell natif Windows** : adapter le hook (devient un `.ps1` avec même enrichissement).

---

**Fin du document.**
