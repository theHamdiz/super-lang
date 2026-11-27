//! Bytecode & AST optimization pipeline for Super.
#![allow(dead_code)] // SROA/LICM rewriters kept for aggressive opt; wired conservatively today.
//!
//! Runs between semantic analysis and emission (and again as a peephole pass on
//! the finished [`Chunk`]). Designed for near-zero allocation on the hot path:
//! AST rewrites reuse existing nodes; peephole mutates the op vector in place.
//!
//! Passes:
//! 1. Constant folding & dead-branch pruning
//! 2. Scalar replacement of aggregates (SROA)
//! 3. Loop-invariant code motion + bounds-check elimination hints
//! 4. Peephole fusion (AddImm, JumpIf*, Move elision, jump-to-jump)

use crate::bytecode::{Chunk, Op, OptNoteKind};
use crate::escape_region::{
    AllocSiteFacts, EscapeClass, EscapeSummary, SiteClass, classify_module,
    classify_module_summary,
};
use crate::value::Value;
use std::collections::{HashMap, HashSet};
use super_core::{Interner, Span, SymId};
use super_frontend::ast::*;
use super_frontend::sema::{TypeKind, TypedModule};

/// Optimizer metadata consumed by the compiler, JIT, and `super disasm`.
#[derive(Clone, Debug, Default)]
pub struct OptMeta {
    /// Spans of `Index` exprs proven safe for unchecked load/store.
    pub unchecked_index: HashSet<u64>,
    /// Locals decomposed by SROA: original name → field → scalar local name.
    pub sroa_map: HashMap<SymId, HashMap<SymId, SymId>>,
    /// Expressions hoisted out of loops (pre-header bindings inserted).
    pub hoisted: Vec<String>,
    /// Request-scoped allocation sites classified for AOT codegen.
    /// Order matches [`collect_request_alloc_sites`] / MakeStruct|MakeMap birth order
    /// inside request-enter functions (see MEMORY_MODEL.md).
    pub escape_classes: Vec<SiteClass>,
    /// Stay / Promote / Conservative counts and Conservative reasons.
    pub escape_summary: EscapeSummary,
    /// Fold / prune counters for metrics.
    pub stats: OptStats,
}

#[derive(Clone, Debug, Default)]
pub struct OptStats {
    pub folds: u32,
    pub dead_branches: u32,
    pub sroa_locals: u32,
    pub licm_hoists: u32,
    pub bce_elided: u32,
    pub peephole_fused: u32,
    pub moves_elided: u32,
    pub jump_chains: u32,
    /// Request-region escape sites classified Conservative (see escape_region).
    pub escape_conservative: u32,
    pub escape_sites: u32,
    pub escape_stay: u32,
    pub escape_promote: u32,
}

/// Optimize a typed module in place, returning metadata for lowering / disasm.
///
/// AST passes are intentionally conservative: constant folding of pure literals
/// and dead `if true/false` only. SROA/LICM run as analysis that feeds metadata
/// (BCE spans) without rewriting aggregates that escape through UFCS/host calls.
pub fn optimize_module(
    module: &mut Module,
    typed: &TypedModule,
    interner: &mut Interner,
) -> OptMeta {
    let mut meta = OptMeta::default();
    // Collect BCE hints from `for` loops; fold only `#` comptime bindings.
    for item in &mut module.items {
        match item {
            Item::Test(t) => mark_bce_walk_block(&t.body, &mut meta),
            Item::Function(f) => match &mut f.body {
                FnBody::Block(b) => {
                    mark_bce_walk_block(b, &mut meta);
                    fold_block_only(b, interner, &mut meta);
                }
                FnBody::Expr(e) => {
                    let taken = std::mem::replace(e, Expr::None(Span::default()));
                    *e = fold_expr(taken, interner, &mut meta);
                }
            },
            Item::Struct(s) => {
                for m in &mut s.methods {
                    match &mut m.body {
                        FnBody::Block(b) => {
                            mark_bce_walk_block(b, &mut meta);
                            fold_block_only(b, interner, &mut meta);
                        }
                        FnBody::Expr(e) => {
                            let taken = std::mem::replace(e, Expr::None(Span::default()));
                            *e = fold_expr(taken, interner, &mut meta);
                        }
                    }
                }
            }
            Item::Sum(s) => {
                for m in &mut s.methods {
                    match &mut m.body {
                        FnBody::Block(b) => {
                            mark_bce_walk_block(b, &mut meta);
                            fold_block_only(b, interner, &mut meta);
                        }
                        FnBody::Expr(e) => {
                            let taken = std::mem::replace(e, Expr::None(Span::default()));
                            *e = fold_expr(taken, interner, &mut meta);
                        }
                    }
                }
            }
            Item::Binding(b) if b.comptime => {
                b.value = fold_expr(b.value.clone(), interner, &mut meta);
            }
            _ => {}
        }
    }
    // Request-scoped escape analysis (MEMORY_MODEL): dataflow Stay/Promote/
    // Conservative over object/record births inside request-enter functions.
    let verbose = std::env::var_os("SUPER_ESCAPE_VERBOSE").is_some();
    let sites = collect_request_alloc_sites(module, interner);
    let (classes, summary) = classify_module_summary(&sites);
    if verbose || std::env::var_os("SUPER_ESCAPE_SUMMARY").is_some() {
        let _ = classify_module(&sites, true);
    }
    meta.stats.escape_sites = summary.total();
    meta.stats.escape_conservative = summary.conservative;
    meta.stats.escape_stay = summary.stay;
    meta.stats.escape_promote = summary.promote;
    meta.escape_classes = classes;
    meta.escape_summary = summary;
    let _ = typed;
    meta
}

/// Map an escape class to the AOT birth-region tag used by `super_obj_alloc_region`.
///
/// Stay → request arena. Promote and Conservative → durable heap at birth
/// (no conservative arena allocation; boundary promote still deep-copies Stay
/// graphs that later escape via Conn/App revive).
pub fn escape_birth_region(class: EscapeClass) -> u8 {
    match class {
        EscapeClass::Stay => 1,          // SUP_REG_REQ
        EscapeClass::Promote => 2,       // SUP_REG_DURABLE
        EscapeClass::Conservative => 2,  // durable — never conservative arena
    }
}

/// HTTP route registration verbs (`App.get` / `#routes` expansion). Handler
/// lambdas run inside `Server.serve`'s `socket_req_enter` / leave bracket.
fn is_route_verb(name: &str) -> bool {
    matches!(
        name,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "route"
            | "connect" | "trace"
    )
}

/// Collect alloc-site facts for struct/record literals via intraprocedural dataflow.
///
/// Every site gets Stay / Promote / Conservative from stores into durable roots,
/// returns, closures, globals/channels, and COW Conn/App updates — not from a
/// blanket `has_boundary_promote = in_req` heuristic.
///
/// Request scope covers: (1) functions that mention `socket_req_enter` /
/// promote / revive, and (2) route handler lambdas registered via `get`/`post`/…
/// (they execute under `Server.serve`'s enter/leave bracket).
fn module_top_level_functions<'a>(
    module: &'a Module,
    interner: &Interner,
) -> HashMap<String, &'a Function> {
    let mut fns = HashMap::new();
    for item in &module.items {
        if let Item::Function(f) = item {
            fns.insert(interner.resolve(f.name).to_string(), f);
        }
    }
    fns
}

pub(crate) fn request_reachable_fn_names(module: &Module, interner: &Interner) -> HashSet<String> {
    let module_fns = module_top_level_functions(module, interner);
    request_reachable_module_fns(module, interner, &module_fns)
}

fn request_reachable_module_fns(
    module: &Module,
    interner: &Interner,
    module_fns: &HashMap<String, &Function>,
) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let mut queue = Vec::new();
    let mut seed_route_lambdas = |body: &FnBody| {
        let mut lambdas = Vec::new();
        match body {
            FnBody::Block(b) => gather_route_handler_lambdas_block(b, interner, &mut lambdas),
            FnBody::Expr(e) => gather_route_handler_lambdas_expr(e, interner, &mut lambdas),
        }
        for lb in lambdas {
            let block = match lb {
                FnBody::Block(b) => b,
                FnBody::Expr(e) => {
                    let span = e.span();
                    queue.push(Block {
                        stmts: vec![Stmt::Expr(e.clone())],
                        span,
                    });
                    continue;
                }
            };
            queue.push(block.clone());
        }
    };
    for item in &module.items {
        match item {
            Item::Function(f) => seed_route_lambdas(&f.body),
            Item::Struct(s) => {
                for m in &s.methods {
                    seed_route_lambdas(&m.body);
                }
            }
            _ => {}
        }
    }
    while let Some(block) = queue.pop() {
        let mut names = Vec::new();
        collect_module_ident_callees(&block, interner, &mut names);
        for name in names {
            if !reachable.insert(name.clone()) {
                continue;
            }
            if let Some(f) = module_fns.get(&name) {
                queue.push(fn_body_as_block(&f.body));
            }
        }
    }
    reachable
}

fn collect_request_alloc_sites(module: &Module, interner: &Interner) -> Vec<AllocSiteFacts> {
    let module_fns = module_top_level_functions(module, interner);
    let req_reachable = request_reachable_module_fns(module, interner, &module_fns);
    let mut sites = Vec::new();
    let mut next_id = 1u32;
    for item in &module.items {
        match item {
            Item::Function(f) => {
                let name = interner.resolve(f.name);
                let force_req = req_reachable.contains(name);
                if let FnBody::Block(b) = &f.body {
                    analyze_fn_alloc_sites(
                        b,
                        interner,
                        &module_fns,
                        &mut sites,
                        &mut next_id,
                        force_req,
                    );
                } else if let FnBody::Expr(e) = &f.body {
                    // Expression-bodied builders (`+build() => #routes { … }`).
                    analyze_route_handler_lambdas_in_expr(
                        e,
                        interner,
                        &module_fns,
                        &mut sites,
                        &mut next_id,
                    );
                }
            }
            Item::Struct(s) => {
                for m in &s.methods {
                    if let FnBody::Block(b) = &m.body {
                        analyze_fn_alloc_sites(
                            b,
                            interner,
                            &module_fns,
                            &mut sites,
                            &mut next_id,
                            false,
                        );
                    } else if let FnBody::Expr(e) = &m.body {
                        analyze_route_handler_lambdas_in_expr(
                            e,
                            interner,
                            &module_fns,
                            &mut sites,
                            &mut next_id,
                        );
                    }
                }
            }
            Item::Sum(s) => {
                for m in &s.methods {
                    if let FnBody::Block(b) = &m.body {
                        analyze_fn_alloc_sites(
                            b,
                            interner,
                            &module_fns,
                            &mut sites,
                            &mut next_id,
                            false,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    sites
}

/// Durable root identifiers recognized for store-escape edges (App/Conn COW).
fn is_durable_root_name(name: &str) -> bool {
    matches!(
        name,
        "app" | "App" | "conn" | "Conn" | "router" | "Router" | "server" | "Server" | "self"
    )
}

fn is_global_or_state_name(name: &str) -> bool {
    matches!(name, "state" | "global" | "STATIC" | "G") || name.starts_with("g_")
}

/// Callees proven not to store the argument into durable state.
fn is_known_safe_callee(name: &str) -> bool {
    matches!(
        name,
        "ok"
            | "created"
            | "bad_request"
            | "not_found"
            | "print"
            | "assert_eq"
            | "assert"
            | "len"
            | "socket_req_enter"
            | "socket_req_leave"
            | "socket_req_promote"
            | "socket_req_keep_str"
            | "socket_conn_enter"
            | "socket_conn_leave"
            | "socket_conn_stats"
            | "conn_req_revive"
            | "ensure"
            | "fail"
            | "panic"
            | "str"
            | "int"
            | "float"
            | "bool"
    )
}

fn analyze_fn_alloc_sites(
    block: &Block,
    interner: &Interner,
    module_fns: &HashMap<String, &Function>,
    sites: &mut Vec<AllocSiteFacts>,
    next_id: &mut u32,
    force_in_req: bool,
) {
    let in_req = force_in_req || block_mentions_req_enter(block, interner);
    if in_req {
        let has_boundary = block_mentions_boundary_promote(block, interner);
        // site_id → index in `sites` (this function's slice starts at `base`).
        let base = sites.len();
        let mut local_to_site: HashMap<SymId, u32> = HashMap::new();

        // Body births first (compiler emits the function body before flushing
        // pending lambdas). Skip route-handler lambdas — analyzed next.
        collect_births(
            block,
            true,
            interner,
            sites,
            next_id,
            &mut local_to_site,
            true,
        );
        propagate_escapes(
            block,
            interner,
            has_boundary,
            &local_to_site,
            &mut sites[base..],
            module_fns,
        );
    }

    // Route-handler lambdas always run under Server.serve's enter/leave, even
    // when the builder that registers them does not mention req_enter.
    // Reverse source order matches LIFO emission (`pending_lambdas.pop()`).
    analyze_route_handler_lambdas(block, interner, module_fns, sites, next_id);
}

/// Analyze `|req| { … }` bodies passed to `App.get` / `post` / … as request-scoped.
fn analyze_route_handler_lambdas(
    block: &Block,
    interner: &Interner,
    module_fns: &HashMap<String, &Function>,
    sites: &mut Vec<AllocSiteFacts>,
    next_id: &mut u32,
) {
    let mut lambdas: Vec<&FnBody> = Vec::new();
    gather_route_handler_lambdas_block(block, interner, &mut lambdas);
    for body in lambdas.into_iter().rev() {
        match body {
            FnBody::Block(b) => {
                analyze_fn_alloc_sites_body_only(b, interner, module_fns, sites, next_id, true);
            }
            FnBody::Expr(e) => {
                let span = e.span();
                let b = Block {
                    stmts: vec![Stmt::Expr((*e).clone())],
                    span,
                };
                analyze_fn_alloc_sites_body_only(&b, interner, module_fns, sites, next_id, true);
            }
        }
    }
}

fn analyze_route_handler_lambdas_in_expr(
    e: &Expr,
    interner: &Interner,
    module_fns: &HashMap<String, &Function>,
    sites: &mut Vec<AllocSiteFacts>,
    next_id: &mut u32,
) {
    let mut lambdas: Vec<&FnBody> = Vec::new();
    gather_route_handler_lambdas_expr(e, interner, &mut lambdas);
    for body in lambdas.into_iter().rev() {
        match body {
            FnBody::Block(b) => {
                analyze_fn_alloc_sites_body_only(b, interner, module_fns, sites, next_id, true);
            }
            FnBody::Expr(e) => {
                let span = e.span();
                let b = Block {
                    stmts: vec![Stmt::Expr((*e).clone())],
                    span,
                };
                analyze_fn_alloc_sites_body_only(&b, interner, module_fns, sites, next_id, true);
            }
        }
    }
}

/// Like [`analyze_fn_alloc_sites`] but does not re-scan nested route lambdas
/// (avoids double-counting when a handler itself registers routes).
fn analyze_fn_alloc_sites_body_only(
    block: &Block,
    interner: &Interner,
    module_fns: &HashMap<String, &Function>,
    sites: &mut Vec<AllocSiteFacts>,
    next_id: &mut u32,
    force_in_req: bool,
) {
    let in_req = force_in_req || block_mentions_req_enter(block, interner);
    if !in_req {
        return;
    }
    let has_boundary = block_mentions_boundary_promote(block, interner);
    let base = sites.len();
    let mut local_to_site: HashMap<SymId, u32> = HashMap::new();
    collect_births(
        block,
        true,
        interner,
        sites,
        next_id,
        &mut local_to_site,
        false,
    );
    propagate_escapes(
        block,
        interner,
        has_boundary,
        &local_to_site,
        &mut sites[base..],
        module_fns,
    );
}

fn fn_body_as_block(body: &FnBody) -> Block {
    match body {
        FnBody::Block(b) => b.clone(),
        FnBody::Expr(e) => Block {
            stmts: vec![Stmt::Return(Some(e.clone()), e.span())],
            span: e.span(),
        },
    }
}

fn collect_module_ident_callees(block: &Block, interner: &Interner, out: &mut Vec<String>) {
    fn walk_expr(e: &Expr, interner: &Interner, out: &mut Vec<String>) {
        match e {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(n, _) = callee.as_ref() {
                    out.push(interner.resolve(*n).to_string());
                }
                for a in args {
                    walk_expr(a, interner, out);
                }
                walk_expr(callee, interner, out);
            }
            Expr::Pipeline { base, args, .. } => {
                walk_expr(base, interner, out);
                for a in args {
                    walk_expr(a, interner, out);
                }
            }
            Expr::Assign { target, value, .. } => {
                walk_expr(target, interner, out);
                walk_expr(value, interner, out);
            }
            Expr::Block(b) => collect_module_ident_callees(b, interner, out),
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk_expr(cond, interner, out);
                collect_module_ident_callees(then_block, interner, out);
                if let Some(b) = else_block {
                    collect_module_ident_callees(b, interner, out);
                }
            }
            Expr::Binary { left, right, .. } | Expr::Or { left, right, .. } => {
                walk_expr(left, interner, out);
                walk_expr(right, interner, out);
            }
            Expr::Field { base, .. } | Expr::Unary { expr: base, .. } => walk_expr(base, interner, out),
            Expr::Array(xs, _) => {
                for x in xs {
                    walk_expr(x, interner, out);
                }
            }
            Expr::Lambda { body, .. } => match body.as_ref() {
                FnBody::Block(b) => collect_module_ident_callees(b, interner, out),
                FnBody::Expr(e) => walk_expr(e, interner, out),
            },
            _ => {}
        }
    }
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(Some(e), _) => walk_expr(e, interner, out),
            Stmt::Binding(b) => walk_expr(&b.value, interner, out),
            Stmt::While { cond, body, .. } => {
                walk_expr(cond, interner, out);
                collect_module_ident_callees(body, interner, out);
            }
            Stmt::Loop { body, .. } | Stmt::Each { body, .. } => {
                collect_module_ident_callees(body, interner, out);
            }
            _ => {}
        }
    }
}

fn gather_route_handler_lambdas_block<'a>(
    block: &'a Block,
    interner: &Interner,
    out: &mut Vec<&'a FnBody>,
) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Binding(Binding { value: e, .. }) => {
                gather_route_handler_lambdas_expr(e, interner, out);
            }
            Stmt::While { cond, body, .. } => {
                gather_route_handler_lambdas_expr(cond, interner, out);
                gather_route_handler_lambdas_block(body, interner, out);
            }
            Stmt::Loop { body, .. } | Stmt::Each { body, .. } => {
                gather_route_handler_lambdas_block(body, interner, out);
            }
            _ => {}
        }
    }
}

fn gather_route_handler_lambdas_expr<'a>(
    e: &'a Expr,
    interner: &Interner,
    out: &mut Vec<&'a FnBody>,
) {
    match e {
        Expr::Pipeline { method, args, .. } if is_route_verb(interner.resolve(*method)) => {
            for a in args {
                if let Expr::Lambda { body, .. } = a {
                    out.push(body.as_ref());
                } else {
                    gather_route_handler_lambdas_expr(a, interner, out);
                }
            }
        }
        Expr::Call { callee, args, .. } => {
            let verb = match callee.as_ref() {
                Expr::Ident(n, _) => Some(interner.resolve(*n)),
                Expr::Field { field, .. } => Some(interner.resolve(*field)),
                _ => None,
            };
            if verb.as_deref().is_some_and(is_route_verb) {
                for a in args {
                    if let Expr::Lambda { body, .. } = a {
                        out.push(body.as_ref());
                    } else {
                        gather_route_handler_lambdas_expr(a, interner, out);
                    }
                }
            } else {
                gather_route_handler_lambdas_expr(callee, interner, out);
                for a in args {
                    gather_route_handler_lambdas_expr(a, interner, out);
                }
            }
        }
        Expr::Unary { expr, .. } | Expr::Field { base: expr, .. } => {
            gather_route_handler_lambdas_expr(expr, interner, out);
        }
        Expr::Binary { left, right, .. } | Expr::Assign { target: left, value: right, .. } => {
            gather_route_handler_lambdas_expr(left, interner, out);
            gather_route_handler_lambdas_expr(right, interner, out);
        }
        Expr::Pipeline { base, args, .. } => {
            gather_route_handler_lambdas_expr(base, interner, out);
            for a in args {
                gather_route_handler_lambdas_expr(a, interner, out);
            }
        }
        Expr::Block(b) => gather_route_handler_lambdas_block(b, interner, out),
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            gather_route_handler_lambdas_expr(cond, interner, out);
            gather_route_handler_lambdas_block(then_block, interner, out);
            if let Some(b) = else_block {
                gather_route_handler_lambdas_block(b, interner, out);
            }
        }
        Expr::Or { left, right, .. } => {
            gather_route_handler_lambdas_expr(left, interner, out);
            gather_route_handler_lambdas_expr(right, interner, out);
        }
        Expr::OrErr { left, body, .. } => {
            gather_route_handler_lambdas_expr(left, interner, out);
            gather_route_handler_lambdas_block(body, interner, out);
        }
        Expr::Array(xs, _) => {
            for x in xs {
                gather_route_handler_lambdas_expr(x, interner, out);
            }
        }
        Expr::Lambda { body, .. } => match body.as_ref() {
            FnBody::Block(b) => gather_route_handler_lambdas_block(b, interner, out),
            FnBody::Expr(e) => gather_route_handler_lambdas_expr(e, interner, out),
        },
        _ => {}
    }
}

fn block_mentions_boundary_promote(block: &Block, interner: &Interner) -> bool {
    fn walk(e: &Expr, interner: &Interner) -> bool {
        match e {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(n, _) = callee.as_ref() {
                    let name = interner.resolve(*n);
                    if name == "socket_req_promote" || name == "conn_req_revive" {
                        return true;
                    }
                }
                walk(callee, interner) || args.iter().any(|a| walk(a, interner))
            }
            Expr::Pipeline { base, args, .. } => {
                walk(base, interner) || args.iter().any(|a| walk(a, interner))
            }
            Expr::Block(b) => block_mentions_boundary_promote(b, interner),
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk(cond, interner)
                    || block_mentions_boundary_promote(then_block, interner)
                    || else_block
                        .as_ref()
                        .is_some_and(|b| block_mentions_boundary_promote(b, interner))
            }
            Expr::Binary { left, right, .. } | Expr::Assign { target: left, value: right, .. } => {
                walk(left, interner) || walk(right, interner)
            }
            Expr::Field { base, .. } | Expr::Unary { expr: base, .. } => walk(base, interner),
            Expr::Lambda { body, .. } => match body.as_ref() {
                FnBody::Block(b) => block_mentions_boundary_promote(b, interner),
                FnBody::Expr(e) => walk(e, interner),
            },
            _ => false,
        }
    }
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(Some(e), _) => {
                if walk(e, interner) {
                    return true;
                }
            }
            Stmt::Binding(b) => {
                if walk(&b.value, interner) {
                    return true;
                }
            }
            Stmt::While { cond, body, .. } => {
                if walk(cond, interner) || block_mentions_boundary_promote(body, interner) {
                    return true;
                }
            }
            Stmt::Loop { body, .. } | Stmt::Each { body, .. } => {
                if block_mentions_boundary_promote(body, interner) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn collect_births(
    block: &Block,
    in_req: bool,
    interner: &Interner,
    sites: &mut Vec<AllocSiteFacts>,
    next_id: &mut u32,
    local_to_site: &mut HashMap<SymId, u32>,
    skip_route_lambdas: bool,
) {
    let in_req = in_req || block_mentions_req_enter(block, interner);
    fn birth_expr(
        e: &Expr,
        in_req: bool,
        interner: &Interner,
        sites: &mut Vec<AllocSiteFacts>,
        next_id: &mut u32,
        local_to_site: &mut HashMap<SymId, u32>,
        skip_route_lambdas: bool,
    ) -> Option<u32> {
        match e {
            Expr::StructLit { fields, .. }
            | Expr::InferredStruct { fields, .. }
            | Expr::Record(fields, _) => {
                for f in fields {
                    birth_expr(
                        &f.value,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                if !in_req {
                    return None;
                }
                let id = *next_id;
                *next_id += 1;
                let mut facts = AllocSiteFacts::local_temp(id);
                facts.in_request_scope = true;
                sites.push(facts);
                Some(id)
            }
            Expr::Map(entries, _) => {
                for ent in entries {
                    birth_expr(
                        &ent.value,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                None
            }
            Expr::Array(xs, _) => {
                for x in xs {
                    birth_expr(
                        x,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                None
            }
            Expr::Call { callee, args, .. } => {
                let verb = match callee.as_ref() {
                    Expr::Ident(n, _) => Some(interner.resolve(*n)),
                    Expr::Field { field, .. } => Some(interner.resolve(*field)),
                    _ => None,
                };
                let route = verb.as_deref().is_some_and(is_route_verb);
                birth_expr(
                    callee,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                for a in args {
                    if skip_route_lambdas && route && matches!(a, Expr::Lambda { .. }) {
                        continue;
                    }
                    birth_expr(
                        a,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                None
            }
            Expr::Pipeline { method, base, args, .. } => {
                let route = is_route_verb(interner.resolve(*method));
                birth_expr(
                    base,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                for a in args {
                    if skip_route_lambdas && route && matches!(a, Expr::Lambda { .. }) {
                        continue;
                    }
                    birth_expr(
                        a,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                None
            }
            Expr::Block(b) => {
                collect_births(
                    b,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                None
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                birth_expr(
                    cond,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                collect_births(
                    then_block,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                if let Some(b) = else_block {
                    collect_births(
                        b,
                        in_req,
                        interner,
                        sites,
                        next_id,
                        local_to_site,
                        skip_route_lambdas,
                    );
                }
                None
            }
            Expr::Binary { left, right, .. } | Expr::Assign { target: left, value: right, .. } => {
                birth_expr(
                    left,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                birth_expr(
                    right,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                )
            }
            Expr::Field { base, .. } | Expr::Unary { expr: base, .. } => birth_expr(
                base,
                in_req,
                interner,
                sites,
                next_id,
                local_to_site,
                skip_route_lambdas,
            ),
            Expr::Lambda { body, .. } => {
                match body.as_ref() {
                    FnBody::Block(b) => {
                        collect_births(
                            b,
                            in_req,
                            interner,
                            sites,
                            next_id,
                            local_to_site,
                            skip_route_lambdas,
                        )
                    }
                    FnBody::Expr(e) => {
                        birth_expr(
                            e,
                            in_req,
                            interner,
                            sites,
                            next_id,
                            local_to_site,
                            skip_route_lambdas,
                        );
                    }
                }
                None
            }
            Expr::Or { left, right, .. } => {
                birth_expr(
                    left,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                birth_expr(
                    right,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                )
            }
            Expr::OrErr { left, body, .. } => {
                birth_expr(
                    left,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                collect_births(
                    body,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                None
            }
            Expr::Ident(n, _) => local_to_site.get(n).copied(),
            _ => None,
        }
    }
    for stmt in &block.stmts {
        match stmt {
            Stmt::Binding(b) => {
                if let Some(id) = birth_expr(
                    &b.value,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                ) {
                    local_to_site.insert(b.name, id);
                }
            }
            Stmt::Expr(e) | Stmt::Return(Some(e), _) => {
                birth_expr(
                    e,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
            }
            Stmt::While { cond, body, .. } => {
                birth_expr(
                    cond,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
                collect_births(
                    body,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
            }
            Stmt::Loop { body, .. } | Stmt::Each { body, .. } => {
                collect_births(
                    body,
                    in_req,
                    interner,
                    sites,
                    next_id,
                    local_to_site,
                    skip_route_lambdas,
                );
            }
            _ => {}
        }
    }
}

fn site_mut_by_id<'a>(sites: &'a mut [AllocSiteFacts], id: u32) -> Option<&'a mut AllocSiteFacts> {
    sites.iter_mut().find(|s| s.id == id)
}

fn propagate_escapes(
    block: &Block,
    interner: &Interner,
    has_boundary: bool,
    local_to_site: &HashMap<SymId, u32>,
    sites: &mut [AllocSiteFacts],
    module_fns: &HashMap<String, &Function>,
) {
    fn expr_site_ids(e: &Expr, local_to_site: &HashMap<SymId, u32>, out: &mut Vec<u32>) {
        match e {
            Expr::Ident(n, _) => {
                if let Some(id) = local_to_site.get(n) {
                    out.push(*id);
                }
            }
            Expr::StructLit { .. } | Expr::InferredStruct { .. } | Expr::Record(_, _) => {
                // Nested births handled separately; aliases come from bindings.
            }
            Expr::Call { callee, args, .. } => {
                expr_site_ids(callee, local_to_site, out);
                for a in args {
                    expr_site_ids(a, local_to_site, out);
                }
            }
            Expr::Pipeline { base, args, .. } => {
                expr_site_ids(base, local_to_site, out);
                for a in args {
                    expr_site_ids(a, local_to_site, out);
                }
            }
            Expr::Binary { left, right, .. } | Expr::Assign { target: left, value: right, .. } => {
                expr_site_ids(left, local_to_site, out);
                expr_site_ids(right, local_to_site, out);
            }
            Expr::Field { base, .. } | Expr::Unary { expr: base, .. } => {
                expr_site_ids(base, local_to_site, out)
            }
            Expr::Array(xs, _) => {
                for x in xs {
                    expr_site_ids(x, local_to_site, out);
                }
            }
            Expr::Or { left, right, .. } => {
                expr_site_ids(left, local_to_site, out);
                expr_site_ids(right, local_to_site, out);
            }
            Expr::Block(b) => {
                for stmt in &b.stmts {
                    match stmt {
                        Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Binding(Binding { value: e, .. }) => {
                            expr_site_ids(e, local_to_site, out);
                        }
                        _ => {}
                    }
                }
            }
            Expr::Lambda { body, .. } => match body.as_ref() {
                FnBody::Expr(e) => expr_site_ids(e, local_to_site, out),
                FnBody::Block(b) => {
                    for stmt in &b.stmts {
                        if let Stmt::Expr(e) | Stmt::Return(Some(e), _) = stmt {
                            expr_site_ids(e, local_to_site, out);
                        }
                    }
                }
            },
            _ => {}
        }
    }

    fn mark_sites(sites: &mut [AllocSiteFacts], ids: &[u32], f: impl Fn(&mut AllocSiteFacts)) {
        for id in ids {
            if let Some(s) = site_mut_by_id(sites, *id) {
                f(s);
            }
        }
    }

    fn walk_expr(
        e: &Expr,
        interner: &Interner,
        has_boundary: bool,
        local_to_site: &HashMap<SymId, u32>,
        sites: &mut [AllocSiteFacts],
        in_escaping_lambda: bool,
        module_fns: &HashMap<String, &Function>,
    ) {
        match e {
            Expr::Assign { target, value, .. } => {
                let mut ids = Vec::new();
                expr_site_ids(value, local_to_site, &mut ids);
                match target.as_ref() {
                    Expr::Ident(n, _) => {
                        let name = interner.resolve(*n);
                        if is_global_or_state_name(name) {
                            mark_sites(sites, &ids, |s| s.stored_into_global = true);
                        } else if is_durable_root_name(name) {
                            mark_sites(sites, &ids, |s| {
                                s.stored_into_durable = true;
                                if has_boundary {
                                    s.has_boundary_promote = true;
                                }
                            });
                        }
                    }
                    Expr::Field { base, .. } => {
                        if let Expr::Ident(n, _) = base.as_ref() {
                            let name = interner.resolve(*n);
                            if is_global_or_state_name(name) {
                                mark_sites(sites, &ids, |s| s.stored_into_global = true);
                            } else if is_durable_root_name(name) {
                                mark_sites(sites, &ids, |s| {
                                    s.stored_into_durable = true;
                                    if has_boundary {
                                        s.has_boundary_promote = true;
                                    }
                                });
                            } else {
                                // Store into unknown object field — Conservative.
                                mark_sites(sites, &ids, |s| {
                                    s.mark_incomplete("store_into_unknown_field")
                                });
                            }
                        } else {
                            mark_sites(sites, &ids, |s| {
                                s.mark_incomplete("store_into_complex_lvalue")
                            });
                        }
                    }
                    Expr::Index { base, .. } => {
                        if let Expr::Ident(n, _) = base.as_ref() {
                            let name = interner.resolve(*n);
                            if is_durable_root_name(name) || is_global_or_state_name(name) {
                                mark_sites(sites, &ids, |s| s.put_in_durable_collection = true);
                            }
                        } else {
                            mark_sites(sites, &ids, |s| {
                                s.mark_incomplete("index_store_unknown_base")
                            });
                        }
                    }
                    _ => {
                        mark_sites(sites, &ids, |s| s.mark_incomplete("assign_complex_target"));
                    }
                }
                walk_expr(
                    value,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                walk_expr(
                    target,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
            }
            Expr::Call { callee, args, .. } => {
                let cname = match callee.as_ref() {
                    Expr::Ident(n, _) => Some(interner.resolve(*n)),
                    Expr::Field { field, .. } => Some(interner.resolve(*field)),
                    _ => None,
                };
                let mut ids = Vec::new();
                for a in args {
                    expr_site_ids(a, local_to_site, &mut ids);
                }
                if let Some(name) = cname {
                    if name == "spawn" || name == "send" || name == "chan_send" {
                        mark_sites(sites, &ids, |s| s.sent_cross_task = true);
                    } else if name == "socket_req_promote" || name == "conn_req_revive" {
                        // Boundary promote of roots — values flowing in are Stay+boundary.
                        mark_sites(sites, &ids, |s| s.has_boundary_promote = true);
                    } else if module_fns.contains_key(name) {
                        // In-module callee: births expanded in `expand_in_module_request_callees`.
                    } else if !is_known_safe_callee(name) {
                        mark_sites(sites, &ids, |s| {
                            s.mark_incomplete(&format!("unknown_call:{name}"))
                        });
                    }
                } else if !ids.is_empty() {
                    mark_sites(sites, &ids, |s| s.mark_incomplete("unknown_callee_expr"));
                }
                walk_expr(
                    callee,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                for a in args {
                    walk_expr(
                        a,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
            }
            Expr::Lambda { body, captures, .. } => {
                let mut ids = Vec::new();
                for cap in captures {
                    if let Some(id) = local_to_site.get(&cap.name) {
                        ids.push(*id);
                    }
                }
                // Also scan body for free idents that alias sites.
                match body.as_ref() {
                    FnBody::Expr(e) => expr_site_ids(e, local_to_site, &mut ids),
                    FnBody::Block(b) => {
                        for stmt in &b.stmts {
                            if let Stmt::Expr(e) | Stmt::Return(Some(e), _) = stmt {
                                expr_site_ids(e, local_to_site, &mut ids);
                            }
                        }
                    }
                }
                if in_escaping_lambda || !ids.is_empty() {
                    mark_sites(sites, &ids, |s| s.captured_by_escaping_closure = true);
                }
                match body.as_ref() {
                    FnBody::Expr(e) => walk_expr(
                        e,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        true,
                        module_fns,
                    ),
                    FnBody::Block(b) => walk_block(
                        b,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        true,
                        module_fns,
                    ),
                }
            }
            Expr::Pipeline { base, args, .. } => {
                walk_expr(
                    base,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                for a in args {
                    walk_expr(
                        a,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
            }
            Expr::Block(b) => walk_block(
                b,
                interner,
                has_boundary,
                local_to_site,
                sites,
                in_escaping_lambda,
                module_fns,
            ),
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk_expr(
                    cond,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                walk_block(
                    then_block,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                if let Some(b) = else_block {
                    walk_block(
                        b,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
            }
            Expr::Binary { left, right, .. } | Expr::Or { left, right, .. } => {
                walk_expr(
                    left,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                walk_expr(
                    right,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
            }
            Expr::Field { base, .. } | Expr::Unary { expr: base, .. } => walk_expr(
                base,
                interner,
                has_boundary,
                local_to_site,
                sites,
                in_escaping_lambda,
                module_fns,
            ),
            Expr::Array(xs, _) => {
                for x in xs {
                    walk_expr(
                        x,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
            }
            Expr::OrErr { left, body, .. } => {
                walk_expr(
                    left,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
                walk_block(
                    body,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                );
            }
            Expr::Chan { capacity, .. } => walk_expr(
                capacity,
                interner,
                has_boundary,
                local_to_site,
                sites,
                in_escaping_lambda,
                module_fns,
            ),
            Expr::Spawn { body, .. } => {
                let mut ids = Vec::new();
                expr_site_ids(body, local_to_site, &mut ids);
                mark_sites(sites, &ids, |s| s.sent_cross_task = true);
                walk_expr(body, interner, has_boundary, local_to_site, sites, true, module_fns);
            }
            _ => {}
        }
    }

    fn walk_block(
        block: &Block,
        interner: &Interner,
        has_boundary: bool,
        local_to_site: &HashMap<SymId, u32>,
        sites: &mut [AllocSiteFacts],
        in_escaping_lambda: bool,
        module_fns: &HashMap<String, &Function>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Return(Some(e), _) => {
                    let mut ids = Vec::new();
                    expr_site_ids(e, local_to_site, &mut ids);
                    mark_sites(sites, &ids, |s| {
                        if has_boundary {
                            // Returning after a promote/revive path: treat as
                            // durable return unless still a pure local Stay.
                            s.returned_as_durable = true;
                        } else {
                            s.returned_as_durable = true;
                        }
                    });
                    walk_expr(
                        e,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                        module_fns,
                    );
                }
                Stmt::Expr(e) => walk_expr(
                    e,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                ),
                Stmt::Binding(b) => {
                    // Rebind durable roots: `app = @app.get(...)` etc.
                    let name = interner.resolve(b.name);
                    let mut ids = Vec::new();
                    expr_site_ids(&b.value, local_to_site, &mut ids);
                    if is_durable_root_name(name) {
                        mark_sites(sites, &ids, |s| {
                            s.stored_into_durable = true;
                            if has_boundary {
                                s.has_boundary_promote = true;
                            }
                        });
                        if let Some(id) = local_to_site.get(&b.name) {
                            mark_sites(sites, &[*id], |s| {
                                s.stored_into_durable = true;
                                if has_boundary {
                                    s.has_boundary_promote = true;
                                }
                            });
                        }
                    } else if is_global_or_state_name(name) {
                        mark_sites(sites, &ids, |s| s.stored_into_global = true);
                    }
                    walk_expr(
                        &b.value,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
                Stmt::While { cond, body, .. } => {
                    walk_expr(
                        cond,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                    walk_block(
                        body,
                        interner,
                        has_boundary,
                        local_to_site,
                        sites,
                        in_escaping_lambda,
                    module_fns,
                    );
                }
                Stmt::Loop { body, .. } | Stmt::Each { body, .. } => walk_block(
                    body,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    in_escaping_lambda,
                    module_fns,
                ),
                _ => {}
            }
        }
    }

    walk_block(
                    block,
                    interner,
                    has_boundary,
                    local_to_site,
                    sites,
                    false,
                    module_fns,
                );
}

pub(crate) fn block_mentions_req_enter(block: &Block, interner: &Interner) -> bool {
    fn walk_expr(e: &Expr, interner: &Interner) -> bool {
        match e {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(n, _) = callee.as_ref() {
                    let name = interner.resolve(*n);
                    if name == "socket_req_enter" || name == "socket_req_promote" || name == "conn_req_revive"
                    {
                        return true;
                    }
                }
                args.iter().any(|a| walk_expr(a, interner)) || walk_expr(callee, interner)
            }
            Expr::Pipeline { base, args, .. } => {
                walk_expr(base, interner) || args.iter().any(|a| walk_expr(a, interner))
            }
            Expr::Block(b) => block_mentions_req_enter(b, interner),
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk_expr(cond, interner)
                    || block_mentions_req_enter(then_block, interner)
                    || else_block
                        .as_ref()
                        .is_some_and(|b| block_mentions_req_enter(b, interner))
            }
            Expr::Binary { left, right, .. } => {
                walk_expr(left, interner) || walk_expr(right, interner)
            }
            Expr::Field { base, .. } => walk_expr(base, interner),
            Expr::Assign { target, value, .. } => {
                walk_expr(target, interner) || walk_expr(value, interner)
            }
            _ => false,
        }
    }
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(e) | Stmt::Return(Some(e), _) => {
                if walk_expr(e, interner) {
                    return true;
                }
            }
            Stmt::Binding(b) => {
                if walk_expr(&b.value, interner) {
                    return true;
                }
            }
            Stmt::While { cond, body, .. } => {
                if walk_expr(cond, interner) || block_mentions_req_enter(body, interner) {
                    return true;
                }
            }
            Stmt::Loop { body, .. } | Stmt::Each { body, .. } => {
                if block_mentions_req_enter(body, interner) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn expr_is_pure_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::None(_) => true,
        Expr::Binary { left, right, .. } => {
            expr_is_pure_literal(left) && expr_is_pure_literal(right)
        }
        Expr::Unary { expr, .. } => expr_is_pure_literal(expr),
        _ => false,
    }
}

fn fold_block_only(block: &mut Block, interner: &mut Interner, meta: &mut OptMeta) {
    for stmt in &mut block.stmts {
        optimize_stmt_shallow(stmt, interner, meta);
        if let Stmt::Each { iter, body, .. } = stmt {
            mark_bce_in_block(body, iter, meta);
            fold_block_only(body, interner, meta);
        }
    }
}

fn mark_bce_walk_block(block: &Block, meta: &mut OptMeta) {
    for stmt in &block.stmts {
        if let Stmt::Each { iter, body, .. } = stmt {
            mark_bce_in_block(body, iter, meta);
            mark_bce_walk_block(body, meta);
        }
    }
}

fn optimize_function(
    f: &mut Function,
    typed: &TypedModule,
    interner: &mut Interner,
    meta: &mut OptMeta,
) {
    let _ = typed;
    match &mut f.body {
        FnBody::Expr(e) => {
            let taken = std::mem::replace(e, Expr::None(Span::default()));
            *e = fold_expr(taken, interner, meta);
        }
        FnBody::Block(b) => {
            fold_block_only(b, interner, meta);
        }
    }
}

fn optimize_block(
    block: &mut Block,
    typed: &TypedModule,
    interner: &mut Interner,
    meta: &mut OptMeta,
    escaping: &mut HashSet<SymId>,
) {
    // First pass: fold + prune + collect escapes for SROA decisions.
    for stmt in &mut block.stmts {
        optimize_stmt(stmt, typed, interner, meta, escaping);
    }
    // SROA: rewrite non-escaping struct locals.
    sroa_block(block, typed, interner, meta, escaping);
    // LICM on each loops.
    licm_block(block, interner, meta);
}

fn optimize_stmt(
    stmt: &mut Stmt,
    typed: &TypedModule,
    interner: &mut Interner,
    meta: &mut OptMeta,
    escaping: &mut HashSet<SymId>,
) {
    match stmt {
        Stmt::Binding(b) => {
            b.value = fold_expr(b.value.clone(), interner, meta);
            collect_escapes(&b.value, escaping);
        }
        Stmt::Expr(e) | Stmt::Fail(e, _) => {
            *e = fold_expr(e.clone(), interner, meta);
            collect_escapes(e, escaping);
        }
        Stmt::Panic(Some(e), _) => {
            *e = fold_expr(e.clone(), interner, meta);
            collect_escapes(e, escaping);
        }
        Stmt::Panic(None, _) => {}
        Stmt::Return(Some(e), _) => {
            *e = fold_expr(e.clone(), interner, meta);
            collect_escapes(e, escaping);
        }
        Stmt::Return(None, _) => {}
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            *cond = fold_expr(cond.clone(), interner, meta);
            optimize_block(else_block, typed, interner, meta, escaping);
        }
        Stmt::Each { iter, body, .. } => {
            *iter = fold_expr(iter.clone(), interner, meta);
            // Mark index exprs inside each over arrays/ranges as BCE candidates.
            mark_bce_in_block(body, iter, meta);
            optimize_block(body, typed, interner, meta, escaping);
        }
        Stmt::While { cond, body, .. } => {
            *cond = fold_expr(cond.clone(), interner, meta);
            optimize_block(body, typed, interner, meta, escaping);
        }
        Stmt::Loop { body, .. } => {
            optimize_block(body, typed, interner, meta, escaping);
        }
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            for v in values.iter_mut() {
                *v = fold_expr(v.clone(), interner, meta);
            }
            for t in targets.iter_mut() {
                *t = fold_expr(t.clone(), interner, meta);
            }
        }
        Stmt::Guarded { stmt, cond, .. } => {
            *cond = fold_expr(cond.clone(), interner, meta);
            optimize_stmt(stmt, typed, interner, meta, escaping);
        }
        Stmt::When { subjects, arms, .. } => {
            for s in subjects.iter_mut() {
                *s = fold_expr(s.clone(), interner, meta);
            }
            for arm in arms {
                fold_when_pattern(&mut arm.pattern, interner, meta);
                arm.body = fold_expr(arm.body.clone(), interner, meta);
            }
        }
        Stmt::Scope { body, .. } => {
            optimize_block(body, typed, interner, meta, escaping);
        }
        Stmt::Destructure { value, .. } => {
            *value = fold_expr(value.clone(), interner, meta);
            collect_escapes(value, escaping);
        }
        Stmt::Comptime(_) => {}
    }
}

// ─── Constant folding & dead branches ───────────────────────────────────────

fn fold_when_pattern(pat: &mut WhenPattern, interner: &mut Interner, meta: &mut OptMeta) {
    match pat {
        WhenPattern::Expr(e) | WhenPattern::After(e) => {
            *e = fold_expr(e.clone(), interner, meta);
        }
        WhenPattern::Tuple { elems, .. } => {
            for e in elems {
                fold_when_pattern(e, interner, meta);
            }
        }
        WhenPattern::Else | WhenPattern::VariantBind { .. } => {}
    }
}

fn fold_expr(expr: Expr, interner: &mut Interner, meta: &mut OptMeta) -> Expr {
    match expr {
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            let left = fold_expr(*left, interner, meta);
            let right = fold_expr(*right, interner, meta);
            if let Some(folded) = try_fold_binary(op, &left, &right, span, interner, meta) {
                meta.stats.folds += 1;
                return folded;
            }
            // Algebraic identities
            if let Some(id) = algebraic_identity(op, &left, &right, span, meta) {
                meta.stats.folds += 1;
                return id;
            }
            Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            }
        }
        Expr::Unary { op, expr, span } => {
            let inner = fold_expr(*expr, interner, meta);
            match (&op, &inner) {
                (UnOp::Neg, Expr::Int(n, _)) => {
                    meta.stats.folds += 1;
                    Expr::Int(-n, span)
                }
                (UnOp::Neg, Expr::Float(f, _)) => {
                    meta.stats.folds += 1;
                    Expr::Float(-f, span)
                }
                (UnOp::Not, Expr::Bool(b, _)) => {
                    meta.stats.folds += 1;
                    Expr::Bool(!b, span)
                }
                _ => Expr::Unary {
                    op,
                    expr: Box::new(inner),
                    span,
                },
            }
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            span,
        } => {
            let cond = fold_expr(*cond, interner, meta);
            match cond {
                Expr::Bool(true, _) => {
                    meta.stats.dead_branches += 1;
                    // Replace with then-block as expression block.
                    let mut tb = then_block;
                    for s in &mut tb.stmts {
                        if let Stmt::Expr(e) = s {
                            *e = fold_expr(e.clone(), interner, meta);
                        }
                    }
                    Expr::Block(tb)
                }
                Expr::Bool(false, _) => {
                    meta.stats.dead_branches += 1;
                    if let Some(mut eb) = else_block {
                        for s in &mut eb.stmts {
                            if let Stmt::Expr(e) = s {
                                *e = fold_expr(e.clone(), interner, meta);
                            }
                        }
                        Expr::Block(eb)
                    } else {
                        Expr::None(span)
                    }
                }
                other => {
                    let mut tb = then_block;
                    for s in &mut tb.stmts {
                        optimize_stmt_shallow(s, interner, meta);
                    }
                    let eb = else_block.map(|mut b| {
                        for s in &mut b.stmts {
                            optimize_stmt_shallow(s, interner, meta);
                        }
                        b
                    });
                    Expr::If {
                        cond: Box::new(other),
                        then_block: tb,
                        else_block: eb,
                        span,
                    }
                }
            }
        }
        Expr::Call {
            callee,
            args,
            punning,
            span,
        } => Expr::Call {
            callee: Box::new(fold_expr(*callee, interner, meta)),
            args: args
                .into_iter()
                .map(|a| fold_expr(a, interner, meta))
                .collect(),
            punning,
            span,
        },
        Expr::Field { base, field, span } => Expr::Field {
            base: Box::new(fold_expr(*base, interner, meta)),
            field,
            span,
        },
        Expr::Index { base, index, span } => Expr::Index {
            base: Box::new(fold_expr(*base, interner, meta)),
            index: Box::new(fold_expr(*index, interner, meta)),
            span,
        },
        Expr::Assign {
            op,
            target,
            value,
            span,
        } => Expr::Assign {
            op,
            target: Box::new(fold_expr(*target, interner, meta)),
            value: Box::new(fold_expr(*value, interner, meta)),
            span,
        },
        Expr::Array(elems, span) => Expr::Array(
            elems
                .into_iter()
                .map(|e| fold_expr(e, interner, meta))
                .collect(),
            span,
        ),
        Expr::Pipeline {
            base,
            method,
            args,
            span,
        } => Expr::Pipeline {
            base: Box::new(fold_expr(*base, interner, meta)),
            method,
            args: args
                .into_iter()
                .map(|a| fold_expr(a, interner, meta))
                .collect(),
            span,
        },
        Expr::Or { left, right, span } => Expr::Or {
            left: Box::new(fold_expr(*left, interner, meta)),
            right: Box::new(fold_expr(*right, interner, meta)),
            span,
        },
        Expr::OrErr { left, body, span } => {
            let left = fold_expr(*left, interner, meta);
            let mut body = body;
            for s in &mut body.stmts {
                optimize_stmt_shallow(s, interner, meta);
            }
            Expr::OrErr {
                left: Box::new(left),
                body,
                span,
            }
        }
        Expr::Block(mut b) => {
            for s in &mut b.stmts {
                optimize_stmt_shallow(s, interner, meta);
            }
            Expr::Block(b)
        }
        other => other,
    }
}

fn optimize_stmt_shallow(stmt: &mut Stmt, interner: &mut Interner, meta: &mut OptMeta) {
    match stmt {
        Stmt::Binding(b) => b.value = fold_expr(b.value.clone(), interner, meta),
        Stmt::Expr(e) | Stmt::Fail(e, _) => *e = fold_expr(e.clone(), interner, meta),
        Stmt::Return(Some(e), _) => *e = fold_expr(e.clone(), interner, meta),
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            *cond = fold_expr(cond.clone(), interner, meta);
            for s in &mut else_block.stmts {
                optimize_stmt_shallow(s, interner, meta);
            }
        }
        Stmt::Each { iter, body, .. } => {
            *iter = fold_expr(iter.clone(), interner, meta);
            for s in &mut body.stmts {
                optimize_stmt_shallow(s, interner, meta);
            }
        }
        _ => {}
    }
}

fn try_fold_binary(
    op: BinOp,
    left: &Expr,
    right: &Expr,
    span: Span,
    interner: &mut Interner,
    meta: &mut OptMeta,
) -> Option<Expr> {
    match (left, right) {
        (Expr::Int(a, _), Expr::Int(b, _)) => {
            let v = match op {
                BinOp::Add => a.wrapping_add(*b),
                BinOp::Sub => a.wrapping_sub(*b),
                BinOp::Mul => a.wrapping_mul(*b),
                BinOp::Div if *b != 0 => a.wrapping_div(*b),
                BinOp::Rem if *b != 0 => a.wrapping_rem(*b),
                BinOp::Eq => return Some(Expr::Bool(a == b, span)),
                BinOp::Ne => return Some(Expr::Bool(a != b, span)),
                BinOp::Lt => return Some(Expr::Bool(a < b, span)),
                BinOp::Le => return Some(Expr::Bool(a <= b, span)),
                BinOp::Gt => return Some(Expr::Bool(a > b, span)),
                BinOp::Ge => return Some(Expr::Bool(a >= b, span)),
                _ => return None,
            };
            Some(Expr::Int(v, span))
        }
        (Expr::Float(a, _), Expr::Float(b, _)) => {
            let v = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                BinOp::Eq => return Some(Expr::Bool(a == b, span)),
                BinOp::Ne => return Some(Expr::Bool(a != b, span)),
                BinOp::Lt => return Some(Expr::Bool(a < b, span)),
                BinOp::Le => return Some(Expr::Bool(a <= b, span)),
                BinOp::Gt => return Some(Expr::Bool(a > b, span)),
                BinOp::Ge => return Some(Expr::Bool(a >= b, span)),
                _ => return None,
            };
            Some(Expr::Float(v, span))
        }
        (Expr::Float(a, _), Expr::Int(b, _)) | (Expr::Int(b, _), Expr::Float(a, _))
            if matches!(
                op,
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
            ) =>
        {
            // Normalize to float fold when mixed — only when left was float pattern above.
            let (fa, fb) = match (left, right) {
                (Expr::Float(a, _), Expr::Int(b, _)) => (*a, *b as f64),
                (Expr::Int(a, _), Expr::Float(b, _)) => (*a as f64, *b),
                _ => return None,
            };
            let v = match op {
                BinOp::Add => fa + fb,
                BinOp::Sub => fa - fb,
                BinOp::Mul => fa * fb,
                BinOp::Div => fa / fb,
                BinOp::Rem => fa % fb,
                _ => return None,
            };
            Some(Expr::Float(v, span))
        }
        (Expr::Bool(a, _), Expr::Bool(b, _)) => match op {
            BinOp::And => Some(Expr::Bool(*a && *b, span)),
            BinOp::Or => Some(Expr::Bool(*a || *b, span)),
            BinOp::Eq => Some(Expr::Bool(a == b, span)),
            BinOp::Ne => Some(Expr::Bool(a != b, span)),
            _ => None,
        },
        (Expr::String(a, _), Expr::String(b, _)) if matches!(op, BinOp::Add) => {
            let sa = interner.resolve(*a).to_owned();
            let sb = interner.resolve(*b).to_owned();
            let fused = interner.intern(&(sa + &sb));
            meta.stats.folds += 1;
            Some(Expr::String(fused, span))
        }
        _ => None,
    }
}

fn algebraic_identity(
    op: BinOp,
    left: &Expr,
    right: &Expr,
    span: Span,
    meta: &mut OptMeta,
) -> Option<Expr> {
    match op {
        BinOp::Add => {
            if is_int_zero(right) {
                return Some(left.clone());
            }
            if is_int_zero(left) {
                return Some(right.clone());
            }
        }
        BinOp::Sub => {
            if is_int_zero(right) {
                return Some(left.clone());
            }
        }
        BinOp::Mul => {
            if is_int_one(right) || is_int_one(left) {
                return Some(if is_int_one(right) {
                    left.clone()
                } else {
                    right.clone()
                });
            }
            if is_int_zero(right) || is_int_zero(left) {
                return Some(Expr::Int(0, span));
            }
            // Strength-reduce x * 2^k → will become ShlImm at peephole when emitted.
            let _ = meta;
        }
        BinOp::Div if is_int_one(right) => {
            return Some(left.clone());
        }
        _ => {}
    }
    None
}

fn is_int_zero(e: &Expr) -> bool {
    matches!(e, Expr::Int(0, _))
}
fn is_int_one(e: &Expr) -> bool {
    matches!(e, Expr::Int(1, _))
}

fn is_power_of_two(n: i64) -> Option<u8> {
    if n > 0 && (n & (n - 1)) == 0 {
        Some(n.trailing_zeros() as u8)
    } else {
        None
    }
}

// ─── Escape analysis & SROA ─────────────────────────────────────────────────

fn collect_escapes(expr: &Expr, escaping: &mut HashSet<SymId>) {
    match expr {
        Expr::Ident(n, _) => {
            let _ = n;
        }
        Expr::Unary {
            op: UnOp::MutRef,
            expr,
            ..
        } => {
            if let Expr::Ident(n, _) = expr.as_ref() {
                escaping.insert(*n);
            }
            collect_escapes(expr, escaping);
        }
        Expr::Call { args, .. } => {
            for a in args {
                if let Expr::Ident(n, _) = a {
                    escaping.insert(*n);
                }
                collect_escapes(a, escaping);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            if let Expr::Ident(n, _) = base.as_ref() {
                // Method call may mutate / observe whole aggregate.
                escaping.insert(*n);
            }
            collect_escapes(base, escaping);
            for a in args {
                collect_escapes(a, escaping);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_escapes(left, escaping);
            collect_escapes(right, escaping);
        }
        Expr::Field { base, .. } => collect_escapes(base, escaping),
        Expr::Index { base, index, .. } => {
            collect_escapes(base, escaping);
            collect_escapes(index, escaping);
        }
        Expr::Assign { target, value, .. } => {
            collect_escapes(target, escaping);
            collect_escapes(value, escaping);
        }
        Expr::Array(xs, _) => {
            for x in xs {
                collect_escapes(x, escaping);
            }
        }
        Expr::StructLit { fields, .. }
        | Expr::InferredStruct { fields, .. }
        | Expr::Record(fields, _) => {
            for f in fields {
                collect_escapes(&f.value, escaping);
            }
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_escapes(cond, escaping);
            collect_escapes_block(then_block, escaping);
            if let Some(b) = else_block {
                collect_escapes_block(b, escaping);
            }
        }
        Expr::Block(b) => collect_escapes_block(b, escaping),
        Expr::Or { left, right, .. } => {
            collect_escapes(left, escaping);
            collect_escapes(right, escaping);
        }
        Expr::OrErr { left, body, .. } => {
            collect_escapes(left, escaping);
            collect_escapes_block(body, escaping);
        }
        Expr::Spawn { body, .. } => collect_escapes(body, escaping),
        _ => {}
    }
}

fn collect_escapes_block(block: &Block, escaping: &mut HashSet<SymId>) {
    for s in &block.stmts {
        match s {
            Stmt::Binding(b) => collect_escapes(&b.value, escaping),
            Stmt::Expr(e) | Stmt::Fail(e, _) => collect_escapes(e, escaping),
            Stmt::Return(Some(e), _) => collect_escapes(e, escaping),
            Stmt::Each { iter, body, .. } => {
                collect_escapes(iter, escaping);
                collect_escapes_block(body, escaping);
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                collect_escapes(cond, escaping);
                collect_escapes_block(else_block, escaping);
            }
            Stmt::Scope { body, .. } => collect_escapes_block(body, escaping),
            Stmt::Destructure { value, .. } => collect_escapes(value, escaping),
            _ => {}
        }
    }
}

fn sroa_block(
    block: &mut Block,
    typed: &TypedModule,
    interner: &mut Interner,
    meta: &mut OptMeta,
    escaping: &HashSet<SymId>,
) {
    let mut rewrites: Vec<(usize, Vec<Stmt>, SymId, HashMap<SymId, SymId>)> = Vec::new();
    for (i, stmt) in block.stmts.iter().enumerate() {
        if let Stmt::Binding(b) = stmt {
            if escaping.contains(&b.name) {
                continue;
            }
            if let Expr::StructLit { fields, .. }
            | Expr::InferredStruct { fields, .. }
            | Expr::Record(fields, _) = &b.value
            {
                if fields.is_empty() {
                    continue;
                }
                // Only SROA when every field has a name and value is a leaf.
                if !fields.iter().all(|f| f.name.is_some()) {
                    continue;
                }
                let mut map = HashMap::new();
                let mut new_stmts = Vec::new();
                for f in fields {
                    let fname = f.name.unwrap();
                    let scalar = interner.intern(&format!(
                        "__sroa_{}_{}",
                        interner.resolve(b.name),
                        interner.resolve(fname)
                    ));
                    map.insert(fname, scalar);
                    new_stmts.push(Stmt::Binding(Binding {
                        exported: false,
                        mutable: b.mutable,
                        comptime: false,
                        name: scalar,
                        ty: None,
                        value: f.value.clone(),
                        span: b.span,
                        attrs: Vec::new(),
                        docs: b.docs,
                    }));
                }
                meta.stats.sroa_locals += 1;
                meta.sroa_map.insert(b.name, map.clone());
                rewrites.push((i, new_stmts, b.name, map));
            }
        }
    }
    if rewrites.is_empty() {
        let _ = typed;
        return;
    }
    // Apply from back to front so indices stay valid.
    for (idx, new_stmts, old, _map) in rewrites.into_iter().rev() {
        block.stmts.remove(idx);
        for (j, s) in new_stmts.into_iter().enumerate() {
            block.stmts.insert(idx + j, s);
        }
        rewrite_sroa_uses_block(block, old, meta);
    }
}

fn sroa_expr(
    expr: &mut Expr,
    typed: &TypedModule,
    interner: &mut Interner,
    meta: &mut OptMeta,
    escaping: &HashSet<SymId>,
) {
    let _ = (typed, interner, escaping);
    rewrite_sroa_uses_expr(expr, meta);
}

fn rewrite_sroa_uses_block(block: &mut Block, old: SymId, meta: &OptMeta) {
    for s in &mut block.stmts {
        match s {
            Stmt::Binding(b) => rewrite_sroa_uses_expr(&mut b.value, meta),
            Stmt::Expr(e) | Stmt::Fail(e, _) => rewrite_sroa_uses_expr(e, meta),
            Stmt::Return(Some(e), _) => rewrite_sroa_uses_expr(e, meta),
            Stmt::Each { iter, body, .. } => {
                rewrite_sroa_uses_expr(iter, meta);
                rewrite_sroa_uses_block(body, old, meta);
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                rewrite_sroa_uses_expr(cond, meta);
                rewrite_sroa_uses_block(else_block, old, meta);
            }
            Stmt::Scope { body, .. } => rewrite_sroa_uses_block(body, old, meta),
            _ => {}
        }
    }
    let _ = old;
}

fn rewrite_sroa_uses_expr(expr: &mut Expr, meta: &OptMeta) {
    match expr {
        Expr::Field { base, field, span } => {
            if let Expr::Ident(n, _) = base.as_ref()
                && let Some(map) = meta.sroa_map.get(n)
                && let Some(scalar) = map.get(field)
            {
                *expr = Expr::Ident(*scalar, *span);
                return;
            }
            rewrite_sroa_uses_expr(base, meta);
        }
        Expr::Assign { target, value, .. } => {
            // `p.x = v` → `__sroa_p_x = v`
            if let Expr::Field { base, field, span } = target.as_mut()
                && let Expr::Ident(n, _) = base.as_ref()
                && let Some(map) = meta.sroa_map.get(n)
                && let Some(scalar) = map.get(field)
            {
                **target = Expr::Ident(*scalar, *span);
            }
            rewrite_sroa_uses_expr(value, meta);
        }
        Expr::Binary { left, right, .. } => {
            rewrite_sroa_uses_expr(left, meta);
            rewrite_sroa_uses_expr(right, meta);
        }
        Expr::Unary { expr, .. } => rewrite_sroa_uses_expr(expr, meta),
        Expr::Call { callee, args, .. } => {
            rewrite_sroa_uses_expr(callee, meta);
            for a in args {
                rewrite_sroa_uses_expr(a, meta);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            rewrite_sroa_uses_expr(base, meta);
            for a in args {
                rewrite_sroa_uses_expr(a, meta);
            }
        }
        Expr::Index { base, index, .. } => {
            rewrite_sroa_uses_expr(base, meta);
            rewrite_sroa_uses_expr(index, meta);
        }
        Expr::Block(b) => {
            for s in &mut b.stmts {
                if let Stmt::Expr(e) = s {
                    rewrite_sroa_uses_expr(e, meta);
                }
            }
        }
        _ => {}
    }
}

// ─── LICM & BCE ─────────────────────────────────────────────────────────────

fn mark_bce_in_block(body: &Block, iter: &Expr, meta: &mut OptMeta) {
    // `for xs` / `for 0..n` — Index(xs, index) inside body is safe.
    let slice_sym = match iter {
        Expr::Ident(n, _) => Some(*n),
        Expr::Range { .. } => None, // range itself isn't indexed
        _ => None,
    };
    walk_block_for_bce(body, slice_sym, meta);
}

fn walk_block_for_bce(block: &Block, slice: Option<SymId>, meta: &mut OptMeta) {
    for s in &block.stmts {
        match s {
            Stmt::Expr(e) | Stmt::Fail(e, _) | Stmt::Return(Some(e), _) => {
                walk_expr_for_bce(e, slice, meta);
            }
            Stmt::Binding(b) => walk_expr_for_bce(&b.value, slice, meta),
            Stmt::Each { iter, body, .. } => {
                walk_expr_for_bce(iter, slice, meta);
                walk_block_for_bce(body, slice, meta);
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                walk_expr_for_bce(cond, slice, meta);
                walk_block_for_bce(else_block, slice, meta);
            }
            _ => {}
        }
    }
}

fn walk_expr_for_bce(expr: &Expr, slice: Option<SymId>, meta: &mut OptMeta) {
    match expr {
        Expr::Index { base, index, span } => {
            let base_ok = matches!((base.as_ref(), slice), (Expr::Ident(n, _), Some(s)) if *n == s);
            let index_ok = matches!(index.as_ref(), Expr::IndexIdent(_) | Expr::It(_));
            if base_ok && index_ok {
                meta.unchecked_index
                    .insert(super_frontend::meta::span_key(*span));
                meta.stats.bce_elided += 1;
            }
            walk_expr_for_bce(base, slice, meta);
            walk_expr_for_bce(index, slice, meta);
        }
        Expr::Binary { left, right, .. } => {
            walk_expr_for_bce(left, slice, meta);
            walk_expr_for_bce(right, slice, meta);
        }
        Expr::Unary { expr, .. } => walk_expr_for_bce(expr, slice, meta),
        Expr::Call { callee, args, .. } => {
            walk_expr_for_bce(callee, slice, meta);
            for a in args {
                walk_expr_for_bce(a, slice, meta);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            walk_expr_for_bce(base, slice, meta);
            for a in args {
                walk_expr_for_bce(a, slice, meta);
            }
        }
        Expr::Field { base, .. } => walk_expr_for_bce(base, slice, meta),
        Expr::Assign { target, value, .. } => {
            walk_expr_for_bce(target, slice, meta);
            walk_expr_for_bce(value, slice, meta);
        }
        _ => {}
    }
}

fn licm_block(block: &mut Block, interner: &mut Interner, meta: &mut OptMeta) {
    let mut i = 0;
    while i < block.stmts.len() {
        let is_each = matches!(&block.stmts[i], Stmt::Each { .. });
        if !is_each {
            i += 1;
            continue;
        }
        // Extract invariant bindings from the each body.
        let (iter_clone, mut body_stmts, each_span, mutable, parallel, bindings) = {
            let Stmt::Each {
                mutable,
                parallel,
                iter,
                bindings,
                body,
                span,
                ..
            } = &mut block.stmts[i]
            else {
                i += 1;
                continue;
            };
            (
                iter.clone(),
                std::mem::take(&mut body.stmts),
                *span,
                *mutable,
                *parallel,
                bindings.clone(),
            )
        };
        let mut hoisted = Vec::new();
        let mut kept = Vec::new();
        for stmt in body_stmts.drain(..) {
            if let Stmt::Binding(b) = &stmt
                && expr_loop_invariant(&b.value)
            {
                meta.stats.licm_hoists += 1;
                meta.hoisted
                    .push(format!("hoist {}", interner.resolve(b.name)));
                hoisted.push(stmt);
                continue;
            }
            kept.push(stmt);
        }
        // Rebuild each with remaining body.
        let new_each = Stmt::Each {
            mutable,
            parallel,
            reduction: None,
            iter: iter_clone,
            bindings,
            body: Block {
                stmts: kept,
                span: each_span,
            },
            span: each_span,
            attrs: Vec::new(),
        };
        // Insert hoisted bindings before the each.
        let n_hoist = hoisted.len();
        block.stmts.remove(i);
        for (j, h) in hoisted.into_iter().enumerate() {
            block.stmts.insert(i + j, h);
        }
        block.stmts.insert(i + n_hoist, new_each);
        i += n_hoist + 1;
    }
}

fn expr_loop_invariant(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::None(_) => true,
        Expr::Ident(_, _) => true, // locals defined outside — conservative OK for hoist of pure binds
        Expr::It(_) | Expr::IndexIdent(_) => false,
        Expr::Binary { left, right, .. } => expr_loop_invariant(left) && expr_loop_invariant(right),
        Expr::Unary {
            op: UnOp::MutRef, ..
        } => false,
        Expr::Unary { expr, .. } => expr_loop_invariant(expr),
        Expr::Field { base, .. } => expr_loop_invariant(base),
        Expr::Call { .. } | Expr::Pipeline { .. } | Expr::Index { .. } => false,
        _ => false,
    }
}

// ─── Peephole, LICM, BCE, jump-relocation compaction ────────────────────────

/// Optimization pipeline on a finished chunk:
/// 1. Fuse immediates / identity moves (leave `Nop` placeholders)
/// 2. Bytecode LICM + bounds-check elimination on `for` / `LoopStep` loops
/// 3. Strip every `Nop` via an `old_ip → new_ip` relocation table that patches
///    jumps, `LoopStep`, function/test entries, and opt-note sites
pub fn peephole(chunk: &mut Chunk) -> OptStats {
    let mut stats = OptStats::default();
    if chunk.ops.is_empty() {
        compute_live_intervals(chunk);
        return stats;
    }

    fuse_in_place(chunk, &mut stats);
    // Production pipeline: BCE + jump-relocation compaction.
    // Full LICM (`licm_and_bce`'s hoist pass) is validated by unit tests and
    // applied here after BCE so hoists see already-proven loops; gated to
    // Mul/Div windows with stack-arity + enclosing-variant checks.
    let licm = licm_and_bce(chunk);
    stats.licm_hoists = licm.licm_hoists;
    stats.bce_elided = licm.bce_elided;

    let stripped = strip_nops_with_relocation(chunk);
    stats.jump_chains = stripped.jump_chains;
    stats.moves_elided += stripped.moves_elided;

    debug_assert!(
        !chunk.ops.iter().any(|o| matches!(o, Op::Nop)),
        "Nop must not survive strip_nops_with_relocation"
    );

    compute_live_intervals(chunk);
    stats
}

fn op_is_jump_target(chunk: &Chunk, ip: usize) -> bool {
    let ip = ip as u32;
    chunk.ops.iter().any(|op| match op {
        Op::Jump(t)
        | Op::JumpIfFalse(t)
        | Op::JumpIfTrue(t)
        | Op::JumpIfLt(t)
        | Op::JumpIfGe(t)
        | Op::JumpIfEq(t)
        | Op::JumpIfNe(t)
        | Op::IterNext(t)
        | Op::ParEachBegin(t) => *t == ip,
        Op::LoopStep { body, .. } => *body == ip,
        _ => false,
    })
}

fn fuse_in_place(chunk: &mut Chunk, stats: &mut OptStats) {
    let n = chunk.ops.len();
    for i in 0..n.saturating_sub(1) {
        if let Op::Move(a, b) = chunk.ops[i]
            && a == b
        {
            chunk.ops[i] = Op::Nop;
            stats.moves_elided += 1;
            continue;
        }
        // Elide `LoadLocal(r); StoreLocal(r)` only when neither op is a jump
        // target — otherwise we delete the store that ternary/if merge jumps to
        // (`x = cond ? a : x` leaves `LoadLocal(x); StoreLocal(x)` on the else edge).
        if let (Op::LoadLocal(a), Op::StoreLocal(b)) = (&chunk.ops[i], &chunk.ops[i + 1])
            && *a == *b
            && !op_is_jump_target(chunk, i)
            && !op_is_jump_target(chunk, i + 1)
        {
            chunk.ops[i] = Op::Nop;
            chunk.ops[i + 1] = Op::Nop;
            stats.moves_elided += 1;
            continue;
        }
        // Fusing into op i+1 is unsound when i+1 is a merge point: the other edge
        // (`a + (if c { 1 } else { 0 })`) arrives there without op i's push.
        let next_is_target = op_is_jump_target(chunk, i + 1);
        if let Op::LoadConst(ci) = chunk.ops[i]
            && !next_is_target
            && let Some(Value::Int(imm)) = chunk.consts.get(ci as usize).cloned()
        {
            let left_hides_display = i > 0
                && match &chunk.ops[i - 1] {
                    // A string or a trait method is on the left. Folding the
                    // right-hand constant into AddImm would skip Display.
                    Op::LoadConst(left) => matches!(
                        chunk.consts.get(*left as usize),
                        Some(
                            Value::Str(_) | Value::Array(_) | Value::Map(_) | Value::Struct { .. }
                        )
                    ),
                    Op::Call(_, _)
                    | Op::CallMut(_, _, _)
                    | Op::CallHost(_, _)
                    | Op::CallValue(_) => true,
                    _ => false,
                };
            let fused = match chunk.ops[i + 1] {
                Op::Add if !left_hides_display && (0..=64).contains(&imm) => Some(Op::AddImm(imm)),
                Op::Sub if !left_hides_display && (0..=64).contains(&imm) => Some(Op::SubImm(imm)),
                Op::Mul if !left_hides_display && matches!(imm, 2 | 4 | 8 | 16 | 32 | 64) => {
                    Some(Op::ShlImm(imm.trailing_zeros() as u8))
                }
                _ => None,
            };
            if let Some(op) = fused {
                let detail = format!("{op:?}");
                chunk.ops[i] = Op::Nop;
                chunk.ops[i + 1] = op;
                chunk.note(i as u32 + 1, OptNoteKind::Fused, detail);
                stats.peephole_fused += 1;
            }
        }
        // Safe compare+branch: only `cmp; JumpIfTrue` (no Ensure skip-Jump).
        let fused_j = match (&chunk.ops[i], &chunk.ops[i + 1]) {
            (Op::Lt, Op::JumpIfTrue(t)) => Some(Op::JumpIfLt(*t)),
            (Op::Ge, Op::JumpIfTrue(t)) => Some(Op::JumpIfGe(*t)),
            (Op::Eq, Op::JumpIfTrue(t)) => Some(Op::JumpIfEq(*t)),
            (Op::Ne, Op::JumpIfTrue(t)) => Some(Op::JumpIfNe(*t)),
            _ => None,
        };
        if let Some(op) = fused_j
            && !next_is_target
        {
            chunk.ops[i] = Op::Nop;
            chunk.ops[i + 1] = op;
            chunk.note(i as u32 + 1, OptNoteKind::Fused, "compare+branch");
            stats.peephole_fused += 1;
        }
    }
}

/// Build `old_ip → new_ip`, patch every absolute target in the chunk, drop Nops.
fn strip_nops_with_relocation(chunk: &mut Chunk) -> OptStats {
    let stats = OptStats::default();
    let n = chunk.ops.len();
    if n == 0 {
        return stats;
    }

    let mut keep = vec![true; n];
    for (i, op) in chunk.ops.iter().enumerate() {
        if matches!(op, Op::Nop) {
            keep[i] = false;
        }
    }
    if keep.iter().all(|&k| k) {
        return stats;
    }

    // First pass: new index of each kept instruction.
    let mut new_index = vec![0u32; n];
    let mut out: Vec<Op> = Vec::with_capacity(keep.iter().filter(|k| **k).count());
    for (old, &k) in keep.iter().enumerate() {
        new_index[old] = out.len() as u32;
        if k {
            out.push(chunk.ops[old].clone());
        }
    }
    let end = out.len() as u32;
    // Deleted sites relocate to the next live instruction (or end).
    for old in 0..n {
        if !keep[old] {
            let mut j = old + 1;
            while j < n && !keep[j] {
                j += 1;
            }
            new_index[old] = if j < n { new_index[j] } else { end };
        }
    }

    for op in &mut out {
        remap_op_jumps(op, &new_index, end);
    }

    // Function / test entry points and opt-note sites.
    for f in &mut chunk.functions {
        f.entry = map_ip(f.entry, &new_index, end);
        for (lo, hi) in &mut f.live_intervals {
            *lo = map_ip(*lo, &new_index, end);
            *hi = map_ip(*hi, &new_index, end);
        }
    }
    for t in &mut chunk.tests {
        t.entry = map_ip(t.entry, &new_index, end);
    }
    for note in &mut chunk.opt_notes {
        note.at = map_ip(note.at, &new_index, end);
    }
    // Escape-analysis birth tags must track Make* ops across Nop compaction.
    for (pc, _) in &mut chunk.escape_birth {
        *pc = map_ip(*pc, &new_index, end);
    }

    chunk.ops = out;
    stats
}

#[inline]
fn map_ip(ip: u32, new_index: &[u32], end: u32) -> u32 {
    let i = ip as usize;
    if i < new_index.len() {
        new_index[i]
    } else {
        end
    }
}

fn remap_op_jumps(op: &mut Op, new_index: &[u32], end: u32) {
    let map = |t: u32| map_ip(t, new_index, end);
    match op {
        Op::Jump(t)
        | Op::JumpIfFalse(t)
        | Op::JumpIfTrue(t)
        | Op::JumpIfLt(t)
        | Op::JumpIfGe(t)
        | Op::JumpIfEq(t)
        | Op::JumpIfNe(t)
        | Op::IterNext(t)
        | Op::ParEachBegin(t) => *t = map(*t),
        Op::LoopStep { body, .. } => *body = map(*body),
        _ => {}
    }
}

fn bump_op_jumps(op: &mut Op, at: usize, delta: i32) {
    let bump = |t: &mut u32| {
        if (*t as usize) >= at {
            let v = *t as i32 + delta;
            *t = v.max(0) as u32;
        }
    };
    match op {
        Op::Jump(t)
        | Op::JumpIfFalse(t)
        | Op::JumpIfTrue(t)
        | Op::JumpIfLt(t)
        | Op::JumpIfGe(t)
        | Op::JumpIfEq(t)
        | Op::JumpIfNe(t)
        | Op::IterNext(t)
        | Op::ParEachBegin(t) => bump(t),
        Op::LoopStep { body, .. } => bump(body),
        _ => {}
    }
}

fn bump_chunk_ips(chunk: &mut Chunk, at: usize, delta: i32) {
    for op in &mut chunk.ops {
        bump_op_jumps(op, at, delta);
    }
    for f in &mut chunk.functions {
        if (f.entry as usize) >= at {
            f.entry = ((f.entry as i32) + delta).max(0) as u32;
        }
    }
    for t in &mut chunk.tests {
        if (t.entry as usize) >= at {
            t.entry = ((t.entry as i32) + delta).max(0) as u32;
        }
    }
    for note in &mut chunk.opt_notes {
        if (note.at as usize) >= at {
            note.at = ((note.at as i32) + delta).max(0) as u32;
        }
    }
    for (pc, _) in &mut chunk.escape_birth {
        if (*pc as usize) >= at {
            *pc = ((*pc as i32) + delta).max(0) as u32;
        }
    }
}

// ─── Bytecode LICM + BCE ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct LoopRegion {
    /// Index of `IterNext` / `LoopStep` header.
    header: usize,
    /// First op of the body (after it/index stores).
    body_start: usize,
    /// Exclusive end (the back-edge `Jump`).
    body_end: usize,
    /// Exit target of `IterNext` / fallthrough after `LoopStep`.
    exit: usize,
    /// `it` local from `PushIt; StoreLocal` (for range-`for`, equals the index).
    it_slot: Option<u16>,
    index_slot: Option<u16>,
    /// Locals written anywhere in `[header, body_end]` (contents or rebinding).
    written: HashSet<u16>,
    /// Locals rebound to a *different* value (not `IndexSet`/`StoreSliceUnchecked`
    /// write-backs). Indexed SoA mutation must not poison BCE.
    rebound: HashSet<u16>,
    /// `it` / `index` and temps that only copy them (multi-assign spills).
    index_aliases: HashSet<u16>,
}

/// True when `outer`'s body lexically contains `inner`'s header/body.
fn loop_contains(outer: &LoopRegion, inner: &LoopRegion) -> bool {
    outer.body_start <= inner.header && inner.body_end <= outer.body_end
}

/// Nested `for` bodies often read `arr[i]` where `i` is an *enclosing* induction
/// variable. Union those outer aliases into the inner region (skipping slots the
/// inner loop rebinds) so BCE elides `sup_ld_*` / `sup_st_*` on them too.
fn propagate_enclosing_index_aliases(loops: &mut [LoopRegion]) {
    let n = loops.len();
    for i in 0..n {
        let mut extra: HashSet<u16> = HashSet::new();
        for j in 0..n {
            if i == j || !loop_contains(&loops[j], &loops[i]) {
                continue;
            }
            for &s in &loops[j].index_aliases {
                // If the inner body stores into `s`, it is no longer the outer IV.
                if !loops[i].written.contains(&s) {
                    extra.insert(s);
                }
            }
        }
        loops[i].index_aliases.extend(extra);
    }
}

#[inline(never)]
fn licm_and_bce(chunk: &mut Chunk) -> OptStats {
    let mut stats = OptStats::default();
    // BCE first while all loop indices are still stable.
    let mut loops = find_loop_regions(&chunk.ops);
    propagate_enclosing_index_aliases(&mut loops);
    for region in loops.into_iter().rev() {
        stats.bce_elided += bce_in_loop(chunk, &region);
    }
    // LICM inserts into the pre-header and invalidates subsequent region
    // indices — re-discover loops after every successful hoist.
    for _ in 0..256 {
        let loops = find_loop_regions(&chunk.ops);
        let mut progressed = false;
        for (idx, region) in loops.iter().enumerate().rev() {
            let variant = enclosing_variant_slots(&loops, idx);
            let n = licm_in_loop(chunk, region, &variant);
            if n > 0 {
                stats.licm_hoists += n;
                progressed = true;
                break;
            }
        }
        if !progressed {
            break;
        }
    }
    stats
}

/// Locals that vary for `loops[idx]`, including `it`/`index`/stores from every
/// lexically enclosing loop (nested `for`).
fn enclosing_variant_slots(loops: &[LoopRegion], idx: usize) -> HashSet<u16> {
    let cur = &loops[idx];
    let mut v = cur.written.clone();
    if let Some(s) = cur.it_slot {
        v.insert(s);
    }
    if let Some(s) = cur.index_slot {
        v.insert(s);
    }
    for (j, outer) in loops.iter().enumerate() {
        if j == idx {
            continue;
        }
        // Outer contains this loop when its body spans our header.
        if outer.body_start <= cur.header && outer.body_end >= cur.header {
            v.extend(&outer.written);
            if let Some(s) = outer.it_slot {
                v.insert(s);
            }
            if let Some(s) = outer.index_slot {
                v.insert(s);
            }
        }
    }
    v
}

fn find_loop_regions(ops: &[Op]) -> Vec<LoopRegion> {
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::IterNext(exit) => {
                let exit = *exit as usize;
                // Find back-edge Jump → i inside (i, exit).
                let mut back = None;
                for j in (i + 1)..exit.min(ops.len()) {
                    if let Op::Jump(t) = ops[j]
                        && t as usize == i
                    {
                        back = Some(j);
                        break;
                    }
                }
                let Some(body_end) = back else { continue };
                let (it_slot, index_slot, body_start) = parse_each_header(ops, i);
                let (written, rebound) = collect_written_and_rebound(ops, i, body_end);
                let mut region = LoopRegion {
                    header: i,
                    body_start,
                    body_end,
                    exit,
                    it_slot,
                    index_slot,
                    written,
                    rebound,
                    index_aliases: HashSet::new(),
                };
                region.index_aliases = collect_index_aliases(ops, &region);
                out.push(region);
            }
            Op::LoopStep {
                counter,
                limit,
                body,
            } => {
                let body = *body as usize;
                // LoopStep jumps to `body` while `counter < limit`, else
                // falls through. Prefer the canonical layout where the body
                // precedes the header (`body < header`); otherwise scan for
                // the back-edge Jump → header.
                let (body_start, body_end, exit) = if body < i {
                    (body, i, i + 1)
                } else {
                    let mut end = ops.len();
                    for j in (i + 1)..ops.len() {
                        if let Op::Jump(t) = ops[j]
                            && t as usize == i
                        {
                            end = j;
                            break;
                        }
                    }
                    let exit = (end + 1).min(ops.len());
                    (body.max(i + 1), end, exit)
                };
                let (mut written, mut rebound) =
                    collect_written_and_rebound(ops, body_start, body_end);
                written.insert(*counter);
                written.insert(*limit);
                rebound.insert(*counter);
                rebound.insert(*limit);
                let mut region = LoopRegion {
                    header: i,
                    body_start,
                    body_end,
                    exit,
                    it_slot: Some(*counter),
                    index_slot: Some(*counter),
                    written,
                    rebound,
                    index_aliases: HashSet::new(),
                };
                region.index_aliases = collect_index_aliases(ops, &region);
                out.push(region);
            }
            _ => {}
        }
    }
    out
}

fn parse_each_header(ops: &[Op], iter_next: usize) -> (Option<u16>, Option<u16>, usize) {
    // Expected: IterNext; PushIt; StoreLocal(it); PushIndex; StoreLocal(index)
    let mut i = iter_next + 1;
    let mut it_slot = None;
    let mut index_slot = None;
    if matches!(ops.get(i), Some(Op::PushIt)) {
        i += 1;
        if let Some(Op::StoreLocal(s)) = ops.get(i) {
            it_slot = Some(*s);
            i += 1;
        }
    }
    if matches!(ops.get(i), Some(Op::PushIndex)) {
        i += 1;
        if let Some(Op::StoreLocal(s)) = ops.get(i) {
            index_slot = Some(*s);
            i += 1;
        }
    }
    // Optional destructure bindings: LoadLocal(it); StoreLocal(b)*
    while i + 1 < ops.len() {
        match (&ops[i], &ops[i + 1]) {
            (Op::LoadLocal(_), Op::StoreLocal(_)) => i += 2,
            _ => break,
        }
    }
    (it_slot, index_slot, i)
}

fn is_each_index(region: &LoopRegion, slot: u16) -> bool {
    region.index_aliases.contains(&slot)
}

/// Induction-variable aliases: `it`/`index` plus locals that are only ever
/// copied from those (e.g. `i = it` before `arr[i] = …`). Needed because
/// multi-assign lowering spills `it` into temps before `IndexSet`.
fn collect_index_aliases(ops: &[Op], region: &LoopRegion) -> HashSet<u16> {
    let mut aliases = HashSet::new();
    if let Some(s) = region.it_slot {
        aliases.insert(s);
    }
    if let Some(s) = region.index_slot {
        aliases.insert(s);
    }
    let start = region.body_start.min(ops.len());
    let end = region.body_end.min(ops.len());
    // Sources seen for each StoreLocal destination inside the loop body.
    let mut sources: HashMap<u16, HashSet<Option<u16>>> = HashMap::new();
    let mut i = start;
    while i + 1 < end {
        match (&ops[i], &ops[i + 1]) {
            (Op::LoadLocal(src), Op::StoreLocal(dst)) => {
                sources.entry(*dst).or_default().insert(Some(*src));
                i += 2;
            }
            (_, Op::StoreLocal(dst)) => {
                sources.entry(*dst).or_default().insert(None);
                i += 1;
            }
            _ => i += 1,
        }
    }
    // Fixpoint: a slot is an alias iff every store into it comes from an alias.
    loop {
        let mut grew = false;
        for (dst, srcs) in &sources {
            if aliases.contains(dst) {
                continue;
            }
            if srcs.is_empty() {
                continue;
            }
            if srcs
                .iter()
                .all(|s| s.is_some_and(|src| aliases.contains(&src)))
            {
                aliases.insert(*dst);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    aliases
}

fn collect_written(ops: &[Op], start: usize, end: usize) -> HashSet<u16> {
    collect_written_and_rebound(ops, start, end).0
}

/// Split loop stores into *any write* vs *rebinding*.
///
/// `arr[i] = v` lowers as `IndexSet` / `StoreSliceUnchecked` then `StoreLocal(arr)`.
/// That write-back keeps the same buffer (length unchanged) and must not block
/// bounds-check elimination on induction-variable indexes. True rebinds
/// (`arr = other`) still poison BCE.
fn collect_written_and_rebound(
    ops: &[Op],
    start: usize,
    end: usize,
) -> (HashSet<u16>, HashSet<u16>) {
    let mut written = HashSet::new();
    let mut rebound = HashSet::new();
    let mut i = start;
    while i < end {
        match &ops[i] {
            Op::StoreLocal(s) => {
                written.insert(*s);
                let from_index_mut =
                    i > start && matches!(ops[i - 1], Op::IndexSet | Op::StoreSliceUnchecked);
                if !from_index_mut {
                    rebound.insert(*s);
                }
            }
            // CallMut writebacks live in `chunk.mut_writebacks`; CallMut is already a
            // hoist barrier, so precise slot tracking here is unnecessary.
            Op::CallMut(_, _, _) => {}
            Op::Move(dst, _) => {
                written.insert(*dst);
                rebound.insert(*dst);
            }
            Op::LoopStep { counter, .. } => {
                written.insert(*counter);
                rebound.insert(*counter);
            }
            _ => {}
        }
        i += 1;
    }
    (written, rebound)
}

fn stack_delta(op: &Op) -> i32 {
    match op {
        Op::Nop => 0,
        Op::LoadConst(_)
        | Op::LoadLocal(_)
        | Op::LoadGlobal(_)
        | Op::PushIt
        | Op::PushIndex
        | Op::Dup
        | Op::IsAbsent => 1,
        Op::StoreLocal(_)
        | Op::StoreGlobal(_)
        | Op::Pop
        | Op::JumpIfFalse(_)
        | Op::JumpIfTrue(_)
        | Op::Print
        | Op::Fail
        | Op::Panic => -1,
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Rem
        | Op::Eq
        | Op::Ne
        | Op::Lt
        | Op::Le
        | Op::Gt
        | Op::Ge
        | Op::And
        | Op::OrLog
        | Op::IndexGet
        | Op::LoadSliceUnchecked
        | Op::AssertEq
        | Op::JumpIfLt(_)
        | Op::JumpIfGe(_)
        | Op::JumpIfEq(_)
        | Op::JumpIfNe(_) => -1,
        Op::AddImm(_)
        | Op::SubImm(_)
        | Op::MulImm(_)
        | Op::ShlImm(_)
        | Op::Neg
        | Op::Not
        | Op::GetField(_)
        | Op::LoadFieldOff(_) => 0,
        Op::Move(_, _) | Op::Jump(_) | Op::Return | Op::Halt => 0,
        _ => {
            // Conservative: treat unknown as barrier (non-hoistable).
            99
        }
    }
}

/// Minimum stack depth required before `op` may execute.
fn stack_arity(op: &Op) -> i32 {
    match op {
        Op::LoadConst(_)
        | Op::LoadLocal(_)
        | Op::LoadGlobal(_)
        | Op::PushIt
        | Op::PushIndex
        | Op::Move(_, _)
        | Op::Jump(_)
        | Op::Return
        | Op::Halt
        | Op::Nop
        | Op::LoopStep { .. }
        | Op::IterBegin
        | Op::IterNext(_) => 0,
        Op::Dup
        | Op::IsAbsent
        | Op::StoreLocal(_)
        | Op::StoreGlobal(_)
        | Op::Pop
        | Op::Print
        | Op::Fail
        | Op::Panic
        | Op::Neg
        | Op::Not
        | Op::AddImm(_)
        | Op::SubImm(_)
        | Op::MulImm(_)
        | Op::ShlImm(_)
        | Op::GetField(_)
        | Op::LoadFieldOff(_)
        | Op::JumpIfFalse(_)
        | Op::JumpIfTrue(_) => 1,
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Rem
        | Op::Eq
        | Op::Ne
        | Op::Lt
        | Op::Le
        | Op::Gt
        | Op::Ge
        | Op::And
        | Op::OrLog
        | Op::IndexGet
        | Op::LoadSliceUnchecked
        | Op::AssertEq
        | Op::JumpIfLt(_)
        | Op::JumpIfGe(_)
        | Op::JumpIfEq(_)
        | Op::JumpIfNe(_)
        | Op::IndexSet
        | Op::StoreSliceUnchecked
        | Op::SetField(_)
        | Op::StoreFieldOff(_) => 2,
        _ => 99,
    }
}

fn is_hoist_barrier(op: &Op) -> bool {
    matches!(
        op,
        Op::Call(_, _)
            | Op::CallMut(_, _, _)
            | Op::CallHost(_, _)
            | Op::CallValue(_)
            | Op::MakeFn(_)
            | Op::MakeClosure(_, _)
            | Op::Jump(_)
            | Op::JumpIfFalse(_)
            | Op::JumpIfTrue(_)
            | Op::JumpIfLt(_)
            | Op::JumpIfGe(_)
            | Op::JumpIfEq(_)
            | Op::JumpIfNe(_)
            | Op::IterBegin
            | Op::IterNext(_)
            | Op::LoopStep { .. }
            | Op::ParEachBegin(_)
            | Op::Return
            | Op::Halt
            | Op::Fail
            | Op::Panic
            | Op::Spawn(_)
            | Op::Await
            | Op::ScopeEnter
            | Op::ScopeExit
            | Op::IndexSet
            | Op::StoreSliceUnchecked
            | Op::SetField(_)
            | Op::StoreFieldOff(_)
            | Op::PushIt
            | Op::PushIndex
            | Op::IterSetIt
    ) || stack_delta(op) == 99
}

fn is_hoist_producer(op: &Op) -> bool {
    matches!(op, Op::LoadConst(_) | Op::LoadLocal(_) | Op::LoadGlobal(_))
}

fn slot_written_before(ops: &[Op], header: usize, slot: u16) -> bool {
    ops.iter().take(header).any(|op| match op {
        Op::StoreLocal(s) => *s == slot,
        Op::Move(dst, _) => *dst == slot,
        // CallMut may write arbitrary slots via mut_writebacks — treat conservatively.
        Op::CallMut(_, _, _) => true,
        _ => false,
    })
}

fn slot_used_after(ops: &[Op], exit: usize, slot: u16) -> bool {
    ops.iter().skip(exit).any(|op| match op {
        Op::LoadLocal(s) | Op::StoreLocal(s) => *s == slot,
        Op::Move(a, b) => *a == slot || *b == slot,
        Op::LoopStep { counter, limit, .. } => *counter == slot || *limit == slot,
        Op::CallMut(_, _, _) => true,
        _ => false,
    })
}

/// Ops safe to appear inside a hoisted window when the loop body has no
/// structural mutations (`SetField` / `IndexSet` / calls). Field loads of an
/// invariant base yield a stable pointer/ref even if element contents change.
fn is_hoist_pure_op(op: &Op, allow_fields: bool) -> bool {
    if matches!(
        op,
        Op::LoadConst(_)
            | Op::LoadLocal(_)
            | Op::LoadGlobal(_)
            | Op::StoreLocal(_)
            | Op::Move(_, _)
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::Neg
            | Op::Not
            | Op::Eq
            | Op::Ne
            | Op::Lt
            | Op::Le
            | Op::Gt
            | Op::Ge
            | Op::And
            | Op::OrLog
            | Op::AddImm(_)
            | Op::SubImm(_)
            | Op::MulImm(_)
            | Op::ShlImm(_)
            | Op::Nop
    ) {
        return true;
    }
    allow_fields && matches!(op, Op::GetField(_) | Op::LoadFieldOff(_))
}

fn loop_has_structural_mutation(ops: &[Op], start: usize, end: usize) -> bool {
    ops.iter().take(end).skip(start).any(|op| {
        matches!(
            op,
            Op::SetField(_)
                | Op::StoreFieldOff(_)
                | Op::IndexSet
                | Op::StoreSliceUnchecked
                | Op::Call(_, _)
                | Op::CallMut(_, _, _)
                | Op::CallHost(_, _)
        )
    })
}

#[inline(never)]
fn licm_in_loop(chunk: &mut Chunk, region: &LoopRegion, variant: &HashSet<u16>) -> u32 {
    let allow_fields =
        !loop_has_structural_mutation(&chunk.ops, region.body_start, region.body_end);
    // Scan body for *self-contained* stack-balanced windows ending in StoreLocal.
    // Critical: at depth 0 only producers (Load*) may start/continue a window —
    // `AddImm`/`Dup` are stack-neutral but require an existing TOS and must never
    // be hoisted as a window prefix (that caused StackUnderflow).
    let mut i = region.body_start;
    while i < region.body_end && i < chunk.ops.len() {
        if matches!(chunk.ops[i], Op::Nop) {
            i += 1;
            continue;
        }
        if !is_hoist_producer(&chunk.ops[i]) {
            i += 1;
            continue;
        }
        // Try to grow a hoistable window starting at i.
        let mut j = i;
        let mut depth = 0i32;
        let mut ok = true;
        let mut last_store: Option<u16> = None;
        while j < region.body_end && j < chunk.ops.len() {
            let op = &chunk.ops[j];
            if matches!(op, Op::Nop) {
                j += 1;
                continue;
            }
            let pure_len = || {
                chunk.ops[i..j]
                    .iter()
                    .filter(|o| !matches!(o, Op::Nop))
                    .count()
            };
            let close_expr =
                |depth: i32, j: usize, i: usize, len: usize| depth == 1 && j > i && len >= 2;

            if is_hoist_barrier(op) || !is_hoist_pure_op(op, allow_fields) {
                if close_expr(depth, j, i, pure_len()) {
                    break;
                }
                ok = false;
                break;
            }
            // Refuse ops that need more stack than we have — the window would
            // be borrowing values produced before `i` (not self-contained).
            if depth < stack_arity(op) {
                if close_expr(depth, j, i, pure_len()) {
                    break;
                }
                ok = false;
                break;
            }
            // At empty stack only producers may appear.
            if depth == 0 && !is_hoist_producer(op) {
                ok = false;
                break;
            }
            // Variant local (this loop or an enclosing one): close a finished
            // invariant expr, else abort.
            if let Op::LoadLocal(s) = op
                && variant.contains(s)
            {
                if close_expr(depth, j, i, pure_len()) {
                    break;
                }
                ok = false;
                break;
            }
            if let Op::Move(dst, src) = op {
                if variant.contains(src) {
                    if close_expr(depth, j, i, pure_len()) {
                        let _ = dst;
                        break;
                    }
                    ok = false;
                    break;
                }
                let _ = dst;
            }
            depth += stack_delta(op);
            if let Op::StoreLocal(s) = op {
                last_store = Some(*s);
                if depth == 0 {
                    j += 1;
                    break;
                }
            }
            if depth < 0 {
                ok = false;
                break;
            }
            j += 1;
            // Cap window size.
            if j - i > 16 {
                ok = false;
                break;
            }
        }
        // Only hoist invariant products/quotients (e.g. `rate * dt`). Require
        // a Mul/Div/ShlImm so cheap add-imm increments never move.
        let window_has_mul = chunk.ops[i..j]
            .iter()
            .any(|o| matches!(o, Op::Mul | Op::Div | Op::MulImm(_) | Op::ShlImm(_)));

        // Store-based LICM: `tmp = rate * dt` where tmp is loop-local.
        if let Some(dst) = last_store
            && ok
            && depth == 0
            && j > i
            && window_has_mul
        {
            let other_writes = chunk.ops[region.body_start..region.body_end]
                .iter()
                .enumerate()
                .filter(|(k, _)| {
                    let pc = region.body_start + *k;
                    pc < i || pc >= j
                })
                .any(|(_, op)| matches!(op, Op::StoreLocal(s) if *s == dst));
            let loop_lo = region.body_start.min(region.header);
            let live_across = slot_written_before(&chunk.ops, loop_lo, dst)
                || slot_used_after(&chunk.ops, region.exit, dst);
            if !other_writes && !live_across && hoist_window_to_preheader(chunk, region, i, j, None)
            {
                return 1;
            }
        }

        // Expression LICM: `acc + rate * dt` → spill `rate * dt` to a fresh temp.
        if ok && depth == 1 && last_store.is_none() && j > i + 1 && window_has_mul {
            let pure_len = chunk.ops[i..j]
                .iter()
                .filter(|o| !matches!(o, Op::Nop))
                .count();
            if pure_len >= 2
                && let Some(tmp) = peek_temp_local(chunk, region.header)
                && hoist_window_to_preheader(chunk, region, i, j, Some(tmp))
            {
                commit_temp_local(chunk, region.header, tmp);
                return 1;
            }
        }
        i += 1;
    }
    0
}

fn function_index_at(chunk: &Chunk, at: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (entry, fi)
    for (fi, f) in chunk.functions.iter().enumerate() {
        let entry = f.entry as usize;
        if entry <= at {
            match best {
                Some((e, _)) if e >= entry => {}
                _ => best = Some((entry, fi)),
            }
        }
    }
    // Prefer the function with the greatest entry <= at whose range still
    // contains `at` (next function entry is the exclusive end).
    let (_, fi) = best?;
    let entry = chunk.functions[fi].entry as usize;
    let end = chunk
        .functions
        .iter()
        .map(|f| f.entry as usize)
        .filter(|&e| e > entry)
        .min()
        .unwrap_or(chunk.ops.len());
    if at < end { Some(fi) } else { None }
}

fn next_temp_local_slot(chunk: &Chunk, fi: usize) -> u16 {
    let entry = chunk.functions[fi].entry as usize;
    let end = chunk
        .functions
        .iter()
        .map(|f| f.entry as usize)
        .filter(|&e| e > entry)
        .min()
        .unwrap_or(chunk.ops.len());
    let mut next = chunk.functions[fi].local_count;
    // Params occupy the low slots even when the body never mentions them.
    next = next.max(chunk.functions[fi].arity as u16);
    for op in chunk.ops.iter().take(end).skip(entry) {
        let bump = |s: u16, next: &mut u16| {
            if s.saturating_add(1) > *next {
                *next = s.saturating_add(1);
            }
        };
        match op {
            Op::LoadLocal(s) | Op::StoreLocal(s) => bump(*s, &mut next),
            Op::CallMut(_, _, wb_id) => {
                if let Some(wbs) = chunk.mut_writebacks.get(*wb_id as usize) {
                    for (slot, _) in wbs {
                        bump(*slot, &mut next);
                    }
                }
            }
            Op::Move(a, b) => {
                bump(*a, &mut next);
                bump(*b, &mut next);
            }
            Op::LoopStep { counter, limit, .. } => {
                bump(*counter, &mut next);
                bump(*limit, &mut next);
            }
            _ => {}
        }
    }
    next
}

fn peek_temp_local(chunk: &Chunk, at: usize) -> Option<u16> {
    let fi = function_index_at(chunk, at)?;
    // Never alias an existing local/param — `local_count` alone can under-count.
    Some(next_temp_local_slot(chunk, fi))
}

fn commit_temp_local(chunk: &mut Chunk, at: usize, slot: u16) {
    if let Some(fi) = function_index_at(chunk, at) {
        let need = slot.saturating_add(1);
        if chunk.functions[fi].local_count < need {
            chunk.functions[fi].local_count = need;
        }
    }
}

/// Move `ops[i..j]` into the loop pre-header. When `spill` is `Some(tmp)`,
/// append `StoreLocal(tmp)` in the pre-header and leave a single
/// `LoadLocal(tmp)` at `i` (remaining body slots become `Nop`).
fn hoist_window_to_preheader(
    chunk: &mut Chunk,
    region: &LoopRegion,
    i: usize,
    j: usize,
    spill: Option<u16>,
) -> bool {
    let mut window: Vec<Op> = chunk.ops[i..j]
        .iter()
        .filter(|o| !matches!(o, Op::Nop))
        .cloned()
        .collect();
    if window.is_empty() {
        return false;
    }
    if let Some(tmp) = spill {
        window.push(Op::StoreLocal(tmp));
    }
    let wlen = window.len();
    for op in &mut chunk.ops[i..j] {
        *op = Op::Nop;
    }
    if let Some(tmp) = spill {
        chunk.ops[i] = Op::LoadLocal(tmp);
    }
    let at = region.header;
    bump_chunk_ips(chunk, at, wlen as i32);
    chunk.ops.splice(at..at, window);
    chunk.note(
        at as u32,
        OptNoteKind::LicmHoist,
        format!("hoisted {wlen} ops above loop header"),
    );
    true
}

fn bce_in_loop(chunk: &mut Chunk, region: &LoopRegion) -> u32 {
    if region.index_slot.is_none() && region.it_slot.is_none() {
        return 0;
    }
    let mut elided = 0u32;
    let start = region.body_start.min(chunk.ops.len());
    let end = region.body_end.min(chunk.ops.len());
    let mut i = start;
    while i + 2 < end {
        // LoadLocal(base); LoadLocal(idx); IndexGet
        let load_get = match (&chunk.ops[i], &chunk.ops[i + 1], &chunk.ops[i + 2]) {
            (Op::LoadLocal(base), Op::LoadLocal(idx), Op::IndexGet)
                if is_each_index(region, *idx) && !region.rebound.contains(base) =>
            {
                Some((*base, *idx))
            }
            _ => None,
        };
        if let Some((base, idx)) = load_get {
            chunk.ops[i + 2] = Op::LoadSliceUnchecked;
            chunk.note(
                (i + 2) as u32,
                OptNoteKind::BoundsCheckElim,
                format!("each index r{idx} on invariant base r{base}"),
            );
            elided += 1;
            i += 3;
            continue;
        }

        // LoadLocal(base); GetField; LoadLocal(idx); IndexGet
        if i + 3 < end {
            let field_get = match (
                &chunk.ops[i],
                &chunk.ops[i + 1],
                &chunk.ops[i + 2],
                &chunk.ops[i + 3],
            ) {
                (
                    Op::LoadLocal(base),
                    Op::GetField(_) | Op::LoadFieldOff(_),
                    Op::LoadLocal(idx),
                    Op::IndexGet,
                ) if is_each_index(region, *idx) && !region.rebound.contains(base) => {
                    Some((*base, *idx))
                }
                _ => None,
            };
            if let Some((base, idx)) = field_get {
                chunk.ops[i + 3] = Op::LoadSliceUnchecked;
                chunk.note(
                    (i + 3) as u32,
                    OptNoteKind::BoundsCheckElim,
                    format!("each index r{idx} via field of r{base}"),
                );
                elided += 1;
                i += 4;
                continue;
            }
        }

        // StoreSliceUnchecked: LoadLocal(base); LoadLocal(idx); <value>; IndexSet
        if i + 3 < end {
            let store = match (&chunk.ops[i], &chunk.ops[i + 1], &chunk.ops[i + 3]) {
                (Op::LoadLocal(base), Op::LoadLocal(idx), Op::IndexSet)
                    if is_each_index(region, *idx) && !region.rebound.contains(base) =>
                {
                    Some(*idx)
                }
                _ => None,
            };
            if let Some(idx) = store {
                chunk.ops[i + 3] = Op::StoreSliceUnchecked;
                chunk.note(
                    (i + 3) as u32,
                    OptNoteKind::BoundsCheckElim,
                    format!("store each index r{idx}"),
                );
                elided += 1;
                i += 4;
                continue;
            }
        }

        // LoadLocal(base); GetField; LoadLocal(idx); <value>; IndexSet
        if i + 4 < end {
            let field_set = match (
                &chunk.ops[i],
                &chunk.ops[i + 1],
                &chunk.ops[i + 2],
                &chunk.ops[i + 4],
            ) {
                (
                    Op::LoadLocal(base),
                    Op::GetField(_) | Op::LoadFieldOff(_),
                    Op::LoadLocal(idx),
                    Op::IndexSet,
                ) if is_each_index(region, *idx) && !region.rebound.contains(base) => Some(*idx),
                _ => None,
            };
            if let Some(idx) = field_set {
                chunk.ops[i + 4] = Op::StoreSliceUnchecked;
                chunk.note(
                    (i + 4) as u32,
                    OptNoteKind::BoundsCheckElim,
                    format!("store each index r{idx} via field"),
                );
                elided += 1;
                i += 5;
                continue;
            }
        }
        i += 1;
    }
    elided
}

fn compute_live_intervals(chunk: &mut Chunk) {
    let n = chunk.ops.len() as u32;
    for fi in 0..chunk.functions.len() {
        let entry = chunk.functions[fi].entry;
        let end = chunk.functions.get(fi + 1).map(|f| f.entry).unwrap_or(n);
        let local_count = chunk.functions[fi].local_count as usize;
        let mut intervals = vec![(entry, entry); local_count];
        for (pc, op) in chunk.ops.iter().enumerate() {
            let pc = pc as u32;
            if pc < entry || pc >= end {
                continue;
            }
            let touch = |intervals: &mut [(u32, u32)], slot: u16| {
                let s = slot as usize;
                if s < intervals.len() {
                    if intervals[s].0 == intervals[s].1 && intervals[s].0 == entry {
                        intervals[s] = (pc, pc + 1);
                    } else {
                        intervals[s].1 = pc + 1;
                    }
                }
            };
            match op {
                Op::LoadLocal(s) | Op::StoreLocal(s) => touch(&mut intervals, *s),
                Op::Move(a, b) => {
                    touch(&mut intervals, *a);
                    touch(&mut intervals, *b);
                }
                Op::LoopStep { counter, limit, .. } => {
                    touch(&mut intervals, *counter);
                    touch(&mut intervals, *limit);
                }
                Op::CallMut(_, _, wb_id) => {
                    if let Some(wbs) = chunk.mut_writebacks.get(*wb_id as usize) {
                        for (slot, _) in wbs {
                            touch(&mut intervals, *slot);
                        }
                    }
                }
                _ => {}
            }
        }
        chunk.functions[fi].live_intervals = intervals;
    }
}

// ─── Disassembly ────────────────────────────────────────────────────────────

/// Format a chunk as annotated bytecode for `super disasm`.
pub fn disassemble(chunk: &Chunk, interner: &Interner, typed: Option<&TypedModule>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "; Super bytecode — {} ops, {} consts, {} functions\n",
        chunk.ops.len(),
        chunk.consts.len(),
        chunk.functions.len()
    ));
    if !chunk.opt_notes.is_empty() {
        out.push_str(&format!("; optimizer notes: {}\n", chunk.opt_notes.len()));
    }
    for (fi, f) in chunk.functions.iter().enumerate() {
        let name = interner.resolve(f.name);
        let owner = f
            .owner
            .map(|o| interner.resolve(o).to_owned())
            .unwrap_or_default();
        let q = if owner.is_empty() {
            name.to_owned()
        } else {
            format!("{owner}.{name}")
        };
        out.push_str(&format!(
            "\nfn[{fi}] {q} arity={} locals={} entry=@{}\n",
            f.arity, f.local_count, f.entry
        ));
        for (slot, (lo, hi)) in f.live_intervals.iter().enumerate() {
            if lo != hi {
                out.push_str(&format!("  ; r{slot} live [{lo}, {hi})\n"));
            }
        }
        if let Some(t) = typed
            && let Some(owner) = f.owner
            && let Some(tid) = t.structs.get(&owner)
            && let TypeKind::Struct { fields, size, .. } = t.types.get_type(*tid)
        {
            out.push_str(&format!("  ; C-ABI sizeof={} bytes\n", size));
            for fl in fields {
                out.push_str(&format!(
                    "  ;   .{} @+{}\n",
                    interner.resolve(fl.name),
                    fl.offset
                ));
            }
        }
    }
    out.push('\n');
    for (pc, op) in chunk.ops.iter().enumerate() {
        for n in chunk.opt_notes.iter().filter(|n| n.at as usize == pc) {
            out.push_str(&format!("  ; {:?}: {}\n", n.kind, n.detail));
        }
        out.push_str(&format!("{pc:04}  {}\n", format_op(op, chunk, interner)));
    }
    out
}

fn format_op(op: &Op, chunk: &Chunk, interner: &Interner) -> String {
    match op {
        Op::LoadConst(i) => format!(
            "LoadConst[{i}] ; {:?}",
            chunk.consts.get(*i as usize).unwrap_or(&Value::Null)
        ),
        Op::LoadLocal(s) => format!("LoadLocal r{s}"),
        Op::StoreLocal(s) => format!("StoreLocal r{s}"),
        Op::Move(d, s) => format!("Move r{d}, r{s}"),
        Op::LoadGlobal(s) => format!("LoadGlobal {}", interner.resolve(*s)),
        Op::StoreGlobal(s) => format!("StoreGlobal {}", interner.resolve(*s)),
        Op::Add => "Add".into(),
        Op::Sub => "Sub".into(),
        Op::Mul => "Mul".into(),
        Op::Div => "Div".into(),
        Op::Rem => "Rem".into(),
        Op::AddImm(v) => format!("AddImm {v}"),
        Op::SubImm(v) => format!("SubImm {v}"),
        Op::MulImm(v) => format!("MulImm {v}"),
        Op::ShlImm(s) => format!("ShlImm {s}"),
        Op::Neg => "Neg".into(),
        Op::Not => "Not".into(),
        Op::Eq => "Eq".into(),
        Op::Ne => "Ne".into(),
        Op::Lt => "Lt".into(),
        Op::Le => "Le".into(),
        Op::Gt => "Gt".into(),
        Op::Ge => "Ge".into(),
        Op::And => "And".into(),
        Op::OrLog => "OrLog".into(),
        Op::BitAnd => "BitAnd".into(),
        Op::BitOr => "BitOr".into(),
        Op::BitXor => "BitXor".into(),
        Op::Shl => "Shl".into(),
        Op::Shr => "Shr".into(),
        Op::UShr => "UShr".into(),
        Op::BitNot => "BitNot".into(),
        Op::Jump(t) => format!("Jump @{t}"),
        Op::JumpIfFalse(t) => format!("JumpIfFalse @{t}"),
        Op::JumpIfTrue(t) => format!("JumpIfTrue @{t}"),
        Op::JumpIfLt(t) => format!("JumpIfLt @{t}"),
        Op::JumpIfGe(t) => format!("JumpIfGe @{t}"),
        Op::JumpIfEq(t) => format!("JumpIfEq @{t}"),
        Op::JumpIfNe(t) => format!("JumpIfNe @{t}"),
        Op::Call(i, a) => format!("Call fn[{i}] arity={a}"),
        Op::CallMut(i, a, wb) => format!("CallMut fn[{i}] arity={a} wb_id={wb}"),
        Op::CallHost(s, a) => format!("CallHost {} arity={a}", interner.resolve(*s)),
        Op::Return => "Return".into(),
        Op::Pop => "Pop".into(),
        Op::Dup => "Dup".into(),
        Op::GetField(f) => format!("GetField .{}", interner.resolve(*f)),
        Op::SetField(f) => format!("SetField .{}", interner.resolve(*f)),
        Op::LoadFieldOff(o) => format!("LoadFieldOff +{o}"),
        Op::StoreFieldOff(o) => format!("StoreFieldOff +{o}"),
        Op::IndexGet => "IndexGet".into(),
        Op::IndexSet => "IndexSet".into(),
        Op::LoadSliceUnchecked => "LoadSliceUnchecked".into(),
        Op::StoreSliceUnchecked => "StoreSliceUnchecked".into(),
        Op::LoopStep {
            counter,
            limit,
            body,
        } => format!("LoopStep r{counter} < r{limit} -> @{body}"),
        Op::IterBegin => "IterBegin".into(),
        Op::IterPop => "IterPop".into(),
        Op::IterNext(t) => format!("IterNext done=@{t}"),
        Op::MakeArray(n) => format!("MakeArray {n}"),
        Op::MakeMap(n) => format!("MakeMap {n}"),
        Op::MakeStruct(name, n) => format!(
            "MakeStruct {} fields={n}",
            name.map(|s| interner.resolve(s).to_owned())
                .unwrap_or_else(|| "_".into())
        ),
        Op::MakeSum {
            name,
            tag,
            n_fields,
        } => format!(
            "MakeSum {} tag={tag} fields={n_fields}",
            interner.resolve(*name)
        ),
        Op::IsVariant(tag) => format!("IsVariant tag={tag}"),
        Op::SumTag => "SumTag".into(),
        Op::MakeRange => "MakeRange".into(),
        Op::PushIt => "PushIt".into(),
        Op::PushIndex => "PushIndex".into(),
        Op::IterSetIt => "IterSetIt".into(),
        Op::OrElse => "OrElse".into(),
        Op::IsAbsent => "IsAbsent".into(),
        Op::Fail => "Fail".into(),
        Op::Panic => "Panic".into(),
        Op::AssertEq => "AssertEq".into(),
        Op::Print => "Print".into(),
        Op::Nop => "Nop".into(),
        Op::Halt => "Halt".into(),
        Op::ScopeEnter => "ScopeEnter".into(),
        Op::ScopeExit => "ScopeExit".into(),
        Op::Spawn(i) => format!("Spawn fn[{i}]"),
        Op::Await => "Await".into(),
        Op::ChanNew => "ChanNew".into(),
        Op::ChanSend => "ChanSend".into(),
        Op::ChanRecv => "ChanRecv".into(),
        Op::ChanClose => "ChanClose".into(),
        Op::ChanTryRecv => "ChanTryRecv".into(),
        Op::SharedNew => "SharedNew".into(),
        Op::SharedLoad => "SharedLoad".into(),
        Op::SharedAdd => "SharedAdd".into(),
        Op::MakeDuration(u) => format!("MakeDuration unit={u}"),
        Op::ParEachBegin(t) => format!("ParEachBegin @{t}"),
        Op::SelectWhen(n) => format!("SelectWhen arms={n}"),
        Op::MakeFn(i) => format!("MakeFn {i}"),
        Op::MakeClosure(i, n) => format!("MakeClosure {i} caps={n}"),
        Op::CallValue(a) => format!("CallValue arity={a}"),
    }
}

/// Hex + crude native disasm dump of a JIT buffer.
pub fn disassemble_native(bytes: &[u8]) -> String {
    let mut out = String::new();
    out.push_str(&format!("; native machine code — {} bytes\n", bytes.len()));
    for (i, chunk) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("{:04x}: ", i * 16));
        for b in chunk {
            out.push_str(&format!("{b:02x} "));
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push(' ');
        for b in chunk {
            let c = if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            };
            out.push(c);
        }
        out.push('\n');
    }
    #[cfg(target_arch = "x86_64")]
    {
        out.push_str("; x86_64 decode (heuristic):\n");
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                0xC3 => {
                    out.push_str(&format!("  {i:04x}: ret\n"));
                    i += 1;
                }
                0xB8 if i + 4 < bytes.len() => {
                    let imm = i32::from_le_bytes([
                        bytes[i + 1],
                        bytes[i + 2],
                        bytes[i + 3],
                        bytes[i + 4],
                    ]);
                    out.push_str(&format!("  {i:04x}: mov eax, {imm:#x}\n"));
                    i += 5;
                }
                0xF2 if i + 1 < bytes.len() && bytes[i + 1] == 0x0F => {
                    let op = bytes.get(i + 2).copied().unwrap_or(0);
                    let name = match op {
                        0x10 => "movsd",
                        0x11 => "movsd",
                        0x58 => "addsd",
                        0x59 => "mulsd",
                        0x5C => "subsd",
                        0x5E => "divsd",
                        _ => "sse2.sd",
                    };
                    out.push_str(&format!("  {i:04x}: {name}\n"));
                    i += 3;
                    // Skip ModRM (+ optional SIB) heuristically.
                    if i < bytes.len() {
                        i += 1;
                    }
                }
                0x66 if i + 2 < bytes.len() && bytes[i + 1] == 0x0F => {
                    let op = bytes[i + 2];
                    let name = match op {
                        0x10 => "movupd",
                        0x11 => "movupd",
                        0x14 => "unpcklpd",
                        0x58 => "addpd",
                        0x59 => "mulpd",
                        0x5C => "subpd",
                        0x5E => "divpd",
                        _ => "sse2.pd",
                    };
                    out.push_str(&format!("  {i:04x}: {name}\n"));
                    i += 3;
                    if i < bytes.len() {
                        i += 1; // ModRM
                    }
                }
                0x48 | 0x49 | 0x4C | 0x4D => {
                    // REX prefix + following opcode (heuristic 2–3 byte groups).
                    out.push_str(&format!("  {i:04x}: rex…\n"));
                    i += 2;
                }
                0x90 => {
                    out.push_str(&format!("  {i:04x}: nop\n"));
                    i += 1;
                }
                0x53 => {
                    out.push_str(&format!("  {i:04x}: push rbx\n"));
                    i += 1;
                }
                0x5B => {
                    out.push_str(&format!("  {i:04x}: pop rbx\n"));
                    i += 1;
                }
                b => {
                    out.push_str(&format!("  {i:04x}: db {b:#x}\n"));
                    i += 1;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_core::{Diagnostics, SourceFile};
    use super_frontend::{Lexer, Parser, SemanticAnalyzer};

    fn parse_opt(src: &str) -> (Module, OptMeta) {
        let source = SourceFile::new("t.sr", src);
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
        let mut module =
            Parser::new(&tokens, &source.text, &mut interner, &mut diags).parse_module();
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        let meta = optimize_module(&mut module, &typed, &mut interner);
        (module, meta)
    }

    #[test]
    fn folds_arithmetic_and_dead_branch() {
        let mut interner = Interner::new();
        let mut meta = OptMeta::default();
        let span = Span::default();
        let e = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Int(1, span)),
            right: Box::new(Expr::Binary {
                op: BinOp::Mul,
                left: Box::new(Expr::Int(2, span)),
                right: Box::new(Expr::Int(3, span)),
                span,
            }),
            span,
        };
        let folded = fold_expr(e, &mut interner, &mut meta);
        assert!(matches!(folded, Expr::Int(7, _)));
        assert!(meta.stats.folds >= 1);

        let dead = fold_expr(
            Expr::If {
                cond: Box::new(Expr::Bool(false, span)),
                then_block: Block {
                    stmts: vec![],
                    span,
                },
                else_block: Some(Block {
                    stmts: vec![Stmt::Expr(Expr::Int(1, span))],
                    span,
                }),
                span,
            },
            &mut interner,
            &mut meta,
        );
        assert!(meta.stats.dead_branches >= 1);
        let _ = dead;

        let (module, _) = parse_opt("main() { x = 1 }\n");
        let _ = module;
    }

    #[test]
    fn peephole_fuses_add_imm() {
        let mut chunk = Chunk::default();
        let c = chunk.add_const(Value::Int(5));
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadConst(c));
        chunk.push(Op::Add);
        chunk.push(Op::Halt);
        let stats = peephole(&mut chunk);
        assert!(stats.peephole_fused >= 1);
        assert!(chunk.ops.iter().any(|o| matches!(o, Op::AddImm(5))));
        assert!(
            !chunk.ops.iter().any(|o| matches!(o, Op::Nop)),
            "compaction must strip all Nops"
        );
    }

    #[test]
    fn peephole_never_fuses_into_a_merge_point() {
        // 10 + (if c { 1 } else { 0 }): the then-edge jumps straight to `Add`.
        let mut chunk = Chunk::default();
        let ten = chunk.add_const(Value::Int(10));
        let one = chunk.add_const(Value::Int(1));
        let zero = chunk.add_const(Value::Int(0));
        chunk.push(Op::LoadConst(ten)); // 0
        chunk.push(Op::LoadLocal(0)); // 1
        chunk.push(Op::JumpIfFalse(5)); // 2
        chunk.push(Op::LoadConst(one)); // 3
        chunk.push(Op::Jump(6)); // 4
        chunk.push(Op::LoadConst(zero)); // 5
        chunk.push(Op::Add); // 6 ← merge
        chunk.push(Op::Halt);
        peephole(&mut chunk);
        assert!(
            !chunk.ops.iter().any(|o| matches!(o, Op::AddImm(_))),
            "{:?}",
            chunk.ops
        );
        assert!(chunk.ops.iter().any(|o| matches!(o, Op::Add)));
    }

    #[test]
    fn peephole_never_fuses_compare_into_a_merge_point() {
        let mut chunk = Chunk::default();
        chunk.push(Op::LoadLocal(0)); // 0
        chunk.push(Op::JumpIfFalse(4)); // 1
        chunk.push(Op::LoadLocal(1)); // 2
        chunk.push(Op::LoadLocal(2)); // 3
        chunk.push(Op::Lt); // 4
        chunk.push(Op::JumpIfTrue(7)); // 5 ← merge
        chunk.push(Op::Jump(1)); // 6 (unused shape filler)
        chunk.push(Op::Halt); // 7
        chunk.ops[1] = Op::JumpIfFalse(5);
        peephole(&mut chunk);
        assert!(
            !chunk.ops.iter().any(|o| matches!(o, Op::JumpIfLt(_))),
            "{:?}",
            chunk.ops
        );
    }

    #[test]
    fn peephole_fuses_shl_imm() {
        let mut chunk = Chunk::default();
        let c = chunk.add_const(Value::Int(8));
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadConst(c));
        chunk.push(Op::Mul);
        chunk.push(Op::Halt);
        let stats = peephole(&mut chunk);
        assert!(stats.peephole_fused >= 1);
        assert!(chunk.ops.iter().any(|o| matches!(o, Op::ShlImm(3))));
        assert!(!chunk.ops.iter().any(|o| matches!(o, Op::Nop)));
    }

    #[test]
    fn jump_if_lt_opcode_roundtrips() {
        let mut chunk = Chunk::default();
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::JumpIfLt(4));
        chunk.push(Op::Halt);
        let _ = peephole(&mut chunk);
        assert!(chunk.ops.iter().any(|o| matches!(o, Op::JumpIfLt(4))));
        assert!(!chunk.ops.iter().any(|o| matches!(o, Op::Nop)));
    }

    #[test]
    fn strip_nops_relocates_jump_targets() {
        let mut chunk = Chunk::default();
        // 0: Nop, 1: Jump(3), 2: Nop, 3: Halt  → after strip: Jump(1), Halt
        chunk.push(Op::Nop);
        chunk.push(Op::Jump(3));
        chunk.push(Op::Nop);
        chunk.push(Op::Halt);
        let mut interner = Interner::default();
        let name = interner.intern("entry");
        chunk.functions.push(crate::bytecode::FunctionChunk {
            name,
            owner: None,
            arity: 0,
            local_count: 0,
            entry: 3,
            param_names: vec![],
            mut_params: vec![],
            always_inline: false,
            live_intervals: vec![],
            param_tags: vec![],
            local_tags: vec![],
            ret_tag: crate::bytecode::CTag::Dyn,
        });
        let _ = peephole(&mut chunk);
        assert_eq!(chunk.ops.len(), 2);
        assert!(matches!(chunk.ops[0], Op::Jump(1)));
        assert!(matches!(chunk.ops[1], Op::Halt));
        assert_eq!(chunk.functions[0].entry, 1);
    }

    #[test]
    fn ternary_self_assign_keeps_store() {
        // Else edge is `LoadLocal(x); StoreLocal(x)` and StoreLocal is a jump target —
        // peep must NOT delete it or the then-value is dropped.
        let mut chunk = Chunk::default();
        // 0: LoadConst true
        // 1: JumpIfFalse 4
        // 2: LoadLocal 1
        // 3: Jump 5
        // 4: LoadLocal 0
        // 5: StoreLocal 0
        // 6: Halt
        let c = chunk.add_const(Value::Bool(true));
        chunk.push(Op::LoadConst(c));
        chunk.push(Op::JumpIfFalse(4));
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::Jump(5));
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::StoreLocal(0));
        chunk.push(Op::Halt);
        let _ = peephole(&mut chunk);
        assert!(
            chunk.ops.iter().any(|o| matches!(o, Op::StoreLocal(0))),
            "StoreLocal(0) must survive peep: {:?}",
            chunk.ops
        );
        assert!(
            chunk.ops.iter().any(|o| matches!(o, Op::LoadLocal(1))),
            "then-branch LoadLocal(1) must survive: {:?}",
            chunk.ops
        );
    }

    #[test]
    fn licm_hoists_invariant_before_loop_step() {
        let mut chunk = Chunk::default();
        // Canonical layout: body precedes LoopStep header.
        //   body: LoadLocal(rate); LoadLocal(dt); Mul; StoreLocal(tmp);
        //   LoopStep { counter, limit, body: 0 }  // falls through to Halt
        // rate/dt invariant; tmp is loop-local (not live across the loop).
        chunk.push(Op::LoadLocal(2));
        chunk.push(Op::LoadLocal(3));
        chunk.push(Op::Mul);
        chunk.push(Op::StoreLocal(4));
        chunk.push(Op::LoopStep {
            counter: 0,
            limit: 1,
            body: 0,
        });
        chunk.push(Op::Halt);
        let stats = peephole(&mut chunk);
        assert!(stats.licm_hoists >= 1, "expected LICM hoist, got {stats:?}");
        let loop_at = chunk
            .ops
            .iter()
            .position(|o| matches!(o, Op::LoopStep { .. }))
            .expect("LoopStep");
        assert!(loop_at > 0, "hoisted ops should precede LoopStep");
        assert!(
            chunk.ops[..loop_at].iter().any(|o| matches!(o, Op::Mul)),
            "Mul should be in pre-header"
        );
        assert!(!chunk.ops.iter().any(|o| matches!(o, Op::Nop)));
    }

    #[test]
    fn bce_elides_soa_mutation_despite_index_set_writeback() {
        // SoA loop: load arr[i], store arr[i] = v. IndexSet; StoreLocal(arr) must
        // not poison BCE — length is unchanged.
        let mut chunk = Chunk::default();
        // body @0:
        //   LoadLocal(0); LoadLocal(1); IndexGet; Pop
        //   LoadLocal(0); LoadLocal(1); LoadConst; IndexSet; StoreLocal(0)
        // LoopStep { counter:1, limit:2, body:0 }
        let c = chunk.add_const(Value::Int(1));
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::IndexGet);
        chunk.push(Op::Pop);
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::LoadConst(c));
        chunk.push(Op::IndexSet);
        chunk.push(Op::StoreLocal(0));
        chunk.push(Op::LoopStep {
            counter: 1,
            limit: 2,
            body: 0,
        });
        chunk.push(Op::Halt);
        let stats = peephole(&mut chunk);
        assert!(
            stats.bce_elided >= 2,
            "expected load+store BCE on SoA mutation, got {stats:?} ops={:?}",
            chunk.ops
        );
        assert!(
            chunk
                .ops
                .iter()
                .any(|o| matches!(o, Op::LoadSliceUnchecked)),
            "IndexGet should become LoadSliceUnchecked: {:?}",
            chunk.ops
        );
        assert!(
            chunk
                .ops
                .iter()
                .any(|o| matches!(o, Op::StoreSliceUnchecked)),
            "IndexSet should become StoreSliceUnchecked: {:?}",
            chunk.ops
        );
    }

    #[test]
    fn bce_elides_outer_induction_index_in_nested_loop() {
        // Outer LoopStep counter=1; inner captures `i` into slot 3 then reads arr[i]
        // while iterating j (counter=2). Slot 3 must BCE inside the inner body.
        // Layout (canonical body-before-header):
        //   // outer body @0
        //   LoadLocal(1); StoreLocal(3);          // i = outer counter
        //     // inner body @2
        //     LoadLocal(0); LoadLocal(3); IndexGet; Pop   // arr[i]
        //     LoadLocal(0); LoadLocal(2); IndexGet; Pop   // arr[j]
        //   LoopStep { counter:2, limit:4, body:2 }       // inner header
        //   LoopStep { counter:1, limit:5, body:0 }       // outer header
        let mut chunk = Chunk::default();
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::StoreLocal(3));
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadLocal(3));
        chunk.push(Op::IndexGet);
        chunk.push(Op::Pop);
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadLocal(2));
        chunk.push(Op::IndexGet);
        chunk.push(Op::Pop);
        chunk.push(Op::LoopStep {
            counter: 2,
            limit: 4,
            body: 2,
        });
        chunk.push(Op::LoopStep {
            counter: 1,
            limit: 5,
            body: 0,
        });
        chunk.push(Op::Halt);
        let stats = peephole(&mut chunk);
        let unchecked = chunk
            .ops
            .iter()
            .filter(|o| matches!(o, Op::LoadSliceUnchecked))
            .count();
        assert!(
            unchecked >= 2,
            "expected BCE on both arr[i] and arr[j], got {unchecked} unchecked, stats={stats:?}, ops={:?}",
            chunk.ops
        );
    }

    #[test]
    fn licm_expr_hoists_rate_times_dt() {
        // IterNext loop body: LoadLocal(acc); LoadLocal(rate); LoadLocal(dt); Mul; Add; StoreLocal(acc)
        let mut chunk = Chunk::default();
        chunk.push(Op::IterBegin); // 0
        let iter = chunk.push(Op::IterNext(0)); // 1 — patched below
        chunk.push(Op::PushIt); // 2
        chunk.push(Op::StoreLocal(3)); // 3
        chunk.push(Op::PushIndex); // 4
        chunk.push(Op::StoreLocal(4)); // 5
        chunk.push(Op::LoadLocal(2)); // 6 acc
        chunk.push(Op::LoadLocal(0)); // 7 rate
        chunk.push(Op::LoadLocal(1)); // 8 dt
        chunk.push(Op::Mul); // 9
        chunk.push(Op::Add); // 10
        chunk.push(Op::StoreLocal(2)); // 11
        chunk.push(Op::Jump(iter)); // 12
        let exit = chunk.push(Op::Halt); // 13
        chunk.patch_jump(iter, exit);
        // Need a function so expr LICM can allocate a spill temp.
        let mut interner = Interner::default();
        let name = interner.intern("f");
        chunk.functions.push(crate::bytecode::FunctionChunk {
            name,
            owner: None,
            arity: 0,
            local_count: 5,
            entry: 0,
            param_names: vec![],
            mut_params: vec![],
            always_inline: false,
            live_intervals: vec![],
            param_tags: vec![],
            local_tags: vec![],
            ret_tag: crate::bytecode::CTag::Dyn,
        });
        let stats = peephole(&mut chunk);
        assert!(stats.licm_hoists >= 1, "expected expr LICM, got {stats:?}");
        let iter_at = chunk
            .ops
            .iter()
            .position(|o| matches!(o, Op::IterNext(_)))
            .unwrap();
        assert!(
            chunk.ops[..iter_at].iter().any(|o| matches!(o, Op::Mul)),
            "Mul should sit in the pre-header before IterNext"
        );
        assert!(!chunk.ops.iter().any(|o| matches!(o, Op::Nop)));
    }
}

#[cfg(test)]
mod escape_route_tests {
    use super::*;
    use super_core::{Diagnostics, SourceFile};
    use super_frontend::{Lexer, Parser, SemanticAnalyzer};

    #[test]
    fn route_handler_lambda_records_are_sites() {
        let src = r#"
+g_escape = 0
App = {
    get(@self, path: str, h) : App { self }
}
main() {
    app = App{}
    app = @app.get("/h", |_req| {
        a = {ok: true, service: "x"}
        a
    })
    app = @app.post("/a", |req| {
        items = [{id: 1}, {id: 2}]
        payload = {ok: true, items}
        escaped = {note: "e"}
        g_escape = 1
        payload
    })
}
"#;
        // Minimal parse without full http — use get/post Pipeline shape only.
        let source = SourceFile::new("t.sr", src);
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
        let mut module =
            Parser::new(&tokens, &source.text, &mut interner, &mut diags).parse_module();
        assert!(!diags.has_errors(), "{:?}", diags);
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        let meta = optimize_module(&mut module, &typed, &mut interner);
        assert!(
            meta.escape_summary.total() >= 3,
            "expected route sites, got {:?}",
            meta.escape_summary
        );
        assert!(meta.stats.escape_stay >= 1, "{:?}", meta.stats);
    }
}