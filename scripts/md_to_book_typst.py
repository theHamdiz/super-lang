#!/usr/bin/env python3
"""Convert BOOK.md into a designed Typst book body.

Design rules:
- Only `# Part …` / `# Expanded…` / `# Appendix…` become level-1 headings.
- Other `#` lines outside fences become bold lead-ins, never TOC chapters.
- ```super / ```sr → dark listing with Super syntax theme.
- ```text / ```output / unlabeled run output → light terminal panel (readable).
- ```bash / ```toml / ```json / ```c / … → language-tagged listings.
"""
from __future__ import annotations

import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BOOK = ROOT / "BOOK.md"
OUT = ROOT / "book" / "book-body.typ"
BOLT_SRC = ROOT / "editors" / "zed" / "icons" / "super.svg"  # Zed extension file icon
BOLT_DST = ROOT / "book" / "assets" / "super-bolt.svg"

PART_H1 = re.compile(
    r"^(Part\s+[IVXLC]+|Expanded Tutorials|Appendix)\b",
    re.IGNORECASE,
)

OUTPUT_LANGS = {"text", "output", "out", "console", "shell-session", ""}
CODE_LANG_MAP = {
    "sr": "super",
    "super": "super",
    "bash": "bash",
    "sh": "bash",
    "zsh": "bash",
    "toml": "toml",
    "json": "json",
    "c": "c",
    "cpp": "cpp",
    "rust": "rust",
    "go": "go",
    "lua": "lua",
    "zig": "zig",
    "python": "python",
    "py": "python",
    "markdown": "markdown",
    "md": "markdown",
    "mermaid": "text",
    "yaml": "yaml",
    "yml": "yaml",
}


def escape_typst_text(s: str) -> str:
    return (
        s.replace("\\", "\\\\")
        .replace("#", "\\#")
        .replace("@", "\\@")
        .replace("$", "\\$")
        .replace("<", "\\<")
        .replace(">", "\\>")
        .replace("*", "\\*")
        .replace("_", "\\_")
        .replace("`", "\\`")
        .replace("[", "\\[")
        .replace("]", "\\]")
    )


def md_inline_to_typst(s: str) -> str:
    out: list[str] = []
    i = 0
    while i < len(s):
        if s.startswith("`", i):
            j = s.find("`", i + 1)
            if j != -1:
                out.append(f"`{s[i + 1 : j]}`")
                i = j + 1
                continue
        if s.startswith("**", i):
            j = s.find("**", i + 2)
            if j != -1:
                out.append("*" + escape_typst_text(s[i + 2 : j]) + "*")
                i = j + 2
                continue
        if s.startswith("[", i):
            m = re.match(r"\[([^\]]+)\]\(([^)]+)\)", s[i:])
            if m:
                label, url = m.group(1), m.group(2)
                out.append(f'#link("{url}")[{escape_typst_text(label)}]')
                i += m.end()
                continue
        ch = s[i]
        if ch in "\\#@$<>*_`[]":
            out.append("\\" + ch)
        else:
            out.append(ch)
        i += 1
    return "".join(out)


# Parallel to `parts`: "para" | "code" | "blank" | "other"; `weights` = source lines of code items.
KINDS: list[str] = []
WEIGHTS: list[int] = []
GROUP_LINES = 26  # lead-in + code + output kept on one page up to this many lines
GROUP_PARA_CHARS = 700


def add(parts: list[str], text: str, kind: str = "other", weight: int = 0) -> None:
    parts.append(text)
    KINDS.append(kind)
    WEIGHTS.append(weight)


# One printed code panel ≈ 58 lines at our mono size; keep logical listings whole until
# they exceed a page, then split only at blank-line boundaries (CONTINUED on chunk 2+).
CODE_KEEP = 58
CHUNK_MAX = 52
LISTING_PREFACE_LINES = 26  # force a page check before tall single-panel listings
SHORT_PARA = 130  # shootout captions between consecutive listings


def table_style(headers: list[str]) -> str:
    h0 = headers[0].lower() if headers else ""
    if h0 == "chapter":
        return '  style: "chapter",\n'
    if h0 == "kernel":
        return '  style: "benchmark",\n'
    return ""


def table_column_spec(headers: list[str]) -> str:
    n = len(headers)
    h0 = headers[0].lower() if headers else ""
    if n == 7 and h0 == "kernel":
        return (
            "1.1fr, 0.47fr, 0.56fr, 0.56fr, 0.45fr, 0.45fr, 1.88fr"
        )
    if n == 7 and h0 == "concern":
        return "1.42fr, 0.68fr, 0.68fr, 0.68fr, 0.62fr, 0.62fr, 0.72fr"
    if n == 2:
        return "1.15fr, 0.72fr"
    if n == 4 and h0 == "chapter":
        return "auto, 0.82fr, 1.2fr, 1.62fr"
    if n == 2 and "layer" in h0:
        return "0.62fr, 1.38fr"
    return ", ".join(["1fr"] * n)


def format_table_cell(cell: str, col: int, headers: list[str]) -> str:
    h0 = headers[0].lower() if headers else ""
    if h0 == "chapter" and col == 2 and len(cell) > 26 and "/" in cell:
        mid = cell.rfind("/", 0, len(cell) // 2 + 8)
        if mid > 0:
            a, b = md_inline_to_typst(cell[: mid + 1]), md_inline_to_typst(cell[mid + 1 :])
            return f'#text(font: "JetBrains Mono", size: 6.75pt)[{a} \\ {b}]'
    if h0 == "kernel" and col == 6:
        bare = re.sub(r"[^\d]", "", cell)
        if len(bare) > 10:
            mid = len(bare) // 2
            a, b = bare[:mid], bare[mid:]
            return f'#text(size: 6.05pt, font: "JetBrains Mono")[{a} \\ {b}]'
    return md_inline_to_typst(cell)


def emit_table(parts: list[str], rows: list[list[str]]) -> None:
    col_spec = table_column_spec(rows[0])
    cells = "\n".join(
        f"  [{format_table_cell(c, ci, rows[0])}],"
        for r in rows
        for ci, c in enumerate(r)
    )
    add(
        parts,
        "#book-table(\n"
        f"  columns: ({col_spec}),\n"
        f"{table_style(rows[0])}"
        '  stroke: 0.45pt + rgb("#d5dde6"),\n'
        '  fill: (col, row) => if row == 0 { rgb("#f1f5f9") } else { white },\n'
        f"{cells}\n"
        ")",
        "table",
    )
    add(parts, "")


def chunk_lines(body: str) -> list[str]:
    """Return one raw string per panel. Multiple panels ⇒ CONTINUED seams in Typst."""
    rows = body.split("\n")
    if len(rows) <= CODE_KEEP:
        return [body]
    segs: list[list[str]] = [[]]
    for r in rows:
        if r.strip() == "":
            if segs[-1]:
                segs.append([])
        else:
            segs[-1].append(r)
    segs = [sg for sg in segs if sg]
    chunks: list[list[str]] = []
    cur: list[str] = []
    for sg in segs:
        if cur and len(cur) + len(sg) + 1 > CHUNK_MAX:
            chunks.append(cur)
            cur = []
        if cur:
            cur.append("")
        cur += sg
    if cur:
        chunks.append(cur)
    panels: list[str] = []
    for chunk in chunks:
        if len(chunk) <= CHUNK_MAX:
            panels.append("\n".join(chunk))
            continue
        for start in range(0, len(chunk), CHUNK_MAX):
            panels.append("\n".join(chunk[start : start + CHUNK_MAX]))
    return panels


def emit_code(parts: list[str], lang: str, body: str) -> None:
    """Items are stored as (call-prefix, raws) markers; `assemble` renders them."""
    raw_lang = (lang or "text").strip().lower()
    mapped = CODE_LANG_MAP.get(raw_lang, raw_lang if raw_lang else "text")
    chunks = chunk_lines(body)
    n = len(body.split("\n"))
    is_out = mapped in OUTPUT_LANGS or raw_lang in OUTPUT_LANGS
    lang_tag = "text" if is_out else mapped
    raws = ", ".join(f"```{lang_tag}\n{c}\n```" for c in chunks)
    fn = "terminal" if is_out else "listing"
    extra = "" if is_out else f'lang: "{mapped}", '
    if not is_out and n >= LISTING_PREFACE_LINES and len(chunks) == 1:
        add(parts, "#listing-preface()", "other")
    # `LEAD` is substituted by assemble() when a lead-in paragraph travels with the block.
    add(parts, f"#{fn}({extra}@@LEAD@@({raws},))", "code", n + (2 if is_out else 3))
    add(parts, "")


def assemble(parts: list[str]) -> str:
    """Group short lead-in + code (+ output) runs into unbreakable blocks and
    close every part with a `#part-end(k)` filler that sizes itself to the space left."""
    out: list[tuple[str, str]] = []  # (text, kind)
    n = len(parts)
    i = 0
    part_no = 0
    while i < n:
        text, kind = parts[i], KINDS[i]
        if text.startswith("= "):
            out.append((f"#part-end({part_no})", "other"))
            out.append(("", "blank"))
            part_no += 1
        if kind != "code":
            out.append((text, kind))
            i += 1
            continue
        run: list[str] = []
        total = 0
        j = i
        while j < n and (KINDS[j] == "code" or parts[j] == ""):
            if KINDS[j] == "code":
                run.append(parts[j])
                total += WEIGHTS[j]
            j += 1
        k = len(out) - 1
        while k >= 0 and out[k][1] == "blank":
            k -= 1
        lead = ""
        if k >= 0 and out[k][1] == "para" and len(out[k][0]) <= GROUP_PARA_CHARS:
            lead = out[k][0]
        lead_weight = (2 + len(lead) // 90) if lead else 0
        if total + lead_weight <= GROUP_LINES:
            if lead:
                del out[k:]
            lead_txt = f"{lead}\n\n" if lead else ""
            body = "\n".join(r.replace("@@LEAD@@", "") for r in run)
            out.append((f"#recipe[{lead_txt}{body}]", "recipe"))
            out.append(("", "blank"))
        else:
            if lead:
                del out[k:]
            for idx, item in enumerate(run):
                lead_arg = f"lead: [{lead}], " if lead and idx == 0 else ""
                call = item.replace("@@LEAD@@", lead_arg)
                if idx > 0:
                    out.append(("#listing-gap()", "other"))
                out.append((call, "code"))
                out.append(("", "blank"))
        i = j

    def prev_code_or_caption(stitched: list[tuple[str, str]]) -> bool:
        k = len(stitched) - 1
        while k >= 0 and stitched[k][1] == "blank":
            k -= 1
        if k < 0:
            return False
        if stitched[k][1] == "code":
            return True
        if stitched[k][1] == "para" and len(stitched[k][0]) <= SHORT_PARA:
            k -= 1
            while k >= 0 and stitched[k][1] == "blank":
                k -= 1
            return k >= 0 and stitched[k][1] == "code"
        return False

    stitched: list[tuple[str, str]] = []
    for text, kind in out:
        if kind == "code" and prev_code_or_caption(stitched):
            stitched.append(("#listing-preface()", "other"))
        stitched.append((text, kind))
    return "\n".join(t for t, _ in stitched)


def convert(md: str) -> str:
    lines = md.splitlines()
    parts: list[str] = []
    i = 0
    in_code = False
    code_lang = "text"
    code_buf: list[str] = []
    para: list[str] = []

    def flush_para(buf: list[str]) -> None:
        text = " ".join(x.strip() for x in buf if x.strip())
        if text:
            add(parts, f"#prose[{md_inline_to_typst(text)}]", "para")
            add(parts, "")

    while i < len(lines):
        line = lines[i]
        if line.startswith("```"):
            if in_code:
                nonempty = [ln for ln in code_buf if ln.strip()]
                body = "\n".join(code_buf)
                if nonempty:
                    mind = min(len(ln) - len(ln.lstrip(" ")) for ln in nonempty)
                    body = "\n".join(
                        ln[mind:] if len(ln) >= mind else ln for ln in code_buf
                    )
                emit_code(parts, code_lang, body)
                in_code = False
                code_buf = []
            else:
                flush_para(para)
                para = []
                in_code = True
                code_lang = line[3:].strip() or "text"
            i += 1
            continue
        if in_code:
            code_buf.append(line)
            i += 1
            continue

        if line.startswith("# "):
            flush_para(para)
            para = []
            title = line[2:].strip()
            if title.lower().startswith("programming in super"):
                i += 1
                continue
            if PART_H1.match(title):
                add(parts, f"= {escape_typst_text(title)}")
            else:
                # Never promote fence comments / stray H1s into TOC chapters.
                add(parts, f"*{' '.join(escape_typst_text(title).split())}*")
            add(parts, "")
        elif line.startswith("## "):
            flush_para(para)
            para = []
            add(parts, f"== {md_inline_to_typst(line[3:].strip())}")
            add(parts, "")
        elif line.startswith("### "):
            flush_para(para)
            para = []
            add(parts, f"=== {md_inline_to_typst(line[4:].strip())}")
            add(parts, "")
        elif line.startswith("- ") or line.startswith("* "):
            flush_para(para)
            para = []
            while i < len(lines) and (
                lines[i].startswith("- ") or lines[i].startswith("* ")
            ):
                item = lines[i][2:].strip()
                add(parts, f"- {md_inline_to_typst(item)}")
                i += 1
            add(parts, "")
            continue
        elif re.match(r"^\d+\.\s+", line):
            flush_para(para)
            para = []
            while i < len(lines) and re.match(r"^\d+\.\s+", lines[i]):
                item = re.sub(r"^\d+\.\s+", "", lines[i]).strip()
                add(parts, f"+ {md_inline_to_typst(item)}")
                i += 1
            add(parts, "")
            continue
        elif line.strip() == "":
            flush_para(para)
            para = []
        elif line.startswith("|"):
            flush_para(para)
            para = []
            rows: list[list[str]] = []
            while i < len(lines) and lines[i].startswith("|"):
                if re.match(r"^\|\s*-+", lines[i]):
                    i += 1
                    continue
                cells = [c.strip() for c in lines[i].strip("|").split("|")]
                rows.append(cells)
                i += 1
            if rows:
                emit_table(parts, rows)
            continue
        elif line.strip() == "---":
            flush_para(para)
            para = []
            add(parts, "#v(6pt)#line(length: 100%, stroke: 0.4pt + rgb(\"#d5dde6\"))#v(6pt)")
            add(parts, "")
        else:
            para.append(line)
        i += 1
    flush_para(para)
    return assemble(parts)


PRELUDE = r'''
// Auto-generated from BOOK.md — helpers must live here (#include does not see outer #lets).
#let bolt-path = "assets/super-bolt.svg"
#let bolt(h: 8.5pt) = box(image(bolt-path, height: h), baseline: 0.18 * h)

#let prose(body) = block(width: 100%, above: 3pt, below: 8pt)[#body]

#let listing-preface() = context {
  let body = page.height - page.margin.top - page.margin.bottom
  let rem = page.height - page.margin.bottom - here().position().y
  // Prefer a fresh page for a full code panel rather than orphaning a seam mid-listing.
  if rem < 0.62 * body { pagebreak(weak: false) }
}

#let listing-gap() = v(3pt)


// Book tables — tuned column widths, tabular digits, tighter wrapping in numeric cells.
#let book-table(columns: (), stroke: none, fill: none, style: "default", ..cells) = block(
  breakable: true,
  width: 100%,
  above: 11pt,
  below: 18pt,
)[
  #set text(size: 8.55pt, hyphenate: true)
  #set par(justify: false, leading: 0.54em, spacing: 0.45em)
  #if style == "chapter" {
    show table.cell.where(x: 0): set text(size: 8.35pt, font: "JetBrains Mono")
    show table.cell.where(x: 1): set text(size: 8.45pt, weight: "medium")
    show table.cell.where(x: 2): set text(font: "JetBrains Mono", size: 6.65pt, hyphenate: false)
    show table.cell.where(x: 3): set text(size: 7.9pt, hyphenate: true)
  } else if style == "benchmark" {
    show table.cell.where(x: 0): set text(hyphenate: true, size: 8.35pt)
    show table.cell.where(x: 6): set text(
      font: "JetBrains Mono", size: 6.05pt, hyphenate: false, tracking: -0.35pt,
    )
    for col in range(1, 6) {
      show table.cell.where(x: col): set text(
        font: "JetBrains Mono", size: 7.45pt, hyphenate: false,
      )
    }
  } else {
    show table.cell.where(x: 0): set text(hyphenate: true, size: 8.45pt)
    for col in range(1, columns.len()) {
      show table.cell.where(x: col): set text(
        font: "JetBrains Mono", size: 7.55pt, hyphenate: false,
      )
    }
  }
  #table(
    columns: columns,
    stroke: stroke,
    inset: (x: 6pt, y: 4.5pt),
    fill: fill,
    ..cells,
  )
]

// Code / output panels. Only listings longer than one printed page are split at blank
// lines; each chunk is unbreakable. CONTINUED appears only on chunk 2+ of those splits.
#let panel(raws, lang: "code", lead: none, terminal: false) = {
  let n = raws.len()
  for (i, r) in raws.enumerate() {
    let first = i == 0
    let last = i == n - 1
    let cont = not first
    if cont {
      listing-preface()
    }
    let edge = if terminal { 0.55pt + rgb("#cfd8e3") } else { 0.6pt + rgb("#243140") }
    let inner = block(
      breakable: false,
      width: 100%,
      fill: if terminal { rgb("#f4f6f8") } else { rgb("#0e151d") },
      radius: (top: if first { 4pt } else { 0pt }, bottom: if last { 4pt } else { 0pt }),
      inset: (x: 9pt, top: if first { 7pt } else if cont { 5pt } else { 4pt }, bottom: if last { 7pt } else { 4pt }),
      stroke: (
        left: edge, right: edge,
        top: if first { edge } else { none },
        bottom: if last { edge } else { none },
      ),
    )[
      #set par(justify: false)
      #if terminal {
        set text(font: "JetBrains Mono", size: 8.15pt, fill: rgb("#1a2330"), weight: "regular")
        set par(leading: 0.56em)
        if first {
          text(size: 6.6pt, fill: rgb("#5a6a7a"), weight: "bold", tracking: 1.1pt)[OUTPUT]
          v(2pt)
        } else {
          text(size: 6.35pt, fill: rgb("#8a97a6"), weight: "bold", tracking: 1pt)[OUTPUT · CONTINUED]
          v(2pt)
        }
        r
      } else {
        set text(font: "JetBrains Mono", size: 8.25pt, fill: rgb("#e8eef5"), weight: "regular")
        set par(leading: 0.58em)
        if first {
          text(size: 6.8pt, fill: rgb("#7f93a8"), weight: "bold", tracking: 1.1pt)[
            #if lang == "super" { bolt(h: 8pt); h(3pt) }#upper(lang)
          ]
          v(3pt)
        } else {
          text(size: 6.5pt, fill: rgb("#607080"), weight: "bold", tracking: 1pt)[
            #if lang == "super" { bolt(h: 7.5pt); h(3pt) }#upper(lang) · CONTINUED
          ]
          v(2pt)
        }
        r
      }
    ]
    let blk = block(
      breakable: false,
      width: 100%,
      above: if first { if terminal { 5pt } else { 7pt } } else { 0pt },
      below: if last { if terminal { 9pt } else { 11pt } } else { 0pt },
    )[#inner]
    if first and lead != none {
      block(breakable: false, width: 100%, above: 5pt, below: 3pt)[#lead #blk]
    } else { blk }
  }
}

#let listing(raws, lang: "code", lead: none) = panel(raws, lang: lang, lead: lead)
#let terminal(raws, lead: none) = panel(raws, lead: lead, terminal: true)

// Short recipe: lead-in sentence + code (+ output) kept on one page.
#let recipe(body) = block(breakable: false, width: 100%, above: 5pt, below: 12pt)[#body]

#let key-ideas = (
  ([Iteration is `for`], [One loop keyword covers ranges, collections and `#for` pipelines.]),
  ([Mutation is `@`], [Writes are explicit at the declaration and at the call site.]),
  ([Parallelism is `par`], [Fan-out is a keyword, not a framework or a thread pool.]),
  ([Scoped data stacks], [Typed hot paths allocate on scoped stacks that rewind in O(1); dynamic values still use the heap.]),
  ([One semantics, three paths], [The VM, the native JIT and the C23 AOT binary agree bit for bit.]),
  ([Immutable by default], [Bindings are local and fixed; `+` is the only way to export.]),
  ([`?` and `!`], [Optionals and value errors are types; `or` handles both inline.]),
  ([UFCS and `it`], [`x.f(y)` is `f(x, y)`; pipelines read left to right with an implicit `it`.]),
  ([Tests ship in the file], [`test` blocks see private items and vanish from release builds.]),
  ([Structs are flat], [Named records are C-ABI value layouts; sum types are compact tags.]),
)

#let ce-accent = rgb("#0d9488")
#let ce-muted = rgb("#5a6a7a")
#let ce-body = rgb("#3b4a5a")
#let ce-rule = rgb("#d7dee7")
#let ce-card = rgb("#f1f6f6")
#let ce-gloss = rgb("#f4f7f9")
#let ce-tease = rgb("#e8f7f5")
#let ce-tease-edge = rgb("#b8ddd8")
#let ce-motif-a = rgb("#f3f8f8")
#let ce-motif-b = rgb("#fafbfc")
#let ce-motif-edge = rgb("#d0ddd8")
#let ce-idea-bg = rgb("#ffffffaa")
#let ce-card-edge = rgb("#cfdcdc")

// One full-width row per term. Definitions are one sentence so they do not
// wrap into the neighboring entry.
#let gloss-terms = (
  ([\@], [In-place write on a parameter, a field, or a loop element.]),
  ([for], [One loop for a range, an array, or a pipeline. #raw("#for") runs at compile time.]),
  ([? !], [An optional and a value error. The caller handles both.]),
  ([or], [Recovers in line from a missing value or a failed call.]),
  ([+], [Exports the next declaration. There is no export list.]),
  ([stack], [A scope bumps a cursor, then rewinds that region in one store.]),
  ([#raw("#size")], [Compile-time check that a struct keeps its C layout.]),
  ([shared], [A counter or flag that parallel work updates without a lock.]),
  ([UFCS], [A method call is the same function with the value first.]),
  ([test], [A test sees private items and is omitted from a release build.]),
)

// Decorative closing band — height scales with leftover space (capped).
#let bolt-band(h: 36pt) = block(
  breakable: false,
  width: 100%,
  height: h,
  fill: rgb("#0e151d"),
  radius: 4pt,
  inset: (x: 12pt, y: calc.max(7pt, (h - 22pt) / 2)),
)[
  #grid(
    columns: (1fr, auto),
    column-gutter: 8pt,
    align: (left + horizon, right + horizon),
    [
      #set text(font: "JetBrains Mono", size: calc.min(9pt, 7pt + h * 0.08), fill: rgb("#2dd4bf"))
      Iteration is for · Mutation is \@ · Parallelism is par
      #v(if h > 40pt { 3pt } else { 1pt })
      #set text(size: calc.min(8pt, 6.8pt + h * 0.05), fill: rgb("#7f93a8"))
      Super 0.1.0 — scoped stacks · SIMD JIT · C23 AOT
    ],
    image(bolt-path, height: calc.min(20pt, h - 8pt)),
  )
]

// Closing band: natural height only — never a fixed box that can paint over prose.
#let part-end-cards(k, n-ideas, map-shown, sec-shown, sec-more, gloss-n) = block(
  breakable: false,
  width: 100%,
)[
  #block(width: 100%, fill: ce-card, radius: 4pt,
        stroke: (left: 2.4pt + ce-accent, rest: 0.5pt + ce-card-edge),
        inset: (x: 11pt, y: 7pt))[
    #set par(justify: false, leading: 0.5em)
    #text(size: 7.3pt, weight: "bold", fill: ce-accent, tracking: 1.3pt)[
      #bolt(h: 8pt)#h(4pt)KEY IDEAS TO CARRY FORWARD
    ]
    #v(2pt)
    #for i in range(n-ideas) {
      let idea = key-ideas.at(calc.rem(k * 2 + i, key-ideas.len()))
      block(above: if i == 0 { 1pt } else { 5pt })[
        #text(size: 8.6pt, weight: "semibold", fill: ce-accent)[#idea.at(0)]
        #v(1pt)
        #set par(justify: false, leading: 0.55em)
        #set text(hyphenate: false)
        #text(size: 8.2pt, fill: ce-body)[#idea.at(1)]
      ]
    }
  ]
  #if map-shown.len() > 0 or sec-shown.len() > 0 or gloss-n > 0 {
    v(5pt)
  }
  #if map-shown.len() > 0 {
    block(width: 100%, inset: (x: 2pt))[
      #text(size: 7.2pt, weight: "bold", fill: ce-muted, tracking: 1.2pt)[WHAT THIS BOOK COVERS]
      #v(2pt)
      #line(length: 100%, stroke: 0.45pt + ce-rule)
      #v(2pt)
      #set text(size: 8.05pt)
      #for p in map-shown [
        #link(p.location())[
          #grid(columns: (1fr, auto), column-gutter: 6pt,
            text(p.body), text(fill: ce-muted)[#counter(page).at(p.location()).first()])
        ]
        #v(2.5pt)
      ]
    ]
    v(4pt)
  } else if sec-shown.len() > 0 {
    block(width: 100%, inset: (x: 2pt))[
      #text(size: 7.2pt, weight: "bold", fill: ce-muted, tracking: 1.2pt)[IN THIS PART]
      #v(2pt)
      #line(length: 100%, stroke: 0.45pt + ce-rule)
      #v(2pt)
      #set text(size: 8.05pt)
      #grid(
        columns: (1fr, 1fr),
        column-gutter: 10pt,
        row-gutter: 4pt,
        ..sec-shown.map(sec => link(sec.location())[
          #box(width: 100%)[#sec.body #h(1fr) #text(fill: ce-muted)[#counter(page).at(sec.location()).first()]]
        ]),
      )
      #if sec-more > 0 [#v(1pt) #text(fill: ce-muted, size: 7.8pt)[… #sec-more more]]
    ]
    v(4pt)
  }
  #if gloss-n > 0 {
    block(width: 100%, fill: ce-gloss, radius: 3pt,
          stroke: 0.45pt + ce-rule, inset: (x: 10pt, y: 7pt))[
      #text(size: 7.1pt, weight: "bold", fill: ce-muted, tracking: 1.1pt)[QUICK GLOSSARY]
      #v(4pt)
      #for i in range(gloss-n) {
        let g = gloss-terms.at(calc.rem(k * 2 + i, gloss-terms.len()))
        block(
          width: 100%,
          inset: (y: 3.4pt),
          stroke: (top: if i == 0 { 0.45pt + ce-rule } else { none }, bottom: 0.4pt + ce-rule),
        )[
          #grid(
            columns: (46pt, 1fr),
            column-gutter: 10pt,
            align: (left + horizon, left + horizon),
            text(font: "JetBrains Mono", size: 8.3pt, weight: "semibold", fill: ce-accent, hyphenate: false)[#g.at(0)],
            {
              set par(justify: false, leading: 0.42em)
              set text(size: 8.25pt, fill: ce-body, hyphenate: false)
              g.at(1)
            },
          )
        ]
      }
    ]
  }
]

#let part-end-foot(has-teaser, next-part) = block(breakable: false, width: 100%)[
  #if has-teaser {
    block(width: 100%, fill: ce-tease, radius: 3pt,
          stroke: (left: 2pt + ce-accent, rest: 0.45pt + ce-tease-edge),
          inset: (x: 10pt, y: 5pt))[
      #text(size: 6.9pt, weight: "bold", fill: ce-accent, tracking: 1pt)[UP NEXT]
      #v(1pt)
      #link(next-part.location())[
        #text(size: 9pt, weight: "semibold")[#next-part.body]
        #h(1fr)
        #text(size: 7.8pt, fill: ce-muted)[p. #counter(page).at(next-part.location()).first()]
      ]
    ]
    v(5pt)
  }
  #bolt-band(h: 42pt)
]

#let part-end-estimate(n-ideas, map-n, sec-n, gloss-n, has-teaser) = {
  let ideas = 52pt + n-ideas * 34pt
  let map = if map-n > 0 { 36pt + map-n * 18pt } else { 0pt }
  let secs = if sec-n > 0 { 42pt + calc.ceil(sec-n / 2) * 20pt } else { 0pt }
  let gloss = if gloss-n > 0 { 28pt + gloss-n * 20pt } else { 0pt }
  let teaser = if has-teaser { 44pt } else { 0pt }
  ideas + map + secs + gloss + teaser + 58pt + 40pt
}

#let part-end(k) = context {
  let pos = here().position()
  if pos.y < 14mm { return }

  let parts-before = query(heading.where(level: 1).before(here()))
  let parts-after = query(heading.where(level: 1).after(here()))
  let cur = if parts-before.len() > 0 { parts-before.last() } else { none }
  let secs = if cur != none {
    query(heading.where(level: 2).after(cur.location()).before(here()))
  } else { () }

  let has-teaser = parts-after.len() > 0
  let next-part = if has-teaser { parts-after.first() } else { none }
  let map-shown = if k == 0 and parts-after.len() > 0 {
    parts-after.slice(0, calc.min(5, parts-after.len()))
  } else { () }

  let n-ideas-want = if k == 0 { 3 } else { 4 }
  let sec-cap = if map-shown.len() == 0 and secs.len() > 0 {
    calc.min(secs.len(), 6)
  } else { 0 }
  let sec-shown-want = if sec-cap > 0 { secs.slice(0, sec-cap) } else { () }
  let sec-more-want = if sec-cap > 0 { secs.len() - sec-cap } else { 0 }
  let gloss-want = if map-shown.len() == 0 and sec-shown-want.len() == 0 { 4 } else if sec-shown-want.len() == 0 { 2 } else { 0 }

  let rem0 = page.height - page.margin.bottom - pos.y - 20pt
  let compact = rem0 < part-end-estimate(n-ideas-want, 0, 0, 0, has-teaser)
  let trim = not compact and rem0 < part-end-estimate(n-ideas-want, map-shown.len(), sec-shown-want.len(), gloss-want, has-teaser)
  let n-ideas = if compact { calc.min(n-ideas-want, 3) } else { n-ideas-want }
  let map-final = if compact { () } else if trim and map-shown.len() > 3 { map-shown.slice(0, 3) } else { map-shown }
  let sec-shown = if compact { () } else if trim and sec-shown-want.len() > 4 { secs.slice(0, 4) } else { sec-shown-want }
  let sec-more = if compact { 0 } else if trim and sec-shown-want.len() > 4 { secs.len() - 4 } else { sec-more-want }
  let gloss-n = if compact { 0 } else if trim and gloss-want > 2 { 2 } else { gloss-want }

  let band-body = stack(
    spacing: 6pt,
    part-end-cards(k, n-ideas, map-final, sec-shown, sec-more, gloss-n),
    part-end-foot(has-teaser, next-part),
  )
  let band-h = measure(band-body).height
  let rem1 = page.height - page.margin.bottom - here().position().y - 20pt
  if rem1 < band-h + 24pt {
    pagebreak(weak: false)
  }

  // Always flow after prose; pagebreak guarantees the unbreakable band fits without painting upward.
  block(breakable: false, width: 100%, above: 12pt, below: 4pt)[#band-body]
}

'''


def main() -> int:
    BOLT_DST.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(BOLT_SRC, BOLT_DST)
    md = BOOK.read_text(encoding="utf-8")
    body = convert(md)
    spec_note = """
= Appendix — Formal specification

The normative grammar, memory model, diagnostics (E01xx–E04xx), package lock format, and C23 lowering rules live in `SPECIFICATION.md` at the repository root. Read it alongside this book when implementing tools or proving matching checksums.

#part-end(7)
"""
    OUT.write_text(PRELUDE + body + "\n" + spec_note, encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
