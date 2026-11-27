#!/usr/bin/env python3
"""Unit/integration tests for the Super site / benchmark report generator."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import report_page as rp


class LandingExcerptTests(unittest.TestCase):
    def test_home_unifies_nav_and_excerpt(self) -> None:
        html = rp.render_home()
        self.assertEqual(html.count('class="site-nav"'), 1)
        self.assertIn('class="landing-excerpt"', html)
        self.assertIn('id="what-is-super"', html)
        self.assertNotIn('<h2>What is Super</h2>', html)
        self.assertNotIn('class="page-hero"', html)
        self.assertIn("data-copy-target", html)
        self.assertIn("Super for APIs and ECS", html)

    def test_language_redirects_to_home(self) -> None:
        lang = rp.render_language()
        self.assertIn('url=../', lang)
        self.assertIn('href="../"', lang)


class DisplayNameTests(unittest.TestCase):
    def test_known_camel(self) -> None:
        self.assertEqual(rp.display_name("super_aot"), "SuperAot")
        self.assertEqual(rp.display_name("super_jit"), "SuperJit")
        self.assertEqual(rp.display_name("c"), "C")
        self.assertEqual(rp.display_name("python"), "Python")

    def test_snake_fallback(self) -> None:
        self.assertEqual(rp.display_name("foo_bar_baz"), "FooBarBaz")


class TimingsMatrixTests(unittest.TestCase):
    def test_matrix_orders_targets(self) -> None:
        algos = [
            {
                "name": "01_nbody",
                "targets": [
                    {"target": "c", "status": "ok", "stats": {"mean": 1.0}},
                    {"target": "super_aot", "status": "ok", "stats": {"mean": 0.5}},
                ],
            }
        ]
        targets, rows = rp.build_timings_matrix(algos)
        self.assertEqual(targets, ["c", "super_aot"])
        self.assertEqual(rows[0]["means"]["super_aot"], 0.5)


class ExtractBookPagesTests(unittest.TestCase):
    def test_missing_pdf_returns_stub(self) -> None:
        # BOOK.md in the repo is preferred; stub only when markdown + PDF absent
        data = rp.extract_book_pages(Path("/tmp/does-not-exist-super-book.pdf"), max_pages=2)
        self.assertIn("pages", data)
        self.assertGreaterEqual(len(data["pages"]), 1)
        self.assertEqual(data["title"], "The Super Programming Language")

    def test_prefers_book_markdown_with_html_and_code(self) -> None:
        data = rp.extract_book_pages(Path("/tmp/does-not-exist-super-book.pdf"), max_pages=12)
        self.assertGreaterEqual(len(data["pages"]), 2)
        self.assertEqual(data["pages"][0].get("kind"), "cover")
        self.assertIn("book-cover", data["pages"][0].get("html", ""))
        joined = "\n".join(p.get("html", "") for p in data["pages"])
        self.assertIn("section-cover", joined)
        self.assertTrue(
            "language-sr" in joined or "sr-block" in joined or "```" in "\n".join(
                "\n".join(p.get("lines") or []) for p in data["pages"]
            ),
            "expected Super code fences in book pages",
        )
        self.assertTrue(any(p.get("html") for p in data["pages"]))

    def test_parse_and_paginate_markdown_helpers(self) -> None:
        md = (
            "# Programming in Super\n\nIntro deck.\n\n"
            "# Part I — Why Super Exists\n\nMental model.\n\n"
            "## 1. Why Super exists\n\nHello `for` world.\n\n"
            "```super\n+add(a: i64) => a\n```\n\n- one\n- two\n"
        )
        blocks = rp.parse_markdown_blocks(md)
        kinds = [b["kind"] for b in blocks]
        self.assertIn("h1", kinds)
        self.assertIn("code", kinds)
        self.assertIn("list", kinds)
        pages = rp.paginate_markdown_blocks(blocks, max_pages=10, page_budget=20)
        self.assertGreaterEqual(len(pages), 2)
        self.assertEqual(pages[0]["kind"], "cover")
        self.assertTrue(any(p.get("kind") == "section" for p in pages))
        body_html = "\n".join(p["html"] for p in pages if p.get("kind") == "body")
        self.assertIn("language-sr", body_html)

    def test_build_from_markdown_file(self) -> None:
        book = rp.REPO / "BOOK.md"
        if not book.is_file():
            self.skipTest("BOOK.md missing")
        data = rp.build_book_pages_from_markdown(book, max_pages=8)
        assert data is not None
        self.assertEqual(len(data["pages"]), 8)
        self.assertEqual(data["pages"][0]["kind"], "cover")
        self.assertIn("book-cover", data["pages"][0]["html"])

    def test_full_book_has_pdf_toc_coverage(self) -> None:
        book = rp.REPO / "BOOK.md"
        if not book.is_file():
            self.skipTest("BOOK.md missing")
        data = rp.build_book_pages_from_markdown(book, max_pages=None)
        assert data is not None
        self.assertGreater(len(data["pages"]), 100)
        blob = "\n".join(
            (p.get("html") or "") + "\n" + "\n".join(p.get("lines") or [])
            for p in data["pages"]
        )
        for needle in (
            "Part I",
            "Part II",
            "Part III",
            "Part IV",
            "Part V",
            "Part VI",
            "Appendix A",
            "Appendix B",
            "Appendix C",
            "Project 10",
            "Tutorial 1",
            "language-sr",
        ):
            self.assertIn(needle, blob, f"missing {needle} in full book pages")
        self.assertEqual(data["pages"][0]["kind"], "cover")
        self.assertGreaterEqual(
            sum(1 for p in data["pages"] if p.get("kind") == "section"), 6
        )

    def test_pdf_pipeline_gets_section_covers(self) -> None:
        raw = [
            {"n": 1, "lines": ["SUPER", "The Super Programming Language"], "html": ""},
            {
                "n": 2,
                "lines": ["Part I — Why Super Exists", "Scoped stacks and O(1) rewind."],
                "html": "",
            },
            {"n": 3, "lines": ["1. Why Super exists", "Ceremony is what was removed."], "html": ""},
            {"n": 4, "lines": ["Ordinary body text about stacks and cursors."], "html": ""},
        ]
        pages = rp.decorate_pages_with_covers(raw, max_pages=12)
        self.assertEqual(pages[0]["kind"], "cover")
        kinds = [p["kind"] for p in pages]
        self.assertIn("section", kinds)
        self.assertGreaterEqual(sum(1 for k in kinds if k == "section"), 2)

    def test_extract_from_real_pdf_when_present(self) -> None:
        pdf = rp.WWW / "downloads" / "SUPER_BOOK.pdf"
        data = rp.extract_book_pages(pdf, max_pages=6)
        self.assertGreaterEqual(len(data["pages"]), 1)
        self.assertEqual(data["pages"][0].get("kind"), "cover")
        self.assertTrue(any(data["pages"][0].get("lines") or data["pages"][0].get("html")))


class BookEscapingTests(unittest.TestCase):
    def test_inline_md_quotes_not_double_escaped(self) -> None:
        html = rp._inline_md('say `"hello"` and "world"')
        self.assertNotIn("&amp;quot;", html)
        self.assertNotIn("&amp;#x27;", html)
        # Real quotes survive in prose / code; entities at most once
        self.assertIn('"hello"', html)
        self.assertIn('"world"', html)
        self.assertNotIn("&quot;", html)

    def test_code_block_quotes_are_real_characters(self) -> None:
        block = {"kind": "code", "lang": "super", "text": '+greet() = "hello"'}
        html = rp.render_md_block(block)
        self.assertIn('"hello"', html)
        self.assertNotIn("&quot;", html)
        self.assertNotIn("&amp;quot;", html)

    def test_paginated_page_with_quotes_has_no_literal_entity(self) -> None:
        md = (
            "# Quote page\n\n"
            'A line with `"hello"` ticks and "plain" quotes.\n\n'
            '```super\n+greet(name: str) = "Hello, {name}!"\n```\n'
        )
        pages = rp.paginate_markdown_blocks(
            rp.parse_markdown_blocks(md), max_pages=8, page_budget=40
        )
        blob = "\n".join(p.get("html") or "" for p in pages)
        self.assertNotIn("&amp;quot;", blob)
        self.assertIn('"hello"', blob)
        self.assertIn('"Hello, {name}!"', blob)
        # Simulate DOM textContent path: html.unescape once must not leave &quot;
        import html as html_mod

        text = html_mod.unescape(blob)
        self.assertNotIn("&quot;", text)
        self.assertIn('"hello"', text)


class BookFlipJsTests(unittest.TestCase):
    def test_pager_is_instant_no_curl(self) -> None:
        js = rp.book_flip_js()
        self.assertIn("ArrowRight", js)
        self.assertIn("book-pages.json", js)
        self.assertIn("book-spread", js)
        self.assertIn('mode: "instant-pager"', js)
        # No 3D / spring animation path
        self.assertNotIn("springTo", js)
        self.assertNotIn("applyCurl", js)
        self.assertNotIn("rotateY", js)
        self.assertNotIn("prepareFlip", js)
        self.assertNotIn("clearCurlChrome", js)
        # Instant turn commits idx then paintSpread
        self.assertIn("function turn(dir)", js)
        self.assertIn("paintSpread()", js)

    def test_flip_js_page_cache_window(self) -> None:
        js = rp.book_flip_js()
        self.assertIn("CACHE_RADIUS = 10", js)
        self.assertIn("BookPageCache", js)
        self.assertIn("BookFlipDebug", js)
        self.assertIn("prefetchAround", js)
        self.assertIn("attachCached", js)
        self.assertIn("frozenHtml", js)
        self.assertIn("assertMultiFlipIdentity", js)
        self.assertIn("Object.freeze(entry)", js)
        drop = js[js.find("dropOutside") : js.find("prefetchAround")]
        self.assertIn("dropOutside", drop)
        self.assertNotIn("_map.delete", drop)

    def test_site_css_cascadia_and_large_stage(self) -> None:
        css = rp.site_css()
        self.assertIn("Cascadia Mono", css)
        self.assertIn("Cascadia Code", css)
        self.assertIn("JetBrains Mono", css)
        self.assertIn("--font-mono", css)
        self.assertIn("assets/fonts/CascadiaMono-Regular.woff2", css)
        # Stage fills ~90% of reader-stage (was 80% with 10% inset)
        self.assertIn("width: 90%", css)
        self.assertIn("height: 90%", css)
        self.assertIn("top: 5%", css)
        self.assertIn("left: 5%", css)
        # Inline ticks visibly chipped
        self.assertIn("code.sr-inline", css)
        self.assertIn("background: #e4d8c8", css)


class BenchmarksJsTests(unittest.TestCase):
    def test_chart_and_radar_and_canvas(self) -> None:
        js = rp.benchmarks_js()
        self.assertIn("drawChart", js)
        self.assertIn("drawRadar", js)
        self.assertIn("score-race", js)
        self.assertIn("selectAxis", js)
        self.assertIn("category", js)


class RenderSiteTests(unittest.TestCase):
    def setUp(self) -> None:
        self.report = {
            "generated_at": "2026-11-25T00:00:00",
            "machine": {"hostname": "hq", "cpus": 32},
            "overall_winner": "super_aot",
            "score_weights": {
                "speed": 0.4,
                "memory": 0.25,
                "reliability": 0.15,
                "cycle": 0.1,
                "loc": 0.1,
            },
            "category_winners": {
                "speed": "super_aot",
                "memory": "c",
                "reliability": "c",
                "cycle": "super_aot",
                "loc": "super_aot",
            },
            "composite_scores": {
                "super_aot": {
                    "score": 99.2,
                    "speed": 100,
                    "memory": 98,
                    "reliability": 100,
                    "cycle": 96,
                    "loc": 100,
                    "contrib": {
                        "speed": 40,
                        "memory": 24.5,
                        "reliability": 15,
                        "cycle": 9.6,
                        "loc": 10,
                    },
                },
                "c": {
                    "score": 38.2,
                    "speed": 40,
                    "memory": 100,
                    "reliability": 100,
                    "cycle": 50,
                    "loc": 40,
                    "contrib": {},
                },
            },
            "algorithms": [
                {
                    "name": "13_polars_data_pipeline",
                    "gate": "ok",
                    "reference_checksum": "abc",
                    "targets": [
                        {
                            "target": "super_aot",
                            "status": "ok",
                            "stats": {"mean": 0.12},
                            "checksum": "abc",
                        },
                        {
                            "target": "c",
                            "status": "ok",
                            "stats": {"mean": 0.18},
                            "checksum": "abc",
                        },
                    ],
                }
            ],
            "toolchains": [{"name": "rustc", "available": True, "version": "1.85"}],
            "loc": {"by_language": {"super": {"code": 80}, "c": {"code": 100}}},
        }

    def test_benchmarks_html_has_camelcase_and_timings(self) -> None:
        html = rp.render_benchmarks(self.report)
        self.assertIn("SuperAot", html)
        self.assertNotIn(">super_aot<", html)
        self.assertIn('id="timings"', html)
        self.assertIn('id="timings-table"', html)
        self.assertIn("timing-cards", html)
        self.assertIn("Overall composite leader, measured on a single machine", html)
        self.assertIn("Category winners", html)
        self.assertIn("hero-board", html)
        self.assertIn("Score receipt", html)  # kept in score section, not header
        self.assertNotIn('id="receipt-hero"', html)
        self.assertNotIn("assets/ui/lang-poster.png", html)
        self.assertNotIn("assets/ui/book-cover.png", html)
        self.assertNotIn("assets/ui/timings-bars.png", html)
        self.assertNotIn("assets/ui/weights-field.png", html)
        self.assertNotIn("assets/ui/book-viewer.png", html)
        self.assertNotIn("assets/ui/receipt-crop.png", html)
        self.assertIn("Wall time", html)
        self.assertIn("site-nav", html)
        self.assertIn("footer-mark", html)
        self.assertNotIn("footer-cols", html)
        self.assertNotIn("informatics-general-programmer", html)
        self.assertNotIn("wikimedia", html.lower())
        self.assertIn('id="rank-chart"', html)
        self.assertIn('id="score-race"', html)
        self.assertIn('id="radar-chart"', html)
        self.assertIn("cat-tile", html)
        self.assertNotIn("Why was CPU removed", html)
        self.assertIn("winner-chip", html)
        self.assertIn("toolchain-grid", html)

    def test_book_is_flip_reader_not_screenshot(self) -> None:
        html = rp.render_book()
        self.assertIn("book-flip.js", html)
        self.assertIn('id="reader"', html)
        self.assertIn('id="book-spread"', html)
        self.assertIn("face front", html)
        self.assertIn("under-left", html)
        self.assertIn("cover-card", html)
        self.assertNotIn("book-cover.png", html)
        self.assertNotIn("book-viewer.png", html)
        self.assertNotIn("application/pdf", html)  # no embedded PDF object as primary UI
        self.assertIn("SUPER_BOOK.pdf", html)

    def test_no_screenshot_imgs_on_secondary_pages(self) -> None:
        for html in (
            rp.render_examples(),
            rp.render_spec(),
            rp.render_install(),
        ):
            self.assertNotIn("<img ", html)
            self.assertNotIn("Safe Rust", html)
            self.assertIn("footer-mark", html)
            self.assertIn("site-nav", html)

    def test_render_site_writes_pages(self) -> None:
        pages = {
            "language": rp.render_language(),
            "book": rp.render_book(),
            "install": rp.render_install(),
            "examples": rp.render_examples(),
            "spec": rp.render_spec(),
            "winners": rp.render_winners(self.report),
            "charts": rp.render_charts(self.report),
            "bench": rp.render_benchmarks(self.report),
        }
        for name, html in pages.items():
            if name == "language":
                self.assertIn("url=../", html)
                continue
            self.assertIn("site-nav", html)
            self.assertIn("Download book", html)
            self.assertIn("footer-mark", html)
            self.assertTrue("winners" in html and "charts" in html)
        self.assertIn("SUPER_BOOK.pdf", pages["book"])
        self.assertIn("language-sr", pages["examples"])
        self.assertIn("super-highlight.js", pages["examples"])
        self.assertIn("language-sr", pages["spec"])
        self.assertNotIn("Why was CPU removed", pages["bench"])
        self.assertNotIn("Safe Rust", pages["language"])
        self.assertIn("SuperAot", pages["bench"])
        self.assertIn("timings-table", pages["bench"])
        self.assertIn("hero-board", pages["winners"])
        self.assertIn("rank-chart", pages["charts"])
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp)
            old_www = rp.WWW
            try:
                rp.WWW = dest / "www"
                (rp.WWW / "downloads").mkdir(parents=True, exist_ok=True)
                # stub pdf absence is fine — extract_book_pages handles it
                out = rp.render_site(self.report, dest / "report.html", sync=False)
                self.assertTrue(out.is_file())
                self.assertTrue((rp.WWW / "index.html").is_file())
                self.assertTrue((rp.WWW / "site.css").is_file())
                # Pretty URLs: book/winners/charts live under slug/index.html
                book = (rp.WWW / "book.html") if (rp.WWW / "book.html").is_file() else (rp.WWW / "book" / "index.html")
                winners = (rp.WWW / "winners.html") if (rp.WWW / "winners.html").is_file() else (rp.WWW / "winners" / "index.html")
                charts = (rp.WWW / "charts.html") if (rp.WWW / "charts.html").is_file() else (rp.WWW / "charts" / "index.html")
                self.assertTrue(book.is_file())
                self.assertTrue(winners.is_file())
                self.assertTrue(charts.is_file())
                self.assertTrue((rp.WWW / "book-flip.js").is_file())
                self.assertTrue((rp.WWW / "book-pages.json").is_file())
                pages_json = json.loads((rp.WWW / "book-pages.json").read_text())
                self.assertIn("pages", pages_json)
            finally:
                rp.WWW = old_www

    def test_site_css_responsive_table_strategy(self) -> None:
        css = rp.site_css()
        self.assertIn("#timings-table { display: none }", css)
        self.assertIn(".timing-cards { display: grid }", css)
        self.assertIn("footer-mark", css)
        self.assertIn("hero-board", css)
        self.assertIn("page.flipping", css)
        self.assertIn("winner-chip", css)
        self.assertIn("tok-kw", css)
        self.assertIn("toolchain-grid", css)
        self.assertIn("geo-mandala.svg", css)
        self.assertIn("geo-star.svg", css)
        self.assertIn("mask-image", css)
        self.assertIn(".tile-winner::before", css)
        self.assertIn(".cat-tile::before", css)
        self.assertIn("width: 90%", css)
        self.assertIn("height: 90%", css)
        self.assertIn(".sheet.hard", css)
        self.assertIn(".book-cover", css)
        self.assertIn(".section-cover", css)
        self.assertIn("color: #050f16", css)
        self.assertIn(".page-inner code.sr-inline .tok-kw", css)
        self.assertIn(".page-inner .sr-block .tok-string", css)


class PublicCopyTests(unittest.TestCase):
    """Public pages must read like a product site, not a local lab notebook."""

    BANNED = (
        "on this machine",
        "press one",
        "sit on the board",
        "Tools on the host",
        "Same machine as the scoreboard",
        "Winner pulse",
        "flip reader",
        "Apple Books",
        "<code>docs/REPORT.md</code>",
        ">spec.html<",
        "Opens a new shell",
        "Live scoreboard",
    )

    def setUp(self) -> None:
        self.report = RenderSiteTests("test_benchmarks_html_has_camelcase_and_timings")
        self.report.setUp()
        self.report = self.report.report
        self.pages = {
            "bench": rp.render_benchmarks(self.report),
            "winners": rp.render_winners(self.report),
            "charts": rp.render_charts(self.report),
            "home": rp.render_home(),
            "install": rp.render_install(),
        }

    def test_no_lab_note_phrasing(self) -> None:
        for name, html in self.pages.items():
            for phrase in self.BANNED:
                self.assertNotIn(phrase, html, f"{name}: {phrase!r}")

    def test_single_machine_caveat_is_kept(self) -> None:
        self.assertIn("measured on a single machine", self.pages["bench"])
        self.assertIn("measured on a single machine", self.pages["winners"])
        self.assertIn("All timings come from one machine", self.pages["bench"])
        self.assertIn("single machine", self.pages["home"])

    def test_filter_interaction_is_explained_plainly(self) -> None:
        self.assertIn("Select a category winner to re-rank the chart", self.pages["bench"])
        self.assertIn("axis buttons to re-rank the chart", self.pages["charts"])

    def test_winner_tiles_link_into_chart_on_winners_page(self) -> None:
        winners = self.pages["winners"]
        self.assertIn('<a class="cat-tile" href="../charts/?axis=speed#live-chart"', winners)
        self.assertNotIn('<button type="button" class="cat-tile"', winners)
        # benchmarks page keeps in-place buttons driven by the chart script
        self.assertIn('<button type="button" class="cat-tile"', self.pages["bench"])
        self.assertIn('get("axis")', self.pages["charts"])

    def test_no_internal_hostname_in_pages(self) -> None:
        for name, html in self.pages.items():
            self.assertNotIn(">hq<", html, name)


class SiteUrlsFragmentTests(unittest.TestCase):
    def test_fragment_is_not_doubled(self) -> None:
        sys_path = str(rp.REPO / "scripts")
        import sys

        if sys_path not in sys.path:
            sys.path.insert(0, sys_path)
        from site_urls import rewrite_html_links

        out = rewrite_html_links('<a href="benchmarks.html#toolchain">x</a>', "winners")
        self.assertIn('href="../benchmarks/#toolchain"', out)
        self.assertNotIn("##", out)
        self.assertIn('href="./#top"', rewrite_html_links('<a href="index.html#top">x</a>', ""))


class SyncWwwTests(unittest.TestCase):
    def test_sync_copies_book_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            www = Path(tmp) / "www"
            live = Path(tmp) / "live"
            www.mkdir()
            (www / "index.html").write_text("ok")
            (www / "site.css").write_text("body{}")
            (www / "book-flip.js").write_text("/*flip*/")
            (www / "book-pages.json").write_text('{"pages":[]}')
            old = rp.WWW
            try:
                rp.WWW = www
                out = rp.sync_www(live)
                self.assertEqual(out, live)
                self.assertTrue((live / "book-flip.js").is_file())
                self.assertTrue((live / "book-pages.json").is_file())
            finally:
                rp.WWW = old


def _node_highlight_sr(src: str) -> str:
    """Run site Super highlighter via Node (CommonJS export)."""
    import subprocess

    js_path = rp.SITE_ASSETS / "super-highlight.js"
    proc = subprocess.run(
        [
            "node",
            "-e",
            "const hl=require(process.argv[1]); process.stdout.write(hl.highlightSr(process.argv[2]));",
            str(js_path),
            src,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return proc.stdout


def _number_token_contents(html: str) -> list[str]:
    import re

    return re.findall(r'<span class="tok-number">([^<]*)</span>', html)


class HighlightAndPublishTests(unittest.TestCase):
    def test_highlight_js_covers_super_tokens(self) -> None:
        js = rp.super_highlight_js()
        self.assertIn("tok-kw", js)
        self.assertIn("tok-macro", js)
        self.assertIn("tok-op", js)
        self.assertIn("highlightInlineTicks", js)
        self.assertIn("renderPlainPage", js)
        self.assertIn("highlightGeneric", js)

    def test_highlight_range_rhs_call_not_truncated(self) -> None:
        """Regression: number scanner must not swallow `0..a` from `0..len(xs)`."""
        for src in (
            "for 0..len(xs) { }",
            "for 0..foo(bar.baz) { }",
            "0..n",
            "xs[0..len(xs)]",
        ):
            html = _node_highlight_sr(src)
            # Full source text must survive highlighting (modulo HTML escapes).
            plain = (
                html.replace('<span class="tok-kw">', "")
                .replace('<span class="tok-number">', "")
                .replace('<span class="tok-op">', "")
                .replace('<span class="tok-type">', "")
                .replace('<span class="tok-string">', "")
                .replace('<span class="tok-macro">', "")
                .replace('<span class="tok-mut">', "")
                .replace('<span class="tok-vis">', "")
                .replace('<span class="tok-comment">', "")
                .replace("</span>", "")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
            )
            self.assertEqual(plain, src, f"round-trip failed for {src!r}: {html}")
            self.assertIn('class="tok-op">..</span>', html, html)
            for num in _number_token_contents(html):
                self.assertNotIn("..", num, f"number token ate range in {src!r}: {html}")
            # RHS call / field must appear after the range op, not truncated mid-ident.
            if "len(" in src:
                self.assertIn("len", html)
                self.assertNotIn('tok-number">0..a</span>', html)
            if "foo(bar.baz)" in src:
                self.assertIn("foo", html)
                self.assertIn("bar", html)
                self.assertIn("baz", html)

    def test_scrub_results_for_docs_strips_abs_paths(self) -> None:
        payload = {
            "toolchains": [
                {"name": "super", "path": str(rp.REPO / "target/release/super")},
                {"name": "bun", "path": str(Path.home() / ".bun/bin/bun")},
                {"name": "cc", "path": "/usr/bin/cc"},
            ],
            "previous_run": {"baseline_path": str(rp.ROOT / "results.json")},
        }
        out = rp.scrub_results_for_docs(payload)
        self.assertEqual(out["toolchains"][0]["path"], "<repo>/target/release/super")
        self.assertEqual(out["toolchains"][1]["path"], "<home-path>")
        self.assertEqual(out["toolchains"][2]["path"], "/usr/bin/cc")
        self.assertEqual(out["previous_run"]["baseline_path"], "<repo>/benchmarks/results.json")

    def test_algo_winner_picks_fastest_wall(self) -> None:
        algo = {
            "name": "x",
            "targets": [
                {"target": "c", "status": "ok", "stats": {"mean": 1.0}, "peak_rss_kb": 100, "cycle_s": 1.0},
                {"target": "zig", "status": "ok", "stats": {"mean": 0.5}, "peak_rss_kb": 80, "cycle_s": 0.5},
            ],
        }
        self.assertEqual(rp.algo_overall_winner(algo), "zig")

    def test_algo_winner_ignores_cycle_skew(self) -> None:
        """Regression: algo 15-style — tiny cycle must not beat a faster wall."""
        algo = {
            "name": "15_physics2d_broadphase_solver",
            "targets": [
                {
                    "target": "super_jit",
                    "status": "ok",
                    "stats": {"median": 0.0536, "mean": 0.0536},
                    "peak_rss_kb": 5404,
                    "cycle_s": 0.054,
                },
                {
                    "target": "zig",
                    "status": "ok",
                    "stats": {"median": 0.0406, "mean": 0.0406},
                    "peak_rss_kb": 12160,
                    "cycle_s": 8.71,
                },
                {
                    "target": "c",
                    "status": "ok",
                    "stats": {"median": 0.0476, "mean": 0.0476},
                    "peak_rss_kb": 4536,
                    "cycle_s": 0.17,
                },
            ],
        }
        self.assertEqual(rp.algo_overall_winner(algo), "zig")

    def test_publish_mandara_writes_subtree(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "mandara"
            dest.mkdir()
            (dest / "docs").mkdir()
            (dest / "docs" / "README.md").write_text("# Docs\n")
            www = Path(tmp) / "www"
            www.mkdir()
            for name in ("index.html", "site.css", "book-flip.js", "super-highlight.js", "book-pages.json"):
                (www / name).write_text("x")
            old = rp.WWW
            try:
                rp.WWW = www
                out = rp.publish_mandara(dest)
                self.assertIsNotNone(out)
                self.assertTrue((dest / "super" / "index.html").is_file())
                self.assertTrue((dest / "super" / "_includes" / "head-custom.html").is_file())
                self.assertIn("super-site-link", (dest / "docs" / "README.md").read_text())
            finally:
                rp.WWW = old


if __name__ == "__main__":
    unittest.main()
