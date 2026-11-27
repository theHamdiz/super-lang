//! Whole-program monomorphizing lowering of Super bytecode to **unboxed C23**.
//!
//! Instead of interpreting a `SuperVal locals[]` / `stack[512]` frame, every function is
//! lowered *per concrete argument signature* ("instance") with a flow-sensitive abstract
//! interpreter over the bytecode:
//!
//! * scalars become `int64_t` / `double` / `bool` C locals, strings `const char *`;
//! * dense arrays become `T *` + length pairs (`T *restrict` for `@` borrows) with
//!   bounds-checked inline loads/stores (or raw indexing for BCE-proven `*Unchecked` ops);
//! * `@` scalar parameters become `T *restrict` out-parameters;
//! * flat value structs become `typedef struct sup_s{id}_t` C values (shape inferred from
//!   `MakeStruct` sites): `GetField` / `SetField` are `.member` loads / stores, structs pass
//!   and return by value, and `@` struct parameters are `T *restrict` copy-in / write-back;
//! * non-escaping `[v; N]` literals become C stack arrays (`T a[N]`), everything else
//!   lives on the bump arena (non-escaping, dynamic size) or the heap (escaping);
//! * `for`/`par` range and array loops become real C `for` loops, with
//!   `#pragma omp parallel for` emitted when a body is provably race-free
//!   (affine disjoint stores, private scalars, `+` reductions);
//! * module-level constants fold into C literals; mutable globals become typed statics.
//!
//! Anything the analysis cannot prove monomorphic (heterogeneous values, sum types, growable
//! boxed arrays, error values, …) is left on the boxed `SuperVal` path: the caller of such a
//! function sees an ordinary boxed call whose result is "unknown" (`Top`).

use crate::bytecode::{CTag, Chunk, Op};
use crate::value::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use super_core::Interner;

type Res<T> = Result<T, String>;

const UNKNOWN_ROOT: u32 = u32::MAX;
const ALLOC_ROOT: u32 = 1 << 20;
const GLOBAL_ROOT: u32 = 1 << 24;
const STACK_ARRAY_LIMIT_BYTES: usize = 16 * 1024;
/// Large stdlib-using `main` bodies (data-pipeline / crypto suites) need more than a
/// handful of join-widen rounds before the abstract state stabilizes. 16 was
/// enough for microbenchmarks but forced a boxed stack-VM fallback on real
/// programs — raise the ceiling; monotone joins still terminate earlier.
const MAX_PASSES: usize = 128;
const MAX_ROUNDS: usize = 96;
const AUTO_PAR_MIN_TRIP: i64 = 32_768;
/// Explicit `par` is a hint: below this trip count OpenMP fork/join dominates
/// useful work on typical host runtimes (measured on 256-row mandelbrot etc.).
/// Explicit `par` pays off once outer trips cover typical row/chunk work
/// (e.g. 256×256 mandelbrot rows). Keep well below auto-par's huge gate.
const EXPLICIT_PAR_MIN_TRIP: i64 = 64;

// ---------------------------------------------------------------------------------------
// Types, constants, affine forms
// ---------------------------------------------------------------------------------------

/// Abstract value class tracked by the lowering (the lattice is flat with `I64 ⊔ F64 = F64`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub(super) enum Ty {
    /// No information yet (unassigned / pending callee result).
    #[default]
    Bot,
    I64,
    F64,
    Bool,
    Str,
    ArrI64,
    ArrF64,
    ArrStr,
    /// Dense `bool` array (`bool *` + length).
    ArrBool,
    /// Dense `u8` byte buffer (`uint8_t *` + length) — peer/C `[N]u8` / `uint8_t[N]` parity.
    /// Indexed loads widen to `i64`; stores truncate with `& 255`.
    ArrU8,
    /// Growable array of dense `i64` arrays (`sup_ai_t` = pointer + length pairs).
    ArrAI,
    /// Growable array of dense `f64` arrays (`sup_af_t` = pointer + length pairs).
    ArrAF,
    /// Growable array of flat value structs (`sup_s{id}_t *` + length).
    ArrStruct(u32),
    /// The statically-empty `[]` literal: element class not yet known. It owns no C
    /// variables; the first `push` (or a merge with a typed array) fixes the class.
    ArrEmpty,
    Unit,
    Range,
    /// Flat value struct `sup_s{id}_t`; the shape lives in [`Plan::structs`].
    Struct(u32),
    /// Result of a boxed call kept as a `SuperVal` C variable: it can be passed to boxed
    /// code, probed by `or` / `or err`, printed, and is unboxed where it meets a typed value.
    Dyn,
    /// Conflicting / dynamic — any use forces the boxed fallback.
    Top,
}

impl Ty {
    fn join(self, o: Ty) -> Ty {
        use Ty::*;
        match (self, o) {
            (Bot, x) | (x, Bot) => x,
            (a, b) if a == b => a,
            (I64, F64) | (F64, I64) => F64,
            (ArrEmpty, a) | (a, ArrEmpty) if a.is_arr() => a,
            (Dyn, t) | (t, Dyn) if t.from_dyn() => t,
            _ => Top,
        }
    }
    /// Classes a boxed `SuperVal` can be converted into at a merge (`sup_unbox_*`).
    fn from_dyn(self) -> bool {
        self.is_scalar()
            || matches!(self, Ty::Str | Ty::Struct(_) | Ty::ArrStruct(_))
            || self.is_flat_arr()
    }
    fn is_scalar(self) -> bool {
        matches!(self, Ty::I64 | Ty::F64 | Ty::Bool)
    }
    fn is_arr(self) -> bool {
        matches!(
            self,
            Ty::ArrI64
                | Ty::ArrF64
                | Ty::ArrStr
                | Ty::ArrBool
                | Ty::ArrU8
                | Ty::ArrAI
                | Ty::ArrAF
                | Ty::ArrStruct(_)
        )
    }
    /// Arrays of dense arrays, stored as inline (pointer, length) element pairs.
    fn is_nested_arr(self) -> bool {
        matches!(self, Ty::ArrAI | Ty::ArrAF)
    }
    /// Arrays a `for` walks element by element (`it` is a scalar, string or struct value).
    fn is_iter_arr(self) -> bool {
        self.is_flat_arr() || self.is_nested_arr() || matches!(self, Ty::ArrStruct(_))
    }
    /// Dense arrays whose elements are scalars / strings (loopable, storable by index).
    fn is_flat_arr(self) -> bool {
        matches!(
            self,
            Ty::ArrI64 | Ty::ArrF64 | Ty::ArrStr | Ty::ArrBool | Ty::ArrU8
        )
    }
    /// Operand classes that participate in arithmetic.
    fn is_num(self) -> bool {
        self.is_scalar()
    }
    fn suffix(self) -> &'static str {
        match self {
            Ty::I64 => "i",
            Ty::F64 => "f",
            Ty::Bool => "b",
            Ty::Str => "s",
            Ty::ArrI64 => "ai",
            Ty::ArrF64 => "af",
            Ty::ArrStr => "as",
            Ty::ArrBool => "ab",
            Ty::ArrU8 => "au",
            Ty::ArrAI => "aa",
            Ty::ArrAF => "aaf",
            Ty::Range => "r",
            Ty::Dyn => "d",
            _ => "x",
        }
    }
    fn is_struct(self) -> bool {
        matches!(self, Ty::Struct(_))
    }
    /// Value classes stored inline in a C struct field (`T name;`, arrays as ptr + len).
    /// A still-empty `[]` field owns no C members until a push / store widens it.
    fn is_field(self) -> bool {
        self.is_scalar()
            || self == Ty::Str
            || self.is_arr()
            || self.is_struct()
            || self == Ty::ArrEmpty
    }
    fn cty(self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Ty::I64 | Ty::Range => "int64_t ",
            Ty::F64 => "double ",
            Ty::Bool => "bool ",
            Ty::Str => "const char *",
            Ty::ArrI64 => "int64_t *",
            Ty::ArrF64 => "double *",
            Ty::ArrStr => "const char **",
            Ty::ArrBool => "bool *",
            Ty::ArrU8 => "uint8_t *",
            Ty::ArrAI => "sup_ai_t *",
            Ty::ArrAF => "sup_af_t *",
            Ty::ArrStruct(id) => return Cow::Owned(format!("{} *", struct_c_name(id))),
            Ty::Struct(id) => return Cow::Owned(format!("{} ", struct_c_name(id))),
            Ty::Dyn => "SuperVal ",
            _ => "void ",
        })
    }
    fn zero(self) -> &'static str {
        match self {
            Ty::F64 => "0.0",
            Ty::Dyn => "SUPER_NULL",
            Ty::Bool => "false",
            t if t == Ty::Str || t.is_arr() => "nullptr",
            Ty::Struct(_) => "{}",
            _ => "0",
        }
    }
    fn elem(self) -> Option<Ty> {
        match self {
            Ty::ArrI64 => Some(Ty::I64),
            Ty::ArrF64 => Some(Ty::F64),
            Ty::ArrStr => Some(Ty::Str),
            Ty::ArrBool => Some(Ty::Bool),
            // Logical element is i64 (peer `u8` widened); storage is `uint8_t`.
            Ty::ArrU8 => Some(Ty::I64),
            Ty::ArrAI => Some(Ty::ArrI64),
            Ty::ArrAF => Some(Ty::ArrF64),
            Ty::ArrStruct(id) => Some(Ty::Struct(id)),
            _ => None,
        }
    }
    /// C element type of a growable array (no trailing space).
    fn c_elem(self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            Ty::ArrI64 => "int64_t",
            Ty::ArrF64 => "double",
            Ty::ArrStr => "const char *",
            Ty::ArrBool => "bool",
            Ty::ArrU8 => "uint8_t",
            Ty::ArrStruct(id) => return Cow::Owned(struct_c_name(id)),
            Ty::ArrAF => "sup_af_t",
            _ => "sup_ai_t",
        })
    }
    /// Growable array class that holds `elem` values (`push` target for `[]`).
    fn grow_of(elem: Ty) -> Option<Ty> {
        match elem {
            Ty::ArrI64 => Some(Ty::ArrAI),
            Ty::ArrF64 => Some(Ty::ArrAF),
            e => Ty::arr_of(e),
        }
    }
    fn arr_of(elem: Ty) -> Option<Ty> {
        match elem {
            Ty::I64 => Some(Ty::ArrI64),
            Ty::F64 => Some(Ty::ArrF64),
            Ty::Str => Some(Ty::ArrStr),
            Ty::Bool => Some(Ty::ArrBool),
            Ty::Struct(id) => Some(Ty::ArrStruct(id)),
            _ => None,
        }
    }
    fn from_tag(t: CTag) -> Option<Ty> {
        match t {
            CTag::I64 => Some(Ty::I64),
            CTag::F64 => Some(Ty::F64),
            CTag::Bool => Some(Ty::Bool),
            CTag::Str => Some(Ty::Str),
            CTag::ArrI64 => Some(Ty::ArrI64),
            CTag::ArrF64 => Some(Ty::ArrF64),
            CTag::Unit => Some(Ty::Unit),
            CTag::Dyn => None,
        }
    }
    /// Class that has a C representation (variables / parameters).
    fn has_repr(self) -> bool {
        !matches!(self, Ty::Unit | Ty::Bot | Ty::Top | Ty::ArrEmpty)
    }
}

/// Compile-time constant (tracked through locals, globals and arithmetic).
#[derive(Clone, Copy, Debug)]
enum Cst {
    I(i64),
    F(f64),
    B(bool),
}

fn same_cst(a: Option<Cst>, b: Option<Cst>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Cst::I(x)), Some(Cst::I(y))) => x == y,
        (Some(Cst::F(x)), Some(Cst::F(y))) => x.to_bits() == y.to_bits(),
        (Some(Cst::B(x)), Some(Cst::B(y))) => x == y,
        _ => false,
    }
}

impl Cst {
    fn ty(self) -> Ty {
        match self {
            Cst::I(_) => Ty::I64,
            Cst::F(_) => Ty::F64,
            Cst::B(_) => Ty::Bool,
        }
    }
    fn lit(self) -> String {
        match self {
            Cst::I(n) => lit_i(n),
            Cst::F(f) => lit_f(f),
            Cst::B(b) => (if b { "true" } else { "false" }).to_string(),
        }
    }
    fn as_f(self) -> f64 {
        match self {
            Cst::I(n) => n as f64,
            Cst::F(f) => f,
            Cst::B(b) => b as i64 as f64,
        }
    }
    fn as_i(self) -> i64 {
        match self {
            Cst::I(n) => n,
            Cst::F(f) => f as i64,
            Cst::B(b) => b as i64,
        }
    }
}

/// Placeholder for the value-less `return` of a `fail` / `none` path; the instance's return
/// class is only final after the pass loop, so [`lower_inst`] substitutes [`absent_return`].
const FAIL_RET: &str = "/*@FAILRET@*/";
/// Opens `SUP_KEEP{0,1}(…, ptr, len, T)` around an array result once the frame's arena use is known.
const KEEP_RET: &str = "/*@KEEP@*/(";

/// Return-class join for instances: `none` (`Unit`) beside a value class keeps the value
/// class and marks the instance optional (`T?`). Module code keeps the plain lattice.
fn ret_join(a: Ty, b: Ty, optional_ok: bool) -> (Ty, bool) {
    match (a, b) {
        (Ty::Unit, t) | (t, Ty::Unit) if optional_ok && t.has_repr() && t != Ty::Range => (t, true),
        _ => (a.join(b), false),
    }
}

/// `return` of a placeholder value for class `ret` (the caller reads `sup_err_msg` first).
fn absent_return(ret: Ty) -> String {
    match ret {
        t if t.is_arr() => "{ *ret_len = 0; /*@ARENA@*/return nullptr; }".into(),
        Ty::Str => "/*@ARENA@*/return \"\";".into(),
        Ty::Struct(id) => format!("/*@ARENA@*/return ({}){{}};", struct_c_name(id)),
        t if t.has_repr() && t != Ty::Range => format!("/*@ARENA@*/return {};", t.zero()),
        _ => "/*@ARENA@*/return;".into(),
    }
}

fn lit_i(n: i64) -> String {
    if n == i64::MIN {
        "(-9223372036854775807LL-1)".into()
    } else if n < 0 {
        format!("({n}LL)")
    } else {
        format!("{n}LL")
    }
}

fn lit_f(f: f64) -> String {
    if f.is_nan() {
        "NAN".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "INFINITY".into()
        } else {
            "(-INFINITY)".into()
        }
    } else {
        let s = format!("{f:?}");
        if f.is_sign_negative() {
            format!("({s})")
        } else {
            s
        }
    }
}

/// Affine form `Σ coef·loopvar(region) + off` used to prove disjoint per-iteration stores.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Lin {
    terms: Vec<(u32, i64)>,
    off: i64,
}

impl Lin {
    fn constant(k: i64) -> Lin {
        Lin {
            terms: vec![],
            off: k,
        }
    }
    fn var(region: u32) -> Lin {
        Lin {
            terms: vec![(region, 1)],
            off: 0,
        }
    }
    fn add(&self, o: &Lin, sign: i64) -> Lin {
        let mut terms = self.terms.clone();
        for &(r, c) in &o.terms {
            let c = c.wrapping_mul(sign);
            if let Some(t) = terms.iter_mut().find(|t| t.0 == r) {
                t.1 = t.1.wrapping_add(c);
            } else {
                terms.push((r, c));
            }
        }
        terms.retain(|t| t.1 != 0);
        terms.sort_unstable();
        Lin {
            terms,
            off: self.off.wrapping_add(o.off.wrapping_mul(sign)),
        }
    }
    fn scale(&self, k: i64) -> Lin {
        let mut terms: Vec<(u32, i64)> = self
            .terms
            .iter()
            .map(|&(r, c)| (r, c.wrapping_mul(k)))
            .collect();
        terms.retain(|t| t.1 != 0);
        Lin {
            terms,
            off: self.off.wrapping_mul(k),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Abstract values and flow state
// ---------------------------------------------------------------------------------------

/// One abstract stack value: a C expression plus what we statically know about it.
#[derive(Clone, Debug)]
struct V {
    ty: Ty,
    /// Scalar / string expression, array pointer expression, or range lower bound.
    a: String,
    /// Array length expression, or range upper bound.
    b: String,
    cst: Option<Cst>,
    /// Local slots whose C variables this (lazy) expression reads.
    deps: Vec<u16>,
    lin: Option<Lin>,
    /// `Some(s)` when the expression is `local[s] + e` with `e` independent of `s`.
    rmw: Option<u16>,
    /// Alias class of an array value.
    root: u32,
    /// Capacity expression of a *freshly grown, exclusively owned* array (`push` result).
    /// Empty means "no spare capacity" (capacity == length), which is always safe.
    c: String,
    /// Constant-pool index when this value is a string literal. Survives spills and merges
    /// (where both arrivals agree), so struct-literal keys stay known across `if` values.
    sconst: Option<u16>,
    /// Result of a fallible (`T!`) / optional (`T?`) unboxed call: the C `const char *`
    /// holding the captured `sup_err_msg` (`nullptr` = present, `sup_none_msg` = `none`).
    ferr: String,
    /// The captured status may be `none` (not only a `fail` message).
    fnone: bool,
    /// Constant `IsAbsent` / `__is_err` outcome: the following branch folds.
    probe: bool,
    /// Base of an [`append_run`] on this local slot: the read did not disown its buffer.
    app: Option<u16>,
    /// Array result straight from a call to this unboxed instance.
    from_call: Option<usize>,
    /// When set, this `Str` is `chr(code)` — append can grow one ASCII byte without a temp.
    chr_code: Option<String>,
    /// `a` is a C array-element lvalue (`p[i]`) — SetField stores through it (peer `|*b|`).
    elem_lv: bool,
    /// C pointer expr of the struct field this array was loaded from (`w.bodies`); survives
    /// StoreLocal so a no-op write-back after in-place element mutation can be elided.
    arr_field: String,
}

impl V {
    fn new(ty: Ty, a: impl Into<String>) -> V {
        V {
            ty,
            a: a.into(),
            b: String::new(),
            cst: None,
            deps: Vec::new(),
            lin: None,
            rmw: None,
            root: 0,
            c: String::new(),
            sconst: None,
            ferr: String::new(),
            fnone: false,
            probe: false,
            app: None,
            from_call: None,
            chr_code: None,
            elem_lv: false,
            arr_field: String::new(),
        }
    }
    /// The statically-empty array literal.
    fn empty_arr() -> V {
        let mut v = V::new(Ty::ArrEmpty, "nullptr");
        v.b = "0LL".into();
        v
    }
    fn unit() -> V {
        V::new(Ty::Unit, "")
    }
    fn bot() -> V {
        V::new(Ty::Bot, "")
    }
    fn top() -> V {
        V::new(Ty::Top, "")
    }
    fn from_cst(c: Cst) -> V {
        let mut v = V::new(c.ty(), c.lit());
        v.cst = Some(c);
        v
    }
    fn arr(ty: Ty, ptr: impl Into<String>, len: impl Into<String>, root: u32) -> V {
        let mut v = V::new(ty, ptr);
        v.b = len.into();
        v.root = root;
        v
    }
}

fn lin_of(v: &V) -> Option<Lin> {
    if v.ty != Ty::I64 {
        return None;
    }
    if let Some(l) = &v.lin {
        return Some(l.clone());
    }
    match v.cst {
        Some(Cst::I(k)) => Some(Lin::constant(k)),
        _ => None,
    }
}

fn merge_deps(a: &[u16], b: &[u16]) -> Vec<u16> {
    let mut d = a.to_vec();
    for x in b {
        if !d.contains(x) {
            d.push(*x);
        }
    }
    d
}

#[derive(Clone, Debug, PartialEq)]
enum IterKind {
    Range {
        lo_cst: Option<i64>,
        hi_cst: Option<i64>,
    },
    Arr(Ty),
}

#[derive(Clone, Debug, PartialEq)]
struct IterTy {
    head: usize,
    kind: IterKind,
}

/// Merge iterator stacks at a CFG join. Soft empty-array walks (ArrEmpty → Range)
/// may upgrade to a real array iterator on a later analysis pass.
fn join_iters(a: &[IterTy], b: &[IterTy]) -> Res<Vec<IterTy>> {
    if a == b {
        return Ok(a.to_vec());
    }
    if a.len() != b.len() {
        return Err("iterator nesting mismatch at merge".into());
    }
    let mut out = Vec::with_capacity(a.len());
    for (x, y) in a.iter().zip(b) {
        if x.head != y.head {
            return Err("iterator nesting mismatch at merge".into());
        }
        let kind = match (&x.kind, &y.kind) {
            (k1, k2) if k1 == k2 => k1.clone(),
            (IterKind::Range { .. }, IterKind::Arr(t))
            | (IterKind::Arr(t), IterKind::Range { .. }) => IterKind::Arr(*t),
            (IterKind::Arr(t1), IterKind::Arr(t2)) => {
                let j = t1.join(*t2);
                if j.is_arr() {
                    IterKind::Arr(j)
                } else {
                    return Err("iterator element class conflict at merge".into());
                }
            }
            (
                IterKind::Range {
                    lo_cst: la,
                    hi_cst: ha,
                },
                IterKind::Range {
                    lo_cst: lb,
                    hi_cst: hb,
                },
            ) => IterKind::Range {
                lo_cst: if la == lb { *la } else { None },
                hi_cst: if ha == hb { *ha } else { None },
            },
        };
        out.push(IterTy { head: x.head, kind });
    }
    Ok(out)
}

/// Joinable abstract machine state at a program point.
#[derive(Clone, Debug)]
struct State {
    locals: Vec<Ty>,
    lcst: Vec<Option<Cst>>,
    llin: Vec<Option<Lin>>,
    lroot: Vec<u32>,
    stack: Vec<Ty>,
    /// Per stack slot: string-literal constant index shared by every arrival, if any.
    skey: Vec<Option<u16>>,
    iters: Vec<IterTy>,
}

impl State {
    fn new(nlocals: usize) -> State {
        State {
            locals: vec![Ty::Bot; nlocals],
            lcst: vec![None; nlocals],
            llin: vec![None; nlocals],
            lroot: vec![0; nlocals],
            stack: Vec::new(),
            skey: Vec::new(),
            iters: Vec::new(),
        }
    }

    fn join(&self, o: &State) -> Res<State> {
        if self.stack.len() != o.stack.len() {
            return Err(format!(
                "stack depth mismatch at merge ({} vs {})",
                self.stack.len(),
                o.stack.len()
            ));
        }
        let joined_iters = join_iters(&self.iters, &o.iters)?;
        let mut r = self.clone();
        r.iters = joined_iters;
        for i in 0..self.locals.len() {
            let (a, b) = (self.locals[i], o.locals[i]);
            r.locals[i] = a.join(b);
            if a == Ty::Bot {
                r.lcst[i] = o.lcst[i];
                r.llin[i] = o.llin[i].clone();
                r.lroot[i] = o.lroot[i];
            } else if b == Ty::Bot {
                // keep self's facts
            } else {
                if !same_cst(self.lcst[i], o.lcst[i]) {
                    r.lcst[i] = None;
                }
                if self.llin[i] != o.llin[i] {
                    r.llin[i] = None;
                }
                if self.lroot[i] != o.lroot[i] {
                    r.lroot[i] = UNKNOWN_ROOT;
                }
            }
        }
        for i in 0..self.stack.len() {
            r.stack[i] = self.stack[i].join(o.stack[i]);
            if self.skey.get(i) != o.skey.get(i)
                && let Some(k) = r.skey.get_mut(i)
            {
                *k = None;
            }
        }
        Ok(r)
    }

    fn same(&self, o: &State) -> bool {
        self.locals == o.locals
            && self.stack == o.stack
            && self.skey == o.skey
            && self.iters == o.iters
            && self.lroot == o.lroot
            && self.llin == o.llin
            && self.lcst.iter().zip(&o.lcst).all(|(a, b)| same_cst(*a, *b))
    }
}

// ---------------------------------------------------------------------------------------
// Instances & program-wide facts
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct GInfo {
    ty: Ty,
    cst: Option<Cst>,
    mirrored_store_seen: bool,
}

/// Heap-pointer effects observed in one lowered instance body (its own ops only); they are
/// closed over the call graph by [`region_modes`] to find frames whose allocations die with them.
#[derive(Clone, Debug, Default)]
struct PtrFx {
    /// A pointer may reach memory outside every frame (global, boxed code, host).
    leak: bool,
    /// Reads a global that can hold pointers in mutable storage.
    gread: bool,
    /// Stores a pointer into an array of unknown origin (struct field, callee result).
    ustore: bool,
    /// Stores a pointer into an array this frame allocated.
    astore: bool,
    /// Stores a pointer into an array parameter.
    pstore: bool,
    /// Replaces a pointer held by an `@` parameter.
    mutw: bool,
    /// Callees whose `@` write-backs land in this frame's own `@` parameters.
    mutw_calls: BTreeSet<usize>,
    /// Callees whose array result is stored into an `@` parameter: a pointer replacement
    /// only when the callee (transitively) allocates.
    mutw_if_alloc: BTreeSet<usize>,
}

/// How a frame's region bracket treats its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegionMode {
    /// Pointer-free result: everything allocated during the call is released on return.
    Scoped,
    /// String result: it is compacted down to the frame mark, everything else is released.
    KeepStr,
}

#[derive(Clone, Debug)]
struct Inst {
    /// `None` = module top-level code.
    fi: Option<usize>,
    params: Vec<Ty>,
    ret: Ty,
    failed: Option<String>,
    /// Lowered statement text (without declarations / signature).
    body: String,
    decls: String,
    prologue: String,
    callees: BTreeSet<usize>,
    dyn_calls: BTreeSet<usize>,
    saw_bot: bool,
    impure: bool,
    writes_mem: bool,
    uses_arena: bool,
    leaf_pure: bool,
    has_loops: bool,
    nops: usize,
    name: String,
    param_vars: HashSet<String>,
    sig_params: String,
    /// Module top-level forwarded a boxed `+main` (or similar) `SuperVal` as the process exit.
    exit_super_val: bool,
    /// Some path `fail`s: the message is left in `sup_err_msg` for the caller.
    fails: bool,
    /// Some path returns `none` (`T?`): `sup_err_msg = sup_none_msg`.
    nones: bool,
    fx: PtrFx,
}

/// One field of an unboxed value struct.
#[derive(Clone, Debug, PartialEq)]
struct SField {
    /// Super field name (the boxed object key).
    name: String,
    /// C member name (arrays also own the length member `{member}__n`).
    member: String,
    ty: Ty,
}

/// Shape of a value struct inferred from its `MakeStruct` sites. Field classes widen
/// monotonically (`I64 ⊔ F64 = F64`) across sites; anything else is a conflict.
#[derive(Clone, Debug)]
struct SDef {
    label: Option<String>,
    fields: Vec<SField>,
}

impl SDef {
    fn field(&self, name: &str) -> Option<&SField> {
        self.fields.iter().find(|f| f.name == name)
    }
}

struct Plan<'a> {
    chunk: &'a Chunk,
    interner: &'a Interner,
    insts: Vec<Inst>,
    keys: HashMap<(Option<usize>, Vec<Ty>), usize>,
    globals: BTreeMap<u32, GInfo>,
    store_counts: HashMap<u32, usize>,
    banned: HashSet<u32>,
    changed: bool,
    structs: Vec<SDef>,
    /// `(label, sorted field names)` → index into `structs`.
    struct_keys: HashMap<(Option<String>, Vec<String>), u32>,
    /// Set once the fixpoint is otherwise stable: arrays still statically empty (`[]` never
    /// widened anywhere) are always empty at run time, so an index read is out of range and
    /// yields the zero element like every other out-of-range unboxed load.
    empty_reads_zero: bool,
}

impl<'a> Plan<'a> {
    fn new(chunk: &'a Chunk, interner: &'a Interner, banned: HashSet<u32>) -> Plan<'a> {
        let mut store_counts: HashMap<u32, usize> = HashMap::new();
        for op in &chunk.ops {
            if let Op::StoreGlobal(s) = op {
                *store_counts.entry(s.index()).or_default() += 1;
            }
        }
        Plan {
            chunk,
            interner,
            insts: Vec::new(),
            keys: HashMap::new(),
            globals: BTreeMap::new(),
            store_counts,
            banned,
            changed: false,
            structs: Vec::new(),
            struct_keys: HashMap::new(),
            empty_reads_zero: false,
        }
    }

    /// Struct class for a literal with `label` and `(field, class)` pairs in source order.
    /// Re-registering a known shape joins field classes; a widening marks the plan dirty.
    fn intern_struct(&mut self, label: Option<String>, fields: &[(String, Ty)]) -> Res<u32> {
        let mut sorted: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
        sorted.sort_unstable();
        let key = (label.clone(), sorted);
        if let Some(&id) = self.struct_keys.get(&key) {
            for (name, ty) in fields {
                self.widen_field(id, name, *ty)?;
            }
            return Ok(id);
        }
        let mut out = Vec::with_capacity(fields.len());
        for (name, ty) in fields {
            if !ty.is_field() {
                return Err(format!("struct field `{name}` has class {ty:?}"));
            }
            out.push(SField {
                name: name.clone(),
                member: c_member(name),
                ty: *ty,
            });
        }
        let mut members: HashSet<String> = HashSet::new();
        for f in &out {
            // `[]` fields reserve their length member: they widen into arrays later.
            let has_len = f.ty.is_arr() || f.ty == Ty::ArrEmpty;
            if !members.insert(f.member.clone())
                || (has_len && !members.insert(len_member(&f.member)))
            {
                return Err(format!(
                    "struct field `{}` collides with another C member",
                    f.name
                ));
            }
        }
        let id = self.structs.len() as u32;
        self.structs.push(SDef { label, fields: out });
        self.struct_keys.insert(key, id);
        self.changed = true;
        Ok(id)
    }

    /// Join `ty` into field `name` of struct `id`.
    fn widen_field(&mut self, id: u32, name: &str, ty: Ty) -> Res<bool> {
        if ty == Ty::Dyn {
            return Err(format!("struct field `{name}` holds a boxed value"));
        }
        let def = &mut self.structs[id as usize];
        let Some(f) = def.fields.iter_mut().find(|f| f.name == name) else {
            return Err(format!("struct has no field `{name}`"));
        };
        let j = f.ty.join(ty);
        if !j.is_field() {
            return Err(format!(
                "struct field `{name}` holds conflicting classes ({:?} vs {ty:?})",
                f.ty
            ));
        }
        if j == f.ty {
            return Ok(false);
        }
        f.ty = j;
        self.changed = true;
        Ok(true)
    }

    /// Does a value of class `ty` (transitively) own array storage?
    fn holds_arr(&self, ty: Ty) -> bool {
        match ty {
            t if t.is_arr() => true,
            Ty::Struct(id) => self.structs[id as usize]
                .fields
                .iter()
                .any(|f| self.holds_arr(f.ty)),
            _ => false,
        }
    }

    /// Does a value of class `ty` (transitively) carry a heap pointer (string or array)?
    fn holds_ptr(&self, ty: Ty) -> bool {
        self.holds_ptr_in(ty, &mut Vec::new())
    }

    fn holds_ptr_in(&self, ty: Ty, seen: &mut Vec<u32>) -> bool {
        match ty {
            Ty::Str | Ty::Dyn | Ty::Top => true,
            t if t.is_arr() => true,
            Ty::Struct(id) => {
                if seen.contains(&id) {
                    return false;
                }
                seen.push(id);
                self.structs[id as usize]
                    .fields
                    .iter()
                    .any(|f| self.holds_ptr_in(f.ty, seen))
            }
            _ => false,
        }
    }

    /// Can memory reachable from a value of class `ty` *receive* a pointer — does it reach an
    /// array whose elements hold pointers? (A by-value struct copy cannot; its arrays can.)
    fn ptr_capable(&self, ty: Ty) -> bool {
        self.ptr_capable_in(ty, &mut Vec::new())
    }

    fn ptr_capable_in(&self, ty: Ty, seen: &mut Vec<u32>) -> bool {
        match ty {
            Ty::Dyn | Ty::Top => true,
            Ty::ArrStr | Ty::ArrAI | Ty::ArrAF => true,
            Ty::ArrStruct(id) | Ty::Struct(id) => {
                if seen.contains(&id) {
                    return false;
                }
                seen.push(id);
                let fields = &self.structs[id as usize].fields;
                if matches!(ty, Ty::ArrStruct(_)) {
                    fields.iter().any(|f| self.holds_ptr(f.ty))
                        || fields.iter().any(|f| self.ptr_capable_in(f.ty, seen))
                } else {
                    fields.iter().any(|f| self.ptr_capable_in(f.ty, seen))
                }
            }
            _ => false,
        }
    }

    /// Approximate C byte size of a value of class `ty` (stack-buffer budget only).
    fn size_of(&self, ty: Ty) -> usize {
        match ty {
            Ty::Struct(id) => self.structs[id as usize]
                .fields
                .iter()
                .map(|f| self.size_of(f.ty))
                .sum::<usize>()
                .max(8),
            t if t.is_arr() => 16,
            _ => 8,
        }
    }

    /// Element bytes of array class `arr` (stack-buffer budget only).
    fn elem_bytes(&self, arr: Ty) -> usize {
        if arr == Ty::ArrU8 {
            return 1;
        }
        arr.elem().map_or(8, |e| self.size_of(e))
    }

    /// Can `sup_box_s{id}` build a boxed object for this struct?
    fn boxable(&self, id: u32) -> bool {
        self.boxable_in(id, &mut Vec::new())
    }

    fn boxable_in(&self, id: u32, path: &mut Vec<u32>) -> bool {
        // A struct reachable from itself through array fields has no finite helper order.
        if path.contains(&id) {
            return false;
        }
        path.push(id);
        let ok = self.structs[id as usize].fields.iter().all(|f| match f.ty {
            Ty::Struct(c) | Ty::ArrStruct(c) => self.boxable_in(c, path),
            t => t.is_scalar() || t == Ty::Str || t == Ty::ArrEmpty || t.is_flat_arr(),
        });
        path.pop();
        ok
    }

    /// Can `sup_unbox_s{id}` rebuild this struct from a boxed object? Dense numeric arrays that
    /// are views of unboxed storage are adopted in place; other arrays are copied.
    fn unboxable(&self, id: u32) -> bool {
        self.unboxable_in(id, &mut Vec::new())
    }

    /// Does the struct (transitively) still hold a `[]` field a later pass may widen?
    fn unbox_pending(&self, id: u32) -> bool {
        fn walk(p: &Plan<'_>, id: u32, seen: &mut Vec<u32>) -> bool {
            if seen.contains(&id) {
                return false;
            }
            seen.push(id);
            p.structs[id as usize].fields.iter().any(|f| match f.ty {
                Ty::ArrEmpty => true,
                Ty::Struct(c) | Ty::ArrStruct(c) => walk(p, c, seen),
                _ => false,
            })
        }
        walk(self, id, &mut Vec::new())
    }

    fn unboxable_in(&self, id: u32, path: &mut Vec<u32>) -> bool {
        if path.contains(&id) {
            return false;
        }
        path.push(id);
        let ok = self.structs[id as usize].fields.iter().all(|f| match f.ty {
            Ty::Struct(c) | Ty::ArrStruct(c) => self.unboxable_in(c, path),
            // An `ArrEmpty` field has no storage to receive elements boxed code may have pushed.
            t => t.is_scalar() || t == Ty::Str || t.is_flat_arr(),
        });
        path.pop();
        ok
    }

    fn inst_name(&self, fi: Option<usize>) -> String {
        match fi {
            None => "module".into(),
            Some(fi) => {
                let f = &self.chunk.functions[fi];
                let owner = f
                    .owner
                    .map(|o| format!("{}_", sanitize(self.interner.resolve(o))))
                    .unwrap_or_default();
                format!("{owner}{}", sanitize(self.interner.resolve(f.name)))
            }
        }
    }

    fn add_inst(&mut self, fi: Option<usize>, params: Vec<Ty>) -> usize {
        let id = self.insts.len();
        let name = self.inst_name(fi);
        self.keys.insert((fi, params.clone()), id);
        self.insts.push(Inst {
            fi,
            params,
            ret: Ty::Bot,
            failed: None,
            body: String::new(),
            decls: String::new(),
            prologue: String::new(),
            callees: BTreeSet::new(),
            dyn_calls: BTreeSet::new(),
            saw_bot: false,
            impure: false,
            writes_mem: false,
            uses_arena: false,
            leaf_pure: false,
            has_loops: false,
            nops: 0,
            name,
            param_vars: HashSet::new(),
            sig_params: String::new(),
            exit_super_val: false,
            fails: false,
            nones: false,
            fx: PtrFx::default(),
        });
        self.changed = true;
        id
    }

    /// Request (creating on demand) the instance of `fi` for concrete argument classes.
    /// `None` when the signature cannot be unboxed (caller must use the boxed entry).
    fn request(&mut self, fi: usize, args: &[Ty]) -> Option<usize> {
        let f = &self.chunk.functions[fi];
        if f.arity as usize != args.len() {
            return None;
        }
        let mut params = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let declared = f.param_tags.get(i).copied().and_then(Ty::from_tag);
            let t = match (declared, *a) {
                (_, Ty::Top | Ty::Bot | Ty::Range | Ty::ArrEmpty | Ty::Dyn) => return None,
                (None, a) => a,
                (Some(d), a) if d == a => a,
                (Some(Ty::F64), Ty::I64) => Ty::F64,
                (Some(Ty::I64), Ty::Bool) => Ty::I64,
                _ => return None,
            };
            params.push(t);
        }
        let id = match self.keys.get(&(Some(fi), params.clone())) {
            Some(&id) => id,
            None => self.add_inst(Some(fi), params),
        };
        if self.insts[id].failed.is_some() {
            None
        } else {
            Some(id)
        }
    }

    fn global_name(&self, idx: u32, sym: super_core::SymId) -> String {
        format!("g_{}_{idx}", sanitize(self.interner.resolve(sym)))
    }
}

pub(super) fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "anon".into() } else { out }
}

// ---------------------------------------------------------------------------------------
// Lowering of one instance
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum BinK {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinK {
    fn is_cmp(self) -> bool {
        matches!(
            self,
            BinK::Eq | BinK::Ne | BinK::Lt | BinK::Le | BinK::Gt | BinK::Ge
        )
    }
    fn c_op(self) -> &'static str {
        match self {
            BinK::Add => "+",
            BinK::Sub => "-",
            BinK::Mul => "*",
            BinK::Div => "/",
            BinK::Rem => "%",
            BinK::Eq => "==",
            BinK::Ne => "!=",
            BinK::Lt => "<",
            BinK::Le => "<=",
            BinK::Gt => ">",
            BinK::Ge => ">=",
        }
    }
}

fn fold(k: BinK, a: Cst, b: Cst) -> Option<Cst> {
    let float = matches!(a, Cst::F(_)) || matches!(b, Cst::F(_));
    if float {
        let (x, y) = (a.as_f(), b.as_f());
        return Some(match k {
            BinK::Add => Cst::F(x + y),
            BinK::Sub => Cst::F(x - y),
            BinK::Mul => Cst::F(x * y),
            BinK::Div => Cst::F(x / y),
            BinK::Rem => Cst::F(x % y),
            BinK::Eq => Cst::B(x == y),
            BinK::Ne => Cst::B(x != y),
            BinK::Lt => Cst::B(x < y),
            BinK::Le => Cst::B(x <= y),
            BinK::Gt => Cst::B(x > y),
            BinK::Ge => Cst::B(x >= y),
        });
    }
    if let (Cst::B(x), Cst::B(y)) = (a, b) {
        match k {
            BinK::Eq => return Some(Cst::B(x == y)),
            BinK::Ne => return Some(Cst::B(x != y)),
            _ => {}
        }
    }
    let (x, y) = (a.as_i(), b.as_i());
    Some(match k {
        BinK::Add => Cst::I(x.wrapping_add(y)),
        BinK::Sub => Cst::I(x.wrapping_sub(y)),
        BinK::Mul => Cst::I(x.wrapping_mul(y)),
        BinK::Div => Cst::I(if y == 0 { 0 } else { x.wrapping_div(y) }),
        BinK::Rem => Cst::I(if y == 0 || y == -1 { 0 } else { x % y }),
        BinK::Eq => Cst::B(x == y),
        BinK::Ne => Cst::B(x != y),
        BinK::Lt => Cst::B(x < y),
        BinK::Le => Cst::B(x <= y),
        BinK::Gt => Cst::B(x > y),
        BinK::Ge => Cst::B(x >= y),
    })
}

struct LoopFrame {
    head: usize,
    close: usize,
    done: usize,
    explicit_par: bool,
    head_state: State,
    kind: IterKind,
    root: u32,
    written: BTreeSet<String>,
    slot_writes: HashMap<u16, u32>,
    slot_reads: HashMap<u16, u32>,
    slot_rmw: HashMap<u16, u32>,
    stores: Vec<(u32, Option<Lin>)>,
    loads: Vec<(u32, Option<Lin>)>,
    impure: bool,
    early_exit: bool,
    dyn_trip: bool,
    parent: Option<usize>,
    /// `PushIt` yielded a struct element lvalue (`it_a[i]`) — `it.f = …` stores
    /// in place; `IterSetIt` must not copy-back (merge clears `elem_alias`).
    it_elem_lv: bool,
}

#[derive(Default)]
struct Lowered {
    body: String,
    decls: String,
    prologue: String,
    ret: Ty,
    callees: BTreeSet<usize>,
    dyn_calls: BTreeSet<usize>,
    saw_bot: bool,
    impure: bool,
    writes_mem: bool,
    uses_arena: bool,
    leaf_pure: bool,
    has_loops: bool,
    nops: usize,
    param_vars: HashSet<String>,
    sig_params: String,
    exit_super_val: bool,
    fails: bool,
    nones: bool,
    fx: PtrFx,
}

struct Lw<'p, 'a> {
    plan: &'p mut Plan<'a>,
    chunk: &'a Chunk,
    interner: &'a Interner,
    inst: usize,
    fi: Option<usize>,
    start: usize,
    end: usize,
    nslots: usize,
    targets: HashSet<usize>,
    loop_pcs: Vec<bool>,
    target_state: HashMap<usize, State>,
    next_target: HashMap<usize, State>,
    out: String,
    vars: BTreeMap<String, (String, String)>,
    bufs: BTreeMap<String, (String, usize)>,
    st: State,
    vs: Vec<V>,
    live: bool,
    loops: Vec<LoopFrame>,
    omp: HashMap<usize, (Option<String>, Option<usize>)>,
    omp_notes: HashMap<usize, String>,
    par_next: bool,
    iter_root: HashMap<usize, u32>,
    ret_cur: Ty,
    is_module: bool,
    param_vars: HashSet<String>,
    mut_value_params: Vec<(usize, Ty)>,
    saw_bot: bool,
    callees: BTreeSet<usize>,
    dyn_calls: BTreeSet<usize>,
    impure: bool,
    writes_mem: bool,
    uses_arena: bool,
    has_calls: bool,
    has_loops: bool,
    tmp_id: usize,
    mut_param: Vec<bool>,
    /// Locals with an `x = x + …` run ([`append_run`]): string values may grow in place
    /// while every other read of the local disowns the buffer (`l{s}_so = nullptr`).
    append_slots: HashSet<u16>,
    /// Locals that alias a struct array element lvalue (`bodies[i]`) — LoadLocal
    /// restores `elem_lv` so `IndexSet` after `SetField` is a no-op (peer `|*b|`).
    elem_alias: HashMap<u16, V>,
    /// Locals holding a struct-field array (`w.bodies`) — LoadLocal restores `arr_field`
    /// so writing the same pointer back is a no-op.
    arr_field_alias: HashMap<u16, String>,
    /// Last boxed call's `SuperVal` in module code — becomes the process exit value.
    module_exit: Option<String>,
    fails: bool,
    nones: bool,
    fx: PtrFx,
}

impl<'p, 'a> Lw<'p, 'a> {
    fn new(plan: &'p mut Plan<'a>, inst: usize) -> Lw<'p, 'a> {
        let chunk = plan.chunk;
        let interner = plan.interner;
        let fi = plan.insts[inst].fi;
        let (start, end) = match fi {
            Some(fi) => fn_range(chunk, fi),
            None => module_range(chunk),
        };
        let mut nslots = fi
            .map(|fi| chunk.functions[fi].local_count as usize)
            .unwrap_or(0)
            .max(plan.insts[inst].params.len());
        let mut targets = HashSet::new();
        let mut loop_pcs = vec![false; end.saturating_sub(start) + 1];
        for pc in start..end {
            let t = match &chunk.ops[pc] {
                Op::Jump(t)
                | Op::JumpIfFalse(t)
                | Op::JumpIfTrue(t)
                | Op::JumpIfLt(t)
                | Op::JumpIfGe(t)
                | Op::JumpIfEq(t)
                | Op::JumpIfNe(t)
                | Op::IterNext(t) => Some(*t as usize),
                Op::LoopStep { body, .. } => Some(*body as usize),
                _ => None,
            };
            if let Some(t) = t {
                targets.insert(t);
                if t <= pc && t >= start {
                    for p in t..=pc {
                        loop_pcs[p - start] = true;
                    }
                }
            }
            match &chunk.ops[pc] {
                Op::LoadLocal(s) | Op::StoreLocal(s) => nslots = nslots.max(*s as usize + 1),
                Op::Move(a, b) => nslots = nslots.max(*a.max(b) as usize + 1),
                Op::CallMut(_, _, wb) => {
                    if let Some(list) = chunk.mut_writebacks.get(*wb as usize) {
                        for (s, _) in list {
                            nslots = nslots.max(*s as usize + 1);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut_param: Vec<bool> = fi
            .map(|fi| chunk.functions[fi].mut_params.clone())
            .unwrap_or_default();
        let append_slots = (start..end)
            .filter_map(|pc| match chunk.ops[pc] {
                Op::LoadLocal(s) if !mut_param.get(s as usize).copied().unwrap_or(false) => {
                    append_run(chunk, &targets, pc, end).map(|_| s)
                }
                _ => None,
            })
            .collect();
        Lw {
            chunk,
            interner,
            inst,
            fi,
            start,
            end,
            nslots,
            targets,
            loop_pcs,
            target_state: HashMap::new(),
            next_target: HashMap::new(),
            out: String::new(),
            vars: BTreeMap::new(),
            bufs: BTreeMap::new(),
            st: State::new(nslots),
            vs: Vec::new(),
            live: true,
            loops: Vec::new(),
            omp: HashMap::new(),
            omp_notes: HashMap::new(),
            par_next: false,
            iter_root: HashMap::new(),
            ret_cur: plan.insts[inst].ret,
            is_module: fi.is_none(),
            param_vars: HashSet::new(),
            mut_value_params: Vec::new(),
            saw_bot: false,
            callees: BTreeSet::new(),
            dyn_calls: BTreeSet::new(),
            impure: false,
            writes_mem: false,
            uses_arena: false,
            has_calls: false,
            has_loops: false,
            tmp_id: 0,
            mut_param,
            append_slots,
            elem_alias: HashMap::new(),
            arr_field_alias: HashMap::new(),
            plan,
            module_exit: None,
            fails: false,
            nones: false,
            fx: PtrFx::default(),
        }
    }

    // ----- small utilities --------------------------------------------------------------

    fn err<T>(&self, msg: impl Into<String>) -> Res<T> {
        Err(msg.into())
    }

    fn declare(&mut self, name: &str, cty: &str, init: &str) {
        self.vars
            .entry(name.to_string())
            .or_insert_with(|| (cty.to_string(), init.to_string()));
    }

    fn declare_pair(&mut self, base: &str, ty: Ty) -> (String, String) {
        let (a, b) = names(base, ty);
        match ty {
            Ty::Range => {
                self.declare(&a, "int64_t ", "0");
                self.declare(&b, "int64_t ", "0");
            }
            t if t.is_arr() => {
                self.declare(&a, &t.cty(), "nullptr");
                self.declare(&b, "int64_t ", "0");
            }
            t if t.is_scalar() || t == Ty::Str || t.is_struct() || t == Ty::Dyn => {
                self.declare(&a, &t.cty(), t.zero())
            }
            _ => {}
        }
        (a, b)
    }

    /// Declare the pointer / length / capacity variables of a growable array local.
    fn declare_arr_vars(&mut self, base: &str, ty: Ty) -> (String, String, String) {
        let (p, n) = self.declare_pair(base, ty);
        let c = cap_name(&p);
        self.declare(&c, "int64_t ", "0");
        (p, n, c)
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.tmp_id += 1;
        format!("{prefix}{}", self.tmp_id)
    }

    fn line(&mut self, s: &str) {
        self.out.push_str("    ");
        for _ in 0..self.loops.len() {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// `lhs = rhs;` that records the write for every enclosing loop region.
    fn assign(&mut self, lhs: &str, rhs: &str) {
        if lhs == rhs {
            return;
        }
        self.note_write_var(lhs);
        self.line(&format!("{lhs} = {rhs};"));
    }

    // ----- region event recording -------------------------------------------------------

    fn note_write_var(&mut self, name: &str) {
        for f in &mut self.loops {
            f.written.insert(name.to_string());
        }
    }
    fn note_slot_write(&mut self, s: u16, rmw: bool) {
        for f in &mut self.loops {
            *f.slot_writes.entry(s).or_default() += 1;
            if rmw {
                *f.slot_rmw.entry(s).or_default() += 1;
            }
        }
    }
    fn note_slot_read(&mut self, s: u16) {
        for f in &mut self.loops {
            *f.slot_reads.entry(s).or_default() += 1;
        }
    }
    fn note_store(&mut self, root: u32, lin: Option<Lin>) {
        self.writes_mem = true;
        for f in &mut self.loops {
            f.stores.push((root, lin.clone()));
        }
    }
    /// Classify a store of an `elem` value into the array with alias class `root`.
    fn note_ptr_store(&mut self, root: u32, elem: Ty) {
        if !self.plan.holds_ptr(elem) {
            return;
        }
        let nparams = self.plan.insts[self.inst].params.len() as u32;
        match root {
            r if (ALLOC_ROOT..GLOBAL_ROOT).contains(&r) => self.fx.astore = true,
            r if r >= GLOBAL_ROOT && r != UNKNOWN_ROOT => self.fx.leak = true,
            r if r >= 1 && r <= nparams => self.fx.pstore = true,
            _ => self.fx.ustore = true,
        }
    }
    /// Is local slot `s` one of this frame's `@` parameters?
    fn is_mut_param_slot(&self, s: usize) -> bool {
        self.fi.is_some()
            && s < self.plan.insts[self.inst].params.len()
            && self.mut_param.get(s).copied().unwrap_or(false)
    }
    fn note_load(&mut self, root: u32, lin: Option<Lin>) {
        for f in &mut self.loops {
            f.loads.push((root, lin.clone()));
        }
    }
    fn note_impure(&mut self) {
        self.impure = true;
        for f in &mut self.loops {
            f.impure = true;
        }
    }

    // ----- stack helpers ----------------------------------------------------------------

    fn push(&mut self, v: V) {
        self.vs.push(v);
    }

    fn pop(&mut self) -> Res<V> {
        self.vs
            .pop()
            .ok_or_else(|| "abstract stack underflow".to_string())
    }

    fn canon(&self, depth: usize, ty: Ty) -> (String, String) {
        names(&format!("s{depth}"), ty)
    }

    /// Spill stack entry `i` into its canonical C variable(s).
    fn mat(&mut self, i: usize) {
        let v = self.vs[i].clone();
        if !(v.ty.has_repr() || v.ty == Ty::Range) {
            return;
        }
        let (na, nb) = self.declare_pair(&format!("s{i}"), v.ty);
        let mut nv = v.clone();
        if v.a != na {
            self.assign(&na, &v.a);
        }
        nv.a = na;
        if (v.ty.is_arr() || v.ty == Ty::Range) && v.b != nb {
            self.assign(&nb, &v.b);
        }
        if v.ty.is_arr() || v.ty == Ty::Range {
            nv.b = nb;
        }
        nv.deps.clear();
        self.vs[i] = nv;
    }

    fn materialize_all(&mut self) {
        for i in 0..self.vs.len() {
            self.mat(i);
        }
    }

    fn materialize_dep(&mut self, slot: u16) {
        for i in 0..self.vs.len() {
            if self.vs[i].deps.contains(&slot) {
                self.mat(i);
            }
        }
    }

    /// Evaluate `expr` into the canonical variable of the *next* stack slot and return it.
    fn eager(&mut self, ty: Ty, expr: &str) -> V {
        let base = self.fresh("t");
        let (na, _) = self.declare_pair(&base, ty);
        self.assign(&na, expr);
        V::new(ty, na)
    }

    fn snapshot(&self) -> State {
        let mut s = self.st.clone();
        s.stack = self.vs.iter().map(|v| v.ty).collect();
        s.skey = self.vs.iter().map(|v| v.sconst).collect();
        s
    }

    fn set_state(&mut self, s: State) {
        self.vs.clear();
        // Keep `elem_alias` across CFG merges: aliases are `it_a[i]` / `arr[k]` expressions
        // that stay valid for the current loop index (clearing left `it` slots unassigned).
        for (d, ty) in s.stack.iter().enumerate() {
            if *ty == Ty::ArrEmpty {
                self.vs.push(V::empty_arr());
                continue;
            }
            let (a, b) = self.canon(d, *ty);
            let mut v = V::new(*ty, a);
            if ty.is_arr() || *ty == Ty::Range {
                v.b = b;
                v.root = UNKNOWN_ROOT;
            }
            v.sconst = s.skey.get(d).copied().flatten();
            self.vs.push(v);
        }
        self.st = s;
        self.st.stack.clear();
        self.st.skey.clear();
    }

    // ----- coercions --------------------------------------------------------------------

    /// Expression for `v` as numeric class `to` (`I64`, `F64`, `Bool`).
    fn conv(&self, v: &V, to: Ty) -> Res<String> {
        Ok(match (v.ty, to) {
            (a, b) if a == b => v.a.clone(),
            (Ty::I64, Ty::F64) | (Ty::Bool, Ty::F64) => format!("((double)({}))", v.a),
            (Ty::Bool, Ty::I64) => format!("((int64_t)({}))", v.a),
            (Ty::F64, Ty::I64) => format!("sup_to_i64({})", v.a),
            (Ty::I64, Ty::Bool) => format!("(({}) != 0)", v.a),
            (Ty::F64, Ty::Bool) => format!("(({}) != 0.0)", v.a),
            _ => return self.err(format!("cannot convert {:?} to {:?}", v.ty, to)),
        })
    }

    fn truthy(&self, v: &V) -> Res<String> {
        Ok(match v.ty {
            Ty::Bool => v.a.clone(),
            Ty::I64 => format!("(({}) != 0)", v.a),
            Ty::F64 => format!("(({}) != 0.0)", v.a),
            Ty::Str => format!("sup_truthy_str({})", v.a),
            t if t.is_arr() => format!("(({}) > 0)", v.b),
            Ty::ArrEmpty | Ty::Unit => "false".into(),
            Ty::Dyn => format!("(super_truthy({}) != 0)", v.a),
            t => return self.err(format!("truthiness of {t:?}")),
        })
    }

    /// Code converting the boxed `SuperVal` variable `src` into the C variable(s) of `base`
    /// at class `to` (a merge of a boxed result with a typed value).
    fn unbox_dyn_into(&mut self, src: &str, base: &str, to: Ty, local: bool) -> Res<String> {
        let unboxable = |plan: &Plan<'_>, t: Ty| match t {
            Ty::Struct(id) | Ty::ArrStruct(id) => plan.unboxable(id),
            _ => true,
        };
        if !unboxable(self.plan, to) {
            if let Ty::Struct(id) | Ty::ArrStruct(id) = to
                && self.plan.unbox_pending(id)
            {
                self.saw_bot = true;
                return Ok(String::new());
            }
            return self.err(format!("boxed value cannot be unboxed into {to:?}"));
        }
        let (a, b) = self.declare_pair(base, to);
        self.note_write_var(&a);
        let code = match to {
            Ty::I64 => format!("{a} = super_as_i({src}); "),
            Ty::F64 => format!("{a} = super_as_f({src}); "),
            Ty::Bool => format!("{a} = (super_truthy({src}) != 0); "),
            Ty::Str => format!("{a} = sup_unbox_str({src}); "),
            Ty::Struct(id) => format!("{a} = sup_unbox_s{id}({src}); "),
            t if t.is_arr() => {
                let f = match t {
                    Ty::ArrI64 => "sup_unbox_ai".to_string(),
                    Ty::ArrF64 => "sup_unbox_af".to_string(),
                    Ty::ArrStr => "sup_unbox_astr".to_string(),
                    Ty::ArrBool => "sup_unbox_ab".to_string(),
                    Ty::ArrU8 => "sup_unbox_au".to_string(),
                    Ty::ArrStruct(id) => format!("sup_unbox_as{id}"),
                    t => return self.err(format!("boxed value cannot be unboxed into {t:?}")),
                };
                self.note_write_var(&b);
                let mut code = format!("{a} = {f}({src}, &{b}); ");
                if local {
                    let c = cap_name(&a);
                    self.declare(&c, "int64_t ", "0");
                    self.note_write_var(&c);
                    let _ = write!(code, "{c} = {b}; ");
                }
                code
            }
            t => return self.err(format!("boxed value cannot be unboxed into {t:?}")),
        };
        Ok(code)
    }

    // ----- merging states at labels -----------------------------------------------------

    /// Emit conversions so `from` matches the (wider) `to` state at a merge point.
    fn coerce_to(&mut self, from: &State, to: &State) -> Res<(String, State)> {
        if from.stack.len() != to.stack.len() {
            return self.err(format!(
                "stack depth mismatch at merge ({} vs {})",
                from.stack.len(),
                to.stack.len()
            ));
        }
        let joined_iters = join_iters(&from.iters, &to.iters)?;
        let mut code = String::new();
        let mut cur = from.clone();
        cur.iters = joined_iters;
        for i in 0..from.locals.len() {
            if from.locals[i] == Ty::I64 && to.locals[i] == Ty::F64 {
                let (src, _) = names(&format!("l{i}"), Ty::I64);
                let (dst, _) = self.declare_pair(&format!("l{i}"), Ty::F64);
                self.declare_pair(&format!("l{i}"), Ty::I64);
                let _ = write!(code, "{dst} = (double){src}; ");
                self.note_write_var(&dst);
                cur.locals[i] = Ty::F64;
                cur.lcst[i] = None;
                cur.llin[i] = None;
            }
        }
        for i in 0..from.locals.len() {
            if from.locals[i] == Ty::ArrEmpty && to.locals[i].is_arr() {
                let (p, n, c) = self.declare_arr_vars(&format!("l{i}"), to.locals[i]);
                let _ = write!(code, "{p} = nullptr; {n} = 0; {c} = 0; ");
                for v in [&p, &n, &c] {
                    self.note_write_var(v);
                }
                cur.locals[i] = to.locals[i];
                cur.lcst[i] = None;
                cur.llin[i] = None;
            }
        }
        for i in 0..from.locals.len() {
            if from.locals[i] == Ty::Dyn && to.locals[i].from_dyn() {
                let (src, _) = names(&format!("l{i}"), Ty::Dyn);
                let c = self.unbox_dyn_into(&src, &format!("l{i}"), to.locals[i], true)?;
                code.push_str(&c);
                cur.locals[i] = to.locals[i];
                cur.lcst[i] = None;
                cur.llin[i] = None;
                cur.lroot[i] = UNKNOWN_ROOT;
            }
        }
        for d in 0..from.stack.len() {
            if from.stack[d] == Ty::Dyn && to.stack[d].from_dyn() {
                let (src, _) = names(&format!("s{d}"), Ty::Dyn);
                let c = self.unbox_dyn_into(&src, &format!("s{d}"), to.stack[d], false)?;
                code.push_str(&c);
                cur.stack[d] = to.stack[d];
            }
            if from.stack[d] == Ty::ArrEmpty && to.stack[d].is_arr() {
                let (p, n) = self.declare_pair(&format!("s{d}"), to.stack[d]);
                let _ = write!(code, "{p} = nullptr; {n} = 0; ");
                self.note_write_var(&p);
                self.note_write_var(&n);
                cur.stack[d] = to.stack[d];
            }
            if from.stack[d] == Ty::I64 && to.stack[d] == Ty::F64 {
                let (src, _) = names(&format!("s{d}"), Ty::I64);
                let (dst, _) = self.declare_pair(&format!("s{d}"), Ty::F64);
                self.declare_pair(&format!("s{d}"), Ty::I64);
                let _ = write!(code, "{dst} = (double){src}; ");
                self.note_write_var(&dst);
                cur.stack[d] = Ty::F64;
            }
        }
        Ok((code, cur))
    }

    fn record_arrival(&mut self, pc: usize, s: State) -> Res<()> {
        match self.next_target.get_mut(&pc) {
            Some(e) => {
                let j = e.join(&s)?;
                *e = j;
            }
            None => {
                self.next_target.insert(pc, s);
            }
        }
        Ok(())
    }

    /// Fallthrough arrival at `pc`: coerce the live state toward the merge state.
    fn arrive_fallthrough(&mut self, pc: usize) -> Res<()> {
        self.materialize_all();
        let snap = self.snapshot();
        let (code, cur) = match self.target_state.get(&pc).cloned() {
            Some(t) => self.coerce_to(&snap, &t)?,
            None => (String::new(), snap),
        };
        if !code.is_empty() {
            let code = code.trim_end().to_string();
            self.line(&code);
        }
        // Reflect coerced types in the live state / stack.
        for (d, ty) in cur.stack.iter().enumerate() {
            if self.vs[d].ty != *ty {
                let (a, b) = self.canon(d, *ty);
                let mut v = V::new(*ty, a);
                if ty.is_arr() || *ty == Ty::Range {
                    v.b = b;
                }
                self.vs[d] = v;
            }
        }
        self.record_arrival(pc, cur.clone())?;
        let mut st = cur;
        st.stack.clear();
        st.skey.clear();
        self.st = st;
        Ok(())
    }

    fn enter_label(&mut self, pc: usize) -> Res<()> {
        if self.live {
            self.arrive_fallthrough(pc)?;
        }
        let st = self
            .target_state
            .get(&pc)
            .cloned()
            .or_else(|| self.next_target.get(&pc).cloned());
        match st {
            Some(s) => {
                self.set_state(s);
                self.live = true;
            }
            None => self.live = false,
        }
        self.out.push_str(&format!("  L_{pc}:;\n"));
        Ok(())
    }

    /// Jump (optionally conditional) to `t` after spilling the stack.
    fn emit_jump(&mut self, pc: usize, t: usize, cond: Option<String>) -> Res<()> {
        self.materialize_all();
        if self.loops.iter().any(|f| f.head == t) {
            return self.err("irregular loop back-edge");
        }
        // Early-exit / dyn-trip apply only to loops this jump leaves or
        // re-enters. A nested `loop`/`continue` back-edge must not mark an
        // enclosing `par` as `dyn_trip` (that forced `schedule(dynamic)` on
        // outer mandelbrot rows via an inner escape-time back-edge).
        for f in &mut self.loops {
            if !(t > f.head && t <= f.close) {
                f.early_exit = true;
            }
        }
        if let Some(f) = self.loops.last_mut()
            && t <= pc
            && t > f.head
            && t <= f.close
        {
            f.dyn_trip = true;
        }
        let snap = self.snapshot();
        let (code, cur) = match self.target_state.get(&t).cloned() {
            Some(ts) => self.coerce_to(&snap, &ts)?,
            None => (String::new(), snap),
        };
        self.record_arrival(t, cur)?;
        match cond {
            None => {
                if !code.is_empty() {
                    let c = code.trim_end().to_string();
                    self.line(&c);
                }
                self.line(&format!("goto L_{t};"));
                self.live = false;
            }
            Some(c) => {
                if code.is_empty() {
                    self.line(&format!("if ({c}) goto L_{t};"));
                } else {
                    self.line(&format!("if ({c}) {{ {code}goto L_{t}; }}"));
                }
            }
        }
        Ok(())
    }

    // ----- constants & locals -----------------------------------------------------------

    fn const_v(&mut self, pc: usize, c: u16) -> Res<V> {
        match self.chunk.consts.get(c as usize) {
            Some(Value::Int(n)) => Ok(V::from_cst(Cst::I(*n))),
            Some(Value::Float(f)) => Ok(V::from_cst(Cst::F(*f))),
            Some(Value::Bool(b)) => Ok(V::from_cst(Cst::B(*b))),
            Some(Value::Str(_)) => {
                let mut v = V::new(Ty::Str, format!("SUPER_STR_{c}"));
                v.sconst = Some(c);
                Ok(v)
            }
            Some(Value::Null | Value::Optional(None)) => Ok(V::unit()),
            Some(Value::Array(elems))
                if !elems.is_empty() && elems.iter().all(|v| matches!(v, Value::Int(_))) =>
            {
                // Mutable copy of the comptime table (LUT stays read-only in `.rodata`).
                let n = elems.len();
                let v = self.alloc_array(pc, Ty::ArrI64, Some(n), &format!("{n}LL"))?;
                self.line(&format!(
                    "memcpy({}, SUPER_LUT_{c}, {n} * sizeof(int64_t));",
                    v.a
                ));
                Ok(v)
            }
            other => self.err(format!("unsupported constant {other:?}")),
        }
    }

    fn load_local(&mut self, s: u16) -> Res<V> {
        let su = s as usize;
        if su >= self.nslots {
            return self.err("local slot out of range");
        }
        self.note_slot_read(s);
        if let Some(v) = self.elem_alias.get(&s) {
            let mut v = v.clone();
            v.deps = vec![s];
            return Ok(v);
        }
        let ty = self.st.locals[su];
        Ok(match ty {
            Ty::Bot => {
                self.saw_bot = true;
                V::bot()
            }
            Ty::Top => V::top(),
            Ty::Unit => V::unit(),
            Ty::ArrEmpty => {
                let mut v = V::empty_arr();
                v.deps = vec![s];
                v.rmw = Some(s);
                v
            }
            ty => {
                let (a, b) = names(&format!("l{s}"), ty);
                let mut v = V::new(ty, a);
                if ty.is_arr() || ty == Ty::Range {
                    v.b = b;
                }
                if let Some(src) = self.arr_field_alias.get(&s) {
                    v.arr_field = src.clone();
                }
                v.deps = vec![s];
                v.rmw = Some(s);
                v.root = self.st.lroot[su];
                v.cst = self.st.lcst[su];
                v.lin = self.st.llin[su].clone();
                if let (Some(c), true) = (v.cst, ty.is_scalar()) {
                    v.a = c.lit();
                    v.deps.clear();
                }
                v
            }
        })
    }

    fn store_local(&mut self, s: u16, v: V) -> Res<()> {
        self.store_local_at(self.end, s, v)
    }

    fn store_local_at(&mut self, pc: usize, s: u16, v: V) -> Res<()> {
        let su = s as usize;
        if su >= self.nslots {
            return self.err("local slot out of range");
        }
        match v.ty {
            Ty::Bot => {
                self.saw_bot = true;
                self.st.locals[su] = Ty::Bot;
                self.st.lcst[su] = None;
                self.st.llin[su] = None;
                self.elem_alias.remove(&s);
                return Ok(());
            }
            Ty::ArrEmpty => {
                // Statically empty: no C variables; a merge with a typed array re-initializes.
                self.materialize_dep(s);
                self.st.locals[su] = Ty::ArrEmpty;
                self.st.lcst[su] = None;
                self.st.llin[su] = None;
                self.st.lroot[su] = 0;
                self.elem_alias.remove(&s);
                self.note_slot_write(s, false);
                return Ok(());
            }
            Ty::Top | Ty::Unit => {
                self.materialize_dep(s);
                self.st.locals[su] = v.ty;
                self.st.lcst[su] = None;
                self.st.llin[su] = None;
                self.elem_alias.remove(&s);
                self.note_slot_write(s, false);
                return Ok(());
            }
            _ => {}
        }
        // Preserve array-element lvalues across Dup/StoreLocal/LoadLocal/IndexSet so
        // `arr[i].f = …` stores through `arr[i]` (peer `|*b|`) instead of copy-back.
        if v.elem_lv {
            self.st.locals[su] = v.ty;
            self.st.lcst[su] = None;
            self.st.llin[su] = None;
            self.st.lroot[su] = v.root;
            self.elem_alias.insert(s, v);
            self.arr_field_alias.remove(&s);
            self.note_slot_write(s, false);
            return Ok(());
        }
        self.elem_alias.remove(&s);
        self.materialize_dep(s);
        let ty = v.ty;
        let (na, nb) = self.declare_pair(&format!("l{s}"), ty);
        let is_param_arr = self.fi.is_some()
            && su < self.plan.insts[self.inst].params.len()
            && ty.is_arr()
            && self.mut_param.get(su).copied().unwrap_or(false);
        // Same-root writeback (`dst = sys_u8_copy_range(dst, …)` / fused be32) keeps the
        // param's identity even when the C pointer expression was rematerialized.
        if is_param_arr && v.a != na && v.root != su as u32 + 1 && v.root != self.st.lroot[su] {
            return self.err("`@` array parameter rebound to a different array");
        }
        let self_assign = v.a == na && (!(ty.is_arr() || ty == Ty::Range) || v.b == nb);
        if !self_assign
            && !is_param_arr
            && self.is_mut_param_slot(su)
            && self.plan.holds_ptr(ty)
            // In-place `SetField` results: `set_field` already judged the field it replaced.
            && !(ty.is_struct() && v.rmw == Some(s))
        {
            self.fx.mutw = true;
        }
        // Drop C writes into locals that are never read again (statement temps after
        // in-place `SetField` / CallMut elem write-backs used to copy `Body` / re-bind
        // `bodies` pointers into dead `s0` / `l36` slots).
        let dead = (ty.is_struct() || ty.is_arr())
            && pc < self.end
            && !slot_live_after(self.chunk, pc + 1, s, self.end, self.nslots);
        if !self_assign && !dead {
            self.assign(&na, &v.a);
            if ty.is_arr() || ty == Ty::Range {
                self.assign(&nb, &v.b);
            }
            if ty.is_arr() {
                // Spare capacity is only ever carried by a freshly grown, exclusively owned
                // array; every other store (aliases, call results, literals) is `cap == len`.
                let cn = cap_name(&na);
                self.declare(&cn, "int64_t ", "0");
                let src = if v.c.is_empty() {
                    v.b.clone()
                } else {
                    v.c.clone()
                };
                self.assign(&cn, &src);
            }
            self.note_slot_write(s, v.rmw == Some(s));
        } else if !self_assign {
            self.note_slot_write(s, v.rmw == Some(s));
        }
        self.st.locals[su] = ty;
        self.st.lcst[su] = if ty.is_scalar() { v.cst } else { None };
        self.st.llin[su] = lin_of(&v);
        self.st.lroot[su] = v.root;
        if ty.is_arr() && v.from_call.is_none() && !v.arr_field.is_empty() {
            self.arr_field_alias.insert(s, v.arr_field.clone());
        } else {
            self.arr_field_alias.remove(&s);
        }
        Ok(())
    }

    // ----- arithmetic -------------------------------------------------------------------

    fn num_expr(&self, v: &V, float: bool) -> String {
        if float && v.ty != Ty::F64 {
            format!("((double)({}))", v.a)
        } else if !float && v.ty == Ty::Bool {
            format!("((int64_t)({}))", v.a)
        } else {
            v.a.clone()
        }
    }

    fn binary(&mut self, k: BinK) -> Res<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        if a.ty == Ty::Bot || b.ty == Ty::Bot {
            self.saw_bot = true;
            self.vs.push(V::bot());
            return Ok(());
        }
        if a.ty == Ty::Dyn || b.ty == Ty::Dyn {
            for s in [a.app, b.app].into_iter().flatten() {
                self.disown(s);
            }
            return self.dyn_binary(k, a, b);
        }
        if a.ty == Ty::Str || b.ty == Ty::Str {
            return self.str_binary(k, a, b);
        }
        if !(a.ty.is_num() && b.ty.is_num()) {
            if k.is_cmp() && a.ty == Ty::Unit && b.ty == Ty::Unit {
                let r = matches!(k, BinK::Eq | BinK::Le | BinK::Ge);
                self.vs.push(V::from_cst(Cst::B(r)));
                return Ok(());
            }
            // `null == 1` is false in the VM (no numeric coercion of `null`).
            let unit_vs_num =
                (a.ty == Ty::Unit && b.ty.is_num()) || (a.ty.is_num() && b.ty == Ty::Unit);
            if unit_vs_num && matches!(k, BinK::Eq | BinK::Ne) {
                self.vs.push(V::from_cst(Cst::B(k == BinK::Ne)));
                return Ok(());
            }
            return self.err(format!(
                "`{:?}` {:?} {:?} is not unboxable",
                k.c_op(),
                a.ty,
                b.ty
            ));
        }
        if let (Some(ca), Some(cb)) = (a.cst, b.cst)
            && let Some(r) = fold(k, ca, cb)
        {
            self.vs.push(V::from_cst(r));
            return Ok(());
        }
        let float = a.ty == Ty::F64 || b.ty == Ty::F64;
        let ea = self.num_expr(&a, float);
        let eb = self.num_expr(&b, float);
        let deps = merge_deps(&a.deps, &b.deps);
        let expr = match k {
            BinK::Div if float => format!("({ea} / {eb})"),
            BinK::Div => format!("sup_idiv({ea}, {eb})"),
            BinK::Rem if float => format!("fmod({ea}, {eb})"),
            BinK::Rem => format!("sup_irem({ea}, {eb})"),
            k => format!("({ea} {} {eb})", k.c_op()),
        };
        let ty = if k.is_cmp() {
            Ty::Bool
        } else if float {
            Ty::F64
        } else {
            Ty::I64
        };
        let mut v = V::new(ty, expr);
        v.deps = deps;
        if ty == Ty::I64 {
            let (la, lb) = (lin_of(&a), lin_of(&b));
            v.lin = match k {
                BinK::Add => la.zip(lb).map(|(x, y)| x.add(&y, 1)),
                BinK::Sub => la.zip(lb).map(|(x, y)| x.add(&y, -1)),
                BinK::Mul => match (la, lb, a.cst, b.cst) {
                    (Some(x), _, _, Some(Cst::I(c))) => Some(x.scale(c)),
                    (_, Some(y), Some(Cst::I(c)), _) => Some(y.scale(c)),
                    _ => None,
                },
                _ => None,
            };
        }
        if k == BinK::Add && !k.is_cmp() {
            if let Some(s) = a.rmw
                && !b.deps.contains(&s)
                && b.rmw != Some(s)
            {
                v.rmw = Some(s);
            }
            if v.rmw.is_none()
                && let Some(s) = b.rmw
                && !a.deps.contains(&s)
                && a.rmw != Some(s)
            {
                v.rmw = Some(s);
            }
        }
        self.vs.push(v);
        Ok(())
    }

    /// Operator with a boxed operand: the runtime's `SuperVal` arithmetic (same semantics as
    /// the boxed backend). Comparisons yield a native `bool`, arithmetic a boxed result.
    fn dyn_binary(&mut self, k: BinK, a: V, b: V) -> Res<()> {
        for v in [&a, &b] {
            if !(v.ty == Ty::Dyn || v.ty == Ty::Unit || v.ty.is_scalar() || v.ty == Ty::Str) {
                return self.err(format!("{:?} operand beside a boxed value", v.ty));
            }
        }
        let (ea, eb) = (self.box_val(&a)?, self.box_val(&b)?);
        let f = match k {
            BinK::Add => "super_add",
            BinK::Sub => "super_sub",
            BinK::Mul => "super_mul",
            BinK::Div => "super_div",
            BinK::Rem => "super_rem",
            BinK::Eq => "super_eq",
            BinK::Ne => "super_ne",
            BinK::Lt => "super_lt",
            BinK::Le => "super_le",
            BinK::Gt => "super_gt",
            BinK::Ge => "super_ge",
        };
        self.note_impure();
        let v = if k.is_cmp() {
            self.eager(Ty::Bool, &format!("(super_truthy({f}({ea}, {eb})) != 0)"))
        } else {
            self.eager(Ty::Dyn, &format!("{f}({ea}, {eb})"))
        };
        self.push(v);
        Ok(())
    }

    /// Any read of local `s` other than an append base may alias its string buffer, so the
    /// buffer stops being owned (later appends copy).
    fn disown(&mut self, s: u16) {
        if !self.append_slots.contains(&s) {
            return;
        }
        let o = format!("{}o", names(&format!("l{s}"), Ty::Str).0);
        self.declare(&o, "char *", "nullptr");
        self.note_write_var(&o);
        self.line(&format!("{o} = nullptr;"));
    }

    /// `l{s} = l{s} + b` growing the owned buffer of local `s` in place.
    fn str_append(&mut self, s: u16, b: &V) -> Res<()> {
        // Pending lazy reads of the old value must see it before the buffer grows.
        self.materialize_dep(s);
        let x = names(&format!("l{s}"), Ty::Str).0;
        let (o, n, c) = (format!("{x}o"), format!("{x}n"), format!("{x}c"));
        self.declare(&o, "char *", "nullptr");
        self.declare(&n, "size_t ", "0");
        self.declare(&c, "size_t ", "0");
        for var in [&x, &o, &n, &c] {
            self.note_write_var(var);
        }
        self.note_slot_write(s, true);
        if let Some(code) = b.chr_code.as_deref() {
            self.line(&format!("sup_sapp_ascii(&{x}, &{o}, &{n}, &{c}, {code});"));
        } else {
            let f = match b.ty {
                Ty::Str => "s",
                Ty::I64 => "i",
                Ty::F64 => "f",
                Ty::Bool => "b",
                t => return self.err(format!("string append of {t:?}")),
            };
            self.line(&format!("sup_sapp_{f}(&{x}, &{o}, &{n}, &{c}, {});", b.a));
        }
        let mut v = V::new(Ty::Str, x);
        v.deps = vec![s];
        v.rmw = Some(s);
        v.app = Some(s);
        self.push(v);
        Ok(())
    }

    fn str_binary(&mut self, k: BinK, a: V, b: V) -> Res<()> {
        let empty = |v: &V| {
            v.ty == Ty::Str
                && matches!(v.sconst.and_then(|c| self.chunk.consts.get(c as usize)),
                    Some(Value::Str(t)) if t.is_empty())
        };
        if k == BinK::Add && b.ty == Ty::Str && empty(&a) {
            self.push(b);
            return Ok(());
        }
        if k == BinK::Add && a.ty == Ty::Str && empty(&b) {
            self.push(a);
            return Ok(());
        }
        if let Some(s) = a.app {
            if k == BinK::Add && matches!(b.ty, Ty::Str | Ty::I64 | Ty::F64 | Ty::Bool) {
                return self.str_append(s, &b);
            }
            self.disown(s);
        }
        if let Some(s) = b.app {
            self.disown(s);
        }
        if k == BinK::Add {
            let fname = match (a.ty, b.ty) {
                (Ty::Str, Ty::Str) => "sup_cat_ss",
                (Ty::Str, Ty::I64) => "sup_cat_si",
                (Ty::Str, Ty::F64) => "sup_cat_sf",
                (Ty::Str, Ty::Bool) => "sup_cat_sb",
                (Ty::I64, Ty::Str) => "sup_cat_is",
                (Ty::F64, Ty::Str) => "sup_cat_fs",
                (Ty::Bool, Ty::Str) => "sup_cat_bs",
                (Ty::Str, Ty::Unit) => {
                    let e = format!("sup_cat_ss({}, \"null\")", a.a);
                    let v = self.eager(Ty::Str, &e);
                    self.vs.push(v);
                    return Ok(());
                }
                (Ty::Unit, Ty::Str) => {
                    let e = format!("sup_cat_ss(\"null\", {})", b.a);
                    let v = self.eager(Ty::Str, &e);
                    self.vs.push(v);
                    return Ok(());
                }
                (x, y) => {
                    return self.err(format!(
                        "string `+` with an unboxable operand ({x:?} + {y:?})"
                    ));
                }
            };
            let e = format!("{fname}({}, {})", a.a, b.a);
            let v = self.eager(Ty::Str, &e);
            self.vs.push(v);
            return Ok(());
        }
        if a.ty == Ty::Str && b.ty == Ty::Str {
            let e = match k {
                BinK::Eq => format!("sup_streq({}, {})", a.a, b.a),
                BinK::Ne => format!("(!sup_streq({}, {}))", a.a, b.a),
                k if k.is_cmp() => format!("(sup_strcmp({}, {}) {} 0)", a.a, b.a, k.c_op()),
                _ => return self.err("unsupported string operator"),
            };
            let mut v = V::new(Ty::Bool, e);
            v.deps = merge_deps(&a.deps, &b.deps);
            self.vs.push(v);
            return Ok(());
        }
        match k {
            BinK::Eq => self.vs.push(V::from_cst(Cst::B(false))),
            BinK::Ne => self.vs.push(V::from_cst(Cst::B(true))),
            _ => return self.err("string compared with a non-string"),
        }
        Ok(())
    }

    fn bit_binary(&mut self, op: &Op) -> Res<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        if a.ty == Ty::Bot || b.ty == Ty::Bot {
            self.saw_bot = true;
            self.vs.push(V::bot());
            return Ok(());
        }
        let ok = |t: Ty| matches!(t, Ty::I64 | Ty::Bool);
        if !(ok(a.ty) && ok(b.ty)) {
            return self.err("bit operation on a non-integer");
        }
        let (ea, eb) = (self.num_expr(&a, false), self.num_expr(&b, false));
        let (name, cf): (&str, fn(i64, i64) -> i64) = match op {
            Op::BitAnd => ("&", |x, y| x & y),
            Op::BitOr => ("|", |x, y| x | y),
            Op::BitXor => ("^", |x, y| x ^ y),
            Op::Shl => ("sup_shl", |x, y| ((x as u64) << (y & 63)) as i64),
            Op::Shr => ("sup_shr", |x, y| x >> (y & 63)),
            Op::UShr => ("sup_ushr", |x, y| ((x as u64) >> (y & 63)) as i64),
            _ => return self.err("not a bit op"),
        };
        if let (Some(ca), Some(cb)) = (a.cst, b.cst) {
            self.vs.push(V::from_cst(Cst::I(cf(ca.as_i(), cb.as_i()))));
            return Ok(());
        }
        let expr = if name.starts_with("sup_") {
            format!("{name}({ea}, {eb})")
        } else {
            format!("({ea} {name} {eb})")
        };
        let mut v = V::new(Ty::I64, expr);
        v.deps = merge_deps(&a.deps, &b.deps);
        if matches!(op, Op::Shl)
            && let (Some(l), Some(Cst::I(c))) = (lin_of(&a), b.cst)
            && (0..32).contains(&c)
        {
            v.lin = Some(l.scale(1i64 << c));
        }
        self.vs.push(v);
        Ok(())
    }

    // ----- arrays -----------------------------------------------------------------------

    /// The function returns a flat array (no nested arrays) and no `@` parameter can carry an
    /// array out, so every array it allocates may live on the Goose stack: `SUP_KEEP` moves
    /// the returned one down to the frame mark.
    fn keeps_ret_on_arena(&self) -> bool {
        let flat = match self.ret_cur {
            Ty::ArrStruct(id) => !self.plan.holds_arr(Ty::Struct(id)),
            t => t.is_flat_arr(),
        };
        flat && !self.is_module && !self.mut_param_escapes_arr()
    }

    /// Can array storage leave this frame through its result or an `@` struct parameter?
    fn frame_returns_arrays(&self) -> bool {
        self.plan.holds_arr(self.ret_cur) || self.mut_param_escapes_arr()
    }

    /// `@` value params (copied or pointer-aliased) that carry array fields out.
    fn mut_param_escapes_arr(&self) -> bool {
        if self
            .mut_value_params
            .iter()
            .any(|&(_, t)| self.plan.holds_arr(t))
        {
            return true;
        }
        let params = &self.plan.insts[self.inst].params;
        self.mut_param.iter().enumerate().any(|(i, m)| {
            *m && params
                .get(i)
                .is_some_and(|t| t.is_struct() && self.plan.holds_arr(*t))
        })
    }

    fn alloc_array(&mut self, pc: usize, ty: Ty, n_cst: Option<usize>, n_expr: &str) -> Res<V> {
        ty.elem().ok_or("alloc of non-array")?;
        let cty = ty.c_elem();
        let esz = self.plan.elem_bytes(ty);
        let bytes = n_cst.map(|n| n * esz);
        let in_loop = self
            .loop_pcs
            .get(pc.saturating_sub(self.start))
            .copied()
            .unwrap_or(false);
        if !self.loops.is_empty() {
            self.note_impure();
        }
        // Storage reachable from the result or an `@` struct parameter outlives this frame.
        let ret_is_arr = self.frame_returns_arrays();
        let root = ALLOC_ROOT + pc as u32;
        // A flat array result is the only array leaving the frame: scratch arrays live on the
        // Goose stack and the result is compacted down to the frame mark on return.
        let keep_ret = self.keeps_ret_on_arena();
        let frame_may_grow_arrays = self.chunk.ops[self.start..self.end].iter().any(|op| {
            matches!(
                op,
                Op::CallHost(sym, _)
                    if self
                        .interner
                        .resolve(*sym)
                        .strip_prefix("host.")
                        .unwrap_or_else(|| self.interner.resolve(*sym))
                        == "sys_arr_push"
            )
        });
        if !self.is_module
            && !ret_is_arr
            && !in_loop
            && !frame_may_grow_arrays
            && bytes.is_some_and(|b| b <= STACK_ARRAY_LIMIT_BYTES)
        {
            let n = n_cst.unwrap_or(0);
            let buf = format!("a{pc}_buf");
            self.bufs
                .insert(buf.clone(), (cty.trim_end().to_string(), n.max(1)));
            self.line(&format!("memset({buf}, 0, sizeof({buf}));"));
            return Ok(V::arr(ty, buf, format!("{n}LL"), root));
        }
        let base = self.fresh("t");
        let (pa, pb) = self.declare_pair(&base, ty);
        let alloc = if self.is_module || (ret_is_arr && !keep_ret) {
            format!(
                "({cty}*)sup_heap_zeroed((size_t)({n_expr}) * sizeof({}))",
                cty.trim_end()
            )
        } else {
            self.uses_arena = true;
            self.impure = true;
            // Prefer typed Goose-stack helpers so the pointer is `T *restrict` without
            // a `uint8_t*` round-trip at the call site.
            match ty {
                Ty::ArrI64 => format!("sup_arena_i64((size_t)({n_expr}))"),
                Ty::ArrF64 => format!("sup_arena_f64((size_t)({n_expr}))"),
                Ty::ArrStr => format!("sup_arena_str((size_t)({n_expr}))"),
                _ => format!(
                    "({cty}*)sup_arena_zeroed((size_t)({n_expr}) * sizeof({}))",
                    cty.trim_end()
                ),
            }
        };
        self.assign(&pb, n_expr);
        self.assign(&pa, &alloc);
        Ok(V::arr(ty, pa, pb, root))
    }

    fn make_array(&mut self, pc: usize, n: usize) -> Res<()> {
        if n == 0 {
            self.push(V::empty_arr());
            return Ok(());
        }
        if self.vs.len() < n {
            return self.err("abstract stack underflow");
        }
        let items: Vec<V> = self.vs.split_off(self.vs.len() - n);
        if items.iter().any(|v| v.ty == Ty::Bot) {
            self.saw_bot = true;
            self.vs.push(V::bot());
            return Ok(());
        }
        let elem = items
            .iter()
            .map(|v| v.ty)
            .reduce(|a, b| if a == b { a } else { Ty::Top })
            .unwrap_or(Ty::Top);
        let ty = Ty::grow_of(elem).ok_or_else(|| {
            "array literal elements are heterogeneous or not scalar/str".to_string()
        })?;
        let v = self.alloc_array(pc, ty, Some(n), &format!("{n}LL"))?;
        for (k, it) in items.iter().enumerate() {
            let e = if ty.is_nested_arr() {
                format!("({}){{{}, {}}}", ty.c_elem(), it.a, it.b)
            } else {
                it.a.clone()
            };
            self.line(&format!("{}[{k}] = {e};", v.a));
        }
        self.push(v);
        Ok(())
    }

    fn index_get(&mut self, unchecked: bool) -> Res<()> {
        let idx = self.pop()?;
        let base = self.pop()?;
        if base.ty == Ty::Bot || idx.ty == Ty::Bot {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        if base.ty == Ty::ArrEmpty {
            // A field / literal (no local deps) that is still `[]` once the fixpoint settled is
            // empty on every run: the read is out of range, which the VM answers with `null`.
            if self.plan.empty_reads_zero && base.deps.is_empty() {
                self.push(V::unit());
                return Ok(());
            }
            // Element class not fixed yet (a later pass resolves it); never resolved ⇒ boxed.
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let i = self.idx_expr(&idx)?;
        if base.ty == Ty::Str {
            let e = format!("sup_str_at({}, {i})", base.a);
            let v = self.eager(Ty::Str, &e);
            self.push(v);
            return Ok(());
        }
        let Some(elem) = base.ty.elem() else {
            return self.err(format!("indexing a {:?}", base.ty));
        };
        self.note_load(base.root, lin_of(&idx));
        // Dense u8: widen to i64 (peer `u8` → larger int).
        if base.ty == Ty::ArrU8 {
            let e = if unchecked {
                format!("(int64_t){}[{i}]", base.a)
            } else {
                format!("sup_ld_u8({}, {}, {i})", base.a, base.b)
            };
            let v = self.eager(Ty::I64, &e);
            self.push(v);
            return Ok(());
        }
        if base.ty.is_nested_arr() {
            // Element arrays are (pointer, length) pairs stored inline; an out-of-range index
            // yields the empty array, like the VM's default element.
            let ld = if base.ty == Ty::ArrAF {
                "sup_ld_aaf"
            } else {
                "sup_ld_aa"
            };
            let e = if unchecked {
                format!("{}[{i}]", base.a)
            } else {
                format!("{ld}({}, {}, {i})", base.a, base.b)
            };
            let tmp = self.fresh("e");
            self.declare(&tmp, &format!("{} ", base.ty.c_elem()), "{nullptr, 0}");
            self.assign(&tmp, &e);
            let base = self.fresh("t");
            let (pa, pb) = self.declare_pair(&base, elem);
            self.assign(&pa, &format!("{tmp}.p"));
            self.assign(&pb, &format!("{tmp}.n"));
            self.push(V::arr(elem, pa, pb, UNKNOWN_ROOT));
            return Ok(());
        }
        if let Ty::Struct(id) = elem {
            // Keep `p[i]` as a C lvalue so `p[i].f = v` stores in place (peer `|*b|`).
            // Checked path gates stores/reads on `i < len` (get_field / set_field); the old
            // full-struct materialization forced Body copy-out/in on every physics resolve.
            let k = if unchecked {
                i.clone()
            } else {
                self.eager(Ty::I64, &i).a
            };
            let _ = id; // layout name only needed for materializing zero structs
            let mut v = V::new(elem, format!("{}[{k}]", base.a));
            v.root = base.root;
            v.deps = base.deps.clone();
            v.elem_lv = true;
            if !unchecked {
                v.b = base.b.clone(); // length for gated field RMW
            }
            self.push(v);
            return Ok(());
        }
        let e = if unchecked {
            format!("{}[{i}]", base.a)
        } else if elem.is_scalar() && elem != Ty::Str {
            // Inline gate (matches peer `a[i]`); avoid `sup_ld_*` call in grid gather.
            format!(
                "((uint64_t)({i}) < (uint64_t)({}) ? {}[{i}] : {})",
                base.b,
                base.a,
                elem.zero()
            )
        } else {
            format!("sup_ld_{}({}, {}, {i})", elem.suffix(), base.a, base.b)
        };
        let v = self.eager(elem, &e);
        self.push(v);
        Ok(())
    }

    fn idx_expr(&self, idx: &V) -> Res<String> {
        match idx.ty {
            Ty::I64 => Ok(idx.a.clone()),
            Ty::Bool | Ty::F64 => self.conv(idx, Ty::I64),
            t => self.err(format!("index of class {t:?}")),
        }
    }

    fn store_elem_expr(&self, arr: Ty, val: &V) -> Res<String> {
        let elem = arr.elem().ok_or("store into non-array")?;
        match (elem, val.ty) {
            (Ty::Str, Ty::Str) => Ok(val.a.clone()),
            (Ty::Str, _) | (_, Ty::Str) => self.err("array element class mismatch"),
            (e, _) if e.is_scalar() => self.conv(val, e),
            (Ty::Struct(a), Ty::Struct(b)) if a == b => Ok(val.a.clone()),
            _ => self.err("array element class mismatch"),
        }
    }

    /// Bounds-checked (or BCE-proven `unchecked`) store of `e` into `base[i]`.
    fn emit_elem_store(&mut self, base: &V, i: &str, e: &str, unchecked: bool) {
        let elem = base.ty.elem().unwrap_or(Ty::I64);
        if base.ty == Ty::ArrU8 {
            if unchecked {
                self.line(&format!("{}[{i}] = (uint8_t)(({e}) & 255);", base.a));
            } else {
                self.line(&format!("sup_st_u8({}, {}, {i}, {e});", base.a, base.b));
            }
            return;
        }
        if unchecked {
            self.line(&format!("{}[{i}] = {e};", base.a));
        } else if elem.is_struct() {
            let k = self.eager(Ty::I64, i).a;
            self.line(&format!(
                "if ((uint64_t){k} < (uint64_t)({})) {}[{k}] = {e};",
                base.b, base.a
            ));
        } else {
            // Inline the bounds gate (peer `a[i] = v`) — `sup_st_*` call overhead
            // dominated physics grid fill (hundreds of thousands of entry stores).
            let k = self.eager(Ty::I64, i).a;
            self.line(&format!(
                "if ((uint64_t){k} < (uint64_t)({})) {}[{k}] = {e};",
                base.b, base.a
            ));
        }
    }

    fn index_set(&mut self, unchecked: bool) -> Res<()> {
        let val = self.pop()?;
        let idx = self.pop()?;
        let base = self.pop()?;
        if base.ty == Ty::Bot || idx.ty == Ty::Bot || val.ty == Ty::Bot {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        if base.ty == Ty::ArrEmpty {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        if !base.ty.is_iter_arr() {
            return self.err(format!("indexed store into {:?}", base.ty));
        }
        let i = self.idx_expr(&idx)?;
        // Element lvalue already mutated in place (`p[i].f = …` / CallMut `&bodies[i]`).
        // Skip whole-struct copy-back even when the index expression text differs
        // (`bodies[t9]` vs `bodies[t12]` after independent loads of the same `i`).
        if val.elem_lv {
            let prefix = format!("{}[", base.a);
            if val.a.starts_with(&prefix) && val.a.ends_with(']') {
                self.push(base);
                return Ok(());
            }
        }
        let e = self.store_elem_expr(base.ty, &val)?;
        self.note_store(base.root, lin_of(&idx));
        self.note_ptr_store(base.root, val.ty);
        self.emit_elem_store(&base, &i, &e, unchecked);
        self.push(base);
        Ok(())
    }

    // ----- value structs ----------------------------------------------------------------

    /// Text of a `LoadConst` string key (struct literals push `"field", value` pairs).
    fn const_str(&self, v: &V) -> Option<String> {
        let c: usize = match v.sconst {
            Some(c) => c as usize,
            None => v.a.strip_prefix("SUPER_STR_")?.parse().ok()?,
        };
        match self.chunk.consts.get(c) {
            Some(Value::Str(s)) if v.ty == Ty::Str => Some(s.clone()),
            _ => None,
        }
    }

    /// `(member, C expression)` stores that put `v` into field `f`.
    fn field_stores(&self, f: &SField, v: &V) -> Res<Vec<(String, String)>> {
        Ok(match f.ty {
            t if t.is_scalar() => vec![(f.member.clone(), self.conv(v, t)?)],
            Ty::ArrEmpty if v.ty == Ty::ArrEmpty => vec![],
            t if t.is_arr() && v.ty == Ty::ArrEmpty => vec![
                (f.member.clone(), "nullptr".into()),
                (len_member(&f.member), "0".into()),
            ],
            t if v.ty != t => {
                return self.err(format!(
                    "struct field `{}` expects {t:?}, got {:?}",
                    f.name, v.ty
                ));
            }
            t if t.is_arr() => vec![
                (f.member.clone(), v.a.clone()),
                (len_member(&f.member), v.b.clone()),
            ],
            _ => vec![(f.member.clone(), v.a.clone())],
        })
    }

    fn struct_def(&self, id: u32) -> SDef {
        self.plan.structs[id as usize].clone()
    }

    /// `MakeStruct(name, n)`: `n` (key, value) pairs → a typed C compound literal.
    fn make_struct(&mut self, name: Option<super_core::SymId>, n: usize) -> Res<()> {
        if self.vs.len() < 2 * n {
            return self.err("struct literal: abstract stack underflow");
        }
        let items: Vec<V> = self.vs.split_off(self.vs.len() - 2 * n);
        let mut fields: Vec<(String, V)> = Vec::with_capacity(n);
        for pair in items.as_chunks::<2>().0 {
            let key = self
                .const_str(&pair[0])
                .ok_or("struct field key is not a constant string")?;
            // The VM inserts pairs last-to-first, so the earliest duplicate wins.
            if !fields.iter().any(|(k, _)| *k == key) {
                fields.push((key, pair[1].clone()));
            }
        }
        if fields.iter().any(|(_, v)| v.ty == Ty::Bot) {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let label = name.map(|s| self.interner.resolve(s).to_string());
        let shape: Vec<(String, Ty)> = fields.iter().map(|(k, v)| (k.clone(), v.ty)).collect();
        let id = self.plan.intern_struct(label, &shape)?;
        let def = self.struct_def(id);
        let mut inits = Vec::with_capacity(def.fields.len());
        for f in &def.fields {
            let v = fields
                .iter()
                .find(|(k, _)| *k == f.name)
                .map(|(_, v)| v)
                .ok_or("struct literal lost a field")?;
            for (m, e) in self.field_stores(f, v)? {
                inits.push(format!(".{m} = {e}"));
            }
        }
        let ty = Ty::Struct(id);
        let lit = format!("({}){{{}}}", struct_c_name(id), inits.join(", "));
        let v = self.eager(ty, &lit);
        self.push(v);
        Ok(())
    }

    fn get_field(&mut self, field: super_core::SymId) -> Res<()> {
        let base = self.pop()?;
        let fname = self.interner.resolve(field);
        let id = match base.ty {
            Ty::Bot => {
                self.saw_bot = true;
                self.push(V::bot());
                return Ok(());
            }
            Ty::Struct(id) => id,
            Ty::Dyn => {
                let e = format!("super_obj_get({}, \"{}\")", base.a, escape(fname));
                let v = self.eager(Ty::Dyn, &e);
                self.push(v);
                return Ok(());
            }
            t => return self.err(format!("GetField .{fname} on {t:?}")),
        };
        let def = self.struct_def(id);
        let Some(f) = def.field(fname) else {
            // VM: a missing field reads as `null`.
            self.push(V::unit());
            return Ok(());
        };
        if f.ty == Ty::ArrEmpty {
            self.push(V::empty_arr());
            return Ok(());
        }
        // Checked elem lvalue: OOB field reads yield zero (matches prior whole-struct `{}`).
        let path = if base.elem_lv && !base.b.is_empty() && !f.ty.is_arr() {
            if let Some(idx) = elem_lv_index(&base.a) {
                let zero = match f.ty {
                    Ty::F64 => "0.0",
                    Ty::Bool => "false",
                    Ty::I64 => "0LL",
                    Ty::Str => "\"\"",
                    _ => "0",
                };
                format!(
                    "((uint64_t){idx} < (uint64_t)({}) ? {}.{} : {zero})",
                    base.b, base.a, f.member
                )
            } else {
                format!("{}.{}", base.a, f.member)
            }
        } else {
            format!("{}.{}", base.a, f.member)
        };
        let mut v = if f.ty.is_arr() {
            let len = format!("{}.{}", base.a, len_member(&f.member));
            let mut v = V::arr(f.ty, path.clone(), len, UNKNOWN_ROOT);
            // Remember the field place so `w.bodies = bodies_tmp` after in-place
            // `@w.bodies[i]` CallMut can be recognized as a no-op write-back.
            v.arr_field = path;
            v
        } else {
            V::new(f.ty, path)
        };
        v.deps = base.deps.clone();
        self.push(v);
        Ok(())
    }

    /// `SetField`: value semantics, so the update lands on a copy unless the result is
    /// stored straight back into the local the base was loaded from (`s.x = …` / `s.x += …`).
    fn set_field(&mut self, pc: usize, field: super_core::SymId) -> Res<()> {
        let val = self.pop()?;
        let base = self.pop()?;
        let fname = self.interner.resolve(field).to_string();
        if base.ty == Ty::Bot || val.ty == Ty::Bot {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let Ty::Struct(id) = base.ty else {
            return self.err(format!("SetField .{fname} on {:?}", base.ty));
        };
        if self.struct_def(id).field(&fname).is_none() {
            return self.err(format!("SetField adds field `{fname}` to a value struct"));
        }
        if self.plan.widen_field(id, &fname, val.ty)? {
            // Re-lowered next round with the wider field class.
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let def = self.struct_def(id);
        let f = def.field(&fname).ok_or("struct field vanished")?;
        let stores = self.field_stores(f, &val)?;
        // `@p.f = v` replacing a pointer `p` hands back; writing the field's own array
        // back after an element store (`@p.f[i] = x`) keeps the pointer, and the result of a
        // callee that allocates nothing cannot be fresh storage.
        let field_ptr = format!("{}.{}", base.a, f.member);
        let same_arr = f.ty.is_arr()
            && val.from_call.is_none()
            && (val.a == field_ptr || val.arr_field == field_ptr);
        if self.plan.holds_ptr(f.ty)
            && !same_arr
            && base
                .deps
                .iter()
                .any(|&s| self.is_mut_param_slot(s as usize))
        {
            match val.from_call {
                Some(c) if f.ty.is_arr() => {
                    self.fx.mutw_if_alloc.insert(c);
                }
                _ => self.fx.mutw = true,
            }
        }
        // Array-element lvalue: `bodies[i].vx = …` stores through `bodies[i]` (no struct copy).
        if base.elem_lv {
            self.note_store(base.root, None);
            self.note_write_var(&base.a);
            let gated = !base.b.is_empty() && elem_lv_index(&base.a).is_some();
            if gated {
                let idx = elem_lv_index(&base.a).unwrap();
                self.line(&format!("if ((uint64_t){idx} < (uint64_t)({})) {{", base.b));
                for (m, e) in stores {
                    self.line(&format!("  {}.{m} = {e};", base.a));
                }
                self.line("}");
            } else {
                for (m, e) in stores {
                    self.line(&format!("{}.{m} = {e};", base.a));
                }
            }
            self.push(base);
            return Ok(());
        }
        let in_place = base.rmw.filter(|&s| {
            base.deps == [s]
                && base.a == names(&format!("l{s}"), base.ty).0
                && match self.chunk.ops.get(pc + 1) {
                    Some(Op::StoreLocal(d)) => *d == s,
                    Some(Op::Dup) => {
                        matches!(self.chunk.ops.get(pc + 2), Some(Op::StoreLocal(d)) if *d == s)
                    }
                    _ => false,
                }
        });
        let out = match in_place {
            Some(s) => {
                self.materialize_dep(s);
                self.note_slot_write(s, false);
                let mut v = V::new(base.ty, base.a.clone());
                v.deps = vec![s];
                v.rmw = Some(s);
                v
            }
            None => self.eager(base.ty, &base.a),
        };
        self.note_write_var(&out.a);
        // Skip no-op `s.arr = s.arr` after in-place element stores (same pointer expr).
        if !same_arr {
            for (m, e) in stores {
                self.line(&format!("{}.{m} = {e};", out.a));
            }
        }
        self.push(out);
        Ok(())
    }

    // ----- loops ------------------------------------------------------------------------

    fn iter_begin(&mut self, pc: usize) -> Res<()> {
        let Some(t) = iter_next_after(self.chunk, pc) else {
            return self.err("IterBegin without IterNext");
        };
        let it = self.pop()?;
        if it.ty == Ty::ArrEmpty || it.ty == Ty::Bot {
            // Element class not fixed yet (an empty literal, or a callee whose result is still
            // unresolved): a zero-trip array walk whose `it` is unresolved, so element-typed
            // uses (`it.field`) wait for the widening pass instead of failing on a stand-in.
            self.saw_bot = true;
            let n = format!("it{t}_n");
            self.declare(&n, "int64_t ", "0");
            self.assign(&n, "0");
            self.has_loops = true;
            self.iter_root.insert(t, 0);
            self.st.iters.push(IterTy {
                head: t,
                kind: IterKind::Arr(Ty::ArrEmpty),
            });
            return Ok(());
        }
        self.has_loops = true;
        match it.ty {
            Ty::Range => {
                let lo = format!("it{t}_lo");
                let hi = format!("it{t}_hi");
                self.declare(&lo, "int64_t ", "0");
                self.declare(&hi, "int64_t ", "0");
                let (a, b) = (it.a.clone(), it.b.clone());
                self.assign(&lo, &a);
                self.assign(&hi, &b);
                let lo_cst = match it.cst {
                    Some(Cst::I(k)) => Some(k),
                    _ => None,
                };
                let hi_cst = match &it.lin {
                    Some(l) if l.terms.is_empty() => Some(l.off),
                    _ => None,
                };
                self.st.iters.push(IterTy {
                    head: t,
                    kind: IterKind::Range { lo_cst, hi_cst },
                });
            }
            ty if ty.is_iter_arr() => {
                let p = format!("it{t}_a");
                let n = format!("it{t}_n");
                self.declare(&p, &ty.cty(), "nullptr");
                self.declare(&n, "int64_t ", "0");
                self.assign(&p, &it.a);
                self.assign(&n, &it.b);
                self.iter_root.insert(t, it.root);
                self.st.iters.push(IterTy {
                    head: t,
                    kind: IterKind::Arr(ty),
                });
            }
            t => return self.err(format!("iterating a {t:?}")),
        }
        Ok(())
    }

    fn iter_next(&mut self, pc: usize, done: usize) -> Res<()> {
        let close = done.wrapping_sub(1);
        if !(close > pc
            && matches!(self.chunk.ops.get(close), Some(Op::Jump(t)) if *t as usize == pc))
        {
            return self.err("loop is not in structured for form");
        }
        let Some(top) = self.st.iters.last().cloned() else {
            return self.err("IterNext without iterator");
        };
        if top.head != pc {
            return self.err("IterNext/IterBegin mismatch");
        }
        let (init, cond, trip) = match &top.kind {
            IterKind::Range { .. } => (
                format!("it{pc}_lo"),
                format!("it{pc}_hi"),
                format!("(it{pc}_hi - it{pc}_lo)"),
            ),
            IterKind::Arr(_) => ("0".into(), format!("it{pc}_n"), format!("it{pc}_n")),
        };
        let _ = trip;
        let marker = format!("/*@OMP{pc}@*/");
        self.out.push_str(&marker);
        self.out.push('\n');
        self.line(&format!(
            "for (int64_t i{pc} = {init}; i{pc} < {cond}; ++i{pc}) {{"
        ));
        let explicit = std::mem::take(&mut self.par_next);
        let parent = self.loops.last().map(|f| f.head);
        let root = self.iter_root.get(&pc).copied().unwrap_or(0);
        let head_state = self.snapshot();
        self.loops.push(LoopFrame {
            head: pc,
            close,
            done,
            explicit_par: explicit,
            head_state,
            kind: top.kind,
            root,
            written: BTreeSet::new(),
            slot_writes: HashMap::new(),
            slot_reads: HashMap::new(),
            slot_rmw: HashMap::new(),
            stores: Vec::new(),
            loads: Vec::new(),
            impure: false,
            early_exit: false,
            dyn_trip: false,
            parent,
            it_elem_lv: false,
        });
        Ok(())
    }

    fn cur_loop(&self) -> Res<(usize, IterKind)> {
        match self.st.iters.last() {
            Some(it) => Ok((it.head, it.kind.clone())),
            None => self.err("iterator op outside a loop"),
        }
    }

    fn push_it(&mut self) -> Res<()> {
        let (t, kind) = self.cur_loop()?;
        match kind {
            IterKind::Range { .. } => {
                let mut v = V::new(Ty::I64, format!("i{t}"));
                v.lin = Some(Lin::var(t as u32));
                self.push(v);
            }
            IterKind::Arr(Ty::ArrEmpty) => {
                self.saw_bot = true;
                self.push(V::bot());
            }
            IterKind::Arr(ty) => {
                let elem = ty.elem().unwrap_or(Ty::I64);
                let root = self.iter_root.get(&t).copied().unwrap_or(0);
                self.note_load(root, Some(Lin::var(t as u32)));
                let e = format!("it{t}_a[i{t}]");
                if ty.is_nested_arr() {
                    // Each row is an inline (pointer, length) pair; `it` views it in place.
                    let base = self.fresh("t");
                    let (pa, pb) = self.declare_pair(&base, elem);
                    self.assign(&pa, &format!("{e}.p"));
                    self.assign(&pb, &format!("{e}.n"));
                    self.push(V::arr(elem, pa, pb, UNKNOWN_ROOT));
                } else if matches!(elem, Ty::Struct(_)) {
                    // peer `|*b|`: keep `it_a[i]` as a C lvalue so `it.f = …` stores in place.
                    if let Some(f) = self.loops.last_mut() {
                        f.it_elem_lv = true;
                    }
                    let mut v = V::new(elem, e);
                    v.root = root;
                    v.elem_lv = true;
                    self.push(v);
                } else {
                    let v = self.eager(elem, &e);
                    self.push(v);
                }
            }
        }
        Ok(())
    }

    fn push_index(&mut self) -> Res<()> {
        let (t, kind) = self.cur_loop()?;
        match kind {
            IterKind::Range { lo_cst, .. } => {
                let mut v = V::new(Ty::I64, format!("(i{t} - it{t}_lo)"));
                if let Some(lo) = lo_cst {
                    let mut l = Lin::var(t as u32);
                    l.off = lo.wrapping_neg();
                    v.lin = Some(l);
                    if lo == 0 {
                        v.a = format!("i{t}");
                    }
                }
                self.push(v);
            }
            IterKind::Arr(_) => {
                let mut v = V::new(Ty::I64, format!("i{t}"));
                v.lin = Some(Lin::var(t as u32));
                self.push(v);
            }
        }
        Ok(())
    }

    fn iter_set_it(&mut self) -> Res<()> {
        let val = self.pop()?;
        let (t, kind) = self.cur_loop()?;
        let IterKind::Arr(ty) = kind else {
            return self.err("IterSetIt on a range loop");
        };
        if ty == Ty::ArrEmpty || val.ty == Ty::Bot {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let root = self.iter_root.get(&t).copied().unwrap_or(0);
        // Struct `|*b|` loops: fields already stored through `it_a[i]`. Never copy-back
        // from the `it` local — CFG merges clear `elem_alias` and would emit a stale /
        // undeclared `lN_S = …` write (broke algo-15 AOT).
        let it_elem_lv = self.loops.last().map(|f| f.it_elem_lv).unwrap_or(false);
        let slot = format!("it{t}_a[i{t}]");
        if it_elem_lv || (val.elem_lv && val.a == slot) {
            self.push(V::arr(ty, format!("it{t}_a"), format!("it{t}_n"), root));
            return Ok(());
        }
        let e = self.store_elem_expr(ty, &val)?;
        self.note_store(root, Some(Lin::var(t as u32)));
        self.note_ptr_store(root, val.ty);
        self.line(&format!("it{t}_a[i{t}] = {e};"));
        self.push(V::arr(ty, format!("it{t}_a"), format!("it{t}_n"), root));
        Ok(())
    }

    /// Close the `for` opened at `frame.head` when the walker reaches its back-edge `Jump`.
    fn close_loop(&mut self) -> Res<()> {
        let frame = self.loops.pop().ok_or("loop close without frame")?;
        if self.live {
            self.materialize_all();
            let snap = self.snapshot();
            // Back-edge arrival at the head merges into the head state.
            let head_t = self.target_state.get(&frame.head).cloned();
            let (code, cur) = match head_t {
                Some(t) => self.coerce_to(&snap, &t)?,
                None => (String::new(), snap),
            };
            self.loops.push(frame);
            if !code.is_empty() {
                let c = code.trim_end().to_string();
                self.line(&c);
            }
            let frame = self.loops.pop().ok_or("loop close without frame")?;
            self.record_arrival(frame.head, cur)?;
            self.finish_close(frame)
        } else {
            self.finish_close(frame)
        }
    }

    fn finish_close(&mut self, mut frame: LoopFrame) -> Res<()> {
        self.line("}");
        // Do not propagate `dyn_trip` / `early_exit` to enclosing loops: a nested
        // `break if` (mandelbrot inner iter) must not force `schedule(dynamic)` on
        // an outer `par` — that oversubscribes and tanks wall time vs `static`.
        // Loop exit must see back-edge refinements (e.g. ArrEmpty → ArrI64 from
        // `xs = arr_push(xs, …)`), not only the entry snapshot.
        let mut exit = self
            .next_target
            .get(&frame.head)
            .cloned()
            .or_else(|| self.target_state.get(&frame.head).cloned())
            .unwrap_or_else(|| frame.head_state.clone());
        if exit.iters.len() < frame.head_state.iters.len() {
            exit = frame.head_state.clone();
        }
        exit.iters.pop();
        let exit_snapshot = exit.clone();
        let (code, cur) = match self.target_state.get(&frame.done).cloned() {
            Some(t) => self.coerce_to(&exit_snapshot, &t)?,
            None => (String::new(), exit_snapshot),
        };
        if !code.is_empty() {
            let c = code.trim_end().to_string();
            self.line(&c);
        }
        self.record_arrival(frame.done, cur)?;
        self.live = false;
        let decision = self.decide_omp(&mut frame);
        match decision {
            Ok(p) => {
                self.omp.insert(frame.head, (Some(p), frame.parent));
            }
            Err(reason) => {
                self.omp.insert(frame.head, (None, frame.parent));
                if frame.explicit_par {
                    self.omp_notes.insert(frame.head, reason);
                }
            }
        }
        Ok(())
    }

    /// Prove the loop body race-free and build the `omp parallel for` pragma.
    fn decide_omp(&mut self, fr: &mut LoopFrame) -> Result<String, String> {
        if fr.impure {
            return Err("body has side effects (I/O, globals, boxed calls, arena)".into());
        }
        if fr.early_exit {
            return Err("body exits the loop early (break/return)".into());
        }
        let head = fr.head;
        let exposed = exposed_reads(self.chunk, head + 1, fr.close, self.nslots);
        let mut reductions: Vec<String> = Vec::new();
        let mut slots: Vec<u16> = fr.slot_writes.keys().copied().collect();
        slots.sort_unstable();
        for s in slots {
            let w = fr.slot_writes[&s];
            if !exposed.contains(&s) {
                if slot_live_after(self.chunk, fr.done, s, self.end, self.nslots) {
                    return Err(format!("private scalar r{s} is live after the loop"));
                }
                continue;
            }
            let reads = fr.slot_reads.get(&s).copied().unwrap_or(0);
            let rmw = fr.slot_rmw.get(&s).copied().unwrap_or(0);
            if !(w > 0 && reads == rmw && rmw == w) {
                return Err(format!("loop-carried scalar r{s}"));
            }
            let ty = fr.head_state.locals[s as usize];
            let ok_ty = ty == Ty::I64 || (ty == Ty::F64 && fr.explicit_par);
            if !ok_ty {
                return Err(format!("reduction r{s} has class {ty:?}"));
            }
            let prefix = format!("l{s}_");
            let n = fr
                .written
                .iter()
                .filter(|nm| nm.starts_with(&prefix) && !nm[prefix.len()..].contains('_'))
                .count();
            if n != 1 {
                return Err(format!("reduction r{s} changes class inside the loop"));
            }
            reductions.push(format!("l{s}_{}", ty.suffix()));
        }
        // Memory: every written array must be touched at one injective index per iteration.
        let mut store_lin: HashMap<u32, Option<Lin>> = HashMap::new();
        for (root, lin) in &fr.stores {
            if *root == UNKNOWN_ROOT || *root == 0 {
                return Err("store through an array of unknown identity".into());
            }
            let Some(l) = lin else {
                return Err("store index is not affine in the loop variable".into());
            };
            if l.terms.len() != 1 || l.terms[0].0 != head as u32 || l.terms[0].1 == 0 {
                return Err("store index is not injective in the loop variable".into());
            }
            match store_lin.get(root) {
                Some(Some(prev)) if prev != l => {
                    return Err("array written at several indices per iteration".into());
                }
                _ => {
                    store_lin.insert(*root, Some(l.clone()));
                }
            }
        }
        if !store_lin.is_empty() {
            for (root, lin) in &fr.loads {
                if *root == UNKNOWN_ROOT || *root == 0 {
                    return Err("read of an array of unknown identity".into());
                }
                if let Some(Some(sl)) = store_lin.get(root)
                    && lin.as_ref() != Some(sl)
                {
                    return Err("array read at an index other than the one written".into());
                }
            }
        }
        if !fr.explicit_par {
            if reductions.is_empty() {
                return Err("auto-parallel requires a `+` reduction".into());
            }
            // Nested auto-OpenMP thrashes on small inner loops (e.g. 3×3 neighbor
            // walks inside an entity sweep). Explicit `par` still nests when asked.
            if fr.parent.is_some() {
                return Err("auto-parallel skips nested loops".into());
            }
        }
        let private: Vec<&String> = fr
            .written
            .iter()
            .filter(|n| !reductions.contains(n))
            .collect();
        // Explicit `par` prefers guided chunks: row costs vary (mandelbrot
        // escape time). Guided,4 beat static/dynamic on hard-order rate vs a
        // full-CPU peer pool in quiet Protocol-n=10 trials. Auto-par stays
        // static unless this frame is dyn_trip.
        let sched = if fr.explicit_par {
            "schedule(guided, 4)"
        } else if fr.dyn_trip {
            "schedule(dynamic, 1)"
        } else {
            "schedule(static)"
        };
        let min_trip = if fr.explicit_par {
            EXPLICIT_PAR_MIN_TRIP
        } else {
            AUTO_PAR_MIN_TRIP
        };
        // Provably-tiny constant trips must stay serial: emitting a dead
        // `#pragma omp ... if(0)` still links libgomp and inflates RSS.
        // Auto-par with a *dynamic* trip is the same footgun: a runtime
        // `if(trip >= 32768)` that never fires (lakehouse feature loops, etc.)
        // still pulls libgomp + drops `-static`, blowing Speed/RSS. Only
        // auto-parallelize when the trip is a compile-time constant ≥ gate.
        // Explicit `par` keeps a runtime trip-gate for intentional parallelism.
        match &fr.kind {
            IterKind::Range {
                lo_cst: Some(lo),
                hi_cst: Some(hi),
            } => {
                let trip = hi.wrapping_sub(*lo);
                if trip < min_trip {
                    return Err(format!(
                        "constant trip {trip} is below OpenMP min {min_trip}"
                    ));
                }
            }
            _ if !fr.explicit_par => {
                return Err("auto-parallel requires a constant trip count".into());
            }
            _ => {}
        }
        let mut p = format!("#pragma omp parallel for {sched}");
        // Grain-based team size: ~`OMP_GRAIN` iters/thread, capped at
        // `OMP_TEAM_MAX`. Explicit `par` uses a finer grain so heavy row work
        // (mandelbrot 256×256) can take ~16–28 threads — measured to beat a
        // full-CPU peer thread pool — while auto-par keeps a coarser grain so
        // tiny/medium loops do not oversubscribe. Cap at 28: a hard 32-wide
        // team on 32-core hosts regresses mandelbrot vs 24–28.
        let (omp_grain, omp_team_max) = if fr.explicit_par {
            // 256-row mandelbrot: 24 threads is the measured quiet-room sweet
            // spot vs a full-CPU peer pool; 28 ties, 32 regresses hard.
            (8_i64, 24_i64)
        } else {
            (32_i64, 32_i64)
        };
        {
            let trip_expr = match &fr.kind {
                IterKind::Range { .. } => format!("(it{head}_hi - it{head}_lo)"),
                IterKind::Arr(_) => format!("it{head}_n"),
            };
            if let IterKind::Range {
                lo_cst: Some(lo),
                hi_cst: Some(hi),
            } = &fr.kind
            {
                let trip = hi.wrapping_sub(*lo);
                let teams = (trip / omp_grain).clamp(1, omp_team_max);
                let _ = write!(p, " num_threads({teams})");
            } else {
                // Dynamic trip: same grain formula as a C expr so short outer
                // loops do not fork a hardware-thread army.
                let _ = write!(
                    p,
                    " num_threads((({trip_expr}) / {omp_grain} < 1) ? 1 : (({trip_expr}) / {omp_grain} > {omp_team_max} ? {omp_team_max} : ({trip_expr}) / {omp_grain}))"
                );
            }
            // Always trip-gate: OpenMP fork/join is not free. Auto-par needs a large
            // trip count; explicit `par` still serializes tiny loops where the hint
            // would otherwise regress wall time vs a plain `for`.
            let _ = write!(p, " if(({trip_expr}) >= {min_trip})");
        }
        if !private.is_empty() {
            let list: Vec<&str> = private.iter().map(|s| s.as_str()).collect();
            let _ = write!(p, " private({})", list.join(", "));
        }
        if !reductions.is_empty() {
            let _ = write!(p, " reduction(+:{})", reductions.join(", "));
        }
        let _ = fr.root;
        Ok(p)
    }

    // ----- calls ------------------------------------------------------------------------

    fn call(&mut self, pc: usize, fi: usize, arity: usize, wb: Option<usize>) -> Res<()> {
        if self.vs.len() < arity {
            return self.err("call: abstract stack underflow");
        }
        self.has_calls = true;
        let args: Vec<V> = self.vs.split_off(self.vs.len() - arity);
        if args.iter().any(|a| a.ty == Ty::Bot) {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        if wb.is_none()
            && arity == 2
            && (args[0].ty == Ty::ArrEmpty || args[0].ty.is_arr())
            && self.is_push_forwarder(fi)
        {
            // `arr_push(a, v)` is a thin wrapper over `host.sys_arr_push`: lower it as the
            // intrinsic so the caller's `x = arr_push(x, v)` grows `x` in place.
            return self.arr_push(pc, args);
        }
        if args.iter().any(|a| a.ty == Ty::ArrEmpty) {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let wbs: Vec<(u16, u16)> = wb
            .and_then(|w| self.chunk.mut_writebacks.get(w))
            .cloned()
            .unwrap_or_default();
        let tys: Vec<Ty> = args.iter().map(|a| a.ty).collect();
        match self.plan.request(fi, &tys) {
            Some(id) => {
                self.call_unboxed(fi, id, args, &wbs)?;
                self.capture_status(pc, id);
                Ok(())
            }
            None => self.call_boxed(fi, args, &wbs),
        }
    }

    /// After a call to a fallible / optional instance: move `sup_err_msg` into a local so the
    /// next nested call cannot clobber it. The status stays attached to the result only when
    /// the very next ops consume it (`or` / `or err` probe, tail `return`, or a discarding
    /// `Pop`); any other use sees an unknown (`Top`) value, exactly like a boxed result.
    fn capture_status(&mut self, pc: usize, id: usize) {
        let (fails, nones) = (self.plan.insts[id].fails, self.plan.insts[id].nones);
        if !(fails || nones) || self.vs.last().is_none_or(|v| v.ty == Ty::Bot) {
            return;
        }
        self.note_impure();
        let e = self.fresh("e");
        self.declare(&e, "const char *", "nullptr");
        self.note_write_var(&e);
        self.line(&format!("{e} = sup_err_msg; sup_err_msg = nullptr;"));
        let ops = &self.chunk.ops;
        let probe = match ops.get(pc + 2) {
            Some(Op::IsAbsent) => true,
            Some(Op::CallHost(s, 1)) => {
                let raw = self.interner.resolve(*s);
                raw.strip_prefix("host.").unwrap_or(raw) == "__is_err"
            }
            _ => false,
        };
        let next = ops.get(pc + 1);
        let checked = matches!(next, Some(Op::Return)) || (matches!(next, Some(Op::Dup)) && probe);
        let discarded = matches!(next, Some(Op::Pop));
        let Some(top) = self.vs.last_mut() else {
            return;
        };
        if checked {
            top.ferr = e;
            top.fnone = nones;
        } else if !discarded {
            *top = V::top();
        }
    }

    fn call_unboxed(&mut self, fi: usize, id: usize, args: Vec<V>, wbs: &[(u16, u16)]) -> Res<()> {
        let params = self.plan.insts[id].params.clone();
        let mut_p = self.chunk.functions[fi].mut_params.clone();
        let cname = inst_c_name(&self.plan.insts[id], id);
        // Spill pending reads of locals the callee may write through `@` pointers.
        // Skip array-element lvalues (`&bodies[i]`): materializing would copy the Body out.
        for i in 0..args.len() {
            if mut_p.get(i).copied().unwrap_or(false)
                && (params[i].is_scalar() || params[i].is_struct())
                && let Some((slot, _)) = wbs.iter().find(|(_, p)| *p as usize == i)
            {
                if self.elem_alias.get(slot).is_some_and(|v| v.elem_lv) {
                    continue;
                }
                self.materialize_dep(*slot);
            }
        }
        let mut cargs: Vec<String> = Vec::new();
        let mut written_slots: Vec<u16> = Vec::new();
        for (i, (a, pty)) in args.iter().zip(&params).enumerate() {
            let is_mut = mut_p.get(i).copied().unwrap_or(false);
            match *pty {
                Ty::Unit => {}
                Ty::I64 | Ty::F64 | Ty::Bool if is_mut => {
                    let slot = wbs.iter().find(|(_, p)| *p as usize == i).map(|(s, _)| *s);
                    let target = match slot {
                        Some(s) if self.st.locals[s as usize] == *pty => {
                            let (n, _) = self.declare_pair(&format!("l{s}"), *pty);
                            written_slots.push(s);
                            n
                        }
                        _ => {
                            let t = self.fresh("tmp");
                            self.declare(&t, &pty.cty(), pty.zero());
                            let e = self.conv(a, *pty)?;
                            self.assign(&t, &e);
                            t
                        }
                    };
                    cargs.push(format!("&{target}"));
                }
                t @ (Ty::Str | Ty::Struct(_)) if is_mut => {
                    if a.ty != t {
                        return self.err("`@` argument class mismatch");
                    }
                    let slot = wbs.iter().find(|(_, p)| *p as usize == i).map(|(s, _)| *s);
                    // peer `resolve(&bodies[i], …)`: pass the live element address when the
                    // `@` place is an array-element lvalue — no Body copy / IndexSet writeback.
                    if let Some(s) = slot
                        && let Some(alias) = self.elem_alias.get(&s).cloned()
                        && alias.elem_lv
                        && alias.ty == t
                    {
                        cargs.push(format!("&({})", alias.a));
                        continue;
                    }
                    let target = match slot {
                        Some(s) if self.st.locals[s as usize] == t => {
                            let (n, _) = self.declare_pair(&format!("l{s}"), t);
                            // Seed from the stack arg when the slot was only an elem alias before.
                            if a.a != n {
                                self.assign(&n, &a.a);
                            }
                            written_slots.push(s);
                            n
                        }
                        _ => self.eager(t, &a.a).a,
                    };
                    cargs.push(format!("&{target}"));
                }
                Ty::I64 | Ty::F64 | Ty::Bool => cargs.push(self.conv(a, *pty)?),
                Ty::Str => cargs.push(a.a.clone()),
                Ty::Struct(_) => {
                    if a.ty != *pty {
                        return self.err("struct argument class mismatch");
                    }
                    cargs.push(a.a.clone());
                }
                t if t.is_arr() => {
                    if a.ty != t {
                        return self.err("array argument class mismatch");
                    }
                    cargs.push(a.a.clone());
                    cargs.push(a.b.clone());
                }
                t => return self.err(format!("cannot pass {t:?}")),
            }
        }
        for s in &written_slots {
            self.note_slot_write(*s, false);
            let su = *s as usize;
            self.st.lcst[su] = None;
            self.st.llin[su] = None;
            if self.is_mut_param_slot(su) && self.plan.holds_ptr(self.st.locals[su]) {
                self.fx.mutw_calls.insert(id);
            }
        }
        self.callees.insert(id);
        let callee_ret = self.plan.insts[id].ret;
        // A flat array result may sit on the Goose stack above this frame's mark (`SUP_KEEP`):
        // a frame that hands no array back releases it on return like its own scratch.
        if callee_ret.is_arr() && !self.is_module && !self.frame_returns_arrays() {
            self.uses_arena = true;
        }
        let (c_impure, c_mem) = (self.plan.insts[id].impure, self.plan.insts[id].writes_mem);
        if c_impure || c_mem {
            for f in &mut self.loops {
                f.impure = true;
            }
        }
        let call = format!("{cname}({})", cargs.join(", "));
        match callee_ret {
            Ty::Bot => {
                self.saw_bot = true;
                self.line(&format!("{call};"));
                self.push(V::bot());
            }
            Ty::Unit => {
                self.line(&format!("{call};"));
                self.push(V::unit());
            }
            Ty::Top => {
                self.line(&format!("{call};"));
                self.push(V::top());
            }
            ty => {
                let base = self.fresh("t");
                let (pa, pb) = self.declare_pair(&base, ty);
                if ty.is_arr() {
                    // Arrays are returned through (pointer, length) out-parameters.
                    self.note_write_var(&pb);
                    let tmp_len = pb.clone();
                    cargs.push(format!("&{tmp_len}"));
                    let call = format!("{cname}({})", cargs.join(", "));
                    self.assign(&pa, &call);
                    let mut v = V::arr(ty, pa, pb, UNKNOWN_ROOT);
                    v.from_call = Some(id);
                    // A callee that hands back one of its array parameters preserves identity.
                    if let Some(r) = self.plan.insts[id].params.iter().position(|p| *p == ty)
                        && self.plan.insts[id]
                            .params
                            .iter()
                            .filter(|p| **p == ty)
                            .count()
                            == 1
                    {
                        v.root = args[r].root;
                    }
                    self.push(v);
                } else {
                    self.assign(&pa, &call);
                    self.push(V::new(ty, pa));
                }
            }
        }
        Ok(())
    }

    fn call_boxed(&mut self, fi: usize, args: Vec<V>, wbs: &[(u16, u16)]) -> Res<()> {
        self.note_impure();
        self.fx.leak = true;
        self.dyn_calls.insert(fi);
        let fname = fn_boxed_name(self.chunk, self.interner, fi);
        let arr = self.fresh("bargs");
        let n = args.len();
        self.line(&format!("SuperVal {arr}[{}];", n.max(1)));
        for (i, a) in args.iter().enumerate() {
            let boxed = self.box_val(a)?;
            self.line(&format!("{arr}[{i}] = {boxed};"));
        }
        let rv = self.fresh("bret");
        self.line(&format!("SuperVal {rv} = {fname}({arr}, {n});"));
        for (slot, pi) in wbs {
            let ty = self.st.locals[*slot as usize];
            let src = format!("{arr}[{pi}]");
            let unb = match ty {
                Ty::I64 => format!("super_as_i({src})"),
                Ty::F64 => format!("super_as_f({src})"),
                Ty::Bool => format!("(super_truthy({src}) != 0)"),
                Ty::Str => format!("(({src}).as.s)"),
                Ty::Struct(id) if self.plan.unboxable(id) => format!("sup_unbox_s{id}({src})"),
                Ty::Struct(id) if self.plan.unbox_pending(id) => {
                    self.saw_bot = true;
                    continue;
                }
                Ty::Struct(_) => {
                    return self.err("`@` struct with array fields passed to boxed code");
                }
                // Boxed code may grow or replace the array: adopt whatever it left behind.
                t if t.is_flat_arr() || matches!(t, Ty::ArrStruct(_)) => {
                    self.materialize_dep(*slot);
                    let code = self.unbox_dyn_into(&src, &format!("l{slot}"), t, true)?;
                    self.line(code.trim_end());
                    self.note_slot_write(*slot, false);
                    self.st.lcst[*slot as usize] = None;
                    self.st.llin[*slot as usize] = None;
                    continue;
                }
                _ => continue,
            };
            self.materialize_dep(*slot);
            let (nm, _) = self.declare_pair(&format!("l{slot}"), ty);
            self.assign(&nm, &unb);
            self.note_slot_write(*slot, false);
            self.st.lcst[*slot as usize] = None;
            self.st.llin[*slot as usize] = None;
        }
        if self.is_module {
            // Module scripts call boxed `+main`; keep its SuperVal for process exit.
            self.module_exit = Some(rv);
            self.push(V::unit());
        } else {
            let v = self.eager(Ty::Dyn, &rv);
            self.push(v);
        }
        Ok(())
    }

    /// Box `v` for a `SuperVal` callee. Dense numeric arrays are lent as *views* that share
    /// the unboxed storage (stack header, never freed), so `@` writes remain visible.
    fn box_val(&mut self, v: &V) -> Res<String> {
        Ok(match v.ty {
            Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str => box_expr_of(v.ty, &v.a),
            Ty::Dyn => v.a.clone(),
            Ty::Unit => "SUPER_NULL".to_string(),
            Ty::ArrEmpty => {
                // Class still unknown in this pass; unresolved at the fixpoint ⇒ boxed fallback.
                self.saw_bot = true;
                "SUPER_NULL".to_string()
            }
            Ty::ArrI64 | Ty::ArrF64 => {
                let hdr = self.fresh("view");
                let init = if v.ty == Ty::ArrI64 { "i64" } else { "f64" };
                self.line(&format!("SuperArr {hdr};"));
                self.line(&format!("sup_view_init_{init}(&{hdr}, {}, {});", v.a, v.b));
                self.writes_mem = true;
                format!("super_arr(&{hdr})")
            }
            Ty::ArrStr => format!("sup_box_astr({}, {})", v.a, v.b),
            Ty::ArrBool => format!("sup_box_ab({}, {})", v.a, v.b),
            Ty::ArrU8 => format!("sup_box_au({}, {})", v.a, v.b),
            Ty::Struct(id) if self.plan.boxable(id) => format!("sup_box_s{id}({})", v.a),
            // Snapshot copy: struct elements have no shared boxed view.
            Ty::ArrStruct(id) if self.plan.boxable(id) => {
                format!("sup_box_as{id}({}, {})", v.a, v.b)
            }
            t => return self.err(format!("cannot box {t:?} for a boxed call")),
        })
    }

    // ----- growable arrays --------------------------------------------------------------

    /// Is `fi` exactly `f(a, v) => host.sys_arr_push(a, v)` (e.g. `std.core.collections.arr_push`)?
    fn is_push_forwarder(&self, fi: usize) -> bool {
        let f = &self.chunk.functions[fi];
        if f.arity != 2 || f.mut_params.iter().any(|m| *m) {
            return false;
        }
        let (s, e) = fn_range(self.chunk, fi);
        match self.chunk.ops.get(s..e.min(self.chunk.ops.len())) {
            Some(
                [
                    Op::LoadLocal(0),
                    Op::LoadLocal(1),
                    Op::CallHost(sym, 2),
                    Op::Return,
                ],
            ) => {
                let raw = self.interner.resolve(*sym);
                raw.strip_prefix("host.").unwrap_or(raw) == "sys_arr_push"
            }
            _ => false,
        }
    }

    /// `sys_arr_push(arr, v)` on typed growable storage (`T *` + length + capacity).
    ///
    /// Value semantics are preserved: the pushed-onto array is only mutated in place when the
    /// result is stored straight back into the very local that held it (`x = push(x, v)`) —
    /// the only alias-free situation the VM also mutates in place. Aliases created earlier
    /// (`y = x`, call arguments, results) always carry `cap == len`, so a push through them
    /// copies, and appends through `x` land beyond every alias's length. Every other push
    /// copies into a fresh buffer. Superseded buffers are never freed (like all heap arrays
    /// in the unboxed backend), so snapshot iterators and aliases can never dangle.
    fn arr_push(&mut self, pc: usize, args: Vec<V>) -> Res<()> {
        let mut it = args.into_iter();
        let (Some(src), Some(val)) = (it.next(), it.next()) else {
            return self.err("push: abstract stack underflow");
        };
        let target = match src.ty {
            Ty::ArrEmpty => Ty::grow_of(val.ty),
            t if t.is_arr() => Some(t),
            _ => None,
        };
        let Some(target) = target.filter(|t| t.elem() == Some(val.ty)) else {
            return self.err(format!(
                "push of {:?} onto {:?} is not a homogeneous typed array",
                val.ty, src.ty
            ));
        };
        let elem = if target.is_nested_arr() {
            format!("({}){{{}, {}}}", target.c_elem(), val.a, val.b)
        } else {
            val.a.clone()
        };
        let cty = target.cty();
        let esz = format!("sizeof({})", target.c_elem());
        self.note_impure();
        self.note_store(UNKNOWN_ROOT, None);

        let dest = match self.chunk.ops.get(pc + 1) {
            Some(Op::StoreLocal(d)) => Some(*d),
            _ => None,
        };
        let in_place = dest.filter(|&d| {
            let du = d as usize;
            let plain = if src.ty == Ty::ArrEmpty {
                true
            } else {
                let (p, n) = names(&format!("l{d}"), src.ty);
                src.a == p && src.b == n
            };
            plain
                && src.rmw == Some(d)
                && src.deps == [d]
                && du < self.nslots
                && self.st.locals[du] == src.ty
                && !self.mut_param.get(du).copied().unwrap_or(false)
        });

        if let Some(d) = in_place {
            self.materialize_dep(d);
            let (p, n, c) = self.declare_arr_vars(&format!("l{d}"), target);
            if src.ty == Ty::ArrEmpty {
                self.line(&format!("{p} = nullptr; {n} = 0; {c} = 0;"));
            }
            self.line(&format!(
                "if ({n} == {c}) {p} = ({cty})sup_grow({p}, {n}, &{c}, {esz});"
            ));
            self.line(&format!("{p}[{n}] = {elem};"));
            self.line(&format!("{n} += 1;"));
            for v in [&p, &n, &c] {
                self.note_write_var(v);
            }
            self.note_slot_write(d, false);
            let du = d as usize;
            self.st.locals[du] = target;
            self.st.lcst[du] = None;
            self.st.llin[du] = None;
            self.st.lroot[du] = UNKNOWN_ROOT;
            let mut v = V::arr(target, p, n, UNKNOWN_ROOT);
            v.deps = vec![d];
            v.rmw = Some(d);
            self.push(v);
            return Ok(());
        }

        let base = self.fresh("t");
        let (np, nn) = self.declare_pair(&base, target);
        let nc = cap_name(&np);
        self.declare(&nc, "int64_t ", "0");
        self.assign(&nn, &src.b);
        self.assign(&nc, "0");
        self.assign(
            &np,
            &format!("({cty})sup_push_copy({}, {nn}, &{nc}, {esz})", src.a),
        );
        self.line(&format!("{np}[{nn}] = {elem};"));
        self.line(&format!("{nn} += 1;"));
        self.note_write_var(&nn);
        let mut v = V::arr(target, np, nn, UNKNOWN_ROOT);
        v.c = nc;
        self.push(v);
        Ok(())
    }

    // ----- host intrinsics --------------------------------------------------------------

    /// `or err` intrinsics: `__is_err(v)` tests the captured status, `__err_text(v)` reads the
    /// `fail` message (a plain string passes through, anything else is `""`, like the VM).
    fn err_probe(&mut self, name: &str, a: &V) -> Res<()> {
        if a.ty == Ty::Top && a.ferr.is_empty() {
            return self.err(format!("{name} on a value-less result"));
        }
        if a.ty == Ty::Dyn {
            let v = if name == "__is_err" {
                V::new(Ty::Bool, format!("({}.tag == SUPER_TAG_ERR)", a.a))
            } else {
                self.eager(Ty::Str, &format!("sup_err_text_boxed({})", a.a))
            };
            self.push(v);
            return Ok(());
        }
        let fallible = !a.ferr.is_empty();
        if name == "__is_err" {
            if fallible && a.fnone {
                // The present path would have to yield `none` itself, which has no C value here.
                return self.err("`or err` on an optional (`T?`) result");
            }
            let v = if fallible {
                V::new(Ty::Bool, format!("({} != nullptr)", a.ferr))
            } else {
                let mut c = V::from_cst(Cst::B(false));
                c.probe = true;
                c
            };
            self.push(v);
            return Ok(());
        }
        let v = if fallible {
            self.eager(Ty::Str, &format!("sup_err_text({})", a.ferr))
        } else if a.ty == Ty::Str {
            let mut s = a.clone();
            s.ferr.clear();
            s
        } else {
            V::new(Ty::Str, "\"\"")
        };
        self.push(v);
        Ok(())
    }

    fn host_call(&mut self, pc: usize, sym: super_core::SymId, arity: usize) -> Res<()> {
        if self.vs.len() < arity {
            return self.err("host call: abstract stack underflow");
        }
        let args: Vec<V> = self.vs.split_off(self.vs.len() - arity);
        if args.iter().any(|a| a.ty == Ty::Bot) {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        let raw = self.interner.resolve(sym).to_string();
        let bare = raw.strip_prefix("host.").unwrap_or(&raw).to_string();
        let f1 = |n: &str| -> Option<&'static str> {
            Some(match n {
                "sys_sqrt" => "sqrt",
                "sys_floor" => "floor",
                "sys_ceil" => "ceil",
                "sys_round" => "round",
                "sys_sin" => "sin",
                "sys_cos" => "cos",
                "sys_tan" => "tan",
                "sys_asin" => "asin",
                "sys_acos" => "acos",
                "sys_atan" => "atan",
                "sys_exp" => "exp",
                "sys_ln" | "sys_log" => "log",
                "sys_log2" => "log2",
                "sys_log10" => "log10",
                "sys_cbrt" => "cbrt",
                "sys_sinh" => "sinh",
                "sys_cosh" => "cosh",
                _ => return None,
            })
        };
        let f2 = |n: &str| -> Option<&'static str> {
            Some(match n {
                "sys_atan2" => "atan2",
                "sys_pow" => "pow",
                "sys_fmod" => "fmod",
                "sys_hypot" => "hypot",
                _ => return None,
            })
        };
        if arity == 1
            && let Some(cf) = f1(&bare)
        {
            let x = self.conv(&args[0], Ty::F64)?;
            let mut v = V::new(Ty::F64, format!("{cf}({x})"));
            v.deps = args[0].deps.clone();
            self.push(v);
            return Ok(());
        }
        if arity == 2
            && let Some(cf) = f2(&bare)
        {
            let x = self.conv(&args[0], Ty::F64)?;
            let y = self.conv(&args[1], Ty::F64)?;
            let mut v = V::new(Ty::F64, format!("{cf}({x}, {y})"));
            v.deps = merge_deps(&args[0].deps, &args[1].deps);
            self.push(v);
            return Ok(());
        }
        if arity == 1 && matches!(bare.as_str(), "__is_err" | "__err_text") {
            return self.err_probe(&bare, &args[0]);
        }
        let is_push = bare == "sys_arr_push" && arity == 2;
        let is_len = matches!(bare.as_str(), "sys_arr_len" | "len" | "__len") && arity == 1;
        if !is_push && !is_len && args.iter().any(|a| a.ty == Ty::ArrEmpty) {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        }
        match (bare.as_str(), arity) {
            ("sys_abs" | "abs" | "__abs", 1) => {
                let a = &args[0];
                let mut v = match a.ty {
                    Ty::F64 => V::new(Ty::F64, format!("fabs({})", a.a)),
                    Ty::I64 | Ty::Bool => {
                        V::new(Ty::I64, format!("sup_iabs({})", self.conv(a, Ty::I64)?))
                    }
                    t => return self.err(format!("abs of {t:?}")),
                };
                v.deps = a.deps.clone();
                self.push(v);
            }
            ("sys_to_int", 1) => {
                let a = &args[0];
                let mut v = match a.ty {
                    Ty::I64 => V::new(Ty::I64, a.a.clone()),
                    Ty::F64 | Ty::Bool => V::new(Ty::I64, self.conv(a, Ty::I64)?),
                    t => return self.err(format!("to_int of {t:?}")),
                };
                v.deps = a.deps.clone();
                v.lin = lin_of(a);
                v.cst = a.cst.map(|c| Cst::I(c.as_i()));
                self.push(v);
            }
            ("sys_to_float", 1) => {
                let a = &args[0];
                let mut v = V::new(Ty::F64, self.conv(a, Ty::F64)?);
                v.deps = a.deps.clone();
                self.push(v);
            }
            ("sys_arr_len" | "len" | "__len", 1) => {
                let a = &args[0];
                if a.ty == Ty::ArrEmpty {
                    self.push(V::from_cst(Cst::I(0)));
                } else if a.ty.is_arr() {
                    let mut v = V::new(Ty::I64, a.b.clone());
                    v.deps = a.deps.clone();
                    self.push(v);
                } else if a.ty == Ty::Str {
                    let e = format!("sup_byte_len({})", a.a);
                    let v = self.eager(Ty::I64, &e);
                    self.push(v);
                } else {
                    return self.err(format!("len of {:?}", a.ty));
                }
            }
            ("sys_str_len", 1) if args[0].ty == Ty::Str => {
                let e = format!("sup_str_len({})", args[0].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_str_find", 3) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let from = self.conv(&args[2], Ty::I64)?;
                let e = format!("sup_str_find({}, {}, {from})", args[0].a, args[1].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_str_match_at", 3) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let at = self.conv(&args[2], Ty::I64)?;
                let e = format!("sup_str_match_at({}, {}, {at})", args[0].a, args[1].a);
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            ("sys_str_find_lit_digits", 3) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let from = self.conv(&args[2], Ty::I64)?;
                let e = format!(
                    "sup_str_find_lit_digits({}, {}, {from})",
                    args[0].a, args[1].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_str_lit_digits_end", 3) if args[0].ty == Ty::Str && args[2].ty == Ty::Str => {
                let start = self.conv(&args[1], Ty::I64)?;
                let e = format!(
                    "sup_str_lit_digits_end({}, {start}, {})",
                    args[0].a, args[2].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_ord", 1) if args[0].ty == Ty::Str => {
                let e = format!("sup_ord({})", args[0].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_byte_at", 2) if args[0].ty == Ty::Str => {
                let i = self.conv(&args[1], Ty::I64)?;
                let e = format!("sup_byte_at({}, {i})", args[0].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_byte_len", 1) if args[0].ty == Ty::Str => {
                let e = format!("sup_byte_len({})", args[0].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_byte_slice", 3) if args[0].ty == Ty::Str => {
                let lo = self.conv(&args[1], Ty::I64)?;
                let hi = self.conv(&args[2], Ty::I64)?;
                let e = format!("sup_byte_slice({}, {lo}, {hi})", args[0].a);
                let v = self.eager(Ty::Str, &e);
                self.push(v);
            }
            ("sys_byte_find", 3) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let from = self.conv(&args[2], Ty::I64)?;
                let e = format!("sup_byte_find({}, {}, {from})", args[0].a, args[1].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_byte_find_limit", 4) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let from = self.conv(&args[2], Ty::I64)?;
                let limit = self.conv(&args[3], Ty::I64)?;
                let e = format!(
                    "sup_byte_find_limit({}, {}, {from}, {limit})",
                    args[0].a, args[1].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_byte_match_at", 3) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let at = self.conv(&args[2], Ty::I64)?;
                let e = format!("sup_byte_match_at({}, {}, {at})", args[0].a, args[1].a);
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            ("sys_byte_eq_range", 4) if args[0].ty == Ty::Str && args[3].ty == Ty::Str => {
                let lo = self.conv(&args[1], Ty::I64)?;
                let hi = self.conv(&args[2], Ty::I64)?;
                let e = format!(
                    "sup_byte_eq_range({}, {lo}, {hi}, {})",
                    args[0].a, args[3].a
                );
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            ("sys_byte_eq_ranges", 6) if args[0].ty == Ty::Str && args[3].ty == Ty::Str => {
                let alo = self.conv(&args[1], Ty::I64)?;
                let ahi = self.conv(&args[2], Ty::I64)?;
                let blo = self.conv(&args[4], Ty::I64)?;
                let bhi = self.conv(&args[5], Ty::I64)?;
                let e = format!(
                    "sup_byte_eq_ranges({}, {alo}, {ahi}, {}, {blo}, {bhi})",
                    args[0].a, args[3].a
                );
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            ("sys_chr", 1) => {
                let x = self.conv(&args[0], Ty::I64)?;
                // Lazy: `out + chr(c)` uses `chr_code` → `sup_sapp_ascii` with no 1-char temp.
                // Materialize `sup_chr(...)` only if the string value is actually read.
                let mut v = V::new(Ty::Str, format!("sup_chr({x})"));
                v.chr_code = Some(x);
                self.push(v);
            }
            ("sys_str_push_ascii", 2) if args[0].ty == Ty::Str => {
                let code = self.conv(&args[1], Ty::I64)?;
                if let Some(s) = args[0].app {
                    self.materialize_dep(s);
                    let x = names(&format!("l{s}"), Ty::Str).0;
                    let (o, n, c) = (format!("{x}o"), format!("{x}n"), format!("{x}c"));
                    self.declare(&o, "char *", "nullptr");
                    self.declare(&n, "size_t ", "0");
                    self.declare(&c, "size_t ", "0");
                    for var in [&x, &o, &n, &c] {
                        self.note_write_var(var);
                    }
                    self.note_slot_write(s, true);
                    self.line(&format!("sup_sapp_ascii(&{x}, &{o}, &{n}, &{c}, {code});"));
                    let mut v = V::new(Ty::Str, x);
                    v.deps = vec![s];
                    v.rmw = Some(s);
                    v.app = Some(s);
                    self.push(v);
                } else {
                    let e = format!("sup_push_ascii({}, {code})", args[0].a);
                    let v = self.eager(Ty::Str, &e);
                    self.push(v);
                }
            }
            ("sys_str_push_i64", 2) if args[0].ty == Ty::Str => {
                let iv = self.conv(&args[1], Ty::I64)?;
                if let Some(s) = args[0].app {
                    self.materialize_dep(s);
                    let x = names(&format!("l{s}"), Ty::Str).0;
                    let (o, n, c) = (format!("{x}o"), format!("{x}n"), format!("{x}c"));
                    self.declare(&o, "char *", "nullptr");
                    self.declare(&n, "size_t ", "0");
                    self.declare(&c, "size_t ", "0");
                    for var in [&x, &o, &n, &c] {
                        self.note_write_var(var);
                    }
                    self.note_slot_write(s, true);
                    self.line(&format!("sup_sapp_i(&{x}, &{o}, &{n}, &{c}, {iv});"));
                    let mut v = V::new(Ty::Str, x);
                    v.deps = vec![s];
                    v.rmw = Some(s);
                    v.app = Some(s);
                    self.push(v);
                } else {
                    let e = format!("sup_push_i64({}, {iv})", args[0].a);
                    let v = self.eager(Ty::Str, &e);
                    self.push(v);
                }
            }
            ("sys_sb_new", 1) => {
                let cap = self.conv(&args[0], Ty::I64)?;
                let v = self.eager(Ty::I64, &format!("sup_sb_new({cap})"));
                self.push(v);
            }
            ("sys_sb_stack", 1) => {
                let cap = self.conv(&args[0], Ty::I64)?;
                let v = self.eager(Ty::I64, &format!("sup_sb_stack({cap})"));
                self.push(v);
            }
            ("sys_sb_lit", 2) if args[1].ty == Ty::Str => {
                let h = self.conv(&args[0], Ty::I64)?;
                // Const strings → length-known litn (peer Buf.lit / no strlen).
                if let Some(s) = self.const_str(&args[1]) {
                    let n = s.len();
                    self.line(&format!("sup_sb_litn({h}, {}, {n}ULL);", args[1].a));
                } else {
                    self.line(&format!("sup_sb_lit({h}, {});", args[1].a));
                }
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_litn", 3) if args[1].ty == Ty::Str => {
                let h = self.conv(&args[0], Ty::I64)?;
                let n = self.conv(&args[2], Ty::I64)?;
                self.line(&format!("sup_sb_litn({h}, {}, (size_t){n});", args[1].a));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_ascii", 2) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let code = self.conv(&args[1], Ty::I64)?;
                self.line(&format!("sup_sb_ascii({h}, {code});"));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_i64", 2) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let iv = self.conv(&args[1], Ty::I64)?;
                self.line(&format!("sup_sb_i64({h}, {iv});"));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_set", 3) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let i = self.conv(&args[1], Ty::I64)?;
                let code = self.conv(&args[2], Ty::I64)?;
                self.line(&format!("sup_sb_set({h}, {i}, {code});"));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_len", 1) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let v = self.eager(Ty::I64, &format!("sup_sb_len({h})"));
                self.push(v);
            }
            ("sys_sb_clear", 1) => {
                let h = self.conv(&args[0], Ty::I64)?;
                self.line(&format!("sup_sb_clear({h});"));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_cat", 2) => {
                let d = self.conv(&args[0], Ty::I64)?;
                let s = self.conv(&args[1], Ty::I64)?;
                self.line(&format!("sup_sb_cat({d}, {s});"));
                self.note_impure();
                self.push(V::unit());
            }
            ("sys_sb_cstr", 1) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let v = self.eager(Ty::Str, &format!("sup_sb_cstr({h})"));
                self.push(v);
            }
            ("sys_sb_finish", 1) => {
                let h = self.conv(&args[0], Ty::I64)?;
                let v = self.eager(Ty::Str, &format!("sup_sb_finish({h})"));
                self.push(v);
            }
            ("sys_mono_ns", 0) => {
                let v = self.eager(Ty::I64, "sup_mono_ns()");
                self.note_impure();
                self.push(v);
            }
            ("__slice", 3)
                if args[0].ty == Ty::Str
                    && args[1..].iter().all(|a| matches!(a.ty, Ty::I64 | Ty::Bool)) =>
            {
                let (lo, hi) = (self.idx_expr(&args[1])?, self.idx_expr(&args[2])?);
                let e = format!("sup_str_slice({}, {lo}, {hi})", args[0].a);
                let v = self.eager(Ty::Str, &e);
                self.push(v);
            }
            // `xs[lo..hi]` copies the clamped range, like the VM (`lo`/`hi` past the end clamp
            // to the length; a negative bound reads as huge and clamps too).
            ("__slice", 3)
                if matches!(args[0].ty, Ty::ArrI64 | Ty::ArrF64 | Ty::ArrStr)
                    && args[1..].iter().all(|a| matches!(a.ty, Ty::I64 | Ty::Bool)) =>
            {
                let (lo, hi) = (self.idx_expr(&args[1])?, self.idx_expr(&args[2])?);
                let src = &args[0];
                let n = self.eager(Ty::I64, &src.b);
                let clamp =
                    |e: &str| format!("((uint64_t)({e}) < (uint64_t){} ? ({e}) : {})", n.a, n.a);
                let l = self.eager(Ty::I64, &clamp(&lo));
                let h = self.eager(Ty::I64, &clamp(&hi));
                let len = self.eager(
                    Ty::I64,
                    &format!("({} > {} ? {} - {} : 0LL)", h.a, l.a, h.a, l.a),
                );
                let out = self.alloc_array(pc, args[0].ty, None, &len.a)?;
                let cty = args[0].ty.elem().ok_or("slice of non-array")?.cty();
                self.line(&format!(
                    "if ({len} > 0) memcpy({dst}, {src} + {l}, (size_t){len} * sizeof({cty}));",
                    len = len.a,
                    dst = out.a,
                    src = src.a,
                    l = l.a,
                    cty = cty.trim_end()
                ));
                self.push(out);
            }
            ("sys_crc32", 1) if args[0].ty == Ty::Str => {
                let e = format!("sup_crc32_str({})", args[0].a);
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_crc32_bytes_range", 3)
                if args[0].ty == Ty::ArrI64 && args[1].ty == Ty::I64 && args[2].ty == Ty::I64 =>
            {
                let e = format!(
                    "sup_crc32_i64_range({}, {}, {}, {})",
                    args[0].a, args[0].b, args[1].a, args[2].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_crc32_bytes_range", 3)
                if args[0].ty == Ty::ArrU8 && args[1].ty == Ty::I64 && args[2].ty == Ty::I64 =>
            {
                let e = format!(
                    "sup_crc32_u8_range({}, {}, {}, {})",
                    args[0].a, args[0].b, args[1].a, args[2].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_bytes_new", 1) if matches!(args[0].ty, Ty::I64 | Ty::Bool) => {
                let n = &args[0];
                let n_cst = match n.cst {
                    Some(Cst::I(k)) => Some(k.max(0) as usize),
                    _ => None,
                };
                let n_expr = match n_cst {
                    Some(k) => format!("{k}LL"),
                    None => format!("sup_max0({})", self.conv(n, Ty::I64)?),
                };
                let arr = self.alloc_array(pc, Ty::ArrU8, n_cst, &n_expr)?;
                self.push(arr);
            }
            ("sys_u8_copy_range", 5)
                if args[0].ty == Ty::ArrU8
                    && args[1].ty == Ty::I64
                    && args[2].ty == Ty::ArrU8
                    && args[3].ty == Ty::I64
                    && args[4].ty == Ty::I64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_u8_copy_range({}, {}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[2].a, args[2].b, args[3].a, args[4].a
                ));
                self.push(args[0].clone());
            }
            // Dense i64-lane byte buffers (legacy WALs): same intrinsic name, word memmove.
            ("sys_u8_copy_range", 5)
                if args[0].ty == Ty::ArrI64
                    && args[1].ty == Ty::I64
                    && args[2].ty == Ty::ArrI64
                    && args[3].ty == Ty::I64
                    && args[4].ty == Ty::I64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_i64_copy_range({}, {}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[2].a, args[2].b, args[3].a, args[4].a
                ));
                self.push(args[0].clone());
            }
            ("sys_be32_put", 3)
                if args[0].ty == Ty::ArrU8 && args[1].ty == Ty::I64 && args[2].ty == Ty::I64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_be32_put_u8({}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[2].a
                ));
                self.push(args[0].clone());
            }
            ("sys_be32_get", 2) if args[0].ty == Ty::ArrU8 && args[1].ty == Ty::I64 => {
                let e = format!(
                    "sup_be32_get_u8({}, {}, {})",
                    args[0].a, args[0].b, args[1].a
                );
                let v = self.eager(Ty::I64, &e);
                self.push(v);
            }
            ("sys_i64_copy_range", 5)
                if args[0].ty == Ty::ArrI64
                    && args[1].ty == Ty::I64
                    && args[2].ty == Ty::ArrI64
                    && args[3].ty == Ty::I64
                    && args[4].ty == Ty::I64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_i64_copy_range({}, {}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[2].a, args[2].b, args[3].a, args[4].a
                ));
                self.push(args[0].clone());
            }
            ("sys_sha256", 1) if args[0].ty == Ty::Str => {
                let e = format!("sup_sha256_hex({})", args[0].a);
                let v = self.eager(Ty::Str, &e);
                self.push(v);
            }
            ("sys_sha256_bytes", 1) if matches!(args[0].ty, Ty::ArrI64) => {
                let tmp = self.fresh("sha");
                self.line(&format!("int64_t {tmp}_n = 0;"));
                let ptr = self.fresh("sha");
                self.line(&format!(
                    "int64_t *{ptr} = sup_sha256_i64({}, {}, &{tmp}_n);",
                    args[0].a, args[0].b
                ));
                self.push(V::arr(
                    Ty::ArrI64,
                    ptr,
                    format!("{tmp}_n"),
                    ALLOC_ROOT + pc as u32,
                ));
            }
            ("sys_sha2_hex_of", 1) if matches!(args[0].ty, Ty::ArrI64) => {
                let e = format!("sup_sha2_hex_of_i64({}, {})", args[0].a, args[0].b);
                let v = self.eager(Ty::Str, &e);
                self.push(v);
            }
            ("sys_hmac_sha256", 2) if args[0].ty == Ty::Str && args[1].ty == Ty::Str => {
                let e = format!("sup_hmac_sha256_hex({}, {})", args[0].a, args[1].a);
                let v = self.eager(Ty::Str, &e);
                self.push(v);
            }
            ("sys_hmac_sha256_bytes", 2)
                if matches!(args[0].ty, Ty::ArrI64) && matches!(args[1].ty, Ty::ArrI64) =>
            {
                // Native RFC 2104 HMAC over i64-encoded bytes → 32-byte digest as ArrI64.
                let tmp = self.fresh("hmac");
                self.line(&format!("int64_t {tmp}_n = 0;"));
                let ptr = self.fresh("hmac");
                self.line(&format!(
                    "int64_t *{ptr} = sup_hmac_sha256_i64({}, {}, {}, {}, &{tmp}_n);",
                    args[0].a, args[0].b, args[1].a, args[1].b
                ));
                self.push(V::arr(
                    Ty::ArrI64,
                    ptr,
                    format!("{tmp}_n"),
                    ALLOC_ROOT + pc as u32,
                ));
            }
            ("sys_hmac_sha256_bytes", 2)
                if matches!(args[0].ty, Ty::ArrI64) && args[1].ty == Ty::Str =>
            {
                // Key as i64 lanes; message as raw UTF-8 / ASCII wire bytes (no temp byte array).
                let tmp = self.fresh("hmac");
                self.line(&format!("int64_t {tmp}_n = 0;"));
                let ptr = self.fresh("hmac");
                self.line(&format!(
                    "int64_t *{ptr} = sup_hmac_sha256_i64_str({}, {}, {}, &{tmp}_n);",
                    args[0].a, args[0].b, args[1].a
                ));
                self.push(V::arr(
                    Ty::ArrI64,
                    ptr,
                    format!("{tmp}_n"),
                    ALLOC_ROOT + pc as u32,
                ));
            }
            ("sys_hmac_sha256_bytes_range", 4)
                if matches!(args[0].ty, Ty::ArrI64) && args[1].ty == Ty::Str =>
            {
                // HMAC over msg[lo..hi) without allocating a slice (bearer claims / WAL spans).
                let lo = self.conv(&args[2], Ty::I64)?;
                let hi = self.conv(&args[3], Ty::I64)?;
                let tmp = self.fresh("hmacr");
                self.line(&format!("int64_t {tmp}_n = 0;"));
                let ptr = self.fresh("hmacr");
                self.line(&format!(
                    "int64_t *{ptr} = sup_hmac_sha256_i64_str_range({}, {}, {}, {lo}, {hi}, &{tmp}_n);",
                    args[0].a, args[0].b, args[1].a
                ));
                self.push(V::arr(
                    Ty::ArrI64,
                    ptr,
                    format!("{tmp}_n"),
                    ALLOC_ROOT + pc as u32,
                ));
            }
            ("sys_sha2_hex_prefix_eq", 4)
                if matches!(args[0].ty, Ty::ArrI64) && args[2].ty == Ty::Str =>
            {
                let nbytes = self.conv(&args[1], Ty::I64)?;
                let lo = self.conv(&args[3], Ty::I64)?;
                let e = format!(
                    "sup_sha2_hex_prefix_eq({}, {nbytes}, {}, {lo})",
                    args[0].a, args[2].a
                );
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            // Fused bearer verify: HMAC(msg[lo..hi)) + truncated hex compare — no arena digest.
            ("sys_hmac_sha256_hex_prefix_eq_range", 7)
                if matches!(args[0].ty, Ty::ArrI64)
                    && args[1].ty == Ty::Str
                    && args[5].ty == Ty::Str =>
            {
                let lo = self.conv(&args[2], Ty::I64)?;
                let hi = self.conv(&args[3], Ty::I64)?;
                let nbytes = self.conv(&args[4], Ty::I64)?;
                let tlo = self.conv(&args[6], Ty::I64)?;
                let e = format!(
                    "sup_hmac_sha256_hex_prefix_eq_range({}, {}, {}, {lo}, {hi}, {nbytes}, {}, {tlo})",
                    args[0].a, args[0].b, args[1].a, args[5].a
                );
                let v = self.eager(Ty::Bool, &e);
                self.push(v);
            }
            // Emit sig8 into a StrBuilder without a 32-lane arena digest.
            ("sys_hmac_sha256_sig8_into_range", 5)
                if args[0].ty == Ty::I64
                    && matches!(args[1].ty, Ty::ArrI64)
                    && args[2].ty == Ty::Str =>
            {
                let lo = self.conv(&args[3], Ty::I64)?;
                let hi = self.conv(&args[4], Ty::I64)?;
                self.line(&format!(
                    "sup_hmac_sha256_sig8_into_range({}, {}, {}, {}, {lo}, {hi});",
                    args[0].a, args[1].a, args[1].b, args[2].a
                ));
                self.push(V::unit());
            }
            // Forged sig8 emit (`mac[0] ^= 1`) — stack only, no arena digest.
            ("sys_hmac_sha256_sig8_forged_into_range", 5)
                if args[0].ty == Ty::I64
                    && matches!(args[1].ty, Ty::ArrI64)
                    && args[2].ty == Ty::Str =>
            {
                let lo = self.conv(&args[3], Ty::I64)?;
                let hi = self.conv(&args[4], Ty::I64)?;
                self.line(&format!(
                    "sup_hmac_sha256_sig8_forged_into_range({}, {}, {}, {}, {lo}, {hi});",
                    args[0].a, args[1].a, args[1].b, args[2].a
                ));
                self.push(V::unit());
            }
            ("sys_crc32_sb", 1) if args[0].ty == Ty::I64 => {
                let v = self.eager(Ty::I64, &format!("sup_crc32_sb({})", args[0].a));
                self.push(v);
            }
            // peer-parity HTTP/14 wire emit: fixed TLS Buf + HMAC; returns new LCG state.
            ("sys_arr_new", 1 | 2) => {
                let n = &args[0];
                if !matches!(n.ty, Ty::I64 | Ty::Bool) {
                    return self.err("sys_arr_new length is not an integer");
                }
                let fill = args.get(1).cloned().unwrap_or_else(V::unit);
                let ty = match fill.ty {
                    Ty::F64 => Ty::ArrF64,
                    Ty::I64 | Ty::Bool | Ty::Unit => Ty::ArrI64,
                    Ty::Str => Ty::ArrStr,
                    other => {
                        return self.err(format!(
                            "sys_arr_new fill {other:?} is not a scalar or string"
                        ));
                    }
                };
                let n_cst = match n.cst {
                    Some(Cst::I(k)) => Some(k.max(0) as usize),
                    _ => None,
                };
                let n_expr = match n_cst {
                    Some(k) => format!("{k}LL"),
                    None => format!("sup_max0({})", self.conv(n, Ty::I64)?),
                };
                let arr = self.alloc_array(pc, ty, n_cst, &n_expr)?;
                let zero = matches!(
                    fill.cst,
                    Some(Cst::I(0)) | Some(Cst::B(false)) | None if fill.ty == Ty::Unit
                ) || matches!(fill.cst, Some(Cst::I(0)) | Some(Cst::B(false)))
                    || matches!(fill.cst, Some(Cst::F(f)) if f == 0.0 && f.is_sign_positive());
                if !zero {
                    let e = self.store_elem_expr(ty, &fill)?;
                    let k = self.fresh("k");
                    self.line(&format!(
                        "for (int64_t {k} = 0; {k} < {}; ++{k}) {}[{k}] = {e};",
                        arr.b, arr.a
                    ));
                }
                self.push(arr);
            }
            ("sys_arr_push", 2) if args[0].ty == Ty::ArrEmpty || args[0].ty.is_arr() => {
                self.arr_push(pc, args)?;
            }
            ("sys_arr_set", 3) => {
                let (base, idx, val) = (&args[0], &args[1], &args[2]);
                if !base.ty.is_arr() {
                    return self.err("sys_arr_set on a non-array");
                }
                let i = self.idx_expr(idx)?;
                let e = self.store_elem_expr(base.ty, val)?;
                self.note_store(base.root, lin_of(idx));
                self.note_ptr_store(base.root, val.ty);
                self.emit_elem_store(base, &i, &e, false);
                self.push(base.clone());
            }
            // peer `@memset(a[0..n], v)` — bulk fill; 0/-1 use libc memset.
            ("sys_arr_memset", 3)
                if args[0].ty == Ty::ArrI64 && args[1].ty == Ty::I64 && args[2].ty == Ty::I64 =>
            {
                self.note_store(args[0].root, None);
                self.note_ptr_store(args[0].root, Ty::I64);
                let n = self.conv(&args[1], Ty::I64)?;
                let v = self.conv(&args[2], Ty::I64)?;
                self.line(&format!(
                    "sup_arr_memset_i64({}, {}, {n}, {v});",
                    args[0].a, args[0].b
                ));
                self.push(args[0].clone());
            }
            ("sys_i64_merge", 5) if args[0].ty == Ty::ArrI64 && args[1].ty == Ty::ArrI64 => {
                let lo = self.conv(&args[2], Ty::I64)?;
                let mid = self.conv(&args[3], Ty::I64)?;
                let hi = self.conv(&args[4], Ty::I64)?;
                self.note_store(args[0].root, None);
                self.note_store(args[1].root, None);
                self.line(&format!(
                    "sup_i64_merge({}, {}, {}, {}, {lo}, {mid}, {hi});",
                    args[0].a, args[0].b, args[1].a, args[1].b
                ));
                self.push(args[0].clone());
            }
            // sys_arr_truncate(arr, n) -> arr with length min(len, max(0,n))
            ("sys_arr_truncate", 2)
                if matches!(
                    args[0].ty,
                    Ty::ArrI64 | Ty::ArrF64 | Ty::ArrStr | Ty::ArrBool | Ty::ArrU8
                ) =>
            {
                self.note_store(args[0].root, None);
                let n = self.conv(&args[1], Ty::I64)?;
                let len = &args[0].b;
                self.line(&format!(
                    "{{ int64_t __tn = {n}; if (__tn < 0) __tn = 0; if ((uint64_t)__tn < (uint64_t){len}) {len} = (int64_t)__tn; }}"
                ));
                let mut out = args[0].clone();
                // Length may shrink; drop constant-size knowledge.
                out.cst = None;
                self.push(out);
            }
            // sys_i64_any_nz(arr) -> 1 if any element != 0 else 0
            ("sys_i64_any_nz", 1) if args[0].ty == Ty::ArrI64 => {
                let v = self.fresh("anz");
                self.line(&format!(
                    "int64_t {v} = 0; for (int64_t __i = 0; __i < {len}; ++__i) {{ if ({ptr}[__i] != 0) {{ {v} = 1; break; }} }}",
                    len = args[0].b,
                    ptr = args[0].a,
                ));
                self.push(V::new(Ty::I64, v));
            }
            // sys_i64_gather(dst, src, idx) -> dst  (dst[i] = src[idx[i]])
            ("sys_i64_gather", 3)
                if args[0].ty == Ty::ArrI64
                    && args[1].ty == Ty::ArrI64
                    && args[2].ty == Ty::ArrI64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_i64_gather({}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[1].b, args[2].a, args[2].b
                ));
                self.push(args[0].clone());
            }
            // Dense: idx in-range — no OOB branch (filter/join take_dense).
            ("sys_i64_gather_dense", 3)
                if args[0].ty == Ty::ArrI64
                    && args[1].ty == Ty::ArrI64
                    && args[2].ty == Ty::ArrI64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_i64_gather_dense({}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[1].b, args[2].a, args[2].b
                ));
                self.push(args[0].clone());
            }
            ("sys_f64_gather", 3)
                if args[0].ty == Ty::ArrF64
                    && args[1].ty == Ty::ArrF64
                    && args[2].ty == Ty::ArrI64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_f64_gather({}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[1].b, args[2].a, args[2].b
                ));
                self.push(args[0].clone());
            }
            ("sys_f64_gather_dense", 3)
                if args[0].ty == Ty::ArrF64
                    && args[1].ty == Ty::ArrF64
                    && args[2].ty == Ty::ArrI64 =>
            {
                self.note_store(args[0].root, None);
                self.line(&format!(
                    "sup_f64_gather_dense({}, {}, {}, {}, {}, {});",
                    args[0].a, args[0].b, args[1].a, args[1].b, args[2].a, args[2].b
                ));
                self.push(args[0].clone());
            }
            _ => {
                // No inline lowering: boxed host call, result unknown.
                self.note_impure();
                self.fx.leak = true;
                let n = args.len();
                let arr = self.fresh("hargs");
                self.line(&format!("SuperVal {arr}[{}];", n.max(1)));
                for (i, a) in args.iter().enumerate() {
                    let b = self.box_val(a)?;
                    self.line(&format!("{arr}[{i}] = {b};"));
                }
                let name = escape(&raw);
                let v = self.eager(Ty::Dyn, &format!("super_host(\"{name}\", {arr}, {n})"));
                self.push(v);
            }
        }
        Ok(())
    }

    // ----- returns / terminators --------------------------------------------------------

    /// `*m{i} = l{i};` write-backs of `@` value parameters before a `return`; `None` while a
    /// parameter's class is still pending this pass (the path is dropped).
    fn ret_epilogue(&mut self) -> Res<Option<String>> {
        let mut epilogue = String::new();
        let writebacks = self.mut_value_params.clone();
        for (i, ty) in writebacks {
            if self.st.locals[i] == Ty::Bot {
                // Pending callee / field widening this pass; a later pass resolves it.
                self.saw_bot = true;
                self.live = false;
                return Ok(None);
            }
            if self.st.locals[i] != ty {
                return self.err("`@` scalar parameter changed class");
            }
            let (n, _) = names(&format!("l{i}"), ty);
            let _ = write!(epilogue, "*m{i} = {n}; ");
        }
        Ok(Some(epilogue))
    }

    /// `fail msg` (always followed by `Return`): leave the message in `sup_err_msg` and return a
    /// placeholder value; callers branch on the captured status before using the result.
    fn do_fail(&mut self) -> Res<()> {
        let m = self.pop()?;
        if m.ty == Ty::Bot {
            self.saw_bot = true;
            self.live = false;
            return Ok(());
        }
        if self.is_module {
            return self.err("`fail` in module code");
        }
        if m.ty != Ty::Str {
            return self.err(format!("`fail` with a {:?} message", m.ty));
        }
        for f in &mut self.loops {
            f.early_exit = true;
        }
        let Some(epilogue) = self.ret_epilogue()? else {
            return Ok(());
        };
        self.fails = true;
        self.note_impure();
        self.line(&format!("sup_err_msg = {}; {epilogue}{FAIL_RET}", m.a));
        self.live = false;
        Ok(())
    }

    /// `IsAbsent` (does not pop): `none` / error test of the top of stack.
    fn is_absent(&mut self) -> Res<()> {
        let v = self.vs.last().cloned().ok_or("IsAbsent on empty stack")?;
        let r = if !v.ferr.is_empty() {
            V::new(Ty::Bool, format!("({} != nullptr)", v.ferr))
        } else {
            match v.ty {
                Ty::Bot => {
                    self.saw_bot = true;
                    V::bot()
                }
                Ty::Top => return self.err("IsAbsent on a value-less result"),
                Ty::Dyn => V::new(
                    Ty::Bool,
                    format!(
                        "({0}.tag == SUPER_TAG_NULL || {0}.tag == SUPER_TAG_ERR)",
                        v.a
                    ),
                ),
                // `none` / `()` is absent; every unboxed value class is present.
                Ty::Unit => {
                    let mut c = V::from_cst(Cst::B(true));
                    c.probe = true;
                    c
                }
                _ => {
                    let mut c = V::from_cst(Cst::B(false));
                    c.probe = true;
                    c
                }
            }
        };
        self.push(r);
        Ok(())
    }

    fn do_return(&mut self) -> Res<()> {
        if self.is_module
            && let Some(rv) = self.module_exit.take()
        {
            for f in &mut self.loops {
                f.early_exit = true;
            }
            self.line(&format!("/*@ARENA@*/return {rv};"));
            self.ret_cur = Ty::Unit;
            self.plan.insts[self.inst].exit_super_val = true;
            self.live = false;
            return Ok(());
        }
        let mut v = if self.vs.is_empty() {
            V::unit()
        } else {
            self.pop()?
        };
        if self.is_module && !v.ty.is_scalar() && v.ty != Ty::Bot {
            v = V::unit();
        }
        if v.ty == Ty::Bot || v.ty == Ty::ArrEmpty {
            self.saw_bot = true;
            self.live = false;
            return Ok(());
        }
        for f in &mut self.loops {
            f.early_exit = true;
        }
        // Conflicting classes (`if c { s.x = 1 }` as the last statement: struct ⊔ null) make
        // the result value-less: callers see `Top`, and any real use of it boxes the caller.
        // `none` beside one value class is an optional (`T?`) result instead.
        let (joined, none) = ret_join(self.ret_cur, v.ty, !self.is_module);
        if joined == Ty::Range {
            return self.err("returns a range");
        }
        self.ret_cur = joined;
        let Some(epilogue) = self.ret_epilogue()? else {
            return Ok(());
        };
        // `return f()` of a fallible call re-raises the callee's status.
        let reraise = (!self.is_module && !v.ferr.is_empty()).then(|| v.ferr.clone());
        if reraise.is_some() {
            self.fails = true;
            self.nones |= v.fnone;
            self.note_impure();
        }
        if none {
            self.nones = true;
            self.note_impure();
            if v.ty == Ty::Unit {
                let status = match &reraise {
                    Some(e) => format!("({e} ? {e} : sup_none_msg)"),
                    None => "sup_none_msg".into(),
                };
                self.line(&format!("sup_err_msg = {status}; {epilogue}{FAIL_RET}"));
                self.live = false;
                return Ok(());
            }
        }
        if let Some(e) = reraise {
            self.line(&format!("sup_err_msg = {e};"));
        }
        // A boxed result returned beside typed results is unboxed to the joined class.
        let v = if v.ty == Ty::Dyn && joined != Ty::Dyn && joined.from_dyn() {
            let base = self.fresh("t");
            let code = self.unbox_dyn_into(&v.a, &base, joined, false)?;
            self.line(code.trim_end());
            let (a, b) = names(&base, joined);
            if joined.is_arr() {
                V::arr(joined, a, b, UNKNOWN_ROOT)
            } else {
                V::new(joined, a)
            }
        } else {
            v
        };
        match joined {
            Ty::Unit | Ty::Top => {
                if !epilogue.is_empty() {
                    let e = epilogue.trim_end().to_string();
                    self.line(&e);
                }
                self.line("/*@ARENA@*/return;");
            }
            t if t.is_scalar() => {
                let e = self.conv(&v, t)?;
                if epilogue.is_empty() {
                    self.line(&format!("/*@ARENA@*/return {e};"));
                } else {
                    let tmp = self.fresh("ret");
                    self.declare(&tmp, &t.cty(), t.zero());
                    self.line(&format!("{tmp} = {e}; {epilogue}/*@ARENA@*/return {tmp};"));
                }
            }
            t @ (Ty::Str | Ty::Struct(_) | Ty::Dyn) => {
                if v.ty != t {
                    return self.err("return class mismatch");
                }
                if !epilogue.is_empty() {
                    let tmp = self.fresh("ret");
                    self.declare(&tmp, &t.cty(), t.zero());
                    self.line(&format!(
                        "{tmp} = {}; {epilogue}/*@ARENA@*/return {tmp};",
                        v.a
                    ));
                } else {
                    self.line(&format!("/*@ARENA@*/return {};", v.a));
                }
            }
            t if t.is_arr() => {
                if v.ty != t {
                    return self.err("array return class mismatch");
                }
                if self.is_module {
                    return self.err("module returns an array");
                }
                let mut pre = epilogue.clone();
                let _ = write!(pre, "*ret_len = {};", v.b);
                let elem = t.c_elem();
                self.line(&format!(
                    "{{ {pre} return {KEEP_RET}{}, {}, {}); }}",
                    v.a,
                    v.b,
                    elem.trim_end()
                ));
            }
            _ => return self.err("unsupported return class"),
        }
        self.live = false;
        Ok(())
    }

    // ----- the walker -------------------------------------------------------------------

    fn step(&mut self, pc: usize) -> Res<()> {
        let op = self.chunk.ops[pc].clone();
        match op {
            Op::Nop => {}
            Op::LoadConst(c) => {
                let v = self.const_v(pc, c)?;
                self.push(v);
            }
            Op::LoadLocal(s) => {
                let mut v = self.load_local(s)?;
                if v.ty == Ty::Str && self.append_slots.contains(&s) {
                    if append_run(self.chunk, &self.targets, pc, self.end).is_some() {
                        v.app = Some(s);
                    } else {
                        self.disown(s);
                    }
                }
                self.push(v);
            }
            Op::StoreLocal(s) => {
                let v = self.pop()?;
                self.store_local_at(pc, s, v)?;
            }
            Op::Move(d, s) => {
                let v = self.load_local(s)?;
                if v.ty == Ty::Str {
                    self.disown(s);
                }
                self.store_local(d, v)?;
            }
            Op::LoadGlobal(sym) => self.load_global(sym)?,
            Op::StoreGlobal(sym) => self.store_global(sym)?,
            Op::Add => self.binary(BinK::Add)?,
            Op::Sub => self.binary(BinK::Sub)?,
            Op::Mul => self.binary(BinK::Mul)?,
            Op::Div => self.binary(BinK::Div)?,
            Op::Rem => self.binary(BinK::Rem)?,
            Op::Eq => self.binary(BinK::Eq)?,
            Op::Ne => self.binary(BinK::Ne)?,
            Op::Lt => self.binary(BinK::Lt)?,
            Op::Le => self.binary(BinK::Le)?,
            Op::Gt => self.binary(BinK::Gt)?,
            Op::Ge => self.binary(BinK::Ge)?,
            Op::AddImm(n) | Op::SubImm(n) | Op::MulImm(n) => {
                self.push(V::from_cst(Cst::I(n)));
                let k = match op {
                    Op::AddImm(_) => BinK::Add,
                    Op::SubImm(_) => BinK::Sub,
                    _ => BinK::Mul,
                };
                self.binary(k)?;
            }
            Op::ShlImm(n) => {
                self.push(V::from_cst(Cst::I(n as i64)));
                self.bit_binary(&Op::Shl)?;
            }
            Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => {
                self.bit_binary(&op)?
            }
            Op::BitNot => {
                let a = self.pop()?;
                if a.ty == Ty::Bot {
                    self.saw_bot = true;
                    self.push(V::bot());
                } else if !matches!(a.ty, Ty::I64 | Ty::Bool) {
                    return self.err("bitwise not of a non-integer");
                } else if let Some(c) = a.cst {
                    self.push(V::from_cst(Cst::I(!c.as_i())));
                } else {
                    let mut v = V::new(Ty::I64, format!("(~{})", self.num_expr(&a, false)));
                    v.deps = a.deps.clone();
                    self.push(v);
                }
            }
            Op::Neg => {
                let a = self.pop()?;
                match (a.ty, a.cst) {
                    (Ty::Bot, _) => {
                        self.saw_bot = true;
                        self.push(V::bot());
                    }
                    (_, Some(Cst::I(n))) => self.push(V::from_cst(Cst::I(n.wrapping_neg()))),
                    (_, Some(Cst::F(f))) => self.push(V::from_cst(Cst::F(-f))),
                    (Ty::I64, _) | (Ty::F64, _) => {
                        let mut v = V::new(a.ty, format!("(-({}))", a.a));
                        v.deps = a.deps.clone();
                        v.lin = lin_of(&a).map(|l| l.scale(-1));
                        self.push(v);
                    }
                    (t, _) => return self.err(format!("negation of {t:?}")),
                }
            }
            // Unit variants coerce to their `i64` tag: matching / reading the tag of an integer
            // is a plain compare / the integer itself (the VM's `Value::Int` arms).
            Op::IsVariant(tag) => {
                let a = self.pop()?;
                match a.ty {
                    Ty::Bot => {
                        self.saw_bot = true;
                        self.push(V::bot());
                    }
                    Ty::I64 => {
                        let mut v = V::new(Ty::Bool, format!("({} == {tag}LL)", a.a));
                        v.deps = a.deps.clone();
                        self.push(v);
                    }
                    t => return self.err(format!("variant test on {t:?}")),
                }
            }
            Op::SumTag => {
                let a = self.pop()?;
                match a.ty {
                    Ty::Bot => {
                        self.saw_bot = true;
                        self.push(V::bot());
                    }
                    Ty::I64 => self.push(a),
                    t => return self.err(format!("sum tag of {t:?}")),
                }
            }
            Op::Not => {
                let a = self.pop()?;
                if a.ty == Ty::Bot {
                    self.saw_bot = true;
                    self.push(V::bot());
                } else {
                    let t = self.truthy(&a)?;
                    let mut v = V::new(Ty::Bool, format!("(!{t})"));
                    v.deps = a.deps.clone();
                    self.push(v);
                }
            }
            Op::And | Op::OrLog => {
                let b = self.pop()?;
                let a = self.pop()?;
                if a.ty == Ty::Bot || b.ty == Ty::Bot {
                    self.saw_bot = true;
                    self.push(V::bot());
                } else {
                    let (ta, tb) = (self.truthy(&a)?, self.truthy(&b)?);
                    let o = if matches!(op, Op::And) { "&&" } else { "||" };
                    let mut v = V::new(Ty::Bool, format!("({ta} {o} {tb})"));
                    v.deps = merge_deps(&a.deps, &b.deps);
                    self.push(v);
                }
            }
            Op::Jump(t) => self.emit_jump(pc, t as usize, None)?,
            Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => {
                let c = self.pop()?;
                if c.ty == Ty::Bot {
                    self.saw_bot = true;
                    return Ok(());
                }
                if let (true, Some(Cst::B(b))) = (c.probe, c.cst) {
                    // Statically present / absent: only one arm of the `or` is reachable.
                    if b == matches!(op, Op::JumpIfTrue(_)) {
                        self.emit_jump(pc, t as usize, None)?;
                    }
                    return Ok(());
                }
                let tr = self.truthy(&c)?;
                let cond = if matches!(op, Op::JumpIfFalse(_)) {
                    format!("!{tr}")
                } else {
                    tr
                };
                self.emit_jump(pc, t as usize, Some(cond))?;
            }
            Op::JumpIfLt(t) | Op::JumpIfGe(t) | Op::JumpIfEq(t) | Op::JumpIfNe(t) => {
                let k = match op {
                    Op::JumpIfLt(_) => BinK::Lt,
                    Op::JumpIfGe(_) => BinK::Ge,
                    Op::JumpIfEq(_) => BinK::Eq,
                    _ => BinK::Ne,
                };
                self.binary(k)?;
                let c = self.pop()?;
                if c.ty == Ty::Bot {
                    self.saw_bot = true;
                    return Ok(());
                }
                self.emit_jump(pc, t as usize, Some(c.a))?;
            }
            Op::Call(fi, n) => self.call(pc, fi as usize, n as usize, None)?,
            Op::CallMut(fi, n, wb) => self.call(pc, fi as usize, n as usize, Some(wb as usize))?,
            Op::CallHost(sym, n) => self.host_call(pc, sym, n as usize)?,
            Op::Return | Op::Halt => self.do_return()?,
            Op::Pop => {
                self.pop()?;
            }
            Op::Dup => {
                let v = self.vs.last().cloned().ok_or("dup on empty stack")?;
                // Only one copy of a grown array may keep its spare capacity.
                let mut dup = v;
                dup.c.clear();
                self.push(dup);
            }
            Op::MakeArray(n) => self.make_array(pc, n as usize)?,
            Op::MakeStruct(name, n) => self.make_struct(name, n as usize)?,
            Op::GetField(f) => self.get_field(f)?,
            Op::SetField(f) => self.set_field(pc, f)?,
            Op::IndexGet => self.index_get(false)?,
            Op::LoadSliceUnchecked => self.index_get(true)?,
            Op::IndexSet => self.index_set(false)?,
            Op::StoreSliceUnchecked => self.index_set(true)?,
            Op::MakeRange => {
                let e = self.pop()?;
                let s = self.pop()?;
                if s.ty == Ty::Bot || e.ty == Ty::Bot {
                    self.saw_bot = true;
                    self.push(V::bot());
                } else {
                    let (lo, hi) = (self.idx_expr(&s)?, self.idx_expr(&e)?);
                    let mut v = V::new(Ty::Range, lo);
                    v.b = hi;
                    // `cst` = lo bound; `lin` (constant-only) = hi bound for trip-count proofs.
                    v.cst = s.cst.map(|c| Cst::I(c.as_i()));
                    if let Some(Cst::I(h)) = e.cst {
                        v.lin = Some(Lin::constant(h));
                    }
                    v.deps = merge_deps(&s.deps, &e.deps);
                    self.push(v);
                }
            }
            Op::ParEachBegin(_) => self.par_next = true,
            Op::IterBegin => self.iter_begin(pc)?,
            Op::IterNext(done) => self.iter_next(pc, done as usize)?,
            Op::IterPop => {
                if self.st.iters.pop().is_none() {
                    return self.err("IterPop without iterator");
                }
            }
            Op::PushIt => self.push_it()?,
            Op::PushIndex => self.push_index()?,
            Op::IterSetIt => self.iter_set_it()?,
            Op::IsAbsent => self.is_absent()?,
            Op::Fail => self.do_fail()?,
            Op::OrElse => {
                // Reached only on the present path of `or err`: the value passes through.
                let v = self.pop()?;
                match v.ty {
                    Ty::Bot => {
                        self.saw_bot = true;
                        self.push(V::bot());
                    }
                    Ty::Top => return self.err("OrElse on a value-less result"),
                    _ => {
                        let mut v = v;
                        v.ferr.clear();
                        self.push(v);
                    }
                }
            }
            Op::Print => {
                let v = self.pop()?;
                if v.ty == Ty::Bot {
                    self.saw_bot = true;
                    self.push(V::unit());
                    return Ok(());
                }
                self.note_impure();
                let boxed = self.box_val(&v)?;
                self.line(&format!("super_print({boxed});"));
                self.push(V::unit());
            }
            Op::AssertEq => {
                let b = self.pop()?;
                let a = self.pop()?;
                if a.ty == Ty::Bot || b.ty == Ty::Bot {
                    self.saw_bot = true;
                    return Ok(());
                }
                self.note_impure();
                self.push(a);
                self.push(b);
                self.binary(BinK::Eq)?;
                let c = self.pop()?;
                self.line(&format!(
                    "if (!({})) {{ fprintf(stderr, \"assert_eq failed\\n\"); abort(); }}",
                    c.a
                ));
                self.push(V::from_cst(Cst::B(true)));
            }
            other => return self.err(format!("unsupported op {other:?}")),
        }
        Ok(())
    }

    fn load_global(&mut self, sym: super_core::SymId) -> Res<()> {
        let idx = sym.index();
        if self.plan.banned.contains(&idx) {
            // Written by boxed code: read through the boxed mirror and unbox.
            let ty = self.plan.globals.get(&idx).map(|g| g.ty).unwrap_or(Ty::Bot);
            let src = format!("super_globals[{idx}]");
            let expr = match ty {
                Ty::I64 => format!("super_as_i({src})"),
                Ty::F64 => format!("super_as_f({src})"),
                Ty::Bool => format!("(super_truthy({src}) != 0)"),
                Ty::Str => format!("(({src}).as.s)"),
                Ty::Bot => {
                    self.saw_bot = true;
                    self.push(V::bot());
                    return Ok(());
                }
                _ => {
                    self.push(V::top());
                    return Ok(());
                }
            };
            let v = self.eager(ty, &expr);
            self.push(v);
            return Ok(());
        }
        let Some(g) = self.plan.globals.get(&idx).cloned() else {
            self.saw_bot = true;
            self.push(V::bot());
            return Ok(());
        };
        match g.ty {
            Ty::Bot => {
                self.saw_bot = true;
                self.push(V::bot());
            }
            Ty::Top | Ty::Range => self.push(V::top()),
            Ty::Unit => self.push(V::unit()),
            ty => {
                if let (Some(c), true) = (g.cst, ty.is_scalar()) {
                    self.push(V::from_cst(c));
                    return Ok(());
                }
                if self.plan.ptr_capable(ty) {
                    self.fx.gread = true;
                }
                let gname = self.plan.global_name(idx, sym);
                let base = self.fresh("t");
                let (pa, pb) = self.declare_pair(&base, ty);
                self.assign(&pa, &gname);
                if ty.is_arr() {
                    self.assign(&pb, &format!("{gname}_n"));
                    self.push(V::arr(ty, pa, pb, GLOBAL_ROOT + idx));
                } else {
                    self.push(V::new(ty, pa));
                }
            }
        }
        Ok(())
    }

    fn store_global(&mut self, sym: super_core::SymId) -> Res<()> {
        let idx = sym.index();
        let v = self.pop()?;
        let banned = self.plan.banned.contains(&idx);
        if v.ty == Ty::Bot {
            self.saw_bot = true;
            return Ok(());
        }
        if !self.is_module {
            self.note_impure();
            if self.plan.holds_ptr(v.ty) {
                self.fx.leak = true;
            }
        }
        if v.ty == Ty::ArrEmpty {
            self.saw_bot = true;
            return Ok(());
        }
        if self.plan.holds_arr(v.ty) && !self.is_module {
            return self.err("array stored into a global outside module initialization");
        }
        if let Ty::Struct(id) = v.ty
            && !self.plan.boxable(id)
        {
            return self.err("struct global has no boxed mirror");
        }
        if v.ty == Ty::Dyn {
            return self.err("boxed value stored into a global");
        }
        let n = self.plan.store_counts.get(&idx).copied().unwrap_or(0);
        let cst = if self.is_module && n == 1 && v.ty.is_scalar() {
            v.cst
        } else {
            None
        };
        if banned && !(v.ty.is_scalar() || v.ty == Ty::Str) {
            return self.err("global written by boxed code holds a non-scalar");
        }
        let g = self.plan.globals.entry(idx).or_insert(GInfo {
            ty: Ty::Bot,
            cst: None,
            mirrored_store_seen: false,
        });
        let before = (g.ty, g.cst.map(|c| c.lit()));
        let nty = g.ty.join(v.ty);
        g.ty = nty;
        // The single store site is re-lowered every pass and round: the constant survives as
        // long as each visit stores the same one.
        g.cst = match (banned, g.mirrored_store_seen, g.cst) {
            (true, _, _) => None,
            (false, false, _) => cst,
            (false, true, old) if same_cst(old, cst) => old,
            _ => None,
        };
        g.mirrored_store_seen = true;
        let after = (g.ty, g.cst.map(|c| c.lit()));
        if before != after {
            self.plan.changed = true;
        }
        let g = self.plan.globals[&idx].clone();
        if matches!(g.ty, Ty::Top | Ty::Range) {
            return self.err(format!(
                "global `{}` holds conflicting classes ({:?} after {:?})",
                self.plan.interner.resolve(sym),
                v.ty,
                before.0
            ));
        }
        let gname = self.plan.global_name(idx, sym);
        match g.ty {
            Ty::Unit => {}
            ty if ty.is_arr() => {
                self.line(&format!("{gname} = {};", v.a));
                self.line(&format!("{gname}_n = {};", v.b));
                // Boxed readers see a header that shares the same storage.
                match ty {
                    Ty::ArrI64 => self.line(&format!(
                        "super_globals[{idx}] = sup_view_i64({gname}, {gname}_n);"
                    )),
                    Ty::ArrF64 => self.line(&format!(
                        "super_globals[{idx}] = sup_view_f64({gname}, {gname}_n);"
                    )),
                    _ => {}
                }
            }
            ty if banned => {
                let e = if ty.is_scalar() {
                    self.conv(&v, ty)?
                } else {
                    v.a.clone()
                };
                let boxed = box_expr_of(ty, &e);
                self.line(&format!("super_globals[{idx}] = {boxed};"));
            }
            ty => {
                let e = if ty.is_scalar() {
                    self.conv(&v, ty)?
                } else {
                    v.a.clone()
                };
                self.line(&format!("{gname} = {e};"));
                // Mirror for boxed readers.
                let boxed = match ty {
                    Ty::I64 => format!("super_int({gname})"),
                    Ty::F64 => format!("super_float({gname})"),
                    Ty::Bool => format!("super_bool({gname})"),
                    Ty::Struct(id) => format!("sup_box_s{id}({gname})"),
                    _ => format!("super_str({gname})"),
                };
                self.line(&format!("super_globals[{idx}] = {boxed};"));
            }
        }
        Ok(())
    }

    /// One linear pass over the instance's ops with the current merge targets.
    fn run_pass(&mut self) -> Res<()> {
        self.out.clear();
        self.vars.clear();
        self.bufs.clear();
        self.next_target.clear();
        self.loops.clear();
        self.omp.clear();
        self.omp_notes.clear();
        self.par_next = false;
        self.iter_root.clear();
        self.saw_bot = false;
        self.callees.clear();
        self.dyn_calls.clear();
        self.impure = false;
        self.writes_mem = false;
        self.uses_arena = false;
        self.has_calls = false;
        self.has_loops = false;
        self.tmp_id = 0;
        self.ret_cur = self.plan.insts[self.inst].ret;
        self.fails = false;
        self.nones = false;
        self.fx = PtrFx::default();
        self.mut_value_params.clear();
        self.param_vars.clear();
        self.vs.clear();
        self.live = true;
        self.st = State::new(self.nslots);
        self.setup_params()?;
        let mut pc = self.start;
        while pc < self.end {
            if self.targets.contains(&pc) {
                self.enter_label(pc)?;
            }
            let at_close = self.loops.last().is_some_and(|f| f.close == pc);
            if !self.live {
                if at_close {
                    self.close_loop()?;
                    pc += 1;
                    continue;
                }
                if matches!(self.chunk.ops[pc], Op::IterBegin)
                    && let Some(t) = iter_next_after(self.chunk, pc)
                    && let Op::IterNext(done) = self.chunk.ops[t]
                {
                    pc = done as usize;
                    continue;
                }
                pc += 1;
                continue;
            }
            if at_close {
                self.close_loop()?;
                pc += 1;
                continue;
            }
            self.step(pc)?;
            pc += 1;
        }
        if self.targets.contains(&self.end) {
            self.enter_label(self.end)?;
        }
        if self.live {
            if !self.loops.is_empty() {
                return self.err("unterminated loop at end of function");
            }
            self.do_return()?;
        }
        self.apply_omp();
        Ok(())
    }

    fn setup_params(&mut self) -> Res<()> {
        let params = self.plan.insts[self.inst].params.clone();
        let mut sig: Vec<String> = Vec::new();
        for (i, ty) in params.iter().enumerate() {
            let is_mut = self.mut_param.get(i).copied().unwrap_or(false);
            self.st.locals[i] = *ty;
            let base = format!("l{i}");
            match *ty {
                Ty::Unit => {}
                t if t.is_scalar() || t == Ty::Str || t.is_struct() => {
                    let (n, _) = names(&base, t);
                    if is_mut {
                        // Always shadow-copy `@` value params (`Body`, `World2D`, scalars).
                        // Pointer-aliasing flat structs as `(*mN)` regresses physics resolve
                        // under Protocol/quiet (~73ms vs ~57ms) despite looking cheaper in C.
                        sig.push(format!("{}*restrict m{i}", t.cty()));
                        let (vn, _) = self.declare_pair(&base, t);
                        self.mut_value_params.push((i, t));
                        let deref = format!("*m{i}");
                        self.line(&format!("{vn} = {deref};"));
                    } else {
                        sig.push(format!("{}{n}", t.cty()));
                        self.param_vars.insert(n);
                    }
                }
                t if t.is_arr() => {
                    let (n, len) = names(&base, t);
                    // Always `restrict`: Super's ownership / borrow model proves these
                    // pointers are disjoint from other params, which unlocks SROA +
                    // vectorization in GCC/Clang.
                    sig.push(format!("{}restrict {n}", t.cty()));
                    sig.push(format!("int64_t {len}"));
                    self.param_vars.insert(n.clone());
                    self.param_vars.insert(len.clone());
                    self.st.lroot[i] = i as u32 + 1;
                    // Borrowed storage: no spare capacity (a push must copy).
                    let cap = cap_name(&n);
                    self.declare(&cap, "int64_t ", "0");
                    self.line(&format!("{cap} = {len};"));
                    let _ = is_mut;
                }
                t => return self.err(format!("parameter class {t:?}")),
            }
        }
        // Array results come back through a length out-parameter.
        if self.plan.insts[self.inst].ret.is_arr() {
            sig.push("int64_t *restrict ret_len".to_string());
        }
        self.plan.insts[self.inst].sig_params = sig.join(", ");
        Ok(())
    }

    /// Resolve nested pragmas (outermost wins) and substitute markers in the output.
    fn apply_omp(&mut self) {
        let heads: Vec<usize> = self.omp.keys().copied().collect();
        for h in heads {
            let (pragma, mut parent) = self.omp[&h].clone();
            let mut text = String::new();
            if let Some(p) = pragma {
                let mut suppressed = false;
                while let Some(ph) = parent {
                    match self.omp.get(&ph) {
                        Some((Some(_), _)) => {
                            suppressed = true;
                            break;
                        }
                        Some((None, pp)) => parent = *pp,
                        None => break,
                    }
                }
                if !suppressed {
                    let mut indent = String::from("    ");
                    for _ in 0..self.depth_of(h) {
                        indent.push_str("  ");
                    }
                    text = format!("{indent}{p}\n");
                }
            } else if let Some(reason) = self.omp_notes.get(&h) {
                text = format!("    /* par: sequential — {reason} */\n");
            }
            let marker = format!("/*@OMP{h}@*/\n");
            self.out = self.out.replace(&marker, &text);
        }
    }

    fn depth_of(&self, head: usize) -> usize {
        let mut d = 0;
        let mut cur = self.omp.get(&head).and_then(|(_, p)| *p);
        while let Some(p) = cur {
            d += 1;
            cur = self.omp.get(&p).and_then(|(_, pp)| *pp);
        }
        d
    }
}

// ---------------------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------------------

/// Drop `L_n:;` lines no `goto L_n;` refers to (structured loops consume most branch targets).
fn strip_unused_labels(body: &str) -> String {
    let used: HashSet<&str> = body
        .match_indices("goto L_")
        .filter_map(|(i, m)| {
            let rest = &body[i + m.len() - 2..];
            rest.find(';').map(|end| &rest[..end])
        })
        .collect();
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let label = line.trim().strip_suffix(":;");
        if label.is_some_and(|l| l.starts_with("L_") && !used.contains(l)) {
            continue;
        }
        out.push_str(line);
    }
    out
}

fn struct_c_name(id: u32) -> String {
    format!("sup_s{id}_t")
}

/// C member for Super field `name` (C keywords gain a trailing `_`, like the ABI typedefs).
fn c_member(name: &str) -> String {
    crate::c_keywords::c_ident(name)
}

fn len_member(member: &str) -> String {
    format!("{member}__n")
}

fn names(base: &str, ty: Ty) -> (String, String) {
    match ty {
        Ty::Range => (format!("{base}_rlo"), format!("{base}_rhi")),
        Ty::Struct(id) => (format!("{base}_S{id}"), String::new()),
        Ty::ArrStruct(id) => (format!("{base}_aS{id}"), format!("{base}_aS{id}_n")),
        t if t.is_arr() => (
            format!("{base}_{}", t.suffix()),
            format!("{base}_{}_n", t.suffix()),
        ),
        t => (format!("{base}_{}", t.suffix()), String::new()),
    }
}

/// Capacity variable that accompanies the array pointer variable `ptr`.
fn cap_name(ptr: &str) -> String {
    format!("{ptr}_c")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Index expression inside a struct-element lvalue `ptr[idx]` (no nested brackets).
fn elem_lv_index(a: &str) -> Option<&str> {
    let start = a.rfind('[')?;
    let end = a.rfind(']')?;
    if end + 1 != a.len() || end <= start + 1 {
        return None;
    }
    Some(&a[start + 1..end])
}

fn box_expr_of(ty: Ty, e: &str) -> String {
    match ty {
        Ty::I64 => format!("super_int({e})"),
        Ty::F64 => format!("super_float({e})"),
        Ty::Bool => format!("super_bool({e})"),
        Ty::Str => format!("super_str({e})"),
        _ => "SUPER_NULL".to_string(),
    }
}

fn fn_boxed_name(chunk: &Chunk, interner: &Interner, fi: usize) -> String {
    crate::c_emit::function_c_name(chunk, interner, fi)
}

fn inst_c_name(inst: &Inst, id: usize) -> String {
    format!("super_u_{}_{id}", inst.name)
}

/// Pull `cty` for parameter `name` out of a comma-separated C parameter list.
fn param_cty_from_sig(sig: &str, name: &str) -> Option<String> {
    for part in sig.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut toks = part.split_whitespace().collect::<Vec<_>>();
        if toks.last().copied() != Some(name) {
            continue;
        }
        toks.pop();
        // Drop `restrict` from the stored field type (frame field is a plain value / pointer).
        toks.retain(|t| *t != "restrict");
        if toks.is_empty() {
            return None;
        }
        let mut cty = toks.join(" ");
        cty.push(' ');
        return Some(cty);
    }
    None
}

/// `[start, end)` op range of function `fi`.
pub(super) fn fn_range(chunk: &Chunk, fi: usize) -> (usize, usize) {
    let start = chunk.functions[fi].entry as usize;
    let end = chunk
        .functions
        .iter()
        .map(|of| of.entry as usize)
        .filter(|&e| e > start)
        .min()
        .unwrap_or_else(|| match chunk.ops.first() {
            Some(Op::Jump(t)) if (*t as usize) > start => *t as usize,
            _ => chunk.ops.len(),
        });
    let end = match chunk.ops.first() {
        Some(Op::Jump(t)) if (*t as usize) > start && (*t as usize) < end => *t as usize,
        _ => end,
    };
    (start, end)
}

/// `[start, end)` op range of the module's top-level code.
pub(super) fn module_range(chunk: &Chunk) -> (usize, usize) {
    let start = match chunk.ops.first() {
        Some(Op::Jump(t)) => (*t as usize).min(chunk.ops.len()),
        _ => 0,
    };
    (start, chunk.ops.len())
}

/// The `IterNext` that heads the loop opened by the `IterBegin` at `pc`. The optimizer may
/// hoist loop-invariant straight-line ops between the two; anything that branches or touches
/// iterator state in between means the loop is not in structured form.
fn iter_next_after(chunk: &Chunk, pc: usize) -> Option<usize> {
    let mut t = pc + 1;
    loop {
        match chunk.ops.get(t)? {
            Op::IterNext(_) => return Some(t),
            Op::Jump(_)
            | Op::JumpIfFalse(_)
            | Op::JumpIfTrue(_)
            | Op::JumpIfLt(_)
            | Op::JumpIfGe(_)
            | Op::JumpIfEq(_)
            | Op::JumpIfNe(_)
            | Op::LoopStep { .. }
            | Op::IterBegin
            | Op::IterPop
            | Op::Return
            | Op::Halt => return None,
            _ => t += 1,
        }
    }
}

fn successors(chunk: &Chunk, pc: usize) -> Vec<usize> {
    match &chunk.ops[pc] {
        Op::Jump(t) => vec![*t as usize],
        Op::JumpIfFalse(t)
        | Op::JumpIfTrue(t)
        | Op::JumpIfLt(t)
        | Op::JumpIfGe(t)
        | Op::JumpIfEq(t)
        | Op::JumpIfNe(t)
        | Op::IterNext(t) => vec![*t as usize, pc + 1],
        Op::LoopStep { body, .. } => vec![*body as usize, pc + 1],
        Op::Return | Op::Halt => vec![],
        _ => vec![pc + 1],
    }
}

/// `LoadLocal(s)` at `pc` opens `x = x + e₁ + e₂ …` when the loaded value stays the left
/// operand of each `Add` that consumes it and the sum goes straight back to `s`, with no
/// branch, join point, other access to `s`, or op beyond plain loads / arithmetic / calls in
/// between. An interpolation's leading `"" +` is transparent. Returns the `StoreLocal` pc.
fn append_run(chunk: &Chunk, targets: &HashSet<usize>, pc: usize, end: usize) -> Option<usize> {
    let Op::LoadLocal(s) = chunk.ops[pc] else {
        return None;
    };
    let empty_prefix = pc > 0
        && matches!(chunk.ops[pc - 1], Op::LoadConst(c)
            if matches!(chunk.consts.get(c as usize), Some(Value::Str(t)) if t.is_empty()));
    // `adds`: `out = out + …`. `grows`: `out = push_ascii(out, …)` / other host builders that
    // take `out` as arg0 and store back — same owned-buffer opportunity.
    let (mut depth, mut adds, mut grows) = (1i32, 0usize, 0usize);
    for q in pc + 1..end.min(pc + 256) {
        if targets.contains(&q) {
            return None;
        }
        let (pops, pushes) = match &chunk.ops[q] {
            Op::StoreLocal(d) if *d == s => {
                return (depth == 1 && (adds > 0 || grows > 0)).then_some(q);
            }
            Op::Add if q == pc + 1 && empty_prefix => continue,
            Op::Add if depth == 2 => {
                adds += 1;
                depth = 1;
                continue;
            }
            Op::LoadLocal(d) if *d == s => return None,
            Op::LoadConst(_) | Op::LoadLocal(_) | Op::LoadGlobal(_) => (0, 1),
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
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::Shl
            | Op::Shr
            | Op::UShr
            | Op::IndexGet => (2, 1),
            Op::Neg
            | Op::Not
            | Op::BitNot
            | Op::AddImm(_)
            | Op::SubImm(_)
            | Op::MulImm(_)
            | Op::ShlImm(_)
            | Op::GetField(_)
            | Op::LoadFieldOff(_) => (1, 1),
            // `CallMut` has the same stack effect as `Call`; writebacks touch `@` receivers, not
            // the append local, so `out = out + f(@self, …)` still grows `out` in place.
            // When arity equals depth, `s` is still arg0 (`out = push_ascii(out, code)`): treat
            // like `Add` — consume args and leave the grown value (depth 1). The generic
            // `depth - pops < 1` check below would reject this case (pops == depth).
            Op::Call(_, n) | Op::CallHost(_, n) | Op::CallMut(_, n, _)
                if depth == i32::from(*n) =>
            {
                grows += 1;
                depth = 1;
                continue;
            }
            Op::Call(_, n) | Op::CallHost(_, n) | Op::CallMut(_, n, _) => (i32::from(*n), 1),
            _ => return None,
        };
        if depth - pops < 1 {
            return None;
        }
        depth += pushes - pops;
    }
    None
}

fn op_writes(chunk: &Chunk, pc: usize) -> Vec<u16> {
    match &chunk.ops[pc] {
        Op::StoreLocal(s) => vec![*s],
        Op::Move(d, _) => vec![*d],
        Op::CallMut(_, _, wb) => chunk
            .mut_writebacks
            .get(*wb as usize)
            .map(|l| l.iter().map(|(s, _)| *s).collect())
            .unwrap_or_default(),
        _ => vec![],
    }
}

fn op_reads(chunk: &Chunk, pc: usize) -> Option<u16> {
    match &chunk.ops[pc] {
        Op::LoadLocal(s) => Some(*s),
        Op::Move(_, s) => Some(*s),
        Op::LoopStep { counter, .. } => Some(*counter),
        _ => None,
    }
}

/// Slots that may be read before being (re)defined within one iteration of `[lo, hi)`.
fn exposed_reads(chunk: &Chunk, lo: usize, hi: usize, nslots: usize) -> HashSet<u16> {
    let mut exposed = HashSet::new();
    if lo >= hi {
        return exposed;
    }
    let n = hi - lo;
    let mut ins: Vec<Option<Vec<bool>>> = vec![None; n];
    ins[0] = Some(vec![false; nslots]);
    let mut work = vec![lo];
    while let Some(pc) = work.pop() {
        let Some(cur) = ins[pc - lo].clone() else {
            continue;
        };
        let mut out = cur;
        for w in op_writes(chunk, pc) {
            if let Some(b) = out.get_mut(w as usize) {
                *b = true;
            }
        }
        for s in successors(chunk, pc) {
            if s < lo || s >= hi {
                continue;
            }
            let slot = &mut ins[s - lo];
            match slot {
                None => {
                    *slot = Some(out.clone());
                    work.push(s);
                }
                Some(prev) => {
                    let mut changed = false;
                    for i in 0..nslots {
                        if prev[i] && !out[i] {
                            prev[i] = false;
                            changed = true;
                        }
                    }
                    if changed {
                        work.push(s);
                    }
                }
            }
        }
    }
    for pc in lo..hi {
        if let (Some(r), Some(state)) = (op_reads(chunk, pc), &ins[pc - lo])
            && !state.get(r as usize).copied().unwrap_or(false)
        {
            exposed.insert(r);
        }
    }
    exposed
}

/// Is `slot` possibly read after `from` before being redefined (reachability scan)?
fn slot_live_after(chunk: &Chunk, from: usize, slot: u16, limit: usize, _nslots: usize) -> bool {
    let mut seen = HashSet::new();
    let mut work = vec![from];
    while let Some(pc) = work.pop() {
        if pc >= limit || pc >= chunk.ops.len() || !seen.insert(pc) {
            continue;
        }
        if op_reads(chunk, pc) == Some(slot) {
            return true;
        }
        if op_writes(chunk, pc).contains(&slot) {
            continue;
        }
        work.extend(successors(chunk, pc));
    }
    false
}

// ---------------------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------------------

/// Result of whole-program planning, consumed by [`super::emit_c_module`].
pub(super) struct Planned {
    /// `typedef struct sup_s{id}_t` for every unboxed value struct (precedes `decls`).
    pub types: String,
    /// Typed global declarations + forward declarations of unboxed instances.
    pub decls: String,
    /// Definitions of reachable unboxed instances.
    pub defs: String,
    /// `Some(wrapper)` when module top-level code was lowered to unboxed C.
    pub module_wrapper: Option<String>,
    /// Functions that still need a boxed (`SuperVal`) body.
    pub dyn_fns: Vec<usize>,
    /// Human-readable reasons for each boxed fallback (`/* … */` banner lines).
    pub notes: Vec<String>,
    /// Incremental TU: `(file stem, function definition C)`.
    pub inst_units: Vec<(String, String)>,
}

fn lower_inst(plan: &mut Plan<'_>, id: usize) -> Res<Lowered> {
    let mut lw = Lw::new(plan, id);
    let mut passes = 0;
    loop {
        passes += 1;
        if passes > MAX_PASSES {
            return Err("flow analysis did not converge".into());
        }
        lw.run_pass()?;
        // Merge this pass's arrivals into the persistent targets (monotone widening).
        let mut changed = false;
        let keys: Vec<usize> = lw.next_target.keys().copied().collect();
        for k in keys {
            let nxt = lw.next_target[&k].clone();
            match lw.target_state.get(&k).cloned() {
                Some(prev) => {
                    let j = prev.join(&nxt)?;
                    if !j.same(&prev) {
                        lw.target_state.insert(k, j);
                        changed = true;
                    }
                }
                None => {
                    lw.target_state.insert(k, nxt);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let leaf_pure = !lw.has_calls
        && !lw.has_loops
        && !lw.impure
        && !lw.writes_mem
        && lw.vars.keys().all(|n| !n.contains("_a"))
        && lw.plan.insts[id].params.iter().all(|p| p.is_scalar())
        && lw.ret_cur.is_scalar();
    let ret = lw.ret_cur;
    let uses_arena = lw.uses_arena;
    let mut decls = String::new();
    for (n, (cty, init)) in &lw.vars {
        if lw.param_vars.contains(n) {
            continue;
        }
        let _ = writeln!(decls, "    {cty}{n} = {init};");
    }
    for (n, (cty, len)) in &lw.bufs {
        let _ = writeln!(decls, "    alignas(64) {cty} {n}[{len}];");
    }
    let mut body = strip_unused_labels(&std::mem::take(&mut lw.out));
    if body.contains(FAIL_RET) {
        body = body.replace(FAIL_RET, &absent_return(ret));
    }
    if uses_arena {
        body = body.replace("/*@ARENA@*/", "g_sup_stack.cursor = sup_mark; ");
        body = body.replace(KEEP_RET, "SUP_KEEP1(sup_mark, ");
        decls.push_str("    size_t sup_mark = g_sup_stack.cursor;\n");
    } else {
        body = body.replace("/*@ARENA@*/", "");
        body = body.replace(KEEP_RET, "SUP_KEEP0(");
    }
    let exit_super_val = lw.plan.insts[id].exit_super_val;
    Ok(Lowered {
        body,
        decls,
        prologue: String::new(),
        ret,
        callees: std::mem::take(&mut lw.callees),
        dyn_calls: std::mem::take(&mut lw.dyn_calls),
        saw_bot: lw.saw_bot,
        impure: lw.impure,
        writes_mem: lw.writes_mem,
        uses_arena,
        leaf_pure,
        has_loops: lw.has_loops,
        nops: lw.end - lw.start,
        param_vars: std::mem::take(&mut lw.param_vars),
        sig_params: lw.plan.insts[id].sig_params.clone(),
        exit_super_val,
        fails: lw.fails,
        nones: lw.nones,
        fx: std::mem::take(&mut lw.fx),
    })
}

/// Unresolved instances to box first: those whose unresolved callees all lead back to them
/// (leaves and bottom call cycles). Callers that were only waiting on such a callee are left
/// alone so the next round can retry them with a boxed call.
fn unresolved_bottom(plan: &Plan<'_>) -> Vec<usize> {
    let open: Vec<usize> = (0..plan.insts.len())
        .filter(|&k| plan.insts[k].failed.is_none() && plan.insts[k].saw_bot)
        .collect();
    let set: HashSet<usize> = open.iter().copied().collect();
    let reach = |k: usize| -> HashSet<usize> {
        let mut seen = HashSet::new();
        let mut work: Vec<usize> = plan.insts[k].callees.iter().copied().collect();
        while let Some(j) = work.pop() {
            if set.contains(&j) && seen.insert(j) {
                work.extend(plan.insts[j].callees.iter().copied());
            }
        }
        seen
    };
    let reaches: HashMap<usize, HashSet<usize>> = open.iter().map(|&k| (k, reach(k))).collect();
    open.iter()
        .copied()
        .filter(|k| reaches[k].iter().all(|j| j == k || reaches[j].contains(k)))
        .collect()
}

/// Run the monomorphization fixpoint rooted at module code; returns `None` when the module
/// itself cannot be unboxed.
fn fixpoint(plan: &mut Plan<'_>) -> bool {
    let root = plan.add_inst(None, vec![]);
    let mut rounds = 0;
    loop {
        rounds += 1;
        if rounds > MAX_ROUNDS {
            plan.insts[root].failed = Some("monomorphization did not converge".into());
            return false;
        }
        plan.changed = false;
        let mut i = 0;
        while i < plan.insts.len() {
            if plan.insts[i].failed.is_some() {
                i += 1;
                continue;
            }
            match lower_inst(plan, i) {
                Err(e) => {
                    plan.insts[i].failed = Some(e);
                    plan.changed = true;
                }
                Ok(l) => {
                    let inst = &mut plan.insts[i];
                    let (nret, none) = ret_join(inst.ret, l.ret, inst.fi.is_some());
                    let nones = inst.nones || l.nones || none;
                    let fails = inst.fails || l.fails;
                    let mut ch = nret != inst.ret || nones != inst.nones || fails != inst.fails;
                    inst.ret = nret;
                    inst.nones = nones;
                    inst.fails = fails;
                    let (ni, nw, na) = (
                        inst.impure || l.impure,
                        inst.writes_mem || l.writes_mem,
                        inst.uses_arena || l.uses_arena,
                    );
                    ch |= ni != inst.impure || nw != inst.writes_mem;
                    inst.impure = ni;
                    inst.writes_mem = nw;
                    inst.uses_arena = na;
                    ch |= inst.callees != l.callees;
                    inst.body = l.body;
                    inst.decls = l.decls;
                    inst.prologue = l.prologue;
                    inst.callees = l.callees;
                    inst.dyn_calls = l.dyn_calls;
                    inst.saw_bot = l.saw_bot;
                    inst.leaf_pure = l.leaf_pure;
                    inst.has_loops = l.has_loops;
                    inst.nops = l.nops;
                    inst.param_vars = l.param_vars;
                    inst.sig_params = l.sig_params;
                    inst.fx = l.fx;
                    ch |= l.exit_super_val && !inst.exit_super_val;
                    inst.exit_super_val |= l.exit_super_val;
                    // Effects propagate through unboxed callees.
                    let callee_effects: Vec<(bool, bool)> = plan.insts[i]
                        .callees
                        .iter()
                        .map(|c| (plan.insts[*c].impure, plan.insts[*c].writes_mem))
                        .collect();
                    let inst = &mut plan.insts[i];
                    for (ci, cw) in callee_effects {
                        if (ci && !inst.impure) || (cw && !inst.writes_mem) {
                            ch = true;
                        }
                        inst.impure |= ci;
                        inst.writes_mem |= cw;
                    }
                    if ch {
                        plan.changed = true;
                    }
                }
            }
            if plan.insts[i].failed.is_some() && i == root {
                return false;
            }
            i += 1;
        }
        if plan.changed {
            continue;
        }
        // Stable: unresolved ("Bot") flows or callee classes still unknown ⇒ boxed fallback.
        // First retry once with reads of never-widened `[]` arrays resolved to `null`.
        if !plan.empty_reads_zero
            && plan
                .insts
                .iter()
                .any(|inst| inst.failed.is_none() && inst.saw_bot)
        {
            plan.empty_reads_zero = true;
            continue;
        }
        let doomed = unresolved_bottom(plan);
        if !doomed.is_empty() {
            for k in doomed {
                plan.insts[k].failed = Some("unresolved value class (never assigned)".into());
            }
            continue;
        }
        // Functions that never return keep `ret = Bot`: treat as unit.
        let mut fixed = false;
        for k in 0..plan.insts.len() {
            if plan.insts[k].failed.is_none() && plan.insts[k].ret == Ty::Bot {
                plan.insts[k].ret = Ty::Unit;
                fixed = true;
            }
        }
        if fixed {
            continue;
        }
        break;
    }
    plan.insts[root].failed.is_none()
}

fn reachable(plan: &Plan<'_>) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut work = vec![0usize];
    while let Some(i) = work.pop() {
        if plan.insts[i].failed.is_some() || !seen.insert(i) {
            continue;
        }
        work.extend(plan.insts[i].callees.iter().copied());
    }
    seen.into_iter().collect()
}

/// Functions reachable (through any call edge) from `seeds`, in ascending index order.
fn call_closure(chunk: &Chunk, seeds: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut set = BTreeSet::new();
    let mut work: Vec<usize> = seeds.iter().copied().collect();
    while let Some(fi) = work.pop() {
        if fi >= chunk.functions.len() || !set.insert(fi) {
            continue;
        }
        let (s, e) = fn_range(chunk, fi);
        for pc in s..e.min(chunk.ops.len()) {
            match &chunk.ops[pc] {
                Op::Call(f, _) | Op::CallMut(f, _, _) | Op::Spawn(f) => work.push(*f as usize),
                _ => {}
            }
        }
    }
    set
}

fn char_table() -> String {
    let mut s = String::from("static const char SUP_CHR_TBL[256][2] = {");
    for c in 0..256u32 {
        if c % 16 == 0 {
            s.push_str("\n  ");
        }
        if c == 0 {
            s.push_str("{0,0},");
        } else {
            let _ = write!(s, "{{{c},0}},");
        }
    }
    s.push_str("\n};\n");
    s
}

pub(super) fn unboxed_prelude() -> String {
    let mut s = String::from(UNBOXED_PRELUDE);
    s.push_str(&char_table());
    s.push_str(UNBOXED_PRELUDE_TAIL);
    s
}

/// Plan the whole module. Always returns a usable [`Planned`]; when the module cannot be
/// unboxed `module_wrapper` is `None` and every reachable function is listed in `dyn_fns`.
pub(super) fn plan(chunk: &Chunk, interner: &Interner, outline_stmt_threshold: usize) -> Planned {
    let mut banned: HashSet<u32> = HashSet::new();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let mut p = Plan::new(chunk, interner, banned.clone());
        let ok = fixpoint(&mut p);
        let reach = if ok { reachable(&p) } else { vec![] };
        // Functions needing boxed bodies.
        let mut seeds: BTreeSet<usize> = BTreeSet::new();
        if ok {
            for &i in &reach {
                seeds.extend(p.insts[i].dyn_calls.iter().copied());
            }
        } else {
            let (s, e) = module_range(chunk);
            for pc in s..e {
                match &chunk.ops[pc] {
                    Op::Call(f, _) | Op::CallMut(f, _, _) | Op::Spawn(f) => {
                        seeds.insert(*f as usize);
                    }
                    _ => {}
                }
            }
        }
        // First-class fn / lambda values (MakeFn / MakeClosure) must stay boxed so
        // CallValue can dispatch them from AOT.
        for op in &chunk.ops {
            match op {
                Op::MakeFn(f) | Op::MakeClosure(f, _) => {
                    seeds.insert(*f as usize);
                }
                _ => {}
            }
        }
        let dyn_set = call_closure(chunk, &seeds);
        // Globals touched by boxed code cannot be typed statics.
        let mut new_ban = HashSet::new();
        if ok {
            for &fi in &dyn_set {
                let (s, e) = fn_range(chunk, fi);
                for pc in s..e.min(chunk.ops.len()) {
                    if let Op::StoreGlobal(sym) = &chunk.ops[pc] {
                        new_ban.insert(sym.index());
                    }
                }
            }
            new_ban.retain(|b| !banned.contains(b));
        }
        if !new_ban.is_empty() && attempts < 8 {
            banned.extend(new_ban);
            continue;
        }
        return finish(p, ok, reach, dyn_set, outline_stmt_threshold);
    }
}

/// Flat C layouts of every inferred value struct. Ids are dense and a nested struct is
/// always registered before its parent literal, so id order is a valid definition order.
fn struct_typedefs(plan: &Plan<'_>) -> String {
    let mut out = String::new();
    if plan.structs.is_empty() {
        return out;
    }
    let _ = writeln!(out, "/* --- unboxed value structs --- */");
    // Array-of-struct fields point at structs that may be registered later (a `[]` field
    // widens after its first push), so every tag is declared up front.
    let any_ptr = plan
        .structs
        .iter()
        .any(|d| d.fields.iter().any(|f| matches!(f.ty, Ty::ArrStruct(_))));
    if any_ptr {
        for id in 0..plan.structs.len() {
            let cname = struct_c_name(id as u32);
            let _ = writeln!(out, "typedef struct {cname} {cname};");
        }
    }
    for (id, def) in plan.structs.iter().enumerate() {
        let cname = struct_c_name(id as u32);
        let label = def.label.as_deref().unwrap_or("record");
        let _ = writeln!(out, "typedef struct {cname} {{ /* {} */", escape(label));
        let mut members = 0;
        for f in &def.fields {
            if f.ty == Ty::ArrEmpty {
                continue;
            }
            members += 1;
            let _ = writeln!(out, "  {}{};", f.ty.cty(), f.member);
            if f.ty.is_arr() {
                let _ = writeln!(out, "  int64_t {};", len_member(&f.member));
            }
        }
        if members == 0 {
            let _ = writeln!(out, "  char sup_empty_;");
        }
        let _ = writeln!(out, "}} {cname};");
    }
    out
}

/// Struct ids ordered so every struct follows the structs its helpers call
/// (by-value fields and array-of-struct fields).
fn struct_helper_order(plan: &Plan<'_>) -> Vec<u32> {
    fn visit(plan: &Plan<'_>, id: u32, seen: &mut [bool], out: &mut Vec<u32>) {
        if std::mem::replace(&mut seen[id as usize], true) {
            return;
        }
        for f in &plan.structs[id as usize].fields {
            if let Ty::Struct(c) | Ty::ArrStruct(c) = f.ty {
                visit(plan, c, seen, out);
            }
        }
        out.push(id);
    }
    let mut seen = vec![false; plan.structs.len()];
    let mut out = Vec::with_capacity(plan.structs.len());
    for id in 0..plan.structs.len() as u32 {
        visit(plan, id, &mut seen, &mut out);
    }
    out
}

/// `sup_box_s{id}` / `sup_unbox_s{id}` bridges between value structs and boxed objects
/// (printing, boxed callees, `@` write-back from boxed code, global mirrors), plus
/// `sup_box_as{id}` snapshot boxing of struct arrays.
fn struct_helpers(plan: &Plan<'_>) -> String {
    let mut out = String::new();
    for id in struct_helper_order(plan) {
        let def = &plan.structs[id as usize];
        let cname = struct_c_name(id);
        if plan.boxable(id) {
            let label = def
                .label
                .as_deref()
                .map(|l| format!("\"{}\"", escape(l)))
                .unwrap_or_else(|| "NULL".into());
            let _ = writeln!(out, "static inline SuperVal sup_box_s{id}({cname} v) {{");
            let _ = writeln!(
                out,
                "  SuperObj *o = super_obj_alloc({}, 0, {label});",
                def.fields.len()
            );
            for f in &def.fields {
                let m = format!("v.{}", f.member);
                let boxed = match f.ty {
                    Ty::Struct(c) => format!("sup_box_s{c}({m})"),
                    Ty::ArrI64 => format!("sup_view_i64({m}, v.{})", len_member(&f.member)),
                    Ty::ArrF64 => format!("sup_view_f64({m}, v.{})", len_member(&f.member)),
                    Ty::ArrStr => format!("sup_box_astr({m}, v.{})", len_member(&f.member)),
                    Ty::ArrBool => format!("sup_box_ab({m}, v.{})", len_member(&f.member)),
                    Ty::ArrU8 => format!("sup_box_au({m}, v.{})", len_member(&f.member)),
                    Ty::ArrStruct(c) => {
                        format!("sup_box_as{c}({m}, v.{})", len_member(&f.member))
                    }
                    Ty::ArrEmpty => "super_arr(super_arr_alloc(0))".into(),
                    t => box_expr_of(t, &m),
                };
                let _ = writeln!(out, "  super_obj_put(o, \"{}\", {boxed});", escape(&f.name));
            }
            let _ = writeln!(out, "  return super_obj_val(o);\n}}");
            let _ = writeln!(
                out,
                "static inline SuperVal sup_box_as{id}(const {cname} *p, int64_t n) {{"
            );
            let _ = writeln!(out, "  size_t len = n > 0 ? (size_t)n : 0;");
            let _ = writeln!(out, "  SuperArr *a = super_arr_alloc(len);");
            let _ = writeln!(out, "  if (!a) abort();");
            let _ = writeln!(
                out,
                "  for (size_t k = 0; k < len; ++k) a->items[k] = sup_box_s{id}(p[k]);"
            );
            let _ = writeln!(out, "  a->len = len;");
            let _ = writeln!(out, "  return super_arr(a);\n}}");
        }
        if plan.unboxable(id) {
            let _ = writeln!(out, "static inline {cname} sup_unbox_s{id}(SuperVal o) {{");
            let _ = writeln!(out, "  {cname} r = {{}};");
            for f in &def.fields {
                let src = format!("super_obj_get(o, \"{}\")", escape(&f.name));
                let n = format!("&r.{}", len_member(&f.member));
                let e = match f.ty {
                    Ty::I64 => format!("super_as_i({src})"),
                    Ty::F64 => format!("super_as_f({src})"),
                    Ty::Bool => format!("(super_truthy({src}) != 0)"),
                    Ty::Struct(c) => format!("sup_unbox_s{c}({src})"),
                    Ty::ArrI64 => format!("sup_unbox_ai({src}, {n})"),
                    Ty::ArrF64 => format!("sup_unbox_af({src}, {n})"),
                    Ty::ArrStr => format!("sup_unbox_astr({src}, {n})"),
                    Ty::ArrBool => format!("sup_unbox_ab({src}, {n})"),
                    Ty::ArrU8 => format!("sup_unbox_au({src}, {n})"),
                    Ty::ArrStruct(c) => format!("sup_unbox_as{c}({src}, {n})"),
                    Ty::ArrEmpty => continue,
                    _ => format!("sup_unbox_str({src})"),
                };
                let _ = writeln!(out, "  r.{} = {e};", f.member);
            }
            let _ = writeln!(out, "  return r;\n}}");
            let _ = writeln!(
                out,
                "static inline {cname} *sup_unbox_as{id}(SuperVal v, int64_t *n) {{"
            );
            let _ = writeln!(out, "  SuperArr *a = sup_boxed_arr(v);");
            let _ = writeln!(out, "  *n = a ? (int64_t)a->len : 0;");
            let _ = writeln!(out, "  if (!a) return nullptr;");
            let _ = writeln!(
                out,
                "  {cname} *p = ({cname} *)malloc((a->len ? a->len : 1) * sizeof *p);"
            );
            let _ = writeln!(out, "  if (!p) abort();");
            let _ = writeln!(
                out,
                "  for (size_t k = 0; k < a->len; ++k) p[k] = a->kind == 0 ? sup_unbox_s{id}(a->items[k]) : ({cname}){{}};"
            );
            let _ = writeln!(out, "  return p;\n}}");
        }
    }
    out
}

/// Allocation helpers that draw from `sup_ralloc` (the region stack while a region is open).
const REGION_ALLOCS: &[&str] = &[
    "sup_heap_zeroed(",
    "sup_grow(",
    "sup_push_copy(",
    "sup_cat_",
    "sup_str_slice(",
    "sup_str_at(",
    "sup_sapp",
];

/// Frames whose allocations — their own and every callee's — are all dead once they return:
/// no pointer can reach a global, boxed or host code, an `@` parameter, or an array the caller
/// can see, nothing fails with a built message, and the result holds no pointer (or is a
/// string, which the bracket compacts down to the frame mark).
fn region_modes(plan: &Plan<'_>, reach: &[usize]) -> HashMap<usize, RegionMode> {
    let n = plan.insts.len();
    let cap: Vec<bool> = plan
        .insts
        .iter()
        .map(|inst| inst.params.iter().any(|&t| plan.ptr_capable(t)))
        .collect();
    let (mut leak, mut gread, mut pstore, mut mutw, mut allocs) = (
        vec![false; n],
        vec![false; n],
        vec![false; n],
        vec![false; n],
        vec![false; n],
    );
    for &i in reach {
        let inst = &plan.insts[i];
        leak[i] = inst.fx.leak || !inst.dyn_calls.is_empty() || inst.fi.is_none();
        gread[i] = inst.fx.gread;
        pstore[i] = cap[i] && (inst.fx.pstore || inst.fx.ustore);
        mutw[i] = inst.fx.mutw;
        allocs[i] = REGION_ALLOCS.iter().any(|p| inst.body.contains(p));
    }
    loop {
        let mut changed = false;
        for &i in reach {
            let inst = &plan.insts[i];
            let callees = || inst.callees.iter().copied().filter(|&c| c < n);
            let callee_pstore = callees().any(|c| pstore[c]);
            let nl = leak[i]
                || callees().any(|c| leak[c])
                || (gread[i] && (inst.fx.ustore || inst.fx.astore || callee_pstore));
            let ng = gread[i] || callees().any(|c| gread[c]);
            let np = pstore[i] || (cap[i] && callee_pstore);
            let nm = mutw[i]
                || inst.fx.mutw_calls.iter().any(|&c| c < n && mutw[c])
                || inst.fx.mutw_if_alloc.iter().any(|&c| c >= n || allocs[c]);
            let na = allocs[i] || callees().any(|c| allocs[c]);
            changed |= (nl, ng, np, nm, na) != (leak[i], gread[i], pstore[i], mutw[i], allocs[i]);
            (leak[i], gread[i], pstore[i], mutw[i], allocs[i]) = (nl, ng, np, nm, na);
        }
        if !changed {
            break;
        }
    }
    let mut modes = HashMap::new();
    for &i in reach {
        let inst = &plan.insts[i];
        if pstore[i] || !allocs[i] {
            continue;
        }
        // `mutw` blocks frames that take pointer params (caller-visible `@` aliases).
        // Paramless entrypoints may mutate locals (e.g. `@rng`) safely inside a region.
        if mutw[i] && !inst.params.is_empty() {
            continue;
        }
        // A formatted `fail` message may point into this frame's allocation
        // region through `sup_err_msg`; keep the frame off scoped regions.
        if inst.fails {
            continue;
        }
        let mode = match inst.ret {
            Ty::Str if !leak[i] => RegionMode::KeepStr,
            // Paramless scalar entrypoints (e.g. `pipeline() : i64`):
            // - ignore transitive host-leak / Dyn-call `fx.leak` (boxed helpers heap-alloc;
            //   the entrypoint still bump-allocs Frame work that should die on leave)
            // - still require a real function id (not the module shim)
            // - Dyn return is OK when the source type was `i64` but a boxed helper forced
            //   SuperVal arithmetic in the body
            Ty::I64 | Ty::F64 | Ty::Bool | Ty::Dyn
                if inst.params.is_empty() && inst.fi.is_some() && !pstore[i] =>
            {
                RegionMode::Scoped
            }
            Ty::I64 | Ty::F64 | Ty::Bool | Ty::Unit if !leak[i] && !mutw[i] => {
                // Unit allowed only when not already covered; prefer i64 entrypoints.
                // Nested enter is a no-op sentinel so outer+inner wrappers stay cheap;
                // only the outermost leave rewinds (nested rewind regressed HTTP router ~50%).
                RegionMode::Scoped
            }
            _ => continue,
        };
        modes.insert(i, mode);
    }
    modes
}

/// Parameter names of a C parameter list (`int64_t l0_i, const char *l1_s` → `l0_i, l1_s`).
fn sig_param_names(sig: &str) -> String {
    sig.split(',')
        .filter_map(|p| {
            let p = p.trim();
            let start = p
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(0, |k| k + 1);
            (!p.is_empty() && p != "void").then(|| &p[start..])
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `name(...)` bracketing `name_rb(...)` with `sup_region_enter` / `sup_region_leave`.
fn region_wrapper(sig: &str, name: &str, inst: &Inst, mode: RegionMode) -> String {
    let args = sig_param_names(&inst.sig_params);
    let call = format!("{name}_rb({args})");
    let mut w = format!("{sig} {{\n    size_t sup_rm = sup_region_enter();\n");
    match (inst.ret, mode) {
        (_, RegionMode::KeepStr) => {
            let _ = writeln!(w, "    const char *r = {call};");
            let _ = writeln!(w, "    return sup_region_keep_str(sup_rm, r);");
        }
        (Ty::Unit | Ty::Bot, _) => {
            let _ = writeln!(w, "    {call};");
            let _ = writeln!(w, "    sup_region_leave(sup_rm);");
        }
        (t, _) => {
            let _ = writeln!(w, "    {}r = {call};", t.cty());
            let _ = writeln!(w, "    sup_region_leave(sup_rm);");
            let _ = writeln!(w, "    return r;");
        }
    }
    w.push_str("}\n\n");
    w
}

fn finish(
    plan: Plan<'_>,
    ok: bool,
    reach: Vec<usize>,
    dyn_set: BTreeSet<usize>,
    outline_stmt_threshold: usize,
) -> Planned {
    let mut notes = Vec::new();
    for inst in &plan.insts {
        if let Some(reason) = &inst.failed {
            notes.push(format!(
                "/* boxed fallback: {} — {} */",
                inst.name,
                reason.replace("*/", "* /")
            ));
        }
    }
    notes.sort();
    notes.dedup();
    if !ok {
        return Planned {
            types: String::new(),
            decls: String::new(),
            defs: String::new(),
            module_wrapper: None,
            dyn_fns: dyn_set.into_iter().collect(),
            notes,
            inst_units: Vec::new(),
        };
    }
    let types = struct_typedefs(&plan);
    let mut decls = format!("{UNBOX_ARR_HELPERS}{}", struct_helpers(&plan));
    let mut defs = String::new();
    let mut inst_units = Vec::new();
    // Typed globals.
    let _ = writeln!(decls, "/* --- typed module globals --- */");
    for (idx, g) in &plan.globals {
        let sym = super_core::SymId::from_index(*idx);
        let Some(sym) = sym else { continue };
        if plan.banned.contains(idx) {
            continue;
        }
        let name = plan.global_name(*idx, sym);
        match g.ty {
            ty if ty.is_arr() => {
                let _ = writeln!(decls, "static {}{name} = nullptr;", ty.cty());
                let _ = writeln!(decls, "static int64_t {name}_n = 0;");
            }
            ty if ty.is_scalar() || ty == Ty::Str || ty.is_struct() => {
                let _ = writeln!(decls, "static {}{name} = {};", ty.cty(), ty.zero());
            }
            _ => {}
        }
    }
    let _ = writeln!(decls, "/* --- unboxed instances --- */");
    let modes = region_modes(&plan, &reach);
    // Region frames: the instance body is emitted as `{name}_rb` behind the bracketing wrapper.
    let body_sig = |i: usize| -> String {
        let inst = &plan.insts[i];
        let ret = match inst.ret {
            Ty::Unit | Ty::Bot => "void ".to_string(),
            t => t.cty().to_string(),
        };
        let params = if inst.sig_params.is_empty() {
            "void".to_string()
        } else {
            inst.sig_params.clone()
        };
        format!("static inline {ret}{}_rb({params})", inst_c_name(inst, i))
    };
    let sig_of = |i: usize| -> String {
        let inst = &plan.insts[i];
        let ret = if inst.ret.is_arr() {
            inst.ret.cty().to_string()
        } else {
            match inst.ret {
                Ty::Unit | Ty::Bot => "void ".into(),
                t => t.cty().to_string(),
            }
        };
        let params = if inst.sig_params.is_empty() {
            "void".to_string()
        } else {
            inst.sig_params.clone()
        };
        let small = inst.nops <= 64 && !inst.has_loops;
        let mut quals = String::from("static ");
        let always = inst
            .fi
            .map(|fi| plan.chunk.functions[fi].always_inline)
            .unwrap_or(false);
        if always || (inst.leaf_pure && small) {
            quals = "static inline __attribute__((always_inline)) ".into();
        }
        let attr = if inst.leaf_pure {
            " [[unsequenced]]"
        } else {
            ""
        };
        format!("{quals}{ret}{}({params}){attr}", inst_c_name(inst, i))
    };
    for &i in &reach {
        if plan.insts[i].fi.is_some() {
            let _ = writeln!(decls, "{};", sig_of(i));
        }
    }
    let _ = writeln!(decls, "static {} super_u_module(void);", {
        if plan.insts[0].exit_super_val {
            "SuperVal".to_string()
        } else {
            match plan.insts[0].ret {
                Ty::Unit | Ty::Bot => "void".to_string(),
                t => t.cty().trim_end().to_string(),
            }
        }
    });
    let mut outline_helpers = String::new();
    for &i in &reach {
        let inst = &plan.insts[i];
        let header = if inst.fi.is_none() {
            let rt = if inst.exit_super_val {
                "SuperVal".to_string()
            } else {
                match inst.ret {
                    Ty::Unit | Ty::Bot => "void".to_string(),
                    t => t.cty().trim_end().to_string(),
                }
            };
            format!("static {rt} super_u_module(void)")
        } else if modes.contains_key(&i) {
            body_sig(i)
        } else {
            sig_of(i)
        };
        let c_name = if inst.fi.is_none() {
            "super_u_module".to_string()
        } else if modes.contains_key(&i) {
            format!("{}_rb", inst_c_name(inst, i))
        } else {
            inst_c_name(inst, i)
        };
        let wrapper = modes
            .get(&i)
            .map(|&m| region_wrapper(&sig_of(i), &inst_c_name(inst, i), inst, m))
            .unwrap_or_default();
        // Frame fields: declared locals + by-value / pointer parameters.
        let mut fields = super::outline::parse_decl_fields(&inst.decls);
        for pv in &inst.param_vars {
            if fields.iter().any(|f| f.name == *pv) {
                continue;
            }
            // Recover a C type from the signature text when possible.
            if let Some(cty) = param_cty_from_sig(&inst.sig_params, pv) {
                fields.push(super::outline::FrameField {
                    cty,
                    name: pv.clone(),
                });
            }
        }
        let (body, helpers) = match super::outline::outline_if_needed(
            &c_name,
            &inst.decls,
            &inst.body,
            &fields,
            outline_stmt_threshold,
        ) {
            Some(ol) => (ol.body, ol.helpers),
            None => (inst.body.clone(), String::new()),
        };
        if !helpers.is_empty() {
            outline_helpers.push_str(&helpers);
            outline_helpers.push('\n');
        }
        let mut unit = format!("{header} {{\n");
        unit.push_str(&inst.decls);
        unit.push_str(&inst.prologue);
        unit.push_str(&body);
        unit.push_str("}\n");
        unit.push_str(&wrapper);
        let _ = writeln!(defs, "{header} {{");
        defs.push_str(&inst.decls);
        defs.push_str(&inst.prologue);
        defs.push_str(&body);
        let _ = writeln!(defs, "}}\n");
        defs.push_str(&wrapper);
        let stem = if let Some(fi) = inst.fi {
            format!("super_u_{fi}")
        } else {
            "super_u_module".into()
        };
        inst_units.push((stem, unit));
    }
    if !outline_helpers.is_empty() {
        // Helpers must appear before the functions that call them.
        defs = format!("{outline_helpers}\n{defs}");
        let _ = writeln!(
            decls,
            "/* --- outlined helpers (forward) — see definitions before instances --- */"
        );
    }
    let wrapper = {
        let m = &plan.insts[0];
        let mut w = String::from("static SuperVal super_module_main(void) {\n");
        if m.exit_super_val {
            // Boxed `+main` SuperVal is the process exit code / status.
            w.push_str("  return super_u_module();\n");
        } else {
            match m.ret {
                Ty::Unit | Ty::Bot => {
                    w.push_str("  super_u_module();\n  return SUPER_NULL;\n");
                }
                Ty::I64 => w.push_str("  return super_int(super_u_module());\n"),
                Ty::F64 => w.push_str("  return super_float(super_u_module());\n"),
                Ty::Bool => w.push_str("  return super_bool(super_u_module());\n"),
                Ty::Str => w.push_str("  return super_str(super_u_module());\n"),
                _ => w.push_str("  (void)super_u_module();\n  return SUPER_NULL;\n"),
            }
        }
        w.push_str("}\n");
        w
    };
    Planned {
        types,
        decls,
        defs,
        module_wrapper: Some(wrapper),
        dyn_fns: dyn_set.into_iter().collect(),
        notes,
        inst_units,
    }
}

const UNBOXED_PRELUDE: &str = r#"
/* --- unboxed helpers (static inline; folded away by cc -O3) --- */
static inline int64_t sup_idiv(int64_t a, int64_t b) [[unsequenced]] {
  if (b == 0) return 0;
  if (b == -1) return (int64_t)(0ull - (uint64_t)a);
  return a / b;
}
static inline int64_t sup_irem(int64_t a, int64_t b) [[unsequenced]] {
  if (b == 0 || b == -1) return 0;
  return a % b;
}
static inline int64_t sup_shl(int64_t a, int64_t b) [[unsequenced]] { return (int64_t)((uint64_t)a << (b & 63)); }
static inline int64_t sup_shr(int64_t a, int64_t b) [[unsequenced]] { return a >> (b & 63); }
static inline int64_t sup_ushr(int64_t a, int64_t b) [[unsequenced]] { return (int64_t)((uint64_t)a >> (b & 63)); }
static inline int64_t sup_iabs(int64_t a) [[unsequenced]] { return a < 0 ? (int64_t)(0ull - (uint64_t)a) : a; }
static inline int64_t sup_max0(int64_t a) [[unsequenced]] { return a < 0 ? 0 : a; }
/* Status of the last fallible (`T!`) / optional (`T?`) unboxed call: nullptr = present,
   sup_none_msg = `none`, anything else = the `fail` message. Callers move it into a local
   right after the call and reset it, so it is always nullptr between calls. */
static const char sup_none_msg[] = "none";
static _Thread_local const char *sup_err_msg = nullptr;
static inline const char *sup_err_text(const char *e) {
  return (e && e != sup_none_msg) ? e : "";
}
static inline int64_t sup_to_i64(double x) [[unsequenced]] {
  if (x != x) return 0;
  if (x >= 9223372036854775807.0) return INT64_MAX;
  if (x <= -9223372036854775808.0) return INT64_MIN;
  return (int64_t)x;
}
static inline int64_t sup_ld_i(const int64_t *p, int64_t n, int64_t i) {
  return (uint64_t)i < (uint64_t)n ? p[i] : 0;
}
static inline double sup_ld_f(const double *p, int64_t n, int64_t i) {
  return (uint64_t)i < (uint64_t)n ? p[i] : 0.0;
}
static inline const char *sup_ld_s(const char *const *p, int64_t n, int64_t i) {
  return (uint64_t)i < (uint64_t)n ? p[i] : "";
}
static inline void sup_st_i(int64_t *p, int64_t n, int64_t i, int64_t v) {
  if ((uint64_t)i < (uint64_t)n) p[i] = v;
}
/* bulk `@memset(a[0..want], v)` — libc memset for 0 and -1 (all-bits-one). */
static inline void sup_arr_memset_i64(int64_t *p, int64_t len, int64_t want, int64_t v) {
  if (!p || want <= 0) return;
  size_t n = (size_t)((want < len) ? want : len);
  if (v == 0) { memset(p, 0, n * sizeof(int64_t)); return; }
  if (v == (int64_t)-1) { memset(p, 0xff, n * sizeof(int64_t)); return; }
  /* Word-fill: unroll ×4 so pathfinding INF clears are not a scalar store loop. */
  size_t i = 0;
  for (; i + 4 <= n; i += 4) {
    p[i] = v; p[i + 1] = v; p[i + 2] = v; p[i + 3] = v;
  }
  for (; i < n; ++i) p[i] = v;
}
static inline void sup_st_f(double *p, int64_t n, int64_t i, double v) {
  if ((uint64_t)i < (uint64_t)n) p[i] = v;
}
static inline void sup_st_s(const char **p, int64_t n, int64_t i, const char *v) {
  if ((uint64_t)i < (uint64_t)n) p[i] = v;
}
static inline bool sup_ld_b(const bool *p, int64_t n, int64_t i) {
  return (uint64_t)i < (uint64_t)n ? p[i] : false;
}
static inline void sup_st_b(bool *p, int64_t n, int64_t i, bool v) {
  if ((uint64_t)i < (uint64_t)n) p[i] = v;
}
static inline int64_t sup_ld_u8(const uint8_t *p, int64_t n, int64_t i) {
  return (uint64_t)i < (uint64_t)n ? (int64_t)p[i] : 0;
}
static inline void sup_st_u8(uint8_t *p, int64_t n, int64_t i, int64_t v) {
  if ((uint64_t)i < (uint64_t)n) p[i] = (uint8_t)(v & 255);
}
/* Fused big-endian u32 store/load on dense u8 buffers (big-endian writeInt/readInt). */
static inline void sup_be32_put_u8(uint8_t *p, int64_t n, int64_t at, int64_t v) {
  if (!p || at < 0 || at + 4 > n) return;
  uint32_t x = (uint32_t)v;
  p[at] = (uint8_t)(x >> 24);
  p[at + 1] = (uint8_t)(x >> 16);
  p[at + 2] = (uint8_t)(x >> 8);
  p[at + 3] = (uint8_t)x;
}
static inline int64_t sup_be32_get_u8(const uint8_t *p, int64_t n, int64_t at) {
  if (!p || at < 0 || at + 4 > n) return 0;
  return ((int64_t)p[at] << 24) | ((int64_t)p[at + 1] << 16) | ((int64_t)p[at + 2] << 8) | (int64_t)p[at + 3];
}
typedef struct { int64_t *p; int64_t n; } sup_ai_t;
static inline sup_ai_t sup_ld_aa(const sup_ai_t *p, int64_t n, int64_t i) {
  sup_ai_t z = {nullptr, 0};
  return (uint64_t)i < (uint64_t)n ? p[i] : z;
}
typedef struct { double *p; int64_t n; } sup_af_t;
static inline sup_af_t sup_ld_aaf(const sup_af_t *p, int64_t n, int64_t i) {
  sup_af_t z = {nullptr, 0};
  return (uint64_t)i < (uint64_t)n ? p[i] : z;
}
/* Region stack (per thread). The compiler brackets calls to frames that provably hand no
   heap pointer back (`sup_region_enter` / `sup_region_leave`); while one is open, unboxed
   string and array storage is bump-allocated here and the whole call's garbage is released
   in O(1) on return. Outside every region `sup_ralloc` is plain `malloc` (never freed). */
typedef struct { uint8_t *base; size_t cur, cap; int64_t depth; bool ready; } SupRegion;
static _Thread_local SupRegion sup_rg;
#ifndef MAP_NORESERVE
#define MAP_NORESERVE 0
#endif
static void sup_region_init(void) {
  sup_rg.ready = true;
#if defined(__linux__) || defined(__APPLE__) || defined(__FreeBSD__)
  for (size_t cap = (size_t)1 << (sizeof(void *) == 8 ? 34 : 28); cap >= ((size_t)16 << 20); cap >>= 2) {
    void *p = mmap(nullptr, cap, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED) continue;
#if defined(MADV_NOHUGEPAGE)
    (void)madvise(p, (size_t)2 << 20, MADV_NOHUGEPAGE);
#endif
    sup_rg.base = (uint8_t *)p;
    sup_rg.cap = cap;
    return;
  }
#endif
}
/* Sentinel from a nested `enter` when an outer region is already open: leave/keep are no-ops
   so inner wrappers cannot rewind the outer bump cursor under live pointers, and hot
   call chains (e.g. HTTP wire build/parse) stay free of nested enter/leave work. */
#define SUP_REGION_NESTED ((size_t)-1)
static inline size_t sup_region_enter(void) {
  if (!sup_rg.ready) sup_region_init();
  if (sup_rg.depth > 0) return SUP_REGION_NESTED;
  sup_rg.depth = 1;
  return sup_rg.cur;
}
static inline void *sup_ralloc(size_t n, size_t align) {
  if (sup_rg.depth > 0 && sup_rg.base) {
    size_t at = (sup_rg.cur + (align - 1)) & ~(align - 1);
    if (at <= sup_rg.cap && n <= sup_rg.cap - at) {
      sup_rg.cur = at + n;
      return sup_rg.base + at;
    }
  }
  void *p = malloc(n ? n : 1);
  if (!p) abort();
  return p;
}
static inline bool sup_in_region(const void *p) {
  uintptr_t b = (uintptr_t)sup_rg.base, q = (uintptr_t)p;
  return b && q >= b && q < b + sup_rg.cap;
}
static inline bool sup_in_goose(const void *p) {
  uintptr_t b = (uintptr_t)g_sup_stack.base, q = (uintptr_t)p;
  return b && q >= b && q < b + g_sup_stack.capacity;
}
/* Grow/copy into a buffer of `nc` elements. In-place grow is allowed only when `p` is the
   *true* topmost bump block: `p + (*cap)*esz == cursor`. Treating "everything from p to cur"
   as one buffer is wrong when several arrays share the region (Expr SoA) — grows would raise
   cap without moving and sibling arrays overlap (strs[i] became 0x5, SIGSEGV).
   Malloc heap uses realloc; never realloc mmap arena pointers. */
static inline void *sup_realloc_copy(const void *p, int64_t n, int64_t nc, int64_t *cap, size_t esz) {
  size_t need = (size_t)nc * esz;
  if (!need) need = 1;
  if (p && *cap > 0 && sup_rg.depth > 0 && sup_rg.base && sup_in_region(p)) {
    uintptr_t q = (uintptr_t)p;
    uintptr_t end = q + (size_t)(*cap) * esz;
    uintptr_t cur = (uintptr_t)sup_rg.base + sup_rg.cur;
    if (end == cur) {
      if (need <= (size_t)(*cap) * esz) {
        *cap = nc;
        return (void *)p;
      }
      size_t extra = need - (size_t)(*cap) * esz;
      if (extra <= (size_t)(sup_rg.cap - sup_rg.cur)) {
        memset((void *)end, 0, extra);
        sup_rg.cur += extra;
        *cap = nc;
        return (void *)p;
      }
    }
  }
  if (p && *cap > 0 && g_sup_stack.base && sup_in_goose(p)) {
    uintptr_t q = (uintptr_t)p;
    uintptr_t end = q + (size_t)(*cap) * esz;
    uintptr_t cur = (uintptr_t)g_sup_stack.base + g_sup_stack.cursor;
    if (end == cur) {
      if (need <= (size_t)(*cap) * esz) {
        *cap = nc;
        return (void *)p;
      }
      size_t extra = need - (size_t)(*cap) * esz;
      if (extra <= g_sup_stack.capacity - g_sup_stack.cursor) {
        memset((void *)end, 0, extra);
        g_sup_stack.cursor += extra;
        *cap = nc;
        return (void *)p;
      }
    }
    /* Not top-of-goose: migrate to malloc (never realloc mmap; never strand bump copies). */
    void *q2 = malloc(need);
    if (!q2) abort();
    if (n > 0) memcpy(q2, p, (size_t)n * esz);
    *cap = nc;
    return q2;
  }
  if (p && !sup_in_region(p) && !sup_in_goose(p)) {
    void *r = realloc((void *)p, need);
    if (!r) abort();
    *cap = nc;
    return r;
  }
  /* Fresh / non-topmost region buffer: malloc so SoA pushes realloc in place later. */
  void *q = malloc(need);
  if (!q) abort();
  if (n > 0 && p) memcpy(q, p, (size_t)n * esz);
  *cap = nc;
  return q;
}
/* In-place append ran out of room (n == cap): move to a buffer of twice the capacity. */
static inline void *sup_grow(const void *p, int64_t n, int64_t *cap, size_t esz) {
  int64_t nc = *cap > 0 ? *cap * 2 : 8;
  if (nc <= n) nc = n * 2 + 8;
  return sup_realloc_copy(p, n, nc, cap, esz);
}
/* `y = push(x, v)`: fresh buffer with room for further appends through `y`. */
static inline void *sup_push_copy(const void *p, int64_t n, int64_t *cap, size_t esz) {
  int64_t nc = (n + 1) * 2;
  if (nc < 8) nc = 8;
  return sup_realloc_copy(p, n, nc, cap, esz);
}
/* Growable arrays (Frame/Expr SoA, filter idx, …) always use the heap — not the bump
   region. Bump-backed arrays strand O(n²) abandoned copies when sibling allocs break
   topmost in-place grow (11GB+/multi-second on bench 13). Strings still use `sup_ralloc`. */
static inline void *sup_heap_zeroed(size_t bytes) {
  void *p = calloc(1, bytes ? bytes : 1);
  if (!p) abort();
  return p;
}
static inline void *sup_arena_zeroed(size_t bytes) {
  sup_stack_init();
  void *p = sup_stack_alloc(&g_sup_stack, bytes ? bytes : 1, 64);
  if (!p) return sup_heap_zeroed(bytes);
  memset(p, 0, bytes ? bytes : 1);
  return p;
}
/* Array result of a frame that allocated on the Goose stack: when it lives above the frame
   mark it is moved down to the (64-byte aligned) mark and stays allocated; everything else
   the frame allocated is released. */
static inline void *sup_arena_keep(size_t mark, void *p, size_t bytes) {
  uintptr_t base = (uintptr_t)g_sup_stack.base, q = (uintptr_t)p;
  if (!base || q < base + mark || q >= base + g_sup_stack.cursor) {
    g_sup_stack.cursor = mark;
    return p;
  }
  size_t dst = (mark + 63) & ~(size_t)63;
  if (base + dst != q) memmove((void *)(base + dst), p, bytes);
  g_sup_stack.cursor = dst + (bytes ? bytes : 1);
  return (void *)(base + dst);
}
#define SUP_KEEP0(p, n, T) (p)
#define SUP_KEEP1(mark, p, n, T) ((T *)sup_arena_keep((mark), (void *)(p), (size_t)(n) * sizeof(T)))
/* Typed Goose-stack views — avoid `uint8_t*` casts at use sites so SROA sees real T*.
   (`restrict` belongs on the callee's parameter / local, not the return type.) */
static inline int64_t *sup_arena_i64(size_t n) {
  return (int64_t *)sup_arena_zeroed(n * sizeof(int64_t));
}
static inline double *sup_arena_f64(size_t n) {
  return (double *)sup_arena_zeroed(n * sizeof(double));
}
static inline const char **sup_arena_str(size_t n) {
  return (const char **)sup_arena_zeroed(n * sizeof(const char *));
}
static inline void sup_view_init_i64(SuperArr *a, int64_t *p, int64_t n) {
  memset(a, 0, sizeof *a);
  a->i64s = p; a->len = (size_t)n; a->cap = (size_t)n; a->rc = (int64_t)1 << 40; a->kind = 2;
}
static inline void sup_view_init_f64(SuperArr *a, double *p, int64_t n) {
  memset(a, 0, sizeof *a);
  a->f64s = p; a->len = (size_t)n; a->cap = (size_t)n; a->rc = (int64_t)1 << 40; a->kind = 1;
}
static inline SuperVal sup_view_i64(int64_t *p, int64_t n) {
  SuperArr *a = (SuperArr *)malloc(sizeof *a);
  if (!a) abort();
  sup_view_init_i64(a, p, n);
  return super_arr(a);
}
static inline SuperVal sup_view_f64(double *p, int64_t n) {
  SuperArr *a = (SuperArr *)malloc(sizeof *a);
  if (!a) abort();
  sup_view_init_f64(a, p, n);
  return super_arr(a);
}
/* Snapshot of a string array for boxed code (strings are immutable, so no view is needed). */
static inline SuperVal sup_box_astr(const char *const *p, int64_t n) {
  size_t len = n > 0 ? (size_t)n : 0;
  SuperArr *a = super_arr_alloc(len);
  if (!a) abort();
  for (size_t k = 0; k < len; ++k) a->items[k] = super_str(p[k] ? p[k] : "");
  a->len = len;
  return super_arr(a);
}
static inline SuperVal sup_box_ab(const bool *p, int64_t n) {
  size_t len = n > 0 ? (size_t)n : 0;
  SuperArr *a = super_arr_alloc(len);
  if (!a) abort();
  for (size_t k = 0; k < len; ++k) a->items[k] = super_bool(p[k]);
  a->len = len;
  return super_arr(a);
}
static inline SuperVal sup_box_au(const uint8_t *p, int64_t n) {
  size_t len = n > 0 ? (size_t)n : 0;
  SuperArr *a = super_arr_alloc(len);
  if (!a) abort();
  for (size_t k = 0; k < len; ++k) a->items[k] = super_int((int64_t)p[k]);
  a->len = len;
  return super_arr(a);
}
/* Boxed → unboxed arrays. A view of unboxed storage (`sup_view_*`) is adopted in place, so
   writes made through it by boxed code stay visible; any other array is copied. */
static inline bool sup_is_view(const SuperArr *a) { return a->rc >= ((int64_t)1 << 40); }
static inline const char *sup_unbox_str(SuperVal v) {
  return (v.tag == SUPER_TAG_STR && v.as.s) ? v.as.s : "";
}
/* `__err_text` of a boxed result: the `fail` message (or a plain string), else "". */
static inline const char *sup_err_text_boxed(SuperVal v) {
  return ((v.tag == SUPER_TAG_ERR || v.tag == SUPER_TAG_STR) && v.as.s) ? v.as.s : "";
}
static inline SuperArr *sup_boxed_arr(SuperVal v) {
  return (v.tag == SUPER_TAG_ARR && v.as.arr) ? v.as.arr : nullptr;
}
static inline const char **sup_unbox_astr(SuperVal v, int64_t *n) {
  SuperArr *a = sup_boxed_arr(v);
  *n = a ? (int64_t)a->len : 0;
  if (!a) return nullptr;
  const char **p = (const char **)malloc((a->len ? a->len : 1) * sizeof *p);
  if (!p) abort();
  for (size_t k = 0; k < a->len; ++k) {
    SuperVal e = a->kind == 0 ? a->items[k] : SUPER_NULL;
    p[k] = (e.tag == SUPER_TAG_STR && e.as.s) ? e.as.s : "";
  }
  return p;
}
static inline bool sup_truthy_str(const char *s) { return s && s[0]; }
static inline bool sup_streq(const char *a, const char *b) { return strcmp(a ? a : "", b ? b : "") == 0; }
static inline int sup_strcmp(const char *a, const char *b) { return strcmp(a ? a : "", b ? b : ""); }
static inline int64_t sup_str_len(const char *s) {
  if (!s) return 0;
  size_t n = strlen(s), cont = 0;
  const unsigned char *p = (const unsigned char *)s;
  for (size_t i = 0; i < n; ++i) cont += (p[i] & 0xC0) == 0x80;
  return (int64_t)(n - cont);
}
/* UTF-8 byte/code-unit length — matches `str` byte length / VM `sys_byte_len`. */
static inline int64_t sup_byte_len(const char *s) { return s ? (int64_t)strlen(s) : 0; }
static inline int64_t sup_ord(const char *s) { return (s && s[0]) ? (int64_t)(unsigned char)s[0] : -1; }
/* O(1) byte/code-unit load — hot path for ASCII JSON/HTTP/CSV scanners (`byte_at`).
   Null-terminated Super strings; index at the terminator yields -1. No strlen. */
static inline int64_t
sup_byte_at(const char *s, int64_t i) {
  if (!s || i < 0) return -1;
  unsigned char c = ((const unsigned char *)s)[(size_t)i];
  return c == 0 ? (int64_t)-1 : (int64_t)c;
}
/* UTF-8 substring of byte range [lo, hi) — pairs with `byte_at` / `json_len` spans. */
static inline const char *sup_byte_slice(const char *s, int64_t lo, int64_t hi) {
  if (!s) return "";
  size_t n = strlen(s);
  size_t a = lo <= 0 ? 0 : ((size_t)lo > n ? n : (size_t)lo);
  size_t b = hi <= 0 ? 0 : ((size_t)hi > n ? n : (size_t)hi);
  if (b <= a) return "";
  size_t len = b - a;
  char *o = (char *)sup_ralloc(len + 1, 1);
  memcpy(o, s + a, len);
  o[len] = 0;
  return o;
}
/* First byte index of `nd` in `s` at/after `from`, or -1 (wire / ASCII scanners).
   No haystack `strlen` — libc `strchr`/`strstr` stop at NUL (HTTP header scans).
   Prefer `sup_byte_find_limit` when the live byte length is known. */
static inline int64_t sup_byte_find(const char *s, const char *nd, int64_t from) {
  if (!s || !nd || !nd[0]) return -1;
  size_t i = from < 0 ? 0 : (size_t)from;
  for (size_t k = 0; k < i; k++) if (!s[k]) return -1;
  if (!nd[1]) {
    const char *p = strchr(s + i, (unsigned char)nd[0]);
    return p ? (int64_t)(p - s) : -1;
  }
  const char *p = strstr(s + i, nd);
  return p ? (int64_t)(p - s) : -1;
}
/* Bounded find: search only in `s[from..limit)` (no haystack `strlen`).
   Uses `memchr` to skip to candidate starts (faster than byte-by-byte `memcmp`). */
static inline int64_t sup_byte_find_limit(const char *s, const char *nd, int64_t from, int64_t limit) {
  if (!s || !nd || !nd[0] || limit <= 0) return -1;
  size_t i = from < 0 ? 0 : (size_t)from;
  size_t lim = (size_t)limit;
  if (i >= lim) return -1;
  size_t m = 0;
  while (nd[m]) m++;
  if (m == 0 || i + m > lim) return -1;
  if (m == 1) {
    const void *p = memchr(s + i, (unsigned char)nd[0], lim - i);
    return p ? (int64_t)((const char *)p - s) : -1;
  }
  const char *base = s + i;
  size_t left = lim - i;
  while (left >= m) {
    const void *hit = memchr(base, (unsigned char)nd[0], left - m + 1);
    if (!hit) return -1;
    const char *p = (const char *)hit;
    if (memcmp(p, nd, m) == 0) return (int64_t)(p - s);
    size_t adv = (size_t)(p - base) + 1;
    base += adv;
    left -= adv;
  }
  return -1;
}
/* `nd` occurs in `s` exactly at byte index `pos` — no haystack `strlen`. */
static inline bool sup_byte_match_at(const char *s, const char *nd, int64_t pos) {
  if (!nd || !nd[0]) return true;
  if (!s || pos < 0) return false;
  size_t m = 0;
  while (nd[m]) m++;
  for (size_t k = 0; k < m; k++) {
    unsigned char c = ((const unsigned char *)s)[(size_t)pos + k];
    if (!c || c != ((const unsigned char *)nd)[k]) return false;
  }
  return true;
}
/* `s[lo..hi)` byte-equals `t` without allocating a slice (no haystack `strlen`). */
static inline bool sup_byte_eq_range(const char *s, int64_t lo, int64_t hi, const char *t) {
  if (!s || !t || lo < 0 || hi < lo) return false;
  size_t want = (size_t)(hi - lo);
  size_t m = 0;
  while (t[m]) m++;
  if (want != m) return false;
  for (size_t k = 0; k < m; k++) {
    if (!((const unsigned char *)s)[(size_t)lo + k]) return false;
  }
  return m == 0 || memcmp(s + (size_t)lo, t, m) == 0;
}
/* `a[alo..ahi)` byte-equals `b[blo..bhi)` — path/router segment compare without slices. */
static inline bool sup_byte_eq_ranges(const char *a, int64_t alo, int64_t ahi,
                                      const char *b, int64_t blo, int64_t bhi) {
  if (!a || !b || alo < 0 || ahi < alo || blo < 0 || bhi < blo) return false;
  size_t al = (size_t)(ahi - alo), bl = (size_t)(bhi - blo);
  if (al != bl) return false;
  for (size_t k = 0; k < al; k++) {
    if (!((const unsigned char *)a)[(size_t)alo + k]) return false;
    if (!((const unsigned char *)b)[(size_t)blo + k]) return false;
  }
  return al == 0 || memcmp(a + (size_t)alo, b + (size_t)blo, al) == 0;
}
/* Pointer `k` Unicode scalars into `s`, or nullptr when `s` ends first. */
static inline const char *sup_utf8_skip(const char *s, int64_t k) {
  const unsigned char *q = (const unsigned char *)s;
  for (; k > 0; --k) {
    if (!*q) return nullptr;
    ++q;
    while ((*q & 0xC0) == 0x80) ++q;
  }
  return (const char *)q;
}
/* Last scalar seek, so ascending `s[i]` / find / slice scans resume instead of restarting.
   Strings are immutable; the address reuses (`sup_sapp`'s realloc, closing a region) clear it. */
static _Thread_local struct { const char *s; int64_t k; const char *p; } sup_seek;
static inline const char *sup_utf8_seek(const char *s, int64_t k) {
  const char *from = s;
  int64_t at = 0;
  if (sup_seek.s == s && sup_seek.k <= k) { from = sup_seek.p; at = sup_seek.k; }
  const char *p = sup_utf8_skip(from, k - at);
  if (p) { sup_seek.s = s; sup_seek.k = k; sup_seek.p = p; }
  return p;
}
/* Closing a region reuses its addresses, so the seek cache is dropped with it.
   When the outermost region pops, advise the kernel to drop physical pages for the
   discarded suffix — otherwise VmHWM/RSS keeps every touched bump page for the process. */
static inline void sup_region_leave(size_t mark) {
  if (mark == SUP_REGION_NESTED) return;
  size_t old = sup_rg.cur;
  sup_rg.depth = 0;
  if (old == mark) return;
  sup_rg.cur = mark;
  sup_seek.s = nullptr;
#if defined(MADV_DONTNEED)
  if (sup_rg.base && old > mark) {
    size_t page = (size_t)4096;
    size_t from = (mark + page - 1) & ~(page - 1);
    if (from < old) (void)madvise(sup_rg.base + from, old - from, MADV_DONTNEED);
  }
#endif
}
/* Close a region whose frame returns string `s`: when `s` was built inside the region it moves
   down to the mark and stays allocated; everything else the frame allocated is released. */
static inline const char *sup_region_keep_str(size_t mark, const char *s) {
  if (mark == SUP_REGION_NESTED) return s;
  uintptr_t b = (uintptr_t)sup_rg.base, q = (uintptr_t)s;
  size_t old = sup_rg.cur;
  sup_rg.depth = 0;
  sup_seek.s = nullptr;
  if (!b || q < b + mark || q >= b + old) {
    if (old != mark) {
      sup_rg.cur = mark;
#if defined(MADV_DONTNEED)
      if (old > mark) {
        size_t page = (size_t)4096;
        size_t from = (mark + page - 1) & ~(page - 1);
        if (from < old) (void)madvise((uint8_t *)b + from, old - from, MADV_DONTNEED);
      }
#endif
    }
    return s;
  }
  size_t n = strlen(s) + 1;
  if (b + mark != q) memmove((void *)(b + mark), s, n);
  sup_rg.cur = mark + n;
#if defined(MADV_DONTNEED)
  if (old > sup_rg.cur) {
    size_t page = (size_t)4096;
    size_t from = (sup_rg.cur + page - 1) & ~(page - 1);
    if (from < old) (void)madvise((uint8_t *)b + from, old - from, MADV_DONTNEED);
  }
#endif
  return (const char *)(b + mark);
}
/* Scalar index of the first `nd` in `s` at/after scalar `start` (negative reads as 0), else -1. */
static inline int64_t sup_str_find(const char *s, const char *nd, int64_t start) {
  if (!nd || !nd[0]) return start;
  int64_t from = start < 0 ? 0 : start;
  /* ASCII needle: character index == byte index on ASCII haystacks (HTTP/JSON/CSV).
     Matches the byte-scan cost without a UTF-8 seek or recount. */
  bool ascii_nd = true;
  for (const unsigned char *q = (const unsigned char *)nd; *q; ++q) {
    if (*q >= 0x80) { ascii_nd = false; break; }
  }
  if (ascii_nd) {
    const char *base = s ? s : "";
    size_t n = strlen(base);
    size_t off = (size_t)from;
    if (off > n) return -1;
    const char *h = nd[1] ? strstr(base + off, nd) : strchr(base + off, nd[0]);
    return h ? (int64_t)(h - base) : -1;
  }
  const char *p = s ? sup_utf8_seek(s, from) : (from ? nullptr : "");
  if (!p) return -1;
  const char *h = nd[1] ? strstr(p, nd) : strchr(p, nd[0]);
  if (!h) return -1;
  for (const unsigned char *q = (const unsigned char *)p; q < (const unsigned char *)h; ++q)
    from += (*q & 0xC0) != 0x80;
  return from;
}
/* `nd` occurs in `s` exactly at scalar index `pos`. */
static inline bool sup_str_match_at(const char *s, const char *nd, int64_t pos) {
  if (!nd || !nd[0]) return true;
  if (pos < 0) return false;
  const char *p = s ? sup_utf8_seek(s, pos) : (pos ? nullptr : "");
  return p && strncmp(p, nd, strlen(nd)) == 0;
}
/* Leftmost `lit` at/after `from` followed by ≥1 ASCII digit; empty `lit` → first digit. */
static inline int64_t sup_str_find_lit_digits(const char *s, const char *lit, int64_t from) {
  int64_t at = from < 0 ? 0 : from;
  if (!lit || !lit[0]) {
    const char *p = s ? sup_utf8_seek(s, at) : (at ? nullptr : "");
    if (!p) return -1;
    for (const unsigned char *q = (const unsigned char *)p; *q; ++q) {
      if ((*q & 0xC0) == 0x80) continue;
      if (*q >= '0' && *q <= '9') return at;
      at += 1;
    }
    return -1;
  }
  size_t llen = strlen(lit);
  while (1) {
    at = sup_str_find(s, lit, at);
    if (at < 0) return -1;
    const char *p = s ? sup_utf8_seek(s, at) : nullptr;
    if (!p) return -1;
    const char *d = p + llen; /* lit is ASCII in the regex fast path */
    if (*d >= '0' && *d <= '9') return at;
    at += 1;
  }
}
/* End scalar index of `lit` + ≥1 ASCII digit beginning at `start`, or -1. */
static inline int64_t sup_str_lit_digits_end(const char *s, int64_t start, const char *lit) {
  if (start < 0 || !sup_str_match_at(s, lit, start)) return -1;
  size_t llen = lit && lit[0] ? strlen(lit) : 0;
  const char *p = s ? sup_utf8_seek(s, start) : nullptr;
  if (!p) return -1;
  const char *d = p + llen;
  if (*d < '0' || *d > '9') return -1;
  int64_t end = start + (int64_t)llen + 1;
  for (++d; *d >= '0' && *d <= '9'; ++d) end += 1;
  return end;
}
static inline int64_t sup_mono_ns(void) {
#if defined(CLOCK_MONOTONIC)
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
#else
  return 0;
#endif
}
static inline const char *sup_cat_ss(const char *a, const char *b) {
  size_t na = a ? strlen(a) : 0, nb = b ? strlen(b) : 0;
  char *o = (char *)sup_ralloc(na + nb + 1, 1);
  if (na) memcpy(o, a, na);
  if (nb) memcpy(o + na, b, nb);
  o[na + nb] = 0;
  return o;
}
/* Decimal `v` into `t` (24 bytes suffice); returns the length. Same text as `%lld`. */
static inline size_t sup_fmt_i64(char *t, int64_t v) {
  char r[24];
  size_t k = 0, m = 0;
  uint64_t u = v < 0 ? 0 - (uint64_t)v : (uint64_t)v;
  do { r[k++] = (char)('0' + u % 10); u /= 10; } while (u);
  if (v < 0) t[m++] = '-';
  while (k) t[m++] = r[--k];
  t[m] = 0;
  return m;
}
/* `*x = *x + p` for a string local. The local's buffer `*o` (byte length `*n`, capacity `*c`)
   grows in place while `*x` still points at it; every other read of the local resets `*o`, so
   any other value of `*x` (shared, literal, or written elsewhere) is first copied. */
static inline void sup_sapp(const char **x, char **o, size_t *n, size_t *c, const char *p, size_t pn) {
  if (!*o || *o != *x) {
    const char *s = *x ? *x : "";
    size_t sn = strlen(s), cap = (sn + pn + 1) * 2;
    if (cap < 32) cap = 32;
    char *b = (char *)sup_ralloc(cap, 1);
    memcpy(b, s, sn);
    *o = b; *n = sn; *c = cap;
  } else if (*n + pn + 1 > *c) {
    size_t cap = (*n + pn + 1) * 2;
    char *b;
    if (!sup_in_region(*o)) {
      b = (char *)realloc(*o, cap);
      if (!b) abort();
      if (b != *o) sup_seek.s = nullptr;
    } else if ((uint8_t *)*o + *c == sup_rg.base + sup_rg.cur && cap - *c <= sup_rg.cap - sup_rg.cur) {
      /* Topmost region block: grow in place. */
      sup_rg.cur += cap - *c;
      b = *o;
    } else {
      b = (char *)sup_ralloc(cap, 1);
      memcpy(b, *o, *n);
    }
    *o = b; *c = cap;
  }
  memcpy(*o + *n, p, pn);
  *n += pn;
  (*o)[*n] = 0;
  *x = *o;
}
static inline void sup_sapp_s(const char **x, char **o, size_t *n, size_t *c, const char *p) {
  if (!p) p = "";
  sup_sapp(x, o, n, c, p, strlen(p));
}
/* Append one ASCII/Latin-1 byte (code & 0xff) — hot path for string builders / `push_ascii`. */
static inline void sup_sapp_ascii(const char **x, char **o, size_t *n, size_t *c, int64_t code) {
  char ch = (char)(unsigned char)(code & 0xff);
  sup_sapp(x, o, n, c, &ch, 1);
}
static inline const char *sup_push_ascii(const char *s, int64_t code) {
  char ch = (char)(unsigned char)(code & 0xff);
  size_t sn = s ? strlen(s) : 0;
  char *b = (char *)sup_ralloc(sn + 2, 1);
  if (sn) memcpy(b, s, sn);
  b[sn] = ch;
  b[sn + 1] = 0;
  return b;
}
static inline const char *sup_push_i64(const char *s, int64_t v) {
  char t[24];
  size_t k = sup_fmt_i64(t, v);
  size_t sn = s ? strlen(s) : 0;
  char *b = (char *)sup_ralloc(sn + k + 1, 1);
  if (sn) memcpy(b, s, sn);
  memcpy(b + sn, t, k);
  b[sn + k] = 0;
  return b;
}
static inline void sup_sapp_i(const char **x, char **o, size_t *n, size_t *c, int64_t v) {
  char t[24];
  size_t k = sup_fmt_i64(t, v);
  sup_sapp(x, o, n, c, t, k);
}
static inline void sup_sapp_f(const char **x, char **o, size_t *n, size_t *c, double v) {
  char t[40];
  int k = snprintf(t, sizeof t, "%g", v);
  sup_sapp(x, o, n, c, t, k > 0 ? (size_t)k : 0);
}
static inline void sup_sapp_b(const char **x, char **o, size_t *n, size_t *c, bool v) {
  sup_sapp(x, o, n, c, v ? "true" : "false", v ? 4 : 5);
}
/* Growable / fixed wire builder (wire `Buf`). Handle is pointer bits as i64.
   `flags&1` = TLS/fixed storage (never realloc; finish() copies out). */
typedef struct { char *p; size_t n, c; uint8_t flags; } SupSb;
#define SUP_SB_STACK 1u
#define SUP_SB_STACK_N 16
#define SUP_SB_STACK_CAP 512
static __thread char sup_sb_tls_d[SUP_SB_STACK_N][SUP_SB_STACK_CAP];
static __thread SupSb sup_sb_tls_b[SUP_SB_STACK_N];
static __thread unsigned sup_sb_tls_used;
static inline void sup_sb_ensure(SupSb *b, size_t need) {
  if (!b) return;
  if (b->p && b->n + need + 1 <= b->c) return;
  if (b->flags & SUP_SB_STACK) return;
  size_t cap = b->n + need + 1;
  if (cap < 64) cap = 64;
  if (cap < b->c * 2) cap = b->c * 2;
  char *nb;
  if (b->p && !sup_in_region(b->p)) {
    nb = (char *)realloc(b->p, cap);
    if (!nb) abort();
  } else if (b->p && (uint8_t *)b->p + b->c == sup_rg.base + sup_rg.cur
             && cap > b->c && cap - b->c <= (sup_rg.cap - sup_rg.cur)) {
    memset(b->p + b->c, 0, cap - b->c);
    sup_rg.cur += cap - b->c;
    b->c = cap;
    return;
  } else {
    nb = (char *)sup_ralloc(cap, 1);
    if (b->p && b->n) memcpy(nb, b->p, b->n);
  }
  b->p = nb; b->c = cap;
}
static inline int64_t sup_sb_new(int64_t cap0) {
  size_t cap = cap0 > 64 ? (size_t)cap0 : 64;
  SupSb *b = (SupSb *)sup_ralloc(sizeof(SupSb), 8);
  b->p = (char *)sup_ralloc(cap, 1);
  b->n = 0; b->c = cap; b->flags = 0; b->p[0] = 0;
  return (int64_t)(uintptr_t)b;
}
/* Fixed wire Buf: TLS char[512] for small caps; larger `cap` → one fixed
   heap slab (still no realloc — matches fixed `obuf` capacity). */
static inline int64_t sup_sb_stack(int64_t cap0) {
  size_t want = cap0 > 0 ? (size_t)cap0 : 64;
  if (want > SUP_SB_STACK_CAP || sup_sb_tls_used >= SUP_SB_STACK_N) {
    size_t cap = want < 64 ? 64 : want;
    SupSb *b = (SupSb *)sup_ralloc(sizeof(SupSb) + cap, 8);
    b->p = (char *)(b + 1);
    b->n = 0; b->c = cap; b->flags = SUP_SB_STACK; b->p[0] = 0;
    return (int64_t)(uintptr_t)b;
  }
  unsigned i = sup_sb_tls_used++;
  SupSb *b = &sup_sb_tls_b[i];
  b->p = sup_sb_tls_d[i];
  b->n = 0;
  b->c = SUP_SB_STACK_CAP;
  b->flags = SUP_SB_STACK;
  b->p[0] = 0;
  return (int64_t)(uintptr_t)b;
}
/* Length-known append (AOT const strings skip strlen). Fast path = Buf.lit. */
static inline void sup_sb_litn(int64_t h, const char *s, size_t pn) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b || !pn || !s) return;
  if (b->p && b->n + pn + 1 <= b->c) {
    memcpy(b->p + b->n, s, pn);
    b->n += pn;
    b->p[b->n] = 0;
    return;
  }
  if (b->flags & SUP_SB_STACK) return;
  sup_sb_ensure(b, pn);
  if (!b->p || b->n + pn + 1 > b->c) return;
  memcpy(b->p + b->n, s, pn);
  b->n += pn;
  b->p[b->n] = 0;
}
static inline void sup_sb_lit(int64_t h, const char *s) {
  if (!s) s = "";
  size_t pn = strlen(s);
  if (pn) sup_sb_litn(h, s, pn);
}
static inline void sup_sb_ascii(int64_t h, int64_t code) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b) return;
  char ch = (char)(unsigned char)(code & 0xff);
  if (b->p && b->n + 2 <= b->c) {
    b->p[b->n++] = ch;
    b->p[b->n] = 0;
    return;
  }
  if (b->flags & SUP_SB_STACK) return;
  sup_sb_ensure(b, 1);
  if (!b->p || b->n + 2 > b->c) return;
  b->p[b->n++] = ch;
  b->p[b->n] = 0;
}
/* Decimal append straight into the Buf (`Out.int`) — no temp[24] + second copy. */
static inline void sup_sb_i64(int64_t h, int64_t v) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b) return;
  if (!(b->p && b->n + 24 <= b->c)) {
    if (b->flags & SUP_SB_STACK) return;
    sup_sb_ensure(b, 24);
    if (!b->p || b->n + 24 > b->c) return;
  }
  size_t k = sup_fmt_i64(b->p + b->n, v);
  if (!k) return;
  b->n += k;
  b->p[b->n] = 0;
}
static inline void sup_sb_set(int64_t h, int64_t i, int64_t code) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b || !b->p || i < 0 || (size_t)i >= b->n) return;
  b->p[i] = (char)(unsigned char)(code & 0xff);
}
static inline int64_t sup_sb_len(int64_t h) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  return b && b->p ? (int64_t)b->n : 0;
}
static inline void sup_sb_clear(int64_t h) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b || !b->p) return;
  b->n = 0; b->p[0] = 0;
}
static inline void sup_sb_cat(int64_t dst, int64_t src) {
  SupSb *s = (SupSb *)(uintptr_t)src;
  if (!s || !s->p || !s->n) return;
  sup_sb_litn(dst, s->p, s->n);
}
/* Peek current contents (valid until clear/overwrite). Prefer over finish() in hot reuse loops. */
static inline const char *sup_sb_cstr(int64_t h) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  return b && b->p ? b->p : "";
}
static inline const char *sup_sb_finish(int64_t h) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b || !b->p) return "";
  if (b->flags & SUP_SB_STACK) {
    char *o = (char *)sup_ralloc(b->n + 1, 1);
    memcpy(o, b->p, b->n);
    o[b->n] = 0;
    b->n = 0; b->p[0] = 0;
    return o;
  }
  const char *r = b->p;
  b->p = nullptr; b->n = 0; b->c = 0;
  return r;
}
static inline const char *sup_cat_si(const char *a, int64_t v) { char t[24]; sup_fmt_i64(t, v); return sup_cat_ss(a, t); }
static inline const char *sup_cat_sf(const char *a, double v) { char t[40]; snprintf(t, sizeof t, "%g", v); return sup_cat_ss(a, t); }
static inline const char *sup_cat_sb(const char *a, bool v) { return sup_cat_ss(a, v ? "true" : "false"); }
static inline const char *sup_cat_is(int64_t v, const char *b) { char t[24]; sup_fmt_i64(t, v); return sup_cat_ss(t, b); }
static inline const char *sup_cat_fs(double v, const char *b) { char t[40]; snprintf(t, sizeof t, "%g", v); return sup_cat_ss(t, b); }
static inline const char *sup_cat_bs(bool v, const char *b) { return sup_cat_ss(v ? "true" : "false", b); }
"#;

/// Boxed → unboxed scalar arrays. They call boxed-runtime converters, so they are emitted
/// with the struct bridges (after the runtime) rather than in the shared prelude header.
const UNBOX_ARR_HELPERS: &str = r#"static inline bool *sup_unbox_ab(SuperVal v, int64_t *n) {
  SuperArr *a = sup_boxed_arr(v);
  *n = a ? (int64_t)a->len : 0;
  if (!a) return nullptr;
  bool *p = (bool *)malloc(a->len ? a->len : 1);
  if (!p) abort();
  for (size_t k = 0; k < a->len; ++k)
    p[k] = a->kind == 2 ? a->i64s[k] != 0 : a->kind == 1 ? a->f64s[k] != 0.0 : super_truthy(a->items[k]) != 0;
  return p;
}
static inline int64_t *sup_unbox_ai(SuperVal v, int64_t *n) {
  SuperArr *a = sup_boxed_arr(v);
  *n = a ? (int64_t)a->len : 0;
  if (!a) return nullptr;
  if (a->kind == 2 && sup_is_view(a)) return a->i64s;
  int64_t *p = (int64_t *)malloc((a->len ? a->len : 1) * sizeof *p);
  if (!p) abort();
  for (size_t k = 0; k < a->len; ++k)
    p[k] = a->kind == 2 ? a->i64s[k] : a->kind == 1 ? sup_to_i64(a->f64s[k]) : super_as_i(a->items[k]);
  return p;
}
static inline double *sup_unbox_af(SuperVal v, int64_t *n) {
  SuperArr *a = sup_boxed_arr(v);
  *n = a ? (int64_t)a->len : 0;
  if (!a) return nullptr;
  if (a->kind == 1 && sup_is_view(a)) return a->f64s;
  double *p = (double *)malloc((a->len ? a->len : 1) * sizeof *p);
  if (!p) abort();
  for (size_t k = 0; k < a->len; ++k)
    p[k] = a->kind == 1 ? a->f64s[k] : a->kind == 2 ? (double)a->i64s[k] : super_as_f(a->items[k]);
  return p;
}
static inline uint8_t *sup_unbox_au(SuperVal v, int64_t *n) {
  SuperArr *a = sup_boxed_arr(v);
  *n = a ? (int64_t)a->len : 0;
  if (!a) return nullptr;
  uint8_t *p = (uint8_t *)malloc(a->len ? a->len : 1);
  if (!p) abort();
  for (size_t k = 0; k < a->len; ++k) {
    int64_t x = a->kind == 2 ? a->i64s[k] : a->kind == 1 ? sup_to_i64(a->f64s[k]) : super_as_i(a->items[k]);
    p[k] = (uint8_t)(x & 255);
  }
  return p;
}
"#;

const UNBOXED_PRELUDE_TAIL: &str = r#"
/* `s[i]` by Unicode scalar, like the VM and the boxed runtime; scans only the first `i` scalars. */
static inline const char *sup_str_at(const char *s, int64_t i) {
  if (!s || i < 0) return "";
  const unsigned char *p = (const unsigned char *)sup_utf8_seek(s, i);
  if (!p) return "";
  if (*p < 0x80) return SUP_CHR_TBL[*p];
  size_t n = 1;
  while ((p[n] & 0xC0) == 0x80) ++n;
  char *o = (char *)sup_ralloc(n + 1, 1);
  memcpy(o, p, n);
  o[n] = 0;
  return o;
}
/* `s[start..end]` by Unicode scalar; indices clamp like the VM (`as usize`, then `min(len)`). */
static inline const char *sup_str_slice(const char *s, int64_t start, int64_t end) {
  if (!s || start < 0) return "";
  const char *a = sup_utf8_seek(s, start);
  if (!a || !*a) return "";
  const char *b = a;
  if (end < 0) b = a + strlen(a);
  else if (end > start) {
    b = sup_utf8_skip(a, end - start);
    if (b) { sup_seek.k = end; sup_seek.p = b; }
    else b = a + strlen(a);
  }
  size_t n = (size_t)(b - a);
  char *o = (char *)sup_ralloc(n + 1, 1);
  memcpy(o, a, n);
  o[n] = 0;
  return o;
}
static inline const char *sup_chr(int64_t c) { return SUP_CHR_TBL[(unsigned char)(c < 0 ? 0 : (c & 0x7f))]; }
static inline uint32_t *sup_crc32_tbl(void) {
  static uint32_t tbl[256];
  static bool ready = false;
  if (!ready) {
    for (uint32_t i = 0; i < 256; i++) {
      uint32_t c = i;
      for (int k = 0; k < 8; k++) c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
      tbl[i] = c;
    }
    ready = true;
  }
  return tbl;
}
static inline int64_t sup_crc32_str(const char *s) {
  uint32_t *tbl = sup_crc32_tbl();
  uint32_t crc = 0xFFFFFFFFu;
  for (const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; p++)
    crc = tbl[(crc ^ *p) & 0xFFu] ^ (crc >> 8);
  return (int64_t)(crc ^ 0xFFFFFFFFu);
}
/* IEEE CRC-32 over i64 byte lanes data[lo..hi). */
static inline int64_t sup_crc32_i64_range(int64_t *restrict data, int64_t len, int64_t lo, int64_t hi) {
  if (!data || lo < 0) lo = 0;
  if (hi > len) hi = len;
  if (hi <= lo) return 0;
  uint32_t *tbl = sup_crc32_tbl();
  uint32_t crc = 0xFFFFFFFFu;
  for (int64_t i = lo; i < hi; ++i) {
    unsigned char b = (unsigned char)(data[i] & 255);
    crc = tbl[(crc ^ b) & 0xFFu] ^ (crc >> 8);
  }
  return (int64_t)(crc ^ 0xFFFFFFFFu);
}
/* IEEE CRC-32 over dense uint8_t bytes — `crc32(buf[lo..hi])` parity. */
static inline int64_t sup_crc32_u8_range(uint8_t *restrict data, int64_t len, int64_t lo, int64_t hi) {
  if (!data || lo < 0) lo = 0;
  if (hi > len) hi = len;
  if (hi <= lo) return 0;
  uint32_t *tbl = sup_crc32_tbl();
  uint32_t crc = 0xFFFFFFFFu;
  for (int64_t i = lo; i < hi; ++i) {
    crc = tbl[(crc ^ data[i]) & 0xFFu] ^ (crc >> 8);
  }
  return (int64_t)(crc ^ 0xFFFFFFFFu);
}
/* dst[dst_off..] ← src[src_off..] for n lanes (clamped). */
static inline void sup_i64_copy_range(int64_t *restrict dst, int64_t dn, int64_t dst_off,
                                     int64_t *restrict src, int64_t sn, int64_t src_off, int64_t n) {
  if (!dst || !src || n <= 0 || dst_off < 0 || src_off < 0) return;
  int64_t lim = n;
  if (dst_off + lim > dn) lim = dn - dst_off;
  if (src_off + lim > sn) lim = sn - src_off;
  if (lim <= 0) return;
  memmove(dst + dst_off, src + src_off, (size_t)lim * sizeof(int64_t));
}
static inline void sup_u8_copy_range(uint8_t *restrict dst, int64_t dn, int64_t dst_off,
                                    uint8_t *restrict src, int64_t sn, int64_t src_off, int64_t n) {
  if (!dst || !src || n <= 0 || dst_off < 0 || src_off < 0) return;
  int64_t lim = n;
  if (dst_off + lim > dn) lim = dn - dst_off;
  if (src_off + lim > sn) lim = sn - src_off;
  if (lim <= 0) return;
  memmove(dst + dst_off, src + src_off, (size_t)lim);
}
/* FIPS 180-4 SHA-256 + RFC 2104 HMAC — host `sys_sha256` / `sys_hmac_sha256` (lowercase hex). */
static inline uint32_t sup_rotr32(uint32_t x, int n) { return (x >> n) | (x << (32 - n)); }
#if defined(__SHA__) && (defined(__x86_64__) || defined(__i386__) || defined(_M_X64))
#include <immintrin.h>
/* Public-domain Intel SHA-NI compress (Jeffrey Walton / Intel / miTLS). */
static void sup_sha256_compress(uint32_t *restrict state, const uint8_t *restrict data) {
  __m128i STATE0, STATE1, MSG, TMP, MSG0, MSG1, MSG2, MSG3, ABEF_SAVE, CDGH_SAVE;
  const __m128i MASK = _mm_set_epi64x(0x0c0d0e0f08090a0bULL, 0x0405060700010203ULL);
  TMP = _mm_loadu_si128((const __m128i *)&state[0]);
  STATE1 = _mm_loadu_si128((const __m128i *)&state[4]);
  TMP = _mm_shuffle_epi32(TMP, 0xB1);
  STATE1 = _mm_shuffle_epi32(STATE1, 0x1B);
  STATE0 = _mm_alignr_epi8(TMP, STATE1, 8);
  STATE1 = _mm_blend_epi16(STATE1, TMP, 0xF0);
  ABEF_SAVE = STATE0; CDGH_SAVE = STATE1;
  MSG = _mm_loadu_si128((const __m128i *)(data + 0));
  MSG0 = _mm_shuffle_epi8(MSG, MASK);
  MSG = _mm_add_epi32(MSG0, _mm_set_epi64x(0xE9B5DBA5B5C0FBCFULL, 0x71374491428A2F98ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG1 = _mm_loadu_si128((const __m128i *)(data + 16));
  MSG1 = _mm_shuffle_epi8(MSG1, MASK);
  MSG = _mm_add_epi32(MSG1, _mm_set_epi64x(0xAB1C5ED5923F82A4ULL, 0x59F111F13956C25BULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG0 = _mm_sha256msg1_epu32(MSG0, MSG1);
  MSG2 = _mm_loadu_si128((const __m128i *)(data + 32));
  MSG2 = _mm_shuffle_epi8(MSG2, MASK);
  MSG = _mm_add_epi32(MSG2, _mm_set_epi64x(0x550C7DC3243185BEULL, 0x12835B01D807AA98ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG1 = _mm_sha256msg1_epu32(MSG1, MSG2);
  MSG3 = _mm_loadu_si128((const __m128i *)(data + 48));
  MSG3 = _mm_shuffle_epi8(MSG3, MASK);
  MSG = _mm_add_epi32(MSG3, _mm_set_epi64x(0xC19BF1749BDC06A7ULL, 0x80DEB1FE72BE5D74ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG3, MSG2, 4);
  MSG0 = _mm_add_epi32(MSG0, TMP);
  MSG0 = _mm_sha256msg2_epu32(MSG0, MSG3);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG2 = _mm_sha256msg1_epu32(MSG2, MSG3);
  MSG = _mm_add_epi32(MSG0, _mm_set_epi64x(0x240CA1CC0FC19DC6ULL, 0xEFBE4786E49B69C1ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG0, MSG3, 4);
  MSG1 = _mm_add_epi32(MSG1, TMP);
  MSG1 = _mm_sha256msg2_epu32(MSG1, MSG0);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG3 = _mm_sha256msg1_epu32(MSG3, MSG0);
  MSG = _mm_add_epi32(MSG1, _mm_set_epi64x(0x76F988DA5CB0A9DCULL, 0x4A7484AA2DE92C6FULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG1, MSG0, 4);
  MSG2 = _mm_add_epi32(MSG2, TMP);
  MSG2 = _mm_sha256msg2_epu32(MSG2, MSG1);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG0 = _mm_sha256msg1_epu32(MSG0, MSG1);
  MSG = _mm_add_epi32(MSG2, _mm_set_epi64x(0xBF597FC7B00327C8ULL, 0xA831C66D983E5152ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG2, MSG1, 4);
  MSG3 = _mm_add_epi32(MSG3, TMP);
  MSG3 = _mm_sha256msg2_epu32(MSG3, MSG2);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG1 = _mm_sha256msg1_epu32(MSG1, MSG2);
  MSG = _mm_add_epi32(MSG3, _mm_set_epi64x(0x1429296706CA6351ULL, 0xD5A79147C6E00BF3ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG3, MSG2, 4);
  MSG0 = _mm_add_epi32(MSG0, TMP);
  MSG0 = _mm_sha256msg2_epu32(MSG0, MSG3);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG2 = _mm_sha256msg1_epu32(MSG2, MSG3);
  MSG = _mm_add_epi32(MSG0, _mm_set_epi64x(0x53380D134D2C6DFCULL, 0x2E1B213827B70A85ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG0, MSG3, 4);
  MSG1 = _mm_add_epi32(MSG1, TMP);
  MSG1 = _mm_sha256msg2_epu32(MSG1, MSG0);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG3 = _mm_sha256msg1_epu32(MSG3, MSG0);
  MSG = _mm_add_epi32(MSG1, _mm_set_epi64x(0x92722C8581C2C92EULL, 0x766A0ABB650A7354ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG1, MSG0, 4);
  MSG2 = _mm_add_epi32(MSG2, TMP);
  MSG2 = _mm_sha256msg2_epu32(MSG2, MSG1);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG0 = _mm_sha256msg1_epu32(MSG0, MSG1);
  MSG = _mm_add_epi32(MSG2, _mm_set_epi64x(0xC76C51A3C24B8B70ULL, 0xA81A664BA2BFE8A1ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG2, MSG1, 4);
  MSG3 = _mm_add_epi32(MSG3, TMP);
  MSG3 = _mm_sha256msg2_epu32(MSG3, MSG2);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG1 = _mm_sha256msg1_epu32(MSG1, MSG2);
  MSG = _mm_add_epi32(MSG3, _mm_set_epi64x(0x106AA070F40E3585ULL, 0xD6990624D192E819ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG3, MSG2, 4);
  MSG0 = _mm_add_epi32(MSG0, TMP);
  MSG0 = _mm_sha256msg2_epu32(MSG0, MSG3);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG2 = _mm_sha256msg1_epu32(MSG2, MSG3);
  MSG = _mm_add_epi32(MSG0, _mm_set_epi64x(0x34B0BCB52748774CULL, 0x1E376C0819A4C116ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG0, MSG3, 4);
  MSG1 = _mm_add_epi32(MSG1, TMP);
  MSG1 = _mm_sha256msg2_epu32(MSG1, MSG0);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG3 = _mm_sha256msg1_epu32(MSG3, MSG0);
  MSG = _mm_add_epi32(MSG1, _mm_set_epi64x(0x682E6FF35B9CCA4FULL, 0x4ED8AA4A391C0CB3ULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG1, MSG0, 4);
  MSG2 = _mm_add_epi32(MSG2, TMP);
  MSG2 = _mm_sha256msg2_epu32(MSG2, MSG1);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG = _mm_add_epi32(MSG2, _mm_set_epi64x(0x8CC7020884C87814ULL, 0x78A5636F748F82EEULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  TMP = _mm_alignr_epi8(MSG2, MSG1, 4);
  MSG3 = _mm_add_epi32(MSG3, TMP);
  MSG3 = _mm_sha256msg2_epu32(MSG3, MSG2);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  MSG = _mm_add_epi32(MSG3, _mm_set_epi64x(0xC67178F2BEF9A3F7ULL, 0xA4506CEB90BEFFFAULL));
  STATE1 = _mm_sha256rnds2_epu32(STATE1, STATE0, MSG);
  MSG = _mm_shuffle_epi32(MSG, 0x0E);
  STATE0 = _mm_sha256rnds2_epu32(STATE0, STATE1, MSG);
  STATE0 = _mm_add_epi32(STATE0, ABEF_SAVE);
  STATE1 = _mm_add_epi32(STATE1, CDGH_SAVE);
  TMP = _mm_shuffle_epi32(STATE0, 0x1B);
  STATE1 = _mm_shuffle_epi32(STATE1, 0xB1);
  STATE0 = _mm_blend_epi16(TMP, STATE1, 0xF0);
  STATE1 = _mm_alignr_epi8(STATE1, TMP, 8);
  _mm_storeu_si128((__m128i *)&state[0], STATE0);
  _mm_storeu_si128((__m128i *)&state[4], STATE1);
}
#else
static void sup_sha256_compress(uint32_t *restrict h, const uint8_t *restrict block) {
  static const uint32_t K[64] = {
    0x428a2f98u,0x71374491u,0xb5c0fbcfu,0xe9b5dba5u,0x3956c25bu,0x59f111f1u,0x923f82a4u,0xab1c5ed5u,
    0xd807aa98u,0x12835b01u,0x243185beu,0x550c7dc3u,0x72be5d74u,0x80deb1feu,0x9bdc06a7u,0xc19bf174u,
    0xe49b69c1u,0xefbe4786u,0x0fc19dc6u,0x240ca1ccu,0x2de92c6fu,0x4a7484aau,0x5cb0a9dcu,0x76f988dau,
    0x983e5152u,0xa831c66du,0xb00327c8u,0xbf597fc7u,0xc6e00bf3u,0xd5a79147u,0x06ca6351u,0x14292967u,
    0x27b70a85u,0x2e1b2138u,0x4d2c6dfcu,0x53380d13u,0x650a7354u,0x766a0abbu,0x81c2c92eu,0x92722c85u,
    0xa2bfe8a1u,0xa81a664bu,0xc24b8b70u,0xc76c51a3u,0xd192e819u,0xd6990624u,0xf40e3585u,0x106aa070u,
    0x19a4c116u,0x1e376c08u,0x2748774cu,0x34b0bcb5u,0x391c0cb3u,0x4ed8aa4au,0x5b9cca4fu,0x682e6ff3u,
    0x748f82eeu,0x78a5636fu,0x84c87814u,0x8cc70208u,0x90befffau,0xa4506cebu,0xbef9a3f7u,0xc67178f2u};
  uint32_t w[64];
  for (int i = 0; i < 16; i++)
    w[i] = ((uint32_t)block[i*4] << 24) | ((uint32_t)block[i*4+1] << 16) |
           ((uint32_t)block[i*4+2] << 8) | (uint32_t)block[i*4+3];
  for (int i = 16; i < 64; i++) {
    uint32_t s0 = sup_rotr32(w[i-15], 7) ^ sup_rotr32(w[i-15], 18) ^ (w[i-15] >> 3);
    uint32_t s1 = sup_rotr32(w[i-2], 17) ^ sup_rotr32(w[i-2], 19) ^ (w[i-2] >> 10);
    w[i] = w[i-16] + s0 + w[i-7] + s1;
  }
  uint32_t a=h[0],b=h[1],c=h[2],d=h[3],e=h[4],f=h[5],g=h[6],hh=h[7];
  for (int i = 0; i < 64; i++) {
    uint32_t S1 = sup_rotr32(e, 6) ^ sup_rotr32(e, 11) ^ sup_rotr32(e, 25);
    uint32_t ch = (e & f) ^ ((~e) & g);
    uint32_t t1 = hh + S1 + ch + K[i] + w[i];
    uint32_t S0 = sup_rotr32(a, 2) ^ sup_rotr32(a, 13) ^ sup_rotr32(a, 22);
    uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
    uint32_t t2 = S0 + maj;
    hh=g; g=f; f=e; e=d+t1; d=c; c=b; b=a; a=t1+t2;
  }
  h[0]+=a; h[1]+=b; h[2]+=c; h[3]+=d; h[4]+=e; h[5]+=f; h[6]+=g; h[7]+=hh;
}
#endif
static void sup_sha256_raw(const uint8_t *msg, size_t len, uint8_t out[32]) {
  uint32_t h[8] = {0x6a09e667u,0xbb67ae85u,0x3c6ef372u,0xa54ff53au,
                   0x510e527fu,0x9b05688cu,0x1f83d9abu,0x5be0cd19u};
  uint8_t block[64];
  size_t off = 0;
  while (off + 64 <= len) { sup_sha256_compress(h, msg + off); off += 64; }
  size_t rem = len - off;
  memset(block, 0, 64);
  if (rem) memcpy(block, msg + off, rem);
  block[rem] = 0x80;
  if (rem >= 56) { sup_sha256_compress(h, block); memset(block, 0, 64); }
  uint64_t bits = (uint64_t)len * 8ull;
  for (int i = 0; i < 8; i++) block[63 - i] = (uint8_t)(bits >> (8 * i));
  sup_sha256_compress(h, block);
  for (int i = 0; i < 8; i++) {
    out[i*4] = (uint8_t)(h[i] >> 24); out[i*4+1] = (uint8_t)(h[i] >> 16);
    out[i*4+2] = (uint8_t)(h[i] >> 8); out[i*4+3] = (uint8_t)h[i];
  }
}
/* Continue SHA-256 from a midstate that already absorbed `prior` bytes; write digest to `out`. */
static void sup_sha256_finish_from(uint32_t h[8], const uint8_t *msg, size_t len, size_t prior,
                                   uint8_t out[32]) {
  uint8_t block[64];
  size_t off = 0;
  while (off + 64 <= len) { sup_sha256_compress(h, msg + off); off += 64; }
  size_t rem = len - off;
  memset(block, 0, 64);
  if (rem) memcpy(block, msg + off, rem);
  block[rem] = 0x80;
  if (rem >= 56) { sup_sha256_compress(h, block); memset(block, 0, 64); }
  uint64_t bits = (uint64_t)(prior + len) * 8ull;
  for (int i = 0; i < 8; i++) block[63 - i] = (uint8_t)(bits >> (8 * i));
  sup_sha256_compress(h, block);
  for (int i = 0; i < 8; i++) {
    out[i * 4] = (uint8_t)(h[i] >> 24); out[i * 4 + 1] = (uint8_t)(h[i] >> 16);
    out[i * 4 + 2] = (uint8_t)(h[i] >> 8); out[i * 4 + 3] = (uint8_t)h[i];
  }
}
static inline void sup_hex32(const uint8_t *d, char *o) {
  static const char *H = "0123456789abcdef";
  for (int i = 0; i < 32; i++) { o[i*2] = H[d[i] >> 4]; o[i*2+1] = H[d[i] & 15]; }
  o[64] = 0;
}
static inline const char *sup_sha256_hex(const char *s) {
  uint8_t dig[32];
  const char *p = s ? s : "";
  sup_sha256_raw((const uint8_t *)p, strlen(p), dig);
  char *o = (char *)sup_ralloc(65, 1);
  sup_hex32(dig, o);
  return o;
}
/* SHA-256 of i64 byte lanes → goose-stack 32-lane digest (crypto digest parity). */
static inline int64_t *sup_sha256_i64(const int64_t *msg, int64_t mn, int64_t *ret_len) {
  size_t mlen = mn > 0 ? (size_t)mn : 0;
  uint8_t mstack[256];
  uint8_t *mb = mstack;
  if (mlen > 256) mb = (uint8_t *)sup_ralloc(mlen ? mlen : 1, 1);
  for (size_t i = 0; i < mlen; i++) mb[i] = (uint8_t)(msg ? msg[i] : 0);
  uint8_t dig[32];
  sup_sha256_raw(mb, mlen, dig);
  int64_t *out = sup_arena_i64(32);
  for (int i = 0; i < 32; i++) out[i] = (int64_t)dig[i];
  if (ret_len) *ret_len = 32;
  return out;
}
/* Lowercase hex of i64 byte lanes (Merkle roots / truncated tokens). */
static inline const char *sup_sha2_hex_of_i64(const int64_t *bytes, int64_t n) {
  size_t len = n > 0 ? (size_t)n : 0;
  char *o = (char *)sup_ralloc(len * 2 + 1, 1);
  static const char *H = "0123456789abcdef";
  for (size_t i = 0; i < len; i++) {
    uint8_t b = (uint8_t)(bytes ? bytes[i] : 0);
    o[i * 2] = H[b >> 4];
    o[i * 2 + 1] = H[b & 15];
  }
  o[len * 2] = 0;
  return o;
}
static inline const char *sup_hmac_sha256_hex(const char *key, const char *msg) {
  uint8_t kbuf[64], dig[32], mac[32];
  memset(kbuf, 0, 64);
  const uint8_t *kb = (const uint8_t *)(key ? key : "");
  size_t klen = strlen((const char *)kb);
  const uint8_t *mb = (const uint8_t *)(msg ? msg : "");
  size_t mlen = strlen((const char *)mb);
  if (klen > 64) { sup_sha256_raw(kb, klen, dig); memcpy(kbuf, dig, 32); }
  else memcpy(kbuf, kb, klen);
  size_t inner_n = 64 + mlen;
  uint8_t *inner = (uint8_t *)sup_ralloc(inner_n ? inner_n : 1, 1);
  uint8_t outer[64 + 32];
  for (int i = 0; i < 64; i++) { inner[i] = (uint8_t)(kbuf[i] ^ 0x36); outer[i] = (uint8_t)(kbuf[i] ^ 0x5c); }
  if (mlen) memcpy(inner + 64, mb, mlen);
  sup_sha256_raw(inner, inner_n, dig);
  memcpy(outer + 64, dig, 32);
  sup_sha256_raw(outer, 64 + 32, mac);
  char *o = (char *)sup_ralloc(65, 1);
  sup_hex32(mac, o);
  return o;
}
/* Byte-array HMAC: key/msg are i64 lanes holding 0..255; returns a fresh 32-lane digest. */
static inline int64_t *sup_hmac_sha256_i64(const int64_t *key, int64_t kn,
                                          const int64_t *msg, int64_t mn, int64_t *ret_len) {
  size_t klen = kn > 0 ? (size_t)kn : 0;
  size_t mlen = mn > 0 ? (size_t)mn : 0;
  uint8_t kraw[64], mstack[256], kbuf[64], dig[32], mac[32];
  uint8_t *kb = kraw;
  uint8_t *mb = mstack;
  if (klen > 64) {
    kb = (uint8_t *)sup_ralloc(klen, 1);
    for (size_t i = 0; i < klen; i++) kb[i] = (uint8_t)(key ? key[i] : 0);
  } else {
    for (size_t i = 0; i < klen; i++) kraw[i] = (uint8_t)(key ? key[i] : 0);
  }
  if (mlen > 256) {
    mb = (uint8_t *)sup_ralloc(mlen, 1);
    for (size_t i = 0; i < mlen; i++) mb[i] = (uint8_t)(msg ? msg[i] : 0);
  } else {
    for (size_t i = 0; i < mlen; i++) mstack[i] = (uint8_t)(msg ? msg[i] : 0);
  }
  memset(kbuf, 0, 64);
  if (klen > 64) { sup_sha256_raw(kb, klen, dig); memcpy(kbuf, dig, 32); }
  else if (klen) memcpy(kbuf, kb, klen);
  uint8_t inner_stack[64 + 256];
  uint8_t *inner = inner_stack;
  size_t inner_n = 64 + mlen;
  if (mlen > 256) inner = (uint8_t *)sup_ralloc(inner_n, 1);
  uint8_t outer[64 + 32];
  for (int i = 0; i < 64; i++) { inner[i] = (uint8_t)(kbuf[i] ^ 0x36); outer[i] = (uint8_t)(kbuf[i] ^ 0x5c); }
  if (mlen) memcpy(inner + 64, mb, mlen);
  sup_sha256_raw(inner, inner_n, dig);
  memcpy(outer + 64, dig, 32);
  sup_sha256_raw(outer, 64 + 32, mac);
  /* Goose-stack digest (digest returned as `[32]u8` by value): frame mark rewind reclaims it —
     region `sup_ralloc` would retain every MAC across 30k HTTP requests. */
  int64_t *out = sup_arena_i64(32);
  for (int i = 0; i < 32; i++) out[i] = (int64_t)mac[i];
  if (ret_len) *ret_len = 32;
  return out;
}
/* Pack i64 byte-lanes into `dst` (clamped to `cap`); returns byte count. */
static inline size_t sup_i64_lanes_to_bytes(const int64_t *src, int64_t sn, uint8_t *dst, size_t cap) {
  size_t n = sn > 0 ? (size_t)sn : 0;
  if (n > cap) n = cap;
  for (size_t i = 0; i < n; i++) dst[i] = (uint8_t)(src ? src[i] : 0);
  return n;
}
/* HMAC-SHA256 of `msg[lo..hi)` into `mac[32]` (stack). Caller supplies exclusive `hi`
   into a live buffer — no `strlen` of the whole wire (30k HTTP verifies).
   Caches ipad/opad *midstates* (one compress each) for the last key pointer so short
   bearer claims finish in ~2 compresses instead of ~4. */
static inline void sup_hmac_sha256_raw_range(const int64_t *key, int64_t kn,
                                            const char *msg, int64_t lo, int64_t hi,
                                            uint8_t mac[32]) {
  static const uint32_t IV[8] = {
      0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
      0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u};
  static _Thread_local const int64_t *cached_key = NULL;
  static _Thread_local int64_t cached_kn = -1;
  static _Thread_local uint32_t cached_istate[8];
  static _Thread_local uint32_t cached_ostate[8];
  const char *ms = msg ? msg : "";
  size_t a = lo <= 0 ? 0 : (size_t)lo;
  size_t b = hi <= 0 ? 0 : (size_t)hi;
  if (b < a) b = a;
  const uint8_t *mb = (const uint8_t *)(ms + a);
  size_t mlen = b - a;
  if (!(key && kn == cached_kn && key == cached_key)) {
    size_t klen = kn > 0 ? (size_t)kn : 0;
    uint8_t kraw[64], kbuf[64], dig[32], ipad[64], opad[64];
    uint8_t *kb = kraw;
    if (klen > 64) {
      kb = (uint8_t *)sup_ralloc(klen, 1);
      for (size_t i = 0; i < klen; i++) kb[i] = (uint8_t)(key ? key[i] : 0);
    } else {
      klen = sup_i64_lanes_to_bytes(key, kn, kraw, 64);
    }
    memset(kbuf, 0, 64);
    if (klen > 64) { sup_sha256_raw(kb, klen, dig); memcpy(kbuf, dig, 32); }
    else if (klen) memcpy(kbuf, kb, klen);
    for (int i = 0; i < 64; i++) {
      ipad[i] = (uint8_t)(kbuf[i] ^ 0x36);
      opad[i] = (uint8_t)(kbuf[i] ^ 0x5c);
    }
    memcpy(cached_istate, IV, sizeof IV);
    memcpy(cached_ostate, IV, sizeof IV);
    sup_sha256_compress(cached_istate, ipad);
    sup_sha256_compress(cached_ostate, opad);
    cached_key = key;
    cached_kn = kn;
  }
  uint32_t ih[8], oh[8];
  uint8_t dig[32];
  memcpy(ih, cached_istate, sizeof ih);
  sup_sha256_finish_from(ih, mb, mlen, 64, dig);
  memcpy(oh, cached_ostate, sizeof oh);
  /* Outer always absorbs the 32-byte inner digest after the opad block. */
  sup_sha256_finish_from(oh, dig, 32, 64, mac);
}
/* HMAC over msg[lo..hi) with i64 key lanes (no slice alloc for claims / wire spans). */
static inline int64_t *sup_hmac_sha256_i64_str_range(const int64_t *key, int64_t kn,
                                                    const char *msg, int64_t lo, int64_t hi,
                                                    int64_t *ret_len) {
  uint8_t mac[32];
  sup_hmac_sha256_raw_range(key, kn, msg, lo, hi, mac);
  int64_t *out = sup_arena_i64(32);
  for (int i = 0; i < 32; i++) out[i] = (int64_t)mac[i];
  if (ret_len) *ret_len = 32;
  return out;
}
/* HMAC with i64 key lanes and a raw C-string message (UTF-8 / ASCII wire; no msg temp array). */
static inline int64_t *sup_hmac_sha256_i64_str(const int64_t *key, int64_t kn,
                                              const char *msg, int64_t *ret_len) {
  const char *ms = msg ? msg : "";
  return sup_hmac_sha256_i64_str_range(key, kn, ms, 0, (int64_t)strlen(ms), ret_len);
}
/* First `nbytes` of `mac` equal lowercase hex at `text[lo..]` (truncated MAC / sig8).
   Walks only the compared span — no full-buffer `strlen`. */
static inline bool sup_sha2_hex_prefix_eq(const int64_t *mac, int64_t nbytes,
                                         const char *text, int64_t lo) {
  static const char HEX[] = "0123456789abcdef";
  if (!mac || !text || nbytes < 0 || lo < 0) return false;
  for (int64_t i = 0; i < nbytes; i++) {
    size_t p = (size_t)lo + (size_t)(2 * i);
    char c0 = text[p], c1 = text[p + 1];
    if (!c0 || !c1) return false;
    uint8_t b = (uint8_t)mac[i];
    if (c0 != HEX[b >> 4] || c1 != HEX[b & 15]) return false;
  }
  return true;
}
/* Stack HMAC + truncated hex compare (bearer verify) — no arena digest. */
static inline bool sup_hmac_sha256_hex_prefix_eq_range(
    const int64_t *key, int64_t kn, const char *msg, int64_t lo, int64_t hi,
    int64_t nbytes, const char *text, int64_t tlo) {
  static const char HEX[] = "0123456789abcdef";
  if (!key || !msg || !text || nbytes < 0 || tlo < 0) return false;
  uint8_t mac[32];
  sup_hmac_sha256_raw_range(key, kn, msg, lo, hi, mac);
  for (int64_t i = 0; i < nbytes && i < 32; i++) {
    size_t p = (size_t)tlo + (size_t)(2 * i);
    char c0 = text[p], c1 = text[p + 1];
    if (!c0 || !c1) return false;
    uint8_t b = mac[i];
    if (c0 != HEX[b >> 4] || c1 != HEX[b & 15]) return false;
  }
  return true;
}
/* Append lowercase hex of the first 4 MAC bytes of HMAC(key, msg[lo..hi)) into a StrBuilder. */
static inline void sup_hmac_sha256_sig8_into_range(
    int64_t sb_h, const int64_t *key, int64_t kn,
    const char *msg, int64_t lo, int64_t hi) {
  static const char HEX[] = "0123456789abcdef";
  uint8_t mac[32];
  char hex[8];
  sup_hmac_sha256_raw_range(key, kn, msg, lo, hi, mac);
  for (int i = 0; i < 4; i++) {
    hex[2 * i] = HEX[mac[i] >> 4];
    hex[2 * i + 1] = HEX[mac[i] & 15];
  }
  sup_sb_litn(sb_h, hex, 8);
}
/* Same as sig8_into_range after `mac[0] ^= 1` (forged bearer emit; stack only). */
static inline void sup_hmac_sha256_sig8_forged_into_range(
    int64_t sb_h, const int64_t *key, int64_t kn,
    const char *msg, int64_t lo, int64_t hi) {
  static const char HEX[] = "0123456789abcdef";
  uint8_t mac[32];
  char hex[8];
  sup_hmac_sha256_raw_range(key, kn, msg, lo, hi, mac);
  mac[0] ^= 1;
  for (int i = 0; i < 4; i++) {
    hex[2 * i] = HEX[mac[i] >> 4];
    hex[2 * i + 1] = HEX[mac[i] & 15];
  }
  sup_sb_litn(sb_h, hex, 8);
}
/* IEEE CRC-32 of a StrBuilder's live bytes (no `strlen` / no `text()` peek). */
static inline int64_t sup_crc32_sb(int64_t h) {
  SupSb *b = (SupSb *)(uintptr_t)h;
  if (!b || !b->p || b->n <= 0) return 0;
  uint32_t *tbl = sup_crc32_tbl();
  uint32_t crc = 0xFFFFFFFFu;
  const unsigned char *p = (const unsigned char *)b->p;
  size_t n = (size_t)b->n;
  for (size_t i = 0; i < n; i++)
    crc = tbl[(crc ^ p[i]) & 0xFFu] ^ (crc >> 8);
  return (int64_t)(crc ^ 0xFFFFFFFFu);
}

static inline void sup_i64_merge(int64_t *restrict a, int64_t an, int64_t *restrict tmp, int64_t tn,
                                 int64_t lo, int64_t mid, int64_t hi) {
  if (lo < 0 || hi > an || hi > tn || lo > mid || mid > hi) return;
  int64_t i = lo, j = mid, k = lo;
  while (i < mid && j < hi) tmp[k++] = (a[i] <= a[j]) ? a[i++] : a[j++];
  while (i < mid) tmp[k++] = a[i++];
  while (j < hi) tmp[k++] = a[j++];
  for (int64_t t = lo; t < hi; ++t) a[t] = tmp[t];
}
/* Dense gather: dst[i] ← src[idx[i]] (clamped; OOB → 0). */
static inline void sup_i64_gather(int64_t *restrict dst, int64_t dn, int64_t *restrict src, int64_t sn,
                                  int64_t *restrict idx, int64_t in) {
  int64_t n = dn < in ? dn : in;
  for (int64_t i = 0; i < n; ++i) {
    int64_t j = idx[i];
    dst[i] = (j >= 0 && j < sn) ? src[j] : 0;
  }
}
/* take_dense: idx guaranteed in-range — no OOB branch. */
static inline void sup_i64_gather_dense(int64_t *restrict dst, int64_t dn, int64_t *restrict src, int64_t sn,
                                        int64_t *restrict idx, int64_t in) {
  (void)sn;
  int64_t n = dn < in ? dn : in;
  for (int64_t i = 0; i < n; ++i) dst[i] = src[idx[i]];
}
static inline void sup_f64_gather(double *restrict dst, int64_t dn, double *restrict src, int64_t sn,
                                  int64_t *restrict idx, int64_t in) {
  int64_t n = dn < in ? dn : in;
  for (int64_t i = 0; i < n; ++i) {
    int64_t j = idx[i];
    dst[i] = (j >= 0 && j < sn) ? src[j] : 0.0;
  }
}
static inline void sup_f64_gather_dense(double *restrict dst, int64_t dn, double *restrict src, int64_t sn,
                                        int64_t *restrict idx, int64_t in) {
  (void)sn;
  int64_t n = dn < in ? dn : in;
  for (int64_t i = 0; i < n; ++i) dst[i] = src[idx[i]];
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ty_join_is_a_flat_lattice_with_numeric_widening() {
        assert_eq!(Ty::Bot.join(Ty::I64), Ty::I64);
        assert_eq!(Ty::I64.join(Ty::Bot), Ty::I64);
        assert_eq!(Ty::I64.join(Ty::F64), Ty::F64);
        assert_eq!(Ty::F64.join(Ty::I64), Ty::F64);
        assert_eq!(Ty::I64.join(Ty::I64), Ty::I64);
        assert_eq!(Ty::Str.join(Ty::I64), Ty::Top);
        assert_eq!(Ty::ArrI64.join(Ty::ArrF64), Ty::Top);
        assert_eq!(Ty::Top.join(Ty::Bot), Ty::Top);
    }

    #[test]
    fn ctag_maps_to_value_classes() {
        assert_eq!(Ty::from_tag(CTag::I64), Some(Ty::I64));
        assert_eq!(Ty::from_tag(CTag::ArrF64), Some(Ty::ArrF64));
        assert_eq!(Ty::from_tag(CTag::Dyn), None);
        assert_eq!(Ty::arr_of(Ty::F64), Some(Ty::ArrF64));
        assert_eq!(Ty::ArrStr.elem(), Some(Ty::Str));
    }

    #[test]
    fn affine_forms_add_scale_and_cancel() {
        let i = Lin::var(7);
        let two_i_plus_3 = i.scale(2).add(&Lin::constant(3), 1);
        assert_eq!(two_i_plus_3.terms, vec![(7, 2)]);
        assert_eq!(two_i_plus_3.off, 3);
        let zero = i.add(&i, -1);
        assert!(zero.terms.is_empty(), "i - i cancels");
        let two_vars = i.add(&Lin::var(9), 1);
        assert_eq!(two_vars.terms.len(), 2);
    }

    #[test]
    fn constant_folding_follows_vm_semantics() {
        assert!(matches!(
            fold(BinK::Div, Cst::I(7), Cst::I(0)),
            Some(Cst::I(0))
        ));
        assert!(matches!(
            fold(BinK::Rem, Cst::I(7), Cst::I(0)),
            Some(Cst::I(0))
        ));
        assert!(matches!(fold(BinK::Add, Cst::I(1), Cst::F(0.5)), Some(Cst::F(f)) if f == 1.5));
        assert!(matches!(
            fold(BinK::Lt, Cst::I(1), Cst::I(2)),
            Some(Cst::B(true))
        ));
        assert!(matches!(
            fold(BinK::Add, Cst::I(i64::MAX), Cst::I(1)),
            Some(Cst::I(i64::MIN))
        ));
    }

    #[test]
    fn c_literals_are_well_formed() {
        assert_eq!(lit_i(5), "5LL");
        assert_eq!(lit_i(-5), "(-5LL)");
        assert_eq!(lit_i(i64::MIN), "(-9223372036854775807LL-1)");
        assert_eq!(lit_f(1.0), "1.0");
        assert_eq!(lit_f(-2.5), "(-2.5)");
        assert_eq!(lit_f(f64::INFINITY), "INFINITY");
        assert_eq!(sanitize("a.b-c"), "a_b_c");
        assert_eq!(sanitize(""), "anon");
    }

    #[test]
    fn state_join_widens_int_to_float_and_drops_disagreeing_constants() {
        let mut a = State::new(2);
        let mut b = State::new(2);
        a.locals = vec![Ty::I64, Ty::I64];
        b.locals = vec![Ty::F64, Ty::I64];
        a.lcst = vec![Some(Cst::I(1)), Some(Cst::I(4))];
        b.lcst = vec![Some(Cst::F(1.0)), Some(Cst::I(4))];
        let j = a.join(&b).unwrap();
        assert_eq!(j.locals, vec![Ty::F64, Ty::I64]);
        assert!(j.lcst[0].is_none());
        assert!(matches!(j.lcst[1], Some(Cst::I(4))));
        let mut c = State::new(2);
        c.stack.push(Ty::I64);
        assert!(a.join(&c).is_err(), "stack depth mismatch must be rejected");
    }

    #[test]
    fn state_join_keeps_agreeing_string_keys_only() {
        let mut a = State::new(0);
        let mut b = State::new(0);
        a.stack = vec![Ty::Str, Ty::Str, Ty::I64];
        b.stack = vec![Ty::Str, Ty::Str, Ty::I64];
        a.skey = vec![Some(3), Some(4), None];
        b.skey = vec![Some(3), Some(5), None];
        let j = a.join(&b).unwrap();
        assert_eq!(j.skey, vec![Some(3), None, None]);
        assert!(a.same(&a.clone()));
        assert!(!a.same(&b), "differing string keys are different states");
    }

    #[test]
    fn exposed_reads_distinguishes_private_temporaries_from_carried_values() {
        // body: r1 = r0 (r0 read first => exposed); r2 = 1; use r2 (not exposed).
        let ops = vec![
            Op::LoadLocal(0),
            Op::StoreLocal(1),
            Op::LoadConst(0),
            Op::StoreLocal(2),
            Op::LoadLocal(2),
            Op::Pop,
        ];
        let chunk = Chunk {
            ops,
            ..Chunk::default()
        };
        let ex = exposed_reads(&chunk, 0, 6, 4);
        assert!(ex.contains(&0));
        assert!(!ex.contains(&2), "defined before use within the iteration");
        assert!(!ex.contains(&1));
    }

    #[test]
    fn slot_liveness_stops_at_redefinition() {
        let ops = vec![Op::StoreLocal(3), Op::LoadLocal(3), Op::Return];
        let chunk = Chunk {
            ops,
            ..Chunk::default()
        };
        assert!(
            !slot_live_after(&chunk, 0, 3, 3, 4),
            "redefined before read"
        );
        assert!(slot_live_after(&chunk, 1, 3, 3, 4), "read directly");
    }

    fn fields(spec: &[(&str, Ty)]) -> Vec<(String, Ty)> {
        spec.iter().map(|(n, t)| (n.to_string(), *t)).collect()
    }

    #[test]
    fn struct_class_joins_only_with_itself() {
        assert_eq!(Ty::Struct(3).join(Ty::Struct(3)), Ty::Struct(3));
        assert_eq!(Ty::Struct(3).join(Ty::Bot), Ty::Struct(3));
        assert_eq!(Ty::Struct(3).join(Ty::Struct(4)), Ty::Top);
        assert_eq!(Ty::Struct(3).join(Ty::F64), Ty::Top);
        assert_eq!(Ty::Struct(3).join(Ty::Unit), Ty::Top);
        assert!(Ty::Struct(0).has_repr());
        assert!(Ty::Struct(0).is_field());
        assert!(!Ty::Struct(0).is_scalar() && !Ty::Struct(0).is_arr());
        assert!(!Ty::Unit.is_field() && !Ty::Top.is_field() && !Ty::Bot.is_field());
        assert!(
            Ty::ArrEmpty.is_field(),
            "`[]` defaults are legal struct fields"
        );
    }

    #[test]
    fn struct_arrays_are_growable_typed_arrays() {
        let a = Ty::ArrStruct(2);
        assert!(a.is_arr() && a.is_iter_arr() && !a.is_flat_arr());
        assert!(a.is_field() && a.has_repr());
        assert_eq!(a.elem(), Some(Ty::Struct(2)));
        assert_eq!(Ty::arr_of(Ty::Struct(2)), Some(a));
        assert_eq!(Ty::grow_of(Ty::Struct(2)), Some(a));
        assert_eq!(a.cty(), "sup_s2_t *");
        assert_eq!(a.c_elem(), "sup_s2_t");
        assert_eq!(a.zero(), "nullptr");
        assert_eq!(
            names("l1", a),
            ("l1_aS2".to_string(), "l1_aS2_n".to_string())
        );
        // `[]` widens into the struct array; distinct element structs never mix.
        assert_eq!(Ty::ArrEmpty.join(a), a);
        assert_eq!(a.join(Ty::ArrEmpty), a);
        assert_eq!(a.join(a), a);
        assert_eq!(a.join(Ty::ArrStruct(3)), Ty::Top);
        assert_eq!(a.join(Ty::ArrI64), Ty::Top);
        assert_eq!(a.join(Ty::Struct(2)), Ty::Top);
    }

    #[test]
    fn empty_array_field_widens_to_struct_array_on_first_push() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let body = p
            .intern_struct(Some("Body".into()), &fields(&[("x", Ty::F64)]))
            .unwrap();
        let world = p
            .intern_struct(
                Some("World".into()),
                &fields(&[("bodies", Ty::ArrEmpty), ("g", Ty::F64)]),
            )
            .unwrap();
        assert!(!p.holds_arr(Ty::Struct(world)), "`[]` owns no storage yet");
        assert!(p.boxable(world), "an empty field boxes as an empty array");
        assert!(
            !p.unboxable(world),
            "boxed pushes into `[]` would have nowhere to go"
        );
        assert!(p.unbox_pending(world), "a later pass may still widen `[]`");
        p.changed = false;
        // Re-registering with `[]` again is a no-op.
        p.intern_struct(
            Some("World".into()),
            &fields(&[("bodies", Ty::ArrEmpty), ("g", Ty::F64)]),
        )
        .unwrap();
        assert!(!p.changed);
        // First push (`w.bodies = arr_push(w.bodies, b)`) fixes the element class.
        assert_eq!(
            p.widen_field(world, "bodies", Ty::ArrStruct(body)),
            Ok(true)
        );
        assert!(p.changed);
        let f = p.structs[world as usize].field("bodies").unwrap().ty;
        assert_eq!(f, Ty::ArrStruct(body));
        // Later `[]` literals fit the widened field; other element classes conflict.
        assert_eq!(p.widen_field(world, "bodies", Ty::ArrEmpty), Ok(false));
        assert!(p.widen_field(world, "bodies", Ty::ArrI64).is_err());
        assert!(p.holds_arr(Ty::Struct(world)));
        assert!(p.boxable(world) && p.unboxable(world));
        assert!(!p.unbox_pending(world));
        // Scalar `[]` fields widen to the scalar arrays as before.
        let q = p
            .intern_struct(Some("Q".into()), &fields(&[("xs", Ty::ArrEmpty)]))
            .unwrap();
        assert_eq!(p.widen_field(q, "xs", Ty::ArrI64), Ok(true));
        assert_eq!(p.structs[q as usize].field("xs").unwrap().ty, Ty::ArrI64);
    }

    #[test]
    fn empty_array_field_reserves_its_length_member() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        assert!(
            p.intern_struct(None, &fields(&[("a", Ty::ArrEmpty), ("a__n", Ty::I64)]))
                .is_err()
        );
    }

    #[test]
    fn struct_array_sizes_feed_the_stack_budget() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let v = p
            .intern_struct(
                None,
                &fields(&[("x", Ty::F64), ("y", Ty::F64), ("z", Ty::F64)]),
            )
            .unwrap();
        assert_eq!(p.size_of(Ty::Struct(v)), 24);
        assert_eq!(p.elem_bytes(Ty::ArrStruct(v)), 24);
        assert_eq!(p.elem_bytes(Ty::ArrI64), 8);
        assert_eq!(p.elem_bytes(Ty::ArrAI), 16);
    }

    #[test]
    fn self_referential_struct_arrays_are_not_boxable() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let node = p
            .intern_struct(
                Some("Node".into()),
                &fields(&[("kids", Ty::ArrEmpty), ("v", Ty::I64)]),
            )
            .unwrap();
        p.widen_field(node, "kids", Ty::ArrStruct(node)).unwrap();
        assert!(
            !p.boxable(node),
            "no finite boxing order for a recursive struct"
        );
        assert_eq!(struct_helper_order(&p), vec![node]);
    }

    #[test]
    fn struct_array_typedefs_forward_declare_and_box_late_element_structs() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        // `World2D.{bodies: []}` registers before `Body` exists.
        let world = p
            .intern_struct(
                Some("World2D".into()),
                &fields(&[("bodies", Ty::ArrEmpty), ("gy", Ty::F64)]),
            )
            .unwrap();
        let empty = p
            .intern_struct(Some("E".into()), &fields(&[("xs", Ty::ArrEmpty)]))
            .unwrap();
        let body = p
            .intern_struct(
                Some("Body".into()),
                &fields(&[("x", Ty::F64), ("y", Ty::F64)]),
            )
            .unwrap();
        p.widen_field(world, "bodies", Ty::ArrStruct(body)).unwrap();
        let t = struct_typedefs(&p);
        let fwd = t.find("typedef struct sup_s2_t sup_s2_t;").unwrap();
        let def = t.find("typedef struct sup_s0_t { /* World2D */").unwrap();
        assert!(fwd < def, "element struct forward-declared first:\n{t}");
        for member in [
            "  sup_s2_t *bodies;",
            "  int64_t bodies__n;",
            "  double gy;",
        ] {
            assert!(t.contains(member), "missing `{member}` in\n{t}");
        }
        let e = &t[t.find("typedef struct sup_s1_t {").unwrap()..];
        let e = &e[..e.find("} sup_s1_t;").unwrap()];
        assert!(e.contains("char sup_empty_;") && !e.contains("xs"), "{e}");
        assert_eq!(empty, 1);
        let h = struct_helpers(&p);
        let box_body = h.find("static inline SuperVal sup_box_s2(").unwrap();
        let box_world = h.find("static inline SuperVal sup_box_s0(").unwrap();
        assert!(
            box_body < box_world,
            "element helpers precede their users:\n{h}"
        );
        assert!(
            h.contains("sup_box_as2(const sup_s2_t *p, int64_t n)"),
            "{h}"
        );
        assert!(h.contains("sup_box_as2(v.bodies, v.bodies__n)"), "{h}");
        assert!(
            h.contains("super_obj_put(o, \"xs\", super_arr(super_arr_alloc(0)));"),
            "{h}"
        );
        assert!(
            h.lines()
                .all(|l| !(l.starts_with("static ") && l.ends_with(';')))
        );
    }

    #[test]
    fn struct_class_c_spelling() {
        assert_eq!(Ty::Struct(7).cty(), "sup_s7_t ");
        assert_eq!(Ty::Struct(7).zero(), "{}");
        assert_eq!(
            names("l2", Ty::Struct(7)),
            ("l2_S7".to_string(), String::new())
        );
        assert_eq!(c_member("x"), "x");
        assert_eq!(c_member("default"), "default_");
        assert_eq!(c_member("int"), "int_");
        assert_eq!(c_member("_Float64"), "_Float64_");
        assert_eq!(len_member("cells"), "cells__n");
    }

    #[test]
    fn struct_registry_interns_by_label_and_field_set() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let a = p
            .intern_struct(Some("P".into()), &fields(&[("x", Ty::F64), ("y", Ty::F64)]))
            .unwrap();
        assert!(p.changed);
        p.changed = false;
        // Same label + field set in another order is the same class.
        let b = p
            .intern_struct(Some("P".into()), &fields(&[("y", Ty::F64), ("x", Ty::F64)]))
            .unwrap();
        assert_eq!(a, b);
        assert!(!p.changed, "re-registering an identical shape is a no-op");
        // Field order is the first site's (C layout / boxed key order).
        let order: Vec<&str> = p.structs[a as usize]
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(order, ["x", "y"]);
        // A different label or field set is a different class.
        let q = p
            .intern_struct(Some("Q".into()), &fields(&[("x", Ty::F64), ("y", Ty::F64)]))
            .unwrap();
        let anon = p
            .intern_struct(None, &fields(&[("x", Ty::F64), ("y", Ty::F64)]))
            .unwrap();
        let wider = p
            .intern_struct(Some("P".into()), &fields(&[("x", Ty::F64)]))
            .unwrap();
        assert_eq!(
            BTreeSet::from([a, q, anon, wider]).len(),
            4,
            "distinct shapes must not alias"
        );
    }

    #[test]
    fn struct_fields_widen_monotonically_and_reject_conflicts() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let id = p
            .intern_struct(Some("R".into()), &fields(&[("w", Ty::I64), ("s", Ty::Str)]))
            .unwrap();
        p.changed = false;
        // I64 ⊔ F64 = F64 widens in place and dirties the plan.
        p.intern_struct(Some("R".into()), &fields(&[("w", Ty::F64), ("s", Ty::Str)]))
            .unwrap();
        assert_eq!(p.structs[id as usize].field("w").unwrap().ty, Ty::F64);
        assert!(p.changed);
        p.changed = false;
        // Narrower values fit the wider field without change.
        assert_eq!(p.widen_field(id, "w", Ty::I64), Ok(false));
        assert!(!p.changed);
        // Conflicting classes and unknown fields are errors (boxed fallback).
        assert!(p.widen_field(id, "s", Ty::I64).is_err());
        assert!(p.widen_field(id, "nope", Ty::I64).is_err());
        assert!(
            p.intern_struct(None, &fields(&[("u", Ty::Unit)])).is_err(),
            "null-valued fields have no C representation"
        );
        let e = p
            .intern_struct(None, &fields(&[("e", Ty::ArrEmpty)]))
            .expect("`[]` default fields are legal");
        assert_eq!(p.structs[e as usize].field("e").unwrap().ty, Ty::ArrEmpty);
        assert_eq!(p.widen_field(e, "e", Ty::ArrF64), Ok(true));
        assert_eq!(p.structs[e as usize].field("e").unwrap().ty, Ty::ArrF64);
    }

    #[test]
    fn struct_members_never_collide() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        // `a` (array) owns `a__n`, so a field spelled `a__n` must be rejected.
        assert!(
            p.intern_struct(None, &fields(&[("a", Ty::ArrI64), ("a__n", Ty::I64)]))
                .is_err()
        );
        // `int` → `int_` collides with a literal `int_` field.
        assert!(
            p.intern_struct(None, &fields(&[("int", Ty::I64), ("int_", Ty::I64)]))
                .is_err()
        );
        // A scalar named like a length member is fine without the matching array.
        assert!(
            p.intern_struct(None, &fields(&[("a", Ty::I64), ("a__n", Ty::I64)]))
                .is_ok()
        );
    }

    #[test]
    fn optional_return_join_keeps_the_value_class() {
        assert_eq!(ret_join(Ty::Unit, Ty::I64, true), (Ty::I64, true));
        assert_eq!(ret_join(Ty::ArrStr, Ty::Unit, true), (Ty::ArrStr, true));
        assert_eq!(
            ret_join(Ty::Struct(3), Ty::Unit, true),
            (Ty::Struct(3), true)
        );
        // Module code and value-less classes keep the plain lattice.
        assert_eq!(ret_join(Ty::Unit, Ty::I64, false), (Ty::Top, false));
        assert_eq!(ret_join(Ty::Unit, Ty::Range, true), (Ty::Top, false));
        assert_eq!(ret_join(Ty::Unit, Ty::Unit, true), (Ty::Unit, false));
        assert_eq!(ret_join(Ty::Bot, Ty::F64, true), (Ty::F64, false));
        assert_eq!(ret_join(Ty::I64, Ty::F64, true), (Ty::F64, false));
    }

    #[test]
    fn absent_return_yields_a_placeholder_per_class() {
        assert_eq!(absent_return(Ty::I64), "/*@ARENA@*/return 0;");
        assert_eq!(absent_return(Ty::F64), "/*@ARENA@*/return 0.0;");
        assert_eq!(absent_return(Ty::Bool), "/*@ARENA@*/return false;");
        assert_eq!(absent_return(Ty::Str), "/*@ARENA@*/return \"\";");
        assert_eq!(absent_return(Ty::Dyn), "/*@ARENA@*/return SUPER_NULL;");
        assert_eq!(
            absent_return(Ty::Struct(2)),
            "/*@ARENA@*/return (sup_s2_t){};"
        );
        for t in [Ty::ArrI64, Ty::ArrBool, Ty::ArrStruct(1)] {
            assert_eq!(
                absent_return(t),
                "{ *ret_len = 0; /*@ARENA@*/return nullptr; }"
            );
        }
        assert_eq!(absent_return(Ty::Unit), "/*@ARENA@*/return;");
    }

    #[test]
    fn bool_arrays_are_dense_flat_arrays() {
        assert_eq!(Ty::arr_of(Ty::Bool), Some(Ty::ArrBool));
        assert_eq!(Ty::grow_of(Ty::Bool), Some(Ty::ArrBool));
        assert_eq!(Ty::ArrBool.elem(), Some(Ty::Bool));
        assert!(Ty::ArrBool.is_arr() && Ty::ArrBool.is_flat_arr() && Ty::ArrBool.is_iter_arr());
        assert!(Ty::ArrBool.from_dyn());
        assert_eq!(Ty::ArrBool.cty(), "bool *");
        assert_eq!(Ty::ArrBool.c_elem(), "bool");
        assert_eq!(Ty::ArrEmpty.join(Ty::ArrBool), Ty::ArrBool);
        assert_eq!(Ty::ArrBool.join(Ty::ArrI64), Ty::Top);
        assert_eq!(Ty::Dyn.join(Ty::ArrBool), Ty::ArrBool);
    }

    #[test]
    fn dyn_merges_only_with_unboxable_classes() {
        for t in [
            Ty::I64,
            Ty::F64,
            Ty::Bool,
            Ty::Str,
            Ty::ArrF64,
            Ty::Struct(0),
            Ty::ArrStruct(0),
        ] {
            assert_eq!(Ty::Dyn.join(t), t);
            assert_eq!(t.join(Ty::Dyn), t);
        }
        assert_eq!(Ty::Dyn.join(Ty::Dyn), Ty::Dyn);
        assert_eq!(Ty::Dyn.join(Ty::Range), Ty::Top);
        assert_eq!(Ty::Dyn.join(Ty::ArrAI), Ty::Top);
        assert_eq!(Ty::Dyn.join(Ty::Unit), Ty::Top);
    }

    #[test]
    fn unresolved_instances_fail_leaves_and_bottom_cycles_first() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let ids: Vec<usize> = (0..6).map(|k| p.add_inst(None, vec![Ty::I64; k])).collect();
        // 0 → 1 → 2 (leaf); 3 ↔ 4 (cycle); 0 → 3; 5 resolved; 1 → 5.
        for (from, to) in [(0, 1), (1, 2), (3, 4), (4, 3), (0, 3), (1, 5)] {
            p.insts[ids[from]].callees.insert(ids[to]);
        }
        for k in 0..5 {
            p.insts[ids[k]].saw_bot = true;
        }
        let mut doomed = unresolved_bottom(&p);
        doomed.sort();
        assert_eq!(
            doomed,
            [2, 3, 4],
            "callers wait for their unresolved callees"
        );
        for k in doomed {
            p.insts[k].failed = Some("x".into());
        }
        // Next sweep: 1 is now a leaf among the open instances; 0 still waits on it.
        assert_eq!(unresolved_bottom(&p), [1]);
        p.insts[1].failed = Some("x".into());
        assert_eq!(unresolved_bottom(&p), [0]);
        p.insts[0].failed = Some("x".into());
        assert!(unresolved_bottom(&p).is_empty());
    }

    #[test]
    fn struct_array_and_boxing_capabilities() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let flat = p
            .intern_struct(
                Some("V".into()),
                &fields(&[("x", Ty::F64), ("tag", Ty::Str)]),
            )
            .unwrap();
        let grid = p
            .intern_struct(Some("G".into()), &fields(&[("cells", Ty::ArrI64)]))
            .unwrap();
        let names = p
            .intern_struct(Some("N".into()), &fields(&[("ss", Ty::ArrStr)]))
            .unwrap();
        let outer = p
            .intern_struct(
                Some("O".into()),
                &fields(&[("v", Ty::Struct(flat)), ("g", Ty::Struct(grid))]),
            )
            .unwrap();
        assert!(!p.holds_arr(Ty::Struct(flat)));
        assert!(p.holds_arr(Ty::Struct(grid)));
        assert!(
            p.holds_arr(Ty::Struct(outer)),
            "nested array storage escapes too"
        );
        assert!(p.holds_arr(Ty::ArrF64) && !p.holds_arr(Ty::F64));
        assert!(p.boxable(flat) && p.unboxable(flat));
        assert!(p.boxable(grid) && p.unboxable(grid));
        assert!(
            p.boxable(names) && p.unboxable(names),
            "string arrays bridge by copy"
        );
        assert!(p.boxable(outer) && p.unboxable(outer));
        let flags = p
            .intern_struct(Some("F".into()), &fields(&[("bs", Ty::ArrBool)]))
            .unwrap();
        assert!(p.boxable(flags) && p.unboxable(flags));
        let rows = p
            .intern_struct(Some("R".into()), &fields(&[("gs", Ty::ArrStruct(grid))]))
            .unwrap();
        assert!(
            p.boxable(rows) && p.unboxable(rows),
            "struct arrays bridge element-wise"
        );
        let pending = p
            .intern_struct(
                Some("P".into()),
                &fields(&[("o", Ty::Struct(outer)), ("later", Ty::ArrEmpty)]),
            )
            .unwrap();
        assert!(p.boxable(pending) && !p.unboxable(pending));
        let host = p
            .intern_struct(Some("H".into()), &fields(&[("ps", Ty::ArrStruct(pending))]))
            .unwrap();
        assert!(
            p.unbox_pending(host),
            "pending `[]` is found through struct arrays"
        );
        assert!(!p.unbox_pending(outer));
    }

    #[test]
    fn struct_typedefs_and_helpers_are_emitted_in_dependency_order() {
        let chunk = Chunk::default();
        let interner = Interner::new();
        let mut p = Plan::new(&chunk, &interner, HashSet::new());
        let v = p
            .intern_struct(
                Some("Vec2".into()),
                &fields(&[("x", Ty::F64), ("y", Ty::F64)]),
            )
            .unwrap();
        p.intern_struct(
            Some("Body".into()),
            &fields(&[
                ("pos", Ty::Struct(v)),
                ("hits", Ty::I64),
                ("default", Ty::Bool),
                ("trail", Ty::ArrF64),
            ]),
        )
        .unwrap();
        let t = struct_typedefs(&p);
        let vec2 = t.find("typedef struct sup_s0_t { /* Vec2 */").unwrap();
        let body = t.find("typedef struct sup_s1_t { /* Body */").unwrap();
        assert!(vec2 < body, "nested struct defined first:\n{t}");
        for member in [
            "  sup_s0_t pos;",
            "  int64_t hits;",
            "  bool default_;",
            "  double *trail;",
            "  int64_t trail__n;",
            "} sup_s1_t;",
        ] {
            assert!(t.contains(member), "missing `{member}` in\n{t}");
        }
        let h = struct_helpers(&p);
        assert!(
            h.contains("static inline SuperVal sup_box_s1(sup_s1_t v) {"),
            "{h}"
        );
        assert!(h.contains("super_obj_alloc(4, 0, \"Body\")"), "{h}");
        assert!(
            h.contains("super_obj_put(o, \"pos\", sup_box_s0(v.pos));"),
            "{h}"
        );
        assert!(
            h.contains("super_obj_put(o, \"default\", super_bool(v.default_));"),
            "{h}"
        );
        assert!(h.contains("sup_view_f64(v.trail, v.trail__n)"), "{h}");
        assert!(
            h.contains("static inline sup_s0_t sup_unbox_s0(SuperVal o) {"),
            "{h}"
        );
        assert!(
            h.contains("static inline sup_s1_t sup_unbox_s1(SuperVal o) {"),
            "{h}"
        );
        assert!(
            h.contains("r.pos = sup_unbox_s0(super_obj_get(o, \"pos\"));"),
            "{h}"
        );
        assert!(
            h.contains("r.trail = sup_unbox_af(super_obj_get(o, \"trail\"), &r.trail__n);"),
            "{h}"
        );
        assert!(
            h.contains("static inline sup_s1_t *sup_unbox_as1(SuperVal v, int64_t *n)"),
            "{h}"
        );
        // Helper definitions must not look like prototypes to the incremental exporter.
        assert!(
            h.lines()
                .all(|l| !(l.starts_with("static ") && l.ends_with(';')))
        );
        assert!(struct_typedefs(&Plan::new(&chunk, &interner, HashSet::new())).is_empty());
    }

    #[test]
    fn unused_labels_are_stripped_but_goto_targets_kept() {
        let body = "  L_3:;\n    x = 1;\n    goto L_12;\n  L_1:;\n  L_12:;\n    if (c) goto L_120;\n  L_120:;\n";
        let out = strip_unused_labels(body);
        assert_eq!(
            out,
            "    x = 1;\n    goto L_12;\n  L_12:;\n    if (c) goto L_120;\n  L_120:;\n"
        );
        assert_eq!(strip_unused_labels(""), "");
    }

    #[test]
    fn names_encode_class_suffixes() {
        assert_eq!(names("l3", Ty::I64), ("l3_i".to_string(), String::new()));
        assert_eq!(
            names("l3", Ty::ArrF64),
            ("l3_af".to_string(), "l3_af_n".to_string())
        );
        assert_eq!(
            names("it", Ty::Range),
            ("it_rlo".to_string(), "it_rhi".to_string())
        );
    }

    #[test]
    fn sig_param_names_strip_types_and_qualifiers() {
        assert_eq!(sig_param_names("void"), "");
        assert_eq!(sig_param_names(""), "");
        assert_eq!(
            sig_param_names("int64_t l0_i, const char *l1_s, sup_s0_t *restrict m0"),
            "l0_i, l1_s, m0"
        );
        assert_eq!(
            sig_param_names("double *restrict l2_af, int64_t l2_af_n"),
            "l2_af, l2_af_n"
        );
    }
}
