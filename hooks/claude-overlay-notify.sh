#!/usr/bin/env bash
set -euo pipefail

PAYLOAD=$(cat)

CWD=$(jq -r '.cwd // empty' <<<"$PAYLOAD")
BASENAME=$(basename "$CWD")
EVENT=$(jq -r '.hook_event_name // empty' <<<"$PAYLOAD")
MESSAGE=$(jq -r '.message // empty' <<<"$PAYLOAD")

if [[ "${TERM_PROGRAM:-}" == "vscode" ]]; then
  SOURCE="vscode"
elif [[ -n "${WT_SESSION:-}" ]]; then
  SOURCE="wt"
else
  SOURCE="unknown"
fi

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
    --argjson timestamp_ms "$(date +%s%3N)" \
    --argjson options "$OPTIONS_JSON" \
    '{event: $event, cwd: $cwd, message: $message, source_type: $source_type, source_basename: $source_basename, wt_session: $wt_session, vscode_ipc_hook: $vscode_ipc_hook, vscode_pid: $vscode_pid, timestamp_ms: $timestamp_ms, options: $options}')

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
