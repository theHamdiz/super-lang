#!/usr/bin/env python3
"""Fail if std.sec.secret and std.sec.vault export the same public +Name."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SECRET = ROOT / "stdlib" / "sec" / "secret.sr"
VAULT = ROOT / "stdlib" / "sec" / "vault.sr"

# Top-level public declarations: +Name = or +Name( or +Name:
PUB = re.compile(r"^\+([A-Za-z_][A-Za-z0-9_]*)\b", re.M)


def publics(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    return set(PUB.findall(text))


def main() -> int:
    s = publics(SECRET)
    v = publics(VAULT)
    both = sorted(s & v)
    print(f"secret public names: {len(s)}")
    print(f"vault public names: {len(v)}")
    if both:
        print("FAIL: shared public names:", ", ".join(both))
        return 1
    print("OK: no shared public names between std.sec.secret and std.sec.vault")
    return 0


if __name__ == "__main__":
    sys.exit(main())
