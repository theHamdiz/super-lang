"""Pretty URL helpers for docs/ (GitHub Pages directory indexes)."""
from __future__ import annotations

from pathlib import Path

SITE_ORIGIN = "https://thehamdiz.github.io/super-lang"


def canonical_url(slug: str) -> str:
    if slug in ("", "home"):
        return f"{SITE_ORIGIN}/"
    return f"{SITE_ORIGIN}/{slug}/"


def page_href(from_slug: str, to_slug: str, fragment: str = "") -> str:
    """Relative href between pretty doc pages (trailing slashes)."""
    frag = f"#{fragment}" if fragment else ""
    if to_slug in ("", "home"):
        return ("../" if from_slug else "./") + frag
    if from_slug == to_slug:
        return "./" + frag
    prefix = "../" if from_slug else ""
    return f"{prefix}{to_slug}/{frag}"


def asset_href(from_slug: str, rel: str) -> str:
    prefix = "../" if from_slug else ""
    return prefix + rel


def fix_html_for_subdir(html: str, slug: str) -> str:
    """Rewrite root-relative assets and .html links after moving to slug/index.html."""
    if not slug:
        return rewrite_html_links(html, "")
    asset = asset_href(slug, "")
    for pat, repl in [
        ('href="site.css"', f'href="{asset_href(slug, "site.css")}"'),
        ('href="assets/', f'href="{asset}assets/'),
        ('src="super-highlight.js"', f'src="{asset_href(slug, "super-highlight.js")}"'),
        ('src="book-flip.js"', f'src="{asset_href(slug, "book-flip.js")}"'),
        ('href="downloads/', f'href="{asset}downloads/'),
        ('fetch("book-pages.json")', f'fetch("{asset_href(slug, "book-pages.json")}")'),
    ]:
        html = html.replace(pat, repl)
    html = rewrite_html_links(html, slug)
    # Benchmark pages are rendered with already-pretty relative links. Once a
    # page moves under `slug/index.html`, sibling slugs need a parent prefix.
    for stem in (
        "benchmarks",
        "winners",
        "charts",
        "spec",
        "book",
        "examples",
        "install",
    ):
        target = "./" if stem == slug else f"../{stem}/"
        html = html.replace(f'href="{stem}/"', f'href="{target}"')
    return html.replace('href="./">Home', 'href="../">Home')


def fix_meta_urls(html: str, slug: str) -> str:
    import re

    url = canonical_url(slug)
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
    ):
        html = html.replace(f"{SITE_ORIGIN}/{stem}.html", f"{SITE_ORIGIN}/{stem}/")
    return html


def write_pretty_page(base: Path, slug: str, html: str) -> Path:
    """Write docs/[slug]/index.html or docs/index.html with asset + link fixes."""
    if slug:
        dest_dir = base / slug
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / "index.html"
        text = fix_html_for_subdir(html, slug)
        text = fix_meta_urls(text, slug)
    else:
        dest = base / "index.html"
        text = rewrite_html_links(html, "")
        text = fix_meta_urls(text, "home")
    dest.write_text(text, encoding="utf-8")
    return dest


def rewrite_html_links(html: str, from_slug: str) -> str:
    import re

    def sub(m: re.Match[str]) -> str:
        target = m.group(1)
        frag = (m.group(2) or "").lstrip("#")
        if target == "index.html":
            return f'href="{page_href(from_slug, "home", frag)}"'
        if target.endswith(".html"):
            slug = target[: -len(".html")]
            return f'href="{page_href(from_slug, slug, frag)}"'
        return m.group(0)

    html = re.sub(r'href="([^"]+\.html)(#[^"]*)?"', sub, html)
    return html