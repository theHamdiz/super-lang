#!/usr/bin/env python3
"""A4 acceptance: count stdlib .sr files/modules and compare to SPECIFICATION.md numbers."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
SPEC = ROOT / "SPECIFICATION.md"


def count_tree() -> tuple[int, int]:
    files = sorted(p for p in STDLIB.rglob("*.sr") if p.is_file())
    # Modules = implementation .sr excluding .law.sr / .proof.sr / mod.sr
    modules = [
        p
        for p in files
        if not p.name.endswith(".law.sr")
        and not p.name.endswith(".proof.sr")
        and p.name != "mod.sr"
    ]
    return len(files), len(modules)


def spec_numbers(text: str) -> list[tuple[int, int, str]]:
    """Find 'N files' / 'M modules' claims near stdlib wording."""
    found: list[tuple[int, int, str]] = []
    for m in re.finditer(
        r"(\d+)\s*\.sr\s*files[^\n]{0,80}?(\d+)\s*implementation modules",
        text,
    ):
        found.append((int(m.group(1)), int(m.group(2)), m.group(0)[:80]))
    for m in re.finditer(r"ships\s+\*\*(\d+)\*\*\s*\.sr\s*files", text):
        found.append((int(m.group(1)), -1, m.group(0)))
    return found


def mod_sr_count() -> int | None:
    """Importable module count from stdlib/mod.sr `stdlib_table()` triples."""
    mod = STDLIB / "mod.sr"
    if not mod.is_file():
        return None
    text = mod.read_text(encoding="utf-8")
    start = text.find("+stdlib_table()")
    if start < 0:
        return None
    end = text.find("]", start)
    vals = re.findall(r'"([^"]*)"', text[start:end])
    return len(vals) // 3


def main() -> int:
    files, modules = count_tree()
    text = SPEC.read_text(encoding="utf-8")
    claims = spec_numbers(text)
    print(f"tree: {files} .sr files, {modules} implementation modules (excl. mod/law/proof)")
    ok = True
    if not claims:
        print("WARN: no numeric stdlib claims matched in SPECIFICATION.md")
    for total, impl, snippet in claims:
        if total != files:
            print(f"FAIL: spec claims {total} files, tree has {files}: {snippet!r}")
            ok = False
        if impl >= 0 and impl != modules:
            print(f"FAIL: spec claims {impl} modules, tree has {modules}: {snippet!r}")
            ok = False
        if total == files and (impl < 0 or impl == modules):
            print(f"OK: claim matches tree ({snippet[:60]}…)")
    listed = mod_sr_count()
    if listed is None:
        print("WARN: could not parse stdlib/mod.sr stdlib_table()")
    elif listed != modules:
        print(f"FAIL: mod.sr lists {listed} modules, tree has {modules}")
        ok = False
    else:
        print(f"OK: mod.sr stdlib_table lists {listed} modules")
    # Always succeed with report when used as inventory; exit 1 only on mismatch.
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
