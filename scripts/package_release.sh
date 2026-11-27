#!/usr/bin/env bash
# Build a Super release bundle: bin/super + stdlib/ + SHA256SUMS entry.
# Used by .github/workflows/release-bundle.yml
#
# When SOURCE_DATE_EPOCH is set, the archive is built for bit-reproducibility:
# sorted tar members, fixed owner/mtime, gzip -n, remapped rustc/C paths.
set -euo pipefail

export LC_ALL=C
export LANG=C
export TZ=UTC

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${SUPER_VERSION:-0.1.0}"
TARGET="${SUPER_TARGET:-$(rustc -vV | awk '/^host:/{print $2}')}"
OUT_DIR="${SUPER_DIST:-$ROOT/dist}"
NAME="super-${VERSION}-${TARGET}"
STAGE="$OUT_DIR/$NAME"
EXE="super"
ARCHIVE_EXT="tar.gz"
if [[ "$TARGET" == *windows* ]]; then
  EXE="super.exe"
  ARCHIVE_EXT="zip"
fi

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  SOURCE_DATE_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || date -u +%s)"
  export SOURCE_DATE_EPOCH
fi

# Remap absolute paths out of the binary (host + crate roots).
REMAP_FLAGS=(
  "--remap-path-prefix=${ROOT}="
  "--remap-path-prefix=${HOME}="
  "--remap-path-prefix=/usr/local/cargo="
  "--remap-path-prefix=/usr/local/rustup="
  "--remap-path-prefix=/cargo="
  "--remap-path-prefix=/rustc="
)
# Cargo ignores CARGO_TARGET_<triple>_RUSTFLAGS whenever RUSTFLAGS is set
# (verified Cargo 1.99). Fold cross-linker/sysroot flags into RUSTFLAGS so
# FreeBSD/musl packaging does not silently drop --sysroot / -fuse-ld=lld.
HOST_TRIPLE="$(rustc -vV | awk '/^host:/{print $2}')"
TARGET_ENV="$(printf '%s' "$TARGET" | tr '[:lower:]-' '[:upper:]_')"
TARGET_RUSTFLAGS_VAR="CARGO_TARGET_${TARGET_ENV}_RUSTFLAGS"
TARGET_RUSTFLAGS=""
if [[ -n "${!TARGET_RUSTFLAGS_VAR:-}" ]]; then
  TARGET_RUSTFLAGS="${!TARGET_RUSTFLAGS_VAR}"
fi
export RUSTFLAGS="${TARGET_RUSTFLAGS} ${RUSTFLAGS:-} ${REMAP_FLAGS[*]}"
# Prevent a stale target-specific var from confusing later tooling.
unset "$TARGET_RUSTFLAGS_VAR" 2>/dev/null || true
export CFLAGS="${CFLAGS:--O2} -ffile-prefix-map=${ROOT}=. -ffile-prefix-map=${HOME}=."
export CXXFLAGS="${CXXFLAGS:-} -ffile-prefix-map=${ROOT}=. -ffile-prefix-map=${HOME}=."

cd "$ROOT"
mkdir -p "$STAGE/bin"
echo "==> Building super-cli ($TARGET) [SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}]…"
# Single-threaded codegen reduces nondeterminism from parallel LLVM.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
if [[ "$TARGET" == "$HOST_TRIPLE" ]]; then
  cargo build --release -p super-cli --locked
  cp -f "$ROOT/target/release/$EXE" "$STAGE/bin/$EXE"
else
  cargo build --release -p super-cli --locked --target "$TARGET"
  cp -f "$ROOT/target/$TARGET/release/$EXE" "$STAGE/bin/$EXE"
fi
[[ "$TARGET" == *windows* ]] || chmod +x "$STAGE/bin/$EXE"

echo "==> Copying stdlib…"
rm -rf "$STAGE/stdlib"
mkdir -p "$STAGE/stdlib"
# Deterministic copy (sorted, no preserved host mtimes in the tree walk).
(
  cd "$ROOT/stdlib"
  find . -type f -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' f; do
    mkdir -p "$STAGE/stdlib/$(dirname "$f")"
    cp -f "$f" "$STAGE/stdlib/$f"
  done
)
{
  echo "version=$VERSION"
  echo "target=$TARGET"
  echo "built=$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
    || date -u -r "${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
    || date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$STAGE/stdlib/.sr-stdlib-install"

COMMIT_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
RUSTC_VER="$(rustc -V 2>/dev/null || echo unknown)"
BUILD_DATE="$(date -u -d "@${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
  || date -u -r "${SOURCE_DATE_EPOCH}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
  || date -u +%Y-%m-%dT%H:%M:%SZ)"
cat > "$STAGE/build-info.txt" <<EOF
version=${VERSION}
target=${TARGET}
commit=${COMMIT_SHA}
rustc=${RUSTC_VER}
built_utc=${BUILD_DATE}
source_date_epoch=${SOURCE_DATE_EPOCH}
EOF

cat > "$STAGE/README.txt" <<EOF
Super ${VERSION} (${TARGET})

Layout:
  bin/$EXE
  stdlib/

Install with the official install.sh, or:
  mkdir -p ~/.super/bin ~/.super/lib
  cp bin/super ~/.super/bin/
  cp -a stdlib ~/.super/lib/
  export PATH="\$HOME/.super/bin:\$PATH"
  export SUPER_HOME="\$HOME/.super"
EOF

# Normalize stage file mtimes for archive determinism.
find "$STAGE" -exec touch -d "@${SOURCE_DATE_EPOCH}" {} + 2>/dev/null \
  || find "$STAGE" -exec touch -r /etc/hostname {} + 2>/dev/null \
  || true

ARCHIVE="$OUT_DIR/${NAME}.${ARCHIVE_EXT}"
rm -f "$ARCHIVE"
if [[ "$ARCHIVE_EXT" == "zip" ]]; then
  if command -v 7z >/dev/null 2>&1; then
    (cd "$OUT_DIR" && 7z a -bd -tzip "$ARCHIVE" "$NAME" >/dev/null)
  else
    powershell -NoProfile -Command \
      "Compress-Archive -LiteralPath '$STAGE' -DestinationPath '$ARCHIVE' -Force"
  fi
else
  # Sorted members, fixed owner, fixed mtime, gzip without timestamp.
  # BusyBox tar (Alpine default) lacks --sort=name; prefer GNU tar, else Python.
  if tar --version 2>&1 | grep -qi 'GNU tar'; then
    tar -C "$OUT_DIR" \
      --sort=name \
      --owner=0 --group=0 --numeric-owner \
      --mtime="@${SOURCE_DATE_EPOCH}" \
      -cf - "$NAME" | gzip -n -9 > "$ARCHIVE"
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "$OUT_DIR" "$NAME" "$ARCHIVE" "$SOURCE_DATE_EPOCH" <<'PY'
import gzip, os, sys, tarfile
out_dir, name, archive, epoch = sys.argv[1:5]
epoch = int(epoch)
root = os.path.join(out_dir, name)
with gzip.GzipFile(archive, "wb", mtime=0) as gz:
    with tarfile.open(fileobj=gz, mode="w") as tf:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames.sort()
            filenames.sort()
            rel_dir = os.path.relpath(dirpath, out_dir)
            # Directory entry
            info = tarfile.TarInfo(rel_dir.replace(os.sep, "/"))
            info.type = tarfile.DIRTYPE
            info.mtime = epoch
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mode = 0o755
            tf.addfile(info)
            for fn in filenames:
                path = os.path.join(dirpath, fn)
                rel = os.path.relpath(path, out_dir).replace(os.sep, "/")
                info = tarfile.TarInfo(rel)
                info.mtime = epoch
                info.uid = info.gid = 0
                info.uname = info.gname = "root"
                st = os.stat(path)
                info.size = st.st_size
                info.mode = 0o755 if os.access(path, os.X_OK) else 0o644
                with open(path, "rb") as f:
                    tf.addfile(info, f)
PY
  else
    echo "error: need GNU tar or python3 for reproducible archives" >&2
    exit 1
  fi
fi
(
  cd "$OUT_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${NAME}.${ARCHIVE_EXT}" > "${NAME}.sha256"
  else
    shasum -a 256 "${NAME}.${ARCHIVE_EXT}" > "${NAME}.sha256"
  fi
)
echo "Bundle: $ARCHIVE"
cat "$OUT_DIR/${NAME}.sha256"
