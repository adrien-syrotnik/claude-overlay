#!/usr/bin/env bash
set -euo pipefail

PAYLOAD=$(cat)

CWD=$(jq -r '.cwd // empty' <<<"$PAYLOAD")
BASENAME=$(basename "$CWD")
MESSAGE=$(jq -r '.message // empty' <<<"$PAYLOAD")
EVENT=$(jq -r '.hook_event_name // empty' <<<"$PAYLOAD")

if [[ "${TERM_PROGRAM:-}" == "vscode" ]]; then
  SOURCE="vscode"
elif [[ -n "${WT_SESSION:-}" ]]; then
  SOURCE="wt"
else
  SOURCE="unknown"
fi

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

(echo "$ENRICHED" | claude-overlay.exe --stdin) &
disown

(powershell.exe -c "[System.Media.SystemSounds]::Beep.Play()" >/dev/null 2>&1) &
disown

exit 0
