//! Pratt / recursive-descent parser for Super.

use crate::ast::*;
use crate::token::{ContextualKeyword, Token, TokenKind};
use std::borrow::Cow;
use super_core::{DiagnosticKind, Diagnostics, Interner, Span, SymId};

struct Preamble {
    outer: SymId,
    inner_lines: Vec<String>,
    attrs: Vec<MetaAttr>,
}

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Source text that token spans index into, starting at byte `source_base`
    /// of file `source_file`. Spans outside this window have no text.
    source: &'a str,
    source_base: u32,
    source_file: u32,
    /// `line_starts[i]` is true when token `i` is the first on its line.
    line_starts: Cow<'a, [bool]>,
    /// Byte offsets of every `\n` in `source` (file mode), for macro locations.
    newlines: Vec<u32>,
    /// Location reported by macro calls parsed out of an expansion.
    inherited_location: Option<(u32, u32)>,
    /// Set while parsing `if`/`while`/`for` heads so `#NAME {` stays a value
    /// followed by the body block rather than a brace-delimited macro call.
    in_block_head: bool,
    interner: &'a mut Interner,
    diagnostics: &'a mut Diagnostics,
    /// `//!` / `/*!` lines collected for the enclosing module.
    module_doc_lines: Vec<String>,
}

impl<'a> Parser<'a> {
    pub fn new(
        tokens: &'a [Token],
        source: &'a str,
        interner: &'a mut Interner,
        diagnostics: &'a mut Diagnostics,
    ) -> Self {
        let mut line_starts = Vec::with_capacity(tokens.len());
        let mut prev_hi = 0usize;
        for (index, token) in tokens.iter().enumerate() {
            let lo = token.span.lo.as_usize();
            let starts = index == 0 || source.get(prev_hi..lo).is_none_or(|gap| gap.contains('\n'));
            line_starts.push(starts);
            prev_hi = token.span.hi.as_usize();
        }
        let newlines = source
            .bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(offset, _)| offset as u32)
            .collect();
        Self {
            tokens,
            pos: 0,
            source,
            source_base: 0,
            source_file: tokens.first().map_or(0, |token| token.span.file),
            line_starts: Cow::Owned(line_starts),
            newlines,
            inherited_location: None,
            in_block_head: false,
            interner,
            diagnostics,
            module_doc_lines: Vec::new(),
        }
    }

    /// Parser over tokens produced by macro expansion. `window` is the source
    /// text of the outermost call (`text`, starting at byte `base` of `file`).
    pub fn for_expansion(
        stream: &'a TokenStream,
        window: (&'a str, u32, u32),
        location: (u32, u32),
        interner: &'a mut Interner,
        diagnostics: &'a mut Diagnostics,
    ) -> Self {
        Self {
            tokens: &stream.tokens,
            pos: 0,
            source: window.0,
            source_base: window.1,
            source_file: window.2,
            line_starts: Cow::Borrowed(&stream.line_starts),
            newlines: Vec::new(),
            inherited_location: Some(location),
            in_block_head: false,
            interner,
            diagnostics,
            module_doc_lines: Vec::new(),
        }
    }

    /// Number of tokens consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Consume a `,` if it is next.
    pub fn eat_comma(&mut self) -> bool {
        self.eat(TokenKind::Comma)
    }

    /// Resume parsing at token `pos` (macro fragment matching).
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.tokens.len());
    }

    /// Parse a `{ … }` block (the `block` macro fragment).
    pub fn parse_block_fragment(&mut self) -> Block {
        self.parse_block()
    }

    /// True once every token has been consumed.
    pub fn at_end(&self) -> bool {
        self.pos >= self.tokens.len() || self.is(TokenKind::Eof)
    }

    /// Parse one expression (used by macro fragments and expansions).
    pub fn parse_expression(&mut self) -> Expr {
        self.parse_expr(0)
    }

    /// Parse an expression that binds tighter than comparison, logic, ranges,
    /// `or`, and assignment (the `term` macro fragment).
    pub fn parse_term(&mut self) -> Expr {
        self.parse_expr(15)
    }

    /// Parse a type expression (the `type` macro fragment).
    pub fn parse_type(&mut self) -> TypeExpr {
        self.parse_type_expr()
    }

    /// Parse statements until the end of input, as the body of a block.
    pub fn parse_statements(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        while !self.at_end() {
            let pos = self.pos;
            let stmt = self.parse_stmt();
            if self.pos == pos && matches!(stmt, Stmt::Expr(Expr::None(_))) {
                self.error_here("unexpected token in macro expansion");
                self.advance();
                continue;
            }
            stmts.push(stmt);
        }
        stmts
    }

    /// Text of `span` when it lies inside this parser's source window.
    fn span_text(&self, span: Span) -> Option<&'a str> {
        if span.file_index() != self.source_file || span.lo.0 < self.source_base {
            return None;
        }
        let lo = (span.lo.0 - self.source_base) as usize;
        let hi = (span.hi.0.checked_sub(self.source_base)?) as usize;
        self.source.get(lo..hi)
    }

    pub fn parse_module(&mut self) -> Module {
        let start = self.current().span;
        self.module_doc_lines.clear();
        let mut items = Vec::new();
        while !self.is(TokenKind::Eof) {
            let before = self.pos;
            let group = self.parse_item_group();
            if group.is_empty() {
                if self.pos == before {
                    self.advance();
                }
            } else {
                items.extend(group);
            }
        }
        let end = self.previous_span();
        let docs = if self.module_doc_lines.is_empty() {
            self.blank_doc()
        } else {
            let text = self.module_doc_lines.join("\n");
            self.interner.intern(&text)
        };
        Module {
            items,
            span: start.merge(end),
            docs,
            macros: Vec::new(),
        }
    }

    /// Leading `///` / `/**` (outer), `//!` / `/*!` (inner), and meta attributes.
    fn take_preamble(&mut self) -> Preamble {
        let mut outer_lines = Vec::new();
        let mut inner_lines = Vec::new();
        let mut attrs = Vec::new();
        loop {
            match self.current().kind {
                TokenKind::DocOuter => {
                    let tok = self.advance();
                    outer_lines.push(self.doc_text(tok));
                }
                TokenKind::DocInner => {
                    let tok = self.advance();
                    inner_lines.push(self.doc_text(tok));
                }
                TokenKind::MetaCommentLine | TokenKind::MetaCommentBlock => {
                    attrs.push(self.parse_meta_attr_token());
                }
                TokenKind::Hash if self.looks_like_hash_meta_attr() => {
                    attrs.push(self.parse_hash_meta_attr());
                }
                _ => break,
            }
        }
        let outer = if outer_lines.is_empty() {
            self.blank_doc()
        } else {
            let text = outer_lines.join("\n");
            self.interner.intern(&text)
        };
        Preamble {
            outer,
            inner_lines,
            attrs,
        }
    }

    fn doc_text(&self, tok: Token) -> String {
        tok.sym
            .map(|sym| self.interner.resolve(sym).to_string())
            .unwrap_or_default()
    }

    fn blank_doc(&mut self) -> SymId {
        self.interner.intern("")
    }

    fn doc_empty(&self, docs: SymId) -> bool {
        self.interner.resolve(docs).is_empty()
    }

    fn prepend_doc(&mut self, outer: SymId, inner: SymId) -> SymId {
        let outer_text = self.interner.resolve(outer).to_string();
        let inner_text = self.interner.resolve(inner).to_string();
        match (outer_text.is_empty(), inner_text.is_empty()) {
            (true, _) => inner,
            (_, true) => outer,
            _ => self.interner.intern(&format!("{outer_text}\n{inner_text}")),
        }
    }

    fn join_doc(&mut self, existing: SymId, lines: &[String]) -> SymId {
        if lines.is_empty() {
            return existing;
        }
        let extra = lines.join("\n");
        let prev = self.interner.resolve(existing).to_string();
        if prev.is_empty() {
            self.interner.intern(&extra)
        } else {
            self.interner.intern(&format!("{prev}\n{extra}"))
        }
    }

    /// Skip documentation and meta-comment tokens when peeking.
    fn sig_at(&self, mut index: usize) -> (usize, Option<TokenKind>) {
        loop {
            match self.peek_kind(index) {
                Some(
                    TokenKind::DocOuter
                    | TokenKind::DocInner
                    | TokenKind::MetaCommentLine
                    | TokenKind::MetaCommentBlock,
                ) => index += 1,
                other => return (index, other),
            }
        }
    }

    /// `#inline` / `#simd` / `#unroll(N)` / `#get("/…")` — not `#Name = expr` comptime bindings.
    fn looks_like_hash_meta_attr(&self) -> bool {
        if self.current().kind != TokenKind::Hash {
            return false;
        }
        match self.peek_kind(1) {
            Some(TokenKind::Ident) => {
                let sym = self.tokens.get(self.pos + 1).and_then(|t| t.sym);
                let Some(sym) = sym else {
                    return false;
                };
                crate::intrinsics::is_attribute_macro(self.interner.resolve(sym))
            }
            _ => false,
        }
    }

    fn parse_hash_meta_attr(&mut self) -> MetaAttr {
        let start = self.expect(TokenKind::Hash).span;
        let name_tok = self.advance();
        let name = name_tok
            .sym
            .map(|s| self.interner.resolve(s).to_string())
            .unwrap_or_default();
        let mut text = name;
        if self.eat(TokenKind::LParen) {
            text.push('(');
            // Copy raw args until matching `)`.
            let mut depth = 1usize;
            while !self.is(TokenKind::Eof) && depth > 0 {
                let tok = self.advance();
                match tok.kind {
                    TokenKind::LParen => {
                        depth += 1;
                        text.push('(');
                    }
                    TokenKind::RParen => {
                        depth -= 1;
                        text.push(')');
                    }
                    TokenKind::Comma => text.push(','),
                    TokenKind::Int => {
                        if let Some(s) = tok.sym {
                            text.push_str(self.interner.resolve(s));
                        }
                    }
                    TokenKind::Ident => {
                        if let Some(s) = tok.sym {
                            text.push_str(self.interner.resolve(s));
                        }
                    }
                    TokenKind::Test => text.push_str("test"),
                    TokenKind::String => {
                        // Preserve quotes so `#law("…")` / `#proof(…)` round-trip.
                        if let Some(s) = tok.sym {
                            text.push('"');
                            text.push_str(self.interner.resolve(s));
                            text.push('"');
                        }
                    }
                    TokenKind::Dot => text.push('.'),
                    TokenKind::Colon => text.push(':'),
                    TokenKind::Eq => text.push('='),
                    TokenKind::Bang => text.push('!'),
                    TokenKind::AmpAmp => text.push_str("&&"),
                    TokenKind::PipePipe => text.push_str("||"),
                    TokenKind::Plus | TokenKind::Minus => {
                        // Keep arithmetic punctuation inside meta args when present.
                        text.push(if tok.kind == TokenKind::Plus {
                            '+'
                        } else {
                            '-'
                        });
                    }
                    _ => {}
                }
            }
        }
        let raw = self.interner.intern(&text);
        let kind = parse_meta_directive(&text, self.interner);
        MetaAttr {
            kind,
            span: start.merge(self.previous_span()),
            raw,
        }
    }

    fn parse_meta_attr_token(&mut self) -> MetaAttr {
        let tok = self.advance();
        let raw = tok.sym.unwrap_or_else(|| self.interner.intern(""));
        let text = self.interner.resolve(raw).to_string();
        let kind = parse_meta_directive(&text, self.interner);
        MetaAttr {
            kind,
            span: tok.span,
            raw,
        }
    }

    /// One preamble may expand to several items (`+a, +b, c = 1, 2, 3`).
    fn parse_item_group(&mut self) -> Vec<Item> {
        let pre = self.take_preamble();
        self.module_doc_lines.extend(pre.inner_lines);
        if self.is(TokenKind::Eof) {
            if !self.doc_empty(pre.outer) {
                self.error_here("documentation comment is not attached to a declaration");
            }
            return Vec::new();
        }
        let mut cfg_attrs = Vec::new();
        let mut attrs = Vec::new();
        for attr in pre.attrs {
            if matches!(attr.kind, MetaAttrKind::Cfg(_)) {
                cfg_attrs.push(attr);
            } else {
                attrs.push(attr);
            }
        }
        let docs = pre.outer;
        let items = if self.looks_like_multi_binding_item() {
            self.parse_multi_binding_items(attrs, docs)
        } else {
            match self.parse_item_single(attrs, docs) {
                Some(item) => vec![item],
                None => Vec::new(),
            }
        };
        if cfg_attrs.is_empty() {
            return items;
        }
        let guard_span = cfg_attrs[0].span;
        let mut guarded = Vec::with_capacity(items.len() * 2);
        for item in items {
            guarded.push(Item::Binding(Binding {
                exported: false,
                mutable: false,
                comptime: false,
                name: self.interner.intern("__cfg_guard"),
                ty: None,
                value: Expr::None(guard_span),
                span: guard_span,
                attrs: cfg_attrs.clone(),
                docs: self.blank_doc(),
            }));
            guarded.push(item);
        }
        guarded
    }

    fn parse_item_single(&mut self, attrs: Vec<MetaAttr>, docs: SymId) -> Option<Item> {
        match self.current().kind {
            TokenKind::Use => {
                let item = Item::Use(self.parse_use());
                Some(self.finish_item(item, Vec::new(), docs))
            }
            TokenKind::Test => {
                let item = Item::Test(self.parse_test());
                Some(self.finish_item(item, Vec::new(), docs))
            }
            TokenKind::State => {
                let item = Item::State(self.parse_state());
                Some(self.finish_item(item, Vec::new(), docs))
            }
            TokenKind::Rewrite => {
                let item = Item::Rewrite(self.parse_rewrite());
                Some(self.finish_item(item, Vec::new(), docs))
            }
            TokenKind::Hash if self.looks_like_macro_def() => {
                Some(Item::Macro(self.parse_macro_def(false, docs)))
            }
            TokenKind::Hash => {
                let item = self.parse_comptime_item(false, attrs)?;
                Some(self.finish_item(item, Vec::new(), docs))
            }
            TokenKind::Plus => {
                // `+#NAME = …` exports a comptime binding; `+#name { … }` a macro.
                if self.peek_kind(1) == Some(TokenKind::Hash) {
                    self.advance(); // consume `+`
                    if self.looks_like_macro_def() {
                        return Some(Item::Macro(self.parse_macro_def(true, docs)));
                    }
                    let item = self.parse_comptime_item(true, attrs)?;
                    return Some(self.finish_item(item, Vec::new(), docs));
                }
                let item = self.parse_exported_item()?;
                Some(self.finish_item(item, attrs, docs))
            }
            TokenKind::Ident => {
                let item = self.parse_named_item(false)?;
                Some(self.finish_item(item, attrs, docs))
            }
            _ => {
                if self.is(TokenKind::Ident) && self.looks_like_binding() {
                    let mut b = self.parse_binding(false, false);
                    b.attrs = attrs;
                    b.docs = docs;
                    return Some(Item::Binding(b));
                }
                if !attrs.is_empty() {
                    // Inline //# comptime directive with no following item.
                    return Some(Item::Binding(Binding {
                        exported: false,
                        mutable: false,
                        comptime: true,
                        name: self.interner.intern("__meta_inline"),
                        ty: None,
                        value: Expr::None(attrs[0].span),
                        span: attrs[0].span,
                        attrs,
                        docs,
                    }));
                }
                if !self.doc_empty(docs) {
                    self.error_here("documentation comment is not attached to a declaration");
                }
                self.error_here("expected item");
                None
            }
        }
    }

    /// `+tasks, +next_id, name = [], 1, "Ahmad"` — module-level multi-binding.
    /// Each name may carry its own `+` (export) or stay private; optional `: T`.
    fn looks_like_multi_binding_item(&self) -> bool {
        let mut i = 0usize;
        if matches!(self.peek_kind(i), Some(TokenKind::Plus | TokenKind::Minus)) {
            i += 1;
        }
        let Some(name_kind) = self.peek_kind(i) else {
            return false;
        };
        if !Self::is_binding_name_kind(name_kind) {
            return false;
        }
        if self.looks_like_conforming_type_at(i) {
            return false;
        }
        i += 1;
        if self.peek_kind(i) == Some(TokenKind::Colon) {
            i = self.skip_type_tokens(i + 1);
        }
        self.peek_kind(i) == Some(TokenKind::Comma)
    }

    /// Skip a type-expression's tokens until a top-level `,` / `=` / brace / EOF.
    fn skip_type_tokens(&self, mut i: usize) -> usize {
        let mut depth = 0i32;
        while let Some(k) = self.peek_kind(i) {
            match k {
                TokenKind::LBracket | TokenKind::LParen => {
                    depth += 1;
                    i += 1;
                }
                TokenKind::RBracket | TokenKind::RParen => {
                    depth -= 1;
                    i += 1;
                }
                TokenKind::Comma | TokenKind::Eq | TokenKind::LBrace | TokenKind::RBrace
                    if depth == 0 =>
                {
                    return i;
                }
                TokenKind::Eof => return i,
                _ => i += 1,
            }
        }
        i
    }

    fn parse_multi_binding_items(&mut self, attrs: Vec<MetaAttr>, docs: SymId) -> Vec<Item> {
        let mut targets: Vec<(bool, SymId, Option<TypeExpr>, Span)> = Vec::new();
        loop {
            let exported = if self.eat(TokenKind::Plus) {
                true
            } else {
                let _ = self.eat(TokenKind::Minus);
                false
            };
            let start = self.current().span;
            let name = self.expect_ident();
            let ty = if self.eat(TokenKind::Colon) {
                Some(self.parse_type_expr())
            } else {
                None
            };
            let span = start.merge(self.previous_span());
            targets.push((exported, name, ty, span));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::Eq);
        let mut values = Vec::new();
        loop {
            values.push(self.parse_expr(2));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        if targets.len() != values.len() {
            self.error_here(format!(
                "multi-binding has {} names but {} values",
                targets.len(),
                values.len()
            ));
        }
        let blank = self.blank_doc();
        let mut out = Vec::with_capacity(targets.len());
        for (idx, ((exported, name, ty, tspan), value)) in
            targets.into_iter().zip(values).enumerate()
        {
            let span = tspan.merge(value.span());
            out.push(Item::Binding(Binding {
                exported,
                mutable: false,
                comptime: false,
                name,
                ty,
                value,
                span,
                attrs: if idx == 0 { attrs.clone() } else { Vec::new() },
                docs: if idx == 0 { docs } else { blank },
            }));
        }
        out
    }

    fn finish_item(&mut self, mut item: Item, attrs: Vec<MetaAttr>, docs: SymId) -> Item {
        match &mut item {
            Item::Function(f) => {
                f.attrs.extend(attrs);
                f.docs = self.prepend_doc(docs, f.docs);
            }
            Item::Struct(s) => {
                s.attrs.extend(attrs);
                s.docs = self.prepend_doc(docs, s.docs);
            }
            Item::Sum(s) => {
                s.docs = self.prepend_doc(docs, s.docs);
            }
            Item::Binding(b) => {
                b.attrs.extend(attrs);
                b.docs = self.prepend_doc(docs, b.docs);
            }
            Item::Use(u) => u.docs = docs,
            Item::State(s) => s.docs = docs,
            Item::Test(t) => t.docs = docs,
            Item::Rewrite(r) => r.docs = docs,
            Item::Trait(t) => t.docs = self.prepend_doc(docs, t.docs),
            Item::Macro(m) => m.docs = self.prepend_doc(docs, m.docs),
        }
        item
    }

    fn parse_comptime_item(&mut self, exported: bool, attrs: Vec<MetaAttr>) -> Option<Item> {
        let start = self.expect(TokenKind::Hash).span;
        // #if / #for / #{ as top-level are rare; primarily #Name = expr
        if self.is(TokenKind::If)
            || self.is(TokenKind::For)
            || self.is(TokenKind::Each)
            || self.is(TokenKind::LBrace)
        {
            let expr = self.parse_hash_expr_after_hash(start);
            return Some(Item::Binding(Binding {
                exported,
                mutable: false,
                comptime: true,
                name: self.interner.intern("__comptime_stmt"),
                ty: None,
                value: expr,
                span: start.merge(self.previous_span()),
                attrs,
                docs: self.blank_doc(),
            }));
        }
        let name = self.expect_ident();
        // `#name(#param: T, …) : Ret => body` — named comptime specialization function.
        if self.is(TokenKind::LParen) {
            self.expect(TokenKind::LParen);
            let mut params = Vec::new();
            while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
                params.push(self.parse_param(None));
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RParen);
            let ret = if self.eat(TokenKind::Colon) {
                Some(self.parse_type_expr())
            } else {
                None
            };
            let body = if self.eat(TokenKind::FatArrow) {
                FnBody::Expr(self.parse_expr(0))
            } else if self.is(TokenKind::Eq) {
                self.error_here(
                    "expression function bodies use `=>`, not `=`; write `f(...) => expr` or `f(...) { … }`",
                );
                self.advance();
                FnBody::Expr(self.parse_expr(0))
            } else {
                FnBody::Block(self.parse_block())
            };
            return Some(Item::Function(Function {
                exported,
                name,
                owner: None,
                params,
                ret,
                body,
                span: start.merge(self.previous_span()),
                attrs,
                docs: self.blank_doc(),
            }));
        }
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.parse_type_expr())
        } else {
            None
        };
        self.expect(TokenKind::Eq);
        let value = self.parse_expr(0);
        Some(Item::Binding(Binding {
            exported,
            mutable: false,
            comptime: true,
            name,
            ty,
            value,
            span: start.merge(self.previous_span()),
            attrs,
            docs: self.blank_doc(),
        }))
    }

    fn parse_rewrite(&mut self) -> RewriteRule {
        let start = self.advance().span;
        let name = self.expect_string_sym();
        self.expect(TokenKind::LParen);
        let mut params = Vec::new();
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            params.push(self.parse_param(None));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        self.expect(TokenKind::LBrace);
        let lhs = self.parse_expr(0);
        self.expect(TokenKind::FatArrow);
        let rhs = self.parse_expr(0);
        self.expect(TokenKind::RBrace);
        RewriteRule {
            name,
            params,
            lhs,
            rhs,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_exported_item(&mut self) -> Option<Item> {
        self.expect(TokenKind::Plus);
        self.parse_named_item(true)
    }

    fn parse_named_item(&mut self, exported: bool) -> Option<Item> {
        if self.is_function_header() {
            return Some(Item::Function(self.parse_function(exported)));
        }
        // name = trait { signatures }
        if self.is(TokenKind::Ident)
            && self.peek_kind(1) == Some(TokenKind::Eq)
            && self.peek_kind(2) == Some(TokenKind::Trait)
        {
            return Some(Item::Trait(self.parse_trait(exported)));
        }
        // name = { ... }  → struct or sum
        if self.is(TokenKind::Ident)
            && self.peek_kind(1) == Some(TokenKind::Eq)
            && self.peek_kind(2) == Some(TokenKind::LBrace)
        {
            return Some(self.parse_type_def(exported));
        }
        // +Name: Widget, Other = { … }  — traits declared on the type
        if self.looks_like_conforming_type() {
            return Some(self.parse_type_def(exported));
        }
        if self.looks_like_binding() {
            return Some(Item::Binding(self.parse_binding(exported, false)));
        }
        self.error_here("expected declaration");
        None
    }

    fn is_function_header(&self) -> bool {
        // name( or + already consumed: name(
        if !self.is(TokenKind::Ident) {
            return false;
        }
        self.peek_kind(1) == Some(TokenKind::LParen)
    }

    fn looks_like_binding(&self) -> bool {
        // Soft keywords (`scope`, `after`, `type`, …) may be binding names when
        // followed by `=` / `:`; statement forms keep priority when more specific
        // (e.g. `scope {` via the Scope arm).
        if !Self::is_binding_name_kind(self.current().kind) {
            return false;
        }
        matches!(self.peek_kind(1), Some(TokenKind::Eq | TokenKind::Colon))
    }

    fn is_binding_name_kind(kind: TokenKind) -> bool {
        Self::is_name_token_kind(kind)
            && !matches!(kind, TokenKind::True | TokenKind::False | TokenKind::None)
    }

    /// Soft keywords usable as *expression* identifiers.
    /// Keep this narrow: control-flow keywords (`if`/`for`/`return`/…) have dedicated
    /// primary arms later in `parse_prefix` and must not be stolen here.
    fn is_expr_ident_kind(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Ident
                | TokenKind::State
                | TokenKind::Test
                | TokenKind::Err
                | TokenKind::Use
                | TokenKind::After
                | TokenKind::Type
                | TokenKind::Code
                | TokenKind::Trait
                | TokenKind::Rewrite
                | TokenKind::Scope
                | TokenKind::Parallel // `chan`/`shared` keep dedicated primary arms (see below).
        )
    }

    fn parse_use(&mut self) -> UseItem {
        let start = self.advance().span;
        let mut path = Vec::new();
        path.push(self.expect_ident());
        while self.eat(TokenKind::Dot) {
            // `use auth.{a, b}` — selectors, not another path segment
            if self.is(TokenKind::LBrace) {
                break;
            }
            path.push(self.expect_ident());
        }
        let mut renames = Vec::new();
        let selectors = if self.eat(TokenKind::LBrace) {
            let mut sels = Vec::new();
            while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
                let name = self.expect_ident();
                sels.push(name);
                if self.at_contextual(ContextualKeyword::As) {
                    self.advance();
                    renames.push((name, self.expect_ident()));
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::RBrace);
            Some(sels)
        } else {
            None
        };
        let alias = if self.at_contextual(ContextualKeyword::As) {
            self.advance();
            Some(self.expect_ident())
        } else {
            None
        };
        UseItem {
            path,
            alias,
            selectors,
            renames,
            bindings: Vec::new(),
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_test(&mut self) -> TestBlock {
        let start = self.advance().span; // test
        let name = self.expect_string_sym();
        let body = self.parse_block();
        TestBlock {
            name,
            body,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_state(&mut self) -> StateDef {
        let start = self.advance().span; // state
        let name = self.expect_ident();
        self.expect(TokenKind::Eq);
        let value = self.parse_expr(0);
        StateDef {
            name,
            value,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_trait(&mut self, exported: bool) -> TraitDef {
        let start = self.current().span;
        let name = self.expect_ident();
        self.expect(TokenKind::Eq);
        self.expect(TokenKind::Trait);
        self.expect(TokenKind::LBrace);
        let mut methods = Vec::new();
        let mut docs = self.blank_doc();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let pre = self.take_preamble();
            docs = self.join_doc(docs, &pre.inner_lines);
            if self.is(TokenKind::RBrace) || self.is(TokenKind::Eof) {
                break;
            }
            let mut method = self.parse_trait_method(name);
            method.docs = pre.outer;
            methods.push(method);
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace);
        TraitDef {
            exported,
            name,
            methods,
            span: start.merge(self.previous_span()),
            docs,
        }
    }

    /// `+paint(self, x: i64) : str` with no body.
    fn parse_trait_method(&mut self, owner: SymId) -> TraitMethod {
        let start = self.current().span;
        let _ = self.eat(TokenKind::Plus);
        let _ = self.eat(TokenKind::Minus);
        let name = self.expect_method_name();
        self.expect(TokenKind::LParen);
        let mut params = Vec::new();
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            params.push(self.parse_param(Some(owner)));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        let ret = if self.eat(TokenKind::Colon) {
            Some(self.parse_type_expr())
        } else {
            None
        };
        if self.is(TokenKind::Eq) || self.is(TokenKind::FatArrow) || self.is(TokenKind::LBrace) {
            self.error_here(
                "a trait method is a signature; write the body on `+Name: Trait = { … }`",
            );
            if self.eat(TokenKind::FatArrow) || self.eat(TokenKind::Eq) {
                let _ = self.parse_expr(0);
            } else if self.is(TokenKind::LBrace) {
                let _ = self.parse_block();
            }
        }
        TraitMethod {
            name,
            params,
            ret,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn expect_method_name(&mut self) -> SymId {
        if Self::is_name_token_kind(self.current().kind) {
            return self.expect_ident();
        }
        let tok = self.advance();
        let text = self.source_span(tok.span);
        if text.is_empty() {
            self.interner.intern("_")
        } else {
            self.interner.intern(&text)
        }
    }

    fn source_span(&self, span: Span) -> String {
        self.span_text(span).unwrap_or("").to_string()
    }

    /// `+Name: Widget, Other = {` — a type body, not `name: Type = { key: "value" }`.
    fn looks_like_conforming_type(&self) -> bool {
        self.looks_like_conforming_type_at(0)
    }

    fn looks_like_conforming_type_at(&self, name_at: usize) -> bool {
        if !matches!(
            self.peek_kind(name_at),
            Some(k) if Self::is_binding_name_kind(k)
        ) {
            return false;
        }
        if self.peek_kind(name_at + 1) != Some(TokenKind::Colon) {
            return false;
        }
        let mut i = name_at + 2;
        if self.peek_kind(i) != Some(TokenKind::Ident) {
            return false;
        }
        i += 1;
        while self.peek_kind(i) == Some(TokenKind::Comma) {
            i += 1;
            if self.peek_kind(i) != Some(TokenKind::Ident) {
                return false;
            }
            i += 1;
        }
        if self.peek_kind(i) != Some(TokenKind::Eq) {
            return false;
        }
        i += 1;
        if self.peek_kind(i) != Some(TokenKind::LBrace) {
            return false;
        }
        self.tokens_look_like_type_body(i + 1)
    }

    fn tokens_look_like_type_body(&self, mut i: usize) -> bool {
        while matches!(
            self.peek_kind(i),
            Some(
                TokenKind::DocOuter
                    | TokenKind::DocInner
                    | TokenKind::MetaCommentLine
                    | TokenKind::MetaCommentBlock
            )
        ) {
            i += 1;
        }
        match self.peek_kind(i) {
            Some(TokenKind::RBrace | TokenKind::Plus | TokenKind::Minus) => true,
            Some(TokenKind::Ident) => match self.peek_kind(i + 1) {
                Some(TokenKind::Colon) => !matches!(
                    self.peek_kind(i + 2),
                    Some(
                        TokenKind::String
                            | TokenKind::Int
                            | TokenKind::Float
                            | TokenKind::True
                            | TokenKind::False
                            | TokenKind::LBrace
                            | TokenKind::LBracket
                            | TokenKind::Dot
                    )
                ),
                _ => true,
            },
            _ => false,
        }
    }

    fn parse_type_def(&mut self, exported: bool) -> Item {
        let start = self.current().span;
        let name = self.expect_ident();
        let mut traits = Vec::new();
        if self.eat(TokenKind::Colon) {
            loop {
                let trait_name = self.expect_ident();
                if !traits.contains(&trait_name) {
                    traits.push(trait_name);
                }
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::Eq);
        self.expect(TokenKind::LBrace);
        // Peek first content to decide struct vs sum
        if self.is_sum_body() {
            let mut variants = Vec::new();
            let mut methods = Vec::new();
            let mut docs = self.blank_doc();
            while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
                let pre = self.take_preamble();
                docs = self.join_doc(docs, &pre.inner_lines);
                if self.is(TokenKind::RBrace) || self.is(TokenKind::Eof) {
                    if !self.doc_empty(pre.outer) {
                        self.error_here("documentation comment is not attached to a declaration");
                    }
                    break;
                }
                if self.looks_like_sum_method() {
                    let private = matches!(self.peek_kind(0), Some(TokenKind::Minus));
                    let mut m = self.parse_function_for_type(exported && !private, Some(name));
                    m.docs = pre.outer;
                    m.attrs.extend(pre.attrs);
                    methods.push(m);
                } else {
                    let mut variant = self.parse_variant();
                    variant.docs = pre.outer;
                    variants.push(variant);
                }
                self.eat(TokenKind::Comma);
            }
            self.expect(TokenKind::RBrace);
            if !traits.is_empty() {
                self.diagnostics.error(
                    DiagnosticKind::Syntax,
                    start,
                    "a trait list belongs on a struct: `+Name: Trait = { field: T }`",
                );
            }
            Item::Sum(SumDef {
                exported,
                name,
                variants,
                methods,
                span: start.merge(self.previous_span()),
                docs,
            })
        } else {
            let mut fields = Vec::new();
            let mut methods = Vec::new();
            let mut docs = self.blank_doc();
            while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
                let pre = self.take_preamble();
                docs = self.join_doc(docs, &pre.inner_lines);
                if self.is(TokenKind::RBrace) || self.is(TokenKind::Eof) {
                    if !self.doc_empty(pre.outer) {
                        self.error_here("documentation comment is not attached to a declaration");
                    }
                    break;
                }
                if self.looks_like_method() {
                    // Member visibility matches fields: public by default on an
                    // exported type; `-name` is file-private; leading `+` is
                    // redundant and ignored (same as `+field`).
                    let private = matches!(self.peek_kind(0), Some(TokenKind::Minus));
                    let mut m = self.parse_function_for_type(exported && !private, Some(name));
                    m.docs = pre.outer;
                    m.attrs.extend(pre.attrs);
                    methods.push(m);
                } else {
                    let mut field = self.parse_field_def();
                    field.docs = pre.outer;
                    fields.push(field);
                }
                // Newlines OR commas may separate fields/methods.
                self.eat(TokenKind::Comma);
            }
            self.expect(TokenKind::RBrace);
            Item::Struct(StructDef {
                exported,
                name,
                fields,
                methods,
                traits,
                span: start.merge(self.previous_span()),
                attrs: Vec::new(),
                docs,
            })
        }
    }

    /// Method: `name(` / `+name(` / `-name(` — not `name: Type`.
    /// Soft keywords (`after`, `use`, `get`, `post`, …) count as method names.
    fn looks_like_method(&self) -> bool {
        let mut i = 0;
        if matches!(self.peek_kind(0), Some(TokenKind::Plus | TokenKind::Minus)) {
            i = 1;
        }
        let name_ok = matches!(self.peek_kind(i), Some(k) if Self::is_name_token_kind(k));
        if !name_ok {
            return false;
        }
        matches!(self.peek_kind(i + 1), Some(TokenKind::LParen))
    }

    /// Sum-body method vs payload variant: both are `Name(…)`, but methods continue
    /// with `: Ret`, `{ body }`, or `= expr` after the parameter list (or use `@self`).
    fn looks_like_sum_method(&self) -> bool {
        if !self.looks_like_method() {
            return false;
        }
        let mut i = 0;
        if matches!(self.peek_kind(0), Some(TokenKind::Plus | TokenKind::Minus)) {
            i = 1;
        }
        i += 1; // name
        if self.peek_kind(i) != Some(TokenKind::LParen) {
            return false;
        }
        let mut depth = 0i32;
        loop {
            match self.peek_kind(i) {
                Some(TokenKind::LParen) => depth += 1,
                Some(TokenKind::At) if depth == 1 => return true, // `@self`
                Some(TokenKind::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                Some(TokenKind::Eof) | None => return false,
                _ => {}
            }
            i += 1;
            if i > 512 {
                return false;
            }
        }
        matches!(
            self.peek_kind(i),
            Some(TokenKind::Colon | TokenKind::LBrace | TokenKind::Eq)
        )
    }

    /// Keywords usable as identifiers in name positions (methods, fields, `.member`).
    /// Statement forms (`use path`, `after …`, `get "…"`) still win in item/stmt parsers.
    fn is_name_token_kind(kind: TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Ident
                | TokenKind::State
                | TokenKind::Test
                | TokenKind::Err
                | TokenKind::Use
                | TokenKind::Ensure
                | TokenKind::When
                | TokenKind::Select
                | TokenKind::Each
                | TokenKind::For
                | TokenKind::Par
                | TokenKind::While
                | TokenKind::Loop
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Spawn
                | TokenKind::Scope
                | TokenKind::Await
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Panic
                | TokenKind::Or
                | TokenKind::In
                | TokenKind::Else
                | TokenKind::If
                | TokenKind::Embed
                | TokenKind::After
                | TokenKind::Rewrite
                | TokenKind::Type
                | TokenKind::Code
                | TokenKind::Trait
                | TokenKind::Parallel
                | TokenKind::Chan
                | TokenKind::Shared
                | TokenKind::None
                | TokenKind::True
                | TokenKind::False
        )
    }

    fn soft_ident_text(kind: TokenKind) -> Option<&'static str> {
        Some(match kind {
            TokenKind::Use => "use",
            TokenKind::State => "state",
            TokenKind::Test => "test",
            TokenKind::Err => "err",
            TokenKind::Ensure => "ensure",
            TokenKind::When => "when",
            TokenKind::Select => "select",
            TokenKind::Each => "each",
            TokenKind::For => "for",
            TokenKind::Par => "par",
            TokenKind::While => "while",
            TokenKind::Loop => "loop",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::Spawn => "spawn",
            TokenKind::Scope => "scope",
            TokenKind::Await => "await",
            TokenKind::Return => "return",
            TokenKind::Fail => "fail",
            TokenKind::Panic => "panic",
            TokenKind::Or => "or",
            TokenKind::In => "in",
            TokenKind::Else => "else",
            TokenKind::If => "if",
            TokenKind::Embed => "embed",
            TokenKind::After => "after",
            TokenKind::Rewrite => "rewrite",
            TokenKind::Type => "type",
            TokenKind::Code => "Code",
            TokenKind::Trait => "trait",
            TokenKind::Parallel => "parallel",
            TokenKind::Chan => "chan",
            TokenKind::Shared => "shared",
            TokenKind::None => "none",
            TokenKind::True => "true",
            TokenKind::False => "false",
            _ => return None,
        })
    }

    fn is_sum_body(&self) -> bool {
        // Sum variants: Ident or Ident( without `: type`.
        // Leading doc comments are not part of the decision.
        let (mut i, kind) = self.sig_at(0);
        if !matches!(
            kind,
            Some(TokenKind::Ident | TokenKind::Plus | TokenKind::Minus)
        ) {
            return false;
        }
        if matches!(kind, Some(TokenKind::Plus | TokenKind::Minus)) {
            i += 1;
        }
        if self.peek_kind(i) != Some(TokenKind::Ident) {
            return false;
        }
        match self.peek_kind(i + 1) {
            Some(TokenKind::Colon) => false, // field
            Some(TokenKind::LParen) => true, // variant with payload
            Some(TokenKind::RBrace) | None => true,
            Some(TokenKind::Ident) | Some(TokenKind::Minus) | Some(TokenKind::Plus) => {
                // Next line-ish: if next token after ident is Ident starting new field with colon
                // Heuristic: bare Ident followed by another Ident → could be unit variants
                true
            }
            _ => true,
        }
    }

    fn parse_variant(&mut self) -> VariantDef {
        let start = self.current().span;
        let name = self.expect_ident();
        let mut fields = Vec::new();
        if self.eat(TokenKind::LParen) {
            while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
                let fname = self.expect_ident();
                self.expect(TokenKind::Colon);
                let ty = self.parse_type_expr();
                fields.push((fname, ty));
                self.eat(TokenKind::Comma);
            }
            self.expect(TokenKind::RParen);
        }
        VariantDef {
            name,
            fields,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_field_def(&mut self) -> FieldDef {
        let start = self.current().span;
        let private = self.eat(TokenKind::Minus);
        let _ = self.eat(TokenKind::Plus); // ignore redundant +
        let name = self.expect_ident();
        self.expect(TokenKind::Colon);
        let ty = self.parse_type_expr();
        let default = if self.eat(TokenKind::Eq) {
            Some(self.parse_expr(0))
        } else {
            None
        };
        FieldDef {
            private,
            name,
            ty,
            default,
            span: start.merge(self.previous_span()),
            docs: self.blank_doc(),
        }
    }

    fn parse_function(&mut self, exported: bool) -> Function {
        self.parse_function_for_type(exported, None)
    }

    fn parse_function_for_type(&mut self, exported: bool, owner: Option<SymId>) -> Function {
        let start = self.current().span;
        let _ = self.eat(TokenKind::Plus);
        let _ = self.eat(TokenKind::Minus);
        let name = self.expect_ident();
        self.expect(TokenKind::LParen);
        let mut params = Vec::new();
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            params.push(self.parse_param(owner));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        let ret = if self.eat(TokenKind::Colon) {
            Some(self.parse_type_expr())
        } else {
            None
        };
        // Expression bodies use `=>` only (`=` is for bindings / defaults).
        // Auto-inline is a later leaf/`#inline`/host heuristic — not every `=>`.
        let body = if self.eat(TokenKind::FatArrow) {
            FnBody::Expr(self.parse_expr(0))
        } else if self.is(TokenKind::Eq) {
            self.error_here(
                "expression function bodies use `=>`, not `=`; write `f(...) => expr` or `f(...) { … }`",
            );
            self.advance();
            FnBody::Expr(self.parse_expr(0))
        } else {
            FnBody::Block(self.parse_block())
        };
        Function {
            exported,
            name,
            owner,
            params,
            ret,
            body,
            span: start.merge(self.previous_span()),
            attrs: Vec::new(),
            docs: self.blank_doc(),
        }
    }

    /// `|x: i64| => x * 2` / `|@acc, x| { … }` / `|| => 0`
    fn parse_lambda(&mut self) -> Expr {
        let start = self.expect(TokenKind::Pipe).span;
        let mut params = Vec::new();
        if !self.is(TokenKind::Pipe) {
            loop {
                params.push(self.parse_param(None));
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::Pipe);
        let ret = if self.eat(TokenKind::Colon) {
            Some(self.parse_type_expr())
        } else {
            None
        };
        let body = if self.eat(TokenKind::FatArrow) {
            FnBody::Expr(self.parse_expr(0))
        } else if self.is(TokenKind::Eq) {
            self.error_here("lambda bodies use `=>` or `{ … }`, not `=`");
            self.advance();
            FnBody::Expr(self.parse_expr(0))
        } else {
            FnBody::Block(self.parse_block())
        };
        Expr::Lambda {
            params,
            ret,
            body: Box::new(body),
            captures: Vec::new(),
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_param(&mut self, owner: Option<SymId>) -> Param {
        let start = self.current().span;
        let comptime = self.eat(TokenKind::Hash);
        let mutable = self.eat(TokenKind::At);
        let name = self.expect_ident();
        // `self` / `@self` inside a type method — type inferred as the owner.
        if let Some(owner) = owner
            && self.is_contextual(name, ContextualKeyword::SelfValue)
            && !self.is(TokenKind::Colon)
        {
            let ty = TypeExpr::Named(owner, start);
            return Param {
                mutable,
                comptime,
                name,
                ty,
                span: start.merge(self.previous_span()),
            };
        }
        // Untyped params default to `_` (inferred / monomorphized).
        let ty = if self.eat(TokenKind::Colon) {
            self.parse_type_expr()
        } else {
            TypeExpr::Named(self.interner.intern("_"), self.previous_span())
        };
        Param {
            mutable,
            comptime,
            name,
            ty,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_binding(&mut self, exported: bool, mutable: bool) -> Binding {
        let start = self.current().span;
        let name = self.expect_ident();
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.parse_type_expr())
        } else {
            None
        };
        self.expect(TokenKind::Eq);
        let value = self.parse_expr(0);
        Binding {
            exported,
            mutable,
            comptime: false,
            name,
            ty,
            value,
            span: start.merge(self.previous_span()),
            attrs: Vec::new(),
            docs: self.blank_doc(),
        }
    }

    fn parse_type_expr(&mut self) -> TypeExpr {
        let start = self.current().span;
        if self.is(TokenKind::Type) {
            let t = self.advance();
            return TypeExpr::Type(t.span);
        }
        if self.is(TokenKind::Code) {
            self.advance();
            self.expect(TokenKind::LBracket);
            let inner = self.parse_type_expr();
            self.expect(TokenKind::RBracket);
            return TypeExpr::Code(Box::new(inner), start.merge(self.previous_span()));
        }
        // `chan[T]` handled elsewhere; named / path types
        if !self.is(TokenKind::Ident) {
            self.error_here("expected type");
            return TypeExpr::Named(self.interner.intern("_"), start);
        }
        let mut path = vec![self.expect_ident()];
        while self.eat(TokenKind::Dot) {
            path.push(self.expect_ident());
        }
        // `Name[Arg, …]` — type application, resolved by name after parsing
        // (the builtin `Fn[Params…, Ret]` is one such constructor).
        if path.len() == 1 && self.eat(TokenKind::LBracket) {
            let mut args = Vec::new();
            if !self.is(TokenKind::RBracket) {
                loop {
                    args.push(self.parse_type_expr());
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(TokenKind::RBracket);
            let mut ty = TypeExpr::Apply {
                name: path[0],
                args,
                span: start.merge(self.previous_span()),
            };
            loop {
                if self.eat(TokenKind::Question) {
                    let span = start.merge(self.previous_span());
                    ty = TypeExpr::Optional(Box::new(ty), span);
                } else if self.eat(TokenKind::Bang) {
                    let span = start.merge(self.previous_span());
                    ty = TypeExpr::Error(Box::new(ty), span);
                } else {
                    break;
                }
            }
            return ty;
        }
        let mut ty = if path.len() == 1 {
            TypeExpr::Named(path[0], start.merge(self.previous_span()))
        } else {
            TypeExpr::Path(path, start.merge(self.previous_span()))
        };
        loop {
            if self.eat(TokenKind::Question) {
                let span = start.merge(self.previous_span());
                ty = TypeExpr::Optional(Box::new(ty), span);
            } else if self.eat(TokenKind::Bang) {
                let span = start.merge(self.previous_span());
                ty = TypeExpr::Error(Box::new(ty), span);
            } else {
                break;
            }
        }
        ty
    }

    fn parse_block(&mut self) -> Block {
        let saved = std::mem::replace(&mut self.in_block_head, false);
        let block = self.parse_block_inner();
        self.in_block_head = saved;
        block
    }

    fn parse_block_inner(&mut self) -> Block {
        let start = self.expect(TokenKind::LBrace).span;
        let mut stmts = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let pos = self.pos;
            let stmt = self.parse_stmt();
            // `parse_stmt` yields a phantom `none` when it is already at `}`.
            // A written `none` consumes a token and is the block's value.
            if self.pos == pos
                && (self.is(TokenKind::RBrace) || self.is(TokenKind::Eof))
                && matches!(stmt, Stmt::Expr(Expr::None(_)))
            {
                break;
            }
            stmts.push(stmt);
        }
        let end = self.expect(TokenKind::RBrace).span;
        Block {
            stmts,
            span: start.merge(end),
        }
    }

    fn parse_stmt(&mut self) -> Stmt {
        let pre = self.take_preamble();
        if !pre.inner_lines.is_empty() {
            self.error_here(
                "inner doc comment (`//!` / `/*!`) documents the enclosing module or type",
            );
        }
        let attrs = pre.attrs;
        let docs = pre.outer;
        if matches!(self.current().kind, TokenKind::RBrace | TokenKind::Eof) {
            if !self.doc_empty(docs) || !pre.inner_lines.is_empty() {
                self.error_here("documentation comment is not attached to a declaration");
            }
            return Stmt::Expr(Expr::None(self.current().span));
        }
        let mut stmt = match self.current().kind {
            TokenKind::Return => {
                let start = self.advance().span;
                let value = if self.is(TokenKind::RBrace)
                    || self.is(TokenKind::Eof)
                    || self.is(TokenKind::If)
                    || self.newline_before_current(start)
                {
                    None
                } else {
                    Some(self.parse_expr(0))
                };
                Stmt::Return(value, start.merge(self.previous_span()))
            }
            TokenKind::Break => {
                let start = self.advance().span;
                Stmt::Break(start)
            }
            TokenKind::Continue => {
                let start = self.advance().span;
                Stmt::Continue(start)
            }
            TokenKind::Fail => {
                let start = self.advance().span;
                let expr = self.parse_expr(0);
                Stmt::Fail(expr, start.merge(self.previous_span()))
            }
            TokenKind::Panic => {
                let start = self.advance().span;
                let message = if self.looks_like_expr_start() {
                    Some(self.parse_expr(0))
                } else {
                    None
                };
                Stmt::Panic(message, start.merge(self.previous_span()))
            }
            TokenKind::Ensure => self.parse_ensure(),
            TokenKind::Each | TokenKind::For | TokenKind::Par => {
                let mut stmt = self.parse_each();
                if let Stmt::Each {
                    attrs: ref mut a, ..
                } = stmt
                {
                    a.extend(attrs);
                }
                stmt
            }
            TokenKind::Loop => {
                let start = self.advance().span;
                let body = self.parse_block();
                Stmt::Loop {
                    body,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::While => {
                let start = self.advance().span;
                let cond = self.parse_head_expr();
                let body = self.parse_block();
                Stmt::While {
                    cond,
                    body,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::When => self.parse_when(),
            TokenKind::Select => self.parse_select(),
            // `scope { … }` only — bare `scope = …` / `scope.foo` are identifiers.
            TokenKind::Scope if self.peek_kind(1) == Some(TokenKind::LBrace) => self.parse_scope(),
            TokenKind::Hash => {
                let start = self.expect(TokenKind::Hash).span;
                if self.is(TokenKind::If) {
                    self.advance();
                    let cond = self.parse_head_expr();
                    let then_block = self.parse_block();
                    let else_block = if self.eat(TokenKind::Else) {
                        Some(self.parse_block())
                    } else {
                        None
                    };
                    Stmt::Comptime(ComptimeStmt::If {
                        cond,
                        then_block,
                        else_block,
                        span: start.merge(self.previous_span()),
                    })
                } else if self.is(TokenKind::Each) {
                    self.advance();
                    let name = self.expect_ident();
                    self.expect(TokenKind::In);
                    let iter = self.parse_head_expr();
                    let body = self.parse_block();
                    Stmt::Comptime(ComptimeStmt::Each {
                        name,
                        iter,
                        body,
                        span: start.merge(self.previous_span()),
                    })
                } else if self.is(TokenKind::LBrace) {
                    let body = self.parse_block();
                    Stmt::Comptime(ComptimeStmt::Block(body.clone(), start.merge(body.span)))
                } else if self.is(TokenKind::Ident) && self.looks_like_binding_after_hash() {
                    let mut b = self.parse_binding(false, false);
                    b.comptime = true;
                    b.attrs = attrs;
                    b.docs = docs;
                    Stmt::Binding(b)
                } else {
                    // `#Name + 1` etc. — resume Pratt so infix ops bind.
                    let lhs = self.parse_hash_expr_after_hash(start);
                    Stmt::Expr(self.parse_expr_continue(lhs, 0))
                }
            }
            TokenKind::LBrace if self.looks_like_destructure() => self.parse_destructure(),
            _ if self.looks_like_binding() => {
                let mut binding = self.parse_binding(false, false);
                binding.docs = docs;
                Stmt::Binding(binding)
            }
            _ => {
                let first = self.parse_expr(0);
                if self.is(TokenKind::Comma) {
                    // `a, b = …` / `a[i], a[j] = …`
                    let start = first.span();
                    let mut targets = vec![first];
                    while self.eat(TokenKind::Comma) {
                        targets.push(self.parse_expr(18));
                    }
                    self.expect(TokenKind::Eq);
                    let mut values = Vec::new();
                    loop {
                        values.push(self.parse_expr(2));
                        if !self.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    Stmt::MultiAssign {
                        targets,
                        values,
                        span: start.merge(self.previous_span()),
                    }
                } else {
                    Stmt::Expr(first)
                }
            }
        };
        if !self.doc_empty(docs) && !matches!(stmt, Stmt::Binding(_)) {
            self.error_here("documentation comment is not attached to a declaration");
        }
        // Postfix guards: `break if cond`, `return expr if cond`, `a = b if cond`.
        // Never steal a statement-level `if cond { … }` (detect brace after the condition).
        if self.is(TokenKind::If) && !self.looks_like_block_if() {
            match &stmt {
                Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::Return(_, _)
                | Stmt::Fail(_, _)
                | Stmt::Panic(_, _)
                | Stmt::MultiAssign { .. }
                | Stmt::Binding(_)
                | Stmt::Expr(Expr::Assign { .. }) => {
                    self.advance(); // if
                    let cond = self.parse_expr(0);
                    let span = match &stmt {
                        Stmt::Break(s) | Stmt::Continue(s) => *s,
                        Stmt::Return(_, s) | Stmt::Fail(_, s) | Stmt::Panic(_, s) => *s,
                        Stmt::Expr(e) => e.span(),
                        Stmt::Binding(b) => b.span,
                        Stmt::MultiAssign { span, .. } => *span,
                        _ => self.previous_span(),
                    }
                    .merge(self.previous_span());
                    stmt = Stmt::Guarded {
                        stmt: Box::new(stmt),
                        cond,
                        span,
                    };
                }
                _ => {}
            }
        }
        stmt
    }

    /// True when `if` introduces `if <cond> { … }` rather than a postfix guard.
    fn looks_like_block_if(&self) -> bool {
        if !self.is(TokenKind::If) {
            return false;
        }
        let mut i = 1usize;
        let mut depth = 0i32;
        while let Some(k) = self.peek_kind(i) {
            match k {
                TokenKind::LBrace if depth == 0 => return true,
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth -= 1,
                TokenKind::RBrace | TokenKind::Eof | TokenKind::Semicolon => return false,
                _ => {}
            }
            i += 1;
            if i > 96 {
                break;
            }
        }
        false
    }

    fn parse_scope(&mut self) -> Stmt {
        let start = self.advance().span;
        let body = self.parse_block();
        Stmt::Scope {
            body,
            span: start.merge(self.previous_span()),
        }
    }

    fn looks_like_destructure(&self) -> bool {
        // `{ ident ... } =`
        if !self.is(TokenKind::LBrace) {
            return false;
        }
        if self.peek_kind(1) != Some(TokenKind::Ident) {
            return false;
        }
        // scan lightly for closing brace then eq
        let mut i = 1;
        while let Some(k) = self.peek_kind(i) {
            if k == TokenKind::RBrace {
                return self.peek_kind(i + 1) == Some(TokenKind::Eq);
            }
            if k == TokenKind::LBrace {
                return false;
            }
            i += 1;
            if i > 32 {
                break;
            }
        }
        false
    }

    fn parse_destructure(&mut self) -> Stmt {
        let start = self.advance().span; // {
        let mut fields = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let src = self.expect_ident();
            let local = if self.eat(TokenKind::Colon) {
                self.expect_ident()
            } else {
                src
            };
            fields.push((src, local));
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace);
        self.expect(TokenKind::Eq);
        let value = self.parse_expr(0);
        Stmt::Destructure {
            fields,
            value,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_ensure(&mut self) -> Stmt {
        let start = self.advance().span;
        let cond = self.parse_head_expr();
        self.expect(TokenKind::Else);
        let else_block = self.parse_block();
        Stmt::Ensure {
            cond,
            else_block,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_each(&mut self) -> Stmt {
        let start = self.advance().span;
        // `par …` is always parallel; `for parallel` / `for parallel` kept as deprecated alias.
        let parallel = matches!(self.tokens[self.pos.saturating_sub(1)].kind, TokenKind::Par)
            || self.eat(TokenKind::Parallel);
        let mut bindings = Vec::new();
        let mutable_prefix = self.eat(TokenKind::At);

        // each [parallel] @(pos: Pos), (vel: Vel) in world { }
        // for/par [parallel] @scores { }
        // for 0..100 { }
        if mutable_prefix && self.is(TokenKind::LParen) {
            bindings.push(self.parse_each_binding(true));
            while self.eat(TokenKind::Comma) {
                let m = self.eat(TokenKind::At);
                bindings.push(self.parse_each_binding(m));
            }
            self.expect(TokenKind::In);
            let iter = self.parse_head_expr();
            let reduction = self.parse_reduction_op();
            let body = self.parse_block();
            return Stmt::Each {
                mutable: true,
                parallel,
                reduction,
                iter,
                bindings,
                body,
                span: start.merge(self.previous_span()),
                attrs: Vec::new(),
            };
        }

        // `for x in xs` / `for x: i64 in 0..N` / `par y in 0..H (+) { … }`
        if self.is(TokenKind::Ident)
            && matches!(self.peek_kind(1), Some(TokenKind::In | TokenKind::Colon))
        {
            let name = self.expect_ident();
            let ty = if self.eat(TokenKind::Colon) {
                Some(self.expect_ident())
            } else {
                None
            };
            bindings.push(EachBinding {
                mutable: mutable_prefix,
                name,
                ty,
            });
            self.expect(TokenKind::In);
            let iter = self.parse_head_expr();
            let reduction = self.parse_reduction_op();
            let body = self.parse_block();
            return Stmt::Each {
                mutable: mutable_prefix,
                parallel,
                reduction,
                iter,
                bindings,
                body,
                span: start.merge(self.previous_span()),
                attrs: Vec::new(),
            };
        }

        let iter = self.parse_head_expr();
        let reduction = self.parse_reduction_op();
        let body = self.parse_block();
        Stmt::Each {
            mutable: mutable_prefix,
            parallel,
            reduction,
            iter,
            bindings,
            body,
            span: start.merge(self.previous_span()),
            attrs: Vec::new(),
        }
    }

    /// Optional `(+)` (or other binop) reduction marker after the iterator expression.
    fn parse_reduction_op(&mut self) -> Option<BinOp> {
        if self.is(TokenKind::LParen) && self.peek_kind(1) == Some(TokenKind::Plus) {
            self.advance(); // (
            self.advance(); // +
            self.expect(TokenKind::RParen);
            return Some(BinOp::Add);
        }
        None
    }

    fn parse_each_binding(&mut self, mutable: bool) -> EachBinding {
        self.expect(TokenKind::LParen);
        let name = self.expect_ident();
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.expect_ident())
        } else {
            None
        };
        self.expect(TokenKind::RParen);
        EachBinding { mutable, name, ty }
    }

    fn parse_when(&mut self) -> Stmt {
        let start = self.advance().span;
        // Multiplex form (legacy alias): `when { arm => … after 16.ms => … }`
        // Prefer `select { … }` (A3). Multi-method: `when (e1, e2, …) { … }`.
        // Classic: `when subject { … }`
        let subjects = if self.is(TokenKind::LBrace) {
            Vec::new()
        } else if self.is(TokenKind::LParen) && self.looks_like_when_tuple_subject() {
            self.parse_when_tuple_subjects()
        } else {
            vec![self.parse_head_expr()]
        };
        self.parse_when_arms(start, subjects)
    }

    /// A3: `select { recv => … after dur => … }` — subject-less channel mux.
    fn parse_select(&mut self) -> Stmt {
        let start = self.advance().span;
        if !self.is(TokenKind::LBrace) {
            self.error_here("expected `{` after `select` (channel multiplex has no subject)");
        }
        self.parse_when_arms(start, Vec::new())
    }

    fn parse_when_arms(&mut self, start: Span, subjects: Vec<Expr>) -> Stmt {
        self.expect(TokenKind::LBrace);
        let mut arms = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let arm_start = self.current().span;
            let pattern = if self.eat(TokenKind::Else) {
                WhenPattern::Else
            } else if self.eat(TokenKind::After) {
                WhenPattern::After(self.parse_expr(0))
            } else if self.looks_like_when_tuple_pattern() {
                self.parse_when_tuple_pattern(arm_start)
            } else if self.looks_like_variant_bind_pattern() {
                self.parse_variant_bind_pattern(arm_start)
            } else {
                WhenPattern::Expr(self.parse_expr(0))
            };
            self.expect(TokenKind::FatArrow);
            let body = self.parse_expr(0);
            arms.push(WhenArm {
                pattern,
                body,
                span: arm_start.merge(self.previous_span()),
            });
        }
        self.expect(TokenKind::RBrace);
        Stmt::When {
            subjects,
            arms,
            span: start.merge(self.previous_span()),
        }
    }

    /// `when (` with a comma before the matching `)` → multi-method subject tuple.
    fn looks_like_when_tuple_subject(&self) -> bool {
        if self.peek_kind(0) != Some(TokenKind::LParen) {
            return false;
        }
        let mut depth = 0i32;
        let mut i = 0;
        loop {
            match self.peek_kind(i) {
                Some(TokenKind::LParen) => depth += 1,
                Some(TokenKind::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                Some(TokenKind::Comma) if depth == 1 => return true,
                Some(TokenKind::LBrace) if depth == 0 => return false,
                Some(TokenKind::Eof) | None => return false,
                _ => {}
            }
            i += 1;
            if i > 512 {
                return false;
            }
        }
    }

    fn parse_when_tuple_subjects(&mut self) -> Vec<Expr> {
        self.expect(TokenKind::LParen);
        let mut subjects = Vec::new();
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            subjects.push(self.parse_expr(0));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        subjects
    }

    /// `(P1, P2, …) =>` arm for multi-method `when`.
    fn looks_like_when_tuple_pattern(&self) -> bool {
        if self.peek_kind(0) != Some(TokenKind::LParen) {
            return false;
        }
        let mut depth = 0i32;
        let mut i = 0;
        let mut saw_comma = false;
        loop {
            match self.peek_kind(i) {
                Some(TokenKind::LParen) => depth += 1,
                Some(TokenKind::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return saw_comma && self.peek_kind(i + 1) == Some(TokenKind::FatArrow);
                    }
                }
                Some(TokenKind::Comma) if depth == 1 => saw_comma = true,
                Some(TokenKind::Eof) | None => return false,
                _ => {}
            }
            i += 1;
            if i > 512 {
                return false;
            }
        }
    }

    fn parse_when_tuple_pattern(&mut self, start: Span) -> WhenPattern {
        self.expect(TokenKind::LParen);
        let mut elems = Vec::new();
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            let elem_start = self.current().span;
            let elem = if self.looks_like_variant_bind_pattern() {
                self.parse_variant_bind_pattern(elem_start)
            } else {
                WhenPattern::Expr(self.parse_expr(0))
            };
            elems.push(elem);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::RParen);
        WhenPattern::Tuple {
            elems,
            span: start.merge(self.previous_span()),
        }
    }

    /// `Sum.Variant { field, field: rename }` ahead of `=>` (when-arm payload destructure).
    fn looks_like_variant_bind_pattern(&self) -> bool {
        if self.peek_kind(0) != Some(TokenKind::Ident) {
            return false;
        }
        if self.peek_kind(1) != Some(TokenKind::Dot) {
            return false;
        }
        if self.peek_kind(2) != Some(TokenKind::Ident) {
            return false;
        }
        let Some(tok) = self.tokens.get(self.pos + 2).copied() else {
            return false;
        };
        let Some(sym) = tok.sym else {
            return false;
        };
        let vname = self.interner.resolve(sym);
        if !vname.starts_with(|c: char| c.is_ascii_uppercase()) {
            return false;
        }
        if self.peek_kind(3) != Some(TokenKind::LBrace) {
            return false;
        }
        // Scan to matching `}` then require `=>` so we do not steal `ecs.World{…}` exprs.
        let mut depth = 0i32;
        let mut i = 3;
        loop {
            match self.peek_kind(i) {
                Some(TokenKind::LBrace) => depth += 1,
                Some(TokenKind::RBrace) => {
                    depth -= 1;
                    if depth == 0 {
                        return self.peek_kind(i + 1) == Some(TokenKind::FatArrow);
                    }
                }
                Some(TokenKind::Eof) | None => return false,
                _ => {}
            }
            i += 1;
            if i > 256 {
                return false;
            }
        }
    }

    fn parse_variant_bind_pattern(&mut self, start: Span) -> WhenPattern {
        let sum = self.expect_ident();
        self.expect(TokenKind::Dot);
        let variant = self.expect_ident();
        self.expect(TokenKind::LBrace);
        let mut fields = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let payload = self.expect_ident();
            let local = if self.eat(TokenKind::Colon) {
                self.expect_ident()
            } else {
                payload
            };
            fields.push((payload, local));
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace);
        WhenPattern::VariantBind {
            sum,
            variant,
            fields,
            span: start.merge(self.previous_span()),
        }
    }

    // --- Pratt expression parser ---

    fn parse_expr(&mut self, min_bp: u8) -> Expr {
        let lhs = self.parse_prefix();
        self.parse_expr_continue(lhs, min_bp)
    }

    fn parse_expr_continue(&mut self, mut lhs: Expr, min_bp: u8) -> Expr {
        loop {
            // postfix
            if self.is(TokenKind::LParen) {
                // Do not treat `(+)` / `(*), …` reduction markers as calls.
                if self.peek_kind(1).is_some_and(|k| {
                    matches!(
                        k,
                        TokenKind::Plus | TokenKind::Star | TokenKind::Minus | TokenKind::Amp
                    )
                }) && self.peek_kind(2) == Some(TokenKind::RParen)
                {
                    break;
                }
                // Newline before `(` starts a new statement / when-arm
                // (`0\n(1, _) => …`), not a call `0(1, _)`.
                if self.newline_before_current(lhs.span()) {
                    break;
                }
                // Multi-method when-arm `(P1, P2) =>` must not be eaten as a call.
                if self.looks_like_when_tuple_pattern() {
                    break;
                }
                lhs = self.finish_call(lhs);
                continue;
            }
            if self.is(TokenKind::Dot) {
                // .{ inferred struct OR field / method OR .(comptime field)
                if self.peek_kind(1) == Some(TokenKind::LBrace) {
                    // A line starting with `.{` is a new (tail) literal, not `prev.{…}`.
                    if self.newline_before_current(lhs.span()) {
                        break;
                    }
                    self.advance(); // .
                    lhs = self.parse_inferred_struct_after_dot(lhs.span());
                    continue;
                }
                if self.peek_kind(1) == Some(TokenKind::LParen) {
                    self.advance(); // .
                    self.expect(TokenKind::LParen);
                    let field = self.parse_expr(0);
                    self.expect(TokenKind::RParen);
                    let span = lhs.span().merge(self.previous_span());
                    lhs = Expr::ComptimeField {
                        base: Box::new(lhs),
                        field: Box::new(field),
                        span,
                    };
                    continue;
                }
                lhs = self.finish_field_or_method(lhs);
                continue;
            }
            if self.is(TokenKind::LBracket) {
                lhs = self.finish_index(lhs);
                continue;
            }
            if self.is(TokenKind::DotDot) {
                let (l_bp, r_bp) = (8u8, 9u8);
                if l_bp < min_bp {
                    break;
                }
                self.advance();
                let end = self.parse_expr(r_bp);
                let span = lhs.span().merge(end.span());
                lhs = Expr::Range {
                    start: Box::new(lhs),
                    end: Box::new(end),
                    span,
                };
                continue;
            }

            // Ternary: `cond ? then : else`
            if self.is(TokenKind::Question) {
                let (l_bp, _) = (2u8, 3u8);
                if l_bp < min_bp {
                    break;
                }
                self.advance();
                let then_expr = self.parse_expr(0);
                self.expect(TokenKind::Colon);
                let else_expr = self.parse_expr(2);
                let span = lhs.span().merge(else_expr.span());
                lhs = Expr::Ternary {
                    cond: Box::new(lhs),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                    span,
                };
                continue;
            }

            // `or` / `or err`
            if self.is(TokenKind::Or) {
                let (l_bp, _) = (1u8, 2u8);
                if l_bp < min_bp {
                    break;
                }
                self.advance();
                if self.is(TokenKind::Err) {
                    self.advance();
                    let body = self.parse_block();
                    let span = lhs.span().merge(body.span);
                    lhs = Expr::OrErr {
                        left: Box::new(lhs),
                        body,
                        span,
                    };
                } else {
                    let right = self.parse_expr(2);
                    let span = lhs.span().merge(right.span());
                    lhs = Expr::Or {
                        left: Box::new(lhs),
                        right: Box::new(right),
                        span,
                    };
                }
                continue;
            }

            // Top-level `+export` / private `-field:` must not continue an expression.
            if self.looks_like_decl_prefix() {
                break;
            }

            let Some(op) = self.binop_of(self.current().kind) else {
                // assignment
                if let Some((op, l_bp, r_bp)) = self.assign_of(self.current().kind) {
                    if l_bp < min_bp {
                        break;
                    }
                    self.advance();
                    let value = self.parse_expr(r_bp);
                    let span = lhs.span().merge(value.span());
                    lhs = Expr::Assign {
                        op,
                        target: Box::new(lhs),
                        value: Box::new(value),
                        span,
                    };
                    continue;
                }
                break;
            };
            // Newline before `+`/`-` starts a new statement (unary ± / next stmt),
            // not an infix continue — so `@self.touch(at)\n-1` is two statements.
            if matches!(op, BinOp::Add | BinOp::Sub) && self.newline_before_current(lhs.span()) {
                break;
            }
            let (l_bp, r_bp) = self.infix_binding_power(op);
            if l_bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_expr(r_bp);
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
                span,
            };
        }
        lhs
    }

    /// True when the current token starts a new line after the expression
    /// that ends at `before`.
    fn newline_before_current(&self, before: Span) -> bool {
        if self.pos == 0 || self.pos >= self.tokens.len() {
            return false;
        }
        let _ = before;
        self.line_starts.get(self.pos).copied().unwrap_or(false)
    }

    fn parse_prefix(&mut self) -> Expr {
        let tok = self.current();
        match tok.kind {
            TokenKind::Int => {
                let t = self.advance();
                let text = self.interner.resolve(t.sym.unwrap());
                let n = parse_int_literal(text);
                Expr::Int(n, t.span)
            }
            TokenKind::Float => {
                let t = self.advance();
                let text = self.interner.resolve(t.sym.unwrap());
                let cleaned: String = text.chars().filter(|c| *c != '_').collect();
                let n: f64 = cleaned.parse().unwrap_or(0.0);
                Expr::Float(n, t.span)
            }
            TokenKind::String => {
                let t = self.advance();
                Expr::String(t.sym.unwrap(), t.span)
            }
            TokenKind::True => {
                let t = self.advance();
                Expr::Bool(true, t.span)
            }
            TokenKind::False => {
                let t = self.advance();
                Expr::Bool(false, t.span)
            }
            TokenKind::None => {
                let t = self.advance();
                Expr::None(t.span)
            }
            TokenKind::Embed => {
                let start = self.advance().span;
                let path = self.expect_string_sym();
                Expr::Embed {
                    path,
                    span: start.merge(self.previous_span()),
                }
            }
            // Soft keywords as values/bindings (`scope`, `after`, `use`, `type`, …).
            // Dedicated forms (`await`/`spawn`/`chan[T]`/`shared x`/`embed`) stay below.
            k if Self::is_expr_ident_kind(k) => {
                let t = self.advance();
                let sym = t.sym.unwrap_or_else(|| {
                    let text = Self::soft_ident_text(t.kind).unwrap_or("anon");
                    self.interner.intern(text)
                });
                if t.kind == TokenKind::Err {
                    Expr::ErrIdent(t.span)
                } else if self.is_contextual(sym, ContextualKeyword::It) {
                    Expr::It(t.span)
                } else if self.is_contextual(sym, ContextualKeyword::Index) {
                    Expr::IndexIdent(t.span)
                } else if self.is(TokenKind::LBrace)
                    && !(self.in_block_head
                        && !self.interner.resolve(sym).starts_with(char::is_uppercase))
                    && self.looks_like_struct_lit_body()
                {
                    // TypeName{ fields } — not `name { stmt... }` blocks after ranges, and
                    // not `if a > b { a }`, where the brace opens the body.
                    self.parse_struct_lit(Some(sym), t.span)
                } else if self.is(TokenKind::Dot) && self.peek_kind(1) == Some(TokenKind::LBrace) {
                    // TypeName.{ field punning… }
                    self.advance(); // .
                    let fields_start = self.current().span;
                    let fields = self.parse_brace_fields();
                    Expr::StructLit {
                        name: Some(sym),
                        fields,
                        span: t.span.merge(fields_start).merge(self.previous_span()),
                    }
                } else if self.is(TokenKind::LBrace)
                    && !self.in_block_head
                    && !self.newline_before_current(t.span)
                {
                    let name = self.interner.resolve(sym).to_owned();
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Syntax,
                        self.current().span,
                        super_core::err::E0001,
                        format!("`{name} {{` is not a struct literal or a block"),
                        format!("to invoke a block macro write `#{name} {{ … }}`"),
                    );
                    self.skip_braced();
                    Expr::Ident(sym, t.span)
                } else {
                    // `ecs.World{}` is handled via postfix field/method parsing
                    Expr::Ident(sym, t.span)
                }
            }
            TokenKind::Minus => {
                let start = self.advance().span;
                let expr = self.parse_expr(21);
                Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Bang => {
                let start = self.advance().span;
                let expr = self.parse_expr(21);
                Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Tilde => {
                let start = self.advance().span;
                let expr = self.parse_expr(21);
                Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(expr),
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::At => {
                let start = self.advance().span;
                let expr = self.parse_expr(21);
                let span = start.merge(expr.span());
                Expr::Unary {
                    op: UnOp::MutRef,
                    expr: Box::new(expr),
                    span,
                }
            }
            // `panic` / `panic msg` in expression position diverges.
            TokenKind::Panic => {
                let start = self.advance().span;
                let message = if self.looks_like_expr_start() && !self.starts_line() {
                    Some(Box::new(self.parse_expr(2)))
                } else {
                    None
                };
                Expr::Panic {
                    message,
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::Await => {
                let start = self.advance().span;
                let expr = self.parse_expr(12);
                let span = start.merge(expr.span());
                Expr::Await(Box::new(expr), span)
            }
            TokenKind::Spawn => {
                let start = self.advance().span;
                let body = if self.is(TokenKind::LBrace) {
                    Expr::Block(self.parse_block())
                } else {
                    self.parse_expr(12)
                };
                let span = start.merge(body.span());
                Expr::Spawn {
                    body: Box::new(body),
                    span,
                }
            }
            TokenKind::Chan => {
                if self.peek_kind(1) == Some(TokenKind::LBracket) {
                    self.parse_chan_expr()
                } else {
                    // Bare `chan` as an identifier (soft).
                    let t = self.advance();
                    let sym = t.sym.unwrap_or_else(|| self.interner.intern("chan"));
                    Expr::Ident(sym, t.span)
                }
            }
            TokenKind::Shared => {
                // `shared expr` vs bare/bound `shared` as an identifier.
                if matches!(
                    self.peek_kind(1),
                    Some(
                        TokenKind::Eq
                            | TokenKind::Colon
                            | TokenKind::Dot
                            | TokenKind::Comma
                            | TokenKind::RParen
                            | TokenKind::RBrace
                            | TokenKind::RBracket
                            | TokenKind::PlusEq
                            | TokenKind::MinusEq
                            | TokenKind::StarEq
                            | TokenKind::SlashEq
                    ) | None
                ) {
                    let t = self.advance();
                    let sym = t.sym.unwrap_or_else(|| self.interner.intern("shared"));
                    Expr::Ident(sym, t.span)
                } else {
                    let start = self.advance().span;
                    let value = self.parse_expr(12);
                    let span = start.merge(value.span());
                    Expr::Shared {
                        value: Box::new(value),
                        span,
                    }
                }
            }
            TokenKind::Pipe => self.parse_lambda(),
            TokenKind::Hash => {
                let start = self.advance().span;
                self.parse_hash_expr_after_hash(start)
            }
            TokenKind::QuoteStart => {
                let start = self.advance().span;
                // `'{` already consumed; parse expr then expect `}`
                let body = self.parse_expr(0);
                self.expect(TokenKind::RBrace);
                Expr::Quote {
                    body: Box::new(body),
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::SpliceStart => {
                let start = self.advance().span;
                let code = self.parse_expr(0);
                self.expect(TokenKind::RBrace);
                Expr::Splice {
                    code: Box::new(code),
                    span: start.merge(self.previous_span()),
                }
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr(0);
                self.expect(TokenKind::RParen);
                expr
            }
            TokenKind::LBracket => self.parse_array(),
            TokenKind::LBrace => self.parse_brace_expr(),
            TokenKind::Dot if self.peek_kind(1) == Some(TokenKind::LBrace) => {
                let start = self.advance().span;
                self.parse_inferred_struct_after_dot(start)
            }
            TokenKind::If => self.parse_if_expr(),
            _ => {
                let span = tok.span;
                self.error_here("expected expression");
                self.advance();
                Expr::None(span)
            }
        }
    }

    /// Expression before a body block (`if c {`, `for x in xs {`, `#if c {`).
    fn parse_head_expr(&mut self) -> Expr {
        let saved = std::mem::replace(&mut self.in_block_head, true);
        let expr = self.parse_expr(0);
        self.in_block_head = saved;
        expr
    }

    fn parse_if_expr(&mut self) -> Expr {
        let start = self.advance().span;
        let cond = self.parse_head_expr();
        let then_block = self.parse_block();
        let else_block = if self.eat(TokenKind::Else) {
            if self.is(TokenKind::If) {
                // `else if c { … }` ≡ `else { if c { … } }`
                let nested = self.parse_if_expr();
                let span = nested.span();
                Some(Block {
                    stmts: vec![Stmt::Expr(nested)],
                    span,
                })
            } else {
                Some(self.parse_block())
            }
        } else {
            None
        };
        Expr::If {
            cond: Box::new(cond),
            then_block,
            else_block,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_array(&mut self) -> Expr {
        let start = self.advance().span;
        if self.is(TokenKind::RBracket) {
            let end = self.advance().span;
            return Expr::Array(vec![], start.merge(end));
        }
        let first = self.parse_expr(0);
        // `[val; N]` stack-array repeat
        if self.eat(TokenKind::Semicolon) {
            let len = self.parse_expr(0);
            let end = self.expect(TokenKind::RBracket).span;
            return Expr::ArrayRepeat {
                value: Box::new(first),
                len: Box::new(len),
                span: start.merge(end),
            };
        }
        let mut elems = vec![first];
        while self.eat(TokenKind::Comma) {
            if self.is(TokenKind::RBracket) {
                break;
            }
            elems.push(self.parse_expr(0));
        }
        let end = self.expect(TokenKind::RBracket).span;
        Expr::Array(elems, start.merge(end))
    }

    fn parse_brace_expr(&mut self) -> Expr {
        // Could be map, record, or block. Prefer block when the body starts with a
        // statement (`x = …`, `when`, `if`, …) so `when … => { … }` works.
        if self.looks_like_block_expr() {
            return Expr::Block(self.parse_block());
        }
        // Prefer map/record for `{k: v}` / `{a, b}` / `{}`.
        let start = self.current().span;
        self.expect(TokenKind::LBrace);
        if self.is(TokenKind::RBrace) {
            self.advance();
            return Expr::Record(vec![], start.merge(self.previous_span()));
        }
        // Lookahead: ident: → map/record field; ident, or ident} → punning record
        // string: or [ → map
        let mut entries_map = Vec::new();
        let mut fields_rec = Vec::new();
        let mut as_map = false;
        let mut as_record = false;

        loop {
            if self.is(TokenKind::RBrace) || self.is(TokenKind::Eof) {
                break;
            }
            if self.is(TokenKind::String) || self.is(TokenKind::LBracket) {
                as_map = true;
                let key = if self.is(TokenKind::String) {
                    MapKey::String(self.expect_string_sym())
                } else {
                    self.expect(TokenKind::LBracket);
                    let e = self.parse_expr(0);
                    self.expect(TokenKind::RBracket);
                    MapKey::Expr(e)
                };
                self.expect(TokenKind::Colon);
                let value = self.parse_expr(0);
                entries_map.push(MapEntry { key, value });
            } else if self.is(TokenKind::Ident) {
                let name = self.expect_ident();
                if self.eat(TokenKind::Colon) {
                    let value = self.parse_expr(0);
                    // ambiguous map vs record — treat unquoted keys as record/map hybrid
                    as_record = true;
                    entries_map.push(MapEntry {
                        key: MapKey::Ident(name),
                        value: value.clone(),
                    });
                    fields_rec.push(RecordField {
                        name: Some(name),
                        value,
                        span: self.previous_span(),
                    });
                } else {
                    // punning
                    as_record = true;
                    let span = self.previous_span();
                    fields_rec.push(RecordField {
                        name: Some(name),
                        value: Expr::Ident(name, span),
                        span,
                    });
                }
            } else {
                break;
            }
            self.eat(TokenKind::Comma);
        }
        let end = self.expect(TokenKind::RBrace).span;
        let span = start.merge(end);
        if as_map && !as_record {
            Expr::Map(entries_map, span)
        } else if as_record {
            // Prefer Record for anonymous structs; maps with string keys already set as_map
            if entries_map
                .iter()
                .any(|e| matches!(e.key, MapKey::String(_) | MapKey::Expr(_)))
            {
                Expr::Map(entries_map, span)
            } else {
                Expr::Record(fields_rec, span)
            }
        } else {
            Expr::Record(fields_rec, span)
        }
    }

    fn parse_struct_lit(&mut self, name: Option<SymId>, start: Span) -> Expr {
        let fields = self.parse_brace_fields();
        Expr::StructLit {
            name,
            fields,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_inferred_struct_after_dot(&mut self, start: Span) -> Expr {
        let fields = self.parse_brace_fields();
        Expr::InferredStruct {
            fields,
            span: start.merge(self.previous_span()),
        }
    }

    fn parse_brace_fields(&mut self) -> Vec<RecordField> {
        self.expect(TokenKind::LBrace);
        let mut fields = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let fstart = self.current().span;
            if self.is(TokenKind::Ident) {
                let name = self.expect_ident();
                if self.eat(TokenKind::Colon) {
                    let value = self.parse_expr(0);
                    fields.push(RecordField {
                        name: Some(name),
                        value,
                        span: fstart.merge(self.previous_span()),
                    });
                } else {
                    fields.push(RecordField {
                        name: Some(name),
                        value: Expr::Ident(name, fstart),
                        span: fstart,
                    });
                }
            } else {
                break;
            }
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace);
        fields
    }

    fn finish_call(&mut self, callee: Expr) -> Expr {
        self.expect(TokenKind::LParen);
        let (args, punning, end) = self.parse_call_args();
        Expr::Call {
            span: callee.span().merge(end),
            callee: Box::new(callee),
            args,
            punning,
        }
    }

    /// Parse `(args…)` after the opening `(` has already been seen (`is(LParen)`).
    fn parse_arg_list_only(&mut self) -> Vec<Expr> {
        self.expect(TokenKind::LParen);
        let (args, _, _) = self.parse_call_args();
        args
    }

    fn parse_call_args(&mut self) -> (Vec<Expr>, bool, Span) {
        let mut args = Vec::new();
        let mut punning = false;
        while !self.is(TokenKind::RParen) && !self.is(TokenKind::Eof) {
            // Call punning: `foo(.)` or `foo(a, .)` — fill remaining from locals
            if self.is(TokenKind::Dot)
                && matches!(
                    self.peek_kind(1),
                    Some(TokenKind::RParen | TokenKind::Comma) | None
                )
            {
                self.advance();
                punning = true;
                self.eat(TokenKind::Comma);
                continue;
            }
            args.push(self.parse_expr(0));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(TokenKind::RParen).span;
        (args, punning, end)
    }

    fn parse_chan_expr(&mut self) -> Expr {
        let start = self.advance().span;
        self.expect(TokenKind::LBracket);
        let elem_ty = self.parse_type_expr();
        self.expect(TokenKind::RBracket);
        // `chan[T](64)` or bare `chan[T]` (default capacity 64).
        let (capacity, end) = if self.eat(TokenKind::LParen) {
            let capacity = self.parse_expr(0);
            let end = self.expect(TokenKind::RParen).span;
            (capacity, end)
        } else {
            (Expr::Int(64, self.previous_span()), self.previous_span())
        };
        Expr::Chan {
            elem_ty,
            capacity: Box::new(capacity),
            span: start.merge(end),
        }
    }

    fn finish_field_or_method(&mut self, base: Expr) -> Expr {
        self.expect(TokenKind::Dot);
        // `Type.{...}` inferred/named struct after path head
        if self.is(TokenKind::LBrace) {
            let name = match base {
                Expr::Ident(n, _) => Some(n),
                _ => None,
            };
            let fields = self.parse_brace_fields();
            return Expr::StructLit {
                name,
                fields,
                span: base.span().merge(self.previous_span()),
            };
        }
        let field = self.expect_ident();
        let span = base.span().merge(self.previous_span());
        // Duration sugar: `16.ms` / `2.s` / `500.us`
        if matches!(base, Expr::Int(_, _) | Expr::Float(_, _)) && !self.is(TokenKind::LParen) {
            let unit_name = self.interner.resolve(field);
            if let Some(unit) = match unit_name {
                "ms" => Some(DurationUnit::Ms),
                "us" => Some(DurationUnit::Us),
                "s" => Some(DurationUnit::S),
                _ => None,
            } {
                return Expr::Duration {
                    value: Box::new(base),
                    unit,
                    span,
                };
            }
        }
        // `ecs.World{}` / `http.Response` path then struct lit.
        // `{ 0 - 1 }` and `{ return x }` are blocks, not field lists.
        if self.is(TokenKind::LBrace) && self.looks_like_struct_lit_body() {
            let fields = self.parse_brace_fields();
            return Expr::StructLit {
                name: Some(field),
                fields,
                span: base.span().merge(self.previous_span()),
            };
        }
        // `Status.Paid(…)`, `MwKind.Recover` — sum variant construction.
        // Variants are UpperCamelCase; lowering here keeps UFCS pipelines intact.
        if let Expr::Ident(sum, _) = &base {
            let vname = self.interner.resolve(field);
            if vname.starts_with(|c: char| c.is_ascii_uppercase()) {
                if self.is(TokenKind::LParen) {
                    let args = self.parse_arg_list_only();
                    let end = self.previous_span();
                    return Expr::NamedVariant {
                        sum: *sum,
                        variant: field,
                        args,
                        span: base.span().merge(end),
                    };
                }
                return Expr::NamedVariant {
                    sum: *sum,
                    variant: field,
                    args: vec![],
                    span,
                };
            }
        }
        if self.is(TokenKind::LParen) {
            // method call → UFCS Call(Field) or Pipeline
            let mut call = self.finish_call(Expr::Field {
                base: Box::new(base),
                field,
                span,
            });
            // Rewrite as Pipeline for clarity when used in chains
            if let Expr::Call {
                callee,
                args,
                punning: _,
                span,
            } = call
            {
                if let Expr::Field { base, field, .. } = *callee {
                    call = Expr::Pipeline {
                        base,
                        method: field,
                        args,
                        span,
                    };
                } else {
                    call = Expr::Call {
                        callee,
                        args,
                        punning: false,
                        span,
                    };
                }
            }
            call
        } else {
            Expr::Field {
                base: Box::new(base),
                field,
                span,
            }
        }
    }

    fn finish_index(&mut self, base: Expr) -> Expr {
        self.expect(TokenKind::LBracket);
        // Slice shorthand: `msg[lo..hi]` — Pratt already folds `lo..hi` into Range.
        let inner = self.parse_expr(0);
        let end = self.expect(TokenKind::RBracket).span;
        let span = base.span().merge(end);
        if let Expr::Range {
            start,
            end: end_expr,
            ..
        } = inner
        {
            Expr::Slice {
                span,
                base: Box::new(base),
                start,
                end: end_expr,
            }
        } else {
            Expr::Index {
                span,
                base: Box::new(base),
                index: Box::new(inner),
            }
        }
    }

    fn binop_of(&self, kind: TokenKind) -> Option<BinOp> {
        Some(match kind {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Rem,
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::NotEq => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::LtEq => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::GtEq => BinOp::Ge,
            TokenKind::AmpAmp => BinOp::And,
            TokenKind::PipePipe => BinOp::Or,
            TokenKind::Amp => BinOp::BitAnd,
            TokenKind::Pipe => BinOp::BitOr,
            TokenKind::Caret => BinOp::BitXor,
            TokenKind::LessLess => BinOp::Shl,
            TokenKind::GreaterGreater => BinOp::Shr,
            TokenKind::GreaterGreaterGreater => BinOp::UShr,
            _ => return None,
        })
    }

    fn infix_binding_power(&self, op: BinOp) -> (u8, u8) {
        match op {
            BinOp::Or => (3, 4),
            BinOp::And => (5, 6),
            BinOp::BitOr => (7, 8),
            BinOp::BitXor => (9, 10),
            BinOp::BitAnd => (11, 12),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (13, 14),
            BinOp::Shl | BinOp::Shr | BinOp::UShr => (15, 16),
            BinOp::Add | BinOp::Sub => (17, 18),
            BinOp::Mul | BinOp::Div | BinOp::Rem => (19, 20),
        }
    }

    fn assign_of(&self, kind: TokenKind) -> Option<(AssignOp, u8, u8)> {
        match kind {
            TokenKind::Eq => Some((AssignOp::Assign, 2, 1)),
            TokenKind::PlusEq => Some((AssignOp::AddAssign, 2, 1)),
            TokenKind::MinusEq => Some((AssignOp::SubAssign, 2, 1)),
            TokenKind::StarEq => Some((AssignOp::MulAssign, 2, 1)),
            TokenKind::SlashEq => Some((AssignOp::DivAssign, 2, 1)),
            _ => None,
        }
    }

    // --- helpers ---

    /// `{` starts a statement block (`x = …`, `when`, `if`, call, …) rather than a record/map.
    fn looks_like_block_expr(&self) -> bool {
        if !self.is(TokenKind::LBrace) {
            return false;
        }
        match self.peek_kind(1) {
            Some(
                TokenKind::If
                | TokenKind::When
                | TokenKind::Select
                | TokenKind::Ensure
                | TokenKind::Each
                | TokenKind::For
                | TokenKind::Par
                | TokenKind::While
                | TokenKind::Loop
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Panic
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Scope
                | TokenKind::Spawn
                | TokenKind::At,
            ) => true,
            Some(TokenKind::Ident) => matches!(
                self.peek_kind(2),
                Some(TokenKind::Eq | TokenKind::LParen | TokenKind::Dot)
            ),
            _ => false,
        }
    }

    /// Distinguish `User{id: 1}` / `n{a, b}` from `for i in 0..n { a, b = 1, 2 }`.
    ///
    /// A bare `Ident {` after a range end is ambiguous: Super allows both value
    /// struct literals with field punning and `for`/`for` bodies. Multi-assign
    /// `a, b = …` must win as a block, not a struct lit.
    fn looks_like_struct_lit_body(&self) -> bool {
        // Current must be `{`; peek inside.
        if !self.is(TokenKind::LBrace) {
            return false;
        }
        match self.peek_kind(1) {
            Some(TokenKind::RBrace) => true,
            Some(TokenKind::Ident) => match self.peek_kind(2) {
                Some(TokenKind::Colon | TokenKind::RBrace) => true,
                Some(TokenKind::Comma) => self.looks_like_struct_punning_not_multi_assign(),
                _ => false,
            },
            Some(TokenKind::Minus) => true, // -private field
            _ => false,
        }
    }

    /// After `{ Ident , …` — true for `{a, b}` / `{a, b: 1}`, false for `{a, b = 1, 2}`.
    fn looks_like_struct_punning_not_multi_assign(&self) -> bool {
        let mut i = 1usize; // first Ident inside `{`
        loop {
            if self.peek_kind(i) != Some(TokenKind::Ident) {
                return false;
            }
            match self.peek_kind(i + 1) {
                Some(TokenKind::Comma) => {
                    i += 2;
                    continue;
                }
                Some(TokenKind::Colon | TokenKind::RBrace) => return true,
                Some(TokenKind::Eq) => return false, // multi-assign
                _ => return false,
            }
        }
    }

    /// `+Name(` / `+Name=` / `+Name:` / `-name:` look like declarations, not infix ops.
    fn looks_like_decl_prefix(&self) -> bool {
        // A declaration prefix begins a line; `a + f(x)` mid-expression is arithmetic.
        if !self.starts_line() {
            return false;
        }
        match self.current().kind {
            TokenKind::Plus => matches!(
                (self.peek_kind(1), self.peek_kind(2)),
                (
                    Some(TokenKind::Ident),
                    Some(TokenKind::LParen | TokenKind::Eq | TokenKind::Colon)
                )
            ),
            TokenKind::Minus => {
                self.peek_kind(1) == Some(TokenKind::Ident)
                    && self.peek_kind(2) == Some(TokenKind::Colon)
            }
            _ => false,
        }
    }

    /// True when the current token is the first on its source line.
    fn starts_line(&self) -> bool {
        if self.pos == 0 {
            return true;
        }
        self.line_starts.get(self.pos).copied().unwrap_or(true)
    }

    fn current(&self) -> Token {
        self.tokens
            .get(self.pos)
            .copied()
            .unwrap_or(Token::new(TokenKind::Eof, Span::default()))
    }

    fn peek_kind(&self, offset: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + offset).map(|t| t.kind)
    }

    fn is(&self, kind: TokenKind) -> bool {
        self.current().kind == kind
    }

    /// Contextual word. `as` stays an identifier so it can still be a name.
    fn at_contextual(&self, keyword: ContextualKeyword) -> bool {
        let tok = self.current();
        tok.kind == TokenKind::Ident && tok.sym.is_some_and(|sym| self.is_contextual(sym, keyword))
    }

    fn is_contextual(&self, sym: SymId, keyword: ContextualKeyword) -> bool {
        ContextualKeyword::from_text(self.interner.resolve(sym)) == Some(keyword)
    }

    fn advance(&mut self) -> Token {
        let t = self.current();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.is(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Token {
        if self.is(kind) {
            self.advance()
        } else {
            self.error_here(format!("expected {kind:?}"));
            Token::new(kind, self.current().span)
        }
    }

    fn expect_ident(&mut self) -> SymId {
        if Self::is_name_token_kind(self.current().kind) {
            let t = self.advance();
            t.sym.unwrap_or_else(|| {
                let text = Self::soft_ident_text(t.kind).unwrap_or("anon");
                self.interner.intern(text)
            })
        } else {
            self.error_here("expected identifier");
            // Always consume one token so recovery cannot spin forever.
            if !self.is(TokenKind::Eof) {
                self.advance();
            }
            self.interner.intern("error")
        }
    }

    fn expect_string_sym(&mut self) -> SymId {
        if self.is(TokenKind::String) {
            self.advance().sym.unwrap()
        } else {
            self.error_here("expected string");
            self.interner.intern("")
        }
    }

    fn previous_span(&self) -> Span {
        if self.pos == 0 {
            return Span::default();
        }
        self.tokens[self.pos - 1].span
    }

    fn looks_like_binding_after_hash(&self) -> bool {
        // After `#`, Ident followed by `=` or `:` is a comptime binding.
        if !self.is(TokenKind::Ident) {
            return false;
        }
        matches!(self.peek_kind(1), Some(TokenKind::Eq | TokenKind::Colon))
    }

    fn parse_hash_expr_after_hash(&mut self, start: Span) -> Expr {
        if self.is(TokenKind::LBrace) {
            let body = self.parse_block();
            return Expr::ComptimeBlock {
                body: body.clone(),
                span: start.merge(body.span),
            };
        }
        if self.is(TokenKind::If) {
            self.advance();
            let cond = self.parse_head_expr();
            let then_block = self.parse_block();
            let else_block = if self.eat(TokenKind::Else) {
                Some(self.parse_block())
            } else {
                None
            };
            return Expr::ComptimeIf {
                cond: Box::new(cond),
                then_block,
                else_block,
                span: start.merge(self.previous_span()),
            };
        }
        if self.is(TokenKind::For) || self.is(TokenKind::Each) {
            self.advance();
            let name = self.expect_ident();
            self.expect(TokenKind::In);
            let iter = self.parse_head_expr();
            let body = self.parse_block();
            return Expr::ComptimeEach {
                name,
                iter: Box::new(iter),
                body,
                span: start.merge(self.previous_span()),
            };
        }
        self.parse_macro_call(start)
    }

    /// Prefix tokens that can start an expression after `panic`.
    fn looks_like_expr_start(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Ident
                | TokenKind::Int
                | TokenKind::Float
                | TokenKind::String
                | TokenKind::True
                | TokenKind::False
                | TokenKind::None
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Minus
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::At
                | TokenKind::Hash
                | TokenKind::Dollar
                | TokenKind::QuoteStart
                | TokenKind::Pipe
                | TokenKind::If
                | TokenKind::Await
                | TokenKind::Spawn
                | TokenKind::Chan
                | TokenKind::Shared
                | TokenKind::Embed
        )
    }

    /// After `#`: `name(…)`, `name { … }` (outside condition heads), or bare `name`.
    /// Macro expansion decides whether the name is a macro, an intrinsic, or a
    /// compile-time value / function.
    fn parse_macro_call(&mut self, start: Span) -> Expr {
        let name = self.expect_ident();
        let name_span = self.previous_span();
        let delim = if self.is(TokenKind::LParen) && !self.newline_before_current(name_span) {
            MacroDelim::Paren
        } else if self.is(TokenKind::LBrace)
            && !self.in_block_head
            && !self.newline_before_current(name_span)
        {
            MacroDelim::Brace
        } else {
            MacroDelim::Bare
        };
        let input = if delim == MacroDelim::Bare {
            TokenStream::default()
        } else {
            self.collect_delimited()
        };
        let span = start.merge(self.previous_span());
        let (line, column) = self.location(start);
        let text = self.interner.intern(self.span_text(span).unwrap_or(""));
        Expr::MacroCall(Box::new(MacroCall {
            name,
            delim,
            input,
            span,
            line,
            column,
            text,
        }))
    }

    /// Consume a balanced `(…)` / `{…}` / `[…]` group, returning the tokens
    /// strictly inside it. Interpolated-string tokens are kept verbatim.
    fn collect_delimited(&mut self) -> TokenStream {
        let open = self.advance();
        let mut depth = 1usize;
        let mut stream = TokenStream::default();
        loop {
            let tok = self.current();
            match tok.kind {
                TokenKind::Eof => {
                    self.diagnostics.error(
                        DiagnosticKind::Syntax,
                        open.span,
                        "unclosed macro input",
                    );
                    break;
                }
                TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                    depth -= 1;
                    if depth == 0 {
                        self.advance();
                        break;
                    }
                }
                _ => {}
            }
            let line_start = self.starts_line();
            stream.push(tok, line_start);
            self.advance();
        }
        stream
    }

    /// One-based line and column of `span` in the original source. Inside a
    /// macro expansion every token reports the outermost call site.
    fn location(&self, span: Span) -> (u32, u32) {
        if let Some(location) = self.inherited_location {
            return location;
        }
        let offset = span.lo.0;
        let line = self.newlines.partition_point(|&nl| nl < offset);
        let line_lo = if line == 0 {
            0
        } else {
            self.newlines[line - 1] + 1
        };
        (line as u32 + 1, offset - line_lo + 1)
    }

    /// `#name { (pattern) => { template } … }` — a pattern macro definition.
    /// Called with the cursor on `#` (after an optional `+`).
    fn parse_macro_def(&mut self, exported: bool, docs: SymId) -> MacroDef {
        let start = self.expect(TokenKind::Hash).span;
        let name = self.expect_ident();
        self.expect(TokenKind::LBrace);
        let mut rules = Vec::new();
        while !self.is(TokenKind::RBrace) && !self.is(TokenKind::Eof) {
            let rule_start = self.current().span;
            if !self.is(TokenKind::LParen) {
                self.error_here("expected `(pattern) => { template }` in macro definition");
                self.recover_to_macro_rule();
                continue;
            }
            let pattern = self.collect_delimited();
            self.expect(TokenKind::FatArrow);
            if !self.is(TokenKind::LBrace) {
                self.error_here("expected `{` to start the macro template");
                self.recover_to_macro_rule();
                continue;
            }
            let template = self.collect_delimited();
            rules.push(MacroRule {
                pattern,
                template,
                span: rule_start.merge(self.previous_span()),
            });
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace);
        if rules.is_empty() {
            self.diagnostics.error(
                DiagnosticKind::Syntax,
                start,
                "macro definition needs at least one `(pattern) => { template }` rule",
            );
        }
        MacroDef {
            exported,
            name,
            rules,
            span: start.merge(self.previous_span()),
            docs,
        }
    }

    fn recover_to_macro_rule(&mut self) {
        while !matches!(
            self.current().kind,
            TokenKind::RBrace | TokenKind::Eof | TokenKind::LParen
        ) {
            self.advance();
        }
        if self.is(TokenKind::LParen) && self.starts_line() {
            return;
        }
        if self.is(TokenKind::LParen) {
            self.advance();
        }
    }

    /// `#name {` followed by `(` on the next line opens a macro definition.
    fn looks_like_macro_def(&self) -> bool {
        self.is(TokenKind::Hash)
            && self.peek_kind(1).is_some_and(Self::is_name_token_kind)
            && self.peek_kind(2) == Some(TokenKind::LBrace)
            && matches!(
                self.peek_kind(3),
                Some(TokenKind::LParen | TokenKind::RBrace)
            )
            && self.tokens.get(self.pos + 1).is_some_and(|tok| {
                tok.sym.is_none_or(|sym| {
                    !crate::intrinsics::is_attribute_macro(self.interner.resolve(sym))
                })
            })
    }

    /// Consume a balanced `{ … }` group without parsing it (error recovery).
    fn skip_braced(&mut self) {
        let mut depth = 0usize;
        while !self.is(TokenKind::Eof) {
            match self.advance().kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn error_here(&mut self, msg: impl Into<String>) {
        let span = self.current().span;
        let snippet = self.span_text(span).unwrap_or("").trim();
        let message = if snippet.is_empty() {
            msg.into()
        } else {
            // Cap snippet so diagnostics stay readable on huge tokens.
            let shown = if snippet.len() > 48 {
                format!("{}…", &snippet[..45])
            } else {
                snippet.to_owned()
            };
            format!("{} near `{shown}`", msg.into())
        };
        self.diagnostics.error_code(
            DiagnosticKind::Syntax,
            span,
            super_core::err::E0001,
            message,
        );
    }
}

/// Parse `//#` / `/*#` directive text into a typed attribute.
pub fn parse_meta_directive(text: &str, interner: &mut Interner) -> MetaAttrKind {
    let t = text.trim();
    if t.is_empty() {
        return MetaAttrKind::Unknown;
    }
    if t.starts_with('#') || t.starts_with("each ") || t.starts_with("if ") {
        return MetaAttrKind::InlineComptime(interner.intern(t));
    }
    if let Some(rest) = t.strip_prefix("unroll(")
        && let Some(end) = rest.find(')')
        && let Ok(n) = rest[..end].trim().parse::<u32>()
    {
        return MetaAttrKind::Unroll(n);
    }
    if t == "simd" {
        return MetaAttrKind::Simd;
    }
    if t == "inline" {
        return MetaAttrKind::Inline;
    }
    if let Some(rest) = t.strip_prefix("cfg(") {
        let predicate = rest.strip_suffix(')').unwrap_or(rest).trim();
        return MetaAttrKind::Cfg(interner.intern(predicate));
    }
    if let Some(rest) = t.strip_prefix("route(") {
        // route("POST", "/orders")
        let inner = rest.trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 2 {
            let method = parts[0].trim_matches('"');
            let path = parts[1].trim_matches('"');
            return MetaAttrKind::Route {
                method: interner.intern(method),
                path: interner.intern(path),
            };
        }
    }
    // Legacy bare routes: get("/tasks") / post("/login") / put(...) / delete(...)
    for method in ["get", "post", "put", "delete"] {
        let prefix = format!("{method}(");
        if let Some(rest) = t.strip_prefix(&prefix) {
            let inner = rest.trim_end_matches(')').trim();
            let path = inner.trim_matches('"');
            if !path.is_empty() {
                return MetaAttrKind::Route {
                    method: interner.intern(&method.to_uppercase()),
                    path: interner.intern(path),
                };
            }
        }
    }
    if let Some(rest) = t.strip_prefix("system(") {
        // system(stage: "physics")
        if let Some(q) = rest.find('"') {
            let after = &rest[q + 1..];
            if let Some(end) = after.find('"') {
                return MetaAttrKind::System {
                    stage: interner.intern(&after[..end]),
                };
            }
        }
    }
    if let Some(rest) = t.strip_prefix("assert_size(") {
        let inner = rest.trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() >= 2
            && let Ok(bytes) = parts[1].parse::<u32>()
        {
            return MetaAttrKind::AssertSize {
                ty: interner.intern(parts[0]),
                bytes,
            };
        }
    }
    if let Some(rest) = t.strip_prefix("inspect(") {
        let name = rest.trim_end_matches(')').trim();
        return MetaAttrKind::Inspect(interner.intern(name));
    }
    if let Some(rest) = t.strip_prefix("law(") {
        let inner = rest.trim_end_matches(')').trim();
        let desc = inner.trim_matches('"').trim();
        if !desc.is_empty() {
            return MetaAttrKind::Law {
                description: interner.intern(desc),
            };
        }
    }
    if t == "law" {
        return MetaAttrKind::Law {
            description: interner.intern(""),
        };
    }
    if let Some(rest) = t.strip_prefix("proof(") {
        let inner = rest.trim_end_matches(')').trim();
        let law = inner.trim_matches('"').trim();
        if !law.is_empty() {
            return MetaAttrKind::Proof {
                law: interner.intern(law),
            };
        }
    }
    if t == "symmetric" {
        return MetaAttrKind::Symmetric;
    }
    MetaAttrKind::Unknown
}

/// Parse Super integer literals: decimal, `0x`/`0b`/`0o`, and `_` separators.
fn parse_int_literal(text: &str) -> i64 {
    let raw: String = text.chars().filter(|c| *c != '_').collect();
    let (sign, body) = if let Some(rest) = raw.strip_prefix('-') {
        (-1i64, rest)
    } else {
        (1i64, raw.as_str())
    };
    let n = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else if let Some(bin) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
        i64::from_str_radix(bin, 2).unwrap_or(0)
    } else if let Some(oct) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) {
        i64::from_str_radix(oct, 8).unwrap_or(0)
    } else {
        body.parse::<i64>().unwrap_or(0)
    };
    n.wrapping_mul(sign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use super_core::Interner;

    fn parse(src: &str) -> (Module, Diagnostics) {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        (module, diags)
    }

    #[test]
    fn parses_callable_type_signature() {
        let (module, diagnostics) = parse("register(handler: Fn[Request, Response]) : i64 => 1");
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
        let Item::Function(function) = &module.items[0] else {
            panic!("expected function");
        };
        let TypeExpr::Apply { args, .. } = &function.params[0].ty else {
            panic!("expected applied parameter type");
        };
        assert_eq!(args.len(), 2);
        assert!(args.iter().all(|arg| matches!(arg, TypeExpr::Named(..))));
    }

    #[test]
    fn parses_import_aliases() {
        let src = "use std.gui as App\nuse std.gui.{Gui as Desk, Node}\n";
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let Item::Use(module_alias) = &module.items[0] else {
            panic!("module alias");
        };
        assert_eq!(interner.resolve(module_alias.alias.expect("alias")), "App");
        let Item::Use(renamed) = &module.items[1] else {
            panic!("rename");
        };
        assert_eq!(interner.resolve(renamed.renames[0].0), "Gui");
        assert_eq!(interner.resolve(renamed.renames[0].1), "Desk");
        assert!(renamed.renames.len() == 1);
    }

    #[test]
    fn parses_braced_sibling_module_import() {
        let src = "use std.data.{column, frame, plan}\n";
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let Item::Use(u) = &module.items[0] else {
            panic!("use");
        };
        assert_eq!(u.path.len(), 2);
        assert_eq!(interner.resolve(u.path[0]), "std");
        assert_eq!(interner.resolve(u.path[1]), "data");
        let sels = u.selectors.as_ref().expect("selectors");
        assert_eq!(sels.len(), 3);
        assert_eq!(interner.resolve(sels[0]), "column");
        assert_eq!(interner.resolve(sels[1]), "frame");
        assert_eq!(interner.resolve(sels[2]), "plan");
    }

    #[test]
    fn parses_inline_trait_declaration() {
        let src = r#"
+Widget = trait {
    +paint(self) : str
    +hit(self, x: i64, y: i64) : bool
}
+Badge: Widget = {
    label: str = ""
    +paint(self) : str => self.label
    +hit(self, x: i64, y: i64) : bool => x == y
}
"#;
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let Item::Trait(tr) = &module.items[0] else {
            panic!("trait");
        };
        assert_eq!(interner.resolve(tr.name), "Widget");
        assert_eq!(tr.methods.len(), 2);
        let Item::Struct(badge) = &module.items[1] else {
            panic!("badge");
        };
        assert_eq!(interner.resolve(badge.traits[0]), "Widget");
        assert_eq!(badge.methods.len(), 2);
    }

    #[test]
    fn parses_bindings() {
        let (m, d) = parse("+TaxRate = 0.14\nhost = \"localhost\"\nport: u16 = 8080");
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert_eq!(m.items.len(), 3);
    }

    #[test]
    fn parses_struct_and_sum() {
        let (m, d) = parse(
            r#"
+User = {
    id: i64
    name: str
    balance: f64 = 0.0
    -secret_token: str = "internal"
}
Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(matches!(m.items[0], Item::Struct(_)));
        assert!(matches!(m.items[1], Item::Sum(_)));
    }

    #[test]
    fn parses_named_variant_construction() {
        let (m, d) = parse(
            r#"
Status = {
    Pending
    Paid(amount: f64)
}
+main() {
    a = Status.Pending
    b = Status.Paid(49.95)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[1] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block");
        };
        let Stmt::Binding(a) = &body.stmts[0] else {
            panic!("expected binding a");
        };
        assert!(
            matches!(&a.value, Expr::NamedVariant { args, .. } if args.is_empty()),
            "Pending should be NamedVariant: {:?}",
            a.value
        );
        let Stmt::Binding(b) = &body.stmts[1] else {
            panic!("expected binding b");
        };
        assert!(
            matches!(&b.value, Expr::NamedVariant { args, .. } if args.len() == 1),
            "Paid(...) should be NamedVariant: {:?}",
            b.value
        );
    }

    #[test]
    fn parses_sum_with_methods() {
        let (m, d, interner) = parse_with_interner(
            r#"
Status = {
    Pending
    Paid(amount: f64)
    is_ok(@self) : bool {
        true
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Sum(s) = &m.items[0] else {
            panic!("expected sum");
        };
        assert_eq!(s.variants.len(), 2);
        assert_eq!(s.methods.len(), 1);
        assert_eq!(interner.resolve(s.methods[0].name), "is_ok");
    }

    #[test]
    fn parses_select_multiplex() {
        let (m, d, _) = parse_with_interner(
            r#"
+main() {
    ch = chan[i64](1)
    select {
        ch.recv() => 1
        after 1 => 0
        else => -1
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block");
        };
        let Stmt::When { subjects, arms, .. } = &body.stmts[1] else {
            panic!("expected select→When, got {:?}", body.stmts[1]);
        };
        assert!(subjects.is_empty(), "select must be subject-less");
        assert_eq!(arms.len(), 3);
        assert!(matches!(arms[1].pattern, WhenPattern::After(_)));
        assert!(matches!(arms[2].pattern, WhenPattern::Else));
    }

    #[test]
    fn select_requires_brace() {
        let (_m, d) = parse("+main() { select 1 }");
        assert!(
            d.has_errors(),
            "select with a subject must be a syntax error"
        );
    }

    #[test]
    fn legacy_when_multiplex_still_parses() {
        let (m, d) = parse(
            r#"
+main() {
    when {
        after 1 => 0
        else => 1
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block");
        };
        let Stmt::When { subjects, .. } = &body.stmts[0] else {
            panic!("expected when multiplex");
        };
        assert!(subjects.is_empty());
    }

    #[test]
    fn parses_when_variant_bind() {
        let (m, d, interner) = parse_with_interner(
            r#"
Status = {
    Pending
    Paid(amount: f64)
}
+main() {
    s = Status.Paid(1.0)
    when s {
        Status.Paid { amount } => amount
        Status.Failed { reason: r } => 0.0
        else => 0.0
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[1] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block");
        };
        let Stmt::When { arms, .. } = &body.stmts[1] else {
            panic!("expected when, got {:?}", body.stmts[1]);
        };
        assert!(matches!(
            &arms[0].pattern,
            WhenPattern::VariantBind { fields, .. } if fields.len() == 1
                && interner.resolve(fields[0].0) == "amount"
                && interner.resolve(fields[0].1) == "amount"
        ));
        assert!(matches!(
            &arms[1].pattern,
            WhenPattern::VariantBind { fields, .. } if fields.len() == 1
                && interner.resolve(fields[0].0) == "reason"
                && interner.resolve(fields[0].1) == "r"
        ));
    }

    #[test]
    fn parses_function_and_ufcs() {
        let (m, d) = parse(
            r#"
+add_tax(amount: f64)  => amount * (1.0 + TaxRate)
+discount(@target: User, percent: f64) : f64 {
    cut = target.balance * (percent / 100.0)
    target.balance -= cut
    return target.balance
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert_eq!(m.items.len(), 2);
    }

    #[test]
    fn parses_panic_and_macro_calls() {
        let (m, d) = parse(
            r#"
main() {
    panic
    panic "boom"
    #todo
    #todo("later")
    #unimplemented
    #unreachable("nope")
    #assert(1 == 1)
    #assert(true, "ok")
    x = #dbg(1 + 2)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block");
        };
        assert!(
            matches!(body.stmts[0], Stmt::Panic(None, _)),
            "bare panic: {:?}",
            body.stmts[0]
        );
        assert!(
            matches!(body.stmts[1], Stmt::Panic(Some(_), _)),
            "panic msg: {:?}",
            body.stmts[1]
        );
        let Stmt::Expr(Expr::MacroCall(todo)) = &body.stmts[2] else {
            panic!("#todo: {:?}", body.stmts[2]);
        };
        assert_eq!(todo.delim, MacroDelim::Bare);
        assert!(todo.input.is_empty());
        let Stmt::Expr(Expr::MacroCall(assert)) = &body.stmts[6] else {
            panic!("#assert: {:?}", body.stmts[6]);
        };
        assert_eq!(assert.delim, MacroDelim::Paren);
        assert_eq!(assert.input.len(), 3);
        assert_eq!((assert.line, assert.column), (9, 5));
        assert!(
            matches!(
                body.stmts.last(),
                Some(Stmt::Binding(Binding {
                    value: Expr::MacroCall(_),
                    ..
                }))
            ),
            "#dbg binding: {:?}",
            body.stmts.last()
        );
    }

    #[test]
    fn parses_symmetric_attr() {
        let (m, d) = parse(
            r#"
#symmetric
+add(a: i64, b: f64) : f64 => a.to_float() + b
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected symmetric fn");
        };
        assert!(f.is_symmetric(), "expected #symmetric on first fn");
    }

    #[test]
    fn parses_tuple_when() {
        let (m, d) = parse(
            r#"
+dispatch(x: i64, y: i64) : i64 {
    when (x, y) {
        (0, 0) => 0
        (1, _) => 1
        else => 2
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(g) = &m.items[0] else {
            panic!("expected dispatch fn");
        };
        let FnBody::Block(body) = &g.body else {
            panic!("expected block");
        };
        let Stmt::When { subjects, arms, .. } = &body.stmts[0] else {
            panic!("expected when, got {:?}", body.stmts[0]);
        };
        assert_eq!(subjects.len(), 2);
        assert_eq!(arms.len(), 3);
        assert!(matches!(&arms[0].pattern, WhenPattern::Tuple { elems, .. } if elems.len() == 2));
        assert!(matches!(&arms[1].pattern, WhenPattern::Tuple { elems, .. } if elems.len() == 2));
        assert!(matches!(arms[2].pattern, WhenPattern::Else));
    }

    #[test]
    fn parses_law_and_proof_attrs() {
        let (m, d) = parse(
            r#"
#law("add is commutative")
add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}

#proof(add_comm)
prove_add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(matches!(&m.items[0], Item::Function(f) if f.is_law()));
        assert!(matches!(&m.items[1], Item::Function(f) if f.is_proof()));
        if let Item::Function(f) = &m.items[0] {
            let desc = f.law_description().expect("law desc");
            // description is interned — resolve via a fresh interner path is awkward here;
            // presence of Law attr is enough alongside is_law().
            let _ = desc;
        }
    }

    #[test]
    fn parses_lambda_and_route_attrs() {
        let (m, d) = parse(
            r#"
#get("/tasks")
list_tasks(req: i64) : i64 => req

#post("/login")
login = |req: i64| : i64 => req

double = |x: i64| => x * 2
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(matches!(&m.items[0], Item::Function(f) if {
            f.attrs.iter().any(|a| matches!(&a.kind, MetaAttrKind::Route { .. }))
        }));
        assert!(matches!(&m.items[1], Item::Binding(b) if {
            matches!(&b.value, Expr::Lambda { .. })
                && b.attrs.iter().any(|a| matches!(&a.kind, MetaAttrKind::Route { .. }))
        }));
        assert!(matches!(&m.items[2], Item::Binding(b) if matches!(&b.value, Expr::Lambda { .. })));
    }

    #[test]
    fn parses_test_block() {
        let (m, d) = parse(
            r#"
test "discount reduces balance accurately" {
    assert_eq(1, 1)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(matches!(m.items[0], Item::Test(_)));
    }

    #[test]
    fn parses_comptime_and_meta() {
        let (m, d) = parse(
            r#"
//# unroll(4)
#N = 4
#TABLE = #for i in 0..4 { i }
#Target = #goal_type
f(x: i64)  => '{ x + 1 }
g(y: i64)  => ${ f(y) }
rewrite "fuse" (xs: i64) {
    xs => xs
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(m.items.iter().any(|i| matches!(i, Item::Rewrite(_))));
        assert!(
            m.items
                .iter()
                .any(|i| matches!(i, Item::Binding(Binding { comptime: true, .. })))
        );
    }

    #[test]
    fn parses_exported_comptime_bindings_and_functions() {
        let (m, d) = parse(
            r#"
+#WIDTH = 8
+#make_add(#n: i64, x: i64) : i64 => x + #n
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(m.items.iter().any(|i| matches!(
            i,
            Item::Binding(Binding {
                exported: true,
                comptime: true,
                ..
            })
        )));
        assert!(
            m.items
                .iter()
                .any(|i| matches!(i, Item::Function(Function { exported: true, .. })))
        );
    }

    #[test]
    fn parses_hash_meta_attrs_without_comment() {
        let (m, d) = parse(
            r#"
#inline
#simd
#unroll(4)
+hot(x: f64) : f64 => x * 2.0
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        assert!(
            f.attrs.len() >= 3,
            "expected #inline/#simd/#unroll attrs, got {:?}",
            f.attrs
        );
    }

    #[test]
    fn parses_multi_assign_inside_for_range_ident() {
        // Regression: `0..n { a, b = … }` must not parse `n{ a, b = … }` as a struct lit.
        let (m, d) = parse(
            r#"
+main() {
    n = 3
    for i in 0..n {
        for j in 0..n {
            ax, ay = 0.0, 0.0
            ax += 1.0
            ay += 1.0
        }
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert!(!m.items.is_empty());
    }

    #[test]
    fn range_span_covers_full_call_rhs() {
        // Site highlighter regression twin: AST span for `0..len(xs)` / `0..foo(bar.baz)`
        // must cover the entire range expression, not stop at a short ident.
        let src = r#"
+main() {
    for 0..len(xs) { }
    for 0..foo(bar.baz) { }
}
"#;
        let (m, d) = parse(src);
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block body");
        };
        let mut ranges = Vec::new();
        for stmt in &body.stmts {
            let Stmt::Each { iter, .. } = stmt else {
                continue;
            };
            if let Expr::Range { span, .. } = iter {
                ranges.push(&src[span.lo.as_usize()..span.hi.as_usize()]);
            }
        }
        assert_eq!(
            ranges,
            vec!["0..len(xs)", "0..foo(bar.baz)"],
            "range spans truncated: {ranges:?}"
        );
    }

    #[test]
    fn parses_module_multi_binding_with_per_name_export() {
        let (m, d, interner) = parse_with_interner(
            r#"
+tasks, +next_id, name = [], 1, "Ahmad"
+a: i64, b: str = 7, "x"
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let bindings: Vec<_> = m
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Binding(b) => Some(b),
                _ => None,
            })
            .collect();
        assert_eq!(
            bindings.len(),
            5,
            "expected 5 bindings, got {}",
            bindings.len()
        );
        assert!(bindings[0].exported);
        assert_eq!(interner.resolve(bindings[0].name), "tasks");
        assert!(bindings[1].exported);
        assert_eq!(interner.resolve(bindings[1].name), "next_id");
        assert!(!bindings[2].exported);
        assert_eq!(interner.resolve(bindings[2].name), "name");
        assert!(bindings[3].exported && bindings[3].ty.is_some());
        assert!(!bindings[4].exported && bindings[4].ty.is_some());
    }

    fn parse_with_interner(src: &str) -> (Module, Diagnostics, Interner) {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        (module, diags, interner)
    }

    #[test]
    fn sum_is_ordinary_ident_not_reduction_keyword() {
        let (m, d, interner) = parse_with_interner("sum = 1\n");
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Binding(b) = &m.items[0] else {
            panic!("expected binding");
        };
        assert_eq!(interner.resolve(b.name), "sum");
        assert!(matches!(b.value, Expr::Int(1, _)));

        let (m, d, interner) =
            parse_with_interner("+main() {\n    rows = [1, 2, 3]\n    total = rows.sum()\n}\n");
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected main");
        };
        let FnBody::Block(body) = &f.body else {
            panic!("expected block body");
        };
        let Stmt::Binding(total) = &body.stmts[1] else {
            panic!("expected total binding");
        };
        assert_eq!(interner.resolve(total.name), "total");
        let Expr::Pipeline {
            base, method, args, ..
        } = &total.value
        else {
            panic!("expected rows.sum() as Pipeline, got {:?}", total.value);
        };
        assert!(args.is_empty());
        assert_eq!(interner.resolve(*method), "sum");
        assert!(matches!(base.as_ref(), Expr::Ident(name, _) if interner.resolve(*name) == "rows"));

        let (_m, d, _) = parse_with_interner("+main() { sum for rows }\n");
        assert!(
            d.has_errors(),
            "sum for rows must not parse as keyword reduction; expected parse errors, got {:?}",
            d.into_vec()
        );
    }

    #[test]
    fn ufcs_chain_parses_as_left_nested_pipelines() {
        let (m, d) = parse("+x() => 49.0.sqrt().floor().to_int()\n");
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Function(f) = &m.items[0] else {
            panic!("expected function")
        };
        // Each stage's result is the next receiver: to_int(floor(sqrt(49.0))).
        let mut depth = 0;
        let mut cur = match &f.body {
            FnBody::Expr(e) => e,
            other => panic!("unexpected body {other:?}"),
        };
        while let Expr::Pipeline { base, args, .. } = cur {
            assert!(args.is_empty());
            depth += 1;
            cur = base;
        }
        assert_eq!(depth, 3);
        assert!(matches!(cur, Expr::Float(..)));
    }

    #[test]
    fn doc_comments_attach_to_the_next_declaration() {
        let src = "\
//! JSON helpers.\n\
/// Adds tax.\n\
+add(amount: f64) => amount\n\
+User = {\n\
    //! A person.\n\
    /// Identifier.\n\
    id: i64\n\
    /// Apply a percent cut.\n\
    discount(@self, percent: f64) : f64 => percent\n\
}\n\
/**\n\
 * Payment state.\n\
 */\n\
Status = {\n\
    /// Waiting.\n\
    Pending\n\
    Paid(amount: f64)\n\
}\n\
+main() {\n\
    /// scratch total\n\
    total = 1\n\
}\n\
";
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        assert_eq!(interner.resolve(module.docs), "JSON helpers.");
        let Item::Function(add) = &module.items[0] else {
            panic!("expected add");
        };
        assert_eq!(interner.resolve(add.docs), "Adds tax.");
        let Item::Struct(user) = &module.items[1] else {
            panic!("expected User");
        };
        assert_eq!(interner.resolve(user.docs), "A person.");
        assert_eq!(interner.resolve(user.fields[0].docs), "Identifier.");
        assert_eq!(
            interner.resolve(user.methods[0].docs),
            "Apply a percent cut."
        );
        let Item::Sum(status) = &module.items[2] else {
            panic!("expected Status");
        };
        assert_eq!(interner.resolve(status.docs), "Payment state.");
        assert_eq!(interner.resolve(status.variants[0].docs), "Waiting.");
        let Item::Function(main) = &module.items[3] else {
            panic!("expected main");
        };
        let FnBody::Block(body) = &main.body else {
            panic!("expected block");
        };
        let Stmt::Binding(total) = &body.stmts[0] else {
            panic!("expected binding, got {:?}", body.stmts[0]);
        };
        assert_eq!(interner.resolve(total.docs), "scratch total");
    }

    #[test]
    fn nested_call_and_ufcs_chain_both_parse() {
        let (_m, d) = parse(
            "+a() => to_int(floor(sqrt(49.0)))\n+b() => 49.0.sqrt().floor().to_int()\n+c(s: str) => s.byte_len().to_float()\n",
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
    }

    #[test]
    fn lowercase_name_before_body_brace_is_not_a_struct_literal() {
        let (module, diagnostics) = parse(
            "mx(a: i64, b: i64) : i64 => if a > b { a } else { b }\nmk(v: i64) : P {\n    if v > 0 { return P{v} }\n    P{v: 0}\n}\n",
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
        let Item::Function(function) = &module.items[0] else {
            panic!("expected function");
        };
        let FnBody::Expr(Expr::If { cond, .. }) = &function.body else {
            panic!("expected if expression, got {:?}", function.body);
        };
        assert!(
            matches!(cond.as_ref(), Expr::Binary { right, .. } if matches!(right.as_ref(), Expr::Ident(..)))
        );
    }

    #[test]
    fn assert_and_routes_are_ordinary_names() {
        let (module, diagnostics) = parse(
            r#"
test "plain names" {
    routes = [1, 2]
    assert(len(routes) == 2)
    #assert(len(routes) == 2)
}
"#,
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
        let Item::Test(test) = &module.items[0] else {
            panic!("expected test");
        };
        assert!(matches!(test.body.stmts[0], Stmt::Binding(..)));
        assert!(matches!(
            &test.body.stmts[1],
            Stmt::Expr(Expr::Call { callee, .. }) if matches!(callee.as_ref(), Expr::Ident(..))
        ));
        assert!(matches!(
            &test.body.stmts[2],
            Stmt::Expr(Expr::MacroCall(_))
        ));
    }

    #[test]
    fn name_followed_by_brace_points_to_the_macro_form() {
        let (module, diagnostics) = parse(
            r#"
+main() {
    app = routes {
        get "/x" { ok(1) }
        post "/y" |req| { ok(2) }
    }
    print(app)
}
"#,
        );
        let errors = diagnostics.into_vec();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0].help.as_deref(),
            Some("to invoke a block macro write `#routes { … }`")
        );
        let Item::Function(Function {
            body: FnBody::Block(body),
            ..
        }) = &module.items[0]
        else {
            panic!("expected a block function");
        };
        assert_eq!(body.stmts.len(), 2, "recovery resumes after the braces");
    }

    #[test]
    fn names_before_block_bodies_are_not_flagged() {
        let (_, diagnostics) = parse(
            r#"
+main() {
    xs = [1, 2]
    ok = true
    for xs { print(it) }
    if ok { print(1) }
    while ok { break }
    when ok { else => print(2) }
}
"#,
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
    }

    #[test]
    fn soft_keyword_methods_fields_and_ufcs_parse() {
        // Regression: `after`/`use`/`get`/`type`/… are statement keywords but must
        // remain valid method, field, and UFCS names (HTTP App fluent API).
        let src = r#"
+App = {
    routes: i64 = 0
    after_count: i64 = 0
    use(@self, kind: i64) : App {
        self.routes += kind
        self
    }
    get(@self, path: str) : App => self
    post(@self, path: str) : App => self
    put(@self, path: str) : App => self
    delete(@self, path: str) : App => self
    type(@self) : i64 => 1
    after(@self, req: i64, res: i64) : i64 {
        self.after_count += 1
        res
    }
    match(self, path: str) : i64 => 0
}
+dispatch(@app: App, req: i64) : i64 {
    _ = @app.use(1).get("/").post("/").put("/").delete("/")
    app.after(req, 200)
}
"#;
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let Item::Struct(app) = &module.items[0] else {
            panic!("expected App struct");
        };
        let names: Vec<&str> = app
            .methods
            .iter()
            .map(|m| interner.resolve(m.name))
            .collect();
        assert_eq!(
            names,
            [
                "use", "get", "post", "put", "delete", "type", "after", "match"
            ]
        );
        assert_eq!(module.items.len(), 2);
    }

    #[test]
    fn get_is_contextual_for_instances_types_and_chains() {
        let (module, diagnostics) = parse(
            r#"
+probe(x: i64) {
    a = x.get("key")
    app_get = App.get
    map_get = Map.get
    chained = x.get("/a", 1).post("/b", 2)
}
"#,
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
        let Item::Function(function) = &module.items[0] else {
            panic!("expected function");
        };
        let FnBody::Block(body) = &function.body else {
            panic!("expected block");
        };
        assert_eq!(body.stmts.len(), 4);
        for statement in [&body.stmts[0], &body.stmts[3]] {
            assert!(matches!(
                statement,
                Stmt::Binding(Binding {
                    value: Expr::Pipeline { .. },
                    ..
                })
            ));
        }
        for statement in [&body.stmts[1], &body.stmts[2]] {
            assert!(matches!(
                statement,
                Stmt::Binding(Binding {
                    value: Expr::Field { .. },
                    ..
                })
            ));
        }
    }

    #[test]
    fn soft_keyword_binding_and_scope_stmt_are_contextual() {
        // `scope = …` is a binding; `scope { … }` remains the concurrency statement.
        let (m, d) = parse(
            r#"
+check(scopes: _, idx: i64) : bool {
    scope = scopes[idx]
    if scope == "" { return true }
    false
}
+region() {
    scope {
        x = 1
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert_eq!(m.items.len(), 2);
    }

    #[test]
    fn exported_type_methods_are_public_by_default_minus_hides() {
        // Leading field forces struct (not sum) body classification.
        let (m, d) = parse(
            "+T = {\n  x: i64 = 0\n  pub_m(self) : i64 => 1\n  +also_pub(self) : i64 => 2\n  -hidden(self) : i64 => 3\n}\n",
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        let Item::Struct(s) = &m.items[0] else {
            panic!("expected struct");
        };
        assert!(s.exported);
        assert_eq!(s.methods.len(), 3);
        assert!(s.methods[0].exported, "bare method should be public");
        assert!(s.methods[1].exported, "redundant +method should be public");
        assert!(!s.methods[2].exported, "-method should be file-private");
    }

    #[test]
    fn stdlib_sources_parse_with_docs() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("sr") {
                    out.push(path);
                }
            }
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(
            files.len() > 50,
            "expected the stdlib tree at {}",
            root.display()
        );
        let mut failed = Vec::new();
        for path in &files {
            let src = std::fs::read_to_string(path).unwrap();
            let (_module, diags) = parse(&src);
            if diags.has_errors() {
                failed.push(format!("{}: {:?}", path.display(), diags.into_vec()));
            }
        }
        assert!(
            failed.is_empty(),
            "stdlib failed to parse:\n{}",
            failed.join("\n")
        );
    }
}
