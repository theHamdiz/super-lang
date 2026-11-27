//! `super lint`: named, stable rules over the parsed (unexpanded) module.
//!
//! Rules work on syntax and tokens only, so linting never needs imports to
//! resolve. Every rule has a stable id (`S001`…), a name, and a default
//! severity; two are mechanically fixable. Rules and their rationale are
//! documented in `docs/lint.md`.

use crate::Lexer;
use crate::ast::{
    Block, Expr, FnBody, Function, Item, MacroCall, Module, Param, RecordField, Stmt, UnOp,
    WhenPattern,
};
use crate::token::{Token, TokenKind};
use crate::{Parser, macros};
use std::collections::{HashMap, HashSet};
use super_core::{BytePos, Diagnostics, Interner, SourceFile, Span, SymId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    Naming,
    Stutter,
    Unused,
    Shadowing,
    MissingLaw,
    NeedlessMut,
    RedundantPlus,
    RedundantPun,
}

impl Rule {
    pub const ALL: [Rule; 8] = [
        Rule::Naming,
        Rule::Stutter,
        Rule::Unused,
        Rule::Shadowing,
        Rule::MissingLaw,
        Rule::NeedlessMut,
        Rule::RedundantPlus,
        Rule::RedundantPun,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Rule::Naming => "S001",
            Rule::Stutter => "S002",
            Rule::Unused => "S003",
            Rule::Shadowing => "S004",
            Rule::MissingLaw => "S005",
            Rule::NeedlessMut => "S006",
            Rule::RedundantPlus => "S007",
            Rule::RedundantPun => "S008",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Rule::Naming => "naming",
            Rule::Stutter => "stutter",
            Rule::Unused => "unused",
            Rule::Shadowing => "shadowing",
            Rule::MissingLaw => "missing-law",
            Rule::NeedlessMut => "needless-mut",
            Rule::RedundantPlus => "redundant-plus",
            Rule::RedundantPun => "redundant-pun",
        }
    }

    pub fn default_severity(self) -> Severity {
        match self {
            Rule::MissingLaw => Severity::Info,
            _ => Severity::Warning,
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Rule::Naming => {
                "types and variants are UpperCamelCase; functions, fields, and params are snake_case"
            }
            Rule::Stutter => "an exported name repeats its module name",
            Rule::Unused => "a private item is never referenced in its module",
            Rule::Shadowing => {
                "a closure parameter or loop/pattern binding hides an enclosing local"
            }
            Rule::MissingLaw => "a public stdlib module declares no `#law`",
            Rule::NeedlessMut => "an `@` parameter or receiver is never mutated",
            Rule::RedundantPlus => {
                "`+` on a struct member is redundant (members are public by default)"
            }
            Rule::RedundantPun => {
                "`name: name` in a struct or record literal can be punned to `name`"
            }
        }
    }

    /// Accepts `S003` or `unused`.
    pub fn parse(text: &str) -> Option<Rule> {
        Rule::ALL
            .into_iter()
            .find(|rule| rule.id().eq_ignore_ascii_case(text) || rule.name() == text)
    }
}

/// A mechanical edit: replace `source[start..end]` with `replacement`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    pub severity: Severity,
    pub line: u32,
    pub column: u32,
    pub message: String,
    /// The named item, for exceptions that target one item.
    pub item: Option<String>,
    pub fix: Option<Fix>,
}

/// One listed exception (`[[lint.exception]]` in `Pod.toml`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exception {
    pub rule: String,
    /// Path suffix the exception applies to (`/`-separated).
    pub path: String,
    /// Restrict to one item name.
    pub item: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintConfig {
    /// Rules switched off (ids or names).
    pub allow: Vec<String>,
    /// Rules promoted to errors (ids or names).
    pub deny: Vec<String>,
    pub exceptions: Vec<Exception>,
}

impl LintConfig {
    pub fn validate(&self) -> Result<(), String> {
        for name in self
            .allow
            .iter()
            .chain(&self.deny)
            .chain(self.exceptions.iter().map(|e| &e.rule))
        {
            if Rule::parse(name).is_none() {
                return Err(format!("unknown lint rule `{name}`"));
            }
        }
        if let Some(bad) = self.exceptions.iter().find(|e| e.reason.trim().is_empty()) {
            return Err(format!(
                "lint exception for `{}` in `{}` needs a reason",
                bad.rule, bad.path
            ));
        }
        Ok(())
    }

    fn lists(list: &[String], rule: Rule) -> bool {
        list.iter().any(|name| Rule::parse(name) == Some(rule))
    }

    /// Apply allow/deny/exceptions to the raw findings for `path`.
    pub fn apply(&self, path: &str, findings: Vec<Finding>) -> Vec<Finding> {
        let path = path.replace('\\', "/");
        findings
            .into_iter()
            .filter(|f| !Self::lists(&self.allow, f.rule))
            .filter(|f| {
                !self.exceptions.iter().any(|e| {
                    Rule::parse(&e.rule) == Some(f.rule)
                        && path_matches(&path, &e.path)
                        && e.item
                            .as_ref()
                            .is_none_or(|item| f.item.as_ref() == Some(item))
                })
            })
            .map(|mut f| {
                if Self::lists(&self.deny, f.rule) {
                    f.severity = Severity::Error;
                }
                f
            })
            .collect()
    }
}

fn path_matches(path: &str, suffix: &str) -> bool {
    let suffix = suffix.trim_start_matches("./");
    path == suffix || path.ends_with(&format!("/{suffix}"))
}

/// What the linter knows about the file beyond its text.
#[derive(Clone, Debug, Default)]
pub struct LintContext {
    /// File stem used by `stutter` (`http` for `http.sr`).
    pub module_name: String,
    /// The file is part of the standard library (`missing-law` applies).
    pub stdlib: bool,
    /// Companion sources (`x.law.sr`, `x.proof.sr`) that may reference items.
    pub companions: Vec<String>,
}

impl LintContext {
    /// Context for a file at `path` (companions are read by the caller).
    pub fn for_path(path: &str) -> Self {
        let normalized = path.replace('\\', "/");
        let file = normalized.rsplit('/').next().unwrap_or(&normalized);
        let module_name = file.split('.').next().unwrap_or_default().to_owned();
        LintContext {
            module_name,
            stdlib: normalized.starts_with("stdlib/") || normalized.contains("/stdlib/"),
            companions: Vec::new(),
        }
    }
}

/// Lint one source file. Returns `Err` when the file does not parse.
pub fn lint_source(source: &str, ctx: &LintContext) -> Result<Vec<Finding>, String> {
    let mut interner = Interner::with_capacity(512);
    let mut diagnostics = Diagnostics::new();
    let tokens = Lexer::new(source, &mut interner, &mut diagnostics).tokenize();
    let module = Parser::new(&tokens, source, &mut interner, &mut diagnostics).parse_module();
    if diagnostics.has_errors() {
        let first = diagnostics
            .into_vec()
            .into_iter()
            .next()
            .map(|d| d.message)
            .unwrap_or_default();
        return Err(first);
    }
    let mut companion_tokens = Vec::new();
    let mut companion_laws = false;
    for companion in &ctx.companions {
        let mut sink = Diagnostics::new();
        let toks = Lexer::new(companion, &mut interner, &mut sink).tokenize();
        let mut parsed = Parser::new(&toks, companion, &mut interner, &mut sink).parse_module();
        macros::expand_module(&mut parsed, &mut interner, &mut sink);
        companion_laws |= parsed
            .items
            .iter()
            .any(|item| matches!(item, Item::Function(f) if f.is_law()));
        companion_tokens.extend(toks);
    }
    let file = SourceFile::new("lint", source);
    let mut linter = Linter {
        source,
        file: &file,
        interner: &interner,
        findings: Vec::new(),
    };
    linter.naming(&module);
    linter.stutter(&module, ctx);
    linter.unused(&module, &tokens, &companion_tokens);
    linter.bodies(&module);
    if ctx.stdlib {
        linter.missing_law(&module, companion_laws, &tokens);
    }
    linter.redundant_plus(&module);
    let mut findings = linter.findings;
    findings.sort_by_key(|f| (f.line, f.column, f.rule));
    findings.dedup();
    Ok(findings)
}

/// Apply every non-overlapping fix in `findings` to `source`.
pub fn apply_fixes(source: &str, findings: &[Finding]) -> String {
    let mut fixes: Vec<&Fix> = findings.iter().filter_map(|f| f.fix.as_ref()).collect();
    fixes.sort_by_key(|fix| fix.start);
    let mut out = String::with_capacity(source.len());
    let mut at = 0usize;
    for fix in fixes {
        if fix.start < at {
            continue;
        }
        out.push_str(&source[at..fix.start]);
        out.push_str(&fix.replacement);
        at = fix.end;
    }
    out.push_str(&source[at..]);
    out
}

struct Linter<'a> {
    source: &'a str,
    file: &'a SourceFile,
    interner: &'a Interner,
    findings: Vec<Finding>,
}

fn is_snake(name: &str) -> bool {
    let name = name.trim_start_matches('_');
    name.is_empty()
        || (name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

fn is_camel(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_screaming(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn camel(stem: &str) -> String {
    stem.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Mutating builtin methods on arrays, maps, and strings.
const MUTATING_METHODS: &[&str] = &[
    "push", "pop", "insert", "remove", "clear", "set", "put", "extend", "append", "sort",
    "reverse", "swap", "truncate", "fill", "retain", "drain", "shift", "unshift",
];

impl<'a> Linter<'a> {
    fn name(&self, sym: SymId) -> &'a str {
        self.interner.resolve(sym)
    }

    fn report(
        &mut self,
        rule: Rule,
        span: Span,
        item: Option<&str>,
        message: String,
        fix: Option<Fix>,
    ) {
        if span.expansion_mark() != 0 {
            return;
        }
        let (line, column) = self.file.line_col(span.lo);
        self.findings.push(Finding {
            rule,
            severity: rule.default_severity(),
            line,
            column,
            message,
            item: item.map(str::to_owned),
            fix,
        });
    }

    fn check_name(&mut self, sym: SymId, span: Span, what: &str, ok: fn(&str) -> bool, want: &str) {
        let name = self.name(sym);
        if !ok(name) {
            self.report(
                Rule::Naming,
                span,
                Some(name),
                format!("{what} `{name}` should be {want}"),
                None,
            );
        }
    }

    fn check_function_names(&mut self, f: &Function) {
        self.check_name(f.name, f.span, "function", is_snake, "snake_case");
        for param in &f.params {
            self.check_name(param.name, param.span, "parameter", is_snake, "snake_case");
        }
    }

    // S001
    fn naming(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Function(f) => self.check_function_names(f),
                Item::Binding(b) => {
                    let name = self.name(b.name);
                    if !(is_snake(name) || is_screaming(name) || is_camel(name)) {
                        self.report(
                            Rule::Naming,
                            b.span,
                            Some(name),
                            format!("binding `{name}` should be snake_case, UPPER_SNAKE, or UpperCamelCase"),
                            None,
                        );
                    }
                }
                Item::Struct(s) => {
                    self.check_name(s.name, s.span, "type", is_camel, "UpperCamelCase");
                    for field in &s.fields {
                        self.check_name(field.name, field.span, "field", is_snake, "snake_case");
                    }
                    for method in &s.methods {
                        self.check_function_names(method);
                    }
                }
                Item::Sum(s) => {
                    self.check_name(s.name, s.span, "type", is_camel, "UpperCamelCase");
                    for variant in &s.variants {
                        self.check_name(
                            variant.name,
                            variant.span,
                            "variant",
                            is_camel,
                            "UpperCamelCase",
                        );
                    }
                    for method in &s.methods {
                        self.check_function_names(method);
                    }
                }
                Item::Trait(t) => {
                    self.check_name(t.name, t.span, "trait", is_camel, "UpperCamelCase")
                }
                Item::Macro(m) => self.check_name(m.name, m.span, "macro", is_snake, "snake_case"),
                Item::State(s) => self.check_name(s.name, s.span, "state", is_snake, "snake_case"),
                Item::Use(_) | Item::Test(_) | Item::Rewrite(_) => {}
            }
        }
    }

    // S002
    fn stutter(&mut self, module: &Module, ctx: &LintContext) {
        let stem = ctx.module_name.as_str();
        if stem.is_empty() || matches!(stem, "main" | "mod" | "lib" | "macros") {
            return;
        }
        let snake_prefix = format!("{stem}_");
        let camel_prefix = camel(stem);
        for item in &module.items {
            let (sym, span, is_type) = match item {
                Item::Function(f) if f.exported => (f.name, f.span, false),
                Item::Struct(s) if s.exported => (s.name, s.span, true),
                Item::Sum(s) if s.exported => (s.name, s.span, true),
                Item::Trait(t) if t.exported => (t.name, t.span, true),
                _ => continue,
            };
            let name = self.name(sym);
            let stutters = if is_type {
                name.len() > camel_prefix.len() && name.starts_with(&camel_prefix)
            } else {
                name.starts_with(&snake_prefix)
            };
            if stutters {
                let rest = if is_type {
                    &name[camel_prefix.len()..]
                } else {
                    &name[snake_prefix.len()..]
                };
                self.report(
                    Rule::Stutter,
                    span,
                    Some(name),
                    format!("`{name}` repeats the module name `{stem}`; consider `{rest}` (`{stem}.{rest}`)"),
                    None,
                );
            }
        }
    }

    // S003
    fn unused(&mut self, module: &Module, tokens: &[Token], companions: &[Token]) {
        let mut uses: HashMap<SymId, usize> = HashMap::new();
        let mut interpolated: HashSet<&str> = HashSet::new();
        for token in tokens.iter().chain(companions) {
            match (token.kind, token.sym) {
                (TokenKind::Ident, Some(sym)) => *uses.entry(sym).or_default() += 1,
                (TokenKind::String, Some(sym)) => {
                    let text = self.name(sym);
                    if text.contains('{') {
                        interpolated.extend(
                            text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                                .filter(|word| !word.is_empty()),
                        );
                    }
                }
                _ => {}
            }
        }
        let unused = |sym: SymId, name: &str| {
            uses.get(&sym).copied().unwrap_or(0) <= 1
                && !interpolated.contains(name)
                && !name.starts_with('_')
                && name != "main"
        };
        let mut found: Vec<(Span, String, &str)> = Vec::new();
        for item in &module.items {
            match item {
                Item::Function(f) if !f.exported && f.attrs.is_empty() => {
                    let name = self.name(f.name);
                    if unused(f.name, name) {
                        found.push((f.span, name.to_owned(), "function"));
                    }
                }
                Item::Binding(b) if !b.exported && b.attrs.is_empty() => {
                    let name = self.name(b.name);
                    if unused(b.name, name) {
                        found.push((b.span, name.to_owned(), "binding"));
                    }
                }
                Item::Struct(s) => {
                    for field in s.fields.iter().filter(|field| field.private) {
                        let name = self.name(field.name);
                        if unused(field.name, name) {
                            found.push((field.span, name.to_owned(), "private field"));
                        }
                    }
                    for method in s
                        .methods
                        .iter()
                        .filter(|m| !m.exported && m.attrs.is_empty())
                    {
                        let name = self.name(method.name);
                        if self.is_private_member(method.span) && unused(method.name, name) {
                            found.push((method.span, name.to_owned(), "private method"));
                        }
                    }
                }
                _ => {}
            }
        }
        for (span, name, what) in found {
            self.report(
                Rule::Unused,
                span,
                Some(&name),
                format!("{what} `{name}` is never used"),
                None,
            );
        }
    }

    /// The member at `span` is written with a leading `-`.
    fn is_private_member(&self, span: Span) -> bool {
        let lo = span.lo.as_usize().min(self.source.len());
        self.source[lo..].starts_with('-') || self.source[..lo].trim_end().ends_with('-')
    }

    // S005
    fn missing_law(&mut self, module: &Module, companion_laws: bool, tokens: &[Token]) {
        let exports_functions = module.items.iter().any(|item| match item {
            Item::Function(f) => f.exported,
            Item::Struct(s) => s.exported && !s.methods.is_empty(),
            Item::Sum(s) => s.exported && !s.methods.is_empty(),
            _ => false,
        });
        let declares_law = companion_laws
            || module
                .items
                .iter()
                .any(|item| matches!(item, Item::Function(f) if f.is_law()))
            || tokens.windows(2).any(|pair| {
                pair[0].kind == TokenKind::Hash
                    && pair[1].sym.is_some_and(|sym| self.name(sym) == "law")
            });
        if exports_functions && !declares_law {
            self.report(
                Rule::MissingLaw,
                Span::empty(BytePos(0)),
                None,
                "public stdlib module declares no `#law`; add a `.law.sr` companion".to_owned(),
                None,
            );
        }
    }

    // S007
    fn redundant_plus(&mut self, module: &Module) {
        let mut spans = Vec::new();
        for item in &module.items {
            let (fields, methods) = match item {
                Item::Struct(s) => (
                    s.fields.iter().map(|f| f.span).collect::<Vec<_>>(),
                    &s.methods,
                ),
                Item::Sum(s) => (Vec::new(), &s.methods),
                _ => continue,
            };
            spans.extend(fields);
            spans.extend(methods.iter().map(|m| m.span));
        }
        for span in spans {
            let lo = span.lo.as_usize().min(self.source.len());
            let plus = if self.source[lo..].starts_with('+') {
                Some(lo)
            } else {
                let before = self.source[..lo].trim_end_matches([' ', '\t']);
                before.ends_with('+').then(|| before.len() - 1)
            };
            let Some(at) = plus else { continue };
            let line_start = self.source[..at].rfind('\n').map_or(0, |i| i + 1);
            if !self.source[line_start..at].trim().is_empty() {
                continue;
            }
            let end = at + 1;
            self.report(
                Rule::RedundantPlus,
                Span::from_offsets(at as u32, end as u32),
                None,
                "`+` on a member is redundant; members of a `+` type are public by default"
                    .to_owned(),
                Some(Fix {
                    start: at,
                    end,
                    replacement: String::new(),
                }),
            );
        }
    }

    // S004 + S006
    fn bodies(&mut self, module: &Module) {
        let mut functions: Vec<&Function> = Vec::new();
        for item in &module.items {
            match item {
                Item::Function(f) => functions.push(f),
                Item::Struct(s) => functions.extend(&s.methods),
                Item::Sum(s) => functions.extend(&s.methods),
                _ => {}
            }
        }
        for f in functions {
            let mut walk = Walk::new(self.interner);
            walk.scopes.push(f.params.iter().map(|p| p.name).collect());
            walk.body(&f.body);
            for (span, name) in std::mem::take(&mut walk.shadows) {
                self.report(
                    Rule::Shadowing,
                    span,
                    Some(&name),
                    format!("`{name}` hides an enclosing local of the same name"),
                    None,
                );
            }
            self.needless_mut(f, &walk.mutated);
            self.redundant_puns(&walk.puns);
        }
        for item in &module.items {
            if let Item::Test(t) = item {
                let mut walk = Walk::new(self.interner);
                walk.block(&t.body);
                self.redundant_puns(&walk.puns);
                for (span, name) in walk.shadows {
                    self.report(
                        Rule::Shadowing,
                        span,
                        Some(&name),
                        format!("`{name}` hides an enclosing local of the same name"),
                        None,
                    );
                }
            }
        }
    }

    // S008
    fn redundant_puns(&mut self, spans: &[Span]) {
        for span in spans {
            let (lo, hi) = (span.lo.as_usize(), span.hi.as_usize());
            let Some(text) = self.source.get(lo..hi) else {
                continue;
            };
            let Some((key, value)) = text.split_once(':') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key != value || !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            self.report(
                Rule::RedundantPun,
                *span,
                Some(key),
                format!("`{key}: {key}` can be written `{key}`"),
                Some(Fix {
                    start: lo,
                    end: hi,
                    replacement: key.to_owned(),
                }),
            );
        }
    }

    fn needless_mut(&mut self, f: &Function, mutated: &HashSet<SymId>) {
        let fname = self.name(f.name);
        let params: Vec<&Param> = f
            .params
            .iter()
            .filter(|p| p.mutable && !mutated.contains(&p.name))
            .collect();
        for param in params {
            let pname = self.name(param.name);
            self.report(
                Rule::NeedlessMut,
                param.span,
                Some(&format!("{fname}.{pname}")),
                format!("`@{pname}` in `{fname}` is never mutated; drop the `@`"),
                None,
            );
        }
    }
}

/// Scoped walk over one function body: shadowing and mutated roots.
struct Walk<'a> {
    interner: &'a Interner,
    scopes: Vec<Vec<SymId>>,
    shadows: Vec<(Span, String)>,
    mutated: HashSet<SymId>,
    /// `name: name` record fields.
    puns: Vec<Span>,
}

fn root(expr: &Expr) -> Option<SymId> {
    match expr {
        Expr::Ident(sym, _) => Some(*sym),
        Expr::Field { base, .. }
        | Expr::Index { base, .. }
        | Expr::Slice { base, .. }
        | Expr::ComptimeField { base, .. } => root(base),
        Expr::Unary { expr, .. } => root(expr),
        Expr::Call { callee, .. } => root(callee),
        Expr::Pipeline { base, .. } => root(base),
        _ => None,
    }
}

impl<'a> Walk<'a> {
    fn new(interner: &'a Interner) -> Self {
        Walk {
            interner,
            scopes: Vec::new(),
            shadows: Vec::new(),
            mutated: HashSet::new(),
            puns: Vec::new(),
        }
    }

    /// Host calls act on handles the compiler cannot see into.
    fn is_host(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Ident(sym, _) if self.interner.resolve(*sym) == "host")
    }

    fn in_scope(&self, sym: SymId) -> bool {
        self.scopes.iter().any(|scope| scope.contains(&sym))
    }

    /// A construct that always introduces a fresh name.
    fn fresh(&mut self, sym: SymId, span: Span) {
        let name = self.interner.resolve(sym);
        if !matches!(name, "_" | "it" | "index" | "err" | "self") && self.in_scope(sym) {
            self.shadows.push((span, name.to_owned()));
        }
        self.declare(sym);
    }

    fn declare(&mut self, sym: SymId) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(sym);
        } else {
            self.scopes.push(vec![sym]);
        }
    }

    fn mutate(&mut self, expr: &Expr) {
        if let Some(sym) = root(expr) {
            self.mutated.insert(sym);
        }
    }

    fn body(&mut self, body: &FnBody) {
        match body {
            FnBody::Expr(e) => self.expr(e),
            FnBody::Block(b) => self.block(b),
        }
    }

    fn block(&mut self, block: &Block) {
        self.scopes.push(Vec::new());
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Binding(b) => {
                self.expr(&b.value);
                if self.in_scope(b.name) {
                    self.mutated.insert(b.name);
                } else {
                    self.declare(b.name);
                }
            }
            Stmt::Destructure {
                fields,
                value,
                span,
            } => {
                self.expr(value);
                for (_, local) in fields {
                    self.fresh(*local, *span);
                }
            }
            Stmt::Expr(e) | Stmt::Fail(e, _) => self.expr(e),
            Stmt::Return(e, _) | Stmt::Panic(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                self.expr(cond);
                self.block(else_block);
            }
            Stmt::Each {
                mutable,
                iter,
                bindings,
                body,
                span,
                ..
            } => {
                self.expr(iter);
                if *mutable {
                    self.mutate(iter);
                }
                self.scopes.push(Vec::new());
                for binding in bindings {
                    self.fresh(binding.name, *span);
                }
                self.block(body);
                self.scopes.pop();
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Loop { body, .. } | Stmt::Scope { body, .. } => self.block(body),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::MultiAssign {
                targets, values, ..
            } => {
                for value in values {
                    self.expr(value);
                }
                for target in targets {
                    self.mutate(target);
                    self.expr(target);
                    if let Expr::Ident(sym, _) = target
                        && !self.in_scope(*sym)
                    {
                        self.declare(*sym);
                    }
                }
            }
            Stmt::Guarded { stmt, cond, .. } => {
                self.expr(cond);
                self.stmt(stmt);
            }
            Stmt::When { subjects, arms, .. } => {
                for subject in subjects {
                    self.expr(subject);
                }
                for arm in arms {
                    self.scopes.push(Vec::new());
                    self.pattern(&arm.pattern);
                    self.expr(&arm.body);
                    self.scopes.pop();
                }
            }
            Stmt::Comptime(c) => match c {
                crate::ast::ComptimeStmt::Block(b, _) => self.block(b),
                crate::ast::ComptimeStmt::If {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => {
                    self.expr(cond);
                    self.block(then_block);
                    if let Some(b) = else_block {
                        self.block(b);
                    }
                }
                crate::ast::ComptimeStmt::Each {
                    name,
                    iter,
                    body,
                    span,
                } => {
                    self.expr(iter);
                    self.scopes.push(Vec::new());
                    self.fresh(*name, *span);
                    self.block(body);
                    self.scopes.pop();
                }
            },
        }
    }

    fn pattern(&mut self, pattern: &WhenPattern) {
        match pattern {
            WhenPattern::Else => {}
            WhenPattern::Expr(e) | WhenPattern::After(e) => self.expr(e),
            WhenPattern::VariantBind { fields, span, .. } => {
                for (_, local) in fields {
                    self.fresh(*local, *span);
                }
            }
            WhenPattern::Tuple { elems, .. } => {
                for elem in elems {
                    self.pattern(elem);
                }
            }
        }
    }

    fn fields(&mut self, fields: &[RecordField]) {
        for field in fields {
            if let (Some(name), Expr::Ident(value, _)) = (field.name, &field.value)
                && name == *value
            {
                self.puns.push(field.span);
            }
            self.expr(&field.value);
        }
    }

    fn macro_call(&mut self, call: &MacroCall) {
        // Macro input is opaque here: any name it mentions may be mutated.
        for token in &call.input.tokens {
            if let Some(sym) = token.sym {
                self.mutated.insert(sym);
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
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
            | Expr::GoalType(_) => {}
            Expr::Binary { left, right, .. } | Expr::Or { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Unary { op, expr, .. } => {
                if *op == UnOp::MutRef {
                    self.mutate(expr);
                }
                self.expr(expr);
            }
            Expr::Call { callee, args, .. } => {
                if let Expr::Field { base, field, .. } = callee.as_ref() {
                    if MUTATING_METHODS.contains(&self.interner.resolve(*field)) {
                        self.mutate(base);
                    }
                    if self.is_host(base) {
                        for arg in args {
                            self.mutate(arg);
                        }
                    }
                }
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Pipeline {
                base, method, args, ..
            } => {
                if MUTATING_METHODS.contains(&self.interner.resolve(*method)) {
                    self.mutate(base);
                }
                if self.is_host(base) {
                    for arg in args {
                        self.mutate(arg);
                    }
                }
                self.expr(base);
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Slice {
                base, start, end, ..
            } => {
                self.expr(base);
                self.expr(start);
                self.expr(end);
            }
            Expr::Field { base, .. } | Expr::Await(base, _) => self.expr(base),
            Expr::Index { base, index, .. } => {
                self.expr(base);
                self.expr(index);
            }
            Expr::ComptimeField { base, field, .. } => {
                self.expr(base);
                self.expr(field);
            }
            Expr::Assign { target, value, .. } => {
                self.mutate(target);
                self.expr(target);
                self.expr(value);
            }
            Expr::Array(items, _) => {
                for item in items {
                    self.expr(item);
                }
            }
            Expr::ArrayRepeat { value, len, .. } => {
                self.expr(value);
                self.expr(len);
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.expr(cond);
                self.expr(then_expr);
                self.expr(else_expr);
            }
            Expr::Map(entries, _) => {
                for entry in entries {
                    if let crate::ast::MapKey::Expr(key) = &entry.key {
                        self.expr(key);
                    }
                    self.expr(&entry.value);
                }
            }
            Expr::Record(fields, _)
            | Expr::StructLit { fields, .. }
            | Expr::InferredStruct { fields, .. } => self.fields(fields),
            Expr::Range { start, end, .. } => {
                self.expr(start);
                self.expr(end);
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
                self.expr(cond);
                self.block(then_block);
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => self.block(b),
            Expr::OrErr { left, body, .. } => {
                self.expr(left);
                self.block(body);
            }
            Expr::Spawn { body, .. } => self.expr(body),
            Expr::Chan { capacity, .. } => self.expr(capacity),
            Expr::Shared { value, .. } | Expr::Duration { value, .. } => self.expr(value),
            Expr::NamedVariant { args, .. } => {
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Quote { body, .. } => self.expr(body),
            Expr::Splice { code, .. } => self.expr(code),
            Expr::ComptimeEach {
                name,
                iter,
                body,
                span,
            } => {
                self.expr(iter);
                self.scopes.push(Vec::new());
                self.fresh(*name, *span);
                self.block(body);
                self.scopes.pop();
            }
            Expr::Lambda { params, body, .. } => {
                self.scopes.push(Vec::new());
                for param in params {
                    self.fresh(param.name, param.span);
                }
                self.body(body);
                self.scopes.pop();
            }
            Expr::Panic { message, .. } => {
                if let Some(m) = message {
                    self.expr(m);
                }
            }
            Expr::MacroCall(call) => self.macro_call(call),
        }
    }
}

#[cfg(test)]
mod tests;
