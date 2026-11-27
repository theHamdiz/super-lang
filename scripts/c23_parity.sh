#!/usr/bin/env bash
# Compare `super run` (VM) against the C23 AOT binary for each given .sr file.
# Compares the full stdout; exits non-zero on any divergence or build failure.
#
#   scripts/c23_parity.sh /tmp/bk/c40/main.sr /tmp/bk/c42/main.sr
#   scripts/c23_parity.sh examples/*/main.sr
#
# Override the binary with SUPER=path/to/super.
set -u
SUPER="${SUPER:-$(dirname "$0")/../target/release/super}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
[ "$#" -gt 0 ] || { echo "usage: $0 file.sr..." >&2; exit 2; }

fail=0
for src in "$@"; do
  name="$(echo "$src" | tr '/' '_')"
  vm="$(timeout "${AOT_TIMEOUT:-30}" "$SUPER" run "$src" 2>/dev/null)"
  if ! "$SUPER" build --release --c23 "$src" -o "$tmp/$name.bin" >"$tmp/$name.log" 2>&1; then
    echo "BUILD-FAIL $src"; fail=1; continue
  fi
  aot="$(timeout "${AOT_TIMEOUT:-30}" "$tmp/$name.bin" 2>/dev/null)"
  vm_ck="$(grep -a CHECKSUM <<<"$vm" | tail -1)"
  aot_ck="$(grep -a CHECKSUM <<<"$aot" | tail -1)"
  if [ "$vm" == "$aot" ]; then
    echo "MATCH      $src ${vm_ck}"
  else
    echo "MISMATCH   $src vm[${vm_ck}] c23[${aot_ck}]"; fail=1
  fi
done
exit "$fail"
