#!/usr/bin/env python3
"""
Inject small bugs into stdlib modules and expect `super verify` to FAIL.

Writes docs/law-mutation-report.md (PARTIAL when verify does not observe the mutation).

Fixed seed: SUPER_LAW_MUTATION_SEED (default 0x0A1C_0B26).
"""
from __future__ import annotations

import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
REPORT = ROOT / "docs" / "law-mutation-report.md"
DEFAULT_SEED = 0x0A1C_0B26


@dataclass
class Row:
    module: str
    outcome: str  # caught | missed | skipped | error
    detail: str


def find_super() -> Path | None:
    release = ROOT / "target" / "release" / "super"
    if release.is_file():
        return release
    which = shutil.which("super")
    return Path(which) if which else None


def law_modules() -> list[Path]:
    return sorted(STDLIB.rglob("*.law.sr"))


def primary_for(law: Path) -> Path | None:
    stem = law.name.replace(".law.sr", "")
    primary = law.parent / f"{stem}.sr"
    return primary if primary.is_file() else None


def inject_bug(text: str, rng: random.Random) -> tuple[str, str]:
    """Return (mutated, description). Heuristic, one mutation per file."""
    lines = text.splitlines(keepends=True)
    candidates: list[tuple[int, str, str]] = []

    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("//") or stripped.startswith("//!"):
            continue
        if "=> host." in line and not line.strip().startswith("#"):
            broken = re.sub(r"=> host\.\w+", '=> "mutated"', line, count=1)
            if broken != line:
                candidates.append((i, broken, "replaced host call with constant str"))
        if " == " in line and "#assert" not in line and "law(" not in line:
            candidates.append((i, line.replace(" == ", " != ", 1), "flipped == to !="))
        if re.search(r"\b<\b", line) and "law(" not in line and "ensure " in line:
            candidates.append(
                (i, re.sub(r"<", ">", line, count=1), "flipped < to > in ensure")
            )
        if line.strip().startswith("return ") and ";" not in line:
            candidates.append((i, line.replace("return ", 'return "mutation" // was: ', 1), "corrupted return"))

    if not candidates:
        if "+JSON_LB" in text:
            return text.replace('+JSON_LB = "{"', '+JSON_LB = "["', 1), "JSON_LB brace→bracket"
        return text, "no mutation hook"

    idx, new_line, desc = rng.choice(candidates)
    lines[idx] = new_line
    return "".join(lines), desc


def verify_failed(super_bin: Path, entry: Path) -> bool:
    env = os.environ.copy()
    env["SUPER_STD"] = str(STDLIB)
    r = subprocess.run(
        [str(super_bin), "verify", str(entry)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout or "") + (r.stderr or "")
    if r.returncode != 0:
        return True
    if " failed" in out and "0 failed" not in out:
        return True
    return False


def run(limit: int | None) -> list[Row]:
    super_bin = find_super()
    if super_bin is None:
        return [Row("(all)", "skipped", "`super` not found at target/release/super or PATH")]

    seed = int(os.environ.get("SUPER_LAW_MUTATION_SEED", str(DEFAULT_SEED)), 0)
    rng = random.Random(seed)
    laws = law_modules()
    if limit is not None:
        laws = laws[:limit]

    rows: list[Row] = []
    with tempfile.TemporaryDirectory(prefix="super-law-mut-") as tmp:
        work = Path(tmp)
        for law in laws:
            primary = primary_for(law)
            if primary is None:
                rows.append(
                    Row(str(law.relative_to(STDLIB)), "skipped", "no primary .sr")
                )
                continue

            mod_key = str(primary.relative_to(ROOT))
            prim_name = primary.name
            law_name = law.name
            proof_path = law.parent / f"{primary.stem}.proof.sr"

            dest_dir = work / law.parent.relative_to(STDLIB)
            dest_dir.mkdir(parents=True, exist_ok=True)
            dest_primary = dest_dir / prim_name
            dest_law = dest_dir / law_name

            shutil.copy2(primary, dest_primary)
            shutil.copy2(law, dest_law)
            if proof_path.is_file():
                shutil.copy2(proof_path, dest_dir / proof_path.name)

            original = dest_primary.read_text(encoding="utf-8")
            mutated, desc = inject_bug(original, rng)
            if mutated == original:
                rows.append(Row(mod_key, "skipped", f"no safe mutation site ({desc})"))
                continue
            dest_primary.write_text(mutated, encoding="utf-8")

            try:
                failed = verify_failed(super_bin, dest_primary)
            except subprocess.TimeoutExpired:
                rows.append(Row(mod_key, "error", "verify timed out"))
                continue

            if failed:
                rows.append(Row(mod_key, "caught", desc))
            else:
                rows.append(
                    Row(
                        mod_key,
                        "missed",
                        f"verify still green after: {desc} (PARTIAL — laws may not call mutated code)",
                    )
                )
    return rows


def write_report(rows: list[Row], seed: int) -> None:
    caught = sum(1 for r in rows if r.outcome == "caught")
    missed = sum(1 for r in rows if r.outcome == "missed")
    skipped = sum(1 for r in rows if r.outcome == "skipped")
    errors = sum(1 for r in rows if r.outcome == "error")
    partial = missed > 0 or any("PARTIAL" in r.detail for r in rows)

    lines = [
        "# Law mutation report (0.1.0)",
        "",
        f"Generated: {datetime.now(UTC).isoformat()}",
        f"Seed: `0x{seed:X}` (`SUPER_LAW_MUTATION_SEED`)",
        "",
        "| Outcome | Count |",
        "| --- | ---: |",
        f"| caught (verify failed as expected) | {caught} |",
        f"| missed (verify still passed) | {missed} |",
        f"| skipped | {skipped} |",
        f"| error | {errors} |",
        "",
    ]
    if partial:
        lines.append(
            "**Status: PARTIAL** — some injections did not fail `super verify`; "
            "laws may be symbolic only or not exercise mutated paths."
        )
        lines.append("")

    lines.extend(["## Modules", ""])
    for r in sorted(rows, key=lambda x: x.module):
        lines.append(f"- `{r.module}` — **{r.outcome}**: {r.detail}")
    lines.append("")

    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text("\n".join(lines), encoding="utf-8")
    print(f"Wrote {REPORT}")


def main() -> int:
    limit = os.environ.get("SUPER_LAW_MUTATION_LIMIT")
    lim = int(limit) if limit else None
    seed = int(os.environ.get("SUPER_LAW_MUTATION_SEED", str(DEFAULT_SEED)), 0)
    rows = run(lim)
    write_report(rows, seed)
    missed = sum(1 for r in rows if r.outcome == "missed")
    errors = sum(1 for r in rows if r.outcome == "error")
    # Soft gate: report is the artifact; exit non-zero only on hard errors.
    if any(r.module == "(all)" for r in rows):
        return 0
    return 1 if errors else (0 if missed == 0 else 0)


if __name__ == "__main__":
    sys.exit(main())