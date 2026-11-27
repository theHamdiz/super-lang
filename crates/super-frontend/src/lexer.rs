//! Super lexer — zero-allocation over source bytes with interned identifiers.

use crate::token::{Token, TokenKind};
use super_core::{BytePos, DiagnosticKind, Diagnostics, Interner, Span};

pub struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    interner: &'a mut Interner,
    diagnostics: &'a mut Diagnostics,
    /// Previous non-trivia token kind — drives `n-1` vs unary `-1` lexing.
    last_kind: Option<TokenKind>,
    /// Module index stamped onto every span. `0` is a standalone file.
    file: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(
        source: &'a str,
        interner: &'a mut Interner,
        diagnostics: &'a mut Diagnostics,
    ) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            interner,
            diagnostics,
            last_kind: None,
            file: 0,
        }
    }

    /// Tag spans with the merged-module index so they do not alias another file.
    pub fn with_file(mut self, file: u32) -> Self {
        self.file = file;
        self
    }

    pub fn tokenize(mut self) -> Vec<Token> {
        let mut tokens = Vec::with_capacity(self.bytes.len() / 4 + 8);
        loop {
            let tok = self.next_token();
            let is_eof = tok.kind == TokenKind::Eof;
            if !is_eof {
                self.last_kind = Some(tok.kind);
            }
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let start = self.pos;
        if self.pos >= self.bytes.len() {
            return Token::new(TokenKind::Eof, self.span_at(start));
        }
        let b = self.bytes[self.pos];
        match b {
            b'(' => self.simple(TokenKind::LParen, start),
            b')' => self.simple(TokenKind::RParen, start),
            b'{' => self.simple(TokenKind::LBrace, start),
            b'}' => self.simple(TokenKind::RBrace, start),
            b'[' => self.simple(TokenKind::LBracket, start),
            b']' => self.simple(TokenKind::RBracket, start),
            b',' => self.simple(TokenKind::Comma, start),
            b';' => self.simple(TokenKind::Semicolon, start),
            b'?' => self.simple(TokenKind::Question, start),
            b'@' => self.simple(TokenKind::At, start),
            b':' => self.simple(TokenKind::Colon, start),
            b'+' => self.compound2(start, b'=', TokenKind::PlusEq, TokenKind::Plus),
            b'*' => self.compound2(start, b'=', TokenKind::StarEq, TokenKind::Star),
            b'%' => self.simple(TokenKind::Percent, start),
            b'-' => {
                if self.peek_at(1) == Some(b'=') {
                    self.pos += 2;
                    Token::new(TokenKind::MinusEq, self.span(start))
                } else if self.peek_at(1).is_some_and(|c| c.is_ascii_digit())
                    && !self.prev_is_value()
                {
                    self.pos += 1;
                    self.number(start)
                } else {
                    self.simple(TokenKind::Minus, start)
                }
            }
            b'/' => match self.slash_class() {
                SlashClass::MetaLine => self.meta_comment_line(start),
                SlashClass::MetaBlock => self.meta_comment_block(start),
                SlashClass::DocOuter => self.doc_line(start, false),
                SlashClass::DocInner => self.doc_line(start, true),
                SlashClass::DocBlockOuter => self.doc_block(start, false),
                SlashClass::DocBlockInner => self.doc_block(start, true),
                SlashClass::LineComment => {
                    self.skip_line_comment();
                    self.next_token()
                }
                SlashClass::BlockComment => {
                    self.skip_block_comment();
                    self.next_token()
                }
                SlashClass::Operator => {
                    self.compound2(start, b'=', TokenKind::SlashEq, TokenKind::Slash)
                }
            },
            b'#' => self.simple(TokenKind::Hash, start),
            b'$' => {
                if self.peek_at(1) == Some(b'{') {
                    self.pos += 2;
                    Token::new(TokenKind::SpliceStart, self.span(start))
                } else {
                    self.simple(TokenKind::Dollar, start)
                }
            }
            b'\'' => {
                if self.peek_at(1) == Some(b'{') {
                    self.pos += 2;
                    Token::new(TokenKind::QuoteStart, self.span(start))
                } else {
                    self.pos += 1;
                    let span = self.span(start);
                    self.diagnostics.error_code(
                        DiagnosticKind::Syntax,
                        span,
                        super_core::err::E0001,
                        "expected '{ after quote tick",
                    );
                    Token::new(TokenKind::Error, span)
                }
            }
            b'=' => {
                if self.peek_at(1) == Some(b'=') {
                    self.pos += 2;
                    Token::new(TokenKind::EqEq, self.span(start))
                } else if self.peek_at(1) == Some(b'>') {
                    self.pos += 2;
                    Token::new(TokenKind::FatArrow, self.span(start))
                } else {
                    self.simple(TokenKind::Eq, start)
                }
            }
            b'!' => {
                if self.peek_at(1) == Some(b'=') {
                    self.pos += 2;
                    Token::new(TokenKind::NotEq, self.span(start))
                } else {
                    self.simple(TokenKind::Bang, start)
                }
            }
            b'.' => {
                if self.peek_at(1) == Some(b'.') {
                    self.pos += 2;
                    Token::new(TokenKind::DotDot, self.span(start))
                } else if self.peek_at(1) == Some(b'{') {
                    // `.{` is Dot then LBrace — leave as Dot for parser
                    self.simple(TokenKind::Dot, start)
                } else {
                    self.simple(TokenKind::Dot, start)
                }
            }
            b'|' => {
                if self.peek_at(1) == Some(b'|') {
                    self.pos += 2;
                    Token::new(TokenKind::PipePipe, self.span(start))
                } else {
                    self.simple(TokenKind::Pipe, start)
                }
            }
            b'&' => {
                if self.peek_at(1) == Some(b'&') {
                    self.pos += 2;
                    Token::new(TokenKind::AmpAmp, self.span(start))
                } else {
                    self.simple(TokenKind::Amp, start)
                }
            }
            b'^' => self.simple(TokenKind::Caret, start),
            b'~' => self.simple(TokenKind::Tilde, start),
            b'<' => {
                if self.peek_at(1) == Some(b'<') {
                    self.pos += 2;
                    Token::new(TokenKind::LessLess, self.span(start))
                } else {
                    self.compound2(start, b'=', TokenKind::LtEq, TokenKind::Lt)
                }
            }
            b'>' => {
                if self.peek_at(1) == Some(b'>') {
                    if self.peek_at(2) == Some(b'>') {
                        self.pos += 3;
                        Token::new(TokenKind::GreaterGreaterGreater, self.span(start))
                    } else {
                        self.pos += 2;
                        Token::new(TokenKind::GreaterGreater, self.span(start))
                    }
                } else {
                    self.compound2(start, b'=', TokenKind::GtEq, TokenKind::Gt)
                }
            }
            b'"' => self.string(start),
            b'r' if self.raw_string_ahead() => self.raw_string(start),
            c if c.is_ascii_digit() => self.number(start),
            c if is_ident_start(c) => self.ident(start),
            _ => {
                self.pos += 1;
                let span = self.span(start);
                self.diagnostics.error_code(
                    DiagnosticKind::Syntax,
                    span,
                    super_core::err::E0001,
                    format!("unexpected character {:?}", b as char),
                );
                Token::new(TokenKind::Error, span)
            }
        }
    }

    fn prev_is_value(&self) -> bool {
        matches!(
            self.last_kind,
            Some(
                TokenKind::Ident
                    | TokenKind::Int
                    | TokenKind::Float
                    | TokenKind::String
                    | TokenKind::True
                    | TokenKind::False
                    | TokenKind::None
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
            )
        )
    }

    fn simple(&mut self, kind: TokenKind, start: usize) -> Token {
        self.pos += 1;
        Token::new(kind, self.span(start))
    }

    fn compound2(&mut self, start: usize, next: u8, two: TokenKind, one: TokenKind) -> Token {
        if self.peek_at(1) == Some(next) {
            self.pos += 2;
            Token::new(two, self.span(start))
        } else {
            self.simple(one, start)
        }
    }

    fn ident(&mut self, start: usize) -> Token {
        self.pos += 1;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let kind = TokenKind::from_ident(text);
        let sym = self.interner.intern(text);
        Token::with_sym(kind, self.span(start), sym)
    }

    fn number(&mut self, start: usize) -> Token {
        // Hex / binary / octal: 0x…, 0b…, 0o… (optional `_` digit separators).
        // `pos` still sits on the leading `0` when `number` is entered.
        if self.bytes.get(start) == Some(&b'0') {
            match self.peek_at(1) {
                Some(b'x' | b'X') => {
                    self.pos += 2;
                    return self.radix_digits(start, |c| c.is_ascii_hexdigit());
                }
                Some(b'b' | b'B') => {
                    self.pos += 2;
                    return self.radix_digits(start, |c| c == b'0' || c == b'1');
                }
                Some(b'o' | b'O') => {
                    self.pos += 2;
                    return self.radix_digits(start, |c| (b'0'..=b'7').contains(&c));
                }
                _ => {}
            }
        }

        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            // Don't treat `1..n` range as a float.
            if self.peek_at(1) != Some(b'.') {
                is_float = true;
                self.pos += 1;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
                    self.pos += 1;
                }
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let exp_digit = self
                .peek_at(1)
                .is_some_and(|c| c.is_ascii_digit() || c == b'+' || c == b'-');
            if exp_digit {
                is_float = true;
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
                    self.pos += 1;
                }
            }
        }
        let text = &self.source[start..self.pos];
        let sym = self.interner.intern(text);
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };
        Token::with_sym(kind, self.span(start), sym)
    }

    fn radix_digits(&mut self, start: usize, mut is_digit: impl FnMut(u8) -> bool) -> Token {
        let mut saw_digit = false;
        while let Some(c) = self.peek() {
            if c == b'_' {
                self.pos += 1;
                continue;
            }
            if is_digit(c) {
                saw_digit = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !saw_digit {
            let span = self.span(start);
            self.diagnostics.error_code(
                DiagnosticKind::Syntax,
                span,
                super_core::err::E0001,
                "expected digits after radix prefix",
            );
            return Token::new(TokenKind::Error, span);
        }
        let text = &self.source[start..self.pos];
        let sym = self.interner.intern(text);
        Token::with_sym(TokenKind::Int, self.span(start), sym)
    }

    fn string(&mut self, start: usize) -> Token {
        self.pos += 1; // opening "
        while let Some(c) = self.peek() {
            if c == b'"' {
                self.pos += 1;
                let text = &self.source[start + 1..self.pos - 1];
                let sym = self.interner.intern(text);
                return Token::with_sym(TokenKind::String, self.span(start), sym);
            }
            if c == b'\\' {
                self.escape_sequence();
                continue;
            }
            // allow `{` for interpolation — still part of string content for lexer
            self.pos += 1;
        }
        let span = self.span(start);
        self.diagnostics
            .error(DiagnosticKind::Syntax, span, "unterminated string");
        Token::new(TokenKind::Error, span)
    }

    /// Validate one `\…` escape at `pos` (which sits on the backslash) and advance past it.
    ///
    /// Accepted: `\n \r \t \0 \\ \" \' \{ \}`, `\xNN` (`NN` ≤ 0x7F) and `\u{H…}`
    /// (1–6 hex digits, a valid Unicode scalar). Decoding happens at compile time
    /// (`parse_string_parts`); this pass only rejects malformed escapes early.
    fn escape_sequence(&mut self) {
        let esc_start = self.pos;
        self.pos += 1; // backslash
        let Some(c) = self.peek() else { return };
        self.pos += 1;
        let problem: Option<String> = match c {
            b'n' | b'r' | b't' | b'0' | b'\\' | b'"' | b'\'' | b'{' | b'}' => None,
            b'x' => {
                let hi = self.peek().and_then(hex_val);
                let lo = self.peek_at(1).and_then(hex_val);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        self.pos += 2;
                        (h * 16 + l > 0x7F).then(|| {
                            "`\\xNN` escape must be ≤ 0x7F (use `\\u{…}` for non-ASCII)".to_owned()
                        })
                    }
                    _ => Some("`\\x` escape needs exactly two hex digits".to_owned()),
                }
            }
            b'u' => {
                if self.peek() != Some(b'{') {
                    Some("`\\u` escape must be written `\\u{…}`".to_owned())
                } else {
                    self.pos += 1;
                    let digits_start = self.pos;
                    while self.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                        self.pos += 1;
                    }
                    let digits = &self.source[digits_start..self.pos];
                    let closed = self.peek() == Some(b'}');
                    if closed {
                        self.pos += 1;
                    }
                    let scalar = u32::from_str_radix(digits, 16)
                        .ok()
                        .and_then(char::from_u32);
                    if !closed || digits.is_empty() || digits.len() > 6 || scalar.is_none() {
                        Some(
                            "`\\u{…}` needs 1-6 hex digits forming a valid Unicode scalar"
                                .to_owned(),
                        )
                    } else {
                        None
                    }
                }
            }
            other => {
                // Skip the whole UTF-8 scalar so the span stays on a char boundary.
                while self.pos < self.bytes.len() && !self.source.is_char_boundary(self.pos) {
                    self.pos += 1;
                }
                Some(format!(
                    "unknown escape sequence `\\{}` (use `\\\\` for a literal backslash, or a raw string `r\"…\"`)",
                    if other.is_ascii() {
                        (other as char).to_string()
                    } else {
                        "?".to_owned()
                    }
                ))
            }
        };
        if let Some(msg) = problem {
            let span = self.span(esc_start);
            self.diagnostics
                .error_code(DiagnosticKind::Syntax, span, super_core::err::E0001, msg);
        }
    }

    /// `r"…"`, `r#"…"#`, `r##"…"##` — is a raw string literal starting at `pos` (on the `r`)?
    fn raw_string_ahead(&self) -> bool {
        let mut i = self.pos + 1;
        while self.bytes.get(i) == Some(&b'#') {
            i += 1;
        }
        self.bytes.get(i) == Some(&b'"')
    }

    /// Raw string: no escapes, no interpolation, may span lines. The body is stored
    /// re-escaped (`\`, `"`, `{`, `}` backslash-prefixed) so every downstream consumer
    /// that decodes ordinary string tokens yields exactly the source bytes.
    fn raw_string(&mut self, start: usize) -> Token {
        self.pos += 1; // r
        let mut hashes = 0usize;
        while self.peek() == Some(b'#') {
            hashes += 1;
            self.pos += 1;
        }
        self.pos += 1; // opening "
        let body_start = self.pos;
        while self.pos < self.bytes.len() {
            if self.bytes[self.pos] == b'"'
                && (1..=hashes).all(|k| self.bytes.get(self.pos + k) == Some(&b'#'))
            {
                let body = &self.source[body_start..self.pos];
                self.pos += 1 + hashes;
                let mut encoded = String::with_capacity(body.len() + 8);
                for ch in body.chars() {
                    if matches!(ch, '\\' | '"' | '{' | '}') {
                        encoded.push('\\');
                    }
                    encoded.push(ch);
                }
                let sym = self.interner.intern(&encoded);
                return Token::with_sym(TokenKind::String, self.span(start), sym);
            }
            self.pos += 1;
        }
        let span = self.span(start);
        self.diagnostics
            .error(DiagnosticKind::Syntax, span, "unterminated raw string");
        Token::new(TokenKind::Error, span)
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.pos += 1;
            }
            // Meta-comments and documentation comments are tokens.
            match self.slash_class() {
                SlashClass::LineComment => {
                    self.skip_line_comment();
                    continue;
                }
                SlashClass::BlockComment => {
                    self.skip_block_comment();
                    continue;
                }
                _ => break,
            }
        }
    }

    fn slash_class(&self) -> SlashClass {
        if self.peek() != Some(b'/') {
            return SlashClass::Operator;
        }
        match self.peek_at(1) {
            Some(b'/') => match self.peek_at(2) {
                Some(b'#') => SlashClass::MetaLine,
                // `///` documents the next declaration. `////` stays an ordinary comment.
                Some(b'/') if self.peek_at(3) != Some(b'/') => SlashClass::DocOuter,
                Some(b'!') => SlashClass::DocInner,
                _ => SlashClass::LineComment,
            },
            Some(b'*') => match self.peek_at(2) {
                Some(b'#') => SlashClass::MetaBlock,
                // `/**` documents the next declaration. `/***` and `/**/` stay block comments.
                Some(b'*') if !matches!(self.peek_at(3), Some(b'*') | Some(b'/')) => {
                    SlashClass::DocBlockOuter
                }
                Some(b'!') => SlashClass::DocBlockInner,
                _ => SlashClass::BlockComment,
            },
            _ => SlashClass::Operator,
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == b'\n' {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self) {
        self.pos += 2; // /*
        while self.pos + 1 < self.bytes.len() {
            if self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/' {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
        self.pos = self.bytes.len();
    }

    fn meta_comment_line(&mut self, start: usize) -> Token {
        self.pos += 3; // //#
        let content_start = self.pos;
        while self.peek().is_some_and(|c| c != b'\n') {
            self.pos += 1;
        }
        let text = self.source[content_start..self.pos].trim();
        let sym = self.interner.intern(text);
        if self.peek() == Some(b'\n') {
            self.pos += 1;
        }
        Token::with_sym(TokenKind::MetaCommentLine, self.span(start), sym)
    }

    fn meta_comment_block(&mut self, start: usize) -> Token {
        self.pos += 3; // /*#
        let content_start = self.pos;
        while self.pos + 2 < self.bytes.len() {
            if self.bytes[self.pos] == b'#'
                && self.bytes[self.pos + 1] == b'*'
                && self.bytes[self.pos + 2] == b'/'
            {
                let text = self.source[content_start..self.pos].trim();
                let sym = self.interner.intern(text);
                self.pos += 3;
                return Token::with_sym(TokenKind::MetaCommentBlock, self.span(start), sym);
            }
            self.pos += 1;
        }
        let span = self.span(start);
        self.diagnostics.error(
            DiagnosticKind::Syntax,
            span,
            "unterminated /*# meta-comment",
        );
        Token::new(TokenKind::Error, span)
    }

    fn doc_line(&mut self, start: usize, inner: bool) -> Token {
        self.pos += 3; // /// or //!
        let mut content_start = self.pos;
        if self.peek() == Some(b' ') {
            content_start += 1;
            self.pos += 1;
        }
        while self.peek().is_some_and(|c| c != b'\n') {
            self.pos += 1;
        }
        let text = self.source[content_start..self.pos].trim_end_matches([' ', '\r']);
        let sym = self.interner.intern(text);
        let kind = if inner {
            TokenKind::DocInner
        } else {
            TokenKind::DocOuter
        };
        Token::with_sym(kind, self.span(start), sym)
    }

    fn doc_block(&mut self, start: usize, inner: bool) -> Token {
        self.pos += 3; // /** or /*!
        let content_start = self.pos;
        while self.pos + 1 < self.bytes.len() {
            if self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/' {
                let raw = &self.source[content_start..self.pos];
                let text = clean_block_doc(raw);
                let sym = self.interner.intern(&text);
                self.pos += 2;
                let kind = if inner {
                    TokenKind::DocInner
                } else {
                    TokenKind::DocOuter
                };
                return Token::with_sym(kind, self.span(start), sym);
            }
            self.pos += 1;
        }
        let span = self.span(start);
        self.diagnostics.error(
            DiagnosticKind::Syntax,
            span,
            "unterminated documentation comment",
        );
        Token::new(TokenKind::Error, span)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn span(&self, start: usize) -> Span {
        Span::new(BytePos(start as u32), BytePos(self.pos as u32)).with_file(self.file)
    }

    fn span_at(&self, start: usize) -> Span {
        Span::empty(BytePos(start as u32)).with_file(self.file)
    }
}

fn hex_val(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

enum SlashClass {
    MetaLine,
    MetaBlock,
    DocOuter,
    DocInner,
    DocBlockOuter,
    DocBlockInner,
    LineComment,
    BlockComment,
    Operator,
}

/// Strip a leading ` *` margin from a block documentation comment body.
fn clean_block_doc(raw: &str) -> String {
    let mut lines: Vec<String> = raw
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('*') {
                rest.strip_prefix(' ').unwrap_or(rest).to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect();
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_core::Interner;

    fn lex(src: &str) -> Vec<TokenKind> {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        Lexer::new(src, &mut interner, &mut diags)
            .tokenize()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn lexes_bindings_and_exports() {
        let kinds = lex("+TaxRate = 0.14");
        assert!(kinds.contains(&TokenKind::Plus));
        assert!(kinds.contains(&TokenKind::Ident));
        assert!(kinds.contains(&TokenKind::Eq));
        assert!(kinds.contains(&TokenKind::Float));
    }

    #[test]
    fn lexes_keywords_and_arrows() {
        let kinds = lex(r#"when x { it > 1 => print("a") else => print("b") }"#);
        assert!(kinds.contains(&TokenKind::When));
        assert!(kinds.contains(&TokenKind::FatArrow));
        assert!(kinds.contains(&TokenKind::Else));
        assert!(kinds.contains(&TokenKind::String));
    }

    #[test]
    fn lexes_select_keyword() {
        let kinds = lex("select { after 1 => 0 }");
        assert!(kinds.contains(&TokenKind::Select));
        assert!(kinds.contains(&TokenKind::After));
        assert!(kinds.contains(&TokenKind::FatArrow));
    }

    #[test]
    fn lexes_at_and_range() {
        let kinds = lex("for @scores { it += 5 } for 0..100 { }");
        assert!(kinds.contains(&TokenKind::For));
        assert!(kinds.contains(&TokenKind::At));
        assert!(kinds.contains(&TokenKind::DotDot));
        assert!(kinds.contains(&TokenKind::PlusEq));
    }

    #[test]
    fn lexes_range_with_call_rhs_not_merged_into_number() {
        // Regression: `0..len(xs)` must be Int / DotDot / Ident / … — never a
        // single numeric span covering `0..a`.
        let src = "for 0..len(xs) { } for 0..foo(bar.baz) { }";
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let toks = Lexer::new(src, &mut interner, &mut diags).tokenize();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());

        let texts: Vec<&str> = toks
            .iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| &src[t.span.lo.as_usize()..t.span.hi.as_usize()])
            .collect();
        assert!(
            texts.contains(&".."),
            "expected DotDot token, got {texts:?}"
        );
        assert!(texts.contains(&"len"), "expected len ident, got {texts:?}");
        assert!(texts.contains(&"foo"), "expected foo ident, got {texts:?}");
        for t in &toks {
            if matches!(t.kind, TokenKind::Int | TokenKind::Float) {
                let slice = &src[t.span.lo.as_usize()..t.span.hi.as_usize()];
                assert!(
                    !slice.contains(".."),
                    "numeric token must not swallow range: {slice:?}"
                );
            }
        }
        // Exact sequence around the first range.
        let kinds: Vec<TokenKind> = toks.iter().map(|t| t.kind).collect();
        let needle = [
            TokenKind::Int,
            TokenKind::DotDot,
            TokenKind::Ident,
            TokenKind::LParen,
            TokenKind::Ident,
            TokenKind::RParen,
        ];
        assert!(
            kinds.windows(needle.len()).any(|w| w == needle),
            "expected Int DotDot Ident ( Ident ) sequence, got {kinds:?}"
        );
    }

    #[test]
    fn lexes_metaprogramming_tokens() {
        let kinds = lex("//# unroll(4)\n#N = 4\n'{ x }\n${ y }");
        assert!(kinds.contains(&TokenKind::MetaCommentLine));
        assert!(kinds.contains(&TokenKind::Hash));
        assert!(kinds.contains(&TokenKind::QuoteStart));
        assert!(kinds.contains(&TokenKind::SpliceStart));
    }

    #[test]
    fn doc_comments_are_tokens_ordinary_comments_are_not() {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let toks = Lexer::new(
            "/// Adds one.\n//! module\n//// still a comment\n/** block */\n/*! inner */\n/***/\n/**/\n+add(a: i64) = a + 1\n",
            &mut interner,
            &mut diags,
        )
        .tokenize();
        let docs: Vec<_> = toks
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::DocOuter | TokenKind::DocInner))
            .map(|t| {
                (
                    t.kind,
                    t.sym
                        .map(|s| interner.resolve(s).to_string())
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            docs,
            vec![
                (TokenKind::DocOuter, "Adds one.".into()),
                (TokenKind::DocInner, "module".into()),
                (TokenKind::DocOuter, "block".into()),
                (TokenKind::DocInner, "inner".into()),
            ]
        );
        assert!(!diags.has_errors());
    }

    #[test]
    fn inert_comments_stripped_meta_kept() {
        let kinds = lex("// plain\n//# inspect(T)\nx = 1");
        assert!(kinds.contains(&TokenKind::MetaCommentLine));
        assert!(!kinds.iter().any(|k| matches!(k, TokenKind::Error)));
        assert!(kinds.contains(&TokenKind::Ident));
    }

    /// Lex `src`, returning the interned text of the first `String` token and whether
    /// any error diagnostic fired.
    fn first_string(src: &str) -> (Option<String>, bool) {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let toks = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let text = toks
            .iter()
            .find(|t| t.kind == TokenKind::String)
            .and_then(|t| t.sym)
            .map(|s| interner.resolve(s).to_owned());
        (text, diags.has_errors())
    }

    #[test]
    fn every_standard_escape_is_accepted() {
        for esc in [
            r"\n",
            r"\r",
            r"\t",
            r"\0",
            r"\\",
            r#"\""#,
            r"\'",
            r"\{",
            r"\}",
            r"\x41",
            r"\x7f",
            r"\u{41}",
            r"\u{1F600}",
            r"\u{10FFFF}",
        ] {
            let src = format!("\"a{esc}b\"");
            let (text, err) = first_string(&src);
            assert!(!err, "escape {esc} rejected");
            assert_eq!(text.as_deref(), Some(&src[1..src.len() - 1]), "{esc}");
        }
    }

    #[test]
    fn escaped_quote_does_not_terminate_string() {
        let (text, err) = first_string(r#""say \"hi\" \\" x"#);
        assert!(!err);
        assert_eq!(text.as_deref(), Some(r#"say \"hi\" \\"#));
    }

    #[test]
    fn malformed_escapes_are_diagnosed() {
        for bad in [
            r#""\q""#,
            r#""\x4""#,
            r#""\xZZ""#,
            r#""\xFF""#,
            r#""\u0041""#,
            r#""\u{}""#,
            r#""\u{110000}""#,
            r#""\u{D800}""#,
            r#""\u{1234567}""#,
            r#""\u{41""#,
            r#""\d+""#,
        ] {
            let (_, err) = first_string(bad);
            assert!(err, "{bad} should be an error");
        }
    }

    #[test]
    fn raw_string_keeps_backslashes_verbatim() {
        // Stored re-escaped, so the standard decoder reproduces the source exactly.
        let (text, err) = first_string(r#"r"C:\Users\n\d+""#);
        assert!(!err);
        assert_eq!(text.as_deref(), Some(r"C:\\Users\\n\\d+"));
    }

    #[test]
    fn raw_string_disables_interpolation_and_escapes() {
        let (text, err) = first_string(r#"r"{name} \q \x \u""#);
        assert!(!err, "no escape validation inside raw strings");
        assert_eq!(text.as_deref(), Some(r"\{name\} \\q \\x \\u"));
    }

    #[test]
    fn raw_string_hash_delimiters_allow_quotes() {
        let (text, err) = first_string(r###"r#"say "hi" \d"#"###);
        assert!(!err);
        assert_eq!(text.as_deref(), Some(r#"say \"hi\" \\d"#));
        let (text, err) = first_string(r####"r##"a "# b"##"####);
        assert!(!err);
        assert_eq!(text.as_deref(), Some(r##"a \"# b"##));
    }

    #[test]
    fn raw_string_spans_lines_and_terminates() {
        let (text, err) = first_string("r\"a\\\nb\" x");
        assert!(!err);
        assert_eq!(text.as_deref(), Some("a\\\\\nb"));
    }

    #[test]
    fn unterminated_raw_string_is_error() {
        let (text, err) = first_string(r##"r#"never closed" "##);
        assert!(err);
        assert!(text.is_none());
    }

    #[test]
    fn identifier_r_is_not_a_raw_string() {
        let kinds = lex("r = 1\nrx = r + 2");
        assert!(!kinds.contains(&TokenKind::String));
        assert!(!kinds.contains(&TokenKind::Error));
        let kinds = lex("route = r");
        assert!(!kinds.contains(&TokenKind::String));
    }
}
