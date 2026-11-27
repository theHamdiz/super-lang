#!/usr/bin/env python3
"""Convert Super Markdown docs to MDX-safe editions (brace escape outside fences)."""

from __future__ import annotations

import re
import sys
from pathlib import Path

FENCE_RE = re.compile(r"^(`{3,}|~{3,})")


def escape_prose_line(line: str) -> str:
    parts = re.split(r"(`[^`\n]*`)", line)
    out: list[str] = []
    for i, part in enumerate(parts):
        if i % 2 == 1:
            out.append(part)
        else:
            out.append(part.replace("{", "&#123;").replace("}", "&#125;"))
    return "".join(out)


def md_body_to_mdx(lines: list[str]) -> list[str]:
    in_fence = False
    out: list[str] = []
    for line in lines:
        if FENCE_RE.match(line.strip()):
            in_fence = not in_fence
            out.append(line)
            continue
        out.append(line if in_fence else escape_prose_line(line))
    return out


def read_frontmatter(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise ValueError(f"{path}: missing YAML frontmatter")
    end = text.find("\n---\n", 4)
    if end < 0:
        raise ValueError(f"{path}: unclosed frontmatter")
    block = text[: end + 5]
    return block.splitlines(keepends=True)


def convert(md_path: Path, mdx_path: Path, blurb: str) -> None:
    front = read_frontmatter(mdx_path)
    md_lines = md_path.read_text(encoding="utf-8").splitlines(keepends=True)
    body = md_body_to_mdx(md_lines)
    intro = (
        f"\n{blurb}\n\n"
        if blurb
        else "\n"
    )
    mdx_path.write_text("".join(front) + intro + "".join(body), encoding="utf-8")


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    specs = [
        (
            root / "SPECIFICATION.md",
            root / "SPECIFICATION.mdx",
            "This MDX edition carries the complete normative text of "
            "[SPECIFICATION.md](./SPECIFICATION.md). Brace characters in prose are "
            "HTML-escaped for MDX renderers; fenced code blocks are unchanged.",
        ),
        (
            root / "BOOK.md",
            root / "BOOK.mdx",
            "This MDX edition carries the complete text of [BOOK.md](./BOOK.md). "
            "Brace characters in prose are HTML-escaped for MDX renderers; fenced code "
            "blocks are unchanged.",
        ),
    ]
    for md, mdx, blurb in specs:
        convert(md, mdx, blurb)
        print(f"wrote {mdx} ({mdx.stat().st_size} bytes) from {md}")
    return 0


if __name__ == "__main__":
    sys.exit(main())