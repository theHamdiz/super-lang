//! Pattern macros.
//!
//! A macro is declared in Super source and lives in the `#` namespace:
//!
//! ```text
//! +#name {
//!     (pattern) => { template }
//!     …
//! }
//! ```
//!
//! It is invoked as `#name(…)`, `#name { … }`, or bare `#name`. Expansion runs
//! right after parsing (and module merging), before comptime evaluation and
//! sema, so every later pass sees ordinary AST.
//!
//! * **Patterns** are token sequences with fragments `$x:kind` and repetitions
//!   `$( … ) sep? op` (`op` is `*`, `+`, or `?`). Rules are tried in order; the
//!   first whose pattern consumes the whole input wins. Repetitions are greedy
//!   and never backtrack.
//! * **Templates** substitute fragments (`$x`) and repeat `$( … )` groups once per
//!   captured iteration. A `$name` that no fragment binds is a fresh hygienic
//!   local, unique to that expansion. Every other template word resolves at the
//!   call site.
//! * **Token intrinsics** ([`TokenIntrinsic`]) run on the transcribed tokens,
//!   innermost first, before the result is parsed.
//!
//! The expansion is parsed as statements. A single expression statement becomes
//! that expression; anything else becomes a block expression, or is spliced into
//! the enclosing block when the call is itself a statement.

use crate::ast::*;
use crate::intrinsics::{GOAL_TYPE, TokenIntrinsic};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::token::{RESERVED_KEYWORDS, Token, TokenKind};
use std::collections::HashMap;
use std::rc::Rc;
use super_core::{BytePos, Diagnostic, DiagnosticKind, Diagnostics, Interner, Span, SymId};

/// Stdlib file whose macros are in scope in every module.
pub const PRELUDE_PATH: &str = "core/macros.sr";

/// Nested expansions deeper than this are reported as runaway recursion.
const MAX_DEPTH: u32 = 64;

/// What a `$x:kind` fragment accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fragment {
    /// One identifier (soft keywords included).
    Ident,
    /// `a` / `a.b.c`.
    Path,
    /// One string literal.
    Str,
    /// Number, string, `true`, `false`, `none`, or a negated number.
    Lit,
    /// A path or a literal other than `none` — a value that can be bound to a
    /// fresh local and evaluated again without side effects.
    Operand,
    /// An expression that binds tighter than comparisons (`a + b`, `f(x).y`).
    Term,
    /// Any expression.
    Expr,
    /// A `{ … }` block.
    Block,
    /// A lambda `|…| …` or a bare `{ … }` block.
    Closure,
    /// One token, or one balanced `( … )` / `[ … ]` / `{ … }` group.
    Tt,
    /// A type expression.
    Type,
}

impl Fragment {
    pub const ALL: [Fragment; 11] = [
        Fragment::Ident,
        Fragment::Path,
        Fragment::Str,
        Fragment::Lit,
        Fragment::Operand,
        Fragment::Term,
        Fragment::Expr,
        Fragment::Block,
        Fragment::Closure,
        Fragment::Tt,
        Fragment::Type,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Fragment::Ident => "ident",
            Fragment::Path => "path",
            Fragment::Str => "str",
            Fragment::Lit => "lit",
            Fragment::Operand => "operand",
            Fragment::Term => "term",
            Fragment::Expr => "expr",
            Fragment::Block => "block",
            Fragment::Closure => "closure",
            Fragment::Tt => "tt",
            Fragment::Type => "type",
        }
    }

    /// Substituted with invisible grouping so `$a * 2` keeps `$a` whole.
    const fn is_grouped(self) -> bool {
        matches!(self, Fragment::Term | Fragment::Expr)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepOp {
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

impl RepOp {
    fn of(kind: TokenKind) -> Option<Self> {
        match kind {
            TokenKind::Star => Some(RepOp::ZeroOrMore),
            TokenKind::Plus => Some(RepOp::OneOrMore),
            TokenKind::Question => Some(RepOp::ZeroOrOne),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
enum PatNode {
    Tok(Token),
    Frag {
        name: SymId,
        kind: Fragment,
    },
    Rep {
        body: Vec<PatNode>,
        sep: Option<Token>,
        op: RepOp,
    },
}

#[derive(Clone, Debug)]
enum TplNode {
    Tok {
        tok: Token,
        line_start: bool,
    },
    Var {
        name: SymId,
        line_start: bool,
    },
    Rep {
        body: Vec<TplNode>,
        sep: Option<Token>,
        op: RepOp,
        line_start: bool,
    },
}

#[derive(Debug)]
struct CompiledRule {
    pattern: Vec<PatNode>,
    template: Vec<TplNode>,
    /// Pattern source, for "no rule matches" diagnostics.
    shown: String,
}

#[derive(Debug)]
struct CompiledMacro {
    rules: Vec<CompiledRule>,
}

/// Tokens captured by one fragment.
#[derive(Clone, Debug)]
struct Capture {
    kind: Fragment,
    tokens: Vec<Token>,
    line_starts: Vec<bool>,
}

#[derive(Clone, Debug)]
enum Bound {
    One(Capture),
    Many(Vec<Bound>),
}

type Bindings = HashMap<SymId, Bound>;

/// Result of expanding one call.
enum Expansion {
    Expr(Expr),
    Stmts(Vec<Stmt>),
    Empty,
}

/// The call being transcribed: where intrinsics report and what `#stringify`
/// may slice.
struct CallSite {
    name: SymId,
    span: Span,
    line: u32,
    column: u32,
    text: String,
}

impl CallSite {
    fn of(call: &MacroCall, interner: &Interner) -> Self {
        Self {
            name: call.name,
            span: call.span,
            line: call.line,
            column: call.column,
            text: interner.resolve(call.text).to_owned(),
        }
    }

    /// Source text between two caller-token offsets, when the call text covers it.
    fn slice(&self, lo: u32, hi: u32) -> Option<&str> {
        let base = self.span.lo.0;
        let lo = lo.checked_sub(base)? as usize;
        let hi = hi.checked_sub(base)? as usize;
        self.text.get(lo..hi)
    }

    /// True for tokens written by the caller inside this call (not by a template).
    fn contains(&self, span: Span) -> bool {
        span.expansion_mark() == 0
            && span.file_index() == self.span.file_index()
            && span.lo >= self.span.lo
            && span.hi <= self.span.hi
    }
}

/// Parse the macro definitions of the prelude ([`PRELUDE_PATH`]).
pub fn prelude_macros(interner: &mut Interner) -> Vec<MacroDef> {
    let Some(text) = crate::modules::embedded_stdlib_get(PRELUDE_PATH) else {
        return Vec::new();
    };
    macros_in_source(text, interner)
}

/// Parse `text` and keep only its macro definitions.
pub fn macros_in_source(text: &str, interner: &mut Interner) -> Vec<MacroDef> {
    let mut scratch = Diagnostics::new();
    let tokens = Lexer::new(text, interner, &mut scratch).tokenize();
    let module = Parser::new(&tokens, text, interner, &mut scratch).parse_module();
    module
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Macro(def) => Some(def),
            _ => None,
        })
        .collect()
}

/// Expand every macro call in `module`, with the prelude macros in scope.
pub fn expand_module(module: &mut Module, interner: &mut Interner, diagnostics: &mut Diagnostics) {
    let prelude = prelude_macros(interner);
    expand_module_with(module, &prelude, interner, diagnostics);
}

/// Expand every macro call in `module` with `extra` macros in scope as well as
/// the module's own definitions (which take precedence).
pub fn expand_module_with(
    module: &mut Module,
    extra: &[MacroDef],
    interner: &mut Interner,
    diagnostics: &mut Diagnostics,
) {
    let mut expander = MacroExpander::new(interner, diagnostics);
    for def in extra {
        expander.define(def, false);
    }
    let mut seen: HashMap<SymId, Span> = HashMap::new();
    for item in &module.items {
        if let Item::Macro(def) = item {
            if let Some(first) = seen.insert(def.name, def.span) {
                if first != def.span {
                    let name = expander.interner.resolve(def.name).to_owned();
                    expander.diagnostics.error(
                        DiagnosticKind::Syntax,
                        def.span,
                        format!("macro `#{name}` is defined more than once"),
                    );
                }
                continue;
            }
            expander.define(def, true);
        }
    }
    for item in &mut module.items {
        expander.expand_item(item);
    }
    let mut visible: Vec<SymId> = expander.macros.keys().copied().collect();
    visible.sort_unstable_by_key(|sym| sym.index());
    module.macros = visible;
}

struct MacroExpander<'a> {
    interner: &'a mut Interner,
    diagnostics: &'a mut Diagnostics,
    macros: HashMap<SymId, Rc<CompiledMacro>>,
    depth: u32,
    next_mark: u16,
    next_gensym: u32,
}

impl<'a> MacroExpander<'a> {
    fn new(interner: &'a mut Interner, diagnostics: &'a mut Diagnostics) -> Self {
        Self {
            interner,
            diagnostics,
            macros: HashMap::new(),
            depth: 0,
            next_mark: 0,
            next_gensym: 0,
        }
    }

    // ---------------------------------------------------------------- definitions

    fn define(&mut self, def: &MacroDef, report: bool) {
        let mut rules = Vec::with_capacity(def.rules.len());
        for rule in &def.rules {
            let pattern = self.compile_pattern(&rule.pattern.tokens);
            let template = self.compile_template(&rule.template.tokens, &rule.template.line_starts);
            match (pattern, template) {
                (Ok(pattern), Ok(template)) => rules.push(CompiledRule {
                    pattern,
                    template,
                    shown: self.render(&rule.pattern.tokens),
                }),
                (Err((span, message)), _) | (_, Err((span, message))) => {
                    if report {
                        let name = self.interner.resolve(def.name).to_owned();
                        self.diagnostics.error(
                            DiagnosticKind::Syntax,
                            span,
                            format!("in macro `#{name}`: {message}"),
                        );
                    }
                }
            }
        }
        self.macros
            .insert(def.name, Rc::new(CompiledMacro { rules }));
    }

    fn compile_pattern(&mut self, tokens: &[Token]) -> Result<Vec<PatNode>, (Span, String)> {
        let mut nodes = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let tok = tokens[i];
            if tok.kind != TokenKind::Dollar {
                nodes.push(PatNode::Tok(tok));
                i += 1;
                continue;
            }
            match tokens.get(i + 1).map(|t| t.kind) {
                Some(TokenKind::Ident) => {
                    let name = tokens[i + 1].sym.expect("ident token has a symbol");
                    if tokens.get(i + 2).map(|t| t.kind) != Some(TokenKind::Colon) {
                        return Err((
                            tok.span,
                            "pattern fragment needs a kind: `$name:expr`".to_owned(),
                        ));
                    }
                    let kind_tok = tokens.get(i + 3).copied();
                    let kind_name = kind_tok.and_then(|t| match t.kind {
                        TokenKind::Type => Some("type".to_owned()),
                        TokenKind::Ident => t.sym.map(|s| self.interner.resolve(s).to_owned()),
                        _ => None,
                    });
                    let Some(kind) = kind_name.as_deref().and_then(Fragment::from_name) else {
                        let kinds = Fragment::ALL.map(Fragment::name).join(", ");
                        return Err((
                            kind_tok.map_or(tok.span, |t| t.span),
                            format!("unknown fragment kind; expected one of {kinds}"),
                        ));
                    };
                    nodes.push(PatNode::Frag { name, kind });
                    i += 4;
                }
                Some(TokenKind::LParen) => {
                    let close = matching_close(tokens, i + 1)
                        .ok_or((tok.span, "unclosed `$(` repetition".to_owned()))?;
                    let body = self.compile_pattern(&tokens[i + 2..close])?;
                    let (sep, op, next) = repetition_suffix(tokens, close + 1)
                        .ok_or((tok.span, "repetition needs `*`, `+`, or `?`".to_owned()))?;
                    nodes.push(PatNode::Rep { body, sep, op });
                    i = next;
                }
                _ => {
                    return Err((
                        tok.span,
                        "expected `$name:kind` or `$( … )*` after `$`".to_owned(),
                    ));
                }
            }
        }
        Ok(nodes)
    }

    fn compile_template(
        &mut self,
        tokens: &[Token],
        line_starts: &[bool],
    ) -> Result<Vec<TplNode>, (Span, String)> {
        self.compile_template_range(tokens, line_starts, 0, tokens.len())
    }

    fn compile_template_range(
        &mut self,
        tokens: &[Token],
        line_starts: &[bool],
        lo: usize,
        hi: usize,
    ) -> Result<Vec<TplNode>, (Span, String)> {
        let mut nodes = Vec::new();
        let mut i = lo;
        while i < hi {
            let tok = tokens[i];
            let line_start = line_starts.get(i).copied().unwrap_or(false);
            if tok.kind != TokenKind::Dollar {
                nodes.push(TplNode::Tok { tok, line_start });
                i += 1;
                continue;
            }
            match tokens.get(i + 1).map(|t| t.kind) {
                Some(TokenKind::Ident) => {
                    let name = tokens[i + 1].sym.expect("ident token has a symbol");
                    nodes.push(TplNode::Var { name, line_start });
                    i += 2;
                }
                Some(TokenKind::LParen) => {
                    let close = matching_close(tokens, i + 1)
                        .filter(|&close| close < hi)
                        .ok_or((tok.span, "unclosed `$(` repetition".to_owned()))?;
                    let body = self.compile_template_range(tokens, line_starts, i + 2, close)?;
                    let (sep, op, next) = repetition_suffix(&tokens[..hi], close + 1)
                        .ok_or((tok.span, "repetition needs `*`, `+`, or `?`".to_owned()))?;
                    nodes.push(TplNode::Rep {
                        body,
                        sep,
                        op,
                        line_start,
                    });
                    i = next;
                }
                _ => {
                    return Err((
                        tok.span,
                        "expected `$name` or `$( … )*` after `$`".to_owned(),
                    ));
                }
            }
        }
        Ok(nodes)
    }

    // ------------------------------------------------------------------- matching

    fn match_nodes(
        &mut self,
        nodes: &[PatNode],
        input: &TokenStream,
        pos: &mut usize,
        out: &mut Bindings,
    ) -> bool {
        for node in nodes {
            match node {
                PatNode::Tok(expected) => match input.tokens.get(*pos) {
                    Some(tok) if same_token(tok, expected) => *pos += 1,
                    _ => return false,
                },
                PatNode::Frag { name, kind } => {
                    let Some(end) = self.match_fragment(*kind, input, *pos) else {
                        return false;
                    };
                    out.insert(
                        *name,
                        Bound::One(Capture {
                            kind: *kind,
                            tokens: input.tokens[*pos..end].to_vec(),
                            line_starts: input.line_starts[*pos..end].to_vec(),
                        }),
                    );
                    *pos = end;
                }
                PatNode::Rep { body, sep, op } => {
                    let mut iterations: Vec<Bindings> = Vec::new();
                    loop {
                        if *op == RepOp::ZeroOrOne && !iterations.is_empty() {
                            break;
                        }
                        let start = *pos;
                        let mut cursor = *pos;
                        if !iterations.is_empty()
                            && let Some(sep) = sep
                        {
                            match input.tokens.get(cursor) {
                                Some(tok) if same_token(tok, sep) => cursor += 1,
                                _ => break,
                            }
                        }
                        let mut found = Bindings::new();
                        if !self.match_nodes(body, input, &mut cursor, &mut found)
                            || cursor == start
                        {
                            *pos = start;
                            break;
                        }
                        *pos = cursor;
                        iterations.push(found);
                    }
                    if *op == RepOp::OneOrMore && iterations.is_empty() {
                        return false;
                    }
                    let mut names = Vec::new();
                    pattern_names(body, &mut names);
                    for name in names {
                        let each = iterations
                            .iter_mut()
                            .map(|found| found.remove(&name).unwrap_or(Bound::Many(Vec::new())))
                            .collect();
                        out.insert(name, Bound::Many(each));
                    }
                }
            }
        }
        true
    }

    /// End index of a `kind` fragment starting at `pos`, if one is there.
    fn match_fragment(&mut self, kind: Fragment, input: &TokenStream, pos: usize) -> Option<usize> {
        let tokens = &input.tokens;
        let tok = *tokens.get(pos)?;
        match kind {
            Fragment::Ident => is_word(tok.kind).then_some(pos + 1),
            Fragment::Path => path_end(tokens, pos),
            Fragment::Str => (tok.kind == TokenKind::String).then_some(pos + 1),
            Fragment::Lit => literal_end(tokens, pos),
            Fragment::Operand => match tok.kind {
                TokenKind::None => None,
                _ => literal_end(tokens, pos).or_else(|| path_end(tokens, pos)),
            },
            Fragment::Tt => match tok.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    matching_close(tokens, pos).map(|close| close + 1)
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => None,
                _ => Some(pos + 1),
            },
            Fragment::Block => (tok.kind == TokenKind::LBrace)
                .then(|| matching_close(tokens, pos).map(|c| c + 1))?,
            Fragment::Closure => match tok.kind {
                TokenKind::LBrace => matching_close(tokens, pos).map(|close| close + 1),
                TokenKind::Pipe | TokenKind::PipePipe => self.parse_fragment(kind, input, pos),
                _ => None,
            },
            Fragment::Term | Fragment::Expr | Fragment::Type => {
                self.parse_fragment(kind, input, pos)
            }
        }
    }

    fn parse_fragment(&mut self, kind: Fragment, input: &TokenStream, pos: usize) -> Option<usize> {
        let mut scratch = Diagnostics::new();
        let mut parser = Parser::for_expansion(
            input,
            ("", 0, u32::MAX),
            (0, 0),
            self.interner,
            &mut scratch,
        );
        parser.seek(pos);
        let accepted = match kind {
            Fragment::Term => !matches!(parser.parse_term(), Expr::None(_)),
            Fragment::Expr => !matches!(parser.parse_expression(), Expr::None(_)),
            Fragment::Closure => matches!(parser.parse_expression(), Expr::Lambda { .. }),
            Fragment::Type => {
                parser.parse_type();
                true
            }
            _ => false,
        };
        let end = parser.position();
        (accepted && !scratch.has_errors() && end > pos).then_some(end)
    }

    // -------------------------------------------------------------- transcription

    fn transcribe(
        &mut self,
        nodes: &[TplNode],
        frame: &HashMap<SymId, &Bound>,
        site: &CallSite,
        gensyms: &mut HashMap<SymId, SymId>,
        out: &mut TokenStream,
    ) -> Result<(), String> {
        for node in nodes {
            match node {
                TplNode::Tok { tok, line_start } => {
                    let mut tok = *tok;
                    tok.span = self.mark(site.span);
                    out.push(tok, *line_start);
                }
                TplNode::Var { name, line_start } => match frame.get(name) {
                    Some(Bound::One(capture)) => push_capture(capture, *line_start, out),
                    Some(Bound::Many(_)) => {
                        let name = self.interner.resolve(*name);
                        return Err(format!(
                            "`${name}` is captured by a repetition; use it inside `$( … )*`"
                        ));
                    }
                    None => {
                        let sym = match gensyms.get(name) {
                            Some(sym) => *sym,
                            None => {
                                self.next_gensym += 1;
                                let text = format!(
                                    "__{}_{}",
                                    self.interner.resolve(*name),
                                    self.next_gensym
                                );
                                let sym = self.interner.intern(&text);
                                gensyms.insert(*name, sym);
                                sym
                            }
                        };
                        let span = self.mark(site.span);
                        out.push(Token::with_sym(TokenKind::Ident, span, sym), *line_start);
                    }
                },
                TplNode::Rep {
                    body,
                    sep,
                    op,
                    line_start,
                } => {
                    let mut vars = Vec::new();
                    template_vars(body, &mut vars);
                    let mut count: Option<usize> = None;
                    for var in &vars {
                        if let Some(Bound::Many(items)) = frame.get(var) {
                            match count {
                                None => count = Some(items.len()),
                                Some(n) if n != items.len() => {
                                    return Err(
                                        "fragments repeated together matched different counts"
                                            .to_owned(),
                                    );
                                }
                                Some(_) => {}
                            }
                        }
                    }
                    let Some(count) = count else {
                        return Err(
                            "`$( … )` in a template must use a repeated fragment".to_owned()
                        );
                    };
                    if *op == RepOp::OneOrMore && count == 0 {
                        return Err("`$( … )+` matched nothing".to_owned());
                    }
                    for index in 0..count {
                        if index > 0
                            && let Some(sep) = sep
                        {
                            let mut sep = *sep;
                            sep.span = self.mark(site.span);
                            out.push(sep, false);
                        }
                        let mut inner = frame.clone();
                        for var in &vars {
                            if let Some(Bound::Many(items)) = frame.get(var) {
                                inner.insert(*var, &items[index]);
                            }
                        }
                        let first = out.len();
                        self.transcribe(body, &inner, site, gensyms, out)?;
                        if let Some(flag) = out.line_starts.get_mut(first) {
                            *flag = *line_start;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn mark(&mut self, span: Span) -> Span {
        self.next_mark = self.next_mark.wrapping_add(1).max(1);
        span.with_expansion_mark(self.next_mark)
    }

    // ----------------------------------------------------------------- intrinsics

    /// Evaluate token intrinsics in `stream`, innermost first.
    fn eval_intrinsics(&mut self, stream: TokenStream, site: &CallSite) -> TokenStream {
        let tokens = &stream.tokens;
        let mut out = TokenStream::default();
        let mut i = 0;
        while i < tokens.len() {
            let tok = tokens[i];
            let line_start = stream.line_starts[i];
            let intrinsic = (tok.kind == TokenKind::Hash)
                .then(|| tokens.get(i + 1))
                .flatten()
                .filter(|next| next.kind == TokenKind::Ident)
                .and_then(|next| next.sym)
                .and_then(|sym| TokenIntrinsic::from_name(self.interner.resolve(sym)));
            let Some(intrinsic) = intrinsic else {
                out.push(tok, line_start);
                i += 1;
                continue;
            };
            let has_args = tokens.get(i + 2).map(|t| t.kind) == Some(TokenKind::LParen);
            let (args, next) = if has_args {
                match matching_close(tokens, i + 2) {
                    Some(close) => (
                        TokenStream {
                            tokens: tokens[i + 3..close].to_vec(),
                            line_starts: stream.line_starts[i + 3..close].to_vec(),
                        },
                        close + 1,
                    ),
                    None => (TokenStream::default(), tokens.len()),
                }
            } else {
                (TokenStream::default(), i + 2)
            };
            let args = self.eval_intrinsics(args, site);
            let at = tok.span.merge(tokens[next - 1].span);
            let result = self.apply_intrinsic(intrinsic, &args, at, site);
            for (index, tok) in result.into_iter().enumerate() {
                out.push(tok, index == 0 && line_start);
            }
            i = next;
        }
        out
    }

    fn apply_intrinsic(
        &mut self,
        intrinsic: TokenIntrinsic,
        args: &TokenStream,
        at: Span,
        site: &CallSite,
    ) -> Vec<Token> {
        let span = self.mark(site.span);
        match intrinsic {
            TokenIntrinsic::Stringify => {
                let text = self.stringify(&args.tokens, site);
                vec![self.string_token(&text, span)]
            }
            TokenIntrinsic::Location => {
                let text = format!("{}:{}", site.line, site.column);
                vec![self.string_token(&text, span)]
            }
            TokenIntrinsic::Concat => {
                let mut text = String::new();
                for part in split_args(&args.tokens) {
                    match part {
                        [tok] => text.push_str(&self.literal_text(tok)),
                        [] => {}
                        many => text.push_str(&self.stringify(many, site)),
                    }
                }
                vec![self.string_token(&text, span)]
            }
            TokenIntrinsic::Upper | TokenIntrinsic::Lower => {
                let [tok] = args.tokens.as_slice() else {
                    self.intrinsic_error(at, intrinsic, "expects exactly one token");
                    return Vec::new();
                };
                let text = self.literal_text(tok);
                let mapped = if intrinsic == TokenIntrinsic::Upper {
                    text.to_uppercase()
                } else {
                    text.to_lowercase()
                };
                if tok.kind == TokenKind::String {
                    vec![self.string_token(&mapped, tok.span)]
                } else {
                    let sym = self.interner.intern(&mapped);
                    vec![Token::with_sym(TokenKind::Ident, tok.span, sym)]
                }
            }
            TokenIntrinsic::Error => {
                let parts = split_args(&args.tokens);
                let (anchor, message) = match parts.as_slice() {
                    [message] => (None, *message),
                    [anchor, message] => (Some(*anchor), *message),
                    _ => {
                        self.intrinsic_error(at, intrinsic, "expects `([anchor,] message)`");
                        return Vec::new();
                    }
                };
                let message = match message {
                    [tok] if tok.kind == TokenKind::String => self.literal_text(tok),
                    _ => {
                        self.intrinsic_error(at, intrinsic, "message must be a string literal");
                        return Vec::new();
                    }
                };
                let span = anchor.and_then(caller_span).unwrap_or(site.span);
                self.diagnostics
                    .error(DiagnosticKind::Syntax, span, message);
                Vec::new()
            }
            TokenIntrinsic::EnsureDistinct => {
                let parts = split_args(&args.tokens);
                let Some((message, groups)) = parts.split_first() else {
                    self.intrinsic_error(at, intrinsic, "expects `(message, (key…) …)`");
                    return Vec::new();
                };
                let message = match message {
                    [tok] if tok.kind == TokenKind::String => self.literal_text(tok),
                    _ => {
                        self.intrinsic_error(at, intrinsic, "message must be a string literal");
                        return Vec::new();
                    }
                };
                let mut seen: Vec<String> = Vec::new();
                for group in groups.iter().flat_map(|part| top_level_groups(part)) {
                    let key = group
                        .iter()
                        .map(|tok| self.literal_text(tok))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if seen.contains(&key) {
                        let span = caller_span(group).unwrap_or(site.span);
                        self.diagnostics.error(
                            DiagnosticKind::Syntax,
                            span,
                            message.replacen("{}", &key, 1),
                        );
                    } else {
                        seen.push(key);
                    }
                }
                Vec::new()
            }
        }
    }

    fn intrinsic_error(&mut self, span: Span, intrinsic: TokenIntrinsic, message: &str) {
        self.diagnostics.error(
            DiagnosticKind::Syntax,
            span,
            format!("`#{}` {message}", intrinsic.name()),
        );
    }

    /// Caller source text for `tokens` when they come from the call site, else the
    /// tokens re-rendered.
    ///
    /// Template tokens before the first or after the last caller token (a closing
    /// `)` the pattern matched literally, say) are rendered around the slice.
    fn stringify(&self, tokens: &[Token], site: &CallSite) -> String {
        let caller = |tok: &Token| site.contains(tok.span);
        let (Some(first), Some(last)) = (
            tokens.iter().position(caller),
            tokens.iter().rposition(caller),
        ) else {
            return self.render(tokens);
        };
        let inner = &tokens[first..=last];
        let lo = inner
            .iter()
            .filter(|t| caller(t))
            .map(|t| t.span.lo.0)
            .min();
        let hi = inner
            .iter()
            .filter(|t| caller(t))
            .map(|t| t.span.hi.0)
            .max();
        let Some(text) = lo.zip(hi).and_then(|(lo, hi)| site.slice(lo, hi)) else {
            return self.render(tokens);
        };
        let mut out = self.render(&tokens[..first]);
        if first > 0 && needs_space(tokens[first - 1].kind, tokens[first].kind) {
            out.push(' ');
        }
        out.push_str(text.trim());
        if let Some(next) = tokens.get(last + 1) {
            if needs_space(tokens[last].kind, next.kind) {
                out.push(' ');
            }
            out.push_str(&self.render(&tokens[last + 1..]));
        }
        out
    }

    /// Text of a literal-like token: string contents unescaped, words verbatim.
    fn literal_text(&self, tok: &Token) -> String {
        match tok.kind {
            TokenKind::String => unescape(self.interner.resolve(tok.sym.expect("string symbol"))),
            _ => self.token_text(tok),
        }
    }

    fn string_token(&mut self, text: &str, span: Span) -> Token {
        let sym = self.interner.intern(&escape(text));
        Token::with_sym(TokenKind::String, span, sym)
    }

    fn token_text(&self, tok: &Token) -> String {
        match tok.kind {
            TokenKind::Ident | TokenKind::Int | TokenKind::Float => tok
                .sym
                .map(|sym| self.interner.resolve(sym).to_owned())
                .unwrap_or_default(),
            TokenKind::String => format!(
                "\"{}\"",
                tok.sym.map(|sym| self.interner.resolve(sym)).unwrap_or("")
            ),
            kind => punct_text(kind)
                .or_else(|| keyword_text(kind))
                .unwrap_or("?")
                .to_owned(),
        }
    }

    /// Tokens joined with conventional spacing.
    fn render(&self, tokens: &[Token]) -> String {
        let mut out = String::new();
        let mut prev: Option<TokenKind> = None;
        for tok in tokens {
            if let Some(prev) = prev
                && needs_space(prev, tok.kind)
            {
                out.push(' ');
            }
            out.push_str(&self.token_text(tok));
            prev = Some(tok.kind);
        }
        out
    }

    // ------------------------------------------------------------------ expansion

    fn expand_call(&mut self, call: &MacroCall) -> Expansion {
        let name = self.interner.resolve(call.name).to_owned();
        if let Some(intrinsic) = TokenIntrinsic::from_name(&name) {
            let site = CallSite::of(call, self.interner);
            let input = self.eval_intrinsics(call.input.clone(), &site);
            let tokens = self.apply_intrinsic(intrinsic, &input, call.span, &site);
            let stream = TokenStream {
                line_starts: vec![false; tokens.len()],
                tokens,
            };
            return self.reparse(stream, &site);
        }
        if name == GOAL_TYPE && call.delim == MacroDelim::Bare {
            return Expansion::Expr(Expr::GoalType(call.span));
        }
        let Some(mac) = self.macros.get(&call.name).cloned() else {
            return self.not_a_macro(call, &name);
        };
        if self.depth >= MAX_DEPTH {
            self.diagnostics.error(
                DiagnosticKind::Syntax,
                call.span,
                format!("macro `#{name}` expands recursively more than {MAX_DEPTH} levels deep"),
            );
            return Expansion::Expr(Expr::None(call.span));
        }
        let site = CallSite::of(call, self.interner);
        for rule in &mac.rules {
            let mut bindings = Bindings::new();
            let mut pos = 0;
            if !self.match_nodes(&rule.pattern, &call.input, &mut pos, &mut bindings)
                || pos != call.input.len()
            {
                continue;
            }
            let frame: HashMap<SymId, &Bound> = bindings.iter().map(|(k, v)| (*k, v)).collect();
            let mut gensyms = HashMap::new();
            let mut out = TokenStream::default();
            if let Err(message) =
                self.transcribe(&rule.template, &frame, &site, &mut gensyms, &mut out)
            {
                self.diagnostics.error(
                    DiagnosticKind::Syntax,
                    call.span,
                    format!("in expansion of `#{name}`: {message}"),
                );
                return Expansion::Expr(Expr::None(call.span));
            }
            let out = self.eval_intrinsics(out, &site);
            return self.reparse(out, &site);
        }
        let shown = mac
            .rules
            .iter()
            .map(|rule| format!("`#{name}({})`", rule.shown))
            .collect::<Vec<_>>()
            .join(", ");
        let input = self.render(&call.input.tokens);
        self.diagnostics.error_help(
            DiagnosticKind::Syntax,
            call.span,
            format!("no rule of macro `#{name}` matches `{input}`"),
            format!("expected one of: {shown}"),
        );
        Expansion::Expr(Expr::None(call.span))
    }

    fn not_a_macro(&mut self, call: &MacroCall, name: &str) -> Expansion {
        let callee = Expr::ComptimeRef {
            name: call.name,
            span: call.span,
        };
        match call.delim {
            MacroDelim::Bare => Expansion::Expr(callee),
            MacroDelim::Paren => {
                let site = CallSite::of(call, self.interner);
                let window = (site.text.as_str(), site.span.lo.0, site.span.file_index());
                let mut parser = Parser::for_expansion(
                    &call.input,
                    window,
                    (call.line, call.column),
                    self.interner,
                    self.diagnostics,
                );
                let mut args = Vec::new();
                while !parser.at_end() {
                    let before = parser.position();
                    args.push(parser.parse_expression());
                    if !parser.eat_comma() && parser.position() == before {
                        break;
                    }
                }
                Expansion::Expr(Expr::Call {
                    callee: Box::new(callee),
                    args,
                    punning: false,
                    span: call.span,
                })
            }
            MacroDelim::Brace => {
                self.diagnostics.error_help(
                    DiagnosticKind::Syntax,
                    call.span,
                    format!("unknown macro `#{name}`"),
                    "define it with `+#name { (pattern) => { template } }` or `use` the module that exports it",
                );
                Expansion::Expr(Expr::None(call.span))
            }
        }
    }

    fn reparse(&mut self, tokens: TokenStream, site: &CallSite) -> Expansion {
        if tokens.is_empty() {
            return Expansion::Empty;
        }
        let window = (site.text.as_str(), site.span.lo.0, site.span.file_index());
        let mut scratch = Diagnostics::new();
        let mut stmts = {
            let mut parser = Parser::for_expansion(
                &tokens,
                window,
                (site.line, site.column),
                self.interner,
                &mut scratch,
            );
            parser.parse_statements()
        };
        if !scratch.is_empty() {
            let name = self.interner.resolve(site.name).to_owned();
            for diagnostic in scratch.into_vec() {
                self.diagnostics.push(Diagnostic {
                    message: format!("in expansion of `#{name}`: {}", diagnostic.message),
                    ..diagnostic
                });
            }
        }
        match stmts.len() {
            0 => Expansion::Empty,
            1 => match stmts.pop().expect("one statement") {
                Stmt::Expr(expr) => Expansion::Expr(expr),
                Stmt::Panic(message, span) => Expansion::Expr(Expr::Panic {
                    message: message.map(Box::new),
                    span,
                }),
                stmt => Expansion::Stmts(vec![stmt]),
            },
            _ => Expansion::Stmts(stmts),
        }
    }

    /// The expansion of `expr` if it is a macro call; `None` leaves it as is.
    fn take_call(&mut self, expr: &Expr) -> Option<Expansion> {
        match expr {
            Expr::MacroCall(call) => Some(self.expand_call(call)),
            _ => None,
        }
    }

    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.depth += 1;
        let out = f(self);
        self.depth -= 1;
        out
    }

    // -------------------------------------------------------------------- walking

    fn expand_item(&mut self, item: &mut Item) {
        match item {
            Item::Use(_) | Item::Macro(_) => {}
            Item::Binding(binding) => self.expand_binding(binding),
            Item::Struct(def) => {
                for field in &mut def.fields {
                    if let Some(default) = &mut field.default {
                        self.expand_expr(default);
                    }
                }
                for method in &mut def.methods {
                    self.expand_function(method);
                }
            }
            Item::Sum(def) => {
                for method in &mut def.methods {
                    self.expand_function(method);
                }
            }
            Item::Function(function) => self.expand_function(function),
            Item::State(state) => self.expand_expr(&mut state.value),
            Item::Test(test) => self.expand_block(&mut test.body),
            Item::Rewrite(rule) => {
                self.expand_expr(&mut rule.lhs);
                self.expand_expr(&mut rule.rhs);
            }
            Item::Trait(_) => {}
        }
    }

    fn expand_function(&mut self, function: &mut Function) {
        self.expand_fn_body(&mut function.body);
    }

    fn expand_fn_body(&mut self, body: &mut FnBody) {
        match body {
            FnBody::Expr(expr) => self.expand_expr(expr),
            FnBody::Block(block) => self.expand_block(block),
        }
    }

    fn expand_binding(&mut self, binding: &mut Binding) {
        self.expand_expr(&mut binding.value);
    }

    fn expand_block(&mut self, block: &mut Block) {
        let stmts = std::mem::take(&mut block.stmts);
        let mut out = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            self.expand_stmt_into(stmt, &mut out);
        }
        block.stmts = out;
    }

    /// Expand `stmt`, splicing a statement-position macro's statements into `out`.
    fn expand_stmt_into(&mut self, mut stmt: Stmt, out: &mut Vec<Stmt>) {
        let expansion = match &stmt {
            Stmt::Expr(expr) => self.take_call(expr),
            _ => None,
        };
        match expansion {
            None => {
                self.expand_stmt(&mut stmt);
                out.push(stmt);
            }
            Some(Expansion::Empty) => {}
            Some(Expansion::Expr(mut expr)) => {
                self.nested(|this| this.expand_expr(&mut expr));
                out.push(Stmt::Expr(expr));
            }
            Some(Expansion::Stmts(stmts)) => self.nested(|this| {
                for stmt in stmts {
                    this.expand_stmt_into(stmt, out);
                }
            }),
        }
    }

    fn expand_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Binding(binding) => self.expand_binding(binding),
            Stmt::Destructure { value, .. } => self.expand_expr(value),
            Stmt::Expr(expr) | Stmt::Fail(expr, _) => self.expand_expr(expr),
            Stmt::Return(value, _) | Stmt::Panic(value, _) => {
                if let Some(value) = value {
                    self.expand_expr(value);
                }
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                self.expand_expr(cond);
                self.expand_block(else_block);
            }
            Stmt::Each { iter, body, .. } => {
                self.expand_expr(iter);
                self.expand_block(body);
            }
            Stmt::While { cond, body, .. } => {
                self.expand_expr(cond);
                self.expand_block(body);
            }
            Stmt::Loop { body, .. } | Stmt::Scope { body, .. } => self.expand_block(body),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::MultiAssign {
                targets, values, ..
            } => {
                for expr in targets.iter_mut().chain(values.iter_mut()) {
                    self.expand_expr(expr);
                }
            }
            Stmt::Guarded { stmt, cond, .. } => {
                self.expand_stmt(stmt);
                self.expand_expr(cond);
            }
            Stmt::When { subjects, arms, .. } => {
                for subject in subjects {
                    self.expand_expr(subject);
                }
                for arm in arms {
                    self.expand_pattern(&mut arm.pattern);
                    self.expand_expr(&mut arm.body);
                }
            }
            Stmt::Comptime(comptime) => match comptime {
                ComptimeStmt::Block(block, _) => self.expand_block(block),
                ComptimeStmt::If {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => {
                    self.expand_expr(cond);
                    self.expand_block(then_block);
                    if let Some(block) = else_block {
                        self.expand_block(block);
                    }
                }
                ComptimeStmt::Each { iter, body, .. } => {
                    self.expand_expr(iter);
                    self.expand_block(body);
                }
            },
        }
    }

    fn expand_pattern(&mut self, pattern: &mut WhenPattern) {
        match pattern {
            WhenPattern::Expr(expr) | WhenPattern::After(expr) => self.expand_expr(expr),
            WhenPattern::Tuple { elems, .. } => {
                for elem in elems {
                    self.expand_pattern(elem);
                }
            }
            WhenPattern::Else | WhenPattern::VariantBind { .. } => {}
        }
    }

    fn expand_expr(&mut self, expr: &mut Expr) {
        if let Some(expansion) = self.take_call(expr) {
            let span = expr.span();
            *expr = match expansion {
                Expansion::Empty => Expr::None(span),
                Expansion::Expr(inner) => inner,
                Expansion::Stmts(stmts) => Expr::Block(Block { stmts, span }),
            };
            self.nested(|this| this.expand_expr(expr));
            return;
        }
        match expr {
            Expr::Ident(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::Bool(..)
            | Expr::String(..)
            | Expr::None(_)
            | Expr::Embed { .. }
            | Expr::It(_)
            | Expr::IndexIdent(_)
            | Expr::ErrIdent(_)
            | Expr::ComptimeRef { .. }
            | Expr::GoalType(_)
            | Expr::MacroCall(_) => {}
            Expr::Binary { left, right, .. } | Expr::Or { left, right, .. } => {
                self.expand_expr(left);
                self.expand_expr(right);
            }
            Expr::Assign { target, value, .. } => {
                self.expand_expr(target);
                self.expand_expr(value);
            }
            Expr::Unary { expr: inner, .. }
            | Expr::Await(inner, _)
            | Expr::Spawn { body: inner, .. }
            | Expr::Shared { value: inner, .. }
            | Expr::Duration { value: inner, .. }
            | Expr::Quote { body: inner, .. }
            | Expr::Splice { code: inner, .. }
            | Expr::Field { base: inner, .. }
            | Expr::Chan {
                capacity: inner, ..
            } => self.expand_expr(inner),
            Expr::Call { callee, args, .. } => {
                self.expand_expr(callee);
                for arg in args {
                    self.expand_expr(arg);
                }
            }
            Expr::Pipeline { base, args, .. } => {
                self.expand_expr(base);
                for arg in args {
                    self.expand_expr(arg);
                }
            }
            Expr::NamedVariant { args, .. } | Expr::Array(args, _) => {
                for arg in args {
                    self.expand_expr(arg);
                }
            }
            Expr::Slice {
                base, start, end, ..
            } => {
                self.expand_expr(base);
                self.expand_expr(start);
                self.expand_expr(end);
            }
            Expr::Index { base, index, .. } => {
                self.expand_expr(base);
                self.expand_expr(index);
            }
            Expr::ComptimeField { base, field, .. } => {
                self.expand_expr(base);
                self.expand_expr(field);
            }
            Expr::ArrayRepeat { value, len, .. } => {
                self.expand_expr(value);
                self.expand_expr(len);
            }
            Expr::Range { start, end, .. } => {
                self.expand_expr(start);
                self.expand_expr(end);
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.expand_expr(cond);
                self.expand_expr(then_expr);
                self.expand_expr(else_expr);
            }
            Expr::Map(entries, _) => {
                for entry in entries {
                    if let MapKey::Expr(key) = &mut entry.key {
                        self.expand_expr(key);
                    }
                    self.expand_expr(&mut entry.value);
                }
            }
            Expr::Record(fields, _)
            | Expr::StructLit { fields, .. }
            | Expr::InferredStruct { fields, .. } => {
                for field in fields {
                    self.expand_expr(&mut field.value);
                }
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            }
            | Expr::ComptimeIf {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.expand_expr(cond);
                self.expand_block(then_block);
                if let Some(block) = else_block {
                    self.expand_block(block);
                }
            }
            Expr::Block(block) | Expr::ComptimeBlock { body: block, .. } => {
                self.expand_block(block)
            }
            Expr::OrErr { left, body, .. } => {
                self.expand_expr(left);
                self.expand_block(body);
            }
            Expr::ComptimeEach { iter, body, .. } => {
                self.expand_expr(iter);
                self.expand_block(body);
            }
            Expr::Lambda { body, .. } => self.expand_fn_body(body),
            Expr::Panic { message, .. } => {
                if let Some(message) = message {
                    self.expand_expr(message);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ token helpers

fn same_token(a: &Token, b: &Token) -> bool {
    a.kind == b.kind
        && (!matches!(
            a.kind,
            TokenKind::Ident | TokenKind::Int | TokenKind::Float | TokenKind::String
        ) || a.sym == b.sym)
}

/// Words usable as identifiers in fragments (`ident`, `path`).
fn is_word(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident
            | TokenKind::State
            | TokenKind::Test
            | TokenKind::Use
            | TokenKind::Type
            | TokenKind::Code
            | TokenKind::Trait
            | TokenKind::After
    )
}

fn path_end(tokens: &[Token], pos: usize) -> Option<usize> {
    if !is_word(tokens.get(pos)?.kind) {
        return None;
    }
    let mut end = pos + 1;
    while tokens.get(end).map(|t| t.kind) == Some(TokenKind::Dot)
        && tokens.get(end + 1).is_some_and(|t| is_word(t.kind))
    {
        end += 2;
    }
    Some(end)
}

fn literal_end(tokens: &[Token], pos: usize) -> Option<usize> {
    match tokens.get(pos)?.kind {
        TokenKind::Int
        | TokenKind::Float
        | TokenKind::String
        | TokenKind::True
        | TokenKind::False
        | TokenKind::None => Some(pos + 1),
        TokenKind::Minus => matches!(
            tokens.get(pos + 1).map(|t| t.kind),
            Some(TokenKind::Int | TokenKind::Float)
        )
        .then_some(pos + 2),
        _ => None,
    }
}

/// Index of the bracket closing the one at `open`.
fn matching_close(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, tok) in tokens.iter().enumerate().skip(open) {
        match tok.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sep? op` after a `$( … )` group: separator, operator, next index.
fn repetition_suffix(tokens: &[Token], at: usize) -> Option<(Option<Token>, RepOp, usize)> {
    let first = tokens.get(at)?;
    if let Some(op) = RepOp::of(first.kind) {
        return Some((None, op, at + 1));
    }
    let op = RepOp::of(tokens.get(at + 1)?.kind)?;
    Some((Some(*first), op, at + 2))
}

fn pattern_names(nodes: &[PatNode], out: &mut Vec<SymId>) {
    for node in nodes {
        match node {
            PatNode::Tok(_) => {}
            PatNode::Frag { name, .. } => out.push(*name),
            PatNode::Rep { body, .. } => pattern_names(body, out),
        }
    }
}

fn template_vars(nodes: &[TplNode], out: &mut Vec<SymId>) {
    for node in nodes {
        match node {
            TplNode::Tok { .. } => {}
            TplNode::Var { name, .. } => {
                if !out.contains(name) {
                    out.push(*name);
                }
            }
            TplNode::Rep { body, .. } => template_vars(body, out),
        }
    }
}

fn push_capture(capture: &Capture, line_start: bool, out: &mut TokenStream) {
    let (Some(first), Some(last)) = (capture.tokens.first(), capture.tokens.last()) else {
        return;
    };
    let grouped = capture.kind.is_grouped() && capture.tokens.len() > 1;
    if grouped {
        let at = Span::empty(first.span.lo).with_file(first.span.file);
        out.push(Token::new(TokenKind::LParen, at), line_start);
    }
    for (index, tok) in capture.tokens.iter().enumerate() {
        let flag = if index == 0 {
            line_start && !grouped
        } else {
            capture.line_starts[index]
        };
        out.push(*tok, flag);
    }
    if grouped {
        let at = Span::empty(BytePos(last.span.hi.0)).with_file(last.span.file);
        out.push(Token::new(TokenKind::RParen, at), false);
    }
}

/// Split at top-level commas.
fn split_args(tokens: &[Token]) -> Vec<&[Token]> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, tok) in tokens.iter().enumerate() {
        match tok.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1)
            }
            TokenKind::Comma if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&tokens[start..]);
    parts
}

/// Contents of each top-level `( … )` group in `tokens`.
fn top_level_groups(tokens: &[Token]) -> Vec<&[Token]> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].kind == TokenKind::LParen
            && let Some(close) = matching_close(tokens, index)
        {
            groups.push(&tokens[index + 1..close]);
            index = close + 1;
            continue;
        }
        index += 1;
    }
    groups
}

/// Span covering the caller-written tokens of `tokens`.
fn caller_span(tokens: &[Token]) -> Option<Span> {
    let mut inside = tokens
        .iter()
        .map(|tok| tok.span)
        .filter(|span| span.expansion_mark() == 0 && span.lo != span.hi);
    let first = inside.next()?;
    Some(inside.fold(first, Span::merge))
}

fn needs_space(prev: TokenKind, next: TokenKind) -> bool {
    use TokenKind as K;
    if matches!(
        prev,
        K::LParen | K::LBracket | K::Dot | K::At | K::Hash | K::Dollar | K::Bang
    ) {
        return false;
    }
    if matches!(
        next,
        K::RParen | K::RBracket | K::Comma | K::Dot | K::Colon | K::Semicolon
    ) {
        return false;
    }
    if matches!(next, K::LParen | K::LBracket)
        && matches!(prev, K::Ident | K::RParen | K::RBracket | K::String)
    {
        return false;
    }
    !(prev == K::DotDot || next == K::DotDot || next == K::Question)
}

fn punct_text(kind: TokenKind) -> Option<&'static str> {
    use TokenKind as K;
    Some(match kind {
        K::Plus => "+",
        K::Minus => "-",
        K::Star => "*",
        K::Slash => "/",
        K::Percent => "%",
        K::Eq => "=",
        K::EqEq => "==",
        K::NotEq => "!=",
        K::Lt => "<",
        K::LtEq => "<=",
        K::Gt => ">",
        K::GtEq => ">=",
        K::PlusEq => "+=",
        K::MinusEq => "-=",
        K::StarEq => "*=",
        K::SlashEq => "/=",
        K::Dot => ".",
        K::DotDot => "..",
        K::Comma => ",",
        K::Semicolon => ";",
        K::Colon => ":",
        K::Question => "?",
        K::Bang => "!",
        K::At => "@",
        K::Hash => "#",
        K::Dollar => "$",
        K::QuoteStart => "'{",
        K::SpliceStart => "${",
        K::FatArrow => "=>",
        K::Pipe => "|",
        K::Amp => "&",
        K::Caret => "^",
        K::Tilde => "~",
        K::AmpAmp => "&&",
        K::PipePipe => "||",
        K::LessLess => "<<",
        K::GreaterGreater => ">>",
        K::GreaterGreaterGreater => ">>>",
        K::LParen => "(",
        K::RParen => ")",
        K::LBrace => "{",
        K::RBrace => "}",
        K::LBracket => "[",
        K::RBracket => "]",
        _ => return None,
    })
}

fn keyword_text(kind: TokenKind) -> Option<&'static str> {
    RESERVED_KEYWORDS
        .iter()
        .copied()
        .find(|word| TokenKind::from_ident(word) == kind)
}

/// Encode text as string-token contents (the lexer's escaped form).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out
}

/// Decode string-token contents. Interpolation holes stay as written.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('u') => {
                let hex: String = chars
                    .by_ref()
                    .skip_while(|c| *c == '{')
                    .take_while(|c| *c != '}')
                    .collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests;
