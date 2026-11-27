#!/usr/bin/env python3
"""Fail if README, site, or README-adjacent claims disagree with docs/REPORT.md (R1)."""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT / "docs" / "REPORT.md"
README = ROOT / "README.md"
DOCS_SITE = ROOT / "docs"


def die(msg: str) -> None:
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def warn(msg: str) -> None:
    print(f"WARN: {msg}")


def parse_report() -> dict[str, str | float]:
    text = REPORT.read_text(encoding="utf-8")
    if not text:
        die("docs/REPORT.md is empty")
    m = re.search(
        r"`super_aot`\s*\|\s*\*\*([\d.]+)\*\*\s*\|\s*([\d.]+)\s*\|\s*([\d.]+)",
        text,
    )
    if not m:
        die("could not parse super_aot row in docs/REPORT.md")
    composite = float(m.group(1))
    speed = float(m.group(2))
    if "speed**: `zig`" not in text and "**speed**: `zig`" not in text:
        if "speed**: `zig`" not in text.replace(" ", ""):
            if "**speed**: `zig`" not in text:
                die("docs/REPORT.md must name zig as speed category winner")
    return {
        "composite_full": composite,
        "composite_display": round(composite, 1),
        "super_speed": speed,
        "speed_winner": "zig",
        "speed_winner_score": 100.0,
    }


def grep_forbidden() -> list[str]:
    # benchmarks-memory soak CSVs contain timestamps like "90.070" that are not
    # the forbidden Stage-1 composite; keep them out of this scan.
    skip_dirs = {".git", "target", "node_modules", "agent-tools", "benchmarks-memory"}
    hits: list[str] = []
    for p in ROOT.rglob("*"):
        if not p.is_file():
            continue
        if any(part in skip_dirs for part in p.parts):
            continue
        if p.suffix in {".png", ".pdf", ".woff2", ".ttf", ".ico", ".mp4", ".json"}:
            if p.name != "results.json":
                continue
        try:
            data = p.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        if p.name in ("check_numbers.py", "FIX_LOG.md"):
            continue
        # Round reports may quote historical composites; they are not live claims.
        if p.name.startswith("ROUND") and p.suffix == ".md":
            continue
        if "90.07" in data:
            hits.append(str(p.relative_to(ROOT)))
    return hits


def check_readme(ref: dict[str, str | float]) -> None:
    text = README.read_text(encoding="utf-8")
    disp = ref["composite_display"]
    if "89.2" not in text:
        die("README.md must show composite 89.2")
    if "90.07" in text:
        die("README.md still contains 90.07")
    if "Zig wins Speed" not in text and "Speed: **Zig**" not in text:
        if "Zig" not in text or "97.87" not in text:
            die("README must state Zig wins Speed with SuperAot 97.87")
    bad_speed = re.search(r"Super.*won.*Speed|Speed.*Super.*#1|wins Speed.*Super", text, re.I)
    if bad_speed:
        die(f"README claims Super won Speed: {bad_speed.group(0)!r}")
    if "within noise" not in text.lower():
        die("README must mention within-noise pairs (nbody, astar, json)")
    for phrase in ("faster than C",):
        if phrase.lower() in text.lower():
            die(f"README contains unqualified claim {phrase!r}")
    if "29,758" not in text and "29758" not in text:
        die("README must mention C port LoC 29,758 with methodology")
    if "2 line" not in text.lower() and "two line" not in text.lower():
        if "2200" not in text or "2198" not in text:
            die("README must note Bun within 2 lines of Super on suite LoC")
    if "docs/REPORT.md" not in text:
        die("README must link docs/REPORT.md")
    print("OK: README numbers and honesty")


def check_no_duplicate_language_nav() -> None:
    """Home is the only landing; /language/ is redirect-only — no Language nav link."""
    bad: list[str] = []
    for page in DOCS_SITE.rglob("*.html"):
        if page.parts[-2:] == ("language", "index.html"):
            continue
        text = page.read_text(encoding="utf-8")
        if re.search(r'href="(?:\.\./)*language/">Language</a>', text):
            bad.append(str(page.relative_to(ROOT)))
    if bad:
        die("Language nav link must not duplicate Home:\n  " + "\n  ".join(bad[:12]))
    lang_page = DOCS_SITE / "language" / "index.html"
    if not lang_page.is_file():
        die("docs/language/index.html missing (redirect to /)")
    if 'url=../' not in lang_page.read_text(encoding="utf-8"):
        die("docs/language/index.html must redirect to site root")
    print("OK: single home landing (no Language nav)")


def check_score_race_includes_julia() -> None:
    """Score race canvas must list every composite language (Julia is 9th; no top-8 cap)."""
    pages = (
        DOCS_SITE / "benchmarks" / "index.html",
        DOCS_SITE / "report" / "index.html",
        DOCS_SITE / "charts" / "index.html",
    )
    for page in pages:
        if not page.is_file():
            die(f"missing benchmark page {page.relative_to(ROOT)}")
        text = page.read_text(encoding="utf-8")
        if "ranked().slice(0, 8)" in text:
            die(f"{page.relative_to(ROOT)} still caps score race at 8 languages")
        if 'data-lang="julia"' not in text and '"julia"' not in text:
            die(f"{page.relative_to(ROOT)} missing Julia in report data or picks")
    print("OK: score race lists all composite languages (incl. Julia)")


def check_reliability_results_json() -> None:
    """Machine field must encode Reliability as all/tie — not a sole SuperAot winner."""
    import json

    path = DOCS_SITE / "results.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    winners = data.get("category_winners") or {}
    rel = winners.get("reliability")
    if rel not in ("all", "tie"):
        die(f"docs/results.json category_winners.reliability must be all|tie, got {rel!r}")
    for lang, row in (data.get("composite_scores") or {}).items():
        if float(row.get("reliability", -1)) != 100.0:
            die(f"reliability score drift for {lang}: {row.get('reliability')}")
    print("OK: results.json Reliability winner = all/tie (scores still 100%)")


def check_reliability_display_pages() -> None:
    """Visitor-facing pages must show Reliability category winner as All (not SuperAot)."""
    paths = [
        ROOT / "docs" / "charts" / "index.html",
        ROOT / "docs" / "benchmarks" / "index.html",
        ROOT / "docs" / "report" / "index.html",
    ]
    bad_category = re.compile(
        r'"category"\s*:\s*\{[^}]*"reliability"\s*:\s*"super_aot"',
        re.DOTALL,
    )
    for path in paths:
        if not path.is_file():
            die(f"missing expected page {path.relative_to(ROOT)}")
        text = path.read_text(encoding="utf-8")
        if "Reliability winner: SuperAot" in text:
            die(f"{path.relative_to(ROOT)} labels SuperAot as Reliability winner")
        if 'data-axis="reliability" data-lang="super_aot"' in text:
            die(f"{path.relative_to(ROOT)} reliability cat-tile still super_aot")
        if bad_category.search(text):
            die(
                f"{path.relative_to(ROOT)} report-data category.reliability must be "
                '"all" (display All), not super_aot'
            )
        if '"reliability": "all"' not in text and '"reliability":"all"' not in text:
            die(f"{path.relative_to(ROOT)} must set category.reliability to all")
        if '"all": "All"' not in text and '"all":"All"' not in text:
            die(f"{path.relative_to(ROOT)} report-data display must include all → All")
    print("OK: charts + benchmarks + report Reliability → All")


def check_site(ref: dict[str, str | float]) -> None:
    home = (DOCS_SITE / "index.html").read_text(encoding="utf-8")
    if 'id="what-is-super"' not in home:
        die("docs/index.html (language home) must include What is Super intro")
    if "Stage-1 equal-algorithm suite" in home and "hero-board" in home:
        die("docs/index.html must be language home, not the benchmark dashboard")
    bench = (DOCS_SITE / "benchmarks" / "index.html").read_text(encoding="utf-8")
    if 'id="what-is-super"' in bench:
        die("docs/benchmarks/ must not include What is Super intro")
    if "89.2" not in bench:
        die("docs/benchmarks/ must display composite 89.2")
    if "data-axis=\"speed\" data-lang=\"zig\"" not in bench:
        die("docs/benchmarks/ must show Zig as Speed category winner")
    if "Reliability winner: All" not in bench and "<strong>All</strong>" not in bench:
        die("docs/benchmarks/ must show All as Reliability category winner (100% pass all langs)")
    if "Reliability winner: SuperAot" in bench:
        die("docs/benchmarks/ must not label SuperAot as Reliability winner")
    if "| **Reliability** | **All**" not in README.read_text(encoding="utf-8"):
        die("README Results table must list Reliability winner as All")
    if "within-noise" not in bench and "within noise" not in bench:
        die("docs/benchmarks/ must label within-noise algorithm pairs")
    if "90.07" in bench:
        die("docs/benchmarks/ contains 90.07")
    if "<noscript>" not in bench or "Composite scores" not in bench:
        die("docs/benchmarks/ must include <noscript> static score tables")
    if "low-boilerplate expression syntax" not in home:
        die("docs/index.html intro must describe Super syntax on its own terms")
    print("OK: site home + benchmarks headline numbers")


def load_facts() -> dict:
    facts_path = ROOT / "docs" / "facts.json"
    if not facts_path.is_file():
        r = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "generate_facts.py")],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if r.returncode != 0:
            die(f"generate_facts.py failed:\n{r.stdout}\n{r.stderr}")
    import json

    return json.loads(facts_path.read_text(encoding="utf-8"))


def check_stdlib_counts(facts: dict) -> None:
    r = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "count_stdlib.py")],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    if r.returncode != 0:
        die(f"count_stdlib.py failed:\n{r.stdout}\n{r.stderr}")
    files = int(facts.get("stdlib_sr_files", -1))
    modules = int(facts.get("stdlib_modules", -1))
    readme = README.read_text(encoding="utf-8")
    if str(files) not in readme or str(modules) not in readme:
        die(f"README must state {files} files / {modules} modules (docs/facts.json)")
    llms = (ROOT / "docs" / "llms.txt").read_text(encoding="utf-8")
    if str(modules) not in llms:
        die(f"docs/llms.txt must mention {modules} stdlib modules")
    print("OK: stdlib counts vs facts.json")


def main() -> int:
    ref = parse_report()
    hits = grep_forbidden()
    if hits:
        die("stale composite 90.07 still present in:\n  " + "\n  ".join(hits[:20]))
    print("OK: no 90.07 in repo text files")
    check_readme(ref)
    check_site(ref)
    check_reliability_results_json()
    check_reliability_display_pages()
    check_score_race_includes_julia()
    check_no_duplicate_language_nav()
    facts = load_facts()
    check_stdlib_counts(facts)
    print(f"OK: canonical composite {ref['composite_full']} (display {ref['composite_display']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())