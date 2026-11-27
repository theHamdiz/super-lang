// Programming in Super — print edition (0.1.0)
// Body: book-body.typ generated from BOOK.md via scripts/md_to_book_typst.py

#let ink = rgb("#121820")
#let mute = rgb("#5a6a7a")
#let rule = rgb("#d7dee7")
#let panel = rgb("#0e151d")
#let panel-edge = rgb("#243140")
#let accent = rgb("#0d9488")
#let accent-bright = rgb("#2dd4bf")
#let paper = rgb("#fafbfc")
#let term-bg = rgb("#f4f6f8")
#let term-ink = rgb("#1a2330")
#let code-fg = rgb("#e8eef5")

#set document(
  title: "The Super Programming Language",
  author: "Ahmad Hamdi",
  keywords: ("Super", "0.1.0", "for", "#for", "C23", "stdlib", "io_uring"),
)

#set page(
  paper: "a4",
  margin: (inside: 20mm, outside: 16mm, top: 17mm, bottom: 18mm),
  fill: paper,
  numbering: "1",
  header: context {
    let n = counter(page).get().first()
    if n > 1 [
      #set text(size: 8pt, fill: mute, font: "IBM Plex Sans")
      #grid(
        columns: (1fr, 1fr),
        align(left + horizon)[#box(image("assets/super-bolt.svg", height: 9pt), baseline: 1.6pt) #h(3pt) Super 0.1.0],
        align(right + horizon)[Programming in Super],
      )
      #v(1pt)
      #line(length: 100%, stroke: 0.45pt + rule)
    ]
  },
  footer: context {
    let n = counter(page).get().first()
    if n > 1 [
      #line(length: 100%, stroke: 0.45pt + rule)
      #v(3pt)
      #set text(size: 8pt, fill: mute, font: "IBM Plex Sans")
      #grid(
        columns: (1fr, auto),
        align(left)[Syntax · Stdlib · Algorithms · Projects],
        align(right)[#counter(page).display()],
      )
    ]
  },
)

#set text(font: "IBM Plex Sans", size: 10pt, fill: ink, lang: "en", costs: (widow: 200%, orphan: 200%, hyphenation: 150%))
#set par(justify: true, leading: 0.72em, spacing: 0.88em, first-line-indent: 0pt)
#set heading(numbering: none)
#set raw(
  syntaxes: "syntaxes/super.sublime-syntax",
  theme: "themes/super-ink.tmTheme",
)

#show heading.where(level: 1): it => {
  let title = repr(it.body)
  let is-toc = title.contains("Contents")
  if is-toc {
    block(breakable: false, above: 2pt, below: 14pt, width: 100%)[
      #box(image("assets/super-bolt.svg", height: 16pt), baseline: 3pt)#h(6pt)#text(size: 22pt, weight: "bold", fill: ink)[#it.body]
      #v(6pt)
      #rect(width: 28mm, height: 2.5pt, fill: accent, radius: 1pt)
    ]
  } else if title.contains("Formal specification") {
    // Short closing appendix: flows on after the last part instead of taking its own page.
    block(breakable: false, sticky: true, above: 18pt, below: 8pt, width: 100%)[
      #line(length: 100%, stroke: 0.45pt + rule)
      #v(6pt)
      #box(image("assets/super-bolt.svg", height: 11pt), baseline: 2pt)#h(5pt)#text(size: 8pt, weight: "medium", fill: accent, tracking: 1.4pt)[APPENDIX]
      #v(4pt)
      #text(size: 17pt, weight: "bold", fill: ink)[#it.body]
    ]
  } else {
    pagebreak(weak: true)
    block(breakable: false, above: 2pt, below: 12pt, width: 100%)[
      #grid(
        columns: (1fr, auto),
        column-gutter: 10pt,
        align: (left + top, right + top),
        [
          #box(image("assets/super-bolt.svg", height: 11pt), baseline: 2pt)#h(5pt)#text(size: 8pt, weight: "medium", fill: accent, tracking: 1.4pt)[PART]
          #v(5pt)
          #text(size: 20pt, weight: "bold", fill: ink)[#it.body]
        ],
        image("assets/super-bolt.svg", height: 26mm),
      )
      #v(-4pt)
      #rect(width: 28mm, height: 2.5pt, fill: accent, radius: 1pt)
    ]
  }
}

#show heading.where(level: 2): it => {
  block(breakable: false, sticky: true, above: 13pt, below: 6pt, width: 100%)[
    #text(size: 13.5pt, weight: "semibold", fill: ink)[#it.body]
    #v(3pt)
    #line(length: 100%, stroke: 0.4pt + rule)
  ]
}

#show heading.where(level: 3): it => block(breakable: false, sticky: true, above: 10pt, below: 4pt)[
  #text(size: 11pt, weight: "semibold", fill: rgb("#243041"))[#it.body]
]

#show link: set text(fill: accent)

// `listing`, `terminal`, `recipe`, `bolt`, `part-end` live in book-body.typ (generated PRELUDE).
// Bolt mark: editors/zed/icons/super.svg, copied to assets/super-bolt.svg by the generator.

#show raw.where(block: false): it => box(
  fill: rgb("#e6f7f4"),
  inset: (x: 3.5pt, y: 1.2pt),
  radius: 2pt,
  baseline: 1pt,
  text(font: "JetBrains Mono", size: 8.1pt, fill: rgb("#0f3d38"), it),
)

#show outline.entry: set block(spacing: 0.42em)
#show outline.entry.where(level: 1): it => {
  v(5pt, weak: true)
  strong(it)
}

// ── Cover (single continuous field — no footer slab) ─────────────────
#let cover-deep = rgb("#080d12")
#let cover-mist = rgb("#13202c")
#let cover-line = rgb("#1e2d3a")
#let cover-sub = rgb("#9aaaba")
#let cover-dim = rgb("#5c6d7d")

#page(margin: 0pt, header: none, footer: none, numbering: none)[
  #block(
    width: 100%,
    height: 100%,
    fill: gradient.linear(
      angle: 138deg,
      cover-deep,
      ink,
      rgb("#0e161f"),
      ink,
      cover-deep,
    ),
  )[
    // Spine accent — vertical motif, not a horizontal split.
    #place(top + left)[
      #block(width: 5mm, height: 100%)[
        #rect(
          width: 100%,
          height: 100%,
          fill: gradient.linear(
            angle: 180deg,
            accent.darken(8%),
            accent-bright,
            accent,
          ),
        )
      ]
    ]

    #place(top + right, dx: -14mm, dy: 18mm)[
      #image("assets/super-bolt.svg", height: 70mm)
    ]

    #place(top + left, dx: 24mm, dy: 22mm)[
      #block(width: 140mm, height: 255mm)[
        #grid(
          rows: (auto, 1fr, auto),
          row-gutter: 0pt,
          [
            #box(image("assets/super-bolt.svg", height: 12pt), baseline: 2pt)
            #h(6pt)
            #text(fill: accent-bright, size: 9.5pt, weight: "medium", tracking: 2.4pt)[SUPER]

            #v(26pt)

            #text(fill: white, size: 44pt, weight: "bold", hyphenate: false)[
              The Super\
              Programming Language
            ]

            #v(18pt)
            #rect(width: 46mm, height: 3pt, fill: accent-bright, radius: 1.2pt)

            #v(18pt)
            #set text(fill: cover-sub, size: 11pt)
            #set par(leading: 1.05em, justify: false)
            A token-compact language for coding agents.\
            Goose-style stacks, memory-safe by construction.\
            C and Rust interop. Native speed that can pass C.\
            Minimal syntax for CLI tools, APIs, games, and automation.

            #v(16pt)
            #text(size: 15pt, weight: "semibold", fill: white)[Ahmad Hamdi]
          ],
          [],
          [
            #text(font: "JetBrains Mono", size: 10.5pt, fill: accent-bright)[
              Iteration is #raw("for"). \
              Mutation is #raw("@"). \
              Parallelism is #raw("par").
            ]

            #v(8pt)
            #block(width: 100%)[
              #set text(size: 9pt, fill: cover-sub)
              #set par(leading: 0.42em)
              - #raw("for"), #raw("for @"), and #raw("#for") — loops, in-place writes, compile time.
              - #raw("par"), #raw(".sum()"), and #raw(".par_sum()") — parallelism and Iterable reductions.
              - #raw("?"), #raw("!"), #raw("or"), #raw("ensure"), and #raw("when") — absence, failure, and matches.
              - #raw("+") exports. UFCS and #raw("it") keep a pipeline short.
              - #raw(".{}") records. Flat C layouts. Rust FFI. A C23 binary.
              - #raw("test") in the file. #raw("#") comptime. #raw("scope"), #raw("spawn"), and #raw("await").
              - Command-line tools, backends and APIs, games, and automation.
            ]

            #v(12pt)

            #text(size: 10pt, fill: cover-sub, style: "italic")[
              The complete language book — syntax, the standard library,\
              and twenty real projects.
            ]

            #v(26pt)
            #line(length: 68mm, stroke: 0.4pt + cover-line.transparentize(40%))

            #v(18pt)
            #text(size: 9.5pt, fill: cover-dim)[
              Version #strong(text(fill: white)[0.1.0]) #h(6pt)·#h(6pt) Full print edition
            ]
            #v(10pt)
            #box(
              fill: cover-deep.transparentize(35%),
              stroke: 0.45pt + accent.darken(25%).transparentize(35%),
              inset: (x: 9pt, y: 5pt),
              radius: 3pt,
            )[
              #text(size: 8pt, fill: accent-bright, tracking: 0.5pt)[
                VM · native JIT · C23 AOT · stdlib 0.1.0
              ]
            ]
          ],
        )
      ]
    ]
  ]
]

#pagebreak()
#outline(title: [Contents], depth: 2, indent: 1.2em)
#pagebreak()

#include "book-body.typ"
