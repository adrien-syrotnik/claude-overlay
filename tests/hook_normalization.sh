#!/usr/bin/env bash
# Verifies that the hook produces correct input_spec JSON for known payload
# shapes. Intercepts the call to claude-overlay.exe by stubbing it.
set -euo pipefail

cd "$(dirname "$0")/.."

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Stub claude-overlay.exe to capture stdin
cat > "$TMPDIR/claude-overlay.exe" <<EOF
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

echo "All hook normalization tests passed."
