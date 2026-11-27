#!/usr/bin/env bash
# Assert release tarball names match install.sh detect_target() triples.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${SUPER_VERSION:-0.1.0}"
TARGET="${SUPER_TARGET:-$(rustc -vV | awk '/^host:/{print $2}')}"
OUT="$ROOT/dist"
EXT="tar.gz"
[[ "$TARGET" == *windows* ]] && EXT="zip"
ARCHIVE="$OUT/super-${VERSION}-${TARGET}.${EXT}"
HASH="$OUT/super-${VERSION}-${TARGET}.sha256"

[[ -f "$ARCHIVE" ]] || { echo "missing archive: $ARCHIVE"; exit 1; }
[[ -f "$HASH" ]] || { echo "missing sidecar hash: $HASH"; exit 1; }

# install.sh only accepts triples that ship on the v0.1.0 release.
case "$TARGET" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu \
  | x86_64-unknown-linux-musl | x86_64-unknown-freebsd \
  | x86_64-apple-darwin | aarch64-apple-darwin \
  | x86_64-pc-windows-gnu) ;;
  *)
    echo "unexpected SUPER_TARGET for installer: $TARGET"
    exit 1
    ;;
esac

if [[ "$EXT" == "zip" ]]; then
  manifest="$(unzip -Z1 "$ARCHIVE")"
else
  manifest="$(tar -tzf "$ARCHIVE")"
fi
if ! printf '%s\n' "$manifest" | grep -Eq '/bin/super(\.exe)?$'; then
  echo "archive layout missing bin/super at bundle root"
  printf '%s\n' "$manifest" | head
  exit 1
fi

echo "OK: super-${VERSION}-${TARGET}.${EXT}"