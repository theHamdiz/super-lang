//! AST → bytecode compiler with destination-driven construction support.

use crate::bytecode::{CTag, Chunk, FunctionChunk, Op, TestChunk};
use crate::opt::{self, OptMeta};
use crate::value::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use super_core::{Diagnostics, Interner, Span, SymId};
use super_frontend::ast::*;
use super_frontend::sema::{TypeKind, TypedModule};
use super_frontend::{Lexer, Parser};

pub struct Compiler<'a> {
    interner: &'a mut Interner,
    /// Used for C-ABI field ordering on named struct literals and call arity checks.
    typed: &'a TypedModule,
    strip_tests: bool,
    chunk: Chunk,
    locals: Vec<HashMap<SymId, u16>>,
    /// Local / param types for owner-aware UFCS (`c.db.insert` when `insert` is ambiguous).
    local_types: Vec<HashMap<SymId, super_core::TypeId>>,
    local_count: u16,
    /// Recycled virtual registers freed when a scope ends (liveness recycling).
    free_regs: Vec<u16>,
    loop_depth: u16,
    /// Patch sites for `break` / `continue` in the current loop.
    /// `pops_iter`: each/for must `IterPop` before break-jump; while/loop must not.
    break_stack: Vec<LoopFrame>,
    /// Directory of the entry `.sr` file. `embed` also searches the package
    /// root and its `assets/` directory when a `Super.toml` is above this path.
    base_dir: PathBuf,
    /// AST-level optimizer metadata (BCE spans, SROA maps, …).
    opt_meta: OptMeta,
    /// Next index into `opt_meta.escape_classes` for MakeMap/MakeStruct/MakeSum.
    escape_cursor: usize,
    /// True while compiling a function body that mentions request-enter/promote.
    in_request_escape_fn: bool,
    /// Module functions reachable from HTTP route lambdas (interprocedural escape).
    request_reachable_fns: HashSet<String>,
    /// True while compiling args of `App.get` / `post` / … so nested lambdas
    /// are tagged as request-scoped escape units.
    compiling_route_handler_arg: bool,
    /// Struct field defaults, so `Board{}` fills `title: str = "ops"`.
    field_defaults: HashMap<SymId, HashMap<SymId, Expr>>,
    /// Declared struct return type of the function being compiled (`f() : User => .{…}`).
    ret_struct: Option<SymId>,
    /// Expected struct for the very next `compile_expr` (consumed on entry), so an inferred
    /// `.{…}` in return / typed-binding position is built as that struct, defaults included.
    infer_hint: Option<SymId>,
    /// Lambda bodies compiled out-of-line after the enclosing function so
    /// `[entry, next.entry)` ranges stay contiguous for the C23 emitter.
    pending_lambdas: Vec<PendingLambda>,
    /// `spawn` thunks — same out-of-line rule as lambdas. Inlining a Jump + body
    /// mid-function made `fn_range` / boxed C emit cut the parent at the thunk
    /// entry and emit `goto L_after` with no label (`await spawn` / `spawn`+`await`).
    pending_spawns: Vec<PendingSpawn>,
    /// Free functions with trait-constrained params — specialized per concrete arg types.
    trait_templates: HashMap<(SymId, Option<SymId>), Function>,
    /// `(name, concrete owner SymIds per arg)` → specialized function index.
    /// Struct/sum receivers share `CTag::Dyn`, so owners distinguish Dense vs Diag.
    trait_specs: HashMap<(SymId, Vec<Option<SymId>>), usize>,
    /// Trait specializations waiting for out-of-line body emission.
    pending_trait_specs: Vec<PendingTraitSpec>,
}

struct PendingTraitSpec {
    idx: usize,
    template: Function,
    owners: Vec<Option<SymId>>,
    tags: Vec<CTag>,
}

struct LoopFrame {
    breaks: Vec<u32>,
    continues: Vec<u32>,
    pops_iter: bool,
}

/// Deferred `|params| body` waiting for out-of-line emission.
struct PendingLambda {
    idx: u16,
    params: Vec<Param>,
    body: FnBody,
    captures: Vec<LambdaCapture>,
    /// True for HTTP route handlers (`App.get` / `#routes`) — escape tagging
    /// must match [`opt::collect_request_alloc_sites`] request-scoped units.
    request_scoped: bool,
}

/// Deferred `spawn` body; compiled with a snapshot of the parent's locals so
/// channels / bindings capture the same slots the fiber seeds at runtime.
struct PendingSpawn {
    idx: u16,
    body: Expr,
    locals: Vec<HashMap<SymId, u16>>,
    local_types: Vec<HashMap<SymId, super_core::TypeId>>,
    local_count: u16,
}

impl<'a> Compiler<'a> {
    pub fn new(interner: &'a mut Interner, typed: &'a TypedModule, strip_tests: bool) -> Self {
        Self::with_base(interner, typed, strip_tests, PathBuf::from("."))
    }

    pub fn with_base(
        interner: &'a mut Interner,
        typed: &'a TypedModule,
        strip_tests: bool,
        base_dir: impl Into<PathBuf>,
    ) -> Self {
        for s in [
            "main",
            "it",
            "index",
            "err",
            "apply",
            "__is_err",
            "print",
            "println",
            "assert_eq",
            "__slice",
        ] {
            interner.intern(s);
        }
        Self {
            interner,
            typed,
            strip_tests,
            chunk: Chunk::default(),
            locals: vec![HashMap::new()],
            local_types: vec![HashMap::new()],
            local_count: 0,
            free_regs: Vec::new(),
            loop_depth: 0,
            break_stack: Vec::new(),
            base_dir: base_dir.into(),
            opt_meta: OptMeta::default(),
            escape_cursor: 0,
            in_request_escape_fn: false,
            request_reachable_fns: HashSet::new(),
            compiling_route_handler_arg: false,
            field_defaults: HashMap::new(),
            ret_struct: None,
            infer_hint: None,
            pending_lambdas: Vec::new(),
            pending_spawns: Vec::new(),
            trait_templates: HashMap::new(),
            trait_specs: HashMap::new(),
            pending_trait_specs: Vec::new(),
        }
    }

    fn collect_field_defaults(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Struct(s) = item else { continue };
            let mut defaults = HashMap::new();
            for field in &s.fields {
                if let Some(expr) = &field.default {
                    defaults.insert(field.name, expr.clone());
                }
            }
            if !defaults.is_empty() {
                self.field_defaults.insert(s.name, defaults);
            }
        }
    }

    /// Attach optimizer metadata produced by [`opt::optimize_module`].
    pub fn with_opt_meta(mut self, meta: OptMeta) -> Self {
        self.opt_meta = meta;
        self.escape_cursor = 0;
        self
    }

    /// Record the escape-analysis birth region for the Make* op at `pc`.
    /// Only consumes classifier output inside request-enter functions so the
    /// cursor stays aligned with [`opt::collect_request_alloc_sites`].
    fn tag_escape_birth(&mut self, pc: u32) {
        if !self.in_request_escape_fn {
            return;
        }
        if let Some(site) = self.opt_meta.escape_classes.get(self.escape_cursor) {
            let region = opt::escape_birth_region(site.class);
            self.chunk.escape_birth.push((pc, region));
            self.escape_cursor += 1;
        }
    }

    pub fn compile(mut self, module: &Module) -> Chunk {
        self.collect_field_defaults(module);
        self.request_reachable_fns = opt::request_reachable_fn_names(module, self.interner);
        // Predeclare free functions and type methods (UFCS / chaining).
        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.predeclare_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                _ => {}
            }
        }

        // Function/route bodies are not top-level entry; jump over them.
        let skip_fns = self.chunk.push(Op::Jump(0));

        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.compile_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                _ => {}
            }
        }

        let top = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(skip_fns, top);

        // Emit top-level bindings and state
        for item in &module.items {
            match item {
                Item::Binding(b) => {
                    self.compile_binding_value(b);
                }
                Item::State(s) => {
                    self.compile_expr(&s.value);
                    self.chunk.push(Op::StoreGlobal(s.name));
                }
                Item::Test(t) if !self.strip_tests => {
                    let _ = t;
                }
                _ => {}
            }
        }

        // Bindings may queue lambdas (rare); emit any leftovers before main.
        self.flush_pending_lambdas();

        // Call main if present (embed mode uses `compile_embed` instead).
        let main = self.interner.intern("main");
        if self.chunk.functions.iter().any(|f| f.name == main) {
            let idx = self
                .chunk
                .functions
                .iter()
                .position(|f| f.name == main)
                .unwrap() as u16;
            self.chunk.push(Op::Call(idx, 0));
        }
        self.chunk.push(Op::Halt);
        opt::peephole(&mut self.chunk);
        self.chunk
    }

    /// Compile a module for embedding: top-level bindings run, `main` is not auto-called.
    pub fn compile_embed(mut self, module: &Module) -> Chunk {
        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.predeclare_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                _ => {}
            }
        }

        let skip_fns = self.chunk.push(Op::Jump(0));
        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.compile_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                _ => {}
            }
        }
        let top = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(skip_fns, top);

        for item in &module.items {
            match item {
                Item::Binding(b) => {
                    self.compile_binding_value(b);
                }
                Item::State(s) => {
                    self.compile_expr(&s.value);
                    self.chunk.push(Op::StoreGlobal(s.name));
                }
                _ => {}
            }
        }
        // Top-level bindings may close over lambdas; emit those bodies before Halt.
        self.flush_pending_lambdas();
        self.chunk.push(Op::Halt);
        opt::peephole(&mut self.chunk);
        self.chunk
    }

    /// Bindings whose value is a lambda are registered under the binding name
    /// so `#get` / direct calls resolve like ordinary functions.
    fn compile_binding_value(&mut self, b: &Binding) {
        if let Expr::Lambda {
            params,
            body,
            captures,
            span,
            ..
        } = &b.value
        {
            let caps = if captures.is_empty() {
                self.discover_captures(params, body.as_ref(), *span)
            } else {
                captures.clone()
            };
            let idx = self.compile_lambda_fn(
                Some(b.name),
                params,
                body.as_ref(),
                &caps,
                self.compiling_route_handler_arg,
            );
            for cap in &caps {
                if let Some(slot) = self.find_local(cap.name) {
                    self.chunk.push(Op::LoadLocal(slot));
                } else {
                    self.chunk.push(Op::LoadGlobal(cap.name));
                }
            }
            if caps.is_empty() {
                self.chunk.push(Op::MakeFn(idx));
            } else {
                self.chunk.push(Op::MakeClosure(idx, caps.len() as u8));
            }
            self.chunk.push(Op::StoreGlobal(b.name));
            return;
        }
        self.compile_expr(&b.value);
        self.chunk.push(Op::StoreGlobal(b.name));
    }

    fn predeclare_function(&mut self, f: &Function) {
        let (param_tags, local_tags, ret_tag) = self.function_tags(f);
        self.chunk.functions.push(FunctionChunk {
            name: f.name,
            owner: f.owner,
            arity: f.params.len() as u8,
            local_count: f.params.len() as u16,
            entry: 0,
            param_names: f.params.iter().map(|p| p.name).collect(),
            mut_params: f.params.iter().map(|p| p.mutable).collect(),
            always_inline: function_always_inline(f),
            live_intervals: Vec::new(),
            param_tags,
            local_tags,
            ret_tag,
        });
    }

    /// Resolve a free function or type method. Prefers `(owner, name)` when owner is known.
    fn is_module_name(&self, name: SymId) -> bool {
        // A local named `cmd` shadows `use std.cli.cmd`. The module binding stays
        // in the typed module, so the call `cmd.parse(argv)` must follow the local.
        if self.find_local(name).is_some() {
            return false;
        }
        self.typed.bindings.get(&name).is_some_and(|info| {
            matches!(self.typed.types.get_type(info.ty), TypeKind::Module { .. })
        })
    }

    fn canonical(&self, name: SymId) -> SymId {
        self.typed.aliases.get(&name).copied().unwrap_or(name)
    }

    fn find_fn_idx(&self, name: SymId, owner: Option<SymId>) -> Option<usize> {
        self.find_fn_idx_arity(name, owner, None, false, None)
    }

    /// C-ABI tag of a call-site argument (literals first, then sema `expr_types`).
    fn expr_c_tag(&self, expr: &Expr) -> CTag {
        match expr {
            Expr::Unary {
                op: UnOp::MutRef,
                expr: inner,
                ..
            } => self.expr_c_tag(inner),
            Expr::Float(..) => CTag::F64,
            Expr::Int(..) => CTag::I64,
            Expr::Bool(..) => CTag::Bool,
            Expr::String(..) => CTag::Str,
            _ => self
                .type_of_expr(expr)
                .map(|t| self.c_tag_of_type(t))
                .unwrap_or(CTag::Dyn),
        }
    }

    /// Rank a `FuncFamily` candidate against concrete argument tags.
    /// Exact tag match scores highest; `Dyn` is a soft match; incompatible → `None`.
    fn overload_score(&self, fi: usize, arg_tags: &[CTag]) -> Option<i32> {
        let f = &self.chunk.functions[fi];
        if f.arity as usize != arg_tags.len() {
            return None;
        }
        let mut score = 0i32;
        for (i, at) in arg_tags.iter().enumerate() {
            let pt = f.param_tags.get(i).copied().unwrap_or(CTag::Dyn);
            if pt == *at && pt != CTag::Dyn {
                score += 4;
            } else if pt == CTag::Dyn || *at == CTag::Dyn {
                score += 1;
            } else {
                let numeric = |t: CTag| matches!(t, CTag::I64 | CTag::F64);
                if numeric(pt) && numeric(*at) {
                    score += 2;
                } else {
                    return None;
                }
            }
        }
        Some(score)
    }

    /// Pick the highest-scoring overload; ties keep declaration order.
    fn pick_best_overload(&self, candidates: &[usize], arg_tags: &[CTag]) -> Option<usize> {
        if candidates.is_empty() {
            return None;
        }
        if arg_tags.is_empty() {
            return candidates.first().copied();
        }
        let mut best: Option<(i32, usize)> = None;
        for &fi in candidates {
            let Some(score) = self.overload_score(fi, arg_tags) else {
                continue;
            };
            match best {
                None => best = Some((score, fi)),
                Some((bs, _)) if score > bs => best = Some((score, fi)),
                _ => {}
            }
        }
        best.map(|(_, fi)| fi)
            .or_else(|| candidates.first().copied())
    }

    fn param_is_trait(&self, te: &TypeExpr) -> bool {
        match te {
            TypeExpr::Named(n, _) => self.typed.traits.contains_key(n),
            TypeExpr::Path(path, _) => path
                .last()
                .is_some_and(|n| self.typed.traits.contains_key(n)),
            _ => false,
        }
    }

    fn fn_has_trait_params(&self, f: &Function) -> bool {
        f.params.iter().any(|p| self.param_is_trait(&p.ty))
    }

    /// Specialize a trait-parameter template for concrete argument types at a call site.
    fn ensure_trait_specialization(&mut self, name: SymId, args: &[Expr]) -> Option<usize> {
        let template = self.trait_templates.get(&(name, None))?.clone();
        if !self.fn_has_trait_params(&template) {
            return None;
        }
        let tags: Vec<CTag> = args.iter().map(|e| self.expr_c_tag(e)).collect();
        if tags.len() != template.params.len() {
            return None;
        }
        let owners: Vec<Option<SymId>> = args.iter().map(|e| self.expr_struct_owner(e)).collect();
        let key = (name, owners.clone());
        if let Some(&idx) = self.trait_specs.get(&key) {
            return Some(idx);
        }
        let ready = owners
            .iter()
            .zip(template.params.iter())
            .all(|(o, p)| !self.param_is_trait(&p.ty) || o.is_some())
            && owners
                .iter()
                .zip(template.params.iter())
                .any(|(o, p)| self.param_is_trait(&p.ty) && o.is_some());
        if !ready {
            return None;
        }
        let idx = self.chunk.functions.len();
        self.chunk.functions.push(FunctionChunk {
            name: template.name,
            owner: template.owner,
            arity: template.params.len() as u8,
            local_count: template.params.len() as u16,
            entry: 0,
            param_names: template.params.iter().map(|p| p.name).collect(),
            mut_params: template.params.iter().map(|p| p.mutable).collect(),
            always_inline: function_always_inline(&template),
            live_intervals: Vec::new(),
            param_tags: tags.clone(),
            local_tags: tags.clone(),
            ret_tag: template
                .ret
                .as_ref()
                .map(|te| self.type_expr_c_tag(te))
                .unwrap_or(CTag::Dyn),
        });
        self.trait_specs.insert(key, idx);
        self.pending_trait_specs.push(PendingTraitSpec {
            idx,
            template,
            owners,
            tags,
        });
        Some(idx)
    }

    fn flush_pending_trait_specs(&mut self) {
        while let Some(spec) = self.pending_trait_specs.pop() {
            let entry = self.chunk.ops.len() as u32;
            if let Some(fc) = self.chunk.functions.get_mut(spec.idx) {
                fc.entry = entry;
                fc.param_tags = spec.tags.clone();
                fc.local_tags = spec.tags.clone();
            }
            self.push_scope();
            self.local_count = 0;
            self.free_regs.clear();
            for (i, p) in spec.template.params.iter().enumerate() {
                let slot = self.local_count;
                self.local_count += 1;
                self.locals.last_mut().unwrap().insert(p.name, slot);
                if let Some(owner) = spec.owners.get(i).copied().flatten()
                    && let Some(tid) = self
                        .typed
                        .structs
                        .get(&owner)
                        .or_else(|| self.typed.sums.get(&owner))
                {
                    self.set_local_type(p.name, *tid);
                    continue;
                }
                if let Some(tid) = self.resolve_param_type(&p.ty) {
                    self.set_local_type(p.name, tid);
                }
            }
            let saved_ret = self.ret_struct;
            self.ret_struct = self.struct_of_type(spec.template.ret.as_ref());
            match &spec.template.body {
                FnBody::Expr(e) => {
                    self.compile_expecting(e, self.ret_struct);
                    self.chunk.push(Op::Return);
                }
                FnBody::Block(b) => self.compile_block_as_return(b),
            }
            self.ret_struct = saved_ret;
            if let Some(fc) = self.chunk.functions.get_mut(spec.idx) {
                fc.local_count = self.local_count.max(fc.local_count);
                let n = fc.local_count as usize;
                if fc.local_tags.len() < n {
                    fc.local_tags.resize(n, CTag::Dyn);
                }
            }
            self.pop_scope();
        }
    }

    /// Locate the predeclared chunk slot for `f` by name/owner/`param_tags`.
    /// Same-name overloads and `#symmetric` mirrors each own a distinct slot.
    fn function_chunk_slot(&mut self, f: &Function) -> Option<usize> {
        let (tags, _, _) = self.function_tags(f);
        if let Some(i) = self
            .chunk
            .functions
            .iter()
            .position(|x| x.name == f.name && x.owner == f.owner && x.param_tags == tags)
        {
            return Some(i);
        }
        // Untyped / Dyn-tagged duplicates: first still-unpatched stub.
        self.chunk
            .functions
            .iter()
            .position(|x| x.name == f.name && x.owner == f.owner && x.entry == 0)
    }

    /// `ufcs_args` = call-site args excluding the UFCS receiver (`recv.m(a,b)` → 2).
    /// When set, ambiguous short names are narrowed by arity (`Store.has` vs `World.has`).
    /// `prefer_free` is for bare `reader(data)` calls that share a name with `ByteBuf.reader`.
    /// `arg_exprs` ranks same-arity `FuncFamily` / `#symmetric` mirrors by parameter tags.
    fn find_fn_idx_arity(
        &self,
        name: SymId,
        owner: Option<SymId>,
        ufcs_args: Option<usize>,
        prefer_free: bool,
        arg_exprs: Option<&[Expr]>,
    ) -> Option<usize> {
        let arg_tags: Option<Vec<CTag>> =
            arg_exprs.map(|args| args.iter().map(|e| self.expr_c_tag(e)).collect());
        if let Some(o) = owner {
            let owned: Vec<usize> = self
                .chunk
                .functions
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name == name && f.owner == Some(o))
                .map(|(i, _)| i)
                .collect();
            if let Some(tags) = arg_tags.as_deref() {
                if let Some(idx) = self.pick_best_overload(&owned, tags) {
                    return Some(idx);
                }
            } else if let Some(idx) = owned.first().copied() {
                return Some(idx);
            }
            // UFCS fallback: free `+name(@recv: T, …)` with matching arity.
            if let Some(n_args) = ufcs_args {
                let want_arity = n_args + 1;
                let free: Vec<usize> = self
                    .chunk
                    .functions
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        f.name == name && f.owner.is_none() && f.arity as usize == want_arity
                    })
                    .map(|(i, _)| i)
                    .collect();
                if let Some(tags) = arg_tags.as_deref() {
                    return self.pick_best_overload(&free, tags);
                }
                if free.len() == 1 {
                    return Some(free[0]);
                }
            }
            return None;
        }
        let mut matches: Vec<usize> = self
            .chunk
            .functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name == name)
            .map(|(i, _)| i)
            .collect();
        if matches.len() <= 1 {
            return matches.first().copied();
        }
        if prefer_free {
            let free: Vec<usize> = matches
                .iter()
                .copied()
                .filter(|&i| self.chunk.functions[i].owner.is_none())
                .collect();
            if free.len() == 1 {
                return Some(free[0]);
            }
            if free.len() > 1 {
                matches = free;
            }
        }
        if let Some(n_args) = ufcs_args {
            let by_arity: Vec<usize> = matches
                .iter()
                .copied()
                .filter(|&i| {
                    let arity = self.chunk.functions[i].arity as usize;
                    arity == n_args + 1 || arity == n_args
                })
                .collect();
            if by_arity.len() == 1 {
                return Some(by_arity[0]);
            }
            if !by_arity.is_empty() {
                if let Some(tags) = arg_tags.as_deref()
                    && let Some(idx) = self.pick_best_overload(&by_arity, tags)
                {
                    return Some(idx);
                }
                let free: Vec<usize> = by_arity
                    .iter()
                    .copied()
                    .filter(|&i| self.chunk.functions[i].owner.is_none())
                    .collect();
                return Some(free.first().copied().unwrap_or(by_arity[0]));
            }
        }
        if let Some(tags) = arg_tags.as_deref()
            && let Some(idx) = self.pick_best_overload(&matches, tags)
        {
            return Some(idx);
        }
        matches
            .iter()
            .copied()
            .find(|&i| self.chunk.functions[i].owner.is_none())
            .or_else(|| matches.first().copied())
    }

    /// Resolve `recv.method(args…)` ≡ `method(recv, args…)` for any receiver type.
    ///
    /// Order: (1) a free function whose first parameter is *exactly* typed like the
    /// receiver (`s.byte_len()` with `byte_len(s: str)`), (2) the owner-aware type
    /// method / unique-name lookup, (3) the unique arity-compatible free function
    /// (generic `_` receiver such as `a.len()`).
    fn resolve_ufcs(&self, base: &Expr, method: SymId, n_args: usize) -> Option<usize> {
        let owner = self.receiver_owner(base);
        let want_arity = n_args + 1;
        let free: Vec<usize> = if owner.is_none() {
            self.chunk
                .functions
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.name == method && f.owner.is_none() && f.arity as usize == want_arity
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        let recv_tag = match base {
            Expr::String(..) => CTag::Str,
            Expr::Float(..) => CTag::F64,
            Expr::Int(..) => CTag::I64,
            Expr::Bool(..) => CTag::Bool,
            _ => self
                .type_of_expr(base)
                .map(|t| self.c_tag_of_type(t))
                .unwrap_or(CTag::Dyn),
        };
        if recv_tag != CTag::Dyn {
            let exact: Vec<usize> = free
                .iter()
                .copied()
                .filter(|&i| self.chunk.functions[i].param_tags.first() == Some(&recv_tag))
                .collect();
            if exact.len() == 1 {
                return Some(exact[0]);
            }
        }
        if owner.is_none() {
            let mut with_recv = self
                .chunk
                .functions
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name == method && f.arity as usize == want_arity)
                .map(|(i, _)| i);
            if let (Some(only), None) = (with_recv.next(), with_recv.next()) {
                return Some(only);
            }
        }
        if let Some(idx) = self.find_fn_idx_arity(method, owner, Some(n_args), false, None) {
            // When the receiver owner is unknown, refuse an owned method that shares its
            // short name with other owners (e.g. `Cell.text` vs `StringBuilder.text` on Dyn).
            if owner.is_some()
                || self.chunk.functions[idx].owner.is_none()
                || !self.ufcs_name_is_multi_owner(method, want_arity)
            {
                return Some(idx);
            }
        }
        let compatible: Vec<usize> = free
            .into_iter()
            .filter(|&i| {
                let tag = self.chunk.functions[i]
                    .param_tags
                    .first()
                    .copied()
                    .unwrap_or(CTag::Dyn);
                let numeric = |t: CTag| matches!(t, CTag::I64 | CTag::F64);
                tag == CTag::Dyn
                    || recv_tag == CTag::Dyn
                    || tag == recv_tag
                    || (numeric(tag) && numeric(recv_tag))
            })
            .collect();
        (compatible.len() == 1).then(|| compatible[0])
    }

    /// True when ≥2 distinct type owners expose `method` at `want_arity` (ambiguous Dyn UFCS).
    fn ufcs_name_is_multi_owner(&self, method: SymId, want_arity: usize) -> bool {
        let mut owners = Vec::new();
        for f in &self.chunk.functions {
            if f.name != method || f.arity as usize != want_arity {
                continue;
            }
            let Some(o) = f.owner else {
                continue;
            };
            if !owners.contains(&o) {
                owners.push(o);
            }
            if owners.len() >= 2 {
                return true;
            }
        }
        false
    }

    fn method_has_self_receiver(&self, idx: usize) -> bool {
        self.chunk.functions[idx]
            .param_names
            .first()
            .is_some_and(|n| self.interner.resolve(*n) == "self")
    }

    fn lookup_qualified_fn(
        &self,
        owner: SymId,
        method: SymId,
    ) -> Option<&super_frontend::sema::FunctionInfo> {
        let want = format!(
            "{}.{}",
            self.interner.resolve(owner),
            self.interner.resolve(method)
        );
        self.typed.functions.iter().find_map(|(k, info)| {
            if self.interner.resolve(*k) == want {
                Some(info)
            } else {
                None
            }
        })
    }

    fn func_ret_type(
        &self,
        info: &super_frontend::sema::FunctionInfo,
    ) -> Option<super_core::TypeId> {
        match self.typed.types.get_type(info.ty) {
            TypeKind::Func { ret, .. } => Some(*ret),
            _ => None,
        }
    }

    fn type_of_expr(&self, expr: &Expr) -> Option<super_core::TypeId> {
        let key = super_frontend::meta::span_key(expr.span());
        if let Some(tid) = self.typed.expr_types.get(&key) {
            return Some(*tid);
        }
        match expr {
            Expr::Ident(name, _) => {
                for scope in self.local_types.iter().rev() {
                    if let Some(tid) = scope.get(name) {
                        return Some(*tid);
                    }
                }
                if let Some(info) = self.typed.bindings.get(name) {
                    return Some(info.ty);
                }
                self.typed.structs.get(name).copied()
            }
            Expr::Field { base, field, .. } => {
                let bt = self.type_of_expr(base)?;
                match self.typed.types.get_type(bt) {
                    TypeKind::Struct { fields, .. } => {
                        fields.iter().find(|f| f.name == *field).map(|f| f.ty)
                    }
                    _ => None,
                }
            }
            Expr::Unary {
                op: UnOp::MutRef,
                expr: inner,
                ..
            } => self.type_of_expr(inner),
            // `Set.new()` / `a.add(3)` — recover return type when expr_types miss.
            Expr::Pipeline { base, method, .. } => {
                if let Expr::Ident(ns, _) = base.as_ref()
                    && self.is_type_owner(*ns)
                {
                    if let Some(info) = self.lookup_qualified_fn(*ns, *method) {
                        return self.func_ret_type(info);
                    }
                    if self.interner.resolve(*method) == "new" {
                        return self.typed.structs.get(ns).copied();
                    }
                }
                let owner = self.expr_struct_owner(base)?;
                let info = self.lookup_qualified_fn(owner, *method)?;
                self.func_ret_type(info)
            }
            Expr::Call { callee, .. } => {
                if let Expr::Field { base, field, .. } = callee.as_ref()
                    && let Expr::Ident(ns, _) = base.as_ref()
                    && self.is_type_owner(*ns)
                {
                    if let Some(info) = self.lookup_qualified_fn(*ns, *field) {
                        return self.func_ret_type(info);
                    }
                    if self.interner.resolve(*field) == "new" {
                        return self.typed.structs.get(ns).copied();
                    }
                }
                None
            }
            // `x = Type.parse(...) or err { … }` / `or fallback` — unwrap ?/! so
            // UFCS can resolve ambiguous same-arity methods via receiver owner.
            Expr::Or { left, right, .. } => {
                if let Some(rt) = self.type_of_expr(right) {
                    return Some(rt);
                }
                let lt = self.type_of_expr(left)?;
                Some(match self.typed.types.get_type(lt) {
                    TypeKind::Optional(inner) | TypeKind::Error(inner) => *inner,
                    _ => lt,
                })
            }
            Expr::OrErr { left, .. } => {
                let lt = self.type_of_expr(left)?;
                Some(match self.typed.types.get_type(lt) {
                    TypeKind::Optional(inner) | TypeKind::Error(inner) => *inner,
                    _ => lt,
                })
            }
            Expr::StructLit { name, .. } => name.and_then(|n| self.typed.structs.get(&n).copied()),
            _ => None,
        }
    }

    fn expr_struct_owner(&self, expr: &Expr) -> Option<SymId> {
        let tid = self.type_of_expr(expr)?;
        match self.typed.types.get_type(tid) {
            TypeKind::Struct { name, .. } | TypeKind::Sum { name, .. } => Some(*name),
            TypeKind::Named(name) => Some(*name),
            _ => None,
        }
    }

    /// Map a sema type to the C value class the AOT emitter can unbox.
    fn c_tag_of_type(&self, tid: super_core::TypeId) -> CTag {
        match self.typed.types.get_type(tid) {
            TypeKind::I8
            | TypeKind::I16
            | TypeKind::I32
            | TypeKind::I64
            | TypeKind::I128
            | TypeKind::U8
            | TypeKind::U16
            | TypeKind::U32
            | TypeKind::U64 => CTag::I64,
            TypeKind::F64 | TypeKind::F32 => CTag::F64,
            TypeKind::Bool => CTag::Bool,
            TypeKind::Str => CTag::Str,
            TypeKind::Void => CTag::Unit,
            TypeKind::Array(elem) => match self.typed.types.get_type(*elem) {
                TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::I128
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64 => CTag::ArrI64,
                TypeKind::F64 | TypeKind::F32 => CTag::ArrF64,
                _ => CTag::Dyn,
            },
            _ => CTag::Dyn,
        }
    }

    /// Map an AST type annotation to a C-ABI tag (used for `FuncFamily` slot identity).
    fn type_expr_c_tag(&self, te: &TypeExpr) -> CTag {
        match te {
            TypeExpr::Named(name, _) => {
                let n = self.interner.resolve(*name);
                match n {
                    "i64" | "int" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32"
                    | "i128" => CTag::I64,
                    "f64" | "f32" | "float" => CTag::F64,
                    "bool" => CTag::Bool,
                    "str" | "string" => CTag::Str,
                    _ => self
                        .typed
                        .structs
                        .get(name)
                        .or_else(|| self.typed.sums.get(name))
                        .map(|tid| self.c_tag_of_type(*tid))
                        .unwrap_or(CTag::Dyn),
                }
            }
            TypeExpr::Path(path, _) => {
                let Some(name) = path.last() else {
                    return CTag::Dyn;
                };
                let n = self.interner.resolve(*name);
                match n {
                    "i64" | "int" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32"
                    | "i128" => CTag::I64,
                    "f64" | "f32" | "float" => CTag::F64,
                    "bool" => CTag::Bool,
                    "str" | "string" => CTag::Str,
                    _ => self
                        .typed
                        .structs
                        .get(name)
                        .or_else(|| self.typed.sums.get(name))
                        .map(|tid| self.c_tag_of_type(*tid))
                        .unwrap_or(CTag::Dyn),
                }
            }
            TypeExpr::Optional(inner, _) | TypeExpr::Error(inner, _) => self.type_expr_c_tag(inner),
            TypeExpr::Chan(_, _)
            | TypeExpr::Type(_)
            | TypeExpr::Code(_, _)
            | TypeExpr::Apply { .. } => CTag::Dyn,
        }
    }

    /// Declared `(param_tags, local_tags, ret_tag)` for `f`.
    ///
    /// Tags come from the AST parameter annotations first so same-name
    /// `FuncFamily` overloads and `#symmetric` mirrors each get a distinct
    /// chunk slot. `TypedModule.functions` only keeps the primary overload.
    fn function_tags(&self, f: &Function) -> (Vec<CTag>, Vec<CTag>, CTag) {
        let mut param_tags: Vec<CTag> = f
            .params
            .iter()
            .map(|p| self.type_expr_c_tag(&p.ty))
            .collect();
        // Fill Dyn holes from the matching `families` member when present.
        if let Some(family) = self.typed.families.get(&f.name) {
            let ast_key = param_tags.clone();
            if let Some(info) = family.iter().find(|info| {
                info.owner == f.owner
                    && info.params.len() == f.params.len()
                    && info.params.iter().enumerate().all(|(i, (_, tid, _))| {
                        let tag = self.c_tag_of_type(*tid);
                        ast_key.get(i) == Some(&CTag::Dyn) || ast_key.get(i) == Some(&tag)
                    })
            }) {
                for (i, (_, tid, _)) in info.params.iter().enumerate() {
                    if param_tags.get(i) == Some(&CTag::Dyn) {
                        param_tags[i] = self.c_tag_of_type(*tid);
                    }
                }
            }
        } else if param_tags.iter().all(|t| *t == CTag::Dyn)
            && let Some(info) = self
                .typed
                .functions
                .get(&f.name)
                .filter(|i| i.owner == f.owner)
                .or_else(|| f.owner.and_then(|o| self.lookup_qualified_fn(o, f.name)))
        {
            for (i, (_, tid, _)) in info.params.iter().enumerate() {
                if let Some(slot) = param_tags.get_mut(i) {
                    *slot = self.c_tag_of_type(*tid);
                }
            }
        }
        let ret_tag = f
            .ret
            .as_ref()
            .map(|te| self.type_expr_c_tag(te))
            .filter(|t| *t != CTag::Dyn)
            .or_else(|| {
                self.typed
                    .families
                    .get(&f.name)
                    .and_then(|fam| {
                        fam.iter().find(|info| {
                            info.owner == f.owner
                                && info.params.len() == f.params.len()
                                && info.params.iter().enumerate().all(|(i, (_, tid, _))| {
                                    self.c_tag_of_type(*tid)
                                        == param_tags.get(i).copied().unwrap_or(CTag::Dyn)
                                })
                        })
                    })
                    .or_else(|| {
                        self.typed
                            .functions
                            .get(&f.name)
                            .filter(|i| i.owner == f.owner)
                    })
                    .and_then(|i| self.func_ret_type(i))
                    .map(|t| self.c_tag_of_type(t))
            })
            .unwrap_or(CTag::Dyn);
        let mut local_tags = param_tags.clone();
        local_tags.resize(f.params.len().max(local_tags.len()), CTag::Dyn);
        (param_tags, local_tags, ret_tag)
    }

    fn resolve_param_type(&self, te: &TypeExpr) -> Option<super_core::TypeId> {
        match te {
            TypeExpr::Named(name, _) => self
                .typed
                .structs
                .get(name)
                .copied()
                .or_else(|| self.typed.sums.get(name).copied()),
            TypeExpr::Path(path, _) => path.last().and_then(|n| self.typed.structs.get(n).copied()),
            _ => None,
        }
    }

    pub fn compile_tests(mut self, module: &Module) -> Chunk {
        self.collect_field_defaults(module);
        // Predeclare functions + type methods so calls inside tests resolve.
        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.predeclare_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.predeclare_function(m);
                    }
                }
                _ => {}
            }
        }
        // Skip function bodies on startup; land on test harness.
        let skip_fns = self.chunk.push(Op::Jump(0));

        // Emit function / method bodies (entries patched inside compile_function).
        for item in &module.items {
            match item {
                Item::Function(f) if !f.is_verify_only() => self.compile_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.compile_function(m);
                    }
                }
                _ => {}
            }
        }

        let tests_start = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(skip_fns, tests_start);

        // Emit top-level bindings used by tests, then test bodies.
        for item in &module.items {
            if let Item::Binding(b) = item {
                self.compile_expr(&b.value);
                self.chunk.push(Op::StoreGlobal(b.name));
            }
        }

        for item in &module.items {
            if let Item::Test(t) = item {
                let entry = self.chunk.ops.len() as u32;
                self.push_scope();
                self.local_count = 0;
                self.free_regs.clear();
                for stmt in &t.body.stmts {
                    self.compile_stmt(stmt);
                }
                self.pop_scope();
                // Jump over out-of-line lambda bodies so falling through a test
                // does not execute them; CallValue still targets their entries.
                let skip_lambdas = self.chunk.push(Op::Jump(0));
                self.flush_pending_lambdas();
                let after = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(skip_lambdas, after);
                self.chunk.tests.push(TestChunk {
                    name: t.name,
                    entry,
                });
            }
        }
        self.flush_pending_lambdas();
        self.chunk.push(Op::Halt);
        opt::peephole(&mut self.chunk);
        self.chunk
    }

    fn compile_function(&mut self, f: &Function) {
        if self.fn_has_trait_params(f) && f.owner.is_none() {
            self.trait_templates.insert((f.name, f.owner), f.clone());
        }
        let entry = self.chunk.ops.len() as u32;
        let (param_tags, local_tags, ret_tag) = self.function_tags(f);
        let param_names: Vec<SymId> = f.params.iter().map(|p| p.name).collect();
        if let Some(slot) = self.function_chunk_slot(f) {
            let fc = &mut self.chunk.functions[slot];
            fc.entry = entry;
            fc.param_names = param_names;
            fc.mut_params = f.params.iter().map(|p| p.mutable).collect();
            fc.always_inline = function_always_inline(f);
            fc.param_tags = param_tags.clone();
            fc.ret_tag = ret_tag;
            if fc.local_tags.len() < param_tags.len() {
                fc.local_tags = local_tags.clone();
            }
        } else {
            self.chunk.functions.push(FunctionChunk {
                name: f.name,
                owner: f.owner,
                arity: f.params.len() as u8,
                local_count: f.params.len() as u16,
                entry,
                param_names,
                mut_params: f.params.iter().map(|p| p.mutable).collect(),
                always_inline: function_always_inline(f),
                live_intervals: Vec::new(),
                param_tags,
                local_tags,
                ret_tag,
            });
        }
        self.push_scope();
        self.local_count = 0;
        self.free_regs.clear();
        for p in &f.params {
            let slot = self.local_count;
            self.local_count += 1;
            self.locals.last_mut().unwrap().insert(p.name, slot);
            if let Some(tid) = self.resolve_param_type(&p.ty) {
                self.set_local_type(p.name, tid);
            } else if self.interner.resolve(p.name) == "self"
                && let Some(owner) = f.owner
                && let Some(tid) = self
                    .typed
                    .structs
                    .get(&owner)
                    .or_else(|| self.typed.sums.get(&owner))
            {
                self.set_local_type(p.name, *tid);
            }
        }
        let saved_ret = self.ret_struct;
        self.ret_struct = self.struct_of_type(f.ret.as_ref());
        let saved_escape = self.in_request_escape_fn;
        // Match opt::collect_request_alloc_sites: req-enter mention, route lambdas
        // (`PendingLambda::request_scoped`), and in-module callees reachable from routes.
        let fname = self.interner.resolve(f.name);
        self.in_request_escape_fn = self.request_reachable_fns.contains(fname)
            || match &f.body {
                FnBody::Block(b) => opt::block_mentions_req_enter(b, self.interner),
                FnBody::Expr(_) => false,
            };
        match &f.body {
            FnBody::Expr(e) => {
                self.compile_expecting(e, self.ret_struct);
                self.chunk.push(Op::Return);
            }
            FnBody::Block(b) => {
                // Last expression/statement is the implicit return value (no `return` needed).
                self.compile_block_as_return(b);
            }
        }
        self.in_request_escape_fn = saved_escape;
        self.ret_struct = saved_ret;
        // Record final local_count on the function chunk (param-tag slot, not first name).
        if let Some(slot) = self.function_chunk_slot(f) {
            let fc = &mut self.chunk.functions[slot];
            fc.local_count = self.local_count.max(fc.local_count);
            let n = fc.local_count as usize;
            if fc.local_tags.len() < n {
                fc.local_tags.resize(n, CTag::Dyn);
            }
        }
        self.pop_scope();
        // Emit any `|…|` / `spawn` bodies referenced inside this function after
        // its contiguous range (C23 `fn_range` assumes no interleaved entries).
        self.flush_pending_out_of_line();
    }

    /// Compile deferred lambda / spawn bodies out-of-line (may enqueue nested ones).
    fn flush_pending_lambdas(&mut self) {
        self.flush_pending_out_of_line();
    }

    fn flush_pending_out_of_line(&mut self) {
        let saved_ret = self.ret_struct.take();
        loop {
            if let Some(pending) = self.pending_lambdas.pop() {
                self.emit_lambda_body(pending);
                continue;
            }
            if let Some(pending) = self.pending_spawns.pop() {
                self.emit_spawn_body(pending);
                continue;
            }
            if !self.pending_trait_specs.is_empty() {
                self.flush_pending_trait_specs();
                continue;
            }
            break;
        }
        self.ret_struct = saved_ret;
    }

    fn emit_lambda_body(&mut self, pending: PendingLambda) {
        let PendingLambda {
            idx,
            params,
            body,
            captures,
            request_scoped,
        } = pending;
        let entry = self.chunk.ops.len() as u32;
        self.chunk.functions[idx as usize].entry = entry;
        self.push_scope();
        let saved_count = self.local_count;
        let saved_free = std::mem::take(&mut self.free_regs);
        let saved_escape = self.in_request_escape_fn;
        if request_scoped {
            self.in_request_escape_fn = true;
        }
        self.local_count = 0;
        for cap in &captures {
            let _ = self.alloc_local(cap.name);
        }
        for p in &params {
            let _ = self.alloc_local(p.name);
        }
        match &body {
            FnBody::Expr(e) => {
                self.compile_expr(e);
                self.chunk.push(Op::Return);
            }
            FnBody::Block(b) => self.compile_block_as_return(b),
        }
        self.chunk.functions[idx as usize].local_count = self.local_count.max(1);
        self.pop_scope();
        self.free_regs = saved_free;
        self.local_count = saved_count;
        self.in_request_escape_fn = saved_escape;
        // Nested lambdas inside this body were queued during compile_expr —
        // keep them pending so the outer flush loop emits them next (still
        // after this body's contiguous range).
    }

    /// Compile `spawn` body into a zero-arity thunk; returns function index.
    ///
    /// The thunk is queued for out-of-line emission (like lambdas) so the parent
    /// function's `[entry, next.entry)` range stays contiguous for the C23 emitter.
    /// A snapshot of the parent locals map is restored when the body is emitted so
    /// captured bindings (channels, etc.) resolve to the same slots the fiber seeds.
    fn compile_spawn_thunk(&mut self, body: &Expr) -> u16 {
        let name = self
            .interner
            .intern(&format!("__spawn_{}", self.chunk.functions.len()));
        let idx = self.chunk.functions.len() as u16;
        let parent_locals = self.local_count;
        self.chunk.functions.push(FunctionChunk {
            name,
            owner: None,
            arity: 0,
            local_count: parent_locals.max(8),
            entry: 0,
            param_names: vec![],
            mut_params: vec![],
            always_inline: false,
            live_intervals: Vec::new(),
            param_tags: vec![],
            local_tags: vec![],
            ret_tag: CTag::Dyn,
        });
        self.pending_spawns.push(PendingSpawn {
            idx,
            body: body.clone(),
            locals: self.locals.clone(),
            local_types: self.local_types.clone(),
            local_count: self.local_count,
        });
        idx
    }

    fn emit_spawn_body(&mut self, pending: PendingSpawn) {
        let PendingSpawn {
            idx,
            body,
            locals,
            local_types,
            local_count,
        } = pending;
        let entry = self.chunk.ops.len() as u32;
        self.chunk.functions[idx as usize].entry = entry;
        let saved_locals = std::mem::replace(&mut self.locals, locals);
        let saved_types = std::mem::replace(&mut self.local_types, local_types);
        let saved_count = self.local_count;
        let saved_free = std::mem::take(&mut self.free_regs);
        self.local_count = local_count;
        match &body {
            Expr::Block(b) => self.compile_block_as_return(b),
            other => {
                self.compile_expr(other);
                self.chunk.push(Op::Return);
            }
        }
        self.chunk.functions[idx as usize].local_count = self.local_count.max(local_count).max(1);
        self.locals = saved_locals;
        self.local_types = saved_types;
        self.local_count = saved_count;
        self.free_regs = saved_free;
    }

    /// Compile a block so the last expression (or explicit `return`) becomes the return value.
    fn compile_block_as_return(&mut self, block: &Block) {
        match block.stmts.split_last() {
            None => {
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
                self.chunk.push(Op::Return);
            }
            Some((last, rest)) => {
                for stmt in rest {
                    self.compile_stmt(stmt);
                }
                self.compile_tail_stmt(last);
            }
        }
    }

    /// Leave the block's last expression value on the stack (used by `if` / block exprs).
    /// Explicit `return` / `fail` still exit the enclosing function.
    fn compile_block_value(&mut self, block: &Block) {
        match block.stmts.split_last() {
            None => {
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
            }
            Some((last, rest)) => {
                for stmt in rest {
                    self.compile_stmt(stmt);
                }
                match last {
                    Stmt::Expr(e) => self.compile_expr(e),
                    Stmt::Return(v, _) => {
                        if let Some(e) = v {
                            self.compile_expecting(e, self.ret_struct);
                        } else {
                            let c = self.chunk.add_const(Value::Null);
                            self.chunk.push(Op::LoadConst(c));
                        }
                        self.chunk.push(Op::Return);
                    }
                    Stmt::Fail(e, _) => {
                        self.compile_expr(e);
                        self.chunk.push(Op::Fail);
                        self.chunk.push(Op::Return);
                    }
                    Stmt::Panic(msg, _) => {
                        self.compile_panic_msg(msg.as_ref());
                        self.chunk.push(Op::Panic);
                    }
                    Stmt::When { .. } => self.compile_when_value(last),
                    other => {
                        self.compile_stmt(other);
                        let c = self.chunk.add_const(Value::Null);
                        self.chunk.push(Op::LoadConst(c));
                    }
                }
            }
        }
    }

    /// Tail position: leave a value on the stack and emit `Return` (or diverge via `fail`).
    fn compile_tail_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr(e) => {
                self.compile_expecting(e, self.ret_struct);
                self.chunk.push(Op::Return);
            }
            Stmt::Return(v, _) => {
                if let Some(e) = v {
                    self.compile_expecting(e, self.ret_struct);
                } else {
                    let c = self.chunk.add_const(Value::Null);
                    self.chunk.push(Op::LoadConst(c));
                }
                self.chunk.push(Op::Return);
            }
            Stmt::Fail(e, _) => {
                self.compile_expr(e);
                self.chunk.push(Op::Fail);
                self.chunk.push(Op::Return);
            }
            Stmt::Panic(msg, _) => {
                self.compile_panic_msg(msg.as_ref());
                self.chunk.push(Op::Panic);
            }
            Stmt::Scope { body, .. } => {
                self.chunk.push(Op::ScopeEnter);
                match body.stmts.split_last() {
                    None => {
                        let c = self.chunk.add_const(Value::Null);
                        self.chunk.push(Op::LoadConst(c));
                    }
                    Some((last, rest)) => {
                        for s in rest {
                            self.compile_stmt(s);
                        }
                        match last {
                            Stmt::Expr(e) => self.compile_expr(e),
                            Stmt::Return(Some(e), _) => self.compile_expr(e),
                            Stmt::Return(None, _) => {
                                let c = self.chunk.add_const(Value::Null);
                                self.chunk.push(Op::LoadConst(c));
                            }
                            Stmt::When { .. } => self.compile_when_value(last),
                            other => {
                                self.compile_stmt(other);
                                let c = self.chunk.add_const(Value::Null);
                                self.chunk.push(Op::LoadConst(c));
                            }
                        }
                    }
                }
                self.chunk.push(Op::ScopeExit);
                self.chunk.push(Op::Return);
            }
            Stmt::When { .. } => {
                self.compile_when_value(stmt);
                self.chunk.push(Op::Return);
            }
            other => {
                self.compile_stmt(other);
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
                self.chunk.push(Op::Return);
            }
        }
    }

    /// Subject-form `when` that leaves the matched arm's value on the stack (no Pop).
    fn compile_when_value(&mut self, stmt: &Stmt) {
        let Stmt::When { subjects, arms, .. } = stmt else {
            let c = self.chunk.add_const(Value::Null);
            self.chunk.push(Op::LoadConst(c));
            return;
        };
        if subjects.is_empty() {
            // Multiplex `when` is statement-only here.
            self.compile_stmt(stmt);
            let c = self.chunk.add_const(Value::Null);
            self.chunk.push(Op::LoadConst(c));
            return;
        }
        if subjects.len() >= 2 {
            self.compile_multi_when_value(subjects, arms);
            return;
        }
        let subj = &subjects[0];
        self.compile_expr(subj);
        let it = self.interner.intern("it");
        let slot = self.alloc_local(it);
        self.chunk.push(Op::Dup);
        self.chunk.push(Op::StoreLocal(slot));
        let mut end_jumps = Vec::new();
        for arm in arms {
            match &arm.pattern {
                WhenPattern::Else => {
                    self.compile_expr(&arm.body);
                    // Leave value; fall through to end after popping subject below.
                    end_jumps.push(self.chunk.push(Op::Jump(0)));
                }
                WhenPattern::Expr(cond) => {
                    if let Some(tag) = self.variant_tag_of(cond) {
                        self.chunk.push(Op::Dup);
                        self.chunk.push(Op::IsVariant(tag));
                    } else if self.pattern_is_value_match(cond) {
                        self.chunk.push(Op::Dup);
                        self.compile_expr(cond);
                        self.chunk.push(Op::Eq);
                    } else {
                        self.compile_expr(cond);
                    }
                    let jf = self.chunk.push(Op::JumpIfFalse(0));
                    self.compile_expr(&arm.body);
                    end_jumps.push(self.chunk.push(Op::Jump(0)));
                    let next = self.chunk.ops.len() as u32;
                    self.chunk.patch_jump(jf, next);
                }
                WhenPattern::VariantBind {
                    sum,
                    variant,
                    fields,
                    ..
                } => {
                    let tag = self
                        .typed
                        .sums
                        .get(sum)
                        .and_then(|tid| match self.typed.types.get_type(*tid) {
                            TypeKind::Sum { variants, .. } => {
                                variants.iter().find(|v| v.name == *variant).map(|v| v.tag)
                            }
                            _ => None,
                        })
                        .unwrap_or(0);
                    self.chunk.push(Op::Dup);
                    self.chunk.push(Op::IsVariant(tag));
                    let jf = self.chunk.push(Op::JumpIfFalse(0));
                    for (payload, local) in fields {
                        self.chunk.push(Op::Dup);
                        self.chunk.push(Op::GetField(*payload));
                        let slot = self.alloc_local(*local);
                        self.chunk.push(Op::StoreLocal(slot));
                    }
                    self.compile_expr(&arm.body);
                    end_jumps.push(self.chunk.push(Op::Jump(0)));
                    let next = self.chunk.ops.len() as u32;
                    self.chunk.patch_jump(jf, next);
                }
                WhenPattern::After(_) => {}
                WhenPattern::Tuple { .. } => {}
            }
        }
        // No arm matched: null
        let c = self.chunk.add_const(Value::Null);
        self.chunk.push(Op::LoadConst(c));
        let end = self.chunk.ops.len() as u32;
        for j in end_jumps {
            self.chunk.patch_jump(j, end);
        }
        // Stack: [subject, arm_value] → keep arm_value only.
        let tmp = self.interner.intern("__when_val");
        let tmp_slot = self.alloc_local(tmp);
        self.chunk.push(Op::StoreLocal(tmp_slot));
        self.chunk.push(Op::Pop); // subject
        self.chunk.push(Op::LoadLocal(tmp_slot));
    }

    /// Multi-subject `when (a, b, …)` value form — linear arm matching for now;
    /// Mode 2 packed tag switch is emitted when all subjects are sums.
    fn compile_multi_when_value(&mut self, subjects: &[Expr], arms: &[WhenArm]) {
        let mut subj_slots = Vec::with_capacity(subjects.len());
        for (i, subj) in subjects.iter().enumerate() {
            self.compile_expr(subj);
            let name = self.interner.intern(&format!("__when_s{i}"));
            let slot = self.alloc_local(name);
            self.chunk.push(Op::StoreLocal(slot));
            subj_slots.push(slot);
        }
        let mut end_jumps = Vec::new();
        for arm in arms {
            match &arm.pattern {
                WhenPattern::Else => {
                    self.compile_expr(&arm.body);
                    end_jumps.push(self.chunk.push(Op::Jump(0)));
                }
                WhenPattern::Tuple { elems, .. } => {
                    let mut fail_jumps = Vec::new();
                    for (i, elem) in elems.iter().enumerate() {
                        let Some(&slot) = subj_slots.get(i) else {
                            break;
                        };
                        match elem {
                            WhenPattern::Expr(cond) => {
                                if let Expr::Ident(n, _) = cond
                                    && self.interner.resolve(*n) == "_"
                                {
                                    // Wildcard — always matches.
                                } else if let Some(tag) = self.variant_tag_of(cond) {
                                    self.chunk.push(Op::LoadLocal(slot));
                                    self.chunk.push(Op::IsVariant(tag));
                                    fail_jumps.push(self.chunk.push(Op::JumpIfFalse(0)));
                                } else if self.pattern_is_value_match(cond) {
                                    self.chunk.push(Op::LoadLocal(slot));
                                    self.compile_expr(cond);
                                    self.chunk.push(Op::Eq);
                                    fail_jumps.push(self.chunk.push(Op::JumpIfFalse(0)));
                                }
                            }
                            WhenPattern::VariantBind {
                                sum,
                                variant,
                                fields,
                                ..
                            } => {
                                let tag = self
                                    .typed
                                    .sums
                                    .get(sum)
                                    .and_then(|tid| match self.typed.types.get_type(*tid) {
                                        TypeKind::Sum { variants, .. } => variants
                                            .iter()
                                            .find(|v| v.name == *variant)
                                            .map(|v| v.tag),
                                        _ => None,
                                    })
                                    .unwrap_or(0);
                                self.chunk.push(Op::LoadLocal(slot));
                                self.chunk.push(Op::IsVariant(tag));
                                fail_jumps.push(self.chunk.push(Op::JumpIfFalse(0)));
                                for (payload, local) in fields {
                                    self.chunk.push(Op::LoadLocal(slot));
                                    self.chunk.push(Op::GetField(*payload));
                                    let lslot = self.alloc_local(*local);
                                    self.chunk.push(Op::StoreLocal(lslot));
                                }
                            }
                            _ => {}
                        }
                    }
                    self.compile_expr(&arm.body);
                    end_jumps.push(self.chunk.push(Op::Jump(0)));
                    let next = self.chunk.ops.len() as u32;
                    for jf in fail_jumps {
                        self.chunk.patch_jump(jf, next);
                    }
                }
                _ => {}
            }
        }
        let c = self.chunk.add_const(Value::Null);
        self.chunk.push(Op::LoadConst(c));
        let end = self.chunk.ops.len() as u32;
        for j in end_jumps {
            self.chunk.patch_jump(j, end);
        }
    }

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Binding(b) => {
                self.compile_expecting(&b.value, self.struct_of_type(b.ty.as_ref()));
                self.seed_binding_type(b);
                let slot = self.alloc_local(b.name);
                self.chunk.push(Op::StoreLocal(slot));
            }
            Stmt::Destructure { fields, value, .. } => {
                self.compile_expr(value);
                for (src, local) in fields {
                    self.chunk.push(Op::Dup);
                    self.chunk.push(Op::GetField(*src));
                    let slot = self.alloc_local(*local);
                    self.chunk.push(Op::StoreLocal(slot));
                }
                self.chunk.push(Op::Pop);
            }
            Stmt::Expr(e) => {
                self.compile_expr(e);
                self.chunk.push(Op::Pop);
            }
            Stmt::Return(v, _) => {
                if let Some(e) = v {
                    self.compile_expecting(e, self.ret_struct);
                } else {
                    let c = self.chunk.add_const(Value::Null);
                    self.chunk.push(Op::LoadConst(c));
                }
                self.chunk.push(Op::Return);
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                self.compile_expr(cond);
                let jf = self.chunk.push(Op::JumpIfFalse(0));
                let jend = self.chunk.push(Op::Jump(0));
                let else_pc = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, else_pc);
                for s in &else_block.stmts {
                    self.compile_stmt(s);
                }
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
            }
            Stmt::While { cond, body, .. } => {
                let loop_start = self.chunk.ops.len() as u32;
                self.compile_expr(cond);
                let exit = self.chunk.push(Op::JumpIfFalse(0));
                self.loop_depth += 1;
                self.break_stack.push(LoopFrame {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    pops_iter: false,
                });
                for s in &body.stmts {
                    self.compile_stmt(s);
                }
                let frame = self.break_stack.pop().unwrap_or(LoopFrame {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    pops_iter: false,
                });
                self.loop_depth -= 1;
                let cont = self.chunk.ops.len() as u32;
                for c in frame.continues {
                    self.chunk.patch_jump(c, cont);
                }
                self.chunk.push(Op::Jump(loop_start));
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(exit, end);
                for b in frame.breaks {
                    self.chunk.patch_jump(b, end);
                }
            }
            Stmt::Loop { body, .. } => {
                let loop_start = self.chunk.ops.len() as u32;
                self.loop_depth += 1;
                self.break_stack.push(LoopFrame {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    pops_iter: false,
                });
                for s in &body.stmts {
                    self.compile_stmt(s);
                }
                let frame = self.break_stack.pop().unwrap_or(LoopFrame {
                    breaks: Vec::new(),
                    continues: Vec::new(),
                    pops_iter: false,
                });
                self.loop_depth -= 1;
                let cont = self.chunk.ops.len() as u32;
                for c in frame.continues {
                    self.chunk.patch_jump(c, cont);
                }
                self.chunk.push(Op::Jump(loop_start));
                let end = self.chunk.ops.len() as u32;
                for b in frame.breaks {
                    self.chunk.patch_jump(b, end);
                }
            }
            Stmt::Break(_) => {
                if let Some(frame) = self.break_stack.last_mut() {
                    if frame.pops_iter {
                        self.chunk.push(Op::IterPop);
                    }
                    let j = self.chunk.push(Op::Jump(0));
                    frame.breaks.push(j);
                } else {
                    self.chunk.push(Op::Halt);
                }
            }
            Stmt::Continue(_) => {
                if let Some(frame) = self.break_stack.last_mut() {
                    let j = self.chunk.push(Op::Jump(0));
                    frame.continues.push(j);
                } else {
                    self.chunk.push(Op::Halt);
                }
            }
            Stmt::MultiAssign {
                targets, values, ..
            } => {
                // Fast path: `a, b = 0.0, 0.0` / `x, y = 1, 2` — emit direct
                // LoadConst→StoreLocal so C-AOT float/int slot seeding still fires
                // (temps would hide the const from peeps).
                let all_ident_targets = targets.iter().all(|t| matches!(t, Expr::Ident(_, _)));
                let all_lit_values = values
                    .iter()
                    .all(|v| matches!(v, Expr::Int(_, _) | Expr::Float(_, _) | Expr::Bool(_, _)));
                if all_ident_targets && all_lit_values && targets.len() == values.len() {
                    for (t, v) in targets.iter().zip(values.iter()) {
                        let Expr::Ident(name, _) = t else {
                            unreachable!()
                        };
                        let dst = self
                            .find_local(*name)
                            .unwrap_or_else(|| self.alloc_local(*name));
                        self.compile_expr(v);
                        self.chunk.push(Op::StoreLocal(dst));
                    }
                } else {
                    // Evaluate all RHS into temps, then store L→R into targets.
                    // Bare Ident targets that are not yet locals are allocated here
                    // (`a, b = 0, 1` introduces bindings).
                    let mut temps = Vec::with_capacity(values.len());
                    for v in values {
                        self.compile_expr(v);
                        let slot = self.alloc_anon();
                        self.chunk.push(Op::StoreLocal(slot));
                        temps.push(slot);
                    }
                    for (i, t) in targets.iter().enumerate() {
                        if let Some(&slot) = temps.get(i) {
                            self.chunk.push(Op::LoadLocal(slot));
                            if let Expr::Ident(name, _) = t
                                && self.find_local(*name).is_none()
                            {
                                let dst = self.alloc_local(*name);
                                self.chunk.push(Op::StoreLocal(dst));
                                continue;
                            }
                            self.compile_assign_from_tos(t);
                        }
                    }
                }
            }
            Stmt::Guarded { stmt, cond, .. } => {
                self.compile_expr(cond);
                let skip = self.chunk.push(Op::JumpIfFalse(0));
                self.compile_stmt(stmt);
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(skip, end);
            }
            Stmt::Each {
                iter,
                body,
                mutable,
                parallel,
                bindings,
                reduction,
                ..
            } => {
                // `par … (+) { expr }` / reduction: accumulate last expr into a local.
                if let Some(BinOp::Add) = reduction {
                    self.compile_reduction_each(iter, body, bindings, *parallel);
                    return;
                }
                self.compile_expr(iter);
                if *parallel {
                    let end_placeholder = self.chunk.push(Op::ParEachBegin(0));
                    self.emit_each_body(body, bindings);
                    let end = self.chunk.ops.len() as u32;
                    self.chunk.patch_jump(end_placeholder, end);
                } else {
                    self.chunk.push(Op::IterBegin);
                    let loop_start = self.chunk.ops.len() as u32;
                    let exit = self.chunk.push(Op::IterNext(0));
                    let (it_slot, index_slot) = self.emit_each_iter_locals(bindings);
                    self.loop_depth += 1;
                    self.break_stack.push(LoopFrame {
                        breaks: Vec::new(),
                        continues: Vec::new(),
                        pops_iter: true,
                    });
                    for s in &body.stmts {
                        self.compile_stmt(s);
                    }
                    let frame = self.break_stack.pop().unwrap_or(LoopFrame {
                        breaks: Vec::new(),
                        continues: Vec::new(),
                        pops_iter: true,
                    });
                    // Write back mutated `it` into the source array when `@` for.
                    if *mutable {
                        self.emit_each_it_writeback(iter, it_slot, index_slot);
                    }
                    self.loop_depth -= 1;
                    let cont = self.chunk.ops.len() as u32;
                    for c in frame.continues {
                        self.chunk.patch_jump(c, cont);
                    }
                    self.chunk.push(Op::Jump(loop_start));
                    let end = self.chunk.ops.len() as u32;
                    self.chunk.patch_jump(exit, end);
                    for b in frame.breaks {
                        self.chunk.patch_jump(b, end);
                    }
                }
            }
            Stmt::When { subjects, arms, .. } => {
                if subjects.len() >= 2 {
                    self.compile_multi_when_value(subjects, arms);
                    self.chunk.push(Op::Pop);
                } else if let Some(subj) = subjects.first() {
                    self.compile_expr(subj);
                    let it = self.interner.intern("it");
                    let slot = self.alloc_local(it);
                    self.chunk.push(Op::Dup);
                    self.chunk.push(Op::StoreLocal(slot));
                    let mut end_jumps = Vec::new();
                    for arm in arms {
                        match &arm.pattern {
                            WhenPattern::Else => {
                                self.compile_expr(&arm.body);
                                self.chunk.push(Op::Pop);
                            }
                            WhenPattern::Expr(cond) => {
                                if let Some(tag) = self.variant_tag_of(cond) {
                                    // `when s { Status.Paid => … }` — tag match.
                                    self.chunk.push(Op::Dup);
                                    self.chunk.push(Op::IsVariant(tag));
                                } else if self.pattern_is_value_match(cond) {
                                    self.chunk.push(Op::Dup);
                                    self.compile_expr(cond);
                                    self.chunk.push(Op::Eq);
                                } else {
                                    self.compile_expr(cond);
                                }
                                let jf = self.chunk.push(Op::JumpIfFalse(0));
                                self.compile_expr(&arm.body);
                                self.chunk.push(Op::Pop);
                                end_jumps.push(self.chunk.push(Op::Jump(0)));
                                let next = self.chunk.ops.len() as u32;
                                self.chunk.patch_jump(jf, next);
                            }
                            WhenPattern::VariantBind {
                                sum,
                                variant,
                                fields,
                                ..
                            } => {
                                let tag = self
                                    .typed
                                    .sums
                                    .get(sum)
                                    .and_then(|tid| match self.typed.types.get_type(*tid) {
                                        TypeKind::Sum { variants, .. } => variants
                                            .iter()
                                            .find(|v| v.name == *variant)
                                            .map(|v| v.tag),
                                        _ => None,
                                    })
                                    .unwrap_or(0);
                                self.chunk.push(Op::Dup);
                                self.chunk.push(Op::IsVariant(tag));
                                let jf = self.chunk.push(Op::JumpIfFalse(0));
                                for (payload, local) in fields {
                                    self.chunk.push(Op::Dup);
                                    self.chunk.push(Op::GetField(*payload));
                                    let slot = self.alloc_local(*local);
                                    self.chunk.push(Op::StoreLocal(slot));
                                }
                                self.compile_expr(&arm.body);
                                self.chunk.push(Op::Pop);
                                end_jumps.push(self.chunk.push(Op::Jump(0)));
                                let next = self.chunk.ops.len() as u32;
                                self.chunk.patch_jump(jf, next);
                            }
                            WhenPattern::After(_) | WhenPattern::Tuple { .. } => {
                                // Ignored in single-subject form; multiplex uses SelectWhen.
                            }
                        }
                    }
                    let end = self.chunk.ops.len() as u32;
                    for j in end_jumps {
                        self.chunk.patch_jump(j, end);
                    }
                    self.chunk.push(Op::Pop); // subject
                } else {
                    // Multiplex `when { x = ch.recv() => … after 16.ms => … }`
                    for arm in arms {
                        match &arm.pattern {
                            WhenPattern::After(d) => {
                                let c = self.chunk.add_const(Value::Int(1));
                                self.chunk.push(Op::LoadConst(c));
                                self.compile_expr(d);
                            }
                            WhenPattern::Expr(e) => {
                                let c = self.chunk.add_const(Value::Int(0));
                                self.chunk.push(Op::LoadConst(c));
                                self.compile_select_channel(e);
                            }
                            WhenPattern::Else => {
                                let c = self.chunk.add_const(Value::Int(2));
                                self.chunk.push(Op::LoadConst(c));
                                let n = self.chunk.add_const(Value::Null);
                                self.chunk.push(Op::LoadConst(n));
                            }
                            WhenPattern::VariantBind { .. } | WhenPattern::Tuple { .. } => {
                                // Subject-form only; multiplex ignores payload/tuple binds.
                                let c = self.chunk.add_const(Value::Int(2));
                                self.chunk.push(Op::LoadConst(c));
                                let n = self.chunk.add_const(Value::Null);
                                self.chunk.push(Op::LoadConst(n));
                            }
                        }
                    }
                    self.chunk.push(Op::SelectWhen(arms.len() as u8));
                    let idx_sym = self.interner.intern("__select_idx");
                    let val_sym = self.interner.intern("__select_val");
                    let idx_slot = self.alloc_local(idx_sym);
                    let val_slot = self.alloc_local(val_sym);
                    // SelectWhen leaves [index, value] array
                    self.chunk.push(Op::Dup);
                    let zero = self.chunk.add_const(Value::Int(0));
                    self.chunk.push(Op::LoadConst(zero));
                    self.chunk.push(Op::IndexGet);
                    self.chunk.push(Op::StoreLocal(idx_slot));
                    let one = self.chunk.add_const(Value::Int(1));
                    self.chunk.push(Op::LoadConst(one));
                    self.chunk.push(Op::IndexGet);
                    self.chunk.push(Op::StoreLocal(val_slot));
                    let mut end_jumps = Vec::new();
                    for (i, arm) in arms.iter().enumerate() {
                        self.chunk.push(Op::LoadLocal(idx_slot));
                        let c = self.chunk.add_const(Value::Int(i as i64));
                        self.chunk.push(Op::LoadConst(c));
                        self.chunk.push(Op::Eq);
                        let jf = self.chunk.push(Op::JumpIfFalse(0));
                        if let WhenPattern::Expr(Expr::Assign { target, .. }) = &arm.pattern
                            && let Expr::Ident(name, _) = target.as_ref()
                        {
                            self.chunk.push(Op::LoadLocal(val_slot));
                            let slot = self.alloc_local(*name);
                            self.chunk.push(Op::StoreLocal(slot));
                        }
                        self.compile_expr(&arm.body);
                        self.chunk.push(Op::Pop);
                        end_jumps.push(self.chunk.push(Op::Jump(0)));
                        let next = self.chunk.ops.len() as u32;
                        self.chunk.patch_jump(jf, next);
                    }
                    let end = self.chunk.ops.len() as u32;
                    for j in end_jumps {
                        self.chunk.patch_jump(j, end);
                    }
                }
            }
            Stmt::Scope { body, .. } => {
                self.chunk.push(Op::ScopeEnter);
                for s in &body.stmts {
                    self.compile_stmt(s);
                }
                self.chunk.push(Op::ScopeExit);
            }
            Stmt::Fail(e, _) => {
                // `fail` leaves the function. The error value is what `or err` matches.
                self.compile_expr(e);
                self.chunk.push(Op::Fail);
                self.chunk.push(Op::Return);
            }
            Stmt::Panic(msg, _) => {
                self.compile_panic_msg(msg.as_ref());
                self.chunk.push(Op::Panic);
            }
            // `#` forms are expanded away before codegen; treat leftovers as no-ops.
            Stmt::Comptime(_) => {}
        }
    }

    fn compile_panic_msg(&mut self, msg: Option<&Expr>) {
        if let Some(e) = msg {
            self.compile_expr(e);
        } else {
            let c = self.chunk.add_const(Value::Str("panic".into()));
            self.chunk.push(Op::LoadConst(c));
        }
    }

    fn emit_each_iter_locals(&mut self, bindings: &[EachBinding]) -> (u16, u16) {
        self.chunk.push(Op::PushIt);
        let it = self.interner.intern("it");
        let slot = self.alloc_local(it);
        self.chunk.push(Op::StoreLocal(slot));
        self.chunk.push(Op::PushIndex);
        let index = self.interner.intern("index");
        let islot = self.alloc_local(index);
        self.chunk.push(Op::StoreLocal(islot));
        for b in bindings {
            self.chunk.push(Op::LoadLocal(slot));
            let bslot = self.alloc_local(b.name);
            self.chunk.push(Op::StoreLocal(bslot));
        }
        (slot, islot)
    }

    /// `for @arr { it += 1 }` / `for @w.bodies { it.f = … }` — write `it` into the live
    /// iterator slot, then sync the array place (`arr`, `w.bodies`, …).
    fn emit_each_it_writeback(&mut self, iter: &Expr, it_slot: u16, _index_slot: u16) {
        let place = match iter {
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => expr.as_ref(),
            other => other,
        };
        if self.place_root_ident(place).is_none() {
            return;
        }
        self.chunk.push(Op::LoadLocal(it_slot));
        self.chunk.push(Op::IterSetIt);
        // IterSetIt leaves the (possibly COW-updated) array on the stack — write it back.
        self.chunk.push(Op::Dup);
        self.store_back_place(place);
        self.chunk.push(Op::Pop);
    }

    fn assign_base_ident(&mut self, expr: &Expr) -> Option<SymId> {
        match expr {
            Expr::Ident(name, _) => Some(*name),
            // Implicit `it` / `index` in `for` / `when` are locals named those strings.
            Expr::It(_) => Some(self.interner.intern("it")),
            Expr::IndexIdent(_) => Some(self.interner.intern("index")),
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => self.assign_base_ident(expr),
            _ => None,
        }
    }

    /// All `@ident` arguments → `(caller_local_slot, param_index)` for `CallMut` writeback.
    fn mut_arg_writebacks(&self, args: &[Expr]) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if let Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } = a
                && let Expr::Ident(name, _) = expr.as_ref()
                && let Some(slot) = self.find_local(*name)
            {
                out.push((slot, i as u16));
            }
        }
        out
    }

    /// Push call arguments. A `@place` argument rooted in a local (`@self.out`, `@g.rows[i]`)
    /// goes through a temp slot that `CallMut` writes back; the returned `(temp, place)`
    /// pairs are stored into their places by [`Self::store_place_temps`] after the call.
    fn compile_args_with_place_temps(
        &mut self,
        args: &[Expr],
        wbs: &mut Vec<(u16, u16)>,
    ) -> Vec<(u16, Expr)> {
        let mut temps = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if let Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } = a
                && matches!(expr.as_ref(), Expr::Field { .. } | Expr::Index { .. })
                && self.place_root_ident(expr).is_some()
            {
                self.compile_expr(expr);
                let tmp = self.alloc_anon();
                self.chunk.push(Op::StoreLocal(tmp));
                self.chunk.push(Op::LoadLocal(tmp));
                wbs.push((tmp, i as u16));
                temps.push((tmp, expr.as_ref().clone()));
            } else {
                self.compile_expr(a);
            }
        }
        temps
    }

    fn store_place_temps(&mut self, temps: Vec<(u16, Expr)>) {
        for (tmp, place) in temps {
            self.chunk.push(Op::LoadLocal(tmp));
            self.store_back_place(&place);
        }
    }

    fn emit_call_or_mut(&mut self, idx: u16, arity: u8, writebacks: Vec<(u16, u16)>) {
        if writebacks.is_empty() {
            self.chunk.push(Op::Call(idx, arity));
        } else {
            let wb_id = self.chunk.mut_writebacks.len() as u16;
            self.chunk.mut_writebacks.push(writebacks);
            self.chunk.push(Op::CallMut(idx, arity, wb_id));
        }
    }

    fn emit_each_body(&mut self, body: &Block, bindings: &[EachBinding]) {
        // ParEachBegin leaves the (possibly partitioned) collection on the stack.
        self.chunk.push(Op::IterBegin);
        let loop_start = self.chunk.ops.len() as u32;
        let exit = self.chunk.push(Op::IterNext(0));
        let _ = self.emit_each_iter_locals(bindings);
        self.loop_depth += 1;
        self.break_stack.push(LoopFrame {
            breaks: Vec::new(),
            continues: Vec::new(),
            pops_iter: true,
        });
        for s in &body.stmts {
            self.compile_stmt(s);
        }
        let frame = self.break_stack.pop().unwrap_or(LoopFrame {
            breaks: Vec::new(),
            continues: Vec::new(),
            pops_iter: true,
        });
        self.loop_depth -= 1;
        let cont = self.chunk.ops.len() as u32;
        for c in frame.continues {
            self.chunk.patch_jump(c, cont);
        }
        self.chunk.push(Op::Jump(loop_start));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(exit, end);
        for b in frame.breaks {
            self.chunk.patch_jump(b, end);
        }
    }

    fn alloc_anon(&mut self) -> u16 {
        let name = self.interner.intern(&format!("__t{}", self.local_count));
        self.alloc_local(name)
    }

    /// Write the updated (copy-on-write) aggregate on TOS back into the place `place`
    /// (`x`, `@x`, `x.f`, `x[i]`, and nestings such as `self.nodes`), consuming it.
    /// Returns `false` (TOS popped) when `place` is not a writable path.
    fn store_back_place(&mut self, place: &Expr) -> bool {
        match place {
            Expr::Ident(name, _) => {
                if let Some(slot) = self.find_local(*name) {
                    self.chunk.push(Op::StoreLocal(slot));
                } else {
                    self.chunk.push(Op::StoreGlobal(*name));
                }
                true
            }
            // `it.f = …` / `index = …` inside `for` — same locals as Ident("it"|"index").
            Expr::It(_) | Expr::IndexIdent(_) => {
                let name = match place {
                    Expr::It(_) => self.interner.intern("it"),
                    _ => self.interner.intern("index"),
                };
                if let Some(slot) = self.find_local(name) {
                    self.chunk.push(Op::StoreLocal(slot));
                } else {
                    self.chunk.push(Op::StoreGlobal(name));
                }
                true
            }
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => self.store_back_place(expr),
            Expr::Field { base, field, .. } if self.place_root_ident(base).is_some() => {
                let tmp = self.alloc_anon();
                self.chunk.push(Op::StoreLocal(tmp));
                self.compile_expr(base);
                self.chunk.push(Op::LoadLocal(tmp));
                self.chunk.push(Op::SetField(*field));
                self.store_back_place(base)
            }
            Expr::Index { base, index, .. } if self.place_root_ident(base).is_some() => {
                let tmp = self.alloc_anon();
                self.chunk.push(Op::StoreLocal(tmp));
                self.compile_expr(base);
                self.compile_expr(index);
                self.chunk.push(Op::LoadLocal(tmp));
                self.chunk.push(Op::IndexSet);
                self.store_back_place(base)
            }
            _ => {
                self.chunk.push(Op::Pop);
                false
            }
        }
    }

    /// Root local/global of a place path `x.f[i].g`, if the path is writable.
    fn place_root_ident(&mut self, expr: &Expr) -> Option<SymId> {
        match expr {
            Expr::Field { base, .. } | Expr::Index { base, .. } => self.place_root_ident(base),
            other => self.assign_base_ident(other),
        }
    }

    /// Store TOS into `target` (ident / index / field), leaving no value.
    fn compile_assign_from_tos(&mut self, target: &Expr) {
        match target {
            Expr::Ident(name, _) => {
                if let Some(slot) = self.find_local(*name) {
                    self.chunk.push(Op::StoreLocal(slot));
                } else {
                    self.chunk.push(Op::StoreGlobal(*name));
                }
            }
            Expr::Index { base, index, span } => {
                let val = self.alloc_anon();
                self.chunk.push(Op::StoreLocal(val));
                let idx = self.alloc_anon();
                self.compile_expr(index);
                self.chunk.push(Op::StoreLocal(idx));
                self.compile_expr(base);
                self.chunk.push(Op::LoadLocal(idx));
                self.chunk.push(Op::LoadLocal(val));
                let key = super_frontend::meta::span_key(*span);
                if self.opt_meta.unchecked_index.contains(&key) {
                    self.chunk.push(Op::StoreSliceUnchecked);
                } else {
                    self.chunk.push(Op::IndexSet);
                }
                // IndexSet leaves the (COW-updated) array on the stack — write it back.
                self.store_back_place(base);
            }
            // `p.x, p.y = …` / `@w.f, … = …`: write the TOS value through the place path
            // (pops it when the target is not writable).
            other => {
                self.store_back_place(other);
            }
        }
    }

    /// Lower `par/for … (+) { … }` to an accumulating loop; result discarded as stmt.
    fn compile_reduction_each(
        &mut self,
        iter: &Expr,
        body: &Block,
        bindings: &[EachBinding],
        parallel: bool,
    ) {
        let acc = self.alloc_anon();
        let zero = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(zero));
        self.chunk.push(Op::StoreLocal(acc));
        self.compile_expr(iter);
        if parallel {
            // Serial reduction for now (OpenMP path lives in C-AOT).
            let _ = parallel;
        }
        self.chunk.push(Op::IterBegin);
        let loop_start = self.chunk.ops.len() as u32;
        let exit = self.chunk.push(Op::IterNext(0));
        let _ = self.emit_each_iter_locals(bindings);
        // Body: evaluate last expr and add into acc.
        for s in &body.stmts {
            match s {
                Stmt::Expr(e) => {
                    self.compile_expr(e);
                    self.chunk.push(Op::LoadLocal(acc));
                    self.chunk.push(Op::Add);
                    self.chunk.push(Op::StoreLocal(acc));
                }
                other => self.compile_stmt(other),
            }
        }
        self.chunk.push(Op::Jump(loop_start));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(exit, end);
        // Leave acc on stack then pop (stmt context).
        self.chunk.push(Op::LoadLocal(acc));
        self.chunk.push(Op::Pop);
    }

    fn compile_select_channel(&mut self, expr: &Expr) {
        match expr {
            Expr::Assign { value, .. } => self.compile_select_channel(value),
            Expr::Pipeline {
                base, method, args, ..
            } if self.interner.resolve(*method) == "recv" && args.is_empty() => {
                self.compile_expr(base);
            }
            Expr::Call { callee, args, .. } if args.is_empty() => {
                if let Expr::Field { base, field, .. } = callee.as_ref()
                    && self.interner.resolve(*field) == "recv"
                {
                    self.compile_expr(base);
                    return;
                }
                self.compile_expr(expr);
            }
            other => self.compile_expr(other),
        }
    }

    fn compile_expecting(&mut self, expr: &Expr, want: Option<SymId>) {
        self.infer_hint = want;
        self.compile_expr(expr);
    }

    /// Is `base` statically a struct with a field named `field`?
    fn declares_field(&self, base: &Expr, field: SymId) -> bool {
        self.type_of_expr(base).is_some_and(|tid| {
            matches!(
                self.typed.types.get_type(tid),
                TypeKind::Struct { fields, .. } if fields.iter().any(|f| f.name == field)
            )
        })
    }

    /// Struct named by a declared type (`User`, `User?`, `User!`), if any.
    fn struct_of_type(&self, ty: Option<&TypeExpr>) -> Option<SymId> {
        match ty? {
            TypeExpr::Named(sym, _) => self.typed.structs.contains_key(sym).then_some(*sym),
            TypeExpr::Optional(inner, _) | TypeExpr::Error(inner, _) => {
                self.struct_of_type(Some(inner))
            }
            _ => None,
        }
    }

    fn compile_expr(&mut self, expr: &Expr) {
        let hint = self.infer_hint.take();
        match expr {
            Expr::Int(n, _) => {
                let c = self.chunk.add_const(Value::Int(*n));
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::Float(f, _) => {
                let c = self.chunk.add_const(Value::Float(*f));
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::Bool(b, _) => {
                let c = self.chunk.add_const(Value::Bool(*b));
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::String(s, span) => {
                let text = self.interner.resolve(*s).to_owned();
                if text.contains('{') || text.contains('\\') {
                    self.compile_interpolated_string(&text, *span);
                } else {
                    let c = self.chunk.add_const(Value::Str(text));
                    self.chunk.push(Op::LoadConst(c));
                }
            }
            Expr::Embed { path, .. } => {
                let rel = self.interner.resolve(*path).to_owned();
                let full = resolve_embed_path(&self.base_dir, &rel);
                let data = std::fs::read(&full).unwrap_or_else(|_| {
                    // Also try repo-relative / CWD as fallback
                    std::fs::read(&rel).unwrap_or_default()
                });
                let v = match String::from_utf8(data.clone()) {
                    Ok(s) => Value::Str(s),
                    Err(_) => Value::Bytes(data),
                };
                let c = self.chunk.add_const(v);
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::None(_) => {
                let c = self.chunk.add_const(Value::Optional(None));
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::Ident(_, _) | Expr::It(_) | Expr::IndexIdent(_) | Expr::ErrIdent(_) => {
                let sym = match expr {
                    Expr::Ident(n, _) => *n,
                    Expr::It(_) => self.interner.intern("it"),
                    Expr::IndexIdent(_) => self.interner.intern("index"),
                    Expr::ErrIdent(_) => self.interner.intern("err"),
                    _ => unreachable!(),
                };
                if let Some(slot) = self.find_local(sym) {
                    self.chunk.push(Op::LoadLocal(slot));
                } else {
                    self.chunk.push(Op::LoadGlobal(sym));
                }
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                if !self.compile_operator_trait(left, right, op) {
                    self.compile_expr(left);
                    self.compile_expr(right);
                    self.chunk.push(match op {
                        BinOp::Add => Op::Add,
                        BinOp::Sub => Op::Sub,
                        BinOp::Mul => Op::Mul,
                        BinOp::Div => Op::Div,
                        BinOp::Rem => Op::Rem,
                        BinOp::Eq => Op::Eq,
                        BinOp::Ne => Op::Ne,
                        BinOp::Lt => Op::Lt,
                        BinOp::Le => Op::Le,
                        BinOp::Gt => Op::Gt,
                        BinOp::Ge => Op::Ge,
                        BinOp::And => Op::And,
                        BinOp::Or => Op::OrLog,
                        BinOp::BitAnd => Op::BitAnd,
                        BinOp::BitOr => Op::BitOr,
                        BinOp::BitXor => Op::BitXor,
                        BinOp::Shl => Op::Shl,
                        BinOp::Shr => Op::Shr,
                        BinOp::UShr => Op::UShr,
                    });
                }
            }
            Expr::Unary { op, expr: e, .. } => {
                match op {
                    UnOp::Neg => {
                        if let Some(idx) = self.trait_method(e, "Neg", "neg") {
                            self.compile_expr(e);
                            self.emit_trait_call(idx, 1, "Neg");
                        } else {
                            self.compile_expr(e);
                            self.chunk.push(Op::Neg);
                        }
                    }
                    UnOp::Not => {
                        self.compile_expr(e);
                        self.chunk.push(Op::Not);
                    }
                    UnOp::BitNot => {
                        self.compile_expr(e);
                        self.chunk.push(Op::BitNot);
                    }
                    UnOp::MutRef => {
                        // `@e.method(args)` parses as MutRef(Pipeline(e, ...))
                        if let Expr::Pipeline {
                            base, method, args, ..
                        } = e.as_ref()
                        {
                            let writeback = match base.as_ref() {
                                Expr::Ident(name, _) => self.find_local(*name),
                                _ => None,
                            };
                            let resolved = self.resolve_ufcs(base, *method, args.len());
                            let route = Self::is_http_route_verb(self.interner.resolve(*method));
                            self.compile_expr(base);
                            self.with_route_handler_args(route, |c| {
                                for a in args {
                                    c.compile_expr(a);
                                }
                            });
                            if let Some(idx) = resolved {
                                let arity = (args.len() + 1) as u8;
                                let wbs =
                                    writeback.map(|slot| vec![(slot, 0u16)]).unwrap_or_default();
                                self.emit_call_or_mut(idx as u16, arity, wbs);
                            } else {
                                self.chunk
                                    .push(Op::CallHost(*method, (args.len() + 1) as u8));
                            }
                        } else {
                            self.compile_expr(e);
                        }
                    }
                }
            }
            Expr::Call {
                callee,
                args,
                punning,
                ..
            } => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    let n = self.interner.resolve(*name);
                    // `print` / `println` both emit a trailing newline (`println` is the
                    // explicit alias). A user-defined `println` takes precedence.
                    if n == "print"
                        || (n == "println"
                            && self
                                .find_fn_idx_arity(*name, None, Some(args.len()), true, Some(args))
                                .is_none())
                    {
                        for a in args {
                            self.compile_expr(a);
                        }
                        if args.is_empty() {
                            // `println()` → bare newline.
                            let c = self.chunk.add_const(Value::Str(String::new()));
                            self.chunk.push(Op::LoadConst(c));
                        }
                        self.chunk.push(Op::Print);
                        return;
                    }
                    if n == "assert_eq" {
                        for a in args {
                            self.compile_expr(a);
                        }
                        self.chunk.push(Op::AssertEq);
                        return;
                    }
                    // Built-in `abs(x)` → branchless int / float abs.
                    if n == "abs" && args.len() == 1 {
                        self.compile_expr(&args[0]);
                        let abs = self.interner.intern("__abs");
                        self.chunk.push(Op::CallHost(abs, 1));
                        return;
                    }
                    // Built-in `len(x)`; only a free user `len` shadows it, never a method.
                    if n == "len"
                        && args.len() == 1
                        && !self
                            .chunk
                            .functions
                            .iter()
                            .any(|f| f.name == *name && f.owner.is_none())
                    {
                        self.compile_expr(&args[0]);
                        let len = self.interner.intern("__len");
                        self.chunk.push(Op::CallHost(len, 1));
                        return;
                    }
                    let call_name = self.canonical(*name);
                    if let Some(idx) =
                        self.ensure_trait_specialization(call_name, args)
                            .or_else(|| {
                                self.find_fn_idx_arity(
                                    call_name,
                                    None,
                                    Some(args.len()),
                                    true,
                                    Some(args),
                                )
                            })
                    {
                        let mut mut_wbs = self.mut_arg_writebacks(args);
                        let temps = self.compile_args_with_place_temps(args, &mut mut_wbs);
                        // Call punning: fill remaining params from same-named locals
                        if *punning {
                            let names = self.chunk.functions[idx].param_names.clone();
                            let arity = names.len().max(args.len()) as u8;
                            for name in names.iter().skip(args.len()) {
                                if let Some(slot) = self.find_local(*name) {
                                    self.chunk.push(Op::LoadLocal(slot));
                                } else {
                                    self.chunk.push(Op::LoadGlobal(*name));
                                }
                            }
                            self.emit_call_or_mut(idx as u16, arity, mut_wbs);
                        } else {
                            self.emit_call_or_mut(idx as u16, args.len() as u8, mut_wbs);
                        }
                        self.store_place_temps(temps);
                        return;
                    }
                    // Local holding a Func / Closure value.
                    if let Some(slot) = self.find_local(*name) {
                        self.chunk.push(Op::LoadLocal(slot));
                        for a in args {
                            self.compile_expr(a);
                        }
                        self.chunk.push(Op::CallValue(args.len() as u8));
                        return;
                    }
                    for a in args {
                        self.compile_expr(a);
                    }
                    self.chunk.push(Op::CallHost(*name, args.len() as u8));
                    return;
                }
                // `Type.method(...)` / `Alias.Type.method(...)` / `http.ok(...)` / `ecs.…`
                if let Expr::Field { base, field, .. } = callee.as_ref() {
                    if let Expr::Field {
                        base: mid,
                        field: type_name,
                        ..
                    } = base.as_ref()
                        && let Expr::Ident(ns, _) = mid.as_ref()
                        && self.is_module_name(*ns)
                    {
                        let owner = self.canonical(*type_name);
                        if self.is_type_owner(owner)
                            && let Some(idx) = self.find_fn_idx(*field, Some(owner))
                        {
                            for a in args {
                                self.compile_expr(a);
                            }
                            self.chunk.push(Op::Call(idx as u16, args.len() as u8));
                            return;
                        }
                    }
                    if let Expr::Ident(ns, _) = base.as_ref() {
                        if self.is_module_name(*ns) {
                            let fname = self.canonical(*field);
                            if let Some(idx) = self.find_fn_idx_arity(
                                fname,
                                None,
                                Some(args.len()),
                                true,
                                Some(args),
                            ) {
                                for a in args {
                                    self.compile_expr(a);
                                }
                                self.chunk.push(Op::Call(idx as u16, args.len() as u8));
                                return;
                            }
                        }
                        let ns_name = self.interner.resolve(*ns);
                        if matches!(ns_name, "http" | "ecs" | "db" | "collections" | "host") {
                            let field_name = self.interner.resolve(*field).to_owned();
                            let qual_name = format!("{ns_name}.{field_name}");
                            for a in args {
                                self.compile_expr(a);
                            }
                            let qual = self.interner.intern(&qual_name);
                            self.chunk.push(Op::CallHost(qual, args.len() as u8));
                            return;
                        }
                        // `Gui.new(...)` and `use std.gui.{Gui as Desk}` → `Desk.new(...)`.
                        let owner = self.canonical(*ns);
                        if self.is_type_owner(owner)
                            && let Some(idx) = self.find_fn_idx(*field, Some(owner))
                        {
                            for a in args {
                                self.compile_expr(a);
                            }
                            self.chunk.push(Op::Call(idx as u16, args.len() as u8));
                            return;
                        }
                    }
                }
                // Dynamic callable (lambda / closure / first-class fn value).
                self.compile_expr(callee);
                for a in args {
                    self.compile_expr(a);
                }
                self.chunk.push(Op::CallValue(args.len() as u8));
            }
            Expr::Slice {
                base, start, end, ..
            } => {
                self.compile_expr(base);
                self.compile_expr(start);
                self.compile_expr(end);
                self.chunk
                    .push(Op::CallHost(self.interner.intern("__slice"), 3));
            }
            Expr::Pipeline {
                base, method, args, ..
            } => {
                let m = self.interner.resolve(*method).to_owned();
                match m.as_str() {
                    // Collection pipelines: lower to loops so `it` predicates/mappers work.
                    // (Host stubs previously ignored closures and returned the receiver.)
                    "keep" if args.len() == 1 => {
                        self.compile_pipeline_keep(base, &args[0]);
                    }
                    "map" if args.len() == 1 => {
                        self.compile_pipeline_map(base, &args[0]);
                    }
                    "sum" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Sum, None, None);
                    }
                    "product" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Product, None, None);
                    }
                    "min" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Min, None, None);
                    }
                    "max" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Max, None, None);
                    }
                    "mean" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Mean, None, None);
                    }
                    "variance" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Variance, None, None);
                    }
                    "any" if args.len() <= 1 => {
                        let pred = args.first();
                        self.compile_pipeline_reduce(base, PipelineReduce::Any, pred, None);
                    }
                    "all" if args.len() <= 1 => {
                        let pred = args.first();
                        self.compile_pipeline_reduce(base, PipelineReduce::All, pred, None);
                    }
                    "reduce" if !args.is_empty() => {
                        // Prefer `xs.reduce(init, combiner)`; single-arg form uses `0` init (see comments below).
                        let (init, combiner) = if args.len() >= 2 {
                            (Some(&args[0]), &args[1])
                        } else {
                            (None, &args[0])
                        };
                        self.compile_pipeline_reduce(
                            base,
                            PipelineReduce::Reduce,
                            None,
                            Some(ReduceCombiner { init, combiner }),
                        );
                    }
                    // ParIterable witnesses: same lowering as sequential on the VM;
                    // the C23 path may OpenMP-chunk pure associative reducers.
                    "par_sum" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Sum, None, None);
                    }
                    "par_product" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Product, None, None);
                    }
                    "par_min" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Min, None, None);
                    }
                    "par_max" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Max, None, None);
                    }
                    "par_mean" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Mean, None, None);
                    }
                    "par_variance" if args.is_empty() => {
                        self.compile_pipeline_reduce(base, PipelineReduce::Variance, None, None);
                    }
                    "par_reduce" if !args.is_empty() => {
                        let (init, combiner) = if args.len() >= 2 {
                            (Some(&args[0]), &args[1])
                        } else {
                            (None, &args[0])
                        };
                        self.compile_pipeline_reduce(
                            base,
                            PipelineReduce::Reduce,
                            None,
                            Some(ReduceCombiner { init, combiner }),
                        );
                    }
                    "send" if args.len() == 1 => {
                        self.compile_expr(base);
                        self.compile_expr(&args[0]);
                        self.chunk.push(Op::ChanSend);
                    }
                    "recv" if args.is_empty() => {
                        self.compile_expr(base);
                        self.chunk.push(Op::ChanRecv);
                    }
                    "close" if args.is_empty() => {
                        self.compile_expr(base);
                        self.chunk.push(Op::ChanClose);
                    }
                    "load" if args.is_empty() => {
                        self.compile_expr(base);
                        self.chunk.push(Op::SharedLoad);
                    }
                    // Scalar conversions — only when no type/UFCS method wins (e.g. IntCol.to_float).
                    "to_float"
                        if args.is_empty() && self.resolve_ufcs(base, *method, 0).is_none() =>
                    {
                        self.compile_expr(base);
                        self.emit_to_float_top();
                    }
                    "to_int"
                        if args.is_empty() && self.resolve_ufcs(base, *method, 0).is_none() =>
                    {
                        self.compile_expr(base);
                        let host = self.interner.intern("sys_to_int");
                        self.chunk.push(Op::CallHost(host, 1));
                    }
                    // Universal UFCS onto language builtins: `x.print()` ≡ `print(x)`,
                    // `a.assert_eq(b)`, `n.abs()`. User-defined free functions of the
                    // same name still win (resolved by the generic arm below).
                    "print" | "println" | "assert_eq" | "abs"
                        if !matches!(base.as_ref(), Expr::Ident(ns, _) if self.is_type_owner(*ns))
                            && self.resolve_ufcs(base, *method, args.len()).is_none() =>
                    {
                        let mut call_args = Vec::with_capacity(args.len() + 1);
                        call_args.push(base.as_ref().clone());
                        call_args.extend(args.iter().cloned());
                        let call = Expr::Call {
                            callee: Box::new(Expr::Ident(*method, expr.span())),
                            args: call_args,
                            punning: false,
                            span: expr.span(),
                        };
                        self.compile_expr(&call);
                    }
                    _ => {
                        // `host.fn(args)` / `http.ok(...)` — namespace call, no receiver arg.
                        if let Expr::Ident(ns, _) = base.as_ref() {
                            let ns_name = self.interner.resolve(*ns);
                            if matches!(ns_name, "http" | "ecs" | "db" | "collections" | "host") {
                                let qual_name = format!("{ns_name}.{m}");
                                for a in args {
                                    self.compile_expr(a);
                                }
                                let qual = self.interner.intern(&qual_name);
                                self.chunk.push(Op::CallHost(qual, args.len() as u8));
                                return;
                            }
                            if self.is_module_name(*ns) {
                                let fname = self.canonical(*method);
                                if let Some(idx) = self.find_fn_idx_arity(
                                    fname,
                                    None,
                                    Some(args.len()),
                                    true,
                                    Some(args),
                                ) {
                                    for a in args {
                                        self.compile_expr(a);
                                    }
                                    self.chunk.push(Op::Call(idx as u16, args.len() as u8));
                                    return;
                                }
                            }
                            // `Gui.new` and `use std.gui.{Gui as Desk}` → `Desk.new`.
                            let owner = self.canonical(*ns);
                            if self.is_type_owner(owner)
                                && let Some(idx) = self.find_fn_idx(*method, Some(owner))
                            {
                                let writeback = if self.method_has_self_receiver(idx) {
                                    args.first().and_then(|a| match a {
                                        Expr::Ident(name, _) => self.find_local(*name),
                                        Expr::Unary {
                                            op: UnOp::MutRef,
                                            expr,
                                            ..
                                        } => match expr.as_ref() {
                                            Expr::Ident(name, _) => self.find_local(*name),
                                            _ => None,
                                        },
                                        _ => None,
                                    })
                                } else {
                                    None
                                };
                                for a in args {
                                    self.compile_expr(a);
                                }
                                let arity = args.len() as u8;
                                let mut wbs =
                                    writeback.map(|slot| vec![(slot, 0u16)]).unwrap_or_default();
                                // Also write back any extra `@` args beyond the receiver.
                                for (i, a) in args.iter().enumerate().skip(1) {
                                    if let Expr::Unary {
                                        op: UnOp::MutRef,
                                        expr,
                                        ..
                                    } = a
                                        && let Expr::Ident(name, _) = expr.as_ref()
                                        && let Some(slot) = self.find_local(*name)
                                    {
                                        wbs.push((slot, i as u16));
                                    }
                                }
                                self.emit_call_or_mut(idx as u16, arity, wbs);
                                return;
                            }
                        }
                        if let Expr::Field {
                            base: mid,
                            field: type_name,
                            ..
                        } = base.as_ref()
                            && let Expr::Ident(ns, _) = mid.as_ref()
                            && self.is_module_name(*ns)
                        {
                            let owner = self.canonical(*type_name);
                            if self.is_type_owner(owner)
                                && let Some(idx) = self.find_fn_idx(*method, Some(owner))
                            {
                                for a in args {
                                    self.compile_expr(a);
                                }
                                self.chunk.push(Op::Call(idx as u16, args.len() as u8));
                                return;
                            }
                        }
                        // Universal collection/string length is intrinsic when
                        // the receiver is not a known nominal type. This must
                        // precede UFCS fallback so an unconstrained `_`
                        // parameter cannot bind to an unrelated owner method
                        // such as `StrBuilder.len`.
                        if self.interner.resolve(*method) == "len"
                            && args.is_empty()
                            && self.receiver_owner(base).is_none()
                        {
                            self.compile_expr(base);
                            let len = self.interner.intern("__len");
                            self.chunk.push(Op::CallHost(len, 1));
                            return;
                        }
                        let writeback = match base.as_ref() {
                            Expr::Unary {
                                op: UnOp::MutRef,
                                expr,
                                ..
                            } => match expr.as_ref() {
                                Expr::Ident(name, _) => self.find_local(*name),
                                _ => None,
                            },
                            _ => None,
                        };
                        let resolved = self.resolve_ufcs(base, *method, args.len());
                        let route = Self::is_http_route_verb(self.interner.resolve(*method));
                        self.compile_expr(base);
                        self.with_route_handler_args(route, |c| {
                            for a in args {
                                c.compile_expr(a);
                            }
                        });
                        if let Some(idx) = resolved {
                            let arity = (args.len() + 1) as u8;
                            let mut wbs =
                                writeback.map(|slot| vec![(slot, 0u16)]).unwrap_or_default();
                            for (i, a) in args.iter().enumerate() {
                                if let Expr::Unary {
                                    op: UnOp::MutRef,
                                    expr,
                                    ..
                                } = a
                                    && let Expr::Ident(name, _) = expr.as_ref()
                                    && let Some(slot) = self.find_local(*name)
                                {
                                    wbs.push((slot, (i + 1) as u16));
                                }
                            }
                            self.emit_call_or_mut(idx as u16, arity, wbs);
                        } else {
                            self.chunk
                                .push(Op::CallHost(*method, (args.len() + 1) as u8));
                        }
                    }
                }
            }
            Expr::Field { base, field, .. } => {
                // `arr.len` / `str.len` — intrinsic length (no heap, no host.sys_* boilerplate),
                // unless the receiver is a struct that declares its own `len` field.
                if self.interner.resolve(*field) == "len" && !self.declares_field(base, *field) {
                    self.compile_expr(base);
                    let len = self.interner.intern("__len");
                    self.chunk.push(Op::CallHost(len, 1));
                    return;
                }
                self.compile_expr(base);
                // Prefer named GetField for Super structs; LoadFieldOff is reserved for
                // ExtRef/C-ABI hosts (emitted when offset metadata is attached later).
                if let Some(off) = self.field_byte_offset(base, *field) {
                    // Annotate disasm with C-ABI offset while keeping GetField semantics.
                    self.chunk.note(
                        self.chunk.ops.len() as u32,
                        crate::bytecode::OptNoteKind::Fused,
                        format!("field off +{off}"),
                    );
                }
                self.chunk.push(Op::GetField(*field));
            }
            Expr::Index { base, index, span } => {
                self.compile_expr(base);
                self.compile_expr(index);
                let key = super_frontend::meta::span_key(*span);
                // BCE: emit unchecked only when the optimizer recorded this span.
                if self.opt_meta.unchecked_index.contains(&key) {
                    self.chunk.push(Op::LoadSliceUnchecked);
                    self.chunk.note(
                        self.chunk.ops.len() as u32 - 1,
                        crate::bytecode::OptNoteKind::BoundsCheckElim,
                        "LoadSliceUnchecked",
                    );
                } else {
                    self.chunk.push(Op::IndexGet);
                }
            }
            Expr::Assign {
                op, target, value, ..
            } => {
                // `@hits += 1` on a shared atomic → SharedAdd
                if let (
                    AssignOp::AddAssign,
                    Expr::Unary {
                        op: UnOp::MutRef,
                        expr,
                        ..
                    },
                ) = (op, target.as_ref())
                    && let Expr::Ident(name, _) = expr.as_ref()
                {
                    if let Some(slot) = self.find_local(*name) {
                        self.chunk.push(Op::LoadLocal(slot));
                    } else {
                        self.chunk.push(Op::LoadGlobal(*name));
                    }
                    self.compile_expr(value);
                    self.chunk.push(Op::SharedAdd);
                    return;
                }
                match target.as_ref() {
                    // `it` / `index` are keywords that lower to dedicated locals in `for`.
                    Expr::It(_) | Expr::IndexIdent(_) => {
                        let name = match target.as_ref() {
                            Expr::It(_) => self.interner.intern("it"),
                            _ => self.interner.intern("index"),
                        };
                        if matches!(
                            op,
                            AssignOp::AddAssign
                                | AssignOp::SubAssign
                                | AssignOp::MulAssign
                                | AssignOp::DivAssign
                        ) {
                            if let Some(slot) = self.find_local(name) {
                                self.chunk.push(Op::LoadLocal(slot));
                            } else {
                                self.chunk.push(Op::LoadGlobal(name));
                            }
                            self.compile_expr(value);
                            self.chunk.push(match op {
                                AssignOp::AddAssign => Op::Add,
                                AssignOp::SubAssign => Op::Sub,
                                AssignOp::MulAssign => Op::Mul,
                                AssignOp::DivAssign => Op::Div,
                                AssignOp::Assign => unreachable!(),
                            });
                        } else {
                            self.compile_expr(value);
                        }
                        if let Some(slot) = self.find_local(name) {
                            self.chunk.push(Op::Dup);
                            self.chunk.push(Op::StoreLocal(slot));
                        } else {
                            self.chunk.push(Op::Dup);
                            self.chunk.push(Op::StoreGlobal(name));
                        }
                    }
                    Expr::Ident(name, _) => {
                        if matches!(
                            op,
                            AssignOp::AddAssign
                                | AssignOp::SubAssign
                                | AssignOp::MulAssign
                                | AssignOp::DivAssign
                        ) {
                            if let Some(slot) = self.find_local(*name) {
                                self.chunk.push(Op::LoadLocal(slot));
                            } else {
                                self.chunk.push(Op::LoadGlobal(*name));
                            }
                            self.compile_expr(value);
                            self.chunk.push(match op {
                                AssignOp::AddAssign => Op::Add,
                                AssignOp::SubAssign => Op::Sub,
                                AssignOp::MulAssign => Op::Mul,
                                AssignOp::DivAssign => Op::Div,
                                AssignOp::Assign => unreachable!(),
                            });
                        } else {
                            self.compile_expr(value);
                        }
                        if let Some(slot) = self.find_local(*name) {
                            self.chunk.push(Op::Dup);
                            self.chunk.push(Op::StoreLocal(slot));
                        } else {
                            self.chunk.push(Op::Dup);
                            self.chunk.push(Op::StoreGlobal(*name));
                        }
                    }
                    Expr::Field { base, field, .. } => {
                        // Support compound assigns on fields: load base.field, op, store.
                        if !matches!(op, AssignOp::Assign) {
                            self.compile_expr(base);
                            self.chunk.push(Op::Dup);
                            self.chunk.push(Op::GetField(*field));
                            self.compile_expr(value);
                            self.chunk.push(match op {
                                AssignOp::AddAssign => Op::Add,
                                AssignOp::SubAssign => Op::Sub,
                                AssignOp::MulAssign => Op::Mul,
                                AssignOp::DivAssign => Op::Div,
                                AssignOp::Assign => unreachable!(),
                            });
                            self.chunk.push(Op::SetField(*field));
                        } else {
                            self.compile_expr(base);
                            self.compile_expr(value);
                            self.chunk.push(Op::SetField(*field));
                        }
                        // Write the updated aggregate back through the full place path
                        // (`b.f =`, `w.bodies[i].f =`, `@w.bodies[i].f =`, …). Ident-only
                        // writeback left indexed/field bases as silent no-ops.
                        if self.place_root_ident(base).is_some() {
                            self.chunk.push(Op::Dup);
                            self.store_back_place(base);
                        }
                    }
                    Expr::Index { base, index, span } => {
                        let idx_name = self.interner.intern("__idx");
                        let idx_slot = self.alloc_local(idx_name);
                        self.compile_expr(index);
                        self.chunk.push(Op::StoreLocal(idx_slot));

                        let key = super_frontend::meta::span_key(*span);
                        let unchecked = self.opt_meta.unchecked_index.contains(&key);
                        let set_op = if unchecked {
                            Op::StoreSliceUnchecked
                        } else {
                            Op::IndexSet
                        };

                        if !matches!(op, AssignOp::Assign) {
                            let val_name = self.interner.intern("__ival");
                            let val_slot = self.alloc_local(val_name);
                            self.compile_expr(base);
                            self.chunk.push(Op::LoadLocal(idx_slot));
                            if unchecked {
                                self.chunk.push(Op::LoadSliceUnchecked);
                            } else {
                                self.chunk.push(Op::IndexGet);
                            }
                            self.compile_expr(value);
                            self.chunk.push(match op {
                                AssignOp::AddAssign => Op::Add,
                                AssignOp::SubAssign => Op::Sub,
                                AssignOp::MulAssign => Op::Mul,
                                AssignOp::DivAssign => Op::Div,
                                AssignOp::Assign => unreachable!(),
                            });
                            self.chunk.push(Op::StoreLocal(val_slot));
                            self.compile_expr(base);
                            self.chunk.push(Op::LoadLocal(idx_slot));
                            self.chunk.push(Op::LoadLocal(val_slot));
                            self.chunk.push(set_op);
                        } else {
                            self.compile_expr(base);
                            self.chunk.push(Op::LoadLocal(idx_slot));
                            self.compile_expr(value);
                            self.chunk.push(set_op);
                        }
                        if self.place_root_ident(base).is_some() {
                            self.chunk.push(Op::Dup);
                            self.store_back_place(base);
                        }
                    }
                    _ => {
                        self.compile_expr(value);
                    }
                }
            }
            Expr::Array(elems, _) => {
                for e in elems {
                    self.compile_expr(e);
                }
                self.chunk.push(Op::MakeArray(elems.len() as u16));
            }
            Expr::ArrayRepeat { value, len, span } => {
                // Always lower `[val; N]` through `host.sys_arr_new` so C-AOT gets
                // dense `f64`/`i64` stack storage (`kind=1/2`) — never boxed SuperVal
                // item arrays from MakeArray, which kill dense peeps / auto-vectorization.
                let host = self.interner.intern("host");
                let fname = self.interner.intern("sys_arr_new");
                self.compile_expr(&Expr::Call {
                    callee: Box::new(Expr::Field {
                        base: Box::new(Expr::Ident(host, *span)),
                        field: fname,
                        span: *span,
                    }),
                    args: vec![len.as_ref().clone(), value.as_ref().clone()],
                    punning: false,
                    span: *span,
                });
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.compile_expr(cond);
                let jf = self.chunk.push(Op::JumpIfFalse(0));
                self.compile_expr(then_expr);
                let jend = self.chunk.push(Op::Jump(0));
                let else_pc = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, else_pc);
                self.compile_expr(else_expr);
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
            }
            Expr::Map(entries, _) => {
                for e in entries {
                    match &e.key {
                        MapKey::Ident(s) | MapKey::String(s) => {
                            let text = self.interner.resolve(*s).to_owned();
                            let c = self.chunk.add_const(Value::Str(text));
                            self.chunk.push(Op::LoadConst(c));
                        }
                        MapKey::Expr(ex) => self.compile_expr(ex),
                    }
                    self.compile_expr(&e.value);
                }
                // Maps are not escape-classified sites (structs/records only); Auto region.
                self.chunk.push(Op::MakeMap(entries.len() as u16));
            }
            Expr::Record(fields, _)
            | Expr::InferredStruct { fields, .. }
            | Expr::StructLit { fields, .. } => {
                let name = match expr {
                    Expr::StructLit { name, .. } => *name,
                    Expr::InferredStruct { .. } => hint,
                    _ => None,
                };
                // Prefer C-ABI layout order from sema when the struct type is known.
                let ordered: Vec<&RecordField> = if let Some(sym) = name {
                    if let Some(tid) = self.typed.structs.get(&sym) {
                        if let TypeKind::Struct { fields: layout, .. } =
                            self.typed.types.get_type(*tid)
                        {
                            let mut by_name: HashMap<SymId, &RecordField> = HashMap::new();
                            for f in fields {
                                if let Some(n) = f.name {
                                    by_name.insert(n, f);
                                }
                            }
                            let mut out = Vec::with_capacity(layout.len());
                            for lf in layout {
                                if let Some(f) = by_name.remove(&lf.name) {
                                    out.push(f);
                                }
                            }
                            // Keep any extra/punning fields not in the layout.
                            for f in fields {
                                if let Some(n) = f.name {
                                    if by_name.contains_key(&n) {
                                        out.push(f);
                                    }
                                } else {
                                    out.push(f);
                                }
                            }
                            out
                        } else {
                            fields.iter().collect()
                        }
                    } else {
                        fields.iter().collect()
                    }
                } else {
                    fields.iter().collect()
                };
                let defaults = name.and_then(|sym| self.field_defaults.get(&sym).cloned());
                let mut extra = 0u16;
                for f in &ordered {
                    let key = f
                        .name
                        .map(|n| self.interner.resolve(n).to_owned())
                        .unwrap_or_else(|| "field".into());
                    let c = self.chunk.add_const(Value::Str(key));
                    self.chunk.push(Op::LoadConst(c));
                    self.compile_expr(&f.value);
                }
                if let Some(defaults) = defaults {
                    for (fname, expr) in defaults {
                        if ordered.iter().any(|f| f.name == Some(fname)) {
                            continue;
                        }
                        let key = self.interner.resolve(fname).to_owned();
                        let c = self.chunk.add_const(Value::Str(key));
                        self.chunk.push(Op::LoadConst(c));
                        self.compile_expr(&expr);
                        extra += 1;
                    }
                }
                let pc = self
                    .chunk
                    .push(Op::MakeStruct(name, ordered.len() as u16 + extra));
                self.tag_escape_birth(pc);
            }
            Expr::Range { start, end, .. } => {
                self.compile_expr(start);
                self.compile_expr(end);
                self.chunk.push(Op::MakeRange);
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.compile_expr(cond);
                let jf = self.chunk.push(Op::JumpIfFalse(0));
                self.compile_block_value(then_block);
                let jend = self.chunk.push(Op::Jump(0));
                let else_pc = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, else_pc);
                if let Some(eb) = else_block {
                    self.compile_block_value(eb);
                } else {
                    let c = self.chunk.add_const(Value::Null);
                    self.chunk.push(Op::LoadConst(c));
                }
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
            }
            Expr::Block(b) => {
                self.compile_block_value(b);
            }
            Expr::Or { left, right, .. } => {
                // Stack discipline:
                //   left; Dup; IsAbsent; JumpIfFalse(keep)
                //   absent: Pop; Pop; right; Jump(end)
                //   keep:   Pop          // drop Dup, leave original left
                self.compile_expr(left);
                self.chunk.push(Op::Dup);
                self.chunk.push(Op::IsAbsent);
                let jf = self.chunk.push(Op::JumpIfFalse(0)); // present → keep left
                self.chunk.push(Op::Pop); // drop Dup copy
                self.chunk.push(Op::Pop); // drop absent left
                self.compile_expr(right);
                let jend = self.chunk.push(Op::Jump(0));
                let keep = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, keep);
                self.chunk.push(Op::Pop); // drop Dup; original left remains
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
            }
            Expr::OrErr { left, body, .. } => {
                self.compile_expr(left);
                self.chunk.push(Op::Dup);
                let is_err_host = self.interner.intern("__is_err");
                self.chunk.push(Op::CallHost(is_err_host, 1));
                let jf = self.chunk.push(Op::JumpIfFalse(0));
                // Stack holds the error. Bind its text to `err`, then yield the handler value.
                let err_text = self.interner.intern("__err_text");
                self.chunk.push(Op::CallHost(err_text, 1));
                let err_name = self.interner.intern("err");
                let err_slot = self.alloc_local(err_name);
                self.chunk.push(Op::StoreLocal(err_slot));
                self.compile_block_value(body);
                let jend = self.chunk.push(Op::Jump(0));
                let ok = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, ok);
                self.chunk.push(Op::OrElse);
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
            }
            Expr::Await(inner, _) => {
                self.compile_expr(inner);
                self.chunk.push(Op::Await);
            }
            Expr::Spawn { body, .. } => {
                let thunk = self.compile_spawn_thunk(body);
                self.chunk.push(Op::Spawn(thunk));
            }
            Expr::Chan { capacity, .. } => {
                self.compile_expr(capacity);
                self.chunk.push(Op::ChanNew);
            }
            Expr::Shared { value, .. } => {
                self.compile_expr(value);
                self.chunk.push(Op::SharedNew);
            }
            Expr::Duration { value, unit, .. } => {
                self.compile_expr(value);
                let tag = match unit {
                    DurationUnit::Ms => 0,
                    DurationUnit::Us => 1,
                    DurationUnit::S => 2,
                };
                self.chunk.push(Op::MakeDuration(tag));
            }
            Expr::NamedVariant {
                sum, variant, args, ..
            } => {
                self.compile_named_variant(*sum, *variant, args);
            }
            // Metaprogramming forms are normally expanded in MetaEngine; emit safe fallbacks.
            Expr::Quote { body, .. } => self.compile_expr(body),
            Expr::Splice { code, .. } => self.compile_expr(code),
            Expr::ComptimeRef { name, span } => {
                self.compile_expr(&Expr::Ident(*name, *span));
            }
            Expr::ComptimeBlock { body, .. } => {
                for s in &body.stmts {
                    self.compile_stmt(s);
                }
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::ComptimeIf {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.compile_expr(cond);
                let jf = self.chunk.push(Op::JumpIfFalse(0));
                for s in &then_block.stmts {
                    self.compile_stmt(s);
                }
                let jend = self.chunk.push(Op::Jump(0));
                let else_pc = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jf, else_pc);
                if let Some(eb) = else_block {
                    for s in &eb.stmts {
                        self.compile_stmt(s);
                    }
                }
                let end = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(jend, end);
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::ComptimeEach { body, .. } => {
                for s in &body.stmts {
                    self.compile_stmt(s);
                }
                let c = self.chunk.add_const(Value::Null);
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::GoalType(_) => {
                let c = self.chunk.add_const(Value::Str("goal".into()));
                self.chunk.push(Op::LoadConst(c));
            }
            Expr::ComptimeField { base, field, span } => {
                // Fall back to runtime field if splice didn't resolve the name.
                if let Expr::String(sym, _) = field.as_ref() {
                    self.compile_expr(base);
                    self.chunk.push(Op::GetField(*sym));
                } else if let Expr::Ident(sym, _) = field.as_ref() {
                    self.compile_expr(base);
                    self.chunk.push(Op::GetField(*sym));
                } else {
                    let _ = span;
                    self.compile_expr(base);
                    self.chunk.push(Op::Pop);
                    let c = self.chunk.add_const(Value::Null);
                    self.chunk.push(Op::LoadConst(c));
                }
            }
            Expr::Lambda {
                params,
                body,
                captures,
                span,
                ..
            } => {
                let caps = if captures.is_empty() {
                    self.discover_captures(params, body.as_ref(), *span)
                } else {
                    captures.clone()
                };
                let idx = self.compile_lambda_fn(
                    None,
                    params,
                    body.as_ref(),
                    &caps,
                    self.compiling_route_handler_arg,
                );
                for cap in &caps {
                    if let Some(slot) = self.find_local(cap.name) {
                        self.chunk.push(Op::LoadLocal(slot));
                    } else {
                        self.chunk.push(Op::LoadGlobal(cap.name));
                    }
                }
                if caps.is_empty() {
                    self.chunk.push(Op::MakeFn(idx));
                } else {
                    self.chunk.push(Op::MakeClosure(idx, caps.len() as u8));
                }
            }
            Expr::Panic { message, .. } => {
                self.compile_panic_msg(message.as_deref());
                self.chunk.push(Op::Panic);
            }
            // Sema rejects unexpanded macro calls; keep the stack balanced anyway.
            Expr::MacroCall(_) => {
                let c = self.chunk.add_const(Value::Optional(None));
                self.chunk.push(Op::LoadConst(c));
            }
        }
    }

    fn discover_captures(&self, params: &[Param], body: &FnBody, span: Span) -> Vec<LambdaCapture> {
        let bound: std::collections::HashSet<SymId> = params.iter().map(|p| p.name).collect();
        let mut names = Vec::new();
        collect_free_idents(body, &bound, &mut names);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (name, mutable) in names {
            if seen.insert(name) && self.find_local(name).is_some() {
                out.push(LambdaCapture {
                    name,
                    mutable,
                    span,
                });
            }
        }
        out
    }

    /// Reserve a chunk function for `|params| body` and queue the body for
    /// out-of-line emission. Capture names become leading locals; CallValue
    /// seeds the env then pops user args. Bodies are not inlined into the
    /// caller so C23 `fn_range` stays a contiguous `[entry, next.entry)`.
    fn compile_lambda_fn(
        &mut self,
        name: Option<SymId>,
        params: &[Param],
        body: &FnBody,
        captures: &[LambdaCapture],
        request_scoped: bool,
    ) -> u16 {
        let name = name.unwrap_or_else(|| {
            self.interner
                .intern(&format!("__lambda_{}", self.chunk.functions.len()))
        });
        let idx = self.chunk.functions.len() as u16;
        // Layout: [capture0.., param0..]; CallValue seeds env then pops user args.
        self.chunk.functions.push(FunctionChunk {
            name,
            owner: None,
            arity: (params.len() + captures.len()) as u8,
            local_count: (params.len() + captures.len()) as u16,
            entry: 0,
            param_names: {
                let mut names = captures.iter().map(|c| c.name).collect::<Vec<_>>();
                names.extend(params.iter().map(|p| p.name));
                names
            },
            mut_params: {
                let mut m = captures.iter().map(|c| c.mutable).collect::<Vec<_>>();
                m.extend(params.iter().map(|p| p.mutable));
                m
            },
            always_inline: false,
            live_intervals: Vec::new(),
            param_tags: vec![CTag::Dyn; params.len() + captures.len()],
            local_tags: vec![CTag::Dyn; params.len() + captures.len()],
            ret_tag: CTag::Dyn,
        });
        self.pending_lambdas.push(PendingLambda {
            idx,
            params: params.to_vec(),
            body: body.clone(),
            captures: captures.to_vec(),
            request_scoped,
        });
        idx
    }

    fn is_http_route_verb(name: &str) -> bool {
        matches!(
            name,
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "route"
                | "connect" | "trace"
        )
    }

    fn with_route_handler_args<R>(&mut self, route: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.compiling_route_handler_arg;
        if route {
            self.compiling_route_handler_arg = true;
        }
        let out = f(self);
        self.compiling_route_handler_arg = saved;
        out
    }

    /// Decode escapes and expand `"hello {name} {user.id}"` into a
    /// left-associated `+` concat chain. A `{` only starts an interpolation when
    /// it encloses a bare path (`ident(.ident)*`); anything else (`"{}"`,
    /// `"{\"a\": 1}"`) is literal text. `\{` / `\}` force a literal brace.
    fn compile_interpolated_string(&mut self, text: &str, _span: Span) {
        let parts = parse_string_parts(text, &mut *self.interner);
        let mut first = true;
        for part in parts {
            match part {
                InterpPart::Lit(s) => {
                    let c = self.chunk.add_const(Value::Str(s));
                    self.chunk.push(Op::LoadConst(c));
                }
                InterpPart::Path(path) => {
                    if first {
                        // Anchor the chain with "" so a lone `"{n}"` stringifies.
                        let c = self.chunk.add_const(Value::Str(String::new()));
                        self.chunk.push(Op::LoadConst(c));
                        first = false;
                    }
                    let mut segs = path.split('.');
                    let head = segs.next().unwrap_or("");
                    let head_sym = self.interner.intern(head);
                    // The hole is not an AST node. Reusing the string span would
                    // read the string's type (`str`) and skip `Display`.
                    let type_span = Span {
                        file: u32::MAX,
                        ..Span::default()
                    };
                    let mut expr = Expr::Ident(head_sym, type_span);
                    // Prefer local / special idents (`it`, `index`, `err`, `self`).
                    self.compile_expr(&expr);
                    for seg in segs {
                        let f = self.interner.intern(seg);
                        if seg == "len" && !self.declares_field(&expr, f) {
                            let len = self.interner.intern("__len");
                            self.chunk.push(Op::CallHost(len, 1));
                        } else {
                            self.chunk.push(Op::GetField(f));
                        }
                        expr = Expr::Field {
                            base: Box::new(expr),
                            field: f,
                            span: type_span,
                        };
                    }
                    self.promote_display(&expr);
                    self.chunk.push(Op::Add);
                    continue;
                }
                InterpPart::Code(expr) => {
                    if first {
                        let c = self.chunk.add_const(Value::Str(String::new()));
                        self.chunk.push(Op::LoadConst(c));
                        first = false;
                    }
                    self.compile_expr(&expr);
                    self.promote_display(&expr);
                    self.chunk.push(Op::Add);
                    continue;
                }
            }
            if !first {
                self.chunk.push(Op::Add);
            }
            first = false;
        }
        if first {
            let c = self.chunk.add_const(Value::Str(String::new()));
            self.chunk.push(Op::LoadConst(c));
        }
    }

    fn alloc_local(&mut self, name: SymId) -> u16 {
        if let Some(slot) = self.locals.last().and_then(|m| m.get(&name).copied()) {
            return slot;
        }
        // Prefer a recycled slot from a prior scope in this function, else bump.
        let slot = if let Some(s) = self.free_regs.pop() {
            self.chunk.note(
                self.chunk.ops.len() as u32,
                crate::bytecode::OptNoteKind::RegRecycle,
                format!("reuse r{s} for {}", self.interner.resolve(name)),
            );
            self.local_count = self.local_count.max(s.saturating_add(1));
            s
        } else {
            let s = self.local_count;
            self.local_count = self.local_count.saturating_add(1);
            s
        };
        self.locals.last_mut().unwrap().insert(name, slot);
        slot
    }

    fn find_local(&self, name: SymId) -> Option<u16> {
        for scope in self.locals.iter().rev() {
            if let Some(&s) = scope.get(&name) {
                return Some(s);
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.locals.push(HashMap::new());
        self.local_types.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        if let Some(scope) = self.locals.pop() {
            // Free registers whose live range ended with this scope.
            for slot in scope.values() {
                if !self.free_regs.contains(slot) {
                    self.free_regs.push(*slot);
                }
            }
        }
        self.local_types.pop();
    }

    fn seed_binding_type(&mut self, b: &Binding) {
        let mut seeded = false;
        if let Some(te) = &b.ty
            && let Some(tid) = self.resolve_param_type(te)
        {
            self.set_local_type(b.name, tid);
            seeded = true;
        }
        let pipe = match &b.value {
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => expr.as_ref(),
            other => other,
        };
        if !seeded
            && let Expr::Pipeline { base, method, .. } = pipe
            && self.interner.resolve(*method) == "new"
            && let Expr::Ident(ns, _) = base.as_ref()
            && let Some(tid) = self.typed.structs.get(ns)
        {
            self.set_local_type(b.name, *tid);
            seeded = true;
        }
        if !seeded && let Some(tid) = self.type_of_expr(&b.value) {
            self.set_local_type(b.name, tid);
        }
    }

    /// `xs.keep(pred)` → new array of elements where `pred` (may use `it`) is true.
    fn compile_pipeline_keep(&mut self, base: &Expr, pred: &Expr) {
        let out = self.alloc_anon();
        // MakeArray(0) is a growable boxed array on both VM and C23 (not a null const).
        self.chunk.push(Op::MakeArray(0));
        self.chunk.push(Op::StoreLocal(out));
        self.compile_expr(base);
        self.chunk.push(Op::IterBegin);
        let loop_start = self.chunk.ops.len() as u32;
        let exit = self.chunk.push(Op::IterNext(0));
        let _ = self.emit_each_iter_locals(&[]);
        self.compile_expr(pred);
        let skip = self.chunk.push(Op::JumpIfFalse(0));
        self.chunk.push(Op::LoadLocal(out));
        let it = self.interner.intern("it");
        let it_slot = self.find_local(it).expect("it local from iter");
        self.chunk.push(Op::LoadLocal(it_slot));
        let push = self.interner.intern("sys_arr_push");
        self.chunk.push(Op::CallHost(push, 2));
        self.chunk.push(Op::StoreLocal(out));
        let cont = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(skip, cont);
        self.chunk.push(Op::Jump(loop_start));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(exit, end);
        self.chunk.push(Op::LoadLocal(out));
    }

    /// `xs.map(mapper)` → new array of `mapper` values (`it` bound per element).
    fn compile_pipeline_map(&mut self, base: &Expr, mapper: &Expr) {
        let out = self.alloc_anon();
        self.chunk.push(Op::MakeArray(0));
        self.chunk.push(Op::StoreLocal(out));
        self.compile_expr(base);
        self.chunk.push(Op::IterBegin);
        let loop_start = self.chunk.ops.len() as u32;
        let exit = self.chunk.push(Op::IterNext(0));
        let _ = self.emit_each_iter_locals(&[]);
        self.chunk.push(Op::LoadLocal(out));
        self.compile_expr(mapper);
        let push = self.interner.intern("sys_arr_push");
        self.chunk.push(Op::CallHost(push, 2));
        self.chunk.push(Op::StoreLocal(out));
        self.chunk.push(Op::Jump(loop_start));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(exit, end);
        self.chunk.push(Op::LoadLocal(out));
    }

    /// Peel trailing `.keep(pred)` / `.map(mapper)` so reductions fuse into one loop.
    fn peel_pipeline_chain<'b>(&self, mut base: &'b Expr) -> FusedPeel<'b> {
        let mut keep = None;
        let mut map = None;
        while let Expr::Pipeline {
            base: inner,
            method,
            args,
            ..
        } = base
        {
            match self.interner.resolve(*method) {
                "keep" if args.len() == 1 => {
                    keep = Some(&args[0]);
                    base = inner;
                }
                "map" if args.len() == 1 => {
                    map = Some(&args[0]);
                    base = inner;
                }
                _ => break,
            }
        }
        FusedPeel {
            source: base,
            keep,
            map,
        }
    }

    fn emit_fused_element(&mut self, it_slot: u16, mapper: Option<&Expr>) {
        if let Some(m) = mapper {
            self.compile_expr(m);
        } else {
            self.chunk.push(Op::LoadLocal(it_slot));
        }
    }

    /// Promote stack-top numeric to float (`int * 1.0` or passthrough float).
    fn emit_to_float_top(&mut self) {
        let one = self.chunk.add_const(Value::Float(1.0));
        self.chunk.push(Op::LoadConst(one));
        self.chunk.push(Op::Mul);
    }

    fn emit_pipeline_guard(&mut self, pred: &Expr) -> u32 {
        self.compile_expr(pred);
        self.chunk.push(Op::JumpIfFalse(0))
    }

    fn emit_incr_local(&mut self, slot: u16) {
        self.chunk.push(Op::LoadLocal(slot));
        let one = self.chunk.add_const(Value::Int(1));
        self.chunk.push(Op::LoadConst(one));
        self.chunk.push(Op::Add);
        self.chunk.push(Op::StoreLocal(slot));
    }

    /// Fused collection reducers (`sum`, `product`, `min`, `max`, `any`, `all`, `mean`,
    /// `variance`, `reduce`). Fuses `.keep` / `.map` prefixes without materializing arrays.
    fn compile_pipeline_reduce(
        &mut self,
        base: &Expr,
        kind: PipelineReduce,
        pred: Option<&Expr>,
        reduce: Option<ReduceCombiner<'_>>,
    ) {
        let fused = self.peel_pipeline_chain(base);
        let guard_expr = fused.keep.or(pred);
        let mapper = fused.map;

        let n_slot = self.alloc_anon();
        let zero = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(zero));
        self.chunk.push(Op::StoreLocal(n_slot));

        let slots = match kind {
            PipelineReduce::Sum => {
                let acc = self.alloc_anon();
                self.chunk.push(Op::LoadConst(zero));
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: None,
                }
            }
            PipelineReduce::Product => {
                let acc = self.alloc_anon();
                let one = self.chunk.add_const(Value::Int(1));
                self.chunk.push(Op::LoadConst(one));
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: None,
                }
            }
            PipelineReduce::Min | PipelineReduce::Max => {
                let acc = self.alloc_anon();
                self.chunk.push(Op::LoadConst(zero));
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: None,
                }
            }
            PipelineReduce::Any => {
                let acc = self.alloc_anon();
                let f = self.chunk.add_const(Value::Bool(false));
                self.chunk.push(Op::LoadConst(f));
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: None,
                }
            }
            PipelineReduce::All => {
                let acc = self.alloc_anon();
                let t = self.chunk.add_const(Value::Bool(true));
                self.chunk.push(Op::LoadConst(t));
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: None,
                }
            }
            PipelineReduce::Mean | PipelineReduce::Variance => {
                let mean = self.alloc_anon();
                let f0 = self.chunk.add_const(Value::Float(0.0));
                self.chunk.push(Op::LoadConst(f0));
                self.chunk.push(Op::StoreLocal(mean));
                let m2 = if matches!(kind, PipelineReduce::Variance) {
                    let s = self.alloc_anon();
                    self.chunk.push(Op::LoadConst(f0));
                    self.chunk.push(Op::StoreLocal(s));
                    Some(s)
                } else {
                    None
                };
                ReduceSlots {
                    primary: mean,
                    mean: Some(mean),
                    m2,
                    acc_named: None,
                }
            }
            PipelineReduce::Reduce => {
                let rc = reduce.as_ref().expect("reduce combiner");
                let acc_sym = self.interner.intern("acc");
                let acc = self.alloc_local(acc_sym);
                if let Some(init) = rc.init {
                    self.compile_expr(init);
                } else {
                    // Single-arg `xs.reduce(combiner)` — implicit `0` init until two-arg pipelines are ubiquitous.
                    self.chunk.push(Op::LoadConst(zero));
                }
                self.chunk.push(Op::StoreLocal(acc));
                ReduceSlots {
                    primary: acc,
                    mean: None,
                    m2: None,
                    acc_named: Some(acc),
                }
            }
        };
        let reduce_combiner = reduce.as_ref();

        self.compile_expr(fused.source);
        self.chunk.push(Op::IterBegin);
        let loop_start = self.chunk.ops.len() as u32;
        let exit = self.chunk.push(Op::IterNext(0));
        let (it_slot, _) = self.emit_each_iter_locals(&[]);

        let mut skip_guard = None;
        if let Some(g) = guard_expr {
            skip_guard = Some(self.emit_pipeline_guard(g));
        }

        match kind {
            PipelineReduce::Sum => {
                self.emit_fused_element(it_slot, mapper);
                self.chunk.push(Op::LoadLocal(slots.primary));
                self.chunk.push(Op::Add);
                self.chunk.push(Op::StoreLocal(slots.primary));
            }
            PipelineReduce::Product => {
                self.emit_fused_element(it_slot, mapper);
                self.chunk.push(Op::LoadLocal(slots.primary));
                self.chunk.push(Op::Mul);
                self.chunk.push(Op::StoreLocal(slots.primary));
            }
            PipelineReduce::Min => {
                self.emit_fused_element(it_slot, mapper);
                self.emit_extremum_update(slots.primary, n_slot, true);
            }
            PipelineReduce::Max => {
                self.emit_fused_element(it_slot, mapper);
                self.emit_extremum_update(slots.primary, n_slot, false);
            }
            PipelineReduce::Any => {
                self.emit_bool_test_value(it_slot, mapper, pred);
                let skip = self.chunk.push(Op::JumpIfFalse(0));
                let t = self.chunk.add_const(Value::Bool(true));
                self.chunk.push(Op::LoadConst(t));
                self.chunk.push(Op::StoreLocal(slots.primary));
                let cont = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(skip, cont);
            }
            PipelineReduce::All => {
                self.emit_bool_test_value(it_slot, mapper, pred);
                let skip = self.chunk.push(Op::JumpIfTrue(0));
                let f = self.chunk.add_const(Value::Bool(false));
                self.chunk.push(Op::LoadConst(f));
                self.chunk.push(Op::StoreLocal(slots.primary));
                let cont = self.chunk.ops.len() as u32;
                self.chunk.patch_jump(skip, cont);
            }
            PipelineReduce::Mean | PipelineReduce::Variance => {
                self.emit_fused_element(it_slot, mapper);
                self.emit_to_float_top();
                self.emit_welford_step(n_slot, slots.mean.unwrap(), slots.m2);
            }
            PipelineReduce::Reduce => {
                let rc = reduce_combiner.expect("reduce combiner");
                self.compile_expr(rc.combiner);
                let acc = slots.acc_named.unwrap_or(slots.primary);
                self.chunk.push(Op::StoreLocal(acc));
            }
        }

        if let Some(j) = skip_guard {
            let cont = self.chunk.ops.len() as u32;
            self.chunk.patch_jump(j, cont);
        }

        self.chunk.push(Op::Jump(loop_start));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(exit, end);

        match kind {
            PipelineReduce::Min | PipelineReduce::Max => {
                self.emit_empty_or_primary(n_slot, slots.primary);
            }
            PipelineReduce::Mean => {
                self.emit_empty_or_float(n_slot, slots.primary);
            }
            PipelineReduce::Variance => {
                self.emit_variance_result(n_slot, slots.mean.unwrap(), slots.m2.expect("m2 slot"));
            }
            _ => {
                self.chunk.push(Op::LoadLocal(slots.primary));
            }
        }
    }

    fn emit_bool_test_value(&mut self, it_slot: u16, mapper: Option<&Expr>, pred: Option<&Expr>) {
        if let Some(p) = pred {
            self.compile_expr(p);
        } else {
            self.emit_fused_element(it_slot, mapper);
        }
    }

    fn emit_extremum_update(&mut self, acc: u16, n_slot: u16, min: bool) {
        let val_tmp = self.alloc_anon();
        self.chunk.push(Op::StoreLocal(val_tmp));
        self.emit_incr_local(n_slot);
        self.chunk.push(Op::LoadLocal(n_slot));
        let one = self.chunk.add_const(Value::Int(1));
        self.chunk.push(Op::LoadConst(one));
        self.chunk.push(Op::Eq);
        let is_first = self.chunk.push(Op::JumpIfTrue(0));
        self.chunk.push(Op::LoadLocal(acc));
        self.chunk.push(Op::LoadLocal(val_tmp));
        if min {
            self.chunk.push(Op::Gt);
            let skip_update = self.chunk.push(Op::JumpIfFalse(0));
            self.chunk.push(Op::LoadLocal(val_tmp));
            self.chunk.push(Op::StoreLocal(acc));
            let after = self.chunk.ops.len() as u32;
            self.chunk.patch_jump(skip_update, after);
            let join = self.chunk.push(Op::Jump(0));
            let first_pc = self.chunk.ops.len() as u32;
            self.chunk.patch_jump(is_first, first_pc);
            self.chunk.push(Op::LoadLocal(val_tmp));
            self.chunk.push(Op::StoreLocal(acc));
            let end = self.chunk.ops.len() as u32;
            self.chunk.patch_jump(join, end);
            return;
        } else {
            self.chunk.push(Op::Lt);
        }
        let skip_update = self.chunk.push(Op::JumpIfFalse(0));
        self.chunk.push(Op::LoadLocal(val_tmp));
        self.chunk.push(Op::StoreLocal(acc));
        let after = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(skip_update, after);
        let join = self.chunk.push(Op::Jump(0));
        let first_pc = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(is_first, first_pc);
        self.chunk.push(Op::LoadLocal(val_tmp));
        self.chunk.push(Op::StoreLocal(acc));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(join, end);
    }

    fn emit_welford_step(&mut self, n_slot: u16, mean_slot: u16, m2_slot: Option<u16>) {
        let x_tmp = self.alloc_anon();
        self.chunk.push(Op::StoreLocal(x_tmp));
        self.emit_incr_local(n_slot);

        self.chunk.push(Op::LoadLocal(x_tmp));
        self.chunk.push(Op::LoadLocal(mean_slot));
        self.chunk.push(Op::Sub);
        let delta_tmp = self.alloc_anon();
        self.chunk.push(Op::StoreLocal(delta_tmp));

        self.chunk.push(Op::LoadLocal(mean_slot));
        self.chunk.push(Op::LoadLocal(delta_tmp));
        self.chunk.push(Op::LoadLocal(n_slot));
        let one_f = self.chunk.add_const(Value::Float(1.0));
        self.chunk.push(Op::LoadConst(one_f));
        self.chunk.push(Op::Mul);
        self.chunk.push(Op::Div);
        self.chunk.push(Op::Add);
        self.chunk.push(Op::StoreLocal(mean_slot));

        if let Some(m2) = m2_slot {
            self.chunk.push(Op::LoadLocal(m2));
            self.chunk.push(Op::LoadLocal(delta_tmp));
            self.chunk.push(Op::LoadLocal(x_tmp));
            self.chunk.push(Op::LoadLocal(mean_slot));
            self.chunk.push(Op::Sub);
            self.chunk.push(Op::Mul);
            self.chunk.push(Op::Add);
            self.chunk.push(Op::StoreLocal(m2));
        }
    }

    fn emit_empty_or_primary(&mut self, n_slot: u16, acc: u16) {
        self.chunk.push(Op::LoadLocal(n_slot));
        let z = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(z));
        self.chunk.push(Op::Eq);
        let empty = self.chunk.push(Op::JumpIfTrue(0));
        self.chunk.push(Op::LoadLocal(acc));
        let done = self.chunk.push(Op::Jump(0));
        let zero_pc = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(empty, zero_pc);
        let zero = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(zero));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(done, end);
    }

    fn emit_empty_or_float(&mut self, n_slot: u16, mean_slot: u16) {
        self.chunk.push(Op::LoadLocal(n_slot));
        let z = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(z));
        self.chunk.push(Op::Eq);
        let empty = self.chunk.push(Op::JumpIfTrue(0));
        self.chunk.push(Op::LoadLocal(mean_slot));
        let done = self.chunk.push(Op::Jump(0));
        let zero_pc = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(empty, zero_pc);
        let f0 = self.chunk.add_const(Value::Float(0.0));
        self.chunk.push(Op::LoadConst(f0));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(done, end);
    }

    fn emit_variance_result(&mut self, n_slot: u16, mean_slot: u16, m2_slot: u16) {
        let _ = mean_slot;
        self.chunk.push(Op::LoadLocal(n_slot));
        let z = self.chunk.add_const(Value::Int(0));
        self.chunk.push(Op::LoadConst(z));
        self.chunk.push(Op::Eq);
        let empty = self.chunk.push(Op::JumpIfTrue(0));
        self.chunk.push(Op::LoadLocal(m2_slot));
        self.chunk.push(Op::LoadLocal(n_slot));
        let n_f = self.chunk.add_const(Value::Float(1.0));
        self.chunk.push(Op::LoadConst(n_f));
        self.chunk.push(Op::Mul);
        self.chunk.push(Op::Div);
        let done = self.chunk.push(Op::Jump(0));
        let zero_pc = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(empty, zero_pc);
        let f0 = self.chunk.add_const(Value::Float(0.0));
        self.chunk.push(Op::LoadConst(f0));
        let end = self.chunk.ops.len() as u32;
        self.chunk.patch_jump(done, end);
    }

    fn field_byte_offset(&self, base: &Expr, field: SymId) -> Option<u32> {
        let tid = self.type_of_expr(base)?;
        match self.typed.types.get_type(tid) {
            TypeKind::Struct { fields, .. } => {
                fields.iter().find(|f| f.name == field).map(|f| f.offset)
            }
            _ => None,
        }
    }

    fn compile_named_variant(&mut self, sum: SymId, variant: SymId, args: &[Expr]) {
        let (tag, field_names) = self
            .typed
            .sums
            .get(&sum)
            .and_then(|tid| match self.typed.types.get_type(*tid) {
                TypeKind::Sum { variants, .. } => variants
                    .iter()
                    .find(|v| v.name == variant)
                    .map(|v| (v.tag, v.fields.iter().map(|(n, _)| *n).collect::<Vec<_>>())),
                _ => None,
            })
            .unwrap_or((0, Vec::new()));
        let n = field_names.len().min(args.len());
        for i in 0..n {
            let key = self.interner.resolve(field_names[i]).to_owned();
            let c = self.chunk.add_const(Value::Str(key));
            self.chunk.push(Op::LoadConst(c));
            self.compile_expr(&args[i]);
        }
        let pc = self.chunk.push(Op::MakeSum {
            name: sum,
            tag,
            n_fields: n as u16,
        });
        self.tag_escape_birth(pc);
    }

    /// Discriminant for a `Sum.Variant` / `Sum.Variant(…)` pattern, if known.
    fn variant_tag_of(&self, expr: &Expr) -> Option<u32> {
        let (sum, variant) = match expr {
            Expr::NamedVariant { sum, variant, .. } => (*sum, *variant),
            _ => return None,
        };
        let tid = self.typed.sums.get(&sum)?;
        match self.typed.types.get_type(*tid) {
            TypeKind::Sum { variants, .. } => {
                variants.iter().find(|v| v.name == variant).map(|v| v.tag)
            }
            _ => None,
        }
    }

    /// Subject-form `when` arms that are values (idents / structs) compare with `==`,
    /// while boolean conditions (`it > 10`, `stage == PAID`) stay as predicates.
    fn pattern_is_value_match(&self, expr: &Expr) -> bool {
        match expr {
            Expr::NamedVariant { .. }
            | Expr::Ident(_, _)
            | Expr::StructLit { .. }
            | Expr::Int(_, _)
            | Expr::Float(_, _)
            | Expr::String(_, _)
            | Expr::Bool(_, _)
            | Expr::None(_) => true,
            Expr::Binary { op, .. } => !matches!(
                op,
                BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or
            ),
            _ => false,
        }
    }

    fn set_local_type(&mut self, name: SymId, tid: super_core::TypeId) {
        if self.local_types.is_empty() {
            self.local_types.push(HashMap::new());
        }
        self.local_types.last_mut().unwrap().insert(name, tid);
    }

    fn local_type_of(&self, name: SymId) -> Option<super_core::TypeId> {
        for scope in self.local_types.iter().rev() {
            if let Some(tid) = scope.get(&name) {
                return Some(*tid);
            }
        }
        None
    }

    fn owner_from_type_id(&self, tid: super_core::TypeId) -> Option<SymId> {
        match self.typed.types.get_type(tid) {
            TypeKind::Struct { name, .. } | TypeKind::Sum { name, .. } | TypeKind::Named(name) => {
                Some(*name)
            }
            _ => None,
        }
    }

    fn is_type_owner(&self, name: SymId) -> bool {
        self.typed.structs.contains_key(&name) || self.typed.sums.contains_key(&name)
    }

    /// Direct call to a language-trait method. The note survives peephole so a
    /// missing witness shows up in `super disasm` instead of a silent builtin.
    fn emit_trait_call(&mut self, idx: u16, arity: u8, trait_name: &str) {
        let at = self.chunk.ops.len() as u32;
        self.chunk.push(Op::Call(idx, arity));
        self.chunk
            .note(at, crate::bytecode::OptNoteKind::TraitDispatch, trait_name);
    }

    fn trait_method(&mut self, expr: &Expr, trait_name: &str, method: &str) -> Option<u16> {
        let tid = self.type_of_expr(expr)?;
        let owner = match self.typed.types.get_type(tid) {
            super_frontend::sema::TypeKind::Struct { name, .. }
            | super_frontend::sema::TypeKind::Named(name) => *name,
            _ => return None,
        };
        let declared = self.typed.impls.get(&owner).is_some_and(|names| {
            names
                .iter()
                .any(|name| self.interner.resolve(*name) == trait_name)
        });
        if !declared {
            return None;
        }
        let method_id = self.interner.intern(method);
        self.find_fn_idx(method_id, Some(owner))
            .map(|idx| idx as u16)
    }

    /// `true` when `op` was lowered to the trait method on `left`.
    fn compile_operator_trait(&mut self, left: &Expr, right: &Expr, op: &BinOp) -> bool {
        let (trait_name, method, finish) = match op {
            BinOp::Add => ("Add", "add", OpFinish::Value),
            BinOp::Sub => ("Sub", "sub", OpFinish::Value),
            BinOp::Mul => ("Mul", "mul", OpFinish::Value),
            BinOp::Div => ("Div", "div", OpFinish::Value),
            BinOp::Rem => ("Rem", "rem", OpFinish::Value),
            BinOp::Eq => ("Eq", "eq", OpFinish::Value),
            BinOp::Ne => ("Eq", "eq", OpFinish::Not),
            BinOp::Lt => ("Ord", "cmp", OpFinish::Cmp(Op::Lt)),
            BinOp::Le => ("Ord", "cmp", OpFinish::Cmp(Op::Le)),
            BinOp::Gt => ("Ord", "cmp", OpFinish::Cmp(Op::Gt)),
            BinOp::Ge => ("Ord", "cmp", OpFinish::Cmp(Op::Ge)),
            _ => return false,
        };
        let Some(idx) = self.trait_method(left, trait_name, method) else {
            return false;
        };
        self.compile_expr(left);
        self.compile_expr(right);
        self.emit_trait_call(idx, 2, trait_name);
        match finish {
            OpFinish::Value => {}
            OpFinish::Not => {
                self.chunk.push(Op::Not);
            }
            OpFinish::Cmp(cmp) => {
                let zero = self.chunk.add_const(Value::Int(0));
                self.chunk.push(Op::LoadConst(zero));
                self.chunk.push(cmp);
            }
        }
        true
    }

    /// Interpolation calls `display` or `to_str` when the static type declares it.
    /// Arrays, maps, and structs with no override keep the builtin witness via `+`.
    fn promote_display(&mut self, expr: &Expr) {
        if let Some(idx) = self
            .trait_method(expr, "Display", "display")
            .or_else(|| self.trait_method(expr, "ToString", "to_str"))
        {
            let name = if self.trait_method(expr, "Display", "display").is_some() {
                "Display"
            } else {
                "ToString"
            };
            self.emit_trait_call(idx, 1, name);
        }
    }

    /// Receiver owner for UFCS, including `@w.method` where base is `Ident` / `MutRef(Ident)`.
    fn receiver_owner(&self, base: &Expr) -> Option<SymId> {
        if let Some(o) = self.expr_struct_owner(base) {
            return Some(o);
        }
        let ident = match base {
            Expr::Ident(name, _) => Some(*name),
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => match expr.as_ref() {
                Expr::Ident(name, _) => Some(*name),
                _ => None,
            },
            _ => None,
        }?;
        self.local_type_of(ident)
            .and_then(|tid| self.owner_from_type_id(tid))
    }
}

#[derive(Clone, Copy)]
enum PipelineReduce {
    Sum,
    Product,
    Min,
    Max,
    Any,
    All,
    Mean,
    Variance,
    Reduce,
}

struct ReduceSlots {
    primary: u16,
    mean: Option<u16>,
    m2: Option<u16>,
    acc_named: Option<u16>,
}

struct FusedPeel<'a> {
    source: &'a Expr,
    keep: Option<&'a Expr>,
    map: Option<&'a Expr>,
}

struct ReduceCombiner<'a> {
    init: Option<&'a Expr>,
    combiner: &'a Expr,
}

enum OpFinish {
    Value,
    Not,
    Cmp(Op),
}

enum InterpPart {
    Lit(String),
    Path(String),
    /// Arbitrary expression hole, e.g. `{status_text(r.status)}`.
    Code(Box<Expr>),
}

/// Parse `src` as a single Super expression by wrapping it in a throwaway binding.
fn parse_hole_expr(src: &str, interner: &mut Interner) -> Option<Expr> {
    let text = format!("__hole = {src}\n");
    let mut diags = Diagnostics::new();
    let tokens = Lexer::new(&text, interner, &mut diags).tokenize();
    let module = Parser::new(&tokens, &text, interner, &mut diags).parse_module();
    if diags.has_errors() || module.items.len() != 1 {
        return None;
    }
    match module.items.into_iter().next()? {
        Item::Binding(b) => Some(b.value),
        _ => None,
    }
}

/// Index of the `}` closing a hole that opens just before `from`, honouring
/// nested braces and (escaped) string literals.
fn hole_end(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut in_str = false;
    let mut i = from;
    while i < chars.len() {
        match chars[i] {
            '\\' if chars.get(i + 1) == Some(&'"') => {
                in_str = !in_str;
                i += 1;
            }
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_interp_path(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut cs = seg.chars();
            cs.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && cs.all(|c| c.is_alphanumeric() || c == '_')
        })
}

/// Split raw string-literal text into decoded literal runs and `{path}` holes.
fn parse_string_parts(text: &str, interner: &mut Interner) -> Vec<InterpPart> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let e = chars[i + 1];
            i += 2;
            match e {
                'n' => lit.push('\n'),
                't' => lit.push('\t'),
                'r' => lit.push('\r'),
                '0' => lit.push('\0'),
                '\\' | '"' | '\'' | '{' | '}' => lit.push(e),
                'x' if i + 1 < chars.len() => {
                    let hex: String = chars[i..i + 2].iter().collect();
                    match u8::from_str_radix(&hex, 16) {
                        Ok(b) if b < 0x80 => {
                            lit.push(b as char);
                            i += 2;
                        }
                        _ => lit.push_str("\\x"),
                    }
                }
                'u' if chars.get(i) == Some(&'{') => {
                    let close = chars[i..].iter().position(|&c| c == '}');
                    let decoded = close.and_then(|off| {
                        let hex: String = chars[i + 1..i + off].iter().collect();
                        u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .map(|ch| (ch, off))
                    });
                    match decoded {
                        Some((ch, off)) => {
                            lit.push(ch);
                            i += off + 1;
                        }
                        None => lit.push_str("\\u"),
                    }
                }
                other => {
                    lit.push('\\');
                    lit.push(other);
                }
            }
            continue;
        }
        if c == '{'
            && let Some(end) = hole_end(&chars, i + 1)
        {
            let inner: String = chars[i + 1..end].iter().collect();
            let inner = inner.trim();
            let hole = if is_interp_path(inner) {
                Some(InterpPart::Path(inner.to_owned()))
            } else if inner.is_empty() || inner.chars().all(|c| c.is_ascii_digit() || c == ',') {
                None
            } else {
                parse_hole_expr(&inner.replace("\\\"", "\""), interner)
                    .map(|e| InterpPart::Code(Box::new(e)))
            };
            if let Some(hole) = hole {
                if !lit.is_empty() {
                    parts.push(InterpPart::Lit(std::mem::take(&mut lit)));
                }
                parts.push(hole);
                i = end + 1;
                continue;
            }
        }
        lit.push(c);
        i += 1;
    }
    if !lit.is_empty() || parts.is_empty() {
        parts.push(InterpPart::Lit(lit));
    }
    parts
}

fn collect_free_idents(
    body: &FnBody,
    bound: &std::collections::HashSet<SymId>,
    out: &mut Vec<(SymId, bool)>,
) {
    fn walk_expr(e: &Expr, bound: &std::collections::HashSet<SymId>, out: &mut Vec<(SymId, bool)>) {
        match e {
            Expr::Ident(n, _) => {
                if !bound.contains(n) {
                    out.push((*n, false));
                }
            }
            Expr::Unary {
                op: UnOp::MutRef,
                expr,
                ..
            } => {
                if let Expr::Ident(n, _) = expr.as_ref() {
                    if !bound.contains(n) {
                        out.push((*n, true));
                    }
                } else {
                    walk_expr(expr, bound, out);
                }
            }
            Expr::Binary { left, right, .. }
            | Expr::Or { left, right, .. }
            | Expr::Assign {
                target: left,
                value: right,
                ..
            } => {
                walk_expr(left, bound, out);
                walk_expr(right, bound, out);
            }
            Expr::Call { callee, args, .. } => {
                walk_expr(callee, bound, out);
                for a in args {
                    walk_expr(a, bound, out);
                }
            }
            Expr::Field { base, .. } => walk_expr(base, bound, out),
            Expr::Pipeline { base, args, .. } => {
                walk_expr(base, bound, out);
                for a in args {
                    walk_expr(a, bound, out);
                }
            }
            Expr::Block(b) => {
                let mut bset = bound.clone();
                for s in &b.stmts {
                    walk_stmt(s, &mut bset, out);
                }
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk_expr(cond, bound, out);
                let mut bset = bound.clone();
                for s in &then_block.stmts {
                    walk_stmt(s, &mut bset, out);
                }
                if let Some(eb) = else_block {
                    let mut bset = bound.clone();
                    for s in &eb.stmts {
                        walk_stmt(s, &mut bset, out);
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                let mut bset = bound.clone();
                for p in params {
                    bset.insert(p.name);
                }
                collect_free_idents(body, &bset, out);
            }
            _ => {}
        }
    }
    fn walk_stmt(
        s: &Stmt,
        bound: &mut std::collections::HashSet<SymId>,
        out: &mut Vec<(SymId, bool)>,
    ) {
        match s {
            Stmt::Binding(b) => {
                walk_expr(&b.value, bound, out);
                bound.insert(b.name);
            }
            Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => walk_expr(e, bound, out),
            Stmt::Panic(Some(e), _) => walk_expr(e, bound, out),
            _ => {}
        }
    }
    match body {
        FnBody::Expr(e) => walk_expr(e, bound, out),
        FnBody::Block(b) => {
            let mut bset = bound.clone();
            for s in &b.stmts {
                walk_stmt(s, &mut bset, out);
            }
        }
    }
}

fn function_always_inline(f: &Function) -> bool {
    if f.attrs
        .iter()
        .any(|a| matches!(a.kind, MetaAttrKind::Inline))
    {
        return true;
    }
    // Heuristic: expression-bodied leaves that only call host / return a trivial expr.
    match &f.body {
        FnBody::Expr(Expr::Call { callee, .. }) => {
            matches!(callee.as_ref(), Expr::Field { base, .. } if matches!(base.as_ref(), Expr::Ident(_, _)))
                || matches!(callee.as_ref(), Expr::Ident(_, _))
        }
        FnBody::Expr(
            Expr::Ident(_, _)
            | Expr::Int(_, _)
            | Expr::Float(_, _)
            | Expr::Bool(_, _)
            | Expr::String(_, _)
            | Expr::None(_),
        ) => true,
        FnBody::Expr(Expr::Binary { left, right, .. }) => {
            matches!(
                left.as_ref(),
                Expr::Ident(_, _) | Expr::Int(_, _) | Expr::Float(_, _)
            ) && matches!(
                right.as_ref(),
                Expr::Ident(_, _) | Expr::Int(_, _) | Expr::Float(_, _)
            )
        }
        _ => false,
    }
}

/// Walk upward from `start` until a directory contains `Pod.toml`.
fn package_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join("Pod.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve an `embed` path.
///
/// Absolute paths stay absolute. Inside a package, `assets/logo.bin` is read
/// from the package root and a bare `logo.bin` is read from `assets/` when
/// that file exists. Otherwise the path is relative to the entry file.
pub fn resolve_embed_path(base_dir: &Path, rel: &str) -> PathBuf {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return rel_path.to_path_buf();
    }
    if let Some(root) = package_root(base_dir) {
        let from_root = root.join(rel_path);
        if from_root.is_file() {
            return from_root;
        }
        if !rel_path.starts_with("assets") {
            let from_assets = root.join("assets").join(rel_path);
            if from_assets.is_file() {
                return from_assets;
            }
        }
    }
    let beside = base_dir.join(rel_path);
    if beside.is_file() {
        return beside;
    }
    PathBuf::from(rel)
}

#[cfg(test)]
mod embed_tests {
    use super::resolve_embed_path;
    use std::fs;

    #[test]
    fn embed_reads_package_assets() {
        let root = std::env::temp_dir().join(format!("super-embed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(
            root.join("Pod.toml"),
            "[pod]\nname = \"t\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("assets/fixture.txt"), "ok").unwrap();
        assert_eq!(
            resolve_embed_path(&root.join("src"), "assets/fixture.txt"),
            root.join("assets/fixture.txt")
        );
        assert_eq!(
            resolve_embed_path(&root.join("src"), "fixture.txt"),
            root.join("assets/fixture.txt")
        );
        let _ = fs::remove_dir_all(&root);
    }
}
