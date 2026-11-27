#!/usr/bin/env bash
# Installer security regression: corrupt archives must abort before extraction.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
for script in install.sh install.ps1; do
  cmp -s "$ROOT/$script" "$ROOT/docs/$script" || {
    echo "docs/$script is out of sync with $script"
    exit 1
  }
done
# shellcheck disable=SC2016 # the sed pattern matches a literal `${SUPER_VERSION:-…}`
VERSION="$(sed -n 's/^SUPER_VERSION="${SUPER_VERSION:-\(.*\)}"$/\1/p' "$ROOT/install.sh")"
[ -n "$VERSION" ] || { echo "install.sh has no default SUPER_VERSION"; exit 1; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/home"

cat >"$WORK/bin/uname" <<'SH'
#!/bin/sh
case "${1:-}" in
  -s) echo Linux ;;
  -m) echo x86_64 ;;
  *) echo Linux ;;
esac
SH

cat >"$WORK/bin/curl" <<SH
#!/bin/sh
dest=
url=
while [ "\$#" -gt 0 ]; do
  case "\$1" in
    -o) dest=\$2; shift 2 ;;
    http*) url=\$1; shift ;;
    *) shift ;;
  esac
done
[ -n "\$dest" ] || exit 2
case "\$url" in
  */SHA256SUMS)
    printf '%064d  super-$VERSION-x86_64-unknown-linux-gnu.tar.gz\\n' 0 >"\$dest"
    ;;
  *)
    printf 'deliberately corrupt archive\\n' >"\$dest"
    ;;
esac
SH
chmod +x "$WORK/bin/uname" "$WORK/bin/curl"

set +e
output="$(
  PATH="$WORK/bin:$PATH" \
  HOME="$WORK/home" \
  SUPER_HOME="$WORK/home/.super" \
  SUPER_BOOTSTRAP_BASE_URL="https://example.invalid" \
  sh "$ROOT/install.sh" 2>&1
)"
status=$?
set -e

if [ "$status" -eq 0 ]; then
  echo "installer accepted a corrupt archive"
  exit 1
fi
case "$output" in
  *"checksum mismatch"*) ;;
  *)
    printf '%s\n' "$output"
    echo "installer did not fail at checksum verification"
    exit 1
    ;;
esac
test ! -e "$WORK/home/.super/bin/super"
echo "OK: corrupt installer checksum rejected"
