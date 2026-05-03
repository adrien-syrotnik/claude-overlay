#!/usr/bin/env bash
set -euo pipefail

PAYLOAD=$(cat)

CWD=$(jq -r '.cwd // empty' <<<"$PAYLOAD")
BASENAME=$(basename "$CWD")
EVENT=$(jq -r '.hook_event_name // empty' <<<"$PAYLOAD")
MESSAGE=$(jq -r '.message // empty' <<<"$PAYLOAD")
NOTIFICATION_TYPE=$(jq -r '.notification_type // empty' <<<"$PAYLOAD")

if [[ "${TERM_PROGRAM:-}" == "vscode" ]]; then
  SOURCE="vscode"
elif [[ -n "${WT_SESSION:-}" ]]; then
  SOURCE="wt"
else
  SOURCE="unknown"
fi

# Walk the process tree up from our parent and return the FIRST interactive
# shell ancestor. That is the shell where Claude was launched — what
# `terminal.processId` reports under VS Code Remote-WSL.
#
# We deliberately match only `bash|zsh|fish|dash|ksh` and NOT plain `sh`,
# because VS Code's WSL relay layer ALSO uses `sh` (visible higher up in the
# tree as a shared helper that's identical across all sessions in the same
# VS Code window). Walking past the first interactive shell would land on
# that shared helper and route every session to the same wrong terminal.
get_shell_pid() {
  local pid=${PPID:-0}
  local depth=0
  while [ "$pid" != "0" ] && [ "$pid" != "1" ] && [ "$depth" -lt 30 ]; do
    local comm
    comm=$(ps -o comm= -p "$pid" 2>/dev/null | tr -d '\n ' || true)
    [ -z "$comm" ] && break
    case "$comm" in
      bash|zsh|fish|dash|ksh) echo "$pid"; return ;;
    esac
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
    [ -z "$pid" ] && break
    depth=$((depth + 1))
  done
  echo "0"
}
SHELL_PID=$(get_shell_pid)

LOG_FILE=/tmp/claude-overlay-hook.log
log() { printf '[%s] %s\n' "$(date '+%H:%M:%S')" "$*" >> "$LOG_FILE" 2>/dev/null || true; }
log "event=$EVENT source=$SOURCE shell_pid=$SHELL_PID basename=$BASENAME"
log "raw_payload=$PAYLOAD"

# PreToolUse for AskUserQuestion: route through overlay synchronously.
# Bash blocks waiting for the user's choice, then emits the PreToolUse
# response that injects the answer as the tool's result.
if [[ "$EVENT" == "PreToolUse" ]]; then
  TOOL=$(jq -r '.tool_name // empty' <<<"$PAYLOAD")
  if [[ "$TOOL" != "AskUserQuestion" ]]; then exit 0; fi
  QUESTION=$(jq -r '.tool_input.question // empty' <<<"$PAYLOAD")
  # AskUserQuestion options can be either ["A","B"] or [{"label":"A"}, …].
  OPTIONS_JSON=$(jq -c '[(.tool_input.options // [])[] | if type == "string" then . else (.label // empty) end] | map(select(length > 0))' <<<"$PAYLOAD")
  if [[ -z "$OPTIONS_JSON" || "$OPTIONS_JSON" == "[]" ]]; then exit 0; fi

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
    --argjson options "$OPTIONS_JSON" \
    '{event: $event, cwd: $cwd, message: $message, source_type: $source_type, source_basename: $source_basename, wt_session: $wt_session, vscode_ipc_hook: $vscode_ipc_hook, vscode_pid: $vscode_pid, shell_pid: $shell_pid, timestamp_ms: $timestamp_ms, options: $options}')

  RESP=$(echo "$ASK_PAYLOAD" | claude-overlay.exe --stdin-ask 2>/dev/null || true)
  ANSWER=$(jq -r '.answer // empty' <<<"$RESP" 2>/dev/null || echo "")
  if [[ -n "$ANSWER" ]]; then
    jq -nc --arg r "$ANSWER" '{decision:"block",reason:$r}'
  fi
  exit 0
fi

if [[ "$EVENT" == "Stop" && -z "$MESSAGE" ]]; then
  LAST=$(jq -r '.last_assistant_message // empty' <<<"$PAYLOAD" | tr -d '\n' | cut -c1-80)
  if [[ -n "$LAST" ]]; then
    MESSAGE="Done › $LAST"
  else
    MESSAGE="Done"
  fi
fi

ENRICHED=$(jq -nc \
  --arg event "$EVENT" \
  --arg cwd "$CWD" \
  --arg message "$MESSAGE" \
  --arg notification_type "$NOTIFICATION_TYPE" \
  --arg source_type "$SOURCE" \
  --arg source_basename "$BASENAME" \
  --arg wt_session "${WT_SESSION:-}" \
  --arg vscode_ipc_hook "${VSCODE_IPC_HOOK_CLI:-}" \
  --arg vscode_pid "${VSCODE_PID:-}" \
  --argjson shell_pid "${SHELL_PID:-0}" \
  --argjson timestamp_ms "$(date +%s%3N)" \
  '{event: $event, cwd: $cwd, message: $message, notification_type: $notification_type, source_type: $source_type, source_basename: $source_basename, wt_session: $wt_session, vscode_ipc_hook: $vscode_ipc_hook, vscode_pid: $vscode_pid, shell_pid: $shell_pid, timestamp_ms: $timestamp_ms}')

(echo "$ENRICHED" | claude-overlay.exe --stdin) &
disown

(powershell.exe -c "[System.Media.SystemSounds]::Beep.Play()" >/dev/null 2>&1) &
disown

exit 0
