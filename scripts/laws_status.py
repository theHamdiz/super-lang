#!/usr/bin/env python3
"""Regenerate docs/LAWS_STATUS.md: real-body vs sidecar by call-trace proof.

A law file is **real-body** when a call-trace from at least one `#law` body
reaches a symbol defined in the sibling implementation `.sr` (not only a
`law_*` model helper). Traces follow `law_*` helpers defined in the same law
file, so a body that only calls `law_foo` still counts when `law_foo` itself
invokes an impl symbol.

Sidecar = every law body's call-trace stays inside `law_*` helpers / builtins
with no impl callee.
"""
from __future__ import annotations

import re
import sys
from collections import defaultdict, deque
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STDLIB = ROOT / "stdlib"
OUT = ROOT / "docs" / "LAWS_STATUS.md"

LAW_BLOCK = re.compile(
    r'#law\(\s*"[^"]*"\s*\)\s*([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*\{',
    re.M,
)
# Top-level helper: name(...) { ... } or name(...) => ...
HELPER_BLOCK = re.compile(
    r"^(?:law_[A-Za-z0-9_]+|[A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*(?::\s*[^=\{]+)?\s*(\{|=>)",
    re.M,
)
HELPER_NAME = re.compile(
    r"^([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*(?::\s*[^=\{]+)?\s*(\{|=>)",
    re.M,
)
CALL = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")
DOT_CALL = re.compile(r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")
PLUS_FN = re.compile(r"^[ \t]*\+([A-Za-z_][A-Za-z0-9_]*)\s*\(", re.M)
METHOD = re.compile(r"^[ \t]+([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)\s*(:|=)", re.M)
SKIP = {
    "if",
    "for",
    "when",
    "ensure",
    "return",
    "assert_eq",
    "print",
    "fail",
    "panic",
    "assert",
    "assert_err",
}


def impl_symbols(impl: Path) -> set[str]:
    if not impl.is_file():
        return set()
    text = impl.read_text(encoding="utf-8")
    names: set[str] = set()
    for m in PLUS_FN.finditer(text):
        names.add(m.group(1))
    for m in METHOD.finditer(text):
        names.add(m.group(1))
    for m in re.finditer(r"\b([a-z][a-z0-9_]*)\s*\(self", text):
        names.add(m.group(1))
    for m in re.finditer(r"\.([a-z][a-z0-9_]*)\s*\(", text):
        names.add(m.group(1))
    return names


def extract_balanced(text: str, start: int) -> str:
    depth = 1
    i = start
    while i < len(text) and depth:
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
        i += 1
    return text[start : i - 1]


def law_bodies(text: str) -> list[str]:
    bodies: list[str] = []
    for m in LAW_BLOCK.finditer(text):
        bodies.append(extract_balanced(text, m.end()))
    return bodies


def helper_bodies(text: str) -> dict[str, str]:
    """Map helper name → body text for same-file call-trace."""
    out: dict[str, str] = {}
    for m in HELPER_NAME.finditer(text):
        name = m.group(1)
        # Skip #law entry points — handled separately.
        if text[max(0, m.start() - 80) : m.start()].rfind("#law(") != -1:
            # Crude: if the nearest preceding non-ws is a law header, skip.
            pass
        kind = m.group(2)
        if kind == "{":
            body = extract_balanced(text, m.end())
        else:
            # Expression body until newline / next top-level.
            rest = text[m.end() :]
            line = rest.split("\n", 1)[0]
            body = line
        out[name] = body
    return out


def callees(body: str) -> set[str]:
    names: set[str] = set()
    for m in CALL.finditer(body):
        n = m.group(1)
        if n not in SKIP:
            names.add(n)
    for m in DOT_CALL.finditer(body):
        names.add(m.group(1))
    return names


def call_trace_hits_impl(
    bodies: list[str], helpers: dict[str, str], impl: set[str]
) -> tuple[bool, list[str]]:
    """BFS from law bodies through same-file helpers. Returns (hit, sample_path).

    Callee enqueue order is sorted so the witness path is stable across
    PYTHONHASHSEED (set iteration otherwise flaps CI `git diff --exit-code`).
    """
    for body in bodies:
        q: deque[tuple[str, list[str]]] = deque()
        for c in sorted(callees(body)):
            q.append((c, [c]))
        seen: set[str] = set()
        while q:
            name, path = q.popleft()
            if name in seen:
                continue
            seen.add(name)
            if name in impl and not name.startswith("law_"):
                return True, path
            if name in impl and name.startswith("law_"):
                # Unusual: impl exports law_* — still counts as impl.
                return True, path
            if name in helpers:
                for c in sorted(callees(helpers[name])):
                    if c not in seen:
                        q.append((c, path + [c]))
    return False, []


def classify(law_path: Path) -> tuple[int, str, str]:
    text = law_path.read_text(encoding="utf-8")
    bodies = law_bodies(text)
    n_laws = len(bodies)
    impl = law_path.parent / (law_path.name[: -len(".law.sr")] + ".sr")
    symbols = impl_symbols(impl)
    helpers = helper_bodies(text)
    # Drop law entry function names from helpers so we don't recurse into them oddly.
    for m in LAW_BLOCK.finditer(text):
        helpers.pop(m.group(1), None)
    hit, path = call_trace_hits_impl(bodies, helpers, symbols)
    evidence = " → ".join(path) if path else ""
    return n_laws, ("real-body" if hit else "sidecar"), evidence


def main() -> int:
    rows: list[tuple[str, int, str, str]] = []
    for path in sorted(STDLIB.rglob("*.law.sr")):
        n, mode, evidence = classify(path)
        rows.append((str(path.relative_to(ROOT)), n, mode, evidence))

    real = sum(1 for _, _, m, _ in rows if m == "real-body")
    side = sum(1 for _, _, m, _ in rows if m == "sidecar")
    impl_mods = sum(
        1
        for p in STDLIB.rglob("*.sr")
        if not p.name.endswith(".law.sr")
        and not p.name.endswith(".proof.sr")
        and p.name != "mod.sr"
    )
    total_sr = len(list(STDLIB.rglob("*.sr")))
    real_mod_names = {
        Path(rel).name[: -len(".law.sr")]
        for rel, _, m, _ in rows
        if m == "real-body"
    }

    lines = [
        "# Laws status (0.1.0)",
        "",
        "Generated by `scripts/laws_status.py` (**call-trace proof**: BFS from",
        "`#law` bodies through same-file helpers until an impl-defined symbol is",
        "reached). Sidecar = no path from any law body to the sibling `.sr` impl.",
        "",
        "",
        "| Law file | #laws | Mode | Trace (sample) |",
        "|---|---:|---|---|",
    ]
    for rel, n, mode, evidence in rows:
        cell = evidence if evidence else "—"
        lines.append(f"| `{rel}` | {n} | {mode} | `{cell}` |" if evidence else f"| `{rel}` | {n} | {mode} | — |")
    lines += [
        "",
        f"Real-body (call-trace): **{real}** / {len(rows)} law files. Sidecar: **{side}**",
        "",
        f"Stdlib tree: **{total_sr}** `.sr` files (claim floor); **{impl_mods}** implementation",
        f"modules (excl. mod/law/proof). Real-body law files covering distinct modules:",
        f"**{len(real_mod_names)}** of {impl_mods} implementation modules",
        f"(and **{real}** of {len(rows)} law files; {total_sr} total `.sr` including sidecars).",
        "",
        "Limits (remain sidecar — do not claim as real-body):",
        "",
        "| Law file | Why still sidecar |",
        "|---|---|",
        "| `data/expr.law.sr` / `data/plan.law.sr` | Query IR / planner host loops under verify |",
        "| `html/parse.law.sr` / `html/template.law.sr` | Parser / template host string walks |",
        "| `io/buf.law.sr` | Ring buffer needs fixtures / host IO |",
        "| `net/url.law.sr` | `url_encode` percent-encode byte loops |",
        "| `sec/argon2,csprng,secret,sha2` | Crypto / CSPRNG host kernels |",
        "| `sys/time.law.sr` | `time_pad2` / sleep need host strlen / clocks |",
        "",
        "Mutation floors (seed `0xa1c0b26`, committed `docs/law-mutants.json`): overall ≥70%;",
        "PRIORITY pure modules ≥80%. Round 9 Item 7: `datetime/cron`, `fmt/csv`,",
        "`html/core`, and `net/http` target ≥80% via call-traced laws + verify-friendly",
        "stdlib helpers (see `docs/law-mutation-report.md`).",
        "",
        "Call-trace / mutation: `docs/law-mutation-report.md`.",
        "",
    ]
    OUT.write_text("\n".join(lines), encoding="utf-8")
    print(f"Wrote {OUT.relative_to(ROOT)}: real-body={real} sidecar={side} / {len(rows)}")
    print(
        f"stdlib .sr={total_sr} impl={impl_mods} real-body modules≈{len(real_mod_names)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
