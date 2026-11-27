#!/usr/bin/env bash
# Install heavily optimized Super tooling + the standard library.
#
# Layout (outside ~/.cargo by default):
#   $SUPER_HOME/bin/super
#   $SUPER_HOME/bin/super-lsp
#   $SUPER_HOME/bin/super-mcp
#   $SUPER_HOME/lib/stdlib/   ← `use std.*` resolves here
#
# SUPER_HOME defaults to ~/.super (or $XDG_DATA_HOME/super when SUPER_USE_XDG=1).
# Legacy overrides: SUPER_PREFIX / SUPER_BIN_DIR / SUPER_SHARE_DIR / SUPER_STD.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -z "${SUPER_HOME:-}" ]]; then
  if [[ "${SUPER_USE_XDG:-0}" == "1" && -n "${XDG_DATA_HOME:-}" ]]; then
    SUPER_HOME="$XDG_DATA_HOME/super"
  else
    SUPER_HOME="${HOME:?HOME unset}/.super"
  fi
fi

# SUPER_PREFIX remains for callers that still pass a FHS-style root.
PREFIX="${SUPER_PREFIX:-$SUPER_HOME}"
BIN_DIR="${SUPER_BIN_DIR:-$PREFIX/bin}"
if [[ -n "${SUPER_SHARE_DIR:-}" ]]; then
  SHARE_DIR="$SUPER_SHARE_DIR"
elif [[ "$PREFIX" == "$SUPER_HOME" ]]; then
  SHARE_DIR="$SUPER_HOME/lib"
else
  SHARE_DIR="$PREFIX/share/super"
fi
STD_DIR="${SUPER_STD_INSTALL:-$SHARE_DIR/stdlib}"

cd "$ROOT"
mkdir -p "$BIN_DIR" "$STD_DIR" "$SUPER_HOME/registry" "$SUPER_HOME/cache"

PROFILE="${SUPER_PROFILE:-release}"
echo "==> Building super profile '$PROFILE'…"
# Prefer a direct release build into SUPER_HOME (avoids cargo's ~/.cargo root).
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release -p super-cli --locked || cargo build --release -p super-cli
  cp -f "$ROOT/target/release/super" "$BIN_DIR/super"
else
  cargo build --profile "$PROFILE" -p super-cli --locked || cargo build --profile "$PROFILE" -p super-cli
  cp -f "$ROOT/target/$PROFILE/super" "$BIN_DIR/super"
fi
chmod +x "$BIN_DIR/super"

cat > "$BIN_DIR/super-lsp" <<'EOF'
#!/usr/bin/env bash
exec super lsp "$@"
EOF
chmod +x "$BIN_DIR/super-lsp"

cat > "$BIN_DIR/super-mcp" <<'EOF'
#!/usr/bin/env bash
exec super mcp "$@"
EOF
chmod +x "$BIN_DIR/super-mcp"

echo "==> Installing stdlib → $STD_DIR"
rm -rf "$STD_DIR"
mkdir -p "$STD_DIR"
cp -a "$ROOT/stdlib/." "$STD_DIR/"
{
  echo "version=$(grep -E '^\+STDLIB_VERSION' "$STD_DIR/mod.sr" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
  echo "installed=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "home=$SUPER_HOME"
  echo "prefix=$PREFIX"
} > "$STD_DIR/.sr-stdlib-install"

# Persist PATH when ~/.super/bin is not already visible.
MARK_BEGIN="# >>> super path >>>"
MARK_END="# <<< super path <<<"
case ":${PATH:-}:" in
  *":$BIN_DIR:"*) ;;
  *)
    shell_name="$(basename "${SHELL:-/bin/sh}")"
    case "$shell_name" in
      zsh) profile="$HOME/.zshenv" ;;
      bash) profile="$HOME/.bashrc" ;;
      fish) profile="$HOME/.config/fish/config.fish" ;;
      *) profile="$HOME/.profile" ;;
    esac
    mkdir -p "$(dirname "$profile")" 2>/dev/null || true
    if [[ ! -f "$profile" ]] || ! grep -q "$MARK_BEGIN" "$profile" 2>/dev/null; then
      if [[ "$shell_name" == "fish" ]]; then
        printf '\n%s\nfish_add_path -g %s\n%s\n' "$MARK_BEGIN" "$BIN_DIR" "$MARK_END" >> "$profile"
      else
        printf '\n%s\ncase ":$PATH:" in\n  *":%s:"*) ;;\n  *) export PATH="%s:$PATH" ;;\nesac\n%s\n' \
          "$MARK_BEGIN" "$BIN_DIR" "$BIN_DIR" "$MARK_END" >> "$profile"
      fi
      echo "Added $BIN_DIR to PATH via $profile"
    fi
    export PATH="$BIN_DIR:$PATH"
    ;;
esac

echo ""
echo "Installed:"
echo "  $BIN_DIR/super"
echo "  $BIN_DIR/super-lsp"
echo "  $BIN_DIR/super-mcp"
echo "  $STD_DIR"
echo ""
echo "Resolution: SUPER_STD > project ./stdlib > \$SUPER_HOME/lib/stdlib > next-to-exe > embedded"
echo "Override:   export SUPER_HOME=$SUPER_HOME"
echo "            export SUPER_STD=$STD_DIR"
if ! command -v super >/dev/null 2>&1; then
  echo ""
  echo "NOTE: $BIN_DIR is not on PATH in this shell. Add:"
  echo "  export PATH=\"$BIN_DIR:\$PATH\""
fi
super --help | head -20 || true
