#!/usr/bin/env python3
"""Compare two emitted C23 AOT files up to codegen numbering and initializer order.

The emitter numbers temporaries, globals and constants from global counters and
writes designated initializers in hash order, so byte equality is too strict.
Two files are *equivalent* when, after renumbering and sorting designated
initializer fields, they contain the same multiset of lines.

    python3 scripts/c_equivalence.py OLD.c NEW.c [OLD2.c NEW2.c ...]

Exit status is 0 when every pair is equivalent, 1 otherwise.
"""

from __future__ import annotations

import re
import sys
from collections import Counter
from pathlib import Path

_GLOBAL_INDEX = re.compile(r"super_globals\[\d+\]")
_CONST_INDEX = re.compile(r"const\[\d+\]")
_COUNTER_SUFFIX = re.compile(r"(?<=[A-Za-z_])\d+")
_DESIGNATED = re.compile(r"\{(\s*\.[A-Za-z_]\w*\s*=[^{}]*)\}")


def _sort_fields(match: re.Match[str]) -> str:
    fields = [part.strip() for part in match.group(1).split(", .")]
    fields = [field if field.startswith(".") else "." + field for field in fields]
    return "{" + ", ".join(sorted(fields)) + "}"


def canonical_lines(text: str) -> Counter[str]:
    text = _GLOBAL_INDEX.sub("super_globals[N]", text)
    text = _CONST_INDEX.sub("const[N]", text)
    text = _COUNTER_SUFFIX.sub("N", text)
    text = _DESIGNATED.sub(_sort_fields, text)
    return Counter(line.rstrip() for line in text.splitlines())


def compare(old: Path, new: Path) -> tuple[bool, list[str], list[str]]:
    a = canonical_lines(old.read_text(encoding="utf-8", errors="replace"))
    b = canonical_lines(new.read_text(encoding="utf-8", errors="replace"))
    only_old = sorted((a - b).elements())
    only_new = sorted((b - a).elements())
    return not only_old and not only_new, only_old, only_new


def main(argv: list[str]) -> int:
    if len(argv) < 2 or len(argv) % 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    ok = True
    for old, new in zip(argv[0::2], argv[1::2]):
        same, only_old, only_new = compare(Path(old), Path(new))
        print(f"{'SAME' if same else 'DIFF'} {new}")
        if not same:
            ok = False
            for line in only_old[:10]:
                print(f"  - {line}")
            for line in only_new[:10]:
                print(f"  + {line}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
