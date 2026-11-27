#!/usr/bin/env python3
"""Reject vacuous stdlib laws and laws that never mention their module's API."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"

VACUOUS = [
    re.compile(r"#assert\s*\(\s*true\s*\)", re.I),
    re.compile(r"#assert\s+true\b", re.I),
    re.compile(r"ensure\s+true\b", re.I),
    re.compile(r"#assert\s*\(\s*1\s*==\s*1\s*\)"),
    # Totality tautologies (always true): ban as vacuous under R8 / Round 6/7.
    re.compile(r"#assert\s*\(\s*\w+\s*\|\|\s*!\s*\w+\s*\)"),
    re.compile(r"#assert\s*\(\s*(\w+)\s*==\s*\1\s*\|\|\s*\1\s*!=\s*\1\s*\)"),
    re.compile(r"#assert\s*\(\s*(\w+(?:\.\w+)?)\s*==\s*0\s*\|\|\s*\1\s*!=\s*0\s*\)"),
    # Integer totality / identity tautologies (Round 7).
    re.compile(
        r"#assert\s*\(\s*(\w+(?:\.\w+)?)\s*>=\s*0\s*\|\|\s*\1\s*<\s*0\s*\)"
    ),
    re.compile(
        r"#assert\s*\(\s*(\w+(?:\.\w+)?)\s*<\s*0\s*\|\|\s*\1\s*>=\s*0\s*\)"
    ),
    re.compile(r"#assert\s*\(\s*(\w+(?:\.\w+)?)\s*==\s*\1\s*\)"),
]

FN_PLUS = re.compile(
    r"^\+([A-Za-z_][A-Za-z0-9_]*)\s*(?:\(|=|:)",
    re.MULTILINE,
)
LAW_BLOCK = re.compile(
    r'#law\s*\(\s*"([^"]*)"\s*\)\s*\n([^{]*?)\{',
    re.MULTILINE,
)


def impl_modules() -> list[Path]:
    out: list[Path] = []
    for p in sorted(STDLIB.rglob("*.sr")):
        if not p.is_file():
            continue
        if p.name.endswith(".law.sr") or p.name.endswith(".proof.sr") or p.name == "mod.sr":
            continue
        out.append(p)
    return out


def public_names(impl: Path) -> set[str]:
    text = impl.read_text(encoding="utf-8", errors="replace")
    names = set(FN_PLUS.findall(text))
    for m in re.finditer(r"^\+([A-Za-z_][A-Za-z0-9_]*)\s*=", text, re.MULTILINE):
        names.add(m.group(1))
    # Drop ALL_CAPS constants unless they look like functions (heuristic: has lower)
    return {n for n in names if not (n.isupper() and "_" in n) or not n.isupper()}


def law_path(impl: Path) -> Path:
    stem = impl.stem
    return impl.with_name(f"{stem}.law.sr")


def law_references_api(law_text: str, names: set[str]) -> bool:
    if not names:
        return True
    for name in sorted(names, key=len, reverse=True):
        if len(name) < 2:
            continue
        if re.search(rf"\b{re.escape(name)}\b", law_text):
            return True
    return False


def scan_law_file(law: Path, impl: Path) -> list[str]:
    text = law.read_text(encoding="utf-8", errors="replace")
    hits: list[str] = []
    rel = law.relative_to(ROOT)

    for pat in VACUOUS:
        if pat.search(text):
            hits.append(f"{rel}: vacuous assertion ({pat.pattern})")

    names = public_names(impl)
    if not law_references_api(text, names):
        hits.append(
            f"{rel}: no reference to exported names from {impl.relative_to(ROOT)} "
            f"(sample: {', '.join(sorted(names)[:8])}…)"
        )
    return hits


def main() -> int:
    all_hits: list[str] = []
    for impl in impl_modules():
        law = law_path(impl)
        if not law.is_file():
            continue
        all_hits.extend(scan_law_file(law, impl))

    if all_hits:
        print("FAIL: vacuous or unanchored laws:", file=sys.stderr)
        for h in all_hits:
            print(f"  {h}", file=sys.stderr)
        return 1

    print("OK: no vacuous or unanchored laws")
    return 0


if __name__ == "__main__":
    sys.exit(main())