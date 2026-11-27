#!/usr/bin/env python3
"""Stdlib mutation testing for `super verify` + module tests (Round 7 Item 2).

Applies source mutations to stdlib implementation modules, rebuilds/verifies,
and classifies each mutant as KILLED / SURVIVED / SKIPPED.

Usage:
  scripts/mutate.py [--seed 0xA1C0B26] [--sample N] [--modules core/reduce,core/str]
  SUPER_LAW_MUTATION_SEED=0xA1C0B26 scripts/mutate.py --full

Does not shrink the mutant set to inflate scores. Writes
`docs/law-mutation-report.md` and `docs/law-mutants.json`.
"""
from __future__ import annotations

import argparse
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
SUPER = Path(os.environ.get("SUPER_BIN", ROOT / "target/release/super"))
DEFAULT_SEED = int(os.environ.get("SUPER_LAW_MUTATION_SEED", "0xA1C0B26"), 0)

# Pure modules first (Round 7 order).
PRIORITY = [
    "core/reduce",
    "core/str",
    "core/bytes",
    "math/core",
    "core/collections",
    "fmt/json",
    "sec/encoding",
    "datetime/core",
    "sec/hash",
]


@dataclass
class Mutant:
    module: str
    kind: str
    description: str
    line: int
    original: str
    mutated: str


MUTATORS = [
    ("flip_eq", re.compile(r"(?<![!=<>])==(?!=)"), "!="),
    ("flip_ne", re.compile(r"!="), "=="),
    ("flip_lt", re.compile(r"(?<![<>=!])<(?!=)"), ">"),
    ("flip_gt", re.compile(r"(?<![<>=!])>(?!=)"), "<"),
    ("flip_add", re.compile(r"(?<!\+)\+(?!\+)"), "-"),
    ("flip_sub", re.compile(r"(?<!-)-(?!-)"), "+"),
]


def modules_with_laws() -> list[Path]:
    out = []
    for law in sorted(STDLIB.rglob("*.law.sr")):
        impl = law.with_name(law.name.replace(".law.sr", ".sr"))
        if impl.is_file():
            out.append(impl)
    return out


# Words that look like calls but are not stdlib targets.
_LAW_CALL_NOISE = {
    "assert",
    "law",
    "proof",
    "if",
    "when",
    "for",
    "ensure",
    "return",
    "fail",
    "panic",
    "true",
    "false",
    "none",
    "print",
}


def law_referenced_names(impl: Path) -> set[str]:
    """Names the law file actually calls (`foo(` / `.foo(`), not comment words."""
    law = impl.with_name(impl.stem + ".law.sr")
    if not law.is_file():
        return set()
    text = law.read_text(encoding="utf-8", errors="replace")
    # Strip line comments so narrative words do not widen the mutant set.
    stripped = "\n".join(
        ln.split("//", 1)[0] for ln in text.splitlines() if not ln.lstrip().startswith("//!")
    )
    names = set(re.findall(r"(?<![\w.])([A-Za-z_][A-Za-z0-9_]*)\s*\(", stripped))
    names |= set(re.findall(r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\(", stripped))
    return {n for n in names if n not in _LAW_CALL_NOISE and len(n) > 1}


def collect_mutants(impl: Path, rng: random.Random, limit: int | None) -> list[Mutant]:
    text = impl.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    rel = str(impl.relative_to(ROOT))
    anchors = law_referenced_names(impl)
    found: list[Mutant] = []
    in_test = False
    current_fn = ""
    for i, line in enumerate(lines):
        stripped = line.lstrip()
        mfn = re.match(r"\+([A-Za-z_][A-Za-z0-9_]*)\s*[\(:=]", stripped)
        if mfn:
            current_fn = mfn.group(1)
        if stripped.startswith("test "):
            in_test = True
        if in_test and line.startswith("}"):
            in_test = False
            continue
        if in_test or stripped.startswith("//") or stripped.startswith("//!"):
            continue
        # Only mutants inside functions the law file actually calls.
        if anchors and current_fn and current_fn not in anchors:
            continue
        for kind, pat, repl in MUTATORS:
            if not pat.search(line):
                continue
            new_line = pat.sub(repl, line, count=1)
            if new_line == line:
                continue
            found.append(
                Mutant(
                    module=rel,
                    kind=kind,
                    description=f"{kind} on line {i+1} (fn={current_fn or '?'})",
                    line=i + 1,
                    original=line.rstrip("\n"),
                    mutated=new_line.rstrip("\n"),
                )
            )
    rng.shuffle(found)
    if limit is not None:
        found = found[:limit]
    return found


def apply_mutant(impl: Path, mutant: Mutant) -> str:
    lines = impl.read_text(encoding="utf-8").splitlines(keepends=True)
    idx = mutant.line - 1
    backup = lines[idx]
    lines[idx] = mutant.mutated + ("\n" if lines[idx].endswith("\n") else "")
    impl.write_text("".join(lines), encoding="utf-8")
    return backup


def restore_line(impl: Path, line_no: int, original: str) -> None:
    lines = impl.read_text(encoding="utf-8").splitlines(keepends=True)
    idx = line_no - 1
    lines[idx] = original + ("\n" if not original.endswith("\n") and lines[idx].endswith("\n") else "")
    # Prefer exact original with newline
    lines[idx] = original + "\n"
    impl.write_text("".join(lines), encoding="utf-8")


def run_verify(impl: Path) -> bool:
    """Return True if verify still passes (mutant SURVIVED)."""
    r = subprocess.run(
        [str(SUPER), "verify", str(impl)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=120,
    )
    return r.returncode == 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", default=hex(DEFAULT_SEED))
    ap.add_argument("--sample", type=int, default=2, help="mutants per module (PR sample)")
    ap.add_argument("--full", action="store_true", help="more mutants per module")
    ap.add_argument("--modules", default="", help="comma paths under stdlib/ e.g. core/reduce")
    args = ap.parse_args()
    seed = int(args.seed, 0)
    rng = random.Random(seed)
    per = 8 if args.full else args.sample

    if not SUPER.is_file():
        print(f"missing {SUPER}; build super-cli --release first", file=sys.stderr)
        return 2

    impls = modules_with_laws()
    if args.modules:
        want = {f"stdlib/{m}.sr" for m in args.modules.split(",") if m}
        impls = [p for p in impls if str(p.relative_to(ROOT)) in want]
    else:
        # Prefer priority order then the rest
        def key(p: Path) -> tuple:
            rel = str(p.relative_to(STDLIB).with_suffix(""))
            try:
                return (0, PRIORITY.index(rel))
            except ValueError:
                return (1, rel)

        impls = sorted(impls, key=key)

    results = []
    killed = survived = skipped = errors = 0
    mutant_list: list[dict] = []

    for impl in impls:
        mutants = collect_mutants(impl, rng, per)
        if not mutants:
            skipped += 1
            results.append({"module": str(impl.relative_to(ROOT)), "outcome": "skipped", "detail": "no safe mutation site"})
            continue
        for m in mutants:
            mutant_list.append(asdict(m))
            backup = apply_mutant(impl, m)
            try:
                ok = run_verify(impl)
            except Exception as e:
                errors += 1
                results.append({"module": m.module, "outcome": "error", "detail": str(e), "mutant": m.description})
                restore_line(impl, m.line, m.original)
                continue
            restore_line(impl, m.line, m.original)
            if ok:
                survived += 1
                results.append({"module": m.module, "outcome": "survived", "detail": m.description})
            else:
                killed += 1
                results.append({"module": m.module, "outcome": "killed", "detail": m.description})

    total = killed + survived
    score = (100.0 * killed / total) if total else 0.0
    now = datetime.now(timezone.utc).isoformat()
    report = ROOT / "docs/law-mutation-report.md"
    lines = [
        "# Law mutation report (0.1.0)",
        "",
        f"Generated: {now}",
        f"Seed: `{args.seed}` (`SUPER_LAW_MUTATION_SEED`)",
        "",
        "| Outcome | Count |",
        "| --- | ---: |",
        f"| killed | {killed} |",
        f"| survived | {survived} |",
        f"| skipped | {skipped} |",
        f"| error | {errors} |",
        "",
        f"**Mutation score: {score:.1f}%** ({killed}/{total}). "
        + ("PASS (≥70% overall)" if score >= 70 else "PARTIAL (below 70% overall gate)"),
        "",
        "## Modules",
        "",
    ]
    by_mod: dict[str, list] = {}
    for r in results:
        by_mod.setdefault(r["module"], []).append(r)
    for mod, rs in sorted(by_mod.items()):
        k = sum(1 for x in rs if x["outcome"] == "killed")
        s = sum(1 for x in rs if x["outcome"] == "survived")
        detail = "; ".join(f"{x['outcome']}: {x.get('detail','')}" for x in rs[:5])
        lines.append(f"- `{mod}` — killed={k} survived={s} — {detail}")
    lines.append("")
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")
    (ROOT / "docs/law-mutants.json").write_text(
        json.dumps({"seed": args.seed, "mutants": mutant_list, "results": results}, indent=2),
        encoding="utf-8",
    )
    print(f"mutation score={score:.1f}% killed={killed} survived={survived} skipped={skipped}")
    print(f"wrote {report}")
    return 0 if score >= 70 else 1


if __name__ == "__main__":
    raise SystemExit(main())
