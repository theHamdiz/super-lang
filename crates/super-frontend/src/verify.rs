//! Law & Proof formal verification (`#law`, `#proof`, `super verify`).
//!
//! Three tiers:
//! 1. Symbolic / SMT-style bitvector discharge (`i64` wraparound = `-fwrapv`)
//! 2. Comptime & equational rewrite discharge
//! 3. Bounded exhaustive / deterministic property fuzzing
//!
//! `#law` / `#proof` items are verify-only and must never reach C23 / JIT / bytecode.

use crate::ast::{BinOp, Block, Expr, FnBody, Function, Item, Module, Stmt, TypeExpr, UnOp};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use super_core::{
    DiagnosticKind, Diagnostics, Interner, Span, SymId,
    err::{E0502, E0503},
};

pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverKind {
    Auto,
    Smt,
    Comptime,
    Fuzz,
}

impl SolverKind {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "smt" | "bitvector" | "bv" => Ok(Self::Smt),
            "comptime" | "rewrite" | "eq" => Ok(Self::Comptime),
            "fuzz" | "property" | "pbt" => Ok(Self::Fuzz),
            other => Err(format!(
                "unknown solver `{other}` (expected auto, smt, comptime, fuzz)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Smt => "smt",
            Self::Comptime => "comptime",
            Self::Fuzz => "fuzz",
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    pub locked_laws: bool,
    pub solver: SolverKind,
    pub timeout_ms: u64,
    pub pod_root: Option<PathBuf>,
    pub law_lock: BTreeMap<String, String>,
    /// When set, discharge structural LAW-BENCH-001 over this `benchmarks/` tree.
    pub bench_root: Option<PathBuf>,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            locked_laws: false,
            solver: SolverKind::Auto,
            timeout_ms: DEFAULT_VERIFY_TIMEOUT_MS,
            pod_root: None,
            law_lock: BTreeMap::new(),
            bench_root: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DischargeTier {
    Smt,
    Comptime,
    Fuzz,
    /// Filesystem / suite-integrity laws (e.g. LAW-BENCH-001) discharged by the
    /// verify engine itself — not by Super `#assert` bodies.
    Structural,
}

impl DischargeTier {
    pub fn name(self) -> &'static str {
        match self {
            Self::Smt => "smt",
            Self::Comptime => "comptime",
            Self::Fuzz => "fuzz",
            Self::Structural => "structural",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Counterexample {
    pub bindings: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct LawResult {
    pub name: String,
    pub description: String,
    pub span: Span,
    pub passed: bool,
    pub tier: Option<DischargeTier>,
    pub message: String,
    pub counterexample: Option<Counterexample>,
    pub code: Option<&'static str>,
    pub body_hash: String,
}

#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub results: Vec<LawResult>,
    pub proofs_seen: usize,
    pub laws_seen: usize,
    pub new_lock: BTreeMap<String, String>,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        self.results.iter().all(|r| r.passed)
    }

    pub fn failed_count(&self) -> usize {
        self.results.iter().filter(|r| !r.passed).count()
    }

    pub fn to_json(&self) -> String {
        let mut laws = Vec::new();
        for r in &self.results {
            let mut obj = String::new();
            obj.push_str(&format!(
                "\"name\":{},\"description\":{},\"passed\":{},\"message\":{},\"body_hash\":{}",
                json_str(&r.name),
                json_str(&r.description),
                r.passed,
                json_str(&r.message),
                json_str(&r.body_hash),
            ));
            if let Some(tier) = r.tier {
                obj.push_str(&format!(",\"tier\":{}", json_str(tier.name())));
            }
            if let Some(code) = r.code {
                obj.push_str(&format!(",\"code\":{}", json_str(code)));
            }
            if let Some(cex) = &r.counterexample {
                let mut binds = String::from("{");
                for (i, (k, v)) in cex.bindings.iter().enumerate() {
                    if i > 0 {
                        binds.push(',');
                    }
                    binds.push_str(&format!("{}:{}", json_str(k), json_str(v)));
                }
                binds.push('}');
                obj.push_str(&format!(",\"counterexample\":{binds}"));
            }
            laws.push(format!("{{{obj}}}"));
        }
        format!(
            "{{\"passed\":{},\"laws_seen\":{},\"proofs_seen\":{},\"failed\":{},\"laws\":[{}]}}",
            self.passed(),
            self.laws_seen,
            self.proofs_seen,
            self.failed_count(),
            laws.join(",")
        )
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn collect_verify_items<'a>(
    module: &'a Module,
    _interner: &Interner,
) -> (Vec<&'a Function>, Vec<&'a Function>) {
    let mut laws = Vec::new();
    let mut proofs = Vec::new();
    for item in &module.items {
        let Item::Function(f) = item else { continue };
        if f.is_law() {
            laws.push(f);
        } else if f.is_proof() {
            proofs.push(f);
        }
    }
    (laws, proofs)
}

/// True when `path` is a Bend-style verify companion (`*.law.sr` / `*.proof.sr`).
pub fn is_verify_sidecar(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.ends_with(".law.sr") || name.ends_with(".proof.sr")
}

/// Resolve the primary module that a law/proof sidecar should be analyzed with.
///
/// Order: sibling `stem.sr` (`ledger.law.sr` → `ledger.sr`) → Pod entry
/// (`src/main.sr` / `src/lib.sr` / …) → none (analyze sidecar alone).
pub fn resolve_sidecar_primary(sidecar: &Path) -> Option<PathBuf> {
    if !is_verify_sidecar(sidecar) {
        return None;
    }
    let name = sidecar.file_name()?.to_str()?;
    let base_stem = name
        .strip_suffix(".law.sr")
        .or_else(|| name.strip_suffix(".proof.sr"))?;
    let parent = sidecar.parent()?;
    let sibling = parent.join(format!("{base_stem}.sr"));
    if sibling.is_file() {
        return Some(sibling);
    }
    // Walk up for Pod.toml and use the package entry.
    let mut cur = parent.to_path_buf();
    loop {
        if cur.join("Pod.toml").is_file() {
            for rel in [
                "src/main.sr",
                "src/lib.sr",
                "src/macros.sr",
                "main.sr",
                "lib.sr",
            ] {
                let p = cur.join(rel);
                if p.is_file() {
                    return Some(p);
                }
            }
            break;
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

pub fn discover_law_proof_files(module_path: &Path, pod_root: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(stem) = module_path.file_stem().and_then(|s| s.to_str()) {
        let base = stem.strip_suffix(".law").unwrap_or(stem);
        let base = base.strip_suffix(".proof").unwrap_or(base);
        if let Some(parent) = module_path.parent() {
            for suffix in [".law.sr", ".proof.sr"] {
                let p = parent.join(format!("{base}{suffix}"));
                if p.is_file() && p != module_path {
                    out.push(p);
                }
            }
        }
    }
    if let Some(root) = pod_root {
        for dir in ["laws", "src"] {
            let dir_path = root.join(dir);
            if !dir_path.is_dir() {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&dir_path) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if (name.ends_with(".law.sr") || name.ends_with(".proof.sr"))
                        && p.is_file()
                        && !out.contains(&p)
                    {
                        out.push(p);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn parse_laws_lock(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().trim_matches('"');
        let v = v.trim().trim_matches('"');
        if !k.is_empty() && !v.is_empty() {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

pub fn format_laws_lock(lock: &BTreeMap<String, String>) -> String {
    let mut out = String::from(
        "# Super laws.lock — content hashes of #law bodies\n# Generated by `super verify`.\n\n[laws]\n",
    );
    for (k, v) in lock {
        out.push_str(&format!("{k} = {v:?}\n"));
    }
    out
}

fn content_hash(data: &[u8]) -> String {
    let mut h0: u64 = 0xcbf29ce484222325;
    let mut h1: u64 = 0x100000001b3;
    for &b in data {
        h0 ^= u64::from(b);
        h0 = h0.wrapping_mul(0x100000001b3);
        h1 ^= u64::from(b.rotate_left(3));
        h1 = h1.wrapping_mul(0xcbf29ce484222325);
    }
    format!("{h0:016x}{h1:016x}")
}

fn law_body_fingerprint(f: &Function, interner: &Interner) -> String {
    let mut buf = String::new();
    buf.push_str(interner.resolve(f.name));
    buf.push('\n');
    if let Some(desc) = f.law_description() {
        buf.push_str(interner.resolve(desc));
        buf.push('\n');
    }
    for p in &f.params {
        buf.push_str(interner.resolve(p.name));
        buf.push(':');
        buf.push_str(&type_fingerprint(&p.ty, interner));
        buf.push('\n');
    }
    fingerprint_body(&f.body, interner, &mut buf);
    content_hash(buf.as_bytes())
}

fn type_fingerprint(ty: &TypeExpr, interner: &Interner) -> String {
    match ty {
        TypeExpr::Named(s, _) => interner.resolve(*s).to_string(),
        TypeExpr::Optional(inner, _) => format!("?{}", type_fingerprint(inner, interner)),
        TypeExpr::Error(inner, _) => format!("!{}", type_fingerprint(inner, interner)),
        TypeExpr::Path(segs, _) => segs
            .iter()
            .map(|s| interner.resolve(*s))
            .collect::<Vec<_>>()
            .join("."),
        TypeExpr::Chan(inner, _) => format!("chan[{}]", type_fingerprint(inner, interner)),
        TypeExpr::Type(_) => "type".into(),
        TypeExpr::Code(inner, _) => format!("Code[{}]", type_fingerprint(inner, interner)),
        TypeExpr::Apply { name, args, .. } => {
            let args = args
                .iter()
                .map(|arg| type_fingerprint(arg, interner))
                .collect::<Vec<_>>();
            format!("{}[{}]", interner.resolve(*name), args.join(","))
        }
    }
}

fn fingerprint_body(body: &FnBody, interner: &Interner, buf: &mut String) {
    match body {
        FnBody::Expr(e) => fingerprint_expr(e, interner, buf),
        FnBody::Block(b) => fingerprint_block(b, interner, buf),
    }
}

fn fingerprint_block(b: &Block, interner: &Interner, buf: &mut String) {
    for s in &b.stmts {
        fingerprint_stmt(s, interner, buf);
    }
}

fn fingerprint_stmt(s: &Stmt, interner: &Interner, buf: &mut String) {
    match s {
        Stmt::Binding(b) => {
            buf.push_str(interner.resolve(b.name));
            buf.push('=');
            fingerprint_expr(&b.value, interner, buf);
        }
        Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => {
            fingerprint_expr(e, interner, buf)
        }
        Stmt::Return(None, _) => buf.push_str("return;"),
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            buf.push_str("ensure:");
            fingerprint_expr(cond, interner, buf);
            fingerprint_block(else_block, interner, buf);
        }
        Stmt::Panic(msg, _) => {
            buf.push_str("panic:");
            if let Some(m) = msg {
                fingerprint_expr(m, interner, buf);
            }
        }
        Stmt::When { subjects, arms, .. } => {
            buf.push_str("when:");
            for s in subjects {
                fingerprint_expr(s, interner, buf);
            }
            for arm in arms {
                fingerprint_expr(&arm.body, interner, buf);
            }
        }
        Stmt::Each { body, .. }
        | Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::Scope { body, .. } => fingerprint_block(body, interner, buf),
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                fingerprint_expr(t, interner, buf);
            }
            for v in values {
                fingerprint_expr(v, interner, buf);
            }
        }
        Stmt::Guarded { stmt, cond, .. } => {
            fingerprint_stmt(stmt, interner, buf);
            fingerprint_expr(cond, interner, buf);
        }
        Stmt::Destructure { value, .. } => fingerprint_expr(value, interner, buf),
        Stmt::Break(_) => buf.push_str("break;"),
        Stmt::Continue(_) => buf.push_str("continue;"),
        Stmt::Comptime(_) => buf.push_str("comptime;"),
    }
}

fn fingerprint_expr(e: &Expr, interner: &Interner, buf: &mut String) {
    match e {
        Expr::Ident(s, _) | Expr::String(s, _) | Expr::ComptimeRef { name: s, .. } => {
            buf.push_str(interner.resolve(*s))
        }
        Expr::Int(n, _) => buf.push_str(&n.to_string()),
        Expr::Float(n, _) => buf.push_str(&n.to_string()),
        Expr::Bool(b, _) => buf.push_str(if *b { "true" } else { "false" }),
        Expr::None(_) => buf.push_str("none"),
        Expr::Binary {
            op, left, right, ..
        } => {
            fingerprint_expr(left, interner, buf);
            buf.push_str(binop_tag(*op));
            fingerprint_expr(right, interner, buf);
        }
        Expr::Unary { op, expr, .. } => {
            buf.push_str(unop_tag(*op));
            fingerprint_expr(expr, interner, buf);
        }
        Expr::Call { callee, args, .. } => {
            fingerprint_expr(callee, interner, buf);
            buf.push('(');
            for a in args {
                fingerprint_expr(a, interner, buf);
                buf.push(',');
            }
            buf.push(')');
        }
        Expr::Field { base, field, .. } => {
            fingerprint_expr(base, interner, buf);
            buf.push('.');
            buf.push_str(interner.resolve(*field));
        }
        Expr::Assign { target, value, .. } => {
            fingerprint_expr(target, interner, buf);
            buf.push('=');
            fingerprint_expr(value, interner, buf);
        }
        Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => fingerprint_block(b, interner, buf),
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            fingerprint_expr(cond, interner, buf);
            fingerprint_block(then_block, interner, buf);
            if let Some(eb) = else_block {
                fingerprint_block(eb, interner, buf);
            }
        }
        Expr::Or { left, right, .. } => {
            fingerprint_expr(left, interner, buf);
            fingerprint_expr(right, interner, buf);
        }
        Expr::OrErr { left, body, .. } => {
            fingerprint_expr(left, interner, buf);
            fingerprint_block(body, interner, buf);
        }
        Expr::Panic { .. } => buf.push_str("PANIC"),
        _ => buf.push('·'),
    }
}

fn binop_tag(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::UShr => ">>>",
    }
}

fn unop_tag(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "!",
        UnOp::BitNot => "~",
        UnOp::MutRef => "@",
    }
}

pub fn verify_module(
    module: &Module,
    interner: &Interner,
    diagnostics: &mut Diagnostics,
    opts: &VerifyOptions,
) -> VerifyReport {
    let (laws, proofs) = collect_verify_items(module, interner);
    let mut report = VerifyReport {
        proofs_seen: proofs.len(),
        laws_seen: laws.len(),
        ..VerifyReport::default()
    };

    let proof_by_law: BTreeMap<SymId, &Function> = proofs
        .iter()
        .filter_map(|p| p.proof_target().map(|law| (law, *p)))
        .collect();

    let mut fns: BTreeMap<SymId, &Function> = BTreeMap::new();
    for item in &module.items {
        if let Item::Function(f) = item
            && !f.is_verify_only()
        {
            fns.insert(f.name, f);
        }
    }
    let globals = collect_module_globals(module, interner);

    for law in &laws {
        let name = interner.resolve(law.name).to_string();
        let desc = law
            .law_description()
            .map(|d| interner.resolve(d).to_string())
            .unwrap_or_default();
        let body_hash = law_body_fingerprint(law, interner);
        report.new_lock.insert(name.clone(), body_hash.clone());

        if opts.locked_laws {
            match opts.law_lock.get(&name) {
                None => {
                    let msg =
                        format!("law `{name}` is not present in laws.lock (locked-laws mode)");
                    diagnostics.error_code(DiagnosticKind::Other, law.span, E0503, msg.clone());
                    report.results.push(LawResult {
                        name,
                        description: desc,
                        span: law.span,
                        passed: false,
                        tier: None,
                        message: msg,
                        counterexample: None,
                        code: Some(E0503),
                        body_hash,
                    });
                    continue;
                }
                Some(expected) if expected != &body_hash => {
                    let msg = format!(
                        "law `{name}` body changed under --locked-laws (hash {body_hash} ≠ lock {expected})"
                    );
                    diagnostics.error_code(DiagnosticKind::Other, law.span, E0503, msg.clone());
                    report.results.push(LawResult {
                        name,
                        description: desc,
                        span: law.span,
                        passed: false,
                        tier: None,
                        message: msg,
                        counterexample: None,
                        code: Some(E0503),
                        body_hash,
                    });
                    continue;
                }
                _ => {}
            }
        }

        let proof = proof_by_law.get(&law.name).copied();
        if let Some(p) = proof
            && proof_has_todo(p, interner)
        {
            let msg = format!(
                "proof `{}` for law `{name}` contains #todo/#unimplemented",
                interner.resolve(p.name)
            );
            diagnostics.error_code(DiagnosticKind::Other, p.span, E0503, msg.clone());
            report.results.push(LawResult {
                name,
                description: desc,
                span: law.span,
                passed: false,
                tier: None,
                message: msg,
                counterexample: None,
                code: Some(E0503),
                body_hash,
            });
            continue;
        }

        let result = discharge_law(law, proof, &fns, &globals, interner, opts);
        if !result.passed {
            let code = result.code.unwrap_or(E0503);
            diagnostics.error_code(
                DiagnosticKind::Other,
                law.span,
                code,
                result.message.clone(),
            );
        }
        report.results.push(result);
    }

    for p in &proofs {
        let Some(target) = p.proof_target() else {
            continue;
        };
        if laws.iter().all(|l| l.name != target) {
            let msg = format!(
                "proof `{}` references unknown law `{}`",
                interner.resolve(p.name),
                interner.resolve(target)
            );
            diagnostics.error_code(DiagnosticKind::Other, p.span, E0503, msg);
        }
    }

    // Structural laws (cannot be expressed as Super `#law` bodies): LAW-BENCH-001/002.
    let bench_root = opts.bench_root.clone().or_else(|| {
        opts.pod_root
            .as_ref()
            .and_then(|r| crate::bench_purity::find_benchmarks_root(Some(r)))
    });
    if let Some(bench_root) = bench_root {
        let (_proof, result) = crate::bench_purity::prove_law_bench_001(&bench_root);
        report.laws_seen += 1;
        report.proofs_seen += 1; // structural proof object (scan + Gate B evidence)
        if !result.passed {
            let code = result.code.unwrap_or(super_core::err::E0504);
            diagnostics.error_code(
                DiagnosticKind::Other,
                result.span,
                code,
                result.message.clone(),
            );
        }
        report
            .new_lock
            .insert(result.name.clone(), result.body_hash.clone());
        report.results.push(result);

        if let Some(workspace) = crate::bench_purity::find_workspace_root(Some(&bench_root)) {
            let (_surf, surf_result) = crate::bench_purity::prove_law_bench_002(&workspace);
            report.laws_seen += 1;
            report.proofs_seen += 1;
            if !surf_result.passed {
                let code = surf_result.code.unwrap_or(super_core::err::E0505);
                diagnostics.error_code(
                    DiagnosticKind::Other,
                    surf_result.span,
                    code,
                    surf_result.message.clone(),
                );
            }
            report
                .new_lock
                .insert(surf_result.name.clone(), surf_result.body_hash.clone());
            report.results.push(surf_result);
        }
    }

    report
}

fn proof_has_todo(f: &Function, interner: &Interner) -> bool {
    match &f.body {
        FnBody::Expr(e) => expr_has_todo(e, interner),
        FnBody::Block(b) => block_has_todo(b, interner),
    }
}

fn block_has_todo(b: &Block, interner: &Interner) -> bool {
    b.stmts.iter().any(|s| stmt_has_todo(s, interner))
}

fn stmt_has_todo(s: &Stmt, interner: &Interner) -> bool {
    match s {
        Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => expr_has_todo(e, interner),
        Stmt::Binding(b) => expr_has_todo(&b.value, interner),
        Stmt::Ensure {
            cond, else_block, ..
        } => expr_has_todo(cond, interner) || block_has_todo(else_block, interner),
        Stmt::Each { body, .. }
        | Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::Scope { body, .. } => block_has_todo(body, interner),
        Stmt::When { subjects, arms, .. } => {
            subjects.iter().any(|e| expr_has_todo(e, interner))
                || arms.iter().any(|a| expr_has_todo(&a.body, interner))
        }
        Stmt::Guarded { stmt, cond, .. } => {
            stmt_has_todo(stmt, interner) || expr_has_todo(cond, interner)
        }
        Stmt::Destructure { value, .. } => expr_has_todo(value, interner),
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            targets.iter().any(|e| expr_has_todo(e, interner))
                || values.iter().any(|e| expr_has_todo(e, interner))
        }
        Stmt::Panic(Some(Expr::String(s, _)), _) => {
            let msg = interner.resolve(*s);
            msg.contains("not yet implemented") || msg.contains("not implemented")
        }
        _ => false,
    }
}

fn expr_has_todo(e: &Expr, interner: &Interner) -> bool {
    match e {
        Expr::Panic { message, .. } => match message.as_deref() {
            Some(Expr::String(s, _)) => {
                let msg = interner.resolve(*s);
                msg.contains("not yet implemented") || msg.contains("not implemented")
            }
            _ => false,
        },
        Expr::Binary { left, right, .. }
        | Expr::Assign {
            target: left,
            value: right,
            ..
        }
        | Expr::Or { left, right, .. } => {
            expr_has_todo(left, interner) || expr_has_todo(right, interner)
        }
        Expr::Unary { expr, .. }
        | Expr::Field { base: expr, .. }
        | Expr::Await(expr, _)
        | Expr::Spawn { body: expr, .. }
        | Expr::Shared { value: expr, .. }
        | Expr::Quote { body: expr, .. }
        | Expr::Splice { code: expr, .. } => expr_has_todo(expr, interner),
        Expr::Call { callee, args, .. } => {
            expr_has_todo(callee, interner) || args.iter().any(|a| expr_has_todo(a, interner))
        }
        Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => block_has_todo(b, interner),
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            expr_has_todo(cond, interner)
                || block_has_todo(then_block, interner)
                || else_block
                    .as_ref()
                    .is_some_and(|b| block_has_todo(b, interner))
        }
        Expr::OrErr { left, body, .. } => {
            expr_has_todo(left, interner) || block_has_todo(body, interner)
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            expr_has_todo(cond, interner)
                || expr_has_todo(then_expr, interner)
                || expr_has_todo(else_expr, interner)
        }
        Expr::Lambda { body, .. } => match body.as_ref() {
            FnBody::Expr(e) => expr_has_todo(e, interner),
            FnBody::Block(b) => block_has_todo(b, interner),
        },
        _ => false,
    }
}

fn discharge_law(
    law: &Function,
    proof: Option<&Function>,
    fns: &BTreeMap<SymId, &Function>,
    globals: &BTreeMap<SymId, SVal>,
    interner: &Interner,
    opts: &VerifyOptions,
) -> LawResult {
    let name = interner.resolve(law.name).to_string();
    let description = law
        .law_description()
        .map(|d| interner.resolve(d).to_string())
        .unwrap_or_default();
    let body_hash = law_body_fingerprint(law, interner);

    let try_smt = matches!(opts.solver, SolverKind::Auto | SolverKind::Smt);
    let try_ct = matches!(opts.solver, SolverKind::Auto | SolverKind::Comptime);
    let try_fuzz = matches!(opts.solver, SolverKind::Auto | SolverKind::Fuzz);

    if try_ct {
        if let Some(p) = proof
            && let Some(true) = try_equational_proof(p, interner)
        {
            return LawResult {
                name,
                description,
                span: law.span,
                passed: true,
                tier: Some(DischargeTier::Comptime),
                message: "discharged via equational / comptime rewrite".into(),
                counterexample: None,
                code: None,
                body_hash,
            };
        }
        if let Some(true) = try_equational_law_asserts(law, interner) {
            return LawResult {
                name,
                description,
                span: law.span,
                passed: true,
                tier: Some(DischargeTier::Comptime),
                message: "law asserts discharged via equational rewrite".into(),
                counterexample: None,
                code: None,
                body_hash,
            };
        }
    }

    if try_smt {
        match try_fuzz_law(law, fns, globals, interner) {
            SymOutcome::Proved => {
                return LawResult {
                    name,
                    description,
                    span: law.span,
                    passed: true,
                    tier: Some(DischargeTier::Smt),
                    message: "discharged via symbolic bitvector reasoning".into(),
                    counterexample: None,
                    code: None,
                    body_hash,
                };
            }
            SymOutcome::Falsified(cex) => {
                let message = format!(
                    "law `{name}` falsified (E0502); counterexample: {}",
                    format_cex(&cex)
                );
                return LawResult {
                    name,
                    description,
                    span: law.span,
                    passed: false,
                    tier: Some(DischargeTier::Smt),
                    message,
                    counterexample: Some(cex),
                    code: Some(E0502),
                    body_hash,
                };
            }
            SymOutcome::Unknown => {}
        }
    }

    if try_fuzz {
        match try_fuzz_law(law, fns, globals, interner) {
            SymOutcome::Proved => {
                return LawResult {
                    name,
                    description,
                    span: law.span,
                    passed: true,
                    tier: Some(DischargeTier::Fuzz),
                    message: "held under bounded deterministic property fuzzing".into(),
                    counterexample: None,
                    code: None,
                    body_hash,
                };
            }
            SymOutcome::Falsified(cex) => {
                let message = format!(
                    "law `{name}` falsified under fuzzing (E0502); counterexample: {}",
                    format_cex(&cex)
                );
                return LawResult {
                    name,
                    description,
                    span: law.span,
                    passed: false,
                    tier: Some(DischargeTier::Fuzz),
                    message,
                    counterexample: Some(cex),
                    code: Some(E0502),
                    body_hash,
                };
            }
            SymOutcome::Unknown => {}
        }
    }

    let msg = if proof.is_some() {
        format!("incomplete proof for law `{name}` (E0503): no tier discharged the invariant")
    } else {
        format!("law `{name}` has no valid proof/discharge (E0503)")
    };
    LawResult {
        name,
        description,
        span: law.span,
        passed: false,
        tier: None,
        message: msg,
        counterexample: None,
        code: Some(E0503),
        body_hash,
    }
}

fn format_cex(cex: &Counterexample) -> String {
    cex.bindings
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

enum SymOutcome {
    Proved,
    Falsified(Counterexample),
    Unknown,
}

fn try_equational_proof(proof: &Function, interner: &Interner) -> Option<bool> {
    let asserts = collect_ensure_conds(&proof.body);
    if asserts.is_empty() {
        return None;
    }
    Some(asserts.iter().all(|e| algebraic_identity(e, interner)))
}

fn try_equational_law_asserts(law: &Function, interner: &Interner) -> Option<bool> {
    let asserts = collect_ensure_conds(&law.body);
    if asserts.is_empty() {
        return None;
    }
    if asserts.iter().any(|e| expr_has_call(e)) {
        return None;
    }
    Some(asserts.iter().all(|e| algebraic_identity(e, interner)))
}

fn collect_ensure_conds(body: &FnBody) -> Vec<&Expr> {
    let mut out = Vec::new();
    match body {
        FnBody::Expr(e) => collect_ensure_from_expr(e, &mut out),
        FnBody::Block(b) => collect_ensure_from_block(b, &mut out),
    }
    out
}

fn collect_ensure_from_block<'a>(b: &'a Block, out: &mut Vec<&'a Expr>) {
    // `#assert(c)` expands to `ok = c; ensure ok else …`: judge the bound condition.
    let mut bound: HashMap<SymId, &'a Expr> = HashMap::new();
    for s in &b.stmts {
        match s {
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                let judged = match cond {
                    Expr::Ident(name, _) => bound.get(name).copied().unwrap_or(cond),
                    _ => cond,
                };
                out.push(judged);
                collect_ensure_from_block(else_block, out);
            }
            Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => {
                collect_ensure_from_expr(e, out)
            }
            Stmt::Binding(bind) => {
                bound.insert(bind.name, &bind.value);
                collect_ensure_from_expr(&bind.value, out)
            }
            Stmt::Each { body, .. }
            | Stmt::While { body, .. }
            | Stmt::Loop { body, .. }
            | Stmt::Scope { body, .. } => collect_ensure_from_block(body, out),
            _ => {}
        }
    }
}

fn collect_ensure_from_expr<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => collect_ensure_from_block(b, out),
        Expr::If {
            then_block,
            else_block,
            ..
        } => {
            collect_ensure_from_block(then_block, out);
            if let Some(eb) = else_block {
                collect_ensure_from_block(eb, out);
            }
        }
        Expr::OrErr { body, .. } => collect_ensure_from_block(body, out),
        _ => {}
    }
}

fn expr_has_call(e: &Expr) -> bool {
    match e {
        Expr::Call { .. } | Expr::Pipeline { .. } => true,
        Expr::Binary { left, right, .. }
        | Expr::Assign {
            target: left,
            value: right,
            ..
        }
        | Expr::Or { left, right, .. } => expr_has_call(left) || expr_has_call(right),
        Expr::Unary { expr, .. }
        | Expr::Field { base: expr, .. }
        | Expr::Index { base: expr, .. } => expr_has_call(expr),
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => expr_has_call(cond) || expr_has_call(then_expr) || expr_has_call(else_expr),
        Expr::Block(b) => b.stmts.iter().any(|s| match s {
            Stmt::Expr(e) | Stmt::Return(Some(e), _) => expr_has_call(e),
            Stmt::Ensure { cond, .. } => expr_has_call(cond),
            Stmt::Binding(b) => expr_has_call(&b.value),
            _ => false,
        }),
        _ => false,
    }
}

fn algebraic_identity(expr: &Expr, interner: &Interner) -> bool {
    match expr {
        Expr::Binary {
            op: BinOp::Eq,
            left,
            right,
            ..
        } => simplify_poly(left, interner) == simplify_poly(right, interner),
        Expr::Binary {
            op: BinOp::And,
            left,
            right,
            ..
        } => algebraic_identity(left, interner) && algebraic_identity(right, interner),
        Expr::Bool(true, _) => true,
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Poly {
    terms: BTreeMap<Vec<String>, i64>,
}

impl Poly {
    fn const_val(n: i64) -> Self {
        let mut terms = BTreeMap::new();
        if n != 0 {
            terms.insert(vec![], n);
        }
        Self { terms }
    }

    fn var(path: Vec<String>) -> Self {
        let mut terms = BTreeMap::new();
        terms.insert(path, 1);
        Self { terms }
    }

    fn add(mut self, other: Self) -> Self {
        for (k, v) in other.terms {
            let e = self.terms.entry(k).or_insert(0i64);
            *e = (*e).wrapping_add(v);
        }
        self.terms.retain(|_, c| *c != 0);
        self
    }

    fn sub(self, other: Self) -> Self {
        let neg = Poly {
            terms: other
                .terms
                .into_iter()
                .map(|(k, v)| (k, v.wrapping_neg()))
                .collect(),
        };
        self.add(neg)
    }

    fn mul(self, other: Self) -> Self {
        let mut terms = BTreeMap::new();
        for (k1, c1) in &self.terms {
            for (k2, c2) in &other.terms {
                let mut k = k1.clone();
                k.extend(k2.iter().cloned());
                k.sort();
                let e = terms.entry(k).or_insert(0i64);
                *e = (*e).wrapping_add(c1.wrapping_mul(*c2));
            }
        }
        terms.retain(|_, c| *c != 0);
        Self { terms }
    }
}

fn simplify_poly(expr: &Expr, interner: &Interner) -> Poly {
    match expr {
        Expr::Int(n, _) => Poly::const_val(*n),
        Expr::Ident(s, _) => Poly::var(vec![interner.resolve(*s).to_string()]),
        Expr::Field { base, field, .. } => {
            let mut path = field_path(base, interner);
            path.push(interner.resolve(*field).to_string());
            Poly::var(path)
        }
        Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } => Poly::const_val(0).sub(simplify_poly(expr, interner)),
        Expr::Binary {
            op: BinOp::Add,
            left,
            right,
            ..
        } => simplify_poly(left, interner).add(simplify_poly(right, interner)),
        Expr::Binary {
            op: BinOp::Sub,
            left,
            right,
            ..
        } => simplify_poly(left, interner).sub(simplify_poly(right, interner)),
        Expr::Binary {
            op: BinOp::Mul,
            left,
            right,
            ..
        } => simplify_poly(left, interner).mul(simplify_poly(right, interner)),
        _ => Poly::var(vec![format!("opaque:{expr:?}")]),
    }
}

fn field_path(expr: &Expr, interner: &Interner) -> Vec<String> {
    match expr {
        Expr::Ident(s, _) => vec![interner.resolve(*s).to_string()],
        Expr::Field { base, field, .. } => {
            let mut p = field_path(base, interner);
            p.push(interner.resolve(*field).to_string());
            p
        }
        _ => vec![format!("opaque:{expr:?}")],
    }
}

fn try_fuzz_law(
    law: &Function,
    fns: &BTreeMap<SymId, &Function>,
    globals: &BTreeMap<SymId, SVal>,
    interner: &Interner,
) -> SymOutcome {
    let samples: &[i64] = &[
        0,
        1,
        -1,
        2,
        -2,
        7,
        42,
        100,
        -100,
        i64::MAX,
        i64::MIN,
        i64::MAX - 1,
        i64::MIN + 1,
    ];

    let mut param_domains: Vec<Vec<SVal>> = Vec::new();
    for p in &law.params {
        let ty_name = match &p.ty {
            TypeExpr::Named(s, _) => interner.resolve(*s).to_string(),
            _ => "i64".into(),
        };
        if ty_name == "bool" {
            param_domains.push(vec![SVal::Bool(false), SVal::Bool(true)]);
        } else if ty_name == "i64" || ty_name == "int" || ty_name == "u64" || ty_name == "usize" {
            param_domains.push(samples.iter().copied().map(SVal::Int).collect());
        } else {
            let mut variants = Vec::new();
            for &n in samples.iter().take(5) {
                let mut fields = BTreeMap::new();
                fields.insert("balance".into(), SVal::Int(n));
                variants.push(SVal::Record(fields));
            }
            param_domains.push(variants);
        }
    }

    if param_domains.is_empty() {
        let mut env = Env::new(fns, globals, interner);
        env.law_mode = true;
        return match eval_law_body(law, &mut env) {
            EvalAssert::Ok => SymOutcome::Proved,
            EvalAssert::Fail(cex) => SymOutcome::Falsified(cex),
            EvalAssert::Abort => SymOutcome::Unknown,
        };
    }

    let mut idxs = vec![0usize; param_domains.len()];
    let mut any = false;
    let mut iterations = 0usize;
    const MAX_ITERS: usize = 4_096;
    loop {
        iterations += 1;
        if iterations > MAX_ITERS {
            break;
        }
        any = true;
        let mut env = Env::new(fns, globals, interner);
        env.law_mode = true;
        let mut bindings = BTreeMap::new();
        for (i, p) in law.params.iter().enumerate() {
            let v = param_domains[i][idxs[i]].clone();
            bindings.insert(interner.resolve(p.name).to_string(), v.display());
            env.locals.insert(p.name, v);
        }
        match eval_law_body(law, &mut env) {
            EvalAssert::Ok => {}
            EvalAssert::Fail(mut cex) => {
                if cex.bindings.is_empty() {
                    cex.bindings = bindings;
                }
                return SymOutcome::Falsified(cex);
            }
            EvalAssert::Abort => return SymOutcome::Unknown,
        }
        let mut carry = true;
        for i in (0..idxs.len()).rev() {
            if !carry {
                break;
            }
            idxs[i] += 1;
            if idxs[i] < param_domains[i].len() {
                carry = false;
            } else {
                idxs[i] = 0;
            }
        }
        if carry {
            break;
        }
    }
    if any {
        SymOutcome::Proved
    } else {
        SymOutcome::Unknown
    }
}

/// Seed module-level literal bindings (`+ENC_HEX = "…"`) for real-body evaluation.
fn collect_module_globals(module: &Module, interner: &Interner) -> BTreeMap<SymId, SVal> {
    let mut out = BTreeMap::new();
    for item in &module.items {
        let Item::Binding(b) = item else { continue };
        if let Some(v) = eval_const_expr(&b.value, interner) {
            out.insert(b.name, v);
        }
    }
    out
}

fn eval_const_expr(e: &Expr, interner: &Interner) -> Option<SVal> {
    match e {
        Expr::Int(n, _) => Some(SVal::Int(*n)),
        Expr::Float(n, _) => Some(SVal::Float(*n)),
        Expr::Bool(b, _) => Some(SVal::Bool(*b)),
        Expr::String(s, _) => Some(SVal::Str(interner.resolve(*s).to_string())),
        Expr::None(_) => Some(SVal::Unit),
        Expr::Array(elems, _) => {
            let mut xs = Vec::with_capacity(elems.len());
            for el in elems {
                xs.push(eval_const_expr(el, interner)?);
            }
            Some(SVal::Arr(xs))
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
enum SVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<SVal>),
    Record(BTreeMap<String, SVal>),
    Unit,
    Err(String),
}

impl SVal {
    fn display(&self) -> String {
        match self {
            SVal::Int(n) => n.to_string(),
            SVal::Float(n) => format!("{n}"),
            SVal::Bool(b) => b.to_string(),
            SVal::Str(s) => format!("\"{s}\""),
            SVal::Arr(xs) => {
                let inner = xs
                    .iter()
                    .map(SVal::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            SVal::Unit => "()".into(),
            SVal::Err(e) => format!("err({e})"),
            SVal::Record(f) => {
                let inner = f
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(".{{{inner}}}")
            }
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            SVal::Bool(b) => Some(*b),
            SVal::Int(n) => Some(*n != 0),
            SVal::Float(n) => Some(*n != 0.0),
            _ => None,
        }
    }

    fn as_int(&self) -> Option<i64> {
        match self {
            SVal::Int(n) => Some(*n),
            SVal::Bool(b) => Some(i64::from(*b)),
            SVal::Float(n) => Some(*n as i64),
            _ => None,
        }
    }

    fn as_float(&self) -> Option<f64> {
        match self {
            SVal::Float(n) => Some(*n),
            SVal::Int(n) => Some(*n as f64),
            SVal::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn eq_val(&self, other: &SVal) -> bool {
        match (self, other) {
            (SVal::Int(a), SVal::Int(b)) => a == b,
            (SVal::Float(a), SVal::Float(b)) => a == b,
            (SVal::Int(a), SVal::Float(b)) | (SVal::Float(b), SVal::Int(a)) => {
                (*a as f64) == *b
            }
            (SVal::Bool(a), SVal::Bool(b)) => a == b,
            (SVal::Str(a), SVal::Str(b)) => a == b,
            (SVal::Unit, SVal::Unit) => true,
            (SVal::Arr(a), SVal::Arr(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_val(y))
            }
            (SVal::Record(a), SVal::Record(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|o| v.eq_val(o)))
            }
            _ => false,
        }
    }
}

/// Built-in Iterable methods the verify fuzz/comptime evaluator can run on
/// concrete `SVal::Arr` values (Round 7 real-body laws).
fn eval_arr_method(method: &str, arr: &[SVal], args: &[SVal]) -> Option<SVal> {
    match method {
        "sum" if args.is_empty() => {
            let mut acc = 0i64;
            for v in arr {
                acc = acc.wrapping_add(v.as_int().unwrap_or(0));
            }
            Some(SVal::Int(acc))
        }
        "product" if args.is_empty() => {
            let mut acc = 1i64;
            for v in arr {
                acc = acc.wrapping_mul(v.as_int().unwrap_or(0));
            }
            Some(SVal::Int(acc))
        }
        "min" if args.is_empty() => {
            let mut it = arr.iter().filter_map(|v| v.as_int());
            let first = it.next().unwrap_or(0);
            Some(SVal::Int(it.fold(first, |a, b| a.min(b))))
        }
        "max" if args.is_empty() => {
            let mut it = arr.iter().filter_map(|v| v.as_int());
            let first = it.next().unwrap_or(0);
            Some(SVal::Int(it.fold(first, |a, b| a.max(b))))
        }
        "len" | "size" if args.is_empty() => Some(SVal::Int(arr.len() as i64)),
        _ => None,
    }
}

fn eval_len_builtin(args: &[SVal]) -> Option<SVal> {
    match args.first() {
        Some(SVal::Arr(xs)) => Some(SVal::Int(xs.len() as i64)),
        Some(SVal::Str(s)) => Some(SVal::Int(s.len() as i64)),
        _ => None,
    }
}

/// String builtins matching `std.core.str` pure surface (byte_len / starts_with).
fn eval_str_builtin(name: &str, args: &[SVal]) -> Option<SVal> {
    match name {
        "byte_len" if args.len() == 1 => match &args[0] {
            SVal::Str(s) => Some(SVal::Int(s.len() as i64)),
            _ => None,
        },
        "starts_with" if args.len() == 2 => match (&args[0], &args[1]) {
            (SVal::Str(s), SVal::Str(p)) => Some(SVal::Bool(s.starts_with(p.as_str()))),
            _ => None,
        },
        "bytes_len" if args.len() == 1 => match &args[0] {
            SVal::Arr(xs) => Some(SVal::Int(xs.len() as i64)),
            _ => None,
        },
        "bytes_equal" if args.len() == 2 => match (&args[0], &args[1]) {
            (SVal::Arr(a), SVal::Arr(b)) => Some(SVal::Bool(
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_val(y)),
            )),
            _ => None,
        },
        "m_min" if args.len() == 2 => {
            let a = args[0].as_float()?;
            let b = args[1].as_float()?;
            Some(SVal::Float(a.min(b)))
        }
        "m_max" if args.len() == 2 => {
            let a = args[0].as_float()?;
            let b = args[1].as_float()?;
            Some(SVal::Float(a.max(b)))
        }
        "m_clamp" if args.len() == 3 => {
            let x = args[0].as_float()?;
            let lo = args[1].as_float()?;
            let hi = args[2].as_float()?;
            Some(SVal::Float(x.max(lo).min(hi)))
        }
        _ => None,
    }
}

struct Env<'a> {
    locals: BTreeMap<SymId, SVal>,
    fns: &'a BTreeMap<SymId, &'a Function>,
    globals: &'a BTreeMap<SymId, SVal>,
    interner: &'a Interner,
    depth: usize,
    /// Implicit `it` / `index` for `for` bodies in the verify evaluator.
    it: Option<SVal>,
    index: Option<i64>,
    /// When true, a failed `ensure` falsifies the law even if an else block
    /// exists (`#assert` expands to `ensure ok else { … }`).
    law_mode: bool,
}

impl<'a> Env<'a> {
    fn new(
        fns: &'a BTreeMap<SymId, &'a Function>,
        globals: &'a BTreeMap<SymId, SVal>,
        interner: &'a Interner,
    ) -> Self {
        Self {
            locals: BTreeMap::new(),
            fns,
            globals,
            interner,
            depth: 0,
            it: None,
            index: None,
            law_mode: false,
        }
    }
}

enum EvalAssert {
    Ok,
    Fail(Counterexample),
    Abort,
}

fn eval_law_body(law: &Function, env: &mut Env<'_>) -> EvalAssert {
    match &law.body {
        FnBody::Expr(e) => match eval_expr(e, env) {
            Ok(_) => EvalAssert::Ok,
            Err(EvalErr::Assert(cex)) => EvalAssert::Fail(cex),
            Err(_) => EvalAssert::Abort,
        },
        FnBody::Block(b) => eval_block_asserts(b, env),
    }
}

fn eval_block_asserts(b: &Block, env: &mut Env<'_>) -> EvalAssert {
    for s in &b.stmts {
        match eval_stmt(s, env) {
            Ok(()) => {}
            Err(EvalErr::Assert(cex)) => return EvalAssert::Fail(cex),
            Err(_) => return EvalAssert::Abort,
        }
    }
    EvalAssert::Ok
}

enum EvalErr {
    Assert(Counterexample),
    Abort,
    #[allow(dead_code)]
    FailValue(SVal),
    /// `return <expr>` from inside an ensured else / nested block.
    EarlyReturn(SVal),
}

fn eval_stmt(s: &Stmt, env: &mut Env<'_>) -> Result<(), EvalErr> {
    match s {
        Stmt::Binding(b) => {
            let v = eval_expr(&b.value, env)?;
            env.locals.insert(b.name, v);
            Ok(())
        }
        Stmt::Expr(e) => {
            let _ = eval_expr(e, env)?;
            Ok(())
        }
        Stmt::Return(Some(e), _) => {
            let v = eval_expr(e, env)?;
            Err(EvalErr::EarlyReturn(v))
        }
        Stmt::Return(None, _) => Err(EvalErr::EarlyReturn(SVal::Unit)),
        Stmt::Ensure {
            cond,
            else_block,
            ..
        } => {
            let v = eval_expr(cond, env)?;
            if v.as_bool() == Some(true) {
                Ok(())
            } else if env.law_mode || else_block.stmts.is_empty() {
                // `#assert` / law ensure: condition failure falsifies the law.
                // (Expansion is `ensure ok else { … }`; else must not mask failure.)
                let mut bindings = BTreeMap::new();
                for (k, v) in &env.locals {
                    bindings.insert(env.interner.resolve(*k).to_string(), v.display());
                }
                Err(EvalErr::Assert(Counterexample { bindings }))
            } else {
                // Ordinary `ensure cond else { return … }` in stdlib bodies.
                eval_block_unit(else_block, env)
            }
        }
        Stmt::Fail(e, _) => {
            let v = eval_expr(e, env)?;
            Err(EvalErr::FailValue(v))
        }
        Stmt::Panic(_, _) => Err(EvalErr::Abort),
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            let vals: Result<Vec<_>, _> = values.iter().map(|v| eval_expr(v, env)).collect();
            let vals = vals?;
            for (t, v) in targets.iter().zip(vals) {
                assign_to(t, v, env)?;
            }
            Ok(())
        }
        Stmt::Each {
            iter,
            bindings,
            body,
            ..
        } => {
            let iv = eval_expr(iter, env)?;
            match iv {
                SVal::Arr(items) => {
                    for (i, item) in items.into_iter().enumerate() {
                        if let Some(b0) = bindings.first() {
                            env.locals.insert(b0.name, item.clone());
                        }
                        env.it = Some(item);
                        env.index = Some(i as i64);
                        eval_block_unit(body, env)?;
                    }
                    env.it = None;
                    env.index = None;
                    Ok(())
                }
                _ => eval_block_unit(body, env),
            }
        }
        Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::Scope { body, .. } => eval_block_unit(body, env),
        Stmt::When {
            subjects, arms, ..
        } => {
            let _ = eval_when(subjects, arms, env)?;
            Ok(())
        }
        Stmt::Guarded { stmt, cond, .. } => {
            let c = eval_expr(cond, env)?;
            if c.as_bool() == Some(true) {
                eval_stmt(stmt, env)
            } else {
                Ok(())
            }
        }
        Stmt::Destructure { fields, value, .. } => {
            let v = eval_expr(value, env)?;
            if let SVal::Record(rec) = v {
                for (src, local) in fields {
                    let key = env.interner.resolve(*src).to_string();
                    if let Some(fv) = rec.get(&key) {
                        env.locals.insert(*local, fv.clone());
                    }
                }
            }
            Ok(())
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Comptime(_) => Ok(()),
    }
}

fn eval_block_unit(b: &Block, env: &mut Env<'_>) -> Result<(), EvalErr> {
    for s in &b.stmts {
        eval_stmt(s, env)?;
    }
    Ok(())
}

fn assign_to(target: &Expr, value: SVal, env: &mut Env<'_>) -> Result<(), EvalErr> {
    match target {
        Expr::Ident(name, _) => {
            env.locals.insert(*name, value);
            Ok(())
        }
        Expr::Field { base, field, .. } => {
            let key = env.interner.resolve(*field).to_string();
            match base.as_ref() {
                Expr::Ident(name, _) => {
                    let mut rec = match env.locals.get(name) {
                        Some(SVal::Record(r)) => r.clone(),
                        _ => BTreeMap::new(),
                    };
                    rec.insert(key, value);
                    env.locals.insert(*name, SVal::Record(rec));
                    Ok(())
                }
                _ => Err(EvalErr::Abort),
            }
        }
        _ => Err(EvalErr::Abort),
    }
}

fn eval_expr(e: &Expr, env: &mut Env<'_>) -> Result<SVal, EvalErr> {
    match e {
        Expr::Ident(s, _) => Ok(env
            .locals
            .get(s)
            .cloned()
            .or_else(|| env.globals.get(s).cloned())
            .unwrap_or(SVal::Err(format!("unbound {}", env.interner.resolve(*s))))),
        Expr::Int(n, _) => Ok(SVal::Int(*n)),
        Expr::Float(n, _) => Ok(SVal::Float(*n)),
        Expr::Bool(b, _) => Ok(SVal::Bool(*b)),
        Expr::String(s, _) => Ok(SVal::Str(env.interner.resolve(*s).to_string())),
        Expr::None(_) => Ok(SVal::Unit),
        Expr::Array(elems, _) => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(eval_expr(e, env)?);
            }
            Ok(SVal::Arr(out))
        }
        Expr::ArrayRepeat { value, len, .. } => {
            let v = eval_expr(value, env)?;
            let n = eval_expr(len, env)?.as_int().unwrap_or(0).max(0) as usize;
            Ok(SVal::Arr(vec![v; n]))
        }
        Expr::Range { start, end, .. } => {
            let s = eval_expr(start, env)?.as_int().unwrap_or(0);
            let e = eval_expr(end, env)?.as_int().unwrap_or(s);
            let mut xs = Vec::new();
            let mut i = s;
            while i < e {
                xs.push(SVal::Int(i));
                i = i.wrapping_add(1);
                if xs.len() > 100_000 {
                    break;
                }
            }
            Ok(SVal::Arr(xs))
        }
        Expr::Index { base, index, .. } => {
            let b = eval_expr(base, env)?;
            let i = eval_expr(index, env)?.as_int().unwrap_or(0);
            match b {
                SVal::Arr(xs) if i >= 0 && (i as usize) < xs.len() => Ok(xs[i as usize].clone()),
                // Match AOT `super_str_char_at`: index yields a one-char string, not a byte int.
                SVal::Str(s) if i >= 0 && (i as usize) < s.len() => {
                    let b = s.as_bytes()[i as usize];
                    Ok(SVal::Str(String::from_utf8_lossy(&[b]).into_owned()))
                }
                _ => Ok(SVal::Unit),
            }
        }
        Expr::It(_) => Ok(env.it.clone().unwrap_or(SVal::Unit)),
        Expr::IndexIdent(_) => Ok(SVal::Int(env.index.unwrap_or(0))),
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = eval_expr(left, env)?;
            let r = eval_expr(right, env)?;
            eval_binop(*op, l, r)
        }
        Expr::Unary { op, expr, .. } => {
            let v = eval_expr(expr, env)?;
            match op {
                UnOp::Neg => Ok(SVal::Int(v.as_int().unwrap_or(0).wrapping_neg())),
                UnOp::Not => Ok(SVal::Bool(!v.as_bool().unwrap_or(false))),
                UnOp::BitNot => Ok(SVal::Int(!v.as_int().unwrap_or(0))),
                UnOp::MutRef => Ok(v),
            }
        }
        Expr::Field { base, field, .. } => {
            let b = eval_expr(base, env)?;
            let key = env.interner.resolve(*field).to_string();
            match b {
                SVal::Record(r) => Ok(r.get(&key).cloned().unwrap_or(SVal::Int(0))),
                other => Ok(other),
            }
        }
        Expr::Assign { target, value, .. } => {
            let v = eval_expr(value, env)?;
            assign_to(target, v.clone(), env)?;
            Ok(v)
        }
        Expr::Call { callee, args, .. } => {
            let arg_vals: Result<Vec<_>, _> = args.iter().map(|a| eval_expr(a, env)).collect();
            let arg_vals = arg_vals?;
            match callee.as_ref() {
                Expr::Ident(name, _) => {
                    let mname = env.interner.resolve(*name);
                    if env.depth > 32 {
                        return Err(EvalErr::Abort);
                    }
                    // Prefer real module function bodies when present (mutation
                    // testing requires the call graph to reach the impl).
                    if let Some(f) = env.fns.get(name) {
                        let mut child = Env::new(env.fns, env.globals, env.interner);
                        child.depth = env.depth + 1;
                        // Stdlib bodies use ensure-else-return; not law_mode.
                        child.law_mode = false;
                        for (p, v) in f.params.iter().zip(&arg_vals) {
                            child.locals.insert(p.name, v.clone());
                        }
                        let body_res = match &f.body {
                            FnBody::Expr(e) => eval_expr(e, &mut child),
                            FnBody::Block(b) => match eval_block_value(b, &mut child) {
                                Ok(v) => Ok(v),
                                Err(EvalErr::EarlyReturn(v)) => Ok(v),
                                Err(EvalErr::FailValue(v)) => Ok(SVal::Err(v.display())),
                                Err(e) => Err(e),
                            },
                        };
                        // Host-only bodies often yield Unit; fall through to builtins.
                        if let Ok(v) = &body_res {
                            if !matches!(v, SVal::Unit) {
                                return body_res;
                            }
                        } else {
                            return body_res;
                        }
                    }
                    // UFCS Iterable builtins / host-mirror fallbacks.
                    if mname == "len" || mname == "__len" {
                        if let Some(v) = eval_len_builtin(&arg_vals) {
                            return Ok(v);
                        }
                    }
                    if let Some(SVal::Arr(xs)) = arg_vals.first()
                        && let Some(v) = eval_arr_method(mname, xs, &arg_vals[1..])
                    {
                        return Ok(v);
                    }
                    if let Some(v) = eval_str_builtin(mname, &arg_vals) {
                        return Ok(v);
                    }
                    Ok(SVal::Unit)
                }
                _ => Ok(SVal::Unit),
            }
        }
        Expr::Or { left, right, .. } => match eval_expr(left, env) {
            Ok(v) => match &v {
                SVal::Err(_) => eval_expr(right, env),
                _ => Ok(v),
            },
            Err(EvalErr::FailValue(_)) => eval_expr(right, env),
            Err(e) => Err(e),
        },
        Expr::OrErr { left, body, .. } => match eval_expr(left, env) {
            Ok(SVal::Err(_)) | Err(EvalErr::FailValue(_)) => {
                eval_block_unit(body, env)?;
                Ok(SVal::Unit)
            }
            Ok(v) => Ok(v),
            Err(e) => Err(e),
        },
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let c = eval_expr(cond, env)?;
            if c.as_bool() == Some(true) {
                eval_block_value(then_block, env)
            } else if let Some(eb) = else_block {
                eval_block_value(eb, env)
            } else {
                Ok(SVal::Unit)
            }
        }
        Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => eval_block_value(b, env),
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            let c = eval_expr(cond, env)?;
            if c.as_bool() == Some(true) {
                eval_expr(then_expr, env)
            } else {
                eval_expr(else_expr, env)
            }
        }
        Expr::StructLit { fields, .. }
        | Expr::InferredStruct { fields, .. }
        | Expr::Record(fields, _) => {
            let mut rec = BTreeMap::new();
            for f in fields {
                let name = f
                    .name
                    .map(|s| env.interner.resolve(s).to_string())
                    .or_else(|| match &f.value {
                        Expr::Ident(s, _) => Some(env.interner.resolve(*s).to_string()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "_".into());
                rec.insert(name, eval_expr(&f.value, env)?);
            }
            Ok(SVal::Record(rec))
        }
        Expr::Panic { .. } => Err(EvalErr::Abort),
        Expr::Pipeline {
            base,
            method,
            args,
            span,
            ..
        } => {
            let mut call_args = vec![(**base).clone()];
            call_args.extend(args.iter().cloned());
            let call = Expr::Call {
                callee: Box::new(Expr::Ident(*method, *span)),
                args: call_args,
                punning: false,
                span: *span,
            };
            eval_expr(&call, env)
        }
        _ => Ok(SVal::Unit),
    }
}

fn eval_block_value(b: &Block, env: &mut Env<'_>) -> Result<SVal, EvalErr> {
    let mut last = SVal::Unit;
    for s in &b.stmts {
        match s {
            // Propagate as EarlyReturn so `if cond { return x }` exits the
            // enclosing function rather than becoming the If's value only.
            Stmt::Return(Some(e), _) => {
                let v = eval_expr(e, env)?;
                return Err(EvalErr::EarlyReturn(v));
            }
            Stmt::Return(None, _) => return Err(EvalErr::EarlyReturn(SVal::Unit)),
            Stmt::Expr(e) => last = eval_expr(e, env)?,
            Stmt::When {
                subjects, arms, ..
            } => last = eval_when(subjects, arms, env)?,
            other => eval_stmt(other, env)?,
        }
    }
    Ok(last)
}

fn eval_when(
    subjects: &[Expr],
    arms: &[crate::ast::WhenArm],
    env: &mut Env<'_>,
) -> Result<SVal, EvalErr> {
    use crate::ast::WhenPattern;
    let subject = if subjects.is_empty() {
        SVal::Unit
    } else if subjects.len() == 1 {
        eval_expr(&subjects[0], env)?
    } else {
        let mut xs = Vec::with_capacity(subjects.len());
        for s in subjects {
            xs.push(eval_expr(s, env)?);
        }
        SVal::Arr(xs)
    };
    let mut else_body: Option<&Expr> = None;
    for arm in arms {
        match &arm.pattern {
            WhenPattern::Else => {
                else_body = Some(&arm.body);
            }
            WhenPattern::Expr(pat) => {
                let pv = eval_expr(pat, env)?;
                if subject.eq_val(&pv) {
                    return eval_expr(&arm.body, env);
                }
            }
            WhenPattern::After(_) | WhenPattern::VariantBind { .. } | WhenPattern::Tuple { .. } => {}
        }
    }
    if let Some(body) = else_body {
        return eval_expr(body, env);
    }
    Ok(SVal::Unit)
}

fn eval_binop(op: BinOp, l: SVal, r: SVal) -> Result<SVal, EvalErr> {
    let li = l.as_int();
    let ri = r.as_int();
    let lf = l.as_float();
    let rf = r.as_float();
    let floatish = matches!(l, SVal::Float(_)) || matches!(r, SVal::Float(_));
    match op {
        BinOp::Add if floatish => Ok(SVal::Float(lf.unwrap_or(0.0) + rf.unwrap_or(0.0))),
        BinOp::Sub if floatish => Ok(SVal::Float(lf.unwrap_or(0.0) - rf.unwrap_or(0.0))),
        BinOp::Mul if floatish => Ok(SVal::Float(lf.unwrap_or(0.0) * rf.unwrap_or(0.0))),
        BinOp::Div if floatish => {
            let d = rf.unwrap_or(1.0);
            Ok(SVal::Float(if d == 0.0 {
                0.0
            } else {
                lf.unwrap_or(0.0) / d
            }))
        }
        BinOp::Add => Ok(SVal::Int(li.unwrap_or(0).wrapping_add(ri.unwrap_or(0)))),
        BinOp::Sub => Ok(SVal::Int(li.unwrap_or(0).wrapping_sub(ri.unwrap_or(0)))),
        BinOp::Mul => Ok(SVal::Int(li.unwrap_or(0).wrapping_mul(ri.unwrap_or(0)))),
        BinOp::Div => {
            let d = ri.unwrap_or(1);
            if d == 0 {
                Ok(SVal::Int(0))
            } else {
                Ok(SVal::Int(li.unwrap_or(0).wrapping_div(d)))
            }
        }
        BinOp::Rem => {
            let d = ri.unwrap_or(1);
            if d == 0 {
                Ok(SVal::Int(0))
            } else {
                Ok(SVal::Int(li.unwrap_or(0).wrapping_rem(d)))
            }
        }
        BinOp::Eq => Ok(SVal::Bool(l.eq_val(&r))),
        BinOp::Ne => {
            let eq = eval_binop(BinOp::Eq, l, r)?;
            Ok(SVal::Bool(!eq.as_bool().unwrap_or(false)))
        }
        BinOp::Lt if floatish => Ok(SVal::Bool(lf.unwrap_or(0.0) < rf.unwrap_or(0.0))),
        BinOp::Le if floatish => Ok(SVal::Bool(lf.unwrap_or(0.0) <= rf.unwrap_or(0.0))),
        BinOp::Gt if floatish => Ok(SVal::Bool(lf.unwrap_or(0.0) > rf.unwrap_or(0.0))),
        BinOp::Ge if floatish => Ok(SVal::Bool(lf.unwrap_or(0.0) >= rf.unwrap_or(0.0))),
        BinOp::Lt => Ok(SVal::Bool(li.unwrap_or(0) < ri.unwrap_or(0))),
        BinOp::Le => Ok(SVal::Bool(li.unwrap_or(0) <= ri.unwrap_or(0))),
        BinOp::Gt => Ok(SVal::Bool(li.unwrap_or(0) > ri.unwrap_or(0))),
        BinOp::Ge => Ok(SVal::Bool(li.unwrap_or(0) >= ri.unwrap_or(0))),
        BinOp::And => Ok(SVal::Bool(
            l.as_bool().unwrap_or(false) && r.as_bool().unwrap_or(false),
        )),
        BinOp::Or => Ok(SVal::Bool(
            l.as_bool().unwrap_or(false) || r.as_bool().unwrap_or(false),
        )),
        BinOp::BitAnd => Ok(SVal::Int(li.unwrap_or(0) & ri.unwrap_or(0))),
        BinOp::BitOr => Ok(SVal::Int(li.unwrap_or(0) | ri.unwrap_or(0))),
        BinOp::BitXor => Ok(SVal::Int(li.unwrap_or(0) ^ ri.unwrap_or(0))),
        BinOp::Shl => Ok(SVal::Int(
            li.unwrap_or(0)
                .wrapping_shl(ri.unwrap_or(0).clamp(0, 63) as u32),
        )),
        BinOp::Shr => Ok(SVal::Int(
            li.unwrap_or(0)
                .wrapping_shr(ri.unwrap_or(0).clamp(0, 63) as u32),
        )),
        BinOp::UShr => Ok(SVal::Int(
            ((li.unwrap_or(0) as u64) >> (ri.unwrap_or(0).clamp(0, 63) as u32)) as i64,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lexer, Parser, SemanticAnalyzer};
    use super_core::{Diagnostics, Interner, SourceFile};

    fn analyze(src: &str) -> (Module, Interner, Diagnostics) {
        let source = SourceFile::new("test.sr", src.to_string());
        let mut interner = Interner::with_capacity(256);
        let mut diagnostics = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diagnostics).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diagnostics);
        let mut module = parser.parse_module();
        crate::macros::expand_module(&mut module, &mut interner, &mut diagnostics);
        let _typed = SemanticAnalyzer::new(&mut interner, &mut diagnostics).analyze(&module);
        (module, interner, diagnostics)
    }

    #[test]
    fn parses_and_discharges_equational_law() {
        let src = r#"
#law("add is commutative for the checked identity")
add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}

#proof(add_comm)
prove_add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}
"#;
        let (module, interner, mut diags) = analyze(src);
        let (laws, proofs) = collect_verify_items(&module, &interner);
        assert_eq!(laws.len(), 1);
        assert_eq!(proofs.len(), 1);
        assert!(laws[0].is_law());
        let report = verify_module(&module, &interner, &mut diags, &VerifyOptions::default());
        assert!(report.passed(), "{:?}", report.results);
        assert_eq!(report.results[0].tier, Some(DischargeTier::Comptime));
    }

    #[test]
    fn e0503_on_todo_proof() {
        let src = r#"
#law("never discharged")
never(x: i64) {
    #assert(x == x)
}

#proof(never)
bad_proof(x: i64) {
    #todo
}
"#;
        let (module, interner, mut diags) = analyze(src);
        let report = verify_module(&module, &interner, &mut diags, &VerifyOptions::default());
        assert!(!report.passed());
        assert_eq!(report.results[0].code, Some(E0503));
    }

    #[test]
    fn e0502_on_broken_law() {
        let src = r#"
#law("broken: claims 1 == 2")
broken(x: i64) {
    #assert(1 == 2)
}
"#;
        let (module, interner, mut diags) = analyze(src);
        let report = verify_module(&module, &interner, &mut diags, &VerifyOptions::default());
        assert!(!report.passed());
        assert_eq!(report.results[0].code, Some(E0502));
    }

    #[test]
    fn real_body_uses_module_string_index() {
        let src = r#"
+HEX = "0123456789ABCDEF"
+hex_val(c: str) : i64 {
    found = -1
    for 0..16 {
        if found < 0 && HEX[it] == c { found = it }
    }
    found
}

#law("hex digit A is 10")
hex_a() {
    #assert(hex_val("A") == 10)
}
"#;
        let (module, interner, mut diags) = analyze(src);
        let report = verify_module(&module, &interner, &mut diags, &VerifyOptions::default());
        assert!(report.passed(), "{:?}", report.results);
    }

    #[test]
    fn sidecar_helpers_detect_and_resolve() {
        assert!(is_verify_sidecar(Path::new("ledger.law.sr")));
        assert!(is_verify_sidecar(Path::new("ledger.proof.sr")));
        assert!(!is_verify_sidecar(Path::new("ledger.sr")));
        assert!(!is_verify_sidecar(Path::new("main.sr")));

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("super-sidecar-{stamp}"));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.join("Pod.toml"), "[pod]\nname=\"t\"\n").unwrap();
        let main = src.join("main.sr");
        let law = src.join("ledger.law.sr");
        std::fs::write(&main, "+main() {}\n").unwrap();
        std::fs::write(&law, "#law(\"x\") x() { #assert(1==1) }\n").unwrap();
        let primary = resolve_sidecar_primary(&law).expect("primary");
        assert_eq!(primary, main);

        let sibling_mod = src.join("reduce.sr");
        let sibling_law = src.join("reduce.law.sr");
        std::fs::write(&sibling_mod, "+add(a: i64, b: i64) => a + b\n").unwrap();
        std::fs::write(&sibling_law, "#law(\"y\") y() { #assert(1==1) }\n").unwrap();
        assert_eq!(
            resolve_sidecar_primary(&sibling_law).expect("sibling"),
            sibling_mod
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
