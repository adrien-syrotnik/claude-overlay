import * as vscode from 'vscode';
import * as crypto from 'crypto';
import * as fs from 'fs';
import WebSocket, { RawData } from 'ws';

const DAEMON_URL = 'ws://127.0.0.1:47843';
const DEBUG_LOG = '/tmp/claude-overlay-ext.log';

function dlog(msg: string) {
  try {
    fs.appendFileSync(DEBUG_LOG, `[${new Date().toISOString()}] ${msg}\n`);
  } catch (_) { /* ignore */ }
}
const RECONNECT_BASE_MS = 500;
const RECONNECT_MAX_MS = 5000;

let ws: WebSocket | null = null;
let reconnectTimer: NodeJS.Timeout | null = null;
let reconnectDelay = RECONNECT_BASE_MS;
const extId = crypto.randomUUID();
const ipcHook = process.env.VSCODE_IPC_HOOK_CLI ?? '';

// Per-terminal state. Cleaned up on terminal close.
const terminalPids = new Map<vscode.Terminal, number | null>();
const terminalLastActive = new Map<vscode.Terminal, number>();

function log(...args: unknown[]) {
  console.log('[claude-overlay]', ...args);
}

function send(obj: unknown) {
  try { ws?.send(JSON.stringify(obj)); } catch (_) { /* socket closed */ }
}

function reply(cmdId: string, data: Record<string, unknown>) {
  send({ type: 'COMMAND_RESULT', cmd_id: cmdId, ok: true, ...data });
}
function replyErr(cmdId: string, reason: string) {
  send({ type: 'COMMAND_RESULT', cmd_id: cmdId, ok: false, reason });
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS);
    connect();
  }, reconnectDelay);
}

function terminalCwd(t: vscode.Terminal): string | null {
  // Prefer shellIntegration.cwd — the live cwd of the running shell, which
  // tracks `cd` mutations. Fallback to creationOptions.cwd.
  const live = (t as any).shellIntegration?.cwd as vscode.Uri | undefined;
  if (live) return live.fsPath;
  const c = (t.creationOptions as vscode.TerminalOptions | undefined)?.cwd;
  if (!c) return null;
  return typeof c === 'string' ? c : c.fsPath;
}

function pathsEqual(a: string | null, b: string | null): boolean {
  if (!a || !b) return false;
  const norm = (s: string) => s.replace(/[/\\]+$/, '');
  const na = norm(a);
  const nb = norm(b);
  if (process.platform === 'win32') return na.toLowerCase() === nb.toLowerCase();
  return na === nb;
}

async function ensurePid(t: vscode.Terminal): Promise<number | null> {
  if (terminalPids.has(t)) return terminalPids.get(t) ?? null;
  try {
    const pid = (await t.processId) ?? null;
    terminalPids.set(t, pid);
    dlog(`ensurePid name=${t.name} → pid=${pid}`);
    return pid;
  } catch (e) {
    terminalPids.set(t, null);
    dlog(`ensurePid name=${t.name} ERROR ${e}`);
    return null;
  }
}

function findTerminalByPid(pid: number): vscode.Terminal | undefined {
  for (const t of vscode.window.terminals) {
    if (terminalPids.get(t) === pid) return t;
  }
  return undefined;
}

function findTerminalSmart(cwd: string, pid?: number): vscode.Terminal | undefined {
  const snapshot = vscode.window.terminals.map(t => ({
    name: t.name,
    cwd: terminalCwd(t),
    pid: terminalPids.get(t),
    exited: t.exitStatus !== undefined,
    lastActive: terminalLastActive.get(t),
  }));
  dlog(`findTerminalSmart req={cwd:${cwd}, pid:${pid}} terminals=${JSON.stringify(snapshot)} active=${vscode.window.activeTerminal?.name}`);

  // 1. Exact PID match wins. This is the strongest signal — Claude is running
  //    in this exact shell process, so this terminal is THE one even when
  //    multiple terminals share a cwd.
  if (pid && pid > 0) {
    const byPid = findTerminalByPid(pid);
    if (byPid) { dlog(`  → matched by pid=${pid} → name=${byPid.name}`); return byPid; }
    dlog(`  pid=${pid} not found in cache, falling back to cwd`);
  }
  // 2. Cwd match. If multiple, prefer the active terminal, then most-recently-active.
  const matches = vscode.window.terminals.filter(t =>
    t.exitStatus === undefined && pathsEqual(terminalCwd(t), cwd)
  );
  if (matches.length === 0) { dlog(`  → no cwd match`); return undefined; }
  if (matches.length === 1) { dlog(`  → single cwd match: ${matches[0].name}`); return matches[0]; }
  const active = vscode.window.activeTerminal;
  if (active && matches.includes(active)) {
    dlog(`  → multiple cwd matches; preferring activeTerminal: ${active.name}`);
    return active;
  }
  matches.sort((a, b) =>
    (terminalLastActive.get(b) ?? 0) - (terminalLastActive.get(a) ?? 0)
  );
  dlog(`  → multiple cwd matches; picking by recency: ${matches[0].name}`);
  return matches[0];
}

async function sendTerminalsUpdate() {
  const terms = await Promise.all(
    vscode.window.terminals.map(async t => ({
      name: t.name,
      cwd: terminalCwd(t),
      pid: await ensurePid(t),
    }))
  );
  send({ type: 'TERMINALS_UPDATED', terminals: terms });
}

function handleMessage(raw: RawData) {
  let msg: any;
  try { msg = JSON.parse(raw.toString()); } catch { return; }
  const cmdId = msg.cmd_id;
  switch (msg.type) {
    case 'FOCUS': {
      const t = findTerminalSmart(msg.cwd, msg.pid);
      if (t) { t.show(false); reply(cmdId, {}); }
      else   { replyErr(cmdId, 'terminal_not_found'); }
      break;
    }
    case 'SEND_TEXT': {
      const t = findTerminalSmart(msg.cwd, msg.pid);
      if (t) { t.sendText(msg.text, false); reply(cmdId, {}); }
      else   { replyErr(cmdId, 'terminal_not_found'); }
      break;
    }
    case 'IS_ACTIVE_TERMINAL': {
      const at = vscode.window.activeTerminal;
      // PID-precise active check when daemon supplies it; else fall back to cwd.
      let active = false;
      if (vscode.window.state.focused && at) {
        if (msg.pid && msg.pid > 0) {
          active = terminalPids.get(at) === msg.pid;
        } else {
          active = pathsEqual(terminalCwd(at), msg.cwd);
        }
      }
      reply(cmdId, { active });
      break;
    }
    case 'PING':
      reply(cmdId, {});
      break;
  }
}

function connect() {
  log('connecting to', DAEMON_URL);
  ws = new WebSocket(DAEMON_URL);

  ws.on('open', async () => {
    log('connected');
    reconnectDelay = RECONNECT_BASE_MS;
    send({
      type: 'REGISTER',
      ext_id: extId,
      vscode_ipc_hook: ipcHook,
      workspace_folders: vscode.workspace.workspaceFolders?.map(f => f.uri.fsPath) ?? [],
      vscode_pid: process.pid,
      window_focused: vscode.window.state.focused,
    });
    // Pre-warm the pid cache before the first TERMINALS_UPDATED so daemon
    // routing has PIDs from the start.
    await Promise.all(vscode.window.terminals.map(t => ensurePid(t)));
    await sendTerminalsUpdate();
  });

  ws.on('message', handleMessage);

  ws.on('close', () => {
    log('disconnected');
    ws = null;
    scheduleReconnect();
  });

  ws.on('error', () => { /* swallow, close will trigger */ });
}

export function activate(ctx: vscode.ExtensionContext) {
  // Mark currently active terminal as recent so the very first FOCUS routing
  // has a tiebreaker even before any onDidChange fires.
  const initialActive = vscode.window.activeTerminal;
  if (initialActive) terminalLastActive.set(initialActive, Date.now());

  connect();

  const onActiveChange = vscode.window.onDidChangeActiveTerminal(t => {
    if (t) terminalLastActive.set(t, Date.now());
  });

  const onOpen = vscode.window.onDidOpenTerminal(async (t) => {
    await ensurePid(t);
    await sendTerminalsUpdate();
  });

  const onClose = vscode.window.onDidCloseTerminal((t) => {
    terminalPids.delete(t);
    terminalLastActive.delete(t);
    sendTerminalsUpdate();
  });

  // shellIntegration fires on initial integration setup AND on cwd changes.
  // Refresh the daemon's view so it sees fresh cwds.
  const onShellIntegration = (vscode.window as any)
    .onDidChangeTerminalShellIntegration?.(async (e: any) => {
      if (e?.terminal) terminalLastActive.set(e.terminal, Date.now());
      await sendTerminalsUpdate();
    });

  // Per-command-end: a strong signal that THIS terminal is the active Claude one.
  const onShellExecEnd = (vscode.window as any)
    .onDidEndTerminalShellExecution?.((e: any) => {
      if (e?.terminal) terminalLastActive.set(e.terminal, Date.now());
    });

  const onWindowState = vscode.window.onDidChangeWindowState(e =>
    send({ type: 'WINDOW_FOCUS_CHANGED', focused: e.focused })
  );

  const subs: (vscode.Disposable | undefined)[] = [
    onOpen, onClose, onActiveChange, onShellIntegration, onShellExecEnd, onWindowState,
  ];
  for (const s of subs) if (s) ctx.subscriptions.push(s);

  ctx.subscriptions.push(
    vscode.commands.registerCommand('claudeOverlay.reconnect', () => {
      ws?.close(); scheduleReconnect();
    }),
    vscode.commands.registerCommand('claudeOverlay.status', () => {
      const state = ws?.readyState === WebSocket.OPEN ? 'connected' : 'disconnected';
      vscode.window.showInformationMessage(`Claude Overlay: ${state} (ext_id=${extId.slice(0,8)})`);
    }),
  );
}

export function deactivate() { ws?.close(); }
