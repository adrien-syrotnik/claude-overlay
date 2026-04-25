#!/usr/bin/env bash
set -euo pipefail

HOOK_SRC="hooks/claude-overlay-notify.sh"
HOOK_DST="$HOME/.claude/hooks/claude-overlay-notify.sh"

mkdir -p "$HOME/.claude/hooks"
cp "$HOOK_SRC" "$HOOK_DST"
chmod +x "$HOOK_DST"
echo "Installed hook: $HOOK_DST"

if ! command -v jq >/dev/null 2>&1; then
  echo "WARNING: jq not found. Install with: sudo apt install -y jq"
fi

echo
echo "Next: edit ~/.claude/settings.json to register the hook on 'Notification' and 'Stop' events."
echo "See docs/install.md for the JSON snippet."
