#!/usr/bin/env python3
"""One-shot: flat docs/*.html -> docs/<slug>/index.html + link rewrites."""
from __future__ import annotations

import re
import shutil
from pathlib import Path

from site_urls import asset_href, canonical_url, fix_html_for_subdir, page_href, rewrite_html_links

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"

ABS_SITE = "https://thehamdiz.github.io/super-lang"


def abs_url(slug: str) -> str:
    return canonical_url(slug)


def fix_meta_urls(html: str, slug: str) -> str:
    """Point canonical/og at pretty URLs."""
    url = abs_url(slug)
    html = re.sub(
        r'<link rel="canonical" href="[^"]+"/>',
        f'<link rel="canonical" href="{url}"/>',
        html,
        count=1,
    )
    html = re.sub(
        r'<meta property="og:url" content="[^"]+"/>',
        f'<meta property="og:url" content="{url}"/>',
        html,
        count=1,
    )
    for stem in (
        "quickstart",
        "language",
        "benchmarks",
        "install",
        "spec",
        "charts",
        "winners",
        "book",
        "examples",
        "report",
        "docs-hub",
        "stdlib-ref",
        "data-stacks",
        "syntax",
        "platform",
        "comptime",
        "macros-pods",
        "law-proof",
        "testing",
        "versioning",
        "install-security",
    ):
        html = html.replace(f"{ABS_SITE}/{stem}.html", f"{ABS_SITE}/{stem}/")
    return html


def main() -> int:
    flat = sorted(DOCS.glob("*.html"))
    for path in flat:
        if path.name == "index.html":
            text = path.read_text(encoding="utf-8")
            text = rewrite_html_links(text, "")
            text = fix_meta_urls(text, "home")
            path.write_text(text, encoding="utf-8")
            continue
        slug = path.stem
        dest_dir = DOCS / slug
        dest_dir.mkdir(parents=True, exist_ok=True)
        text = path.read_text(encoding="utf-8")
        text = fix_html_for_subdir(text, slug)
        text = fix_meta_urls(text, slug)
        (dest_dir / "index.html").write_text(text, encoding="utf-8")
        path.unlink()
        print(f"moved {path.name} -> {slug}/index.html")

    # Root assets: fix any stragglers in _includes
    inc_nav = DOCS / "_includes" / "nav.html"
    if inc_nav.is_file():
        t = inc_nav.read_text(encoding="utf-8")
        t = t.replace("'/index.html'", "'/'")
        t = t.replace("'/language.html'", "'/language/'")
        t = t.replace("'/benchmarks.html'", "'/benchmarks/'")
        for s in (
            "winners",
            "charts",
            "spec",
            "book",
            "examples",
            "install",
        ):
            t = t.replace(f"'/{s}.html'", f"'/{s}/'")
        inc_nav.write_text(t, encoding="utf-8")

    footer = DOCS / "_includes" / "footer.html"
    if footer.is_file():
        t = footer.read_text(encoding="utf-8")
        t = rewrite_html_links(t, "")
        footer.write_text(t, encoding="utf-8")

    # sitemap
    sm = DOCS / "sitemap.xml"
    if sm.is_file():
        t = sm.read_text(encoding="utf-8")
        t = re.sub(r"\.html</loc>", "/</loc>", t)
        t = t.replace(f"{ABS_SITE}//", f"{ABS_SITE}/")
        sm.write_text(t, encoding="utf-8")

    # llms
    for name in ("llms.txt", "llms-full.txt"):
        p = DOCS / name
        if p.is_file():
            t = p.read_text(encoding="utf-8")
            for stem in (
                "quickstart",
                "syntax",
                "spec",
                "stdlib-ref",
                "law-proof",
                "data-stacks",
                "benchmarks",
            ):
                t = t.replace(f"{ABS_SITE}/{stem}.html", f"{ABS_SITE}/{stem}/")
            p.write_text(t, encoding="utf-8")

    # benchmarks mirror
    bench = DOCS / "benchmarks" / "index.html"
    rep = ROOT / "benchmarks" / "report.html"
    if bench.is_file() and rep.parent.is_dir():
        shutil.copy2(bench, rep)

    print("pretty-url migration done")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())