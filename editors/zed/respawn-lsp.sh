#!/usr/bin/env bash
# Respawn super-lsp inside the already-open Zed window after an extension reload.
#
# Why this exists:
#   Zed stops extension language servers on "zed: reload extensions". On Linux
#   it does not restart them for already-open buffers (`activate` is a no-op;
#   `zeditor -a`/`-e` do nothing useful). `zeditor --reuse <repo> <file.sr>`
#   replaces the workspace in the existing window and starts super-lsp with the
#   correct working directory.
#
# Usage:
#   ./editors/zed/respawn-lsp.sh
#   SUPER_ZED_REPO=/path/to/other/project ./editors/zed/respawn-lsp.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${SUPER_ZED_REPO:-$(cd "$ROOT/../.." && pwd)}"

resolve_cmd() {
  local c
  for c in "$@"; do
    if command -v "$c" >/dev/null 2>&1; then
      command -v "$c"
      return 0
    fi
  done
  return 1
}

ZED_CLI=""
if ZED_CLI="$(resolve_cmd zeditor zed zed-editor 2>/dev/null)"; then
  :
elif [[ -x /usr/lib/zed/zed-editor ]]; then
  ZED_CLI=/usr/lib/zed/zed-editor
elif [[ -x /usr/bin/zeditor ]]; then
  ZED_CLI=/usr/bin/zeditor
fi

if [[ -z "$ZED_CLI" ]]; then
  echo "error: Zed CLI not found (zeditor / zed)" >&2
  exit 1
fi

NUDGE=""
for c in \
  "$REPO/stdlib/mod.sr" \
  "$REPO/examples/02_task_api/src/main.sr" \
  "$REPO/examples/01_space_shooter/main.sr" \
  "$REPO/examples/03_log_analyzer/main.sr"
do
  if [[ -f "$c" ]]; then
    NUDGE="$c"
    break
  fi
done

if [[ -z "$NUDGE" ]]; then
  echo "error: no .sr file found under $REPO to activate Super language" >&2
  exit 1
fi

if ! pgrep -x zed-editor >/dev/null 2>&1 && ! pgrep -f '/zed-editor( |$)' >/dev/null 2>&1; then
  echo "Zed is not running — opening $REPO"
  exec "$ZED_CLI" "$REPO" "$NUDGE"
fi

echo "Respawning super-lsp via: $(basename "$ZED_CLI") --reuse $REPO $(basename "$NUDGE")"
"$ZED_CLI" --reuse "$REPO" "$NUDGE" >/dev/null 2>&1 || true

for _ in $(seq 1 50); do
  if pgrep -f '[/]super lsp' >/dev/null 2>&1 || pgrep -f '[/]cargo/bin/super lsp' >/dev/null 2>&1; then
    echo "super-lsp alive (pid $(pgrep -f '[/]super lsp' | head -1))"
    exit 0
  fi
  sleep 0.15
done

echo "warning: super-lsp not detected — focus a .sr buffer and run 'editor: restart language server'" >&2
exit 1
