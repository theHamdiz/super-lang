#!/usr/bin/env python3
"""Regression tests for book generator chunking and table cells."""
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MOD_PATH = ROOT / "scripts" / "md_to_book_typst.py"
spec = importlib.util.spec_from_file_location("md_to_book_typst", MOD_PATH)
mod = importlib.util.module_from_spec(spec)
sys.modules["md_to_book_typst"] = mod
assert spec.loader is not None
spec.loader.exec_module(mod)


class ChunkLinesTests(unittest.TestCase):
    def test_medium_listing_stays_one_panel(self) -> None:
        body = "\n".join(f"line {i}" for i in range(42))
        self.assertEqual(mod.chunk_lines(body), [body])

    def test_short_listing_unchanged(self) -> None:
        body = "a\nb\nc"
        self.assertEqual(mod.chunk_lines(body), [body])

    def test_long_listing_splits_at_blank_lines(self) -> None:
        head = "\n".join(f"h{i}" for i in range(40))
        tail = "\n".join(f"t{i}" for i in range(30))
        body = f"{head}\n\n{tail}"
        chunks = mod.chunk_lines(body)
        self.assertGreater(len(chunks), 1)
        for ch in chunks:
            self.assertLessEqual(len(ch.split("\n")), mod.CHUNK_MAX)

    def test_long_dense_listing_hard_splits(self) -> None:
        body = "\n".join(f"line{i}" for i in range(120))
        chunks = mod.chunk_lines(body)
        self.assertGreater(len(chunks), 1)
        self.assertEqual(sum(len(c.split("\n")) for c in chunks), 120)

    def test_under_code_keep_never_splits(self) -> None:
        body = "\n".join("x" for _ in range(mod.CODE_KEEP))
        self.assertEqual(mod.chunk_lines(body), [body])


class TableCellTests(unittest.TestCase):
    def test_checksum_splits_at_digit_midpoint(self) -> None:
        headers = [
            "Kernel",
            "C (ms)",
            "Super AOT (ms)",
            "Super JIT (ms)",
            "AOT vs C",
            "JIT vs C",
            "Shared checksum",
        ]
        out = mod.format_table_cell("1422520636761", 6, headers)
        self.assertIn("142252", out)
        self.assertIn("0636761", out)
        self.assertIn("\\", out)


if __name__ == "__main__":
    unittest.main()