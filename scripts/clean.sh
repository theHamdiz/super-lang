#!/usr/bin/env bash
# Clean Super build artifacts.
#
# Usage:
#   ./scripts/clean.sh              # same as: all
#   ./scripts/clean.sh debug        # target/debug only
#   ./scripts/clean.sh release      # target/release only
#   ./scripts/clean.sh cargo        # cargo clean (workspace)
#   ./scripts/clean.sh zed          # editors/zed/target
#   ./scripts/clean.sh grammar      # tree-sitter generated/build bits
#   ./scripts/clean.sh all          # everything above
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODE="${1:-all}"

bytes_of() {
  if [[ -e "$1" ]]; then
    du -sh "$1" 2>/dev/null | awk '{print $1}'
  else
    echo "absent"
  fi
}

rm_tree() {
  local path="$1"
  if [[ -e "$path" ]]; then
    local size
    size="$(bytes_of "$path")"
    echo "  removing $path ($size)"
    rm -rf "$path"
  else
    echo "  skip $path (missing)"
  fi
}

clean_debug() {
  echo "==> clean debug"
  rm_tree "$ROOT/target/debug"
  # Incremental leftovers Cargo sometimes leaves beside profile dirs
  rm_tree "$ROOT/target/tmp"
}

clean_release() {
  echo "==> clean release"
  rm_tree "$ROOT/target/release"
}

clean_cargo() {
  echo "==> cargo clean (workspace)"
  if [[ -f "$ROOT/Cargo.toml" ]]; then
    cargo clean --manifest-path "$ROOT/Cargo.toml"
  fi
  # Ensure profile dirs are gone even if cargo left stubs / alternate targets
  rm_tree "$ROOT/target/debug"
  rm_tree "$ROOT/target/release"
  rm_tree "$ROOT/target/tmp"
  # Cross / wasm / incremental / fingerprint leftovers under target/
  if [[ -d "$ROOT/target" ]]; then
    find "$ROOT/target" -mindepth 1 -maxdepth 1 \
      ! -name '.rustc_info.json' \
      ! -name 'CACHEDIR.TAG' \
      -exec rm -rf {} +
    echo "  pruned $ROOT/target/*"
  fi
}

clean_zed() {
  echo "==> clean Zed extension target"
  rm_tree "$ROOT/editors/zed/target"
}

clean_grammar() {
  echo "==> clean tree-sitter build artifacts"
  local g="$ROOT/editors/tree-sitter-super"
  rm_tree "$g/build"
  rm_tree "$g/bindings"
  rm_tree "$g/node_modules"
  # Keep grammar.js / queries / committed src/ — only wipe generated wasm if present
  # as a rebuildable artifact (install.sh regenerates it).
  if [[ -f "$g/tree-sitter-super.wasm" ]]; then
    rm_tree "$g/tree-sitter-super.wasm"
  fi
}

case "$MODE" in
  debug)   clean_debug ;;
  release) clean_release ;;
  cargo)   clean_cargo ;;
  zed)     clean_zed ;;
  grammar) clean_grammar ;;
  all|everything|distclean)
    clean_cargo
    clean_zed
    clean_grammar
    echo "==> done (all)"
    ;;
  -h|--help|help)
    cat <<'USAGE'
Clean Super build artifacts.

Usage:
  ./scripts/clean.sh              # same as: all
  ./scripts/clean.sh debug        # target/debug only
  ./scripts/clean.sh release      # target/release only
  ./scripts/clean.sh cargo        # cargo clean (workspace) + prune target/
  ./scripts/clean.sh zed          # editors/zed/target
  ./scripts/clean.sh grammar      # tree-sitter build/wasm artifacts
  ./scripts/clean.sh all          # everything above

Makefile: make clean-debug | clean-release | clean | clean-all
USAGE
    exit 0
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    echo "use: debug | release | cargo | zed | grammar | all" >&2
    exit 1
    ;;
esac
