#!/usr/bin/env python3
"""Report stdlib law/proof coverage; enforce modules_with_laws floor."""
from __future__ import annotations

import json
import re
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
OUT_JSON = ROOT / "docs" / "law_coverage.json"
FLOOR_JSON = ROOT / "docs" / "law_coverage_floor.json"

LAW_DECL = re.compile(r'^#law\s*\(\s*"([^"]*)"\s*\)', re.MULTILINE)
PROOF_DECL = re.compile(r"^#proof\s*\(\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\)", re.MULTILINE)


def list_impl_modules() -> list[Path]:
    files = sorted(p for p in STDLIB.rglob("*.sr") if p.is_file())
    return [
        p
        for p in files
        if not p.name.endswith(".law.sr")
        and not p.name.endswith(".proof.sr")
        and p.name != "mod.sr"
    ]


def module_id(path: Path) -> str:
    rel = path.relative_to(STDLIB)
    return str(rel.with_suffix("")).replace("\\", "/")


def sidecar(impl: Path, suffix: str) -> Path:
    stem = impl.name.removesuffix(".sr")
    return impl.with_name(f"{stem}{suffix}")


def count_laws(text: str) -> tuple[int, list[str]]:
    names = LAW_DECL.findall(text)
    return len(names), names


def proof_status(law_names: list[str], proof_text: str | None) -> str:
    if not proof_text:
        return "none"
    proofs = set(PROOF_DECL.findall(proof_text))
    if not law_names:
        return "orphan_proof" if proofs else "none"
    missing = [n for n in law_names if n not in proofs]
    if not missing:
        return "complete"
    if proofs:
        return f"partial ({len(proofs)}/{len(law_names)})"
    return "missing"


def analyze(impl: Path) -> dict:
    law_p = sidecar(impl, ".law.sr")
    proof_p = sidecar(impl, ".proof.sr")
    has_law = law_p.is_file()
    has_proof = proof_p.is_file()
    law_count = 0
    law_names: list[str] = []
    if has_law:
        law_count, law_names = count_laws(law_p.read_text(encoding="utf-8"))
    proof_text = proof_p.read_text(encoding="utf-8") if has_proof else None
    return {
        "module": module_id(impl),
        "path": str(impl.relative_to(ROOT)),
        "has_law": has_law,
        "has_proof": has_proof,
        "law_count": law_count,
        "proof_status": proof_status(law_names, proof_text),
    }


def load_floor() -> int:
    if not FLOOR_JSON.is_file():
        return 0
    data = json.loads(FLOOR_JSON.read_text(encoding="utf-8"))
    return int(data.get("modules_with_laws", 0))


def write_floor(n: int) -> None:
    FLOOR_JSON.parent.mkdir(parents=True, exist_ok=True)
    FLOOR_JSON.write_text(
        json.dumps({"modules_with_laws": n}, indent=2) + "\n",
        encoding="utf-8",
    )


def markdown_table(rows: list[dict], summary: dict) -> str:
    lines = [
        f"# Stdlib law coverage ({summary['generated_at']})",
        "",
        f"Implementation modules: **{summary['stdlib_modules']}** · "
        f"with laws: **{summary['modules_with_laws']}** · "
        f"with proofs: **{summary['modules_with_proofs']}** · "
        f"total `#law`: **{summary['laws_total']}**",
        "",
        "| module | law | proof | laws | proof status |",
        "| --- | :---: | :---: | ---: | --- |",
    ]
    for r in sorted(rows, key=lambda x: x["module"]):
        lines.append(
            f"| `{r['module']}` | {'yes' if r['has_law'] else 'no'} | "
            f"{'yes' if r['has_proof'] else 'no'} | {r['law_count']} | {r['proof_status']} |"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    impls = list_impl_modules()
    rows = [analyze(p) for p in impls]
    modules_with_laws = sum(1 for r in rows if r["has_law"])
    modules_with_proofs = sum(1 for r in rows if r["has_proof"])
    laws_total = sum(r["law_count"] for r in rows)

    summary = {
        "version": "0.1.0",
        "generated_at": datetime.now(UTC).replace(microsecond=0).isoformat(),
        "stdlib_sr_files": len(list(STDLIB.rglob("*.sr"))),
        "stdlib_modules": len(impls),
        "modules_with_laws": modules_with_laws,
        "modules_with_proofs": modules_with_proofs,
        "laws_total": laws_total,
        "modules": rows,
    }

    OUT_JSON.parent.mkdir(parents=True, exist_ok=True)
    OUT_JSON.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")

    floor = load_floor()
    ok = True
    if modules_with_laws < floor:
        print(
            f"FAIL: modules_with_laws {modules_with_laws} < floor {floor} "
            f"({FLOOR_JSON.relative_to(ROOT)})",
            file=sys.stderr,
        )
        ok = False
    elif modules_with_laws > floor or not FLOOR_JSON.is_file():
        write_floor(modules_with_laws)
        print(f"OK: law floor set to {modules_with_laws}")

    print(markdown_table(rows, summary), end="")

    if ok:
        print(f"Wrote {OUT_JSON.relative_to(ROOT)}", file=sys.stderr)
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main())