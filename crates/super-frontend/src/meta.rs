//! Staged `#` compile-time evaluation, quote/splice expansion, and `//#` transforms.
//!
//! Runs during semantic analysis against a sandboxed interpreter (mirrors VM ops for
//! const-foldable Super subsets). Results are cached by input hash for LSP <2ms hot paths.

use crate::ast::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use super_core::{DiagnosticKind, Diagnostics, Interner, Span, SymId};

/// Evaluated compile-time value (subset of runtime `Value`).
#[derive(Clone, Debug)]
pub enum CtValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<CtValue>),
    /// Quoted AST held as `Code[T]`.
    Code(Expr),
    /// Type reflection handle.
    TypeRef(SymId),
}

impl CtValue {
    pub fn as_int(&self) -> i64 {
        match self {
            CtValue::Int(n) => *n,
            CtValue::Float(f) => *f as i64,
            CtValue::Bool(b) => i64::from(*b),
            _ => 0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            CtValue::Bool(b) => *b,
            CtValue::Int(0) | CtValue::Null => false,
            _ => true,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            CtValue::Str(s) => s.clone(),
            CtValue::Int(n) => n.to_string(),
            CtValue::Float(f) => f.to_string(),
            CtValue::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    /// Lower a comptime value into a runtime AST literal expression.
    pub fn to_expr(&self, span: Span, interner: &mut Interner) -> Expr {
        match self {
            CtValue::Null => Expr::None(span),
            CtValue::Bool(b) => Expr::Bool(*b, span),
            CtValue::Int(n) => Expr::Int(*n, span),
            CtValue::Float(f) => Expr::Float(*f, span),
            CtValue::Str(s) => Expr::String(interner.intern(s), span),
            CtValue::Array(xs) => {
                let elems: Vec<Expr> = xs.iter().map(|v| v.to_expr(span, interner)).collect();
                Expr::Array(elems, span)
            }
            CtValue::Code(e) => e.clone(),
            CtValue::TypeRef(sym) => Expr::Ident(*sym, span),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MetaCache {
    pub bindings: HashMap<SymId, CtValue>,
    pub expansions: HashMap<u64, String>,
    pub route_table: Vec<(String, String, SymId)>,
    pub rewrite_rules: Vec<RewriteRule>,
}

pub struct MetaEngine<'a> {
    interner: &'a mut Interner,
    diagnostics: &'a mut Diagnostics,
    pub cache: MetaCache,
    /// Struct field layouts for reflection: name → (field_name, private, size_hint)
    struct_fields: HashMap<SymId, Vec<(SymId, bool)>>,
    struct_sizes: HashMap<SymId, u32>,
    sum_variants: HashMap<SymId, Vec<SymId>>,
}

impl<'a> MetaEngine<'a> {
    pub fn new(interner: &'a mut Interner, diagnostics: &'a mut Diagnostics) -> Self {
        Self {
            interner,
            diagnostics,
            cache: MetaCache::default(),
            struct_fields: HashMap::new(),
            struct_sizes: HashMap::new(),
            sum_variants: HashMap::new(),
        }
    }

    /// Collect type metadata then expand `#` / `//#` / quotes into concrete AST.
    pub fn expand_module(&mut self, module: &mut Module) {
        self.collect_type_info(module);
        // First pass: evaluate comptime bindings & rewrite rules.
        let mut new_items = Vec::with_capacity(module.items.len());
        for item in std::mem::take(&mut module.items) {
            match item {
                Item::Rewrite(rule) => {
                    self.cache.rewrite_rules.push(rule.clone());
                    // Keep for documentation / LSP; no runtime emission needed.
                    new_items.push(Item::Rewrite(rule));
                }
                Item::Binding(mut b) if b.comptime => {
                    self.apply_binding_attrs(&b);
                    let val = self.eval_expr(&b.value);
                    self.cache.bindings.insert(b.name, val.clone());
                    // Materialize as a normal const binding for runtime when useful.
                    b.comptime = false;
                    b.value = val.to_expr(b.span, self.interner);
                    new_items.push(Item::Binding(b));
                }
                Item::Function(mut f) => {
                    self.apply_fn_attrs(&mut f);
                    self.expand_fn_body(&mut f);
                    if let Some(mirror) = self.synthesize_symmetric(&f) {
                        new_items.push(Item::Function(f));
                        new_items.push(Item::Function(mirror));
                    } else {
                        new_items.push(Item::Function(f));
                    }
                }
                Item::Struct(mut s) => {
                    self.apply_struct_attrs(&s);
                    let mut methods = Vec::with_capacity(s.methods.len() * 2);
                    for mut m in std::mem::take(&mut s.methods) {
                        self.apply_fn_attrs(&mut m);
                        self.expand_fn_body(&mut m);
                        if let Some(mirror) = self.synthesize_symmetric(&m) {
                            methods.push(m);
                            methods.push(mirror);
                        } else {
                            methods.push(m);
                        }
                    }
                    s.methods = methods;
                    new_items.push(Item::Struct(s));
                }
                Item::Sum(mut s) => {
                    let mut methods = Vec::with_capacity(s.methods.len() * 2);
                    for mut m in std::mem::take(&mut s.methods) {
                        self.apply_fn_attrs(&mut m);
                        self.expand_fn_body(&mut m);
                        if let Some(mirror) = self.synthesize_symmetric(&m) {
                            methods.push(m);
                            methods.push(mirror);
                        } else {
                            methods.push(m);
                        }
                    }
                    s.methods = methods;
                    new_items.push(Item::Sum(s));
                }
                Item::Test(mut t) => {
                    self.expand_block(&mut t.body);
                    new_items.push(Item::Test(t));
                }
                other => new_items.push(other),
            }
        }
        module.items = new_items;
        // Second pass: expand remaining blocks in state/bindings.
        for item in &mut module.items {
            match item {
                Item::Binding(b) => {
                    b.value = self.expand_expr(std::mem::replace(&mut b.value, Expr::None(b.span)));
                }
                Item::State(s) => {
                    s.value = self.expand_expr(std::mem::replace(&mut s.value, Expr::None(s.span)));
                }
                _ => {}
            }
        }
    }

    fn collect_type_info(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    let fields: Vec<_> = s.fields.iter().map(|f| (f.name, f.private)).collect();
                    // Rough C-ABI size: 8 bytes per field (pointer/i64/f64 dominant).
                    let size = (fields.len() as u32) * 8;
                    self.struct_fields.insert(s.name, fields);
                    self.struct_sizes.insert(s.name, size.max(8));
                }
                Item::Sum(s) => {
                    let vs: Vec<_> = s.variants.iter().map(|v| v.name).collect();
                    self.sum_variants.insert(s.name, vs);
                }
                _ => {}
            }
        }
    }

    fn apply_binding_attrs(&mut self, b: &Binding) {
        for a in &b.attrs {
            self.dispatch_attr(a);
        }
        self.apply_binding_route_attrs(b);
    }

    fn apply_struct_attrs(&mut self, s: &StructDef) {
        for a in &s.attrs {
            self.dispatch_attr(a);
        }
    }

    fn apply_fn_attrs(&mut self, f: &mut Function) {
        for a in &f.attrs.clone() {
            self.dispatch_attr(a);
            if let MetaAttrKind::Route { method, path } = &a.kind {
                let m = self.interner.resolve(*method).to_string();
                let p = self.interner.resolve(*path).to_string();
                self.cache.route_table.push((m, p, f.name));
            }
            if matches!(a.kind, MetaAttrKind::Symmetric) {
                self.validate_symmetric(f, a.span);
            }
        }
    }

    /// `#symmetric` requires arity 2 with distinct parameter types / mutability.
    fn validate_symmetric(&mut self, f: &Function, span: Span) {
        if f.params.len() != 2 {
            self.diagnostics.error_code(
                DiagnosticKind::Type,
                span,
                super_core::err::E0501,
                format!(
                    "`#symmetric` requires exactly 2 parameters (found {})",
                    f.params.len()
                ),
            );
            return;
        }
        let a = &f.params[0];
        let b = &f.params[1];
        let same_ty = type_expr_key(&a.ty) == type_expr_key(&b.ty);
        if same_ty && a.mutable == b.mutable {
            self.diagnostics.error_code(
                DiagnosticKind::Type,
                span,
                super_core::err::E0501,
                "`#symmetric` requires distinct parameter types or `@` mutability",
            );
        }
    }

    /// Synthesize `f(b, a) => f(a, b)` preserving visibility and `@` on swapped params.
    fn synthesize_symmetric(&mut self, f: &Function) -> Option<Function> {
        if !f.is_symmetric() || f.params.len() != 2 {
            return None;
        }
        let a = &f.params[0];
        let b = &f.params[1];
        if type_expr_key(&a.ty) == type_expr_key(&b.ty) && a.mutable == b.mutable {
            return None;
        }
        let p0 = Param {
            mutable: b.mutable,
            comptime: b.comptime,
            name: b.name,
            ty: b.ty.clone(),
            span: b.span,
        };
        let p1 = Param {
            mutable: a.mutable,
            comptime: a.comptime,
            name: a.name,
            ty: a.ty.clone(),
            span: a.span,
        };
        let call_args = vec![
            if a.mutable {
                Expr::Unary {
                    op: UnOp::MutRef,
                    expr: Box::new(Expr::Ident(a.name, a.span)),
                    span: a.span,
                }
            } else {
                Expr::Ident(a.name, a.span)
            },
            if b.mutable {
                Expr::Unary {
                    op: UnOp::MutRef,
                    expr: Box::new(Expr::Ident(b.name, b.span)),
                    span: b.span,
                }
            } else {
                Expr::Ident(b.name, b.span)
            },
        ];
        // Mirror body calls the original with original argument order.
        let body = FnBody::Expr(Expr::Call {
            callee: Box::new(Expr::Ident(f.name, f.span)),
            args: call_args,
            punning: false,
            span: f.span,
        });
        // Drop Symmetric on the mirror to avoid recursive synthesis.
        let attrs: Vec<MetaAttr> = f
            .attrs
            .iter()
            .filter(|a| !matches!(a.kind, MetaAttrKind::Symmetric))
            .cloned()
            .collect();
        Some(Function {
            exported: f.exported,
            name: f.name,
            owner: f.owner,
            params: vec![p0, p1],
            ret: f.ret.clone(),
            body,
            span: f.span,
            attrs,
            docs: f.docs,
        })
    }

    fn apply_binding_route_attrs(&mut self, b: &Binding) {
        for a in &b.attrs {
            if let MetaAttrKind::Route { method, path } = &a.kind {
                let m = self.interner.resolve(*method).to_string();
                let p = self.interner.resolve(*path).to_string();
                self.cache.route_table.push((m, p, b.name));
            }
        }
    }

    fn dispatch_attr(&mut self, a: &MetaAttr) {
        match &a.kind {
            MetaAttrKind::AssertSize { ty, bytes } => {
                let actual = self.struct_sizes.get(ty).copied().unwrap_or(0);
                if actual != *bytes {
                    let name = self.interner.resolve(*ty).to_string();
                    self.diagnostics.error_code(
                        DiagnosticKind::Type,
                        a.span,
                        super_core::err::E0501,
                        format!("//# assert_size({name}, {bytes}) failed: actual size is {actual}"),
                    );
                }
            }
            MetaAttrKind::Inspect(sym) => {
                let name = self.interner.resolve(*sym).to_string();
                let preview = if let Some(v) = self.cache.bindings.get(sym) {
                    format!("{v:?}")
                } else if let Some(fields) = self.struct_fields.get(sym) {
                    format!("struct fields={fields:?}")
                } else {
                    format!("unknown `{name}`")
                };
                let key = span_key(a.span);
                self.cache.expansions.insert(key, preview);
            }
            MetaAttrKind::InlineComptime(raw) => {
                let text = self.interner.resolve(*raw).to_string();
                let _ = text; // parsed/executed via surrounding # forms when present
            }
            _ => {}
        }
    }

    fn expand_fn_body(&mut self, f: &mut Function) {
        match &mut f.body {
            FnBody::Expr(e) => {
                *e = self.expand_expr(std::mem::replace(e, Expr::None(f.span)));
            }
            FnBody::Block(b) => self.expand_block(b),
        }
    }

    fn expand_block(&mut self, block: &mut Block) {
        let mut out = Vec::with_capacity(block.stmts.len());
        for stmt in std::mem::take(&mut block.stmts) {
            out.extend(self.expand_stmt(stmt));
        }
        block.stmts = out;
    }

    fn expand_stmt(&mut self, stmt: Stmt) -> Vec<Stmt> {
        match stmt {
            Stmt::Comptime(cs) => self.eval_comptime_stmt(cs),
            Stmt::Each {
                mutable,
                parallel,
                reduction,
                iter,
                bindings,
                body,
                span,
                attrs,
            } => {
                let unroll = attrs.iter().find_map(|a| match a.kind {
                    MetaAttrKind::Unroll(n) => Some(n),
                    _ => None,
                });
                let mut body = body;
                self.expand_block(&mut body);
                let iter = self.expand_expr(iter);
                if let Some(n) = unroll {
                    return self.unroll_each(n, mutable, iter, body, span);
                }
                vec![Stmt::Each {
                    mutable,
                    parallel,
                    reduction,
                    iter,
                    bindings,
                    body,
                    span,
                    attrs,
                }]
            }
            Stmt::While {
                cond,
                mut body,
                span,
            } => {
                self.expand_block(&mut body);
                vec![Stmt::While {
                    cond: self.expand_expr(cond),
                    body,
                    span,
                }]
            }
            Stmt::Loop { mut body, span } => {
                self.expand_block(&mut body);
                vec![Stmt::Loop { body, span }]
            }
            Stmt::Continue(s) => vec![Stmt::Continue(s)],
            Stmt::MultiAssign {
                targets,
                values,
                span,
            } => vec![Stmt::MultiAssign {
                targets: targets.into_iter().map(|e| self.expand_expr(e)).collect(),
                values: values.into_iter().map(|e| self.expand_expr(e)).collect(),
                span,
            }],
            Stmt::Guarded { stmt, cond, span } => {
                let mut expanded = self.expand_stmt(*stmt);
                let inner = if expanded.len() == 1 {
                    expanded.pop().unwrap()
                } else {
                    Stmt::Expr(Expr::Block(Block {
                        stmts: expanded,
                        span,
                    }))
                };
                vec![Stmt::Guarded {
                    stmt: Box::new(inner),
                    cond: self.expand_expr(cond),
                    span,
                }]
            }
            Stmt::Binding(mut b) => {
                b.value = self.expand_expr(b.value);
                if b.comptime {
                    let val = self.eval_expr(&b.value);
                    self.cache.bindings.insert(b.name, val.clone());
                    b.comptime = false;
                    b.value = val.to_expr(b.span, self.interner);
                }
                vec![Stmt::Binding(b)]
            }
            Stmt::Ensure {
                cond,
                mut else_block,
                span,
            } => {
                self.expand_block(&mut else_block);
                vec![Stmt::Ensure {
                    cond: self.expand_expr(cond),
                    else_block,
                    span,
                }]
            }
            Stmt::When {
                subjects,
                mut arms,
                span,
            } => {
                for arm in &mut arms {
                    arm.body =
                        self.expand_expr(std::mem::replace(&mut arm.body, Expr::None(arm.span)));
                    if let WhenPattern::Tuple { elems, .. } = &mut arm.pattern {
                        for elem in elems {
                            if let WhenPattern::Expr(e) = elem {
                                *e = self.expand_expr(std::mem::replace(e, Expr::None(arm.span)));
                            }
                        }
                    }
                }
                vec![Stmt::When {
                    subjects: subjects.into_iter().map(|s| self.expand_expr(s)).collect(),
                    arms,
                    span,
                }]
            }
            Stmt::Scope { mut body, span } => {
                self.expand_block(&mut body);
                vec![Stmt::Scope { body, span }]
            }
            Stmt::Return(v, span) => vec![Stmt::Return(v.map(|e| self.expand_expr(e)), span)],
            Stmt::Fail(e, span) => vec![Stmt::Fail(self.expand_expr(e), span)],
            Stmt::Panic(msg, span) => {
                vec![Stmt::Panic(msg.map(|e| self.expand_expr(e)), span)]
            }
            Stmt::Expr(e) => vec![Stmt::Expr(self.expand_expr(e))],
            Stmt::Destructure {
                fields,
                value,
                span,
            } => vec![Stmt::Destructure {
                fields,
                value: self.expand_expr(value),
                span,
            }],
            Stmt::Break(span) => vec![Stmt::Break(span)],
        }
    }

    fn eval_comptime_stmt(&mut self, cs: ComptimeStmt) -> Vec<Stmt> {
        match cs {
            ComptimeStmt::Block(mut body, _) => {
                self.expand_block(&mut body);
                // Execute for side-effects on cache; statements that remain are dropped
                // unless they are materializable bindings.
                let mut out = Vec::new();
                for s in body.stmts {
                    if let Stmt::Binding(mut b) = s {
                        let val = self.eval_expr(&b.value);
                        self.cache.bindings.insert(b.name, val.clone());
                        b.value = val.to_expr(b.span, self.interner);
                        b.comptime = false;
                        out.push(Stmt::Binding(b));
                    }
                }
                out
            }
            ComptimeStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if self.eval_expr(&cond).as_bool() {
                    let mut b = then_block;
                    self.expand_block(&mut b);
                    b.stmts
                } else if let Some(mut eb) = else_block {
                    self.expand_block(&mut eb);
                    eb.stmts
                } else {
                    vec![]
                }
            }
            ComptimeStmt::Each {
                name,
                iter,
                body,
                span,
            } => {
                let list = self.eval_expr(&iter);
                let mut out = Vec::new();
                if let CtValue::Array(xs) = list {
                    for x in xs {
                        self.cache.bindings.insert(name, x);
                        let mut b = body.clone();
                        self.expand_block(&mut b);
                        out.extend(b.stmts);
                    }
                } else {
                    self.diagnostics.error_code(
                        DiagnosticKind::Type,
                        span,
                        super_core::err::E0501,
                        "#for expects a compile-time array",
                    );
                }
                out
            }
        }
    }

    /// Unroll `for 0..N` (or fixed array) into straight-line statements.
    /// Only fires when the iteration bound is a compile-time constant; otherwise
    /// keeps the original `for` so runtime-bounded loops stay correct.
    fn unroll_each(
        &mut self,
        n: u32,
        mutable: bool,
        iter: Expr,
        body: Block,
        span: Span,
    ) -> Vec<Stmt> {
        let const_count = match &iter {
            Expr::Range { start, end, .. } => {
                let s = self.eval_expr(start);
                let e = self.eval_expr(end);
                // Both ends must be known ints (not Null from unbound idents).
                match (&s, &e) {
                    (CtValue::Int(a), CtValue::Int(b)) => Some((*b - *a).max(0) as u32),
                    (CtValue::Int(a), _) if matches!(e, CtValue::Float(_)) => {
                        Some((e.as_int() - *a).max(0) as u32)
                    }
                    _ => None,
                }
            }
            Expr::Array(xs, _) => Some(xs.len() as u32),
            _ => None,
        };
        let limit = const_count.unwrap_or(n).min(n);
        if const_count.is_none() && !matches!(iter, Expr::Array(_, _)) {
            // Runtime-bounded — keep the loop, attrs preserved for JIT hints.
            return vec![Stmt::Each {
                mutable,
                parallel: false,
                reduction: None,
                iter,
                bindings: Vec::new(),
                body,
                span,
                attrs: vec![MetaAttr {
                    kind: MetaAttrKind::Unroll(n),
                    span,
                    raw: self.interner.intern(&format!("unroll({n})")),
                }],
            }];
        }
        let mut out = Vec::new();
        for i in 0..limit {
            let mut b = body.clone();
            substitute_it_index(&mut b, i as i64, span);
            self.expand_block(&mut b);
            out.extend(b.stmts);
        }
        out
    }

    fn expand_expr(&mut self, expr: Expr) -> Expr {
        match expr {
            Expr::Quote { body, span } => {
                let expanded = self.expand_expr(*body);
                let key = span_key(span);
                self.cache
                    .expansions
                    .insert(key, format!("Code[…] quote @ {span:?}"));
                Expr::Quote {
                    body: Box::new(expanded),
                    span,
                }
            }
            Expr::Splice { code, span } => {
                let code_e = self.expand_expr(*code);
                match self.eval_expr(&code_e) {
                    CtValue::Code(inner) => {
                        let key = span_key(span);
                        self.cache
                            .expansions
                            .insert(key, format!("spliced AST @ {span:?}"));
                        self.expand_expr(inner)
                    }
                    CtValue::Str(s) => Expr::String(self.interner.intern(&s), span),
                    CtValue::Int(n) => Expr::Int(n, span),
                    CtValue::Array(xs) => {
                        // Join-like splice of quoted pieces → concatenate string literals when possible.
                        let mut parts = Vec::new();
                        for x in xs {
                            match x {
                                CtValue::Code(e) => parts.push(self.expand_expr(e)),
                                other => parts.push(other.to_expr(span, self.interner)),
                            }
                        }
                        if parts.is_empty() {
                            Expr::String(self.interner.intern(""), span)
                        } else if parts.len() == 1 {
                            parts.pop().unwrap()
                        } else {
                            // Fold with `+` for stringy joins.
                            let mut acc = parts.remove(0);
                            for p in parts {
                                let s = acc.span().merge(p.span());
                                acc = Expr::Binary {
                                    op: BinOp::Add,
                                    left: Box::new(acc),
                                    right: Box::new(p),
                                    span: s,
                                };
                            }
                            acc
                        }
                    }
                    other => other.to_expr(span, self.interner),
                }
            }
            Expr::ComptimeRef { name, span } => {
                if let Some(v) = self.cache.bindings.get(&name).cloned() {
                    let key = span_key(span);
                    self.cache.expansions.insert(key, format!("{v:?}"));
                    v.to_expr(span, self.interner)
                } else {
                    Expr::Ident(name, span)
                }
            }
            Expr::ComptimeBlock { body, span } => {
                let val = self.eval_block_value(&body);
                val.to_expr(span, self.interner)
            }
            Expr::ComptimeIf {
                cond,
                then_block,
                else_block,
                span,
            } => {
                if self.eval_expr(&cond).as_bool() {
                    self.eval_block_value(&then_block)
                        .to_expr(span, self.interner)
                } else if let Some(eb) = else_block {
                    self.eval_block_value(&eb).to_expr(span, self.interner)
                } else {
                    Expr::None(span)
                }
            }
            Expr::ComptimeEach {
                name,
                iter,
                body,
                span,
            } => {
                let list = self.eval_expr(&iter);
                let mut acc = Vec::new();
                if let CtValue::Array(xs) = list {
                    for x in xs {
                        self.cache.bindings.insert(name, x);
                        acc.push(self.eval_block_value(&body));
                    }
                }
                CtValue::Array(acc).to_expr(span, self.interner)
            }
            Expr::GoalType(span) => {
                // Placeholder: materialize as string "goal" for runtime; LSP shows real type.
                Expr::String(self.interner.intern("goal"), span)
            }
            Expr::ComptimeField { base, field, span } => {
                let field_v = self.eval_expr(&field);
                let fname = match field_v {
                    CtValue::Str(s) => self.interner.intern(&s),
                    CtValue::TypeRef(s) => s,
                    _ => self.interner.intern("_"),
                };
                Expr::Field {
                    base: Box::new(self.expand_expr(*base)),
                    field: fname,
                    span,
                }
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let left = self.expand_expr(*left);
                let right = self.expand_expr(*right);
                // Apply rewrite rules on pipeline-shaped trees.
                let rebuilt = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                    span,
                };
                self.apply_rewrites(rebuilt)
            }
            Expr::Pipeline {
                base,
                method,
                args,
                span,
            } => {
                let base = self.expand_expr(*base);
                let args: Vec<_> = args.into_iter().map(|a| self.expand_expr(a)).collect();
                let rebuilt = Expr::Pipeline {
                    base: Box::new(base),
                    method,
                    args,
                    span,
                };
                self.apply_rewrites(rebuilt)
            }
            Expr::Call {
                callee,
                args,
                punning,
                span,
            } => Expr::Call {
                callee: Box::new(self.expand_expr(*callee)),
                args: args.into_iter().map(|a| self.expand_expr(a)).collect(),
                punning,
                span,
            },
            Expr::Unary { op, expr, span } => Expr::Unary {
                op,
                expr: Box::new(self.expand_expr(*expr)),
                span,
            },
            Expr::Field { base, field, span } => {
                // Type reflection: `T.fields` / `T.size` / `T.variants` at comptime names.
                if let Expr::Ident(ty_name, _) = base.as_ref() {
                    let fname = self.interner.resolve(field);
                    if fname == "fields"
                        && let Some(fields) = self.struct_fields.get(ty_name)
                    {
                        let arr: Vec<CtValue> = fields
                            .iter()
                            .map(|(n, priv_)| {
                                CtValue::Array(vec![
                                    CtValue::Str(self.interner.resolve(*n).to_string()),
                                    CtValue::Bool(*priv_),
                                ])
                            })
                            .collect();
                        return CtValue::Array(arr).to_expr(span, self.interner);
                    }
                    if fname == "size"
                        && let Some(sz) = self.struct_sizes.get(ty_name)
                    {
                        return Expr::Int(*sz as i64, span);
                    }
                    if fname == "variants"
                        && let Some(vs) = self.sum_variants.get(ty_name)
                    {
                        let arr: Vec<CtValue> = vs
                            .iter()
                            .map(|n| CtValue::Str(self.interner.resolve(*n).to_string()))
                            .collect();
                        return CtValue::Array(arr).to_expr(span, self.interner);
                    }
                    if fname == "align" {
                        return Expr::Int(8, span);
                    }
                }
                Expr::Field {
                    base: Box::new(self.expand_expr(*base)),
                    field,
                    span,
                }
            }
            Expr::Array(xs, span) => {
                Expr::Array(xs.into_iter().map(|e| self.expand_expr(e)).collect(), span)
            }
            Expr::If {
                cond,
                mut then_block,
                else_block,
                span,
            } => {
                self.expand_block(&mut then_block);
                let else_block = else_block.map(|mut b| {
                    self.expand_block(&mut b);
                    b
                });
                Expr::If {
                    cond: Box::new(self.expand_expr(*cond)),
                    then_block,
                    else_block,
                    span,
                }
            }
            Expr::Block(mut b) => {
                self.expand_block(&mut b);
                Expr::Block(b)
            }
            Expr::Assign {
                op,
                target,
                value,
                span,
            } => Expr::Assign {
                op,
                target: Box::new(self.expand_expr(*target)),
                value: Box::new(self.expand_expr(*value)),
                span,
            },
            Expr::Or { left, right, span } => Expr::Or {
                left: Box::new(self.expand_expr(*left)),
                right: Box::new(self.expand_expr(*right)),
                span,
            },
            Expr::OrErr {
                left,
                mut body,
                span,
            } => {
                self.expand_block(&mut body);
                Expr::OrErr {
                    left: Box::new(self.expand_expr(*left)),
                    body,
                    span,
                }
            }
            Expr::Spawn { body, span } => Expr::Spawn {
                body: Box::new(self.expand_expr(*body)),
                span,
            },
            Expr::Shared { value, span } => Expr::Shared {
                value: Box::new(self.expand_expr(*value)),
                span,
            },
            Expr::Await(e, span) => Expr::Await(Box::new(self.expand_expr(*e)), span),
            Expr::Index { base, index, span } => Expr::Index {
                base: Box::new(self.expand_expr(*base)),
                index: Box::new(self.expand_expr(*index)),
                span,
            },
            Expr::Slice {
                base,
                start,
                end,
                span,
            } => Expr::Slice {
                base: Box::new(self.expand_expr(*base)),
                start: Box::new(self.expand_expr(*start)),
                end: Box::new(self.expand_expr(*end)),
                span,
            },
            Expr::Range { start, end, span } => Expr::Range {
                start: Box::new(self.expand_expr(*start)),
                end: Box::new(self.expand_expr(*end)),
                span,
            },
            Expr::Lambda {
                params,
                ret,
                body,
                captures,
                span,
            } => {
                let body = match *body {
                    FnBody::Expr(e) => FnBody::Expr(self.expand_expr(e)),
                    FnBody::Block(mut b) => {
                        self.expand_block(&mut b);
                        FnBody::Block(b)
                    }
                };
                Expr::Lambda {
                    params,
                    ret,
                    body: Box::new(body),
                    captures,
                    span,
                }
            }
            Expr::Panic { message, span } => Expr::Panic {
                message: message.map(|m| Box::new(self.expand_expr(*m))),
                span,
            },
            other => other,
        }
    }

    fn apply_rewrites(&mut self, expr: Expr) -> Expr {
        // fuse_keep_map: xs.keep(pred).map(mapper) => xs.filter_map(…)
        if let Expr::Pipeline {
            base,
            method,
            args,
            span,
        } = &expr
        {
            let method_name = self.interner.resolve(*method).to_string();
            // keep→map fusion is disabled until `filter_map` is emitted by the backend.
            let _ = (method_name, base, method, args, span);
        }
        let rule_names: Vec<String> = self
            .cache
            .rewrite_rules
            .iter()
            .map(|r| self.interner.resolve(r.name).to_string())
            .collect();
        for rname in rule_names {
            if rname.contains("fuse_keep_map")
                && let Expr::Pipeline {
                    base,
                    method,
                    args,
                    span,
                } = &expr
                && self.interner.resolve(*method) == "map"
                && let Expr::Pipeline {
                    base: ib,
                    method: im,
                    args: ia,
                    ..
                } = base.as_ref()
                && self.interner.resolve(*im) == "keep"
            {
                let filter_map = self.interner.intern("filter_map");
                let mut fused = ia.clone();
                fused.extend(args.clone());
                return Expr::Pipeline {
                    base: ib.clone(),
                    method: filter_map,
                    args: fused,
                    span: *span,
                };
            }
        }
        expr
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> CtValue {
        match expr {
            Expr::Int(n, _) => CtValue::Int(*n),
            Expr::Float(f, _) => CtValue::Float(*f),
            Expr::Bool(b, _) => CtValue::Bool(*b),
            Expr::String(s, _) => CtValue::Str(self.interner.resolve(*s).to_string()),
            Expr::None(_) => CtValue::Null,
            Expr::Ident(sym, _) => {
                if let Some(v) = self.cache.bindings.get(sym) {
                    v.clone()
                } else if self.struct_fields.contains_key(sym)
                    || self.sum_variants.contains_key(sym)
                {
                    CtValue::TypeRef(*sym)
                } else {
                    CtValue::Null
                }
            }
            Expr::ComptimeRef { name, .. } => self
                .cache
                .bindings
                .get(name)
                .cloned()
                .unwrap_or(CtValue::Null),
            Expr::Quote { body, .. } => CtValue::Code((**body).clone()),
            Expr::Array(xs, _) => CtValue::Array(xs.iter().map(|e| self.eval_expr(e)).collect()),
            Expr::Binary {
                op, left, right, ..
            } => {
                let l = self.eval_expr(left);
                let r = self.eval_expr(right);
                eval_binop(*op, &l, &r)
            }
            Expr::Unary { op, expr, .. } => {
                let v = self.eval_expr(expr);
                match op {
                    UnOp::Neg => CtValue::Int(-v.as_int()),
                    UnOp::Not => CtValue::Bool(!v.as_bool()),
                    UnOp::BitNot => CtValue::Int(!v.as_int()),
                    UnOp::MutRef => v,
                }
            }
            Expr::ComptimeBlock { body, .. } => self.eval_block_value(body),
            Expr::ComptimeIf {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if self.eval_expr(cond).as_bool() {
                    self.eval_block_value(then_block)
                } else if let Some(eb) = else_block {
                    self.eval_block_value(eb)
                } else {
                    CtValue::Null
                }
            }
            Expr::ComptimeEach {
                name, iter, body, ..
            } => {
                let list = self.eval_expr(iter);
                let mut acc = Vec::new();
                if let CtValue::Array(xs) = list {
                    for x in xs {
                        self.cache.bindings.insert(*name, x);
                        acc.push(self.eval_block_value(body));
                    }
                }
                CtValue::Array(acc)
            }
            Expr::GoalType(_) => CtValue::Str("goal".into()),
            Expr::Field { base, field, .. } => {
                let b = self.eval_expr(base);
                let fname = self.interner.resolve(*field);
                match b {
                    CtValue::TypeRef(ty) => {
                        if fname == "fields"
                            && let Some(fields) = self.struct_fields.get(&ty)
                        {
                            return CtValue::Array(
                                fields
                                    .iter()
                                    .map(|(n, p)| {
                                        CtValue::Array(vec![
                                            CtValue::Str(self.interner.resolve(*n).to_string()),
                                            CtValue::Bool(*p),
                                        ])
                                    })
                                    .collect(),
                            );
                        }
                        if fname == "size" {
                            return CtValue::Int(
                                self.struct_sizes.get(&ty).copied().unwrap_or(0) as i64
                            );
                        }
                        if fname == "variants"
                            && let Some(vs) = self.sum_variants.get(&ty)
                        {
                            return CtValue::Array(
                                vs.iter()
                                    .map(|n| CtValue::Str(self.interner.resolve(*n).to_string()))
                                    .collect(),
                            );
                        }
                        if fname == "align" {
                            return CtValue::Int(8);
                        }
                        CtValue::Null
                    }
                    CtValue::Array(xs) if fname == "len" => CtValue::Int(xs.len() as i64),
                    _ => CtValue::Null,
                }
            }
            Expr::Range { start, end, .. } => {
                let s = self.eval_expr(start).as_int();
                let e = self.eval_expr(end).as_int();
                let mut xs = Vec::new();
                let mut i = s;
                while i < e {
                    xs.push(CtValue::Int(i));
                    i += 1;
                }
                CtValue::Array(xs)
            }
            Expr::Pipeline {
                base, method, args, ..
            } => {
                let method_name = self.interner.resolve(*method);
                // Helper: parts.join_code('{ ", " })
                if method_name == "join_code" || method_name == "join" {
                    let base_v = self.eval_expr(base);
                    let sep = args
                        .first()
                        .map(|a| self.eval_expr(a))
                        .unwrap_or(CtValue::Str(",".into()));
                    let sep_s = match sep {
                        CtValue::Code(Expr::String(s, _)) => self.interner.resolve(s).to_string(),
                        CtValue::Str(s) => s,
                        CtValue::Code(e) => match self.eval_expr(&e) {
                            CtValue::Str(s) => s,
                            _ => ",".into(),
                        },
                        _ => ",".into(),
                    };
                    if let CtValue::Array(xs) = base_v {
                        let mut out = String::new();
                        for (i, x) in xs.iter().enumerate() {
                            if i > 0 {
                                out.push_str(&sep_s);
                            }
                            out.push_str(&x.as_str());
                        }
                        return CtValue::Str(out);
                    }
                }
                self.eval_expr(base)
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if self.eval_expr(cond).as_bool() {
                    self.eval_block_value(then_block)
                } else if let Some(eb) = else_block {
                    self.eval_block_value(eb)
                } else {
                    CtValue::Null
                }
            }
            Expr::Block(b) => self.eval_block_value(b),
            Expr::Call { callee, args, .. } => {
                // Builtins used in comptime table generation.
                if let Expr::Ident(name, _) = callee.as_ref() {
                    let n = self.interner.resolve(*name).to_string();
                    if n == "sin_approx" || n == "cos_approx" {
                        let x = args
                            .first()
                            .map(|a| self.eval_expr(a).as_float())
                            .unwrap_or(0.0);
                        let y = if n.starts_with('s') { x.sin() } else { x.cos() };
                        return CtValue::Float(y);
                    }
                }
                CtValue::Null
            }
            _ => CtValue::Null,
        }
    }

    fn eval_block_value(&mut self, block: &Block) -> CtValue {
        let mut last = CtValue::Null;
        for stmt in &block.stmts {
            match stmt {
                Stmt::Binding(b) => {
                    let v = self.eval_expr(&b.value);
                    self.cache.bindings.insert(b.name, v.clone());
                    last = v;
                }
                Stmt::Expr(e) => {
                    last = self.eval_expr(e);
                }
                Stmt::Return(Some(e), _) => {
                    return self.eval_expr(e);
                }
                Stmt::Comptime(cs) => {
                    let produced = self.eval_comptime_stmt(cs.clone());
                    for s in produced {
                        if let Stmt::Binding(b) = s {
                            let v = self.eval_expr(&b.value);
                            self.cache.bindings.insert(b.name, v.clone());
                            last = v;
                        }
                    }
                }
                Stmt::Each { iter, body, .. } => {
                    let list = self.eval_expr(iter);
                    if let CtValue::Array(xs) = list {
                        for (i, x) in xs.into_iter().enumerate() {
                            let it = self.interner.intern("it");
                            let index = self.interner.intern("index");
                            self.cache.bindings.insert(it, x);
                            self.cache.bindings.insert(index, CtValue::Int(i as i64));
                            last = self.eval_block_value(body);
                        }
                    }
                }
                _ => {}
            }
        }
        last
    }
}

fn eval_binop(op: BinOp, l: &CtValue, r: &CtValue) -> CtValue {
    match op {
        BinOp::Add => match (l, r) {
            (CtValue::Str(a), b) => CtValue::Str(format!("{}{}", a, b.as_str())),
            (a, CtValue::Str(b)) => CtValue::Str(format!("{}{}", a.as_str(), b)),
            (CtValue::Float(_), _) | (_, CtValue::Float(_)) => {
                CtValue::Float(l.as_float() + r.as_float())
            }
            _ => CtValue::Int(l.as_int() + r.as_int()),
        },
        BinOp::Sub => CtValue::Int(l.as_int() - r.as_int()),
        BinOp::Mul => {
            if matches!(l, CtValue::Float(_)) || matches!(r, CtValue::Float(_)) {
                CtValue::Float(l.as_float() * r.as_float())
            } else {
                CtValue::Int(l.as_int() * r.as_int())
            }
        }
        BinOp::Div => {
            if matches!(l, CtValue::Float(_)) || matches!(r, CtValue::Float(_)) {
                CtValue::Float(l.as_float() / r.as_float().max(1e-12))
            } else {
                let d = r.as_int();
                CtValue::Int(if d == 0 {
                    0
                } else {
                    l.as_int().wrapping_div(d)
                })
            }
        }
        BinOp::Rem => {
            let d = r.as_int();
            CtValue::Int(if d == 0 {
                0
            } else {
                l.as_int().wrapping_rem(d)
            })
        }
        BinOp::Eq => CtValue::Bool(l.as_int() == r.as_int() || l.as_str() == r.as_str()),
        BinOp::Ne => CtValue::Bool(l.as_int() != r.as_int()),
        BinOp::Lt => CtValue::Bool(l.as_int() < r.as_int()),
        BinOp::Le => CtValue::Bool(l.as_int() <= r.as_int()),
        BinOp::Gt => CtValue::Bool(l.as_int() > r.as_int()),
        BinOp::Ge => CtValue::Bool(l.as_int() >= r.as_int()),
        BinOp::And => CtValue::Bool(l.as_bool() && r.as_bool()),
        BinOp::Or => CtValue::Bool(l.as_bool() || r.as_bool()),
        BinOp::BitAnd => CtValue::Int(l.as_int() & r.as_int()),
        BinOp::BitOr => CtValue::Int(l.as_int() | r.as_int()),
        BinOp::BitXor => CtValue::Int(l.as_int() ^ r.as_int()),
        BinOp::Shl => CtValue::Int(l.as_int().wrapping_shl((r.as_int() as u32) & 63)),
        BinOp::Shr => CtValue::Int(l.as_int().wrapping_shr((r.as_int() as u32) & 63)),
        BinOp::UShr => {
            let a = l.as_int() as u64;
            CtValue::Int((a.wrapping_shr((r.as_int() as u32) & 63)) as i64)
        }
    }
}

impl CtValue {
    fn as_float(&self) -> f64 {
        match self {
            CtValue::Float(f) => *f,
            CtValue::Int(n) => *n as f64,
            _ => 0.0,
        }
    }
}

/// Stable key for comparing parameter type annotations (for `#symmetric`).
fn type_expr_key(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Named(n, _) => format!("N{}", n.index()),
        TypeExpr::Optional(inner, _) => format!("?{}", type_expr_key(inner)),
        TypeExpr::Error(inner, _) => format!("!{}", type_expr_key(inner)),
        TypeExpr::Path(parts, _) => {
            let ids: Vec<_> = parts.iter().map(|p| p.index().to_string()).collect();
            format!("P{}", ids.join("."))
        }
        TypeExpr::Chan(inner, _) => format!("C{}", type_expr_key(inner)),
        TypeExpr::Type(_) => "type".into(),
        TypeExpr::Code(inner, _) => format!("Code{}", type_expr_key(inner)),
        TypeExpr::Apply { name, args, .. } => {
            let args = args.iter().map(type_expr_key).collect::<Vec<_>>().join(",");
            format!("A{}[{args}]", name.index())
        }
    }
}

fn substitute_it_index(block: &mut Block, i: i64, span: Span) {
    for stmt in &mut block.stmts {
        subst_stmt(stmt, i, span);
    }
}

fn subst_stmt(stmt: &mut Stmt, i: i64, span: Span) {
    match stmt {
        Stmt::Expr(e) | Stmt::Fail(e, _) => subst_expr(e, i, span),
        Stmt::Panic(Some(e), _) => subst_expr(e, i, span),
        Stmt::Return(Some(e), _) => subst_expr(e, i, span),
        Stmt::Binding(b) => subst_expr(&mut b.value, i, span),
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            subst_expr(cond, i, span);
            for s in &mut else_block.stmts {
                subst_stmt(s, i, span);
            }
        }
        Stmt::Each { body, iter, .. } => {
            subst_expr(iter, i, span);
            for s in &mut body.stmts {
                subst_stmt(s, i, span);
            }
        }
        _ => {}
    }
}

fn subst_expr(expr: &mut Expr, i: i64, span: Span) {
    match expr {
        Expr::It(_) => *expr = Expr::Int(i, span),
        Expr::IndexIdent(_) => *expr = Expr::Int(i, span),
        Expr::Binary { left, right, .. } => {
            subst_expr(left, i, span);
            subst_expr(right, i, span);
        }
        Expr::Unary { expr, .. } => subst_expr(expr, i, span),
        Expr::Call { callee, args, .. } => {
            subst_expr(callee, i, span);
            for a in args {
                subst_expr(a, i, span);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            subst_expr(base, i, span);
            for a in args {
                subst_expr(a, i, span);
            }
        }
        Expr::Field { base, .. } => subst_expr(base, i, span),
        Expr::Assign { target, value, .. } => {
            subst_expr(target, i, span);
            subst_expr(value, i, span);
        }
        Expr::Array(xs, _) => {
            for e in xs {
                subst_expr(e, i, span);
            }
        }
        _ => {}
    }
}

pub fn span_key(span: Span) -> u64 {
    let packed = ((span.lo.0 as u64) << 32) | span.hi.0 as u64;
    // File 0 keeps the historical key. Any other module mixes in its index so
    // the same byte range in two files does not share an expression type.
    packed ^ (span.file as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Hash source text for incremental LSP comptime cache keys.
pub fn source_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn expands_comptime_binding_and_unroll_attr() {
        let src = r#"
#N = 4
//# unroll(4)
f() {
    for 0..4 {
        x = it
    }
}
"#;
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let mut module = parser.parse_module();
        let mut meta = MetaEngine::new(&mut interner, &mut diags);
        meta.expand_module(&mut module);
        assert!(meta.cache.bindings.contains_key(&interner.intern("N")));
    }

    #[test]
    fn span_key_packs_lo_hi_for_expr_type_map() {
        let span = Span::from_offsets(12, 34);
        assert_eq!(span_key(span), (12u64 << 32) | 34);
        assert_ne!(span_key(span), span_key(Span::from_offsets(12, 35)));
        assert_ne!(span_key(span), span_key(span.with_file(1)));
    }
}
