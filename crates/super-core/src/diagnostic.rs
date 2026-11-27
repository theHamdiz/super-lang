//! Compiler and LSP diagnostics.

use crate::span::{SourceFile, Span};
use std::fmt;
use std::io::{self, IsTerminal};

/// Stable Super error codes (`E0101`, …) for diagnostics.
pub mod err {
    pub const E0001: &str = "E0001"; // syntax / lexer / parser
    pub const E0101: &str = "E0101"; // undefined name
    pub const E0102: &str = "E0102"; // import / scope
    pub const E0103: &str = "E0103"; // unknown field
    pub const E0104: &str = "E0104"; // private field
    pub const E0201: &str = "E0201"; // type mismatch
    pub const E0202: &str = "E0202"; // optional / error needs `or`
    pub const E0203: &str = "E0203"; // trait conformance
    pub const E0204: &str = "E0204"; // language-trait witness (missing declaration or drifted core trait)
    pub const E0205: &str = "E0205"; // ambiguous multiple dispatch
    pub const E0206: &str = "E0206"; // duplicate overload (identical canonical params)
    pub const E0301: &str = "E0301"; // missing `@` for in-place mutation
    pub const E0302: &str = "E0302"; // cross-spawn mutable capture
    pub const E0303: &str = "E0303"; // unused variable
    pub const E0304: &str = "E0304"; // unused parameter
    pub const E0305: &str = "E0305"; // unused import / binding
    pub const E0401: &str = "E0401"; // escaping `@` root lifetime
    pub const E0402: &str = "E0402"; // escaping non-`@` stack borrow
    pub const E0501: &str = "E0501"; // comptime / meta
    pub const E0502: &str = "E0502"; // law violation (counterexample)
    pub const E0503: &str = "E0503"; // incomplete or invalid proof
    pub const E0504: &str = "E0504"; // LAW-BENCH-001: host import/call in benchmarks/**/*.sr
    pub const E0505: &str = "E0505"; // LAW-BENCH-002: bench-shaped host/C API in super-host / c_emit

    // std.html.template — stable forever once released (never renumber).
    pub const T001: &str = "T001"; // template syntax
    pub const T002: &str = "T002"; // unknown template name / partial
    pub const T003: &str = "T003"; // value not renderable
    pub const T004: &str = "T004"; // unsafe HTML context (script/style/unquoted attr without typed encoder)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Syntax,
    Type,
    Mutation,
    Lifetime,
    Runtime,
    Other,
}

impl DiagnosticKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Type => "type",
            Self::Mutation => "mutation",
            Self::Lifetime => "lifetime",
            Self::Runtime => "runtime",
            Self::Other => "super",
        }
    }
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
    pub span: Span,
    pub message: String,
    /// Optional stable code (`E0104`, …) shown in `render()`.
    pub code: Option<&'static str>,
    /// Optional one-line fix / context for humans and LSP.
    pub help: Option<String>,
}

/// ANSI / plain styling for terminal diagnostics.
#[derive(Clone, Copy, Debug)]
struct Style {
    color: bool,
}

impl Style {
    fn plain() -> Self {
        Self { color: false }
    }

    fn auto() -> Self {
        Self {
            color: wants_color(),
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn bold(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[1;{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// True when stderr should use ANSI colors.
///
/// Precedence: `FORCE_COLOR` (non-empty, not `0`) → on; else `NO_COLOR` → off;
/// else stderr is a TTY.
pub fn wants_color() -> bool {
    match std::env::var("FORCE_COLOR") {
        Ok(v) if !v.is_empty() && v != "0" => return true,
        _ => {}
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    io::stderr().is_terminal()
}

impl Diagnostic {
    pub fn error(kind: DiagnosticKind, span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            kind,
            span,
            message: message.into(),
            code: None,
            help: None,
        }
    }

    pub fn warning(kind: DiagnosticKind, span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            kind,
            span,
            message: message.into(),
            code: None,
            help: None,
        }
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn code_tag(&self) -> String {
        self.code
            .map(|c| c.to_string())
            .unwrap_or_else(|| match self.kind {
                DiagnosticKind::Syntax => err::E0001.to_string(),
                _ => self.kind.label().to_string(),
            })
    }

    fn severity_ansi(&self) -> &'static str {
        match self.severity {
            // True-color tones match `super b` / std.io.console (neon yellow warnings).
            Severity::Error => "38;2;255;84;112",
            Severity::Warning => "38;2;255;255;0",
            Severity::Info => "38;2;64;196;255",
            Severity::Hint => "38;2;0;214;170",
        }
    }

    fn render_head(&self, style: Style) -> String {
        let sev = self.severity.label();
        let tag = self.code_tag();
        let head = format!("{sev}[{tag}]:");
        format!(
            "{} {}",
            style.bold(self.severity_ansi(), &head),
            style.paint("1", &self.message) // bold message body
        )
    }

    /// Stable multi-line rendering (no ANSI). Stable for tests / LSP copy.
    pub fn render(&self, file: &SourceFile) -> String {
        self.render_with(file, Style::plain())
    }

    /// Same layout as [`Self::render`], with ANSI colors when `wants_color()`.
    pub fn render_colored(&self, file: &SourceFile) -> String {
        self.render_with(file, Style::auto())
    }

    /// Force ANSI on/off (for tests and `FORCE_COLOR` / piping).
    pub fn render_with_color(&self, file: &SourceFile, color: bool) -> String {
        self.render_with(file, Style { color })
    }

    fn render_with(&self, file: &SourceFile, style: Style) -> String {
        let ((line, col), _) = file.span_line_col(self.span);
        let path = if file.path.is_empty() {
            "<input>"
        } else {
            file.path.as_str()
        };

        // Pixel-perfect peer toolchain gutter: width from the line number digits (min 1).
        //
        //   error[E0101]: …
        //  --> path:1:5
        //   |
        // 1 | x = missing
        //   |     ^^^^^^^
        //   = help: …
        let gutter = digits(line).max(1);
        let blank = " ".repeat(gutter);
        let pipe = style.paint("34", "|");

        let mut out = String::new();
        out.push_str(&self.render_head(style));
        out.push('\n');

        let loc = format!("{path}:{line}:{col}");
        out.push_str(&format!(
            "{blank}{} {}\n",
            style.bold("34", "-->"),
            style.paint("1", &loc)
        ));
        out.push_str(&format!("{blank} {pipe}\n"));

        if let Some(src_line) = file.line_text(line) {
            let trimmed = src_line.trim_end_matches(['\r', '\n']);
            let mark_len = self.span.len().max(1) as usize;
            let caret_col = col.saturating_sub(1) as usize;
            let available = trimmed.len().saturating_sub(caret_col).max(1);
            let carets = "^".repeat(mark_len.min(available));
            let underline = format!(
                "{spaces}{carets}",
                spaces = " ".repeat(caret_col),
                carets = style.bold(self.severity_ansi(), &carets)
            );

            out.push_str(&format!(
                "{line:>gutter$} {pipe} {trimmed}\n",
                gutter = gutter
            ));
            out.push_str(&format!("{blank} {pipe} {underline}\n"));
        }

        if let Some(help) = &self.help {
            let label = style.bold("32", "=");
            let help_tag = style.paint("1", "help:");
            out.push_str(&format!("{blank} {label} {help_tag} {help}\n"));
        }
        out
    }
}

fn digits(n: u32) -> usize {
    if n == 0 {
        1
    } else {
        ((n as f64).log10().floor() as usize) + 1
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = self.code_tag();
        write!(f, "{}[{}]: {}", self.severity.label(), tag, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn error(&mut self, kind: DiagnosticKind, span: Span, message: impl Into<String>) {
        self.push(Diagnostic::error(kind, span, message));
    }

    pub fn error_help(
        &mut self,
        kind: DiagnosticKind,
        span: Span,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push(Diagnostic::error(kind, span, message).with_help(help));
    }

    pub fn error_code(
        &mut self,
        kind: DiagnosticKind,
        span: Span,
        code: &'static str,
        message: impl Into<String>,
    ) {
        self.push(Diagnostic::error(kind, span, message).with_code(code));
    }

    pub fn error_code_help(
        &mut self,
        kind: DiagnosticKind,
        span: Span,
        code: &'static str,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push(
            Diagnostic::error(kind, span, message)
                .with_code(code)
                .with_help(help),
        );
    }

    pub fn warn_code_help(
        &mut self,
        kind: DiagnosticKind,
        span: Span,
        code: &'static str,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        self.push(
            Diagnostic::warning(kind, span, message)
                .with_code(code)
                .with_help(help),
        );
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Truncate to `len` entries (speculative parse rollback).
    pub fn truncate(&mut self, len: usize) {
        self.items.truncate(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{SourceFile, Span};

    #[test]
    fn tracks_errors() {
        let mut d = Diagnostics::new();
        assert!(!d.has_errors());
        d.error(DiagnosticKind::Syntax, Span::from_offsets(0, 1), "boom");
        assert!(d.has_errors());
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn friendly_render_shows_underline_and_help() {
        let file = SourceFile::new("demo.sr", "x = missing\n");
        let d = Diagnostic::error(
            DiagnosticKind::Type,
            Span::from_offsets(4, 11),
            "undefined name `missing`",
        )
        .with_help("check the spelling, or import it with `use std.…`");
        let d = d.with_code(err::E0101);
        let rendered = d.render(&file);
        assert!(rendered.contains("error[E0101]: undefined name `missing`"));
        assert!(rendered.contains("--> demo.sr:1:5"));
        assert!(rendered.contains("x = missing"));
        assert!(rendered.contains("^"));
        assert!(rendered.contains("help:"));
        let lines: Vec<&str> = rendered.lines().collect();
        assert!(lines.len() >= 4, "{rendered}");
        // All `|` gutters share one column; `-->` starts one column earlier.
        let pipe_cols: Vec<usize> = lines.iter().skip(2).filter_map(|l| l.find('|')).collect();
        assert!(pipe_cols.len() >= 2, "{rendered}");
        assert!(
            pipe_cols.windows(2).all(|w| w[0] == w[1]),
            "misaligned pipes:\n{rendered}"
        );
        let arrow = lines[1].find("-->").expect("arrow");
        assert_eq!(
            arrow + 1,
            pipe_cols[0],
            "rustc gutter geometry:\n{rendered}"
        );
    }

    #[test]
    fn wide_line_numbers_keep_alignment() {
        let mut src = String::new();
        for _ in 0..99 {
            src.push_str("ok\n");
        }
        src.push_str("bad_name = 1\n");
        let file = SourceFile::new("wide.sr", src);
        let lo = file.offset_at_lsp(99, 0).0;
        let d = Diagnostic::error(DiagnosticKind::Type, Span::from_offsets(lo, lo + 8), "demo")
            .with_code(err::E0101);
        let rendered = d.render(&file);
        assert!(rendered.contains("100 |"), "{rendered}");
        let lines: Vec<&str> = rendered.lines().collect();
        let pipe_cols: Vec<usize> = lines.iter().skip(2).filter_map(|l| l.find('|')).collect();
        assert!(
            pipe_cols.windows(2).all(|w| w[0] == w[1]),
            "misaligned pipes:\n{rendered}"
        );
    }

    #[test]
    fn colored_render_emits_ansi_when_forced() {
        let file = SourceFile::new("c.sr", "x = 1\n");
        let d = Diagnostic::error(DiagnosticKind::Type, Span::from_offsets(0, 1), "boom")
            .with_code(err::E0201)
            .with_help("fix it");
        let colored = d.render_with_color(&file, true);
        assert!(colored.contains("\x1b["), "{colored}");
        assert!(colored.contains("error[E0201]"), "{colored}");
        let plain = d.render_with_color(&file, false);
        assert!(!plain.contains("\x1b["), "{plain}");
    }
}
