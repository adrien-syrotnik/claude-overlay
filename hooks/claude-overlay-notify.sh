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
    # AskUserQuestion-specific PreToolUse output: echo back the original
    # questions array and provide an `answers` map. Bypasses Claude Code's
    # native UI without rendering as a "blocking error". The legacy
    # decision:block path is deprecated for PreToolUse per docs.
    ORIG_QUESTIONS=$(jq -c '.tool_input.questions' <<<"$PAYLOAD")
    jq -nc \
      --arg q "$QUESTION" \
      --arg a "$ANSWER" \
      --argjson questions "$ORIG_QUESTIONS" \
      '{
        hookSpecificOutput: {
          hookEventName: "PreToolUse",
          updatedInput: {
            questions: $questions,
            answers: { ($q): $a }
          }
        }
      }'
  fi
  exit 0
fi

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
