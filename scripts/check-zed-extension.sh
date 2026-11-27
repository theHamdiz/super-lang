#!/usr/bin/env bash
# Verify the Super Zed extension grammar triple is consistent.
#
# Checks (in-repo source and optionally an installed copy):
#   [grammars.super]  ↔  languages/super/config.toml grammar="super"  ↔  *.wasm stem
#
# Usage:
#   ./scripts/check-zed-extension.sh
#   ./scripts/check-zed-extension.sh ~/.local/share/zed/extensions/installed/super
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-$ROOT/editors/zed}"

python3 - "$TARGET" <<'PY'
import re, sys
from pathlib import Path

root = Path(sys.argv[1])
ext_path = root / "extension.toml"
cfg_path = root / "languages" / "super" / "config.toml"
errors = []

if not ext_path.is_file():
    raise SystemExit(f"missing {ext_path}")
if not cfg_path.is_file():
    raise SystemExit(f"missing {cfg_path}")

ext = ext_path.read_text()
keys = re.findall(r"^\[grammars\.([^\]]+)\]", ext, flags=re.M)
if keys != ["super"]:
    errors.append(f"extension.toml grammar keys={keys!r}, expected ['super']")

# Reject dead Syr absolute paths in file:// grammar repos.
for m in re.finditer(r'^repository\s*=\s*"(file://[^"]+)"', ext, flags=re.M):
    url = m.group(1)
    if "/Syr/" in url or url.endswith("/Syr"):
        errors.append(f"stale Syr grammar repository: {url}")

cfg = cfg_path.read_text()
gm = re.search(r'^grammar\s*=\s*"([^"]+)"', cfg, flags=re.M)
if not gm or gm.group(1) != "super":
    errors.append(f"config.toml grammar={gm.group(1) if gm else None!r}, expected 'super'")

# Installed layouts ship grammars/super.wasm; source tree may only have the
# sibling tree-sitter build artifact.
gram_dir = root / "grammars"
candidates = []
if gram_dir.is_dir():
    candidates = list(gram_dir.glob("*.wasm"))
sib = root.parent / "tree-sitter-super" / "tree-sitter-super.wasm"
if sib.is_file():
    candidates.append(sib)

if gram_dir.is_dir():
    stems = sorted({p.stem for p in gram_dir.glob("*.wasm")})
    if stems != ["super"]:
        errors.append(f"grammars/*.wasm stems={stems!r}, expected ['super']")
elif not candidates:
    # Source checkout without a prebuilt wasm is OK if grammar section is correct;
    # install.sh builds/copies it.
    pass

if errors:
    print("FAIL: Zed Super extension grammar drift detected:")
    for e in errors:
        print(f"  - {e}")
    raise SystemExit(1)

print(f"OK: {root} — grammars.super ↔ grammar=\"super\"")
PY
