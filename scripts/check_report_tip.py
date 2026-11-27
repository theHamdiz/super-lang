#!/usr/bin/env python3
"""Fail when docs/ROUND*_REPORT.md tip hash differs from git HEAD.

Looks for a machine-readable line in each Round report:

    Report tip: <full-or-abbrev-sha>

Uses the highest-numbered ROUND*_REPORT.md that contains such a line.
That tip must match `git rev-parse HEAD` (full SHA or unique prefix).
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
TIP_RE = re.compile(
    r"^Report tip:\s*`?([0-9a-fA-F]{7,40}|HEAD)`?\s*$",
    re.IGNORECASE | re.MULTILINE,
)
ROUND_RE = re.compile(r"^ROUND(\d+)_REPORT\.md$")


def git_head() -> str:
    out = subprocess.check_output(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
        text=True,
    )
    return out.strip()


def round_reports() -> list[tuple[int, Path]]:
    found: list[tuple[int, Path]] = []
    if not DOCS.is_dir():
        return found
    for p in DOCS.iterdir():
        if not p.is_file():
            continue
        m = ROUND_RE.match(p.name)
        if m:
            found.append((int(m.group(1)), p))
    return sorted(found, key=lambda t: t[0])


def extract_tip(text: str) -> str | None:
    m = TIP_RE.search(text)
    if not m:
        return None
    raw = m.group(1)
    return raw if raw.upper() == "HEAD" else raw.lower()


def tips_match(report_tip: str, head: str) -> bool:
    tip = report_tip.strip()
    # Sentinel: a commit cannot contain its own full SHA; `HEAD` means "this tip".
    if tip.upper() == "HEAD":
        return True
    tip = tip.lower()
    head = head.lower()
    if tip == head:
        return True
    # Allow unique prefix of HEAD (report may cite short SHA).
    if len(tip) < len(head) and head.startswith(tip):
        return True
    if len(head) < len(tip) and tip.startswith(head):
        return True
    return False


def main() -> int:
    reports = round_reports()
    if not reports:
        print("FAIL: no docs/ROUND*_REPORT.md found", file=sys.stderr)
        return 1

    tip: str | None = None
    used: Path | None = None
    for _n, path in reversed(reports):
        text = path.read_text(encoding="utf-8")
        tip = extract_tip(text)
        if tip is not None:
            used = path
            break

    if tip is None or used is None:
        print(
            "FAIL: no 'Report tip: <sha>' line in any docs/ROUND*_REPORT.md",
            file=sys.stderr,
        )
        return 1

    try:
        head = git_head()
    except subprocess.CalledProcessError as exc:
        print(f"FAIL: git rev-parse HEAD: {exc}", file=sys.stderr)
        return 1

    if not tips_match(tip, head):
        rel = used.relative_to(ROOT)
        print(
            f"FAIL: {rel} tip {tip} != HEAD {head}",
            file=sys.stderr,
        )
        return 1

    rel = used.relative_to(ROOT)
    print(f"OK: {rel} tip matches HEAD ({head})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
