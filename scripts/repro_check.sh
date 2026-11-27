#!/usr/bin/env bash
# Build the same release artifact twice in isolated trees and compare SHA256.
# Gate 4: identical digests → PASS for the claimed target.
#
# Usage:
#   scripts/repro_check.sh [--target TRIPLE] [--keep]
# Env:
#   SUPER_VERSION (default 0.1.0)
#   SOURCE_DATE_EPOCH (default: tip commit timestamp)
#   SUPER_REPRO_TARGET (default: host triple)
set -euo pipefail

export LC_ALL=C
export LANG=C
export TZ=UTC

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${SUPER_VERSION:-0.1.0}"
TARGET="${SUPER_REPRO_TARGET:-${SUPER_TARGET:-$(rustc -vV | awk '/^host:/{print $2}')}}"
KEEP=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) TARGET="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -h|--help)
      sed -n '1,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct)}"
export SOURCE_DATE_EPOCH SUPER_VERSION="$VERSION" SUPER_TARGET="$TARGET"

# Real rustup install (shared); only CARGO_HOME / tree paths differ.
HOST_HOME="${REAL_HOME:-$HOME}"
_sys="$(rustc --print sysroot)"
RUSTUP_HOME_REAL="${_sys%%/toolchains/*}"
export RUSTUP_HOME="$RUSTUP_HOME_REAL"

WORKDIR="${SUPER_REPRO_WORKDIR:-$(mktemp -d /tmp/super-repro-XXXXXX)}"
A="$WORKDIR/build-alice"
B="$WORKDIR/build-bob"
mkdir -p "$A" "$B"

echo "==> repro_check target=${TARGET} epoch=${SOURCE_DATE_EPOCH} work=${WORKDIR}"

clone_tree() {
  local dest="$1"
  # Snapshot the working tree (includes uncommitted packaging fixes) minus build junk.
  rsync -a \
    --exclude '.git/' \
    --exclude 'target/' \
    --exclude 'dist/' \
    --exclude 'benchmarks-memory/' \
    --exclude '*.check.c' \
    "$ROOT/" "$dest/"
  chmod +x "$dest/scripts/"*.sh 2>/dev/null || true
}

clone_tree "$A"
clone_tree "$B"

HOME_A="$WORKDIR/home-alice"
HOME_B="$WORKDIR/home-bob"
mkdir -p "$HOME_A" "$HOME_B"

build_one() {
  local tree="$1" home="$2" label="$3"
  echo "==> build ${label} in ${tree}"
  (
    cd "$tree"
    export HOME="$home"
    export CARGO_HOME="$home/.cargo"
    export RUSTUP_HOME="$RUSTUP_HOME_REAL"
    mkdir -p "$CARGO_HOME"
    # Ensure cargo/rustc from host PATH remain visible.
    export PATH="${HOST_HOME}/.cargo/bin:/usr/local/cargo/bin:${PATH}"
    export SUPER_DIST="$tree/dist"
    export CARGO_TARGET_DIR="$tree/target"
    export CARGO_BUILD_JOBS=1
    ./scripts/package_release.sh
  )
}

build_one "$A" "$HOME_A" "A"
build_one "$B" "$HOME_B" "B"

ARCH_A="$A/dist/super-${VERSION}-${TARGET}.tar.gz"
ARCH_B="$B/dist/super-${VERSION}-${TARGET}.tar.gz"
SHA_A="$(sha256sum "$ARCH_A" | awk '{print $1}')"
SHA_B="$(sha256sum "$ARCH_B" | awk '{print $1}')"

echo
echo "target=${TARGET}"
echo "build_A=${SHA_A}"
echo "build_B=${SHA_B}"

OUT_DIR="${SUPER_REPRO_OUT:-$ROOT/docs}"
mkdir -p "$OUT_DIR"
REPORT="$OUT_DIR/repro-${TARGET}.txt"
{
  echo "target=${TARGET}"
  echo "source_date_epoch=${SOURCE_DATE_EPOCH}"
  echo "version=${VERSION}"
  echo "build_A=${SHA_A}"
  echo "build_B=${SHA_B}"
  if [[ "$SHA_A" == "$SHA_B" ]]; then
    echo "result=PASS"
  else
    echo "result=FAIL"
  fi
} | tee "$REPORT"

if [[ "$SHA_A" != "$SHA_B" ]]; then
  echo "==> digests differ; member-level diff" >&2
  mkdir -p "$WORKDIR/diff"
  tar -C "$WORKDIR/diff" -xzf "$ARCH_A"
  mv "$WORKDIR/diff/super-${VERSION}-${TARGET}" "$WORKDIR/diff/A"
  tar -C "$WORKDIR/diff" -xzf "$ARCH_B"
  mv "$WORKDIR/diff/super-${VERSION}-${TARGET}" "$WORKDIR/diff/B"
  if command -v diffoscope >/dev/null 2>&1; then
    diffoscope "$WORKDIR/diff/A" "$WORKDIR/diff/B" | head -200 || true
  else
    diff -ruN "$WORKDIR/diff/A" "$WORKDIR/diff/B" | head -200 || true
    if ! cmp -s "$WORKDIR/diff/A/bin/super" "$WORKDIR/diff/B/bin/super"; then
      echo "bin/super differs"
      sha256sum "$WORKDIR/diff/A/bin/super" "$WORKDIR/diff/B/bin/super"
    fi
  fi
  [[ "$KEEP" == 1 ]] || rm -rf "$WORKDIR"
  exit 1
fi

echo "PASS: byte-identical ${TARGET}"
[[ "$KEEP" == 1 ]] || rm -rf "$WORKDIR"
exit 0
