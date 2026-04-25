import * as vscode from 'vscode';
import * as crypto from 'crypto';
import WebSocket, { RawData } from 'ws';

const DAEMON_URL = 'ws://127.0.0.1:57843';
const RECONNECT_BASE_MS = 500;
const RECONNECT_MAX_MS = 5000;

let ws: WebSocket | null = null;
let reconnectTimer: NodeJS.Timeout | null = null;
let reconnectDelay = RECONNECT_BASE_MS;
const extId = crypto.randomUUID();
const ipcHook = process.env.VSCODE_IPC_HOOK_CLI ?? '';

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
  const c = (t.creationOptions as vscode.TerminalOptions | undefined)?.cwd;
  if (!c) return null;
  return typeof c === 'string' ? c : c.fsPath;
}

function findTerminal(cwd: string): vscode.Terminal | undefined {
  return vscode.window.terminals.find(t => terminalCwd(t) === cwd);
}

function sendTerminalsUpdate() {
  send({
    type: 'TERMINALS_UPDATED',
    terminals: vscode.window.terminals.map(t => ({
      name: t.name,
      cwd: terminalCwd(t),
      pid: null,
    })),
  });
}

function handleMessage(raw: RawData) {
  let msg: any;
  try { msg = JSON.parse(raw.toString()); } catch { return; }
  const cmdId = msg.cmd_id;
  switch (msg.type) {
    case 'FOCUS': {
      const t = findTerminal(msg.cwd);
      if (t) { t.show(false); reply(cmdId, {}); }
      else   { replyErr(cmdId, 'terminal_not_found'); }
      break;
    }
    case 'SEND_TEXT': {
      const t = findTerminal(msg.cwd);
      if (t) { t.sendText(msg.text, false); reply(cmdId, {}); }
      else   { replyErr(cmdId, 'terminal_not_found'); }
      break;
    }
    case 'IS_ACTIVE_TERMINAL': {
      const active =
        vscode.window.state.focused &&
        terminalCwd(vscode.window.activeTerminal as vscode.Terminal) === msg.cwd;
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

  ws.on('open', () => {
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
    sendTerminalsUpdate();
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
  connect();

  ctx.subscriptions.push(
    vscode.window.onDidOpenTerminal(sendTerminalsUpdate),
    vscode.window.onDidCloseTerminal(sendTerminalsUpdate),
    vscode.window.onDidChangeWindowState(e =>
      send({ type: 'WINDOW_FOCUS_CHANGED', focused: e.focused })
    ),
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
