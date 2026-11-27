#!/usr/bin/env python3
"""Small, dependency-free CommonMark subset renderer for the generated site pages.

Covers what the repository's Markdown sources use: ATX headings, paragraphs,
fenced code, bullet and ordered lists, pipe tables, block quotes, rules, and the
inline forms `code`, **strong**, *emphasis*, and [links](url).
"""

from __future__ import annotations

import html
import re

_FENCE = re.compile(r"^(\s*)(`{3,}|~{3,})\s*([\w+-]*)")
_HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
_BULLET = re.compile(r"^(\s*)[-*+]\s+(.*)$")
_ORDERED = re.compile(r"^(\s*)\d+[.)]\s+(.*)$")
_RULE = re.compile(r"^\s*([-*_])(\s*\1){2,}\s*$")
_TABLE_SEP = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")
_CODE_SPAN = re.compile(r"(`+)(.+?)\1")
_LINK = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")
_STRONG = re.compile(r"\*\*(.+?)\*\*")
_EMPH = re.compile(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])")

_SUPER_LANGS = {"sr", "super"}


def slugify(text: str) -> str:
    """GitHub-style heading anchor."""
    plain = html.unescape(re.sub(r"<[^>]+>", "", text))
    plain = re.sub(r"[^\w\s-]", "", plain.lower())
    return re.sub(r"\s", "-", plain.strip())


def inline(text: str) -> str:
    """Render inline Markdown; code spans are protected from other rules."""
    spans: list[str] = []

    def stash(match: re.Match[str]) -> str:
        spans.append(f"<code>{html.escape(match.group(2).strip())}</code>")
        return f"\x00{len(spans) - 1}\x00"

    text = _CODE_SPAN.sub(stash, text)
    text = html.escape(text, quote=False)
    text = _LINK.sub(lambda m: f'<a href="{html.escape(m.group(2))}">{m.group(1)}</a>', text)
    text = _STRONG.sub(r"<strong>\1</strong>", text)
    text = _EMPH.sub(r"<em>\1</em>", text)
    return re.sub(r"\x00(\d+)\x00", lambda m: spans[int(m.group(1))], text)


def _cells(row: str) -> list[str]:
    row = row.strip()
    if row.startswith("|"):
        row = row[1:]
    if row.endswith("|"):
        row = row[:-1]
    return [cell.strip() for cell in row.split("|")]


def render(md: str, *, heading_offset: int = 0) -> str:
    """Render `md` to HTML. `heading_offset` demotes headings (`#` → `h1 + offset`)."""
    lines = md.splitlines()
    out: list[str] = []
    para: list[str] = []
    i = 0

    def flush() -> None:
        if para:
            out.append(f"<p>{inline(' '.join(part.strip() for part in para))}</p>")
            para.clear()

    while i < len(lines):
        line = lines[i]
        fence = _FENCE.match(line)
        if fence:
            flush()
            marker, lang = fence.group(2), fence.group(3).lower()
            body: list[str] = []
            i += 1
            while i < len(lines) and not lines[i].strip().startswith(marker):
                body.append(lines[i])
                i += 1
            i += 1
            cls = "language-sr" if lang in _SUPER_LANGS else (f"language-{lang}" if lang else "")
            pre = ' class="sr-block"' if lang in _SUPER_LANGS else ""
            attr = f' class="{cls}"' if cls else ""
            out.append(f"<pre{pre}><code{attr}>{html.escape(chr(10).join(body))}</code></pre>")
            continue
        heading = _HEADING.match(line)
        if heading:
            flush()
            level = min(len(heading.group(1)) + heading_offset, 6)
            text = inline(heading.group(2))
            out.append(f'<h{level} id="{slugify(text)}">{text}</h{level}>')
            i += 1
            continue
        if not line.strip():
            flush()
            i += 1
            continue
        if _RULE.match(line):
            flush()
            out.append("<hr/>")
            i += 1
            continue
        if "|" in line and i + 1 < len(lines) and _TABLE_SEP.match(lines[i + 1]):
            flush()
            head = _cells(line)
            i += 2
            rows: list[list[str]] = []
            while i < len(lines) and "|" in lines[i] and lines[i].strip():
                rows.append(_cells(lines[i]))
                i += 1
            th = "".join(f"<th>{inline(cell)}</th>" for cell in head)
            body_rows = "".join(
                "<tr>" + "".join(f"<td>{inline(cell)}</td>" for cell in row) + "</tr>" for row in rows
            )
            out.append(
                f'<div class="table-wrap"><table class="data"><thead><tr>{th}</tr></thead>'
                f"<tbody>{body_rows}</tbody></table></div>"
            )
            continue
        if line.lstrip().startswith(">"):
            flush()
            quoted: list[str] = []
            while i < len(lines) and lines[i].lstrip().startswith(">"):
                quoted.append(lines[i].lstrip()[1:].lstrip())
                i += 1
            out.append(f"<blockquote>{render(chr(10).join(quoted), heading_offset=heading_offset)}</blockquote>")
            continue
        bullet, ordered = _BULLET.match(line), _ORDERED.match(line)
        if bullet or ordered:
            flush()
            tag = "ul" if bullet else "ol"
            pattern = _BULLET if bullet else _ORDERED
            items: list[str] = []
            while i < len(lines):
                item = pattern.match(lines[i])
                if item:
                    items.append(item.group(2))
                elif lines[i].strip() and lines[i].startswith((" ", "\t")) and items:
                    items[-1] += " " + lines[i].strip()
                else:
                    break
                i += 1
            out.append(f"<{tag}>" + "".join(f"<li>{inline(item)}</li>" for item in items) + f"</{tag}>")
            continue
        para.append(line)
        i += 1
    flush()
    return "\n".join(out)
