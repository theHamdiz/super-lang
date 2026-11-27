#!/usr/bin/env python3
"""Fail when Super product version strings drift from workspace 0.1.0."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARGO = ROOT / "Cargo.toml"
CANONICAL = "0.1.0"

WORKSPACE_VER = re.compile(
    r"\[workspace\.package\]\s*\n(?:[^\[]*\n)*?^version\s*=\s*\"([^\"]+)\"",
    re.MULTILINE,
)

# Super release / marketing version drift (not Rust crate deps like serde 1.0.x).
DRIFT_PATTERNS = [
    ("other_super_patch", re.compile(r"\b0\.1\.[1-9]\b")),
    ("other_super_minor", re.compile(r"\b0\.[2-9]\.\d+")),
    ("super_prerelease", re.compile(r"\b0\.1\.0[-+]")),
    ("wrong_product_line", re.compile(r'"version"\s*:\s*"1\.0\.0"')),
    ("super_lang_banner", re.compile(r"\bSuper\s+0\.1\.[1-9]\b")),
]

SCAN_ROOTS = [
    ROOT / "README.md",
    ROOT / "CHANGELOG.md",
    ROOT / "SPECIFICATION.md",
    ROOT / "SPECIFICATION.mdx",
    ROOT / "docs",
    ROOT / "scripts",
    ROOT / "install.sh",
    ROOT / "install.ps1",
    ROOT / "Cargo.toml",
    ROOT / "Pod.toml",
    ROOT / ".github" / "workflows",
]

SKIP_FILES = {
    "check_version.py",
    "law_coverage.py",
    "generate_facts.py",
    "results.json",
    "FIX_LOG.md",
    "book-pages.json",
}

SKIP_DIR_PARTS = {".git", "target", "node_modules", "benchmarks", "agent-tools"}


def canonical_from_cargo() -> str:
    text = CARGO.read_text(encoding="utf-8")
    m = WORKSPACE_VER.search(text)
    if not m:
        print("FAIL: could not read [workspace.package] version from Cargo.toml", file=sys.stderr)
        sys.exit(1)
    ver = m.group(1)
    if ver != CANONICAL:
        print(f"FAIL: workspace package version is {ver!r}, expected {CANONICAL}", file=sys.stderr)
        sys.exit(1)
    return ver


def iter_files() -> list[Path]:
    out: list[Path] = []
    for base in SCAN_ROOTS:
        if base.is_file():
            out.append(base)
            continue
        if not base.is_dir():
            continue
        for p in base.rglob("*"):
            if not p.is_file():
                continue
            if any(part in SKIP_DIR_PARTS for part in p.parts):
                continue
            if p.suffix in {".png", ".pdf", ".woff2", ".ttf", ".ico", ".wasm"}:
                continue
            if p.name in SKIP_FILES:
                continue
            out.append(p)
    return sorted(set(out))


def is_third_party_version_line(line: str) -> bool:
    low = line.lower()
    if re.search(r"^\s*(?:toml|serde|clap|semver|thiserror|serde_json)\s*=\s*\{", low):
        return True
    if re.search(r"^\s*[a-z_][a-z0-9_-]*\s*=\s*\"\d+\.\d+\.\d+\"", line):
        if "super" not in low and "workspace.package" not in low:
            return True
    if "version =" in low and "{" in line and "path" in low:
        return False
    if re.search(r'\b(?:serde|clap|toml|semver|thiserror|tokio|hyper)\b.*version\s*=\s*"[0-9]', low):
        return True
    if re.search(r'version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"', line) and "super" not in low:
        if "workspace" not in low and "package" not in low:
            return True
    return False


def is_allowed_super_prerelease_line(line: str) -> bool:
    """Release bundle filenames use super-0.1.0-<triple>, not semver prerelease tags."""
    if "super-0.1.0-" in line and any(x in line for x in (".tar.gz", ".zip", "archive", "extract")):
        return True
    if "shields.io/badge/version-0.1.0-" in line:
        return True
    if '"version": "0.1.0"' in line or "'version': '0.1.0'" in line:
        return True
    if re.search(r"\bsuper\s+0\.1\.0\b", line):
        return True
    return False


def scan_file(path: Path, patterns: list[tuple[str, re.Pattern[str]]]) -> list[str]:
    try:
        text = path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return []
    hits: list[str] = []
    rel = path.relative_to(ROOT)
    for i, line in enumerate(text.splitlines(), 1):
        low = line.lower()
        if is_third_party_version_line(line):
            continue
        for label, pat in patterns:
            if pat.search(line):
                if label == "super_prerelease" and is_allowed_super_prerelease_line(line):
                    continue
                if label in ("other_super_patch", "other_super_minor", "super_lang_banner"):
                    if "super" not in low and "Super" not in line:
                        continue
                    if label == "other_super_patch" and re.search(
                        r"\bsuper\b.*\b0\.1\.0\b", line, re.I
                    ):
                        continue
                hits.append(f"{rel}:{i}: {label}: {line.strip()[:120]}")
    return hits


def main() -> int:
    canonical_from_cargo()
    all_hits: list[str] = []
    for path in iter_files():
        all_hits.extend(scan_file(path, DRIFT_PATTERNS))

    if all_hits:
        print("FAIL: Super version drift (expected 0.1.0 only):", file=sys.stderr)
        for h in all_hits[:40]:
            print(f"  {h}", file=sys.stderr)
        if len(all_hits) > 40:
            print(f"  … and {len(all_hits) - 40} more", file=sys.stderr)
        return 1

    print(f"OK: Super product version locked at {CANONICAL}")
    return 0


if __name__ == "__main__":
    sys.exit(main())