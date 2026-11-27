# Super formatting style

`super fmt` gives every `.sr` file one canonical layout. The formatter is
deterministic and idempotent: formatting a formatted file changes nothing,
so `fmt(fmt(x)) == fmt(x)` for every input. CI runs
`super fmt --check stdlib examples pods` and fails when a file differs.

## Commands

```text
super fmt                    # format every .sr file under the current directory
super fmt stdlib examples    # format files or directories
super fmt --check stdlib     # write nothing; list unformatted files and exit 1
super fmt - < in.sr > out.sr # stdin to stdout
super fmt --check - < in.sr  # exit 1 when stdin is not formatted
```

Editors get the same result through the language server:
`textDocument/formatting` returns one whole-document edit, or none when the
file is already formatted or does not lex.

## What the formatter changes

The formatter owns layout only. It never adds, removes, or reorders tokens.
Before it returns, it lexes the input and the output and compares the token
streams. If they differ, it reports an internal error and writes nothing, so
formatting cannot change what a program means.

1. **Line endings.** CRLF and CR become LF. The file ends with exactly one
   newline.
2. **Trailing whitespace** is removed, except inside a string literal that
   continues onto the next line.
3. **Indentation** is four spaces per level. The level of a line is set by
   the brackets `{`, `(`, `[` that are still open when it starts. Each open
   bracket indents its contents one level deeper than the line that opened
   it, so several brackets opened on one line (`f(g([`) indent once.
4. **Closing lines.** A line that starts with `}`, `)` or `]` sits at the
   level of the line that opened the outermost bracket it closes.
   `} else {` stays on the closing line.
5. **Method chains.** A line that starts with `.name` continues the
   expression above it and is indented one extra level. A line that starts
   with `.{` is an inferred struct literal, not a chain, and is not indented.
6. **Hanging bodies.** A line after one that ends in `=>` or a plain `=` is
   indented one extra level. Comparisons and compound assignments (`==`,
   `<=`, `+=`, …) do not hang.
7. **Blank lines.** Runs of blank lines collapse to one. Blank lines directly
   after an opening bracket, directly before a closing line, and at the start
   of the file are removed.
8. **Verbatim regions.** Lines that begin inside a multi-line string, raw
   string (`r"…"`, `r#"…"#`) or block comment (`/* … */`, `/*# … #*/`) are
   kept byte for byte.

## What the formatter does not change

- Spacing between tokens on a line (`a+b` stays `a+b`).
- Line breaks: the formatter never joins or splits lines.
- Comments other than their indentation.
- Visual alignment. Continuation lines are re-indented to their bracket
  level; column alignment under an opening bracket is not preserved.

These limits are deliberate: a layout-only formatter cannot change behavior,
and its output is easy to predict from the rules above.

## Example

Input:

```text
+Pos = {
  x: f64
  y: f64


  moved(self, dx: f64) : Pos =>
  Pos{x: self.x + dx, y: self.y}
}
+main() {
cfg = App.new()
.get("/", |req| { ok(req) })
print(cfg)
}
```

Output:

```text
+Pos = {
    x: f64
    y: f64

    moved(self, dx: f64) : Pos =>
        Pos{x: self.x + dx, y: self.y}
}
+main() {
    cfg = App.new()
        .get("/", |req| { ok(req) })
    print(cfg)
}
```
