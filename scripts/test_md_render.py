#!/usr/bin/env python3
"""Tests for scripts/md_render.py and the pages generated with it."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from md_render import inline, render, slugify  # noqa: E402


class InlineTests(unittest.TestCase):
    def test_code_spans_are_rendered_and_escaped(self) -> None:
        self.assertEqual(inline("run `a < b`"), "run <code>a &lt; b</code>")

    def test_code_spans_protect_markdown(self) -> None:
        self.assertEqual(inline("`**x**` and **y**"), "<code>**x**</code> and <strong>y</strong>")

    def test_links_and_emphasis(self) -> None:
        self.assertEqual(
            inline("see [spec](spec.md) and *this*"),
            'see <a href="spec.md">spec</a> and <em>this</em>',
        )

    def test_multiplication_is_not_emphasis(self) -> None:
        self.assertEqual(inline("a * b * c"), "a * b * c")

    def test_double_backtick_spans(self) -> None:
        self.assertEqual(inline("``a ` b``"), "<code>a ` b</code>")


class BlockTests(unittest.TestCase):
    def test_all_heading_levels_render(self) -> None:
        out = render("# One\n## Two\n### Three\n#### Four")
        for level, text in enumerate(["One", "Two", "Three", "Four"], start=1):
            self.assertIn(f'<h{level} id="{text.lower()}">{text}</h{level}>', out)
        self.assertNotIn("<p>#", out)

    def test_heading_offset(self) -> None:
        self.assertIn("<h3", render("# Title", heading_offset=2))

    def test_paragraph_lines_join(self) -> None:
        self.assertEqual(render("one\ntwo\n\nthree"), "<p>one two</p>\n<p>three</p>")

    def test_fenced_super_code_is_escaped(self) -> None:
        out = render("```super\nx = a < b\n```")
        self.assertEqual(out, '<pre class="sr-block"><code class="language-sr">x = a &lt; b</code></pre>')

    def test_fenced_other_language(self) -> None:
        self.assertIn('<code class="language-bash">', render("```bash\nls\n```"))

    def test_lists(self) -> None:
        self.assertEqual(render("- a\n- `b`"), "<ul><li>a</li><li><code>b</code></li></ul>")
        self.assertEqual(render("1. a\n2. b"), "<ol><li>a</li><li>b</li></ol>")

    def test_list_continuation_lines(self) -> None:
        self.assertEqual(render("- a\n  more\n- b"), "<ul><li>a more</li><li>b</li></ul>")

    def test_tables(self) -> None:
        out = render("| A | B |\n|---|---|\n| `x` | 2 |")
        self.assertIn("<th>A</th><th>B</th>", out)
        self.assertIn("<td><code>x</code></td><td>2</td>", out)

    def test_block_quote_and_rule(self) -> None:
        self.assertEqual(render("> note"), "<blockquote><p>note</p></blockquote>")
        self.assertEqual(render("---"), "<hr/>")

    def test_slugify(self) -> None:
        self.assertEqual(slugify("3.9 Keywords & macros"), "39-keywords--macros")


class GeneratedPagesTests(unittest.TestCase):
    """The published quickstart and spec pages contain no raw Markdown."""

    def _page(self, slug: str) -> str:
        return (ROOT / "docs" / slug / "index.html").read_text(encoding="utf-8")

    def test_quickstart_has_no_raw_markdown(self) -> None:
        page = self._page("quickstart")
        self.assertNotIn("<p>#", page)
        self.assertNotIn("<p>- ", page)
        self.assertNotRegex(page, r"<p>[^<]*`[^<]*</p>")

    def test_spec_has_no_raw_markdown(self) -> None:
        page = self._page("spec")
        self.assertNotIn("<p>#", page)
        self.assertNotRegex(page, r"<p>[^<]*`[^<]*</p>")
        self.assertNotIn("<p>- ", page)


if __name__ == "__main__":
    unittest.main()
