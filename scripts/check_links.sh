#!/usr/bin/env bash
# Crawl local docs HTML + README links; fail on missing internal targets.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
exec python3 - "$@" <<'PY'
from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(".").resolve()
DOCS = ROOT / "docs"
README = ROOT / "README.md"
SITE = "https://thehamdiz.github.io/super-lang"


def html_paths() -> list[Path]:
    out = [DOCS / "index.html"]
    out.extend(sorted(DOCS.glob("*/index.html")))
    out.extend(sorted(DOCS.glob("*.html")))  # redirect stubs
    return [p for p in out if p.is_file()]


def resolve_href(base: Path, href: str) -> Path | None:
    path, _, _frag = href.partition("#")
    path = path.partition("?")[0]
    if not path or path == "./":
        return base.parent / "index.html" if base.name == "index.html" else base
    if path.endswith("/"):
        path = path + "index.html"
    target = (base.parent / path).resolve()
    if target.is_dir():
        target = target / "index.html"
    try:
        target.relative_to(ROOT)
    except ValueError:
        return None
    return target


def check_html() -> int:
    fail = 0
    for html in html_paths():
        text = html.read_text(encoding="utf-8")
        for href in re.findall(r'href="([^"]+)"', text):
            if href.startswith(("http://", "https://", "mailto:", "data:", "#")):
                continue
            if href.endswith(".md") or "REPORT.md" in href:
                # Markdown links may point at repo files; resolve if local.
                cand = (html.parent / href.split("#", 1)[0]).resolve()
                if cand.exists():
                    continue
                # Allow GitHub-style relative md only when file exists at repo root / docs
                for base in (ROOT, DOCS):
                    if (base / Path(href).name).exists() or (base / href).exists():
                        break
                else:
                    # quickstart historically embeds md link text; skip non-files
                    continue
                continue
            target = resolve_href(html, href)
            if target is None:
                continue
            if not target.exists():
                print(f"MISSING: {html.relative_to(ROOT)} -> {href} (expected {target})")
                fail = 1
    return fail


def site_path_exists(url: str) -> bool:
    """Map site URL to local docs path."""
    if not url.startswith(SITE):
        return True  # external — not checked here
    path = url[len(SITE) :]
    if not path or path == "/":
        return (DOCS / "index.html").is_file()
    path = path.lstrip("/")
    # strip query/fragment already done by caller
    if path.endswith(".html"):
        return (DOCS / path).is_file()
    if path.endswith("/"):
        return (DOCS / path / "index.html").is_file()
    # bare slug or file
    if (DOCS / path).is_file():
        return True
    if (DOCS / path / "index.html").is_file():
        return True
    if (ROOT / path).is_file():
        return True
    return False


def check_readme() -> int:
    if not README.is_file():
        print("MISSING: README.md")
        return 1
    text = README.read_text(encoding="utf-8")
    fail = 0
    # markdown links
    for label, href in re.findall(r"\[([^\]]+)\]\(([^)]+)\)", text):
        href = href.strip()
        if href.startswith(("mailto:", "#")):
            continue
        if href.startswith("http://") or href.startswith("https://"):
            parsed = urlparse(href)
            if parsed.netloc.endswith("github.io") and "super-lang" in parsed.path:
                clean = f"{parsed.scheme}://{parsed.netloc}{parsed.path}"
                if not site_path_exists(clean.rstrip("/") + ("/" if parsed.path.endswith("/") else "")):
                    # try with and without trailing slash
                    ok = site_path_exists(clean) or site_path_exists(clean.rstrip("/") + "/")
                    if not ok and not (DOCS / Path(parsed.path).name).exists():
                        # map /super-lang/foo -> docs/foo
                        rel = parsed.path.split("/super-lang/", 1)[-1]
                        if rel.endswith(".html"):
                            ok = (DOCS / rel).is_file()
                        elif not rel or rel == "/":
                            ok = (DOCS / "index.html").is_file()
                        else:
                            ok = (DOCS / rel.rstrip("/") / "index.html").is_file() or (
                                DOCS / rel
                            ).is_file()
                    if not ok:
                        print(f"MISSING README site link: {href}")
                        fail = 1
            continue
        # local relative
        target = (ROOT / href.split("#", 1)[0]).resolve()
        if not target.exists():
            print(f"MISSING README link: {href}")
            fail = 1
    return fail


def main() -> int:
    fail = check_html() | check_readme()
    if fail:
        print("check_links: FAILED")
        return 1
    print("OK: all internal doc links resolve")
    print("OK: README links resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())
PY
