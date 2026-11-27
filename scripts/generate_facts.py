#!/usr/bin/env python3
"""Emit docs/facts.json — toolchain inventory for CI and agent context."""
from __future__ import annotations

import json
import re
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
PLATFORM_MD = ROOT / "docs" / "platform.md"
LAW_COVERAGE = ROOT / "docs" / "law_coverage.json"
OUT = ROOT / "docs" / "facts.json"
CARGO = ROOT / "Cargo.toml"

WORKSPACE_VER = re.compile(
    r"\[workspace\.package\]\s*\n(?:[^\[]*\n)*?^version\s*=\s*\"([^\"]+)\"",
    re.MULTILINE,
)


def stdlib_counts() -> tuple[int, int]:
    files = sorted(p for p in STDLIB.rglob("*.sr") if p.is_file())
    modules = [
        p
        for p in files
        if not p.name.endswith(".law.sr")
        and not p.name.endswith(".proof.sr")
        and p.name != "mod.sr"
    ]
    return len(files), len(modules)


def load_law_summary() -> dict:
    if not LAW_COVERAGE.is_file():
        r = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "law_coverage.py")],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if r.returncode != 0:
            print(r.stdout, file=sys.stderr)
            print(r.stderr, file=sys.stderr)
            sys.exit(1)
    return json.loads(LAW_COVERAGE.read_text(encoding="utf-8"))


def parse_platforms(text: str) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for m in re.finditer(
        r"<tr><td>([^<]+)</td><td>(\d+)</td><td>([^<]+)</td><td>([^<]+)</td><td>([^<]+)</td><td><code>([^<]+)</code></td></tr>",
        text,
    ):
        rows.append(
            {
                "platform": m.group(1).strip(),
                "tier": m.group(2),
                "pal": m.group(3).strip(),
                "aot_sockets": m.group(4).strip(),
                "jit": m.group(5).strip(),
                "archive_suffix": m.group(6).strip(),
            }
        )
    return rows


def workspace_version() -> str:
    cargo = CARGO.read_text(encoding="utf-8")
    m = WORKSPACE_VER.search(cargo)
    return m.group(1) if m else "0.0.0"


def main() -> int:
    sr_files, modules = stdlib_counts()
    law = load_law_summary()
    platforms: list[dict[str, str]] = []
    if PLATFORM_MD.is_file():
        platforms = parse_platforms(PLATFORM_MD.read_text(encoding="utf-8"))

    mut = ROOT / "docs" / "law-mutation-report.md"
    mutation_score = None
    if mut.is_file():
        m = re.search(r"Mutation score:\s\*?\*?([0-9.]+)%", mut.read_text(encoding="utf-8"))
        if m:
            mutation_score = float(m.group(1))

    repro: dict[str, str] = {}
    for path in sorted((ROOT / "docs").glob("repro-*.txt")):
        text = path.read_text(encoding="utf-8")
        target = ""
        digest = ""
        result = ""
        for line in text.splitlines():
            if line.startswith("target="):
                target = line.split("=", 1)[1]
            elif line.startswith("build_A="):
                digest = line.split("=", 1)[1]
            elif line.startswith("result="):
                result = line.split("=", 1)[1]
        if target:
            repro[target] = f"{result}:{digest[:16]}…" if digest else result

    # Soak CSVs live under gitignored `benchmarks-memory/` (CI artifacts only).
    soak_meta: dict[str, object] = {
        "dir": "benchmarks-memory/",
        "tracked": False,
        "note": "local/CI workspace; gitignored — see docs/SOAK_GATE.md",
    }

    doc = {
        "version": workspace_version(),
        "generated_at": datetime.now(UTC).replace(microsecond=0).isoformat(),
        "stdlib_sr_files": sr_files,
        "stdlib_modules": modules,
        "modules_with_laws": law.get("modules_with_laws", 0),
        "modules_with_proofs": law.get("modules_with_proofs", 0),
        "laws_total": law.get("laws_total", 0),
        "platforms": platforms,
        "platform_tier1_count": sum(1 for p in platforms if p.get("tier") == "1"),
        "platform_tier2_count": sum(1 for p in platforms if p.get("tier") == "2"),
        "law_mutation_score_pct": mutation_score,
        "repro_targets": repro,
        "soak_memory": soak_meta,
        "alloc_probe_per_request": {"allocs": 0, "frees": 0, "note": "malloc counter; conn arena bump not free()"},
        "windows_aot_sockets": "Winsock2 (WSAPoll) in 0.1.0",
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {OUT.relative_to(ROOT)} ({modules} modules, {sr_files} .sr files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())