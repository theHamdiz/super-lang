#!/usr/bin/env bash
# Build & install the Super Zed extension as a Zed *dev extension*.
#
# Why `dist/` (not a symlink to editors/zed itself):
#   Symlinking `installed/super` → the crate root pulls in Cargo.toml + target/.
#   Zed's extension watcher then sees every `cargo build` / WASM write as an
#   extension change, unloads Super mid-deploy, and often leaves LSP dead until
#   a full Zed restart. Reload cannot recover because the unload race already
#   dropped the language server registration.
#
# This script:
#   1. Builds extension.wasm + tree-sitter grammar
#   2. Stages a *clean* runtime tree into editors/zed/dist/ (no Cargo/target)
#   3. Symlinks installed/super → dist/ once; later deploys only update files
#      inside dist/ (no unlink) so Zed hot-reloads without unloading Super
#   4. Bumps extension.toml version every deploy so the index entry changes
#
# Env overrides:
#   ZED_DATA_DIR   — force Zed user data root (contains extensions/)
#   ZED_CONFIG_DIR — force Zed config root (contains settings.json)
#   SUPER_BIN      — path to super binary for LSP settings
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$ROOT/../.." && pwd)"
GRAM_SRC="$REPO/editors/tree-sitter-super"
DIST="$ROOT/dist"

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

# --- discover Zed CLI --------------------------------------------------------

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
elif [[ -x /Applications/Zed.app/Contents/MacOS/cli ]]; then
  ZED_CLI=/Applications/Zed.app/Contents/MacOS/cli
fi

# --- discover Zed data / config dirs -----------------------------------------

candidate_data_dirs() {
  [[ -n "${ZED_DATA_DIR:-}" ]] && printf '%s\n' "$ZED_DATA_DIR"
  printf '%s\n' "${XDG_DATA_HOME:-$HOME/.local/share}/zed"
  printf '%s\n' "$HOME/.var/app/dev.zed.Zed/data/zed"
  printf '%s\n' "$HOME/.var/app/dev.zed.Zed-Preview/data/zed"
  printf '%s\n' "$HOME/snap/zed/common/.local/share/zed"
  printf '%s\n' "$HOME/snap/zed/current/.local/share/zed"
  printf '%s\n' "$HOME/Library/Application Support/Zed"
  printf '%s\n' "$HOME/.zed"
}

candidate_config_dirs() {
  [[ -n "${ZED_CONFIG_DIR:-}" ]] && printf '%s\n' "$ZED_CONFIG_DIR"
  printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/zed"
  printf '%s\n' "$HOME/.var/app/dev.zed.Zed/config/zed"
  printf '%s\n' "$HOME/.var/app/dev.zed.Zed-Preview/config/zed"
  printf '%s\n' "$HOME/snap/zed/common/.config/zed"
  printf '%s\n' "$HOME/Library/Application Support/Zed"
}

pick_existing_dir() {
  local d
  while IFS= read -r d; do
    [[ -z "$d" ]] && continue
    if [[ -d "$d" ]]; then
      printf '%s\n' "$d"
      return 0
    fi
  done
  return 1
}

ZED_DATA=""
while IFS= read -r d; do
  [[ -z "$d" ]] && continue
  if [[ -d "$d/extensions" ]]; then
    ZED_DATA="$d"
    break
  fi
done < <(candidate_data_dirs)

if [[ -z "$ZED_DATA" ]]; then
  ZED_DATA="$(candidate_data_dirs | pick_existing_dir || true)"
fi
if [[ -z "$ZED_DATA" ]]; then
  ZED_DATA="${XDG_DATA_HOME:-$HOME/.local/share}/zed"
fi

ZED_CONFIG=""
while IFS= read -r d; do
  [[ -z "$d" ]] && continue
  if [[ -f "$d/settings.json" ]]; then
    ZED_CONFIG="$d"
    break
  fi
done < <(candidate_config_dirs)
if [[ -z "$ZED_CONFIG" ]]; then
  ZED_CONFIG="$(candidate_config_dirs | pick_existing_dir || true)"
fi
if [[ -z "$ZED_CONFIG" ]]; then
  ZED_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/zed"
fi

INSTALL_DIR="$ZED_DATA/extensions/installed/super"
SETTINGS="$ZED_CONFIG/settings.json"

echo "==> Zed discovery"
echo "    CLI:    ${ZED_CLI:-"(not found — install still proceeds)"}"
echo "    data:   $ZED_DATA"
echo "    config: $ZED_CONFIG"

# --- build -------------------------------------------------------------------

echo "==> Building extension WASM (wasm32-wasip2)"
rustup target add wasm32-wasip2 >/dev/null
(cd "$ROOT" && cargo build --release --target wasm32-wasip2)

echo "==> Building tree-sitter grammar WASM"
GRAM_WASM="$GRAM_SRC/tree-sitter-super.wasm"
if ! command -v tree-sitter >/dev/null 2>&1; then
  if [[ -f "$GRAM_WASM" ]]; then
    echo "    warning: tree-sitter CLI missing — reusing existing $GRAM_WASM"
  else
    echo "error: tree-sitter CLI not on PATH (cargo install tree-sitter-cli)" >&2
    exit 1
  fi
else
  if ! (cd "$GRAM_SRC" && tree-sitter generate && tree-sitter build --wasm -o tree-sitter-super.wasm .); then
    if [[ -f "$GRAM_WASM" ]]; then
      echo "    warning: grammar regenerate failed — reusing existing $GRAM_WASM"
    else
      echo "error: failed to build tree-sitter-super.wasm" >&2
      exit 1
    fi
  fi
fi

REV="$(git -C "$GRAM_SRC" rev-parse HEAD 2>/dev/null || echo unknown)"

WASM_SRC="$ROOT/target/wasm32-wasip2/release/zed_super.wasm"
if [[ ! -f "$WASM_SRC" ]]; then
  WASM_SRC="$(find "$ROOT/target/wasm32-wasip2/release" -maxdepth 1 -name '*.wasm' | head -1)"
fi
[[ -f "$WASM_SRC" ]] || { echo "error: extension wasm not found under target/wasm32-wasip2/release" >&2; exit 1; }

# --- stage clean runtime tree into dist/ -------------------------------------
#
# Only runtime files. No Cargo.toml / src / target — those make Zed treat every
# local build as an extension change and unload Super.

echo "==> Staging clean runtime → $DIST"
mkdir -p "$ZED_DATA/extensions/installed" "$ZED_DATA/extensions/work" "$ZED_DATA/extensions/staging"
mkdir -p "$DIST/grammars" "$DIST/languages/super" "$DIST/icons" "$DIST/icon_themes"

# Drop rename-era leftovers and empty/stale work dirs that pin old WASM or
# confuse reloads. Always purge — Zed recreates work/super when needed.
rm -rf "$ZED_DATA/extensions/installed/syr" "$ZED_DATA/extensions/work/syr"
rm -rf "$ZED_DATA/extensions/work/super"

# Never leave a non-git grammars/super/ dir — Zed rebuild treats that as fatal.
rm -rf "$DIST/grammars/super" "$ROOT/grammars/super"

# Atomic file replaces inside dist/ (directory stays put → symlink stays valid).
cp "$WASM_SRC" "$DIST/extension.wasm.tmp"
mv -f "$DIST/extension.wasm.tmp" "$DIST/extension.wasm"
cp "$GRAM_WASM" "$DIST/grammars/super.wasm.tmp"
mv -f "$DIST/grammars/super.wasm.tmp" "$DIST/grammars/super.wasm"

# Language queries + config (highlights live here — not inside the grammar wasm).
rsync -a --delete \
  --exclude '.git' \
  "$ROOT/languages/super/" "$DIST/languages/super/"

if [[ -d "$ROOT/icons" ]]; then
  rsync -a --delete "$ROOT/icons/" "$DIST/icons/"
fi
if [[ -d "$ROOT/icon_themes" ]]; then
  rsync -a --delete "$ROOT/icon_themes/" "$DIST/icon_themes/"
fi

# Also keep a copy at the crate root for local inspection / older docs.
cp -f "$DIST/extension.wasm" "$ROOT/extension.wasm"
mkdir -p "$ROOT/grammars"
cp -f "$DIST/grammars/super.wasm" "$ROOT/grammars/super.wasm"

# Content-hash versioning: only bump the manifest version (and force a Zed
# reload) when runtime bytes actually change. Identical reinstalls must not
# stop super-lsp.
CONTENT_HASH="$(
  {
    sha256sum "$DIST/extension.wasm" "$DIST/grammars/super.wasm"
    # Stable ordering for language/query/icon files.
    find "$DIST/languages" "$DIST/icons" "$DIST/icon_themes" -type f 2>/dev/null \
      | sort | xargs -r sha256sum
    printf 'rev:%s\n' "$REV"
  } | sha256sum | awk '{print $1}'
)"
PREV_HASH=""
if [[ -f "$DIST/.content-hash" ]]; then
  PREV_HASH="$(tr -d '[:space:]' < "$DIST/.content-hash" || true)"
fi
FORCE_RELOAD=0
if [[ "$CONTENT_HASH" != "$PREV_HASH" ]]; then
  FORCE_RELOAD=1
fi

if [[ "$FORCE_RELOAD" -eq 1 ]]; then
  DEPLOY_VERSION="0.1.$(date +%s)"
else
  # Keep the previous manifest version so Zed's watcher sees no change.
  if [[ -f "$DIST/extension.toml" ]]; then
    DEPLOY_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$DIST/extension.toml" | head -1)"
  fi
  if [[ -z "${DEPLOY_VERSION:-}" ]]; then
    DEPLOY_VERSION="0.1.$(date +%s)"
    FORCE_RELOAD=1
  fi
fi
printf '%s\n' "$CONTENT_HASH" > "$DIST/.content-hash"

cat > "$DIST/extension.toml" <<TOML
id = "super"
name = "Super"
version = "$DEPLOY_VERSION"
schema_version = 1
description = "Super language support — Tree-sitter highlighting, brackets, outline, super-lsp, and .sr file icons"
repository = "https://github.com/theHamdiz/super-lang"
authors = ["Super Contributors"]
themes = []
icon_themes = ["icon_themes/super-icons.json"]
languages = ["languages/super"]
capabilities = []

[lib]
kind = "Rust"
version = "0.7.0"

# Prebuilt grammars/super.wasm ships in dist/. The repository/rev metadata is
# for Zed's optional rebuild path; day-to-day reload uses the wasm on disk.
[grammars.super]
repository = "file://$GRAM_SRC"
rev = "$REV"

[language_servers.super-lsp]
language = "Super"
languages = ["Super"]

[language_servers.super-lsp.language_ids]

[context_servers]

[agent_servers]

[slash_commands]
TOML

# Mirror extension.toml into the crate root so editors opening it see the same version.
cp -f "$DIST/extension.toml" "$ROOT/extension.toml"
echo "    content-hash: ${CONTENT_HASH:0:12}… (force_reload=$FORCE_RELOAD version=$DEPLOY_VERSION)"

python3 - "$DIST" <<'PY'
import re, sys
from pathlib import Path

dist = Path(sys.argv[1])
ext = (dist / "extension.toml").read_text()
keys = re.findall(r"^\[grammars\.([^\]]+)\]", ext, flags=re.M)
if keys != ["super"]:
    raise SystemExit(f"drift: extension.toml grammar keys={keys!r}, expected ['super']")

cfg = (dist / "languages" / "super" / "config.toml").read_text()
m = re.search(r'^grammar\s*=\s*"([^"]+)"', cfg, flags=re.M)
if not m or m.group(1) != "super":
    raise SystemExit(f"drift: languages/super/config.toml grammar={m.group(1) if m else None!r}, expected 'super'")

for req in (
    dist / "extension.wasm",
    dist / "grammars" / "super.wasm",
    dist / "languages" / "super" / "highlights.scm",
    dist / "languages" / "super" / "config.toml",
):
    if not req.is_file():
        raise SystemExit(f"drift: missing {req}")

# Hard fail if we accidentally staged build inputs into dist/.
for banned in ("Cargo.toml", "Cargo.lock", "src", "target", "install.sh"):
    if (dist / banned).exists():
        raise SystemExit(f"drift: dist must not contain {banned!r} (Zed watches it and unloads Super)")

print("    dist OK: wasm + grammar + languages; no Cargo/target")
PY

# --- symlink installed/super → dist/ (create once; never unlink on redeploy) -

echo "==> Ensuring installed/super → $DIST (dev extension)"
mkdir -p "$(dirname "$INSTALL_DIR")"

want="$(readlink -f "$DIST")"
have=""
if [[ -L "$INSTALL_DIR" ]]; then
  have="$(readlink -f "$INSTALL_DIR" 2>/dev/null || true)"
fi

if [[ -L "$INSTALL_DIR" && "$have" == "$want" ]]; then
  echo "    symlink already correct — updating files in place (no unload)"
elif [[ -e "$INSTALL_DIR" || -L "$INSTALL_DIR" ]]; then
  # Wrong target or a leftover copy-install directory.
  echo "    replacing stale install at $INSTALL_DIR"
  LINK_STAGE="$ZED_DATA/extensions/staging/super-link-$$"
  rm -rf "$LINK_STAGE"
  ln -s "$DIST" "$LINK_STAGE"
  rm -rf "$INSTALL_DIR"
  mv "$LINK_STAGE" "$INSTALL_DIR"
else
  ln -s "$DIST" "$INSTALL_DIR"
fi

if [[ ! -L "$INSTALL_DIR" ]]; then
  echo "error: $INSTALL_DIR is not a symlink — Zed will not treat Super as a dev extension" >&2
  exit 1
fi
if [[ "$(readlink -f "$INSTALL_DIR")" != "$want" ]]; then
  echo "error: symlink target mismatch: $(readlink -f "$INSTALL_DIR") != $want" >&2
  exit 1
fi
echo "    symlink OK: $INSTALL_DIR → $(readlink "$INSTALL_DIR")"
echo "    version:    $DEPLOY_VERSION"

# Do NOT hand-write extensions/index.json. Zed rebuilds it from disk and sets
# `dev: true` only when installed/super is a symlink.

# --- wire settings.json for LSP ---------------------------------------------

SUPER_BIN="${SUPER_BIN:-}"
if [[ -z "$SUPER_BIN" ]]; then
  # Prefer the installer home (~/.super). Never fall back to cargo/local
  # shims — those are removed; the only supported install is via install.sh.
  if [[ -x "${SUPER_HOME:-$HOME/.super}/bin/super" ]]; then
    SUPER_BIN="${SUPER_HOME:-$HOME/.super}/bin/super"
  elif command -v super >/dev/null 2>&1; then
    SUPER_BIN="$(command -v super)"
  elif [[ -x "$REPO/target/release/super" ]]; then
    SUPER_BIN="$REPO/target/release/super"
  fi
fi

if [[ -n "$SUPER_BIN" ]]; then
  mkdir -p "$ZED_CONFIG"
  python3 - "$SETTINGS" "$SUPER_BIN" <<'PY'
import json, re, sys
from pathlib import Path

settings_path = Path(sys.argv[1])
super_bin = sys.argv[2]

def load(text: str):
    try:
        return "", json.loads(text)
    except json.JSONDecodeError:
        # Strip // and /* */ comments Zed allows in settings.json.
        no_block = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
        no_line = re.sub(r"(?m)//.*?$", "", no_block)
        return text[: text.find("{")] if "{" in text else "", json.loads(no_line)

raw = settings_path.read_text() if settings_path.is_file() else "{}\n"
header, data = load(raw)
if not isinstance(data, dict):
    data = {}

lsp = data.setdefault("lsp", {})
super_lsp = lsp.setdefault("super-lsp", {})
binary = super_lsp.setdefault("binary", {})
binary["path"] = super_bin
binary["arguments"] = ["lsp"]

langs = data.setdefault("languages", {})
if "Syr" in langs:
    del langs["Syr"]
    print("    removed stale languages.Syr")
super_lang = langs.setdefault("Super", {})
super_lang["language_servers"] = ["super-lsp"]
settings_path.write_text(header + json.dumps(data, indent=2) + "\n")
print(f"    wired lsp.super-lsp → {super_bin} in {settings_path}")
print(f"    icon_theme left as: {data.get('icon_theme', '(unset)')}")
PY
else
  echo "    note: super binary not found — skip LSP settings (run make install first)"
fi

# Trigger Zed's extension watcher only when runtime content actually changed.
#
# Watching through a symlink often misses in-place writes under dist/.
# `ln -sfn` replaces the symlink inode so inotify fires a reload — but Zed
# then STOPS super-lsp and, on Linux, will not respawn it for already-open
# buffers (`activate is not implemented`). Do NOT pkill super-lsp either.
#
# Identical reinstalls skip this path so a live LSP keeps running.
RELOADED=0
if [[ "$FORCE_RELOAD" -eq 1 ]]; then
  echo "==> Content changed — refreshing installed/super symlink (Zed reload)"
  touch "$DIST/extension.toml" "$DIST/extension.wasm" "$DIST/grammars/super.wasm" 2>/dev/null || true
  ln -sfn "$DIST" "$INSTALL_DIR"
  if [[ ! -L "$INSTALL_DIR" ]] || [[ "$(readlink -f "$INSTALL_DIR")" != "$(readlink -f "$DIST")" ]]; then
    echo "error: failed to refresh symlink at $INSTALL_DIR" >&2
    exit 1
  fi
  RELOADED=1
else
  echo "==> Content unchanged — skip symlink refresh (keep live super-lsp)"
fi

# Confirm index (only expect a version bump when we forced a reload).
INDEX="$ZED_DATA/extensions/index.json"
python3 - "$INDEX" "$DEPLOY_VERSION" "$FORCE_RELOAD" <<'PY'
import json, sys, time
from pathlib import Path

idx_path = Path(sys.argv[1])
want_ver = sys.argv[2]
force = sys.argv[3] == "1"
for _ in range(40):
    if not idx_path.is_file():
        time.sleep(0.1)
        continue
    try:
        idx = json.loads(idx_path.read_text())
    except json.JSONDecodeError:
        time.sleep(0.1)
        continue
    ext = (idx.get("extensions") or {}).get("super") or {}
    man = ext.get("manifest") or {}
    ver = man.get("version")
    dev = ext.get("dev")
    if dev is True and (not force or ver == want_ver):
        print(f"    index OK: version={ver} dev=true")
        break
    time.sleep(0.15)
else:
    print("    note: index.json not yet refreshed — run 'zed: reload extensions' then this script's LSP heal")
PY

# --- heal super-lsp after extension reload ----------------------------------
#
# Zed stops extension language servers on reload and does not restart them for
# existing buffers on Linux. `zeditor -a`/`-e` only log
# "activate is not implemented". `zeditor --reuse <repo>` replaces the
# workspace in the existing window and starts super-lsp with the correct cwd.
#
# Also heal when LSP is already dead even if this install skipped reload
# (e.g. user ran "zed: reload extensions" earlier).

zed_is_running() {
  pgrep -x zed-editor >/dev/null 2>&1 || pgrep -f '/zed-editor( |$)' >/dev/null 2>&1
}

super_lsp_running() {
  pgrep -f '[/]super lsp' >/dev/null 2>&1 || pgrep -f '[/]\.super/bin/super lsp' >/dev/null 2>&1
}

# Pick a .sr file inside the repo so --reuse opens Super language (directory
# alone can restore a workspace with zero buffers → no LSP start).
pick_nudge_sr() {
  local c
  for c in \
    "$REPO/stdlib/mod.sr" \
    "$REPO/examples/02_task_api/src/main.sr" \
    "$REPO/examples/01_space_shooter/main.sr" \
    "$REPO/examples/03_log_analyzer/main.sr"
  do
    if [[ -f "$c" ]]; then
      printf '%s\n' "$c"
      return 0
    fi
  done
  return 1
}

heal_super_lsp() {
  local reason="$1"
  if [[ -z "${ZED_CLI:-}" ]]; then
    echo "    note: Zed CLI not found — cannot auto-heal LSP ($reason)"
    return 0
  fi
  if ! zed_is_running; then
    echo "    note: Zed not running — LSP will start on next open ($reason)"
    return 0
  fi

  local nudge=""
  nudge="$(pick_nudge_sr || true)"
  if [[ -z "$nudge" ]]; then
    echo "    warning: no .sr nudge file found under $REPO — cannot auto-heal LSP" >&2
    return 0
  fi

  # Settle so Zed finishes registering Super + super-lsp after reload.
  # Healing too early replaces the workspace before adapters are ready.
  local log="${ZED_DATA}/logs/Zed.log"
  sleep 0.9

  local nudge_label
  nudge_label="$(basename "$(dirname "$nudge")")/$(basename "$nudge")"
  echo "==> Healing super-lsp ($reason) via: $(basename "$ZED_CLI") --reuse <repo> <$nudge_label>"
  local before=0
  if [[ -f "$log" ]]; then
    before="$(wc -l < "$log")"
  fi
  # Repo first (correct worktree cwd), then a .sr so Super language activates.
  # On Linux, -a/-e only log "activate is not implemented" and never start LSP.
  "$ZED_CLI" --reuse "$REPO" "$nudge" >/dev/null 2>&1 || true

  local i
  for i in $(seq 1 50); do
    if super_lsp_running; then
      echo "    super-lsp alive (pid $(pgrep -f '[/]super lsp' | head -1))"
      return 0
    fi
    if [[ -f "$log" ]] && tail -n +"$((before + 1))" "$log" 2>/dev/null \
      | grep -q 'starting language server process'; then
      sleep 0.4
      if super_lsp_running; then
        echo "    super-lsp alive (pid $(pgrep -f '[/]super lsp' | head -1))"
        return 0
      fi
    fi
    sleep 0.15
  done
  echo "    warning: super-lsp did not respawn — run editors/zed/respawn-lsp.sh or 'editor: restart language server'" >&2
  return 0
}

NEED_HEAL=0
if [[ "$RELOADED" -eq 1 ]]; then
  NEED_HEAL=1
elif zed_is_running && ! super_lsp_running; then
  NEED_HEAL=1
fi

if [[ "$NEED_HEAL" -eq 1 ]]; then
  if [[ "$RELOADED" -eq 1 ]]; then
    heal_super_lsp "post-reload"
  else
    heal_super_lsp "LSP was dead"
  fi
else
  if super_lsp_running; then
    echo "    super-lsp already running — left untouched"
  fi
fi

echo ""
echo "==> Super Zed extension installed as DEV symlink → dist/"
echo "    extension: $INSTALL_DIR -> $(readlink "$INSTALL_DIR")"
echo "    version:   $DEPLOY_VERSION"
echo "    grammar:   $DIST/grammars/super.wasm"
echo "    Hot reload (no full Zed restart):"
echo "      • make install-zed   — updates dist/, reloads only when content changes,"
echo "                            then heals super-lsp with 'zeditor --reuse'"
echo "      • after manual 'zed: reload extensions' → run: editors/zed/respawn-lsp.sh"
echo "      • or command palette → 'editor: restart language server' (on a .sr buffer)"
echo "      • do NOT use 'zed: rebuild dev extension' — use make install-zed instead"
