//! `super fmt`: the canonical layout of Super source.
//!
//! The formatter owns layout only: line endings, indentation, blank lines, and
//! trailing whitespace. It never adds, removes, or reorders tokens, and every
//! result is checked against the input's token stream before it is returned, so
//! formatting cannot change what a program means. The rules are documented in
//! `docs/formatting-style.md`.

use crate::Lexer;
use crate::token::TokenKind;
use super_core::{Diagnostics, Interner};

/// One indentation level.
pub const INDENT: &str = "    ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The source does not lex; formatting it could not be checked.
    Lex(String),
    /// Formatting would have changed the token stream (a formatter bug).
    TokensChanged,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Lex(message) => {
                write!(f, "cannot format a file that does not lex: {message}")
            }
            FormatError::TokensChanged => {
                f.write_str("internal formatter error: layout changed the token stream")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// Lexical state carried from one physical line to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carry {
    Code,
    String,
    RawString { hashes: usize },
    BlockComment,
}

/// What the scanner learned about one physical line.
struct LineInfo<'a> {
    text: &'a str,
    /// The line begins inside a string or block comment; it is kept verbatim.
    verbatim_start: bool,
    /// The line ends inside a string; its trailing whitespace is content.
    open_end: bool,
    /// Indentation level of the line.
    depth: usize,
}

/// Format `source`. Formatting is idempotent: `format(format(x)) == format(x)`.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let before = token_fingerprint(&normalized)?;
    let formatted = layout(&normalized);
    if token_fingerprint(&formatted)? != before {
        return Err(FormatError::TokensChanged);
    }
    Ok(formatted)
}

/// Is `source` already formatted?
pub fn is_formatted(source: &str) -> Result<bool, FormatError> {
    Ok(format_source(source)? == source)
}

fn token_fingerprint(source: &str) -> Result<Vec<(TokenKind, String)>, FormatError> {
    let mut interner = Interner::with_capacity(256);
    let mut diagnostics = Diagnostics::new();
    let tokens = Lexer::new(source, &mut interner, &mut diagnostics).tokenize();
    if diagnostics.has_errors() {
        let first = diagnostics
            .into_vec()
            .into_iter()
            .next()
            .map(|diagnostic| diagnostic.message)
            .unwrap_or_default();
        return Err(FormatError::Lex(first));
    }
    Ok(tokens
        .iter()
        .map(|token| {
            let text = token
                .sym
                .map(|sym| interner.resolve(sym).to_owned())
                .unwrap_or_default();
            (token.kind, text)
        })
        .collect())
}

fn scan(source: &str) -> Vec<LineInfo<'_>> {
    let mut lines = Vec::new();
    let mut carry = Carry::Code;
    // The indentation level of the line that opened each open bracket. The
    // contents of a bracket sit one level deeper than its opening line, so
    // several brackets opened on one line indent the following lines once.
    let mut open: Vec<usize> = Vec::new();
    let mut hang_next = false;
    for text in source.split('\n') {
        let verbatim_start = carry != Carry::Code;
        let start_open = open.clone();
        let mut leading_closers = 0usize;
        let body = text.trim_start();
        let chained = body
            .strip_prefix('.')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_alphabetic() || c == '_');
        let hang = hang_next && !body.is_empty();
        // Provisional level; leading closers are applied once they are known.
        // A line that starts by closing brackets sits at the level of the line
        // that opened the outermost of them.
        let level = |closers: usize| match closers.min(start_open.len()) {
            0 => start_open.last().map_or(0, |opener| opener + 1) + usize::from(chained || hang),
            closers => start_open[start_open.len() - closers],
        };
        let mut code_end = 0usize;
        let mut leading = !verbatim_start;
        let bytes = text.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            match carry {
                Carry::String => {
                    match b {
                        b'\\' => i += 1,
                        b'"' => carry = Carry::Code,
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                Carry::RawString { hashes } => {
                    if b == b'"'
                        && bytes[i + 1..]
                            .iter()
                            .take(hashes)
                            .filter(|&&c| c == b'#')
                            .count()
                            == hashes
                    {
                        carry = Carry::Code;
                        i += 1 + hashes;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                Carry::BlockComment => {
                    if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        carry = Carry::Code;
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                Carry::Code => {}
            }
            if leading && !b.is_ascii_whitespace() && !matches!(b, b'}' | b')' | b']') {
                leading = false;
            }
            if !b.is_ascii_whitespace() {
                code_end = i + 1;
            }
            match b {
                b'"' => carry = Carry::String,
                b'r' if raw_string_hashes(&bytes[i..]).is_some()
                    && !ident_byte_before(bytes, i) =>
                {
                    let hashes = raw_string_hashes(&bytes[i..]).unwrap_or(0);
                    carry = Carry::RawString { hashes };
                    i += 1 + hashes;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => break,
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    carry = Carry::BlockComment;
                    i += 1;
                }
                b'{' | b'(' | b'[' => open.push(level(leading_closers)),
                b'}' | b')' | b']' => {
                    open.pop();
                    if leading {
                        leading_closers += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let depth = level(leading_closers);
        if !body.is_empty() && !verbatim_start {
            let code = text[..code_end].trim_end();
            hang_next = carry == Carry::Code && (code.ends_with("=>") || ends_with_assign(code));
        }
        lines.push(LineInfo {
            text,
            verbatim_start,
            open_end: matches!(carry, Carry::String | Carry::RawString { .. }),
            depth,
        });
    }
    lines
}

/// A trailing `=` (not `==`, `!=`, `<=`, `>=`, `+=`…) continues on the next line.
fn ends_with_assign(code: &str) -> bool {
    code.strip_suffix('=').is_some_and(|rest| {
        !rest.ends_with(['=', '!', '<', '>', '+', '-', '*', '/', '%', '&', '|', '^'])
    })
}

/// `r"`, `r#"`, `r##"` at the start of `bytes`: the number of `#`s.
fn raw_string_hashes(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'r') {
        return None;
    }
    let hashes = bytes[1..].iter().take_while(|&&c| c == b'#').count();
    (bytes.get(1 + hashes) == Some(&b'"')).then_some(hashes)
}

fn ident_byte_before(bytes: &[u8], i: usize) -> bool {
    i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')
}

fn layout(source: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut pending_blank = false;
    for line in scan(source) {
        if line.verbatim_start {
            out.push(line.text.to_owned());
            pending_blank = false;
            continue;
        }
        let body = if line.open_end {
            line.text.trim_start()
        } else {
            line.text.trim()
        };
        if body.is_empty() {
            pending_blank = true;
            continue;
        }
        let opens_after_blank = out.last().is_some_and(|prev| opens_block(prev));
        let closes = body.starts_with(['}', ')', ']']);
        if pending_blank && !out.is_empty() && !opens_after_blank && !closes {
            out.push(String::new());
        }
        pending_blank = false;
        out.push(format!("{}{}", INDENT.repeat(line.depth), body));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// The previous emitted line opens a bracket as its last code character.
fn opens_block(line: &str) -> bool {
    let code = match line.find("//") {
        Some(at) if !line[..at].contains('"') => &line[..at],
        _ => line,
    };
    code.trim_end().ends_with(['{', '(', '['])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src).expect("formats")
    }

    #[test]
    fn indents_by_bracket_depth() {
        assert_eq!(
            fmt("f() {\nif x {\ny()\n}\n}\n"),
            "f() {\n    if x {\n        y()\n    }\n}\n"
        );
    }

    #[test]
    fn closing_lines_dedent_and_else_stays_with_brace() {
        assert_eq!(
            fmt("f() {\n  if x {\n  a()\n  } else {\n  b()\n  }\n}"),
            "f() {\n    if x {\n        a()\n    } else {\n        b()\n    }\n}\n"
        );
    }

    #[test]
    fn collapses_blank_runs_and_trims_block_edges() {
        assert_eq!(
            fmt("\n\na = 1\n\n\n\nb = 2\nf() {\n\n    c()\n\n}\n\n"),
            "a = 1\n\nb = 2\nf() {\n    c()\n}\n"
        );
    }

    #[test]
    fn removes_trailing_whitespace_and_normalizes_line_endings() {
        assert_eq!(fmt("a = 1   \r\nb = 2\t\r\n"), "a = 1\nb = 2\n");
    }

    #[test]
    fn multi_line_strings_and_block_comments_are_verbatim() {
        let src =
            "a = \"one\n   two  \n three\"\n/* keep\n      this */\nb = r#\"x\n  \"y\"\n\"#\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn brackets_inside_strings_and_comments_do_not_count() {
        assert_eq!(
            fmt("f() {\nprint(\"{ ( [\") // } ) ]\ng()\n}\n"),
            "f() {\n    print(\"{ ( [\") // } ) ]\n    g()\n}\n"
        );
    }

    #[test]
    fn continuation_lines_indent_inside_parens() {
        assert_eq!(
            fmt("x = call(\n1,\n2,\n)\n"),
            "x = call(\n    1,\n    2,\n)\n"
        );
    }

    #[test]
    fn brackets_opened_on_one_line_indent_once() {
        assert_eq!(
            fmt("m = mat(2, 2, [\n1.0, 0.0,\n0.0, 1.0])\nf(g({\nx: 1\n}))\n"),
            "m = mat(2, 2, [\n    1.0, 0.0,\n    0.0, 1.0])\nf(g({\n    x: 1\n}))\n"
        );
    }

    #[test]
    fn method_chain_lines_indent_one_level() {
        assert_eq!(
            fmt(
                "f() : P {\ncfg = Config.new()\n.timeout(1)\n.limit(2)\nfor 0..3 {\n}\n.{cfg}\n}\n"
            ),
            "f() : P {\n    cfg = Config.new()\n        .timeout(1)\n        .limit(2)\n    for 0..3 {\n    }\n    .{cfg}\n}\n"
        );
    }

    #[test]
    fn brackets_opened_on_a_chain_line_nest_under_it() {
        assert_eq!(
            fmt(
                "app = App.new()\n.crud(\"/t\", rest(\n|r| { a(r) },\n|r| { b(r) }\n))\n.get(\"/\")\n"
            ),
            "app = App.new()\n    .crud(\"/t\", rest(\n        |r| { a(r) },\n        |r| { b(r) }\n    ))\n    .get(\"/\")\n"
        );
    }

    #[test]
    fn expression_bodies_after_arrow_or_assign_hang_one_level() {
        assert_eq!(
            fmt("{\nf(x) =>\nx + 1\ny =\n2\nok = a ==\nb\n}\n"),
            "{\n    f(x) =>\n        x + 1\n    y =\n        2\n    ok = a ==\n    b\n}\n"
        );
    }

    #[test]
    fn formatting_is_idempotent() {
        let src = "+User = {\n id: i64\n\n\n name: str\n greet(self) => \"hi {self.name}\"\n}\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
        assert!(is_formatted(&once).unwrap());
    }

    #[test]
    fn refuses_sources_that_do_not_lex() {
        assert!(matches!(
            format_source("a = \"open\n"),
            Err(FormatError::Lex(_))
        ));
    }

    #[test]
    fn every_repository_source_formats_idempotently() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut stack = vec![root.join("examples"), root.join("pods")];
        let mut checked = 0;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !matches!(
                        path.file_name().and_then(|n| n.to_str()),
                        Some("target" | "vendor")
                    ) {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "sr") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let once =
                        format_source(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    assert_eq!(format_source(&once).unwrap(), once, "{}", path.display());
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "only {checked} files found");
    }

    #[test]
    fn every_stdlib_file_formats_idempotently() {
        for (path, text) in crate::modules::EMBEDDED_STDLIB {
            let once = format_source(text).unwrap_or_else(|e| panic!("{path}: {e}"));
            let twice = format_source(&once).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(once, twice, "{path} is not idempotent");
        }
    }
}
