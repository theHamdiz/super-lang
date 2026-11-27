#!/usr/bin/env python3
"""Integration checks for the top-level docs/ Jekyll + Pages site layout."""
from __future__ import annotations

import unittest
from pathlib import Path

import report_page as rp

REPO = Path(__file__).resolve().parent.parent
DOCS = REPO / "docs"


def _config_text() -> str:
    return (DOCS / "_config.yml").read_text(encoding="utf-8")


class JekyllScaffoldTests(unittest.TestCase):
    def test_docs_is_www_target(self) -> None:
        self.assertEqual(rp.WWW.resolve(), DOCS.resolve())
        legacy = REPO / "benchmarks" / "www"
        self.assertTrue(legacy.is_symlink() or legacy.resolve() == DOCS.resolve())

    def test_config_exists_and_parses(self) -> None:
        cfg_path = DOCS / "_config.yml"
        self.assertTrue(cfg_path.is_file(), "_config.yml missing")
        text = _config_text()
        self.assertRegex(text, r"(?m)^title:\s*Super\s*$")
        self.assertIn("remote_theme:", text)
        self.assertIn("pages-themes/primer", text)
        self.assertIn("jekyll-seo-tag", text)
        self.assertRegex(text, r'(?m)^baseurl:\s*"/super-lang"\s*$')

    def test_key_includes_exist(self) -> None:
        for name in ("head.html", "nav.html", "footer.html"):
            path = DOCS / "_includes" / name
            self.assertTrue(path.is_file(), f"missing include {name}")
            text = path.read_text(encoding="utf-8")
            self.assertGreater(len(text.strip()), 40)

    def test_nav_include_lists_product_routes(self) -> None:
        nav = (DOCS / "_includes" / "nav.html").read_text(encoding="utf-8")
        for needle in (
            "'/'",
            "/benchmarks/",
            "/winners/",
            "/charts/",
            "/book/",
            "/install/",
        ):
            self.assertIn(needle, nav)
        self.assertNotIn("Language", nav)

    def test_layouts_exist(self) -> None:
        default = (DOCS / "_layouts" / "default.html").read_text(encoding="utf-8")
        page = (DOCS / "_layouts" / "page.html").read_text(encoding="utf-8")
        self.assertIn("include", default)
        self.assertIn("layout: default", page)
        self.assertIn("{{ content }}", page)

    def test_default_layout_wires_chrome(self) -> None:
        layout = (DOCS / "_layouts" / "default.html").read_text(encoding="utf-8")
        self.assertIn("head.html", layout)
        self.assertIn("nav.html", layout)
        self.assertIn("footer.html", layout)

    def test_sass_and_gemfile(self) -> None:
        self.assertTrue((DOCS / "_sass" / "_variables.scss").is_file())
        self.assertTrue((DOCS / "_sass" / "site.scss").is_file())
        self.assertTrue((DOCS / "Gemfile").is_file())
        gem = (DOCS / "Gemfile").read_text(encoding="utf-8")
        self.assertIn("github-pages", gem)

    def test_guide_uses_jekyll_layout(self) -> None:
        guide = DOCS / "guide.md"
        self.assertTrue(guide.is_file())
        text = guide.read_text(encoding="utf-8")
        self.assertTrue(text.startswith("---"))
        self.assertIn("layout:", text)
        self.assertIn("permalink: /guide.html", text)


class GeneratorPublishPathTests(unittest.TestCase):
    def test_site_html_pages_include_winners_charts(self) -> None:
        self.assertIn("winners/index.html", rp.SITE_HTML_PAGES)
        self.assertIn("charts/index.html", rp.SITE_HTML_PAGES)

    def test_nav_html_matches_jekyll_routes(self) -> None:
        # report_page may still emit legacy .html hrefs; Jekyll include is source of truth.
        nav = (DOCS / "_includes" / "nav.html").read_text(encoding="utf-8")
        for href in ("/winners/", "/charts/", "/book/", "/"):
            self.assertIn(href, nav)
        footer = rp.footer_html()
        self.assertTrue("winners" in footer and "charts" in footer)

    def test_render_winners_and_charts_smoke(self) -> None:
        report = {
            "overall_winner": "super_aot",
            "category_winners": {"speed": "super_aot", "memory": "c"},
            "composite_scores": {
                "super_aot": {
                    "score": 90.0,
                    "speed": 90,
                    "memory": 80,
                    "reliability": 100,
                    "cycle": 70,
                    "loc": 60,
                    "contrib": {},
                }
            },
            "score_weights": {
                "speed": 0.4,
                "memory": 0.25,
                "reliability": 0.15,
                "cycle": 0.1,
                "loc": 0.1,
            },
            "machine": {"hostname": "test", "cpus": 8},
            "generated_at": "test",
        }
        winners = rp.render_winners(report)
        charts = rp.render_charts(report)
        self.assertIn("hero-board", winners)
        self.assertIn("SuperAot", winners)
        self.assertIn("site-nav", winners)
        self.assertIn('id="rank-chart"', charts)
        self.assertIn("report-data", charts)
        self.assertIn("winners.html", winners)
        self.assertIn("charts.html", charts)


if __name__ == "__main__":
    unittest.main()
