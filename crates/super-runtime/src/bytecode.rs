//! Compact bytecode for the register-pinned VM.
//!
//! Includes fused superinstructions produced by [`crate::opt`] peephole passes.

use crate::value::Value;
use super_core::SymId;

#[derive(Clone, Debug)]
pub enum Op {
    LoadConst(u16),
    LoadLocal(u16),
    StoreLocal(u16),
    /// Register move: `dst = src`. Identity `Move(r,r)` is stripped by peephole.
    Move(u16, u16),
    LoadGlobal(SymId),
    StoreGlobal(SymId),
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    OrLog,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    BitNot,
    /// `acc = acc + imm` (top of stack).
    AddImm(i64),
    SubImm(i64),
    MulImm(i64),
    /// Integer `acc <<= shamt` (power-of-two multiply strength reduction).
    ShlImm(u8),
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    /// Pop `b`, pop `a`; jump if `a < b`.
    JumpIfLt(u32),
    JumpIfGe(u32),
    JumpIfEq(u32),
    JumpIfNe(u32),
    Call(u16, u8), // func index, arity
    /// Like `Call`, but writes mutated `@` params back to caller locals on return.
    /// `(func, arity, wb_id)` — `chunk.mut_writebacks[wb_id]` is
    /// `[(caller_slot, param_idx), …]` copied from the callee frame.
    CallMut(u16, u8, u16),
    /// Push a bare function value (`Value::Func`) for later `CallValue`.
    MakeFn(u16),
    /// Pop `n` capture values (by-value), push `Value::Closure { fn_idx, env }`.
    MakeClosure(u16, u8),
    /// Pop `arity` args then a `Func`/`Closure` callable; invoke it.
    CallValue(u8),
    CallHost(SymId, u8),
    Return,
    Pop,
    Dup,
    MakeArray(u16),
    MakeMap(u16),
    MakeStruct(Option<SymId>, u16),
    /// Tagged sum / enum value: stack has `n_fields` (key, value) pairs like MakeStruct.
    MakeSum {
        name: SymId,
        tag: u32,
        n_fields: u16,
    },
    /// Pop a sum value; push `true` if its tag equals `tag`.
    IsVariant(u32),
    /// Pop a sum (or int) value; push its discriminant as i64.
    SumTag,
    GetField(SymId),
    SetField(SymId),
    /// C-ABI field load by byte offset (ExtRef / packed struct).
    LoadFieldOff(u32),
    StoreFieldOff(u32),
    IndexGet,
    IndexSet,
    /// Bounds-check-elided index ops (LICM/BCE proved `0 <= i < len`).
    LoadSliceUnchecked,
    StoreSliceUnchecked,
    MakeRange,
    IterBegin,
    IterNext(u32), // jump if done
    /// Pop the current iterator (for `break` out of `for`).
    IterPop,
    /// Increment local counter, compare to limit local, jump to body while in range.
    /// Stack-neutral. `LoopStep { counter, limit, body }`.
    LoopStep {
        counter: u16,
        limit: u16,
        body: u32,
    },
    PushIt,
    PushIndex,
    /// Pop value; write into the current `for` element (`iter.items[index-1]`).
    /// Leaves the (possibly COW-updated) array on the stack for local write-back.
    IterSetIt,
    OrElse,
    /// Push `true` if top-of-stack is none/null/error (does not pop).
    IsAbsent,
    Fail,
    /// Unrecoverable abort — message already on the stack (or default).
    Panic,
    AssertEq,
    Print,
    Nop,
    Halt,
    // --- Structured concurrency / channels / shared ---
    ScopeEnter,
    ScopeExit,
    /// Push a task handle; body is a compiled thunk function index.
    Spawn(u16),
    Await,
    ChanNew,
    ChanSend,
    ChanRecv,
    ChanClose,
    ChanTryRecv,
    SharedNew,
    SharedLoad,
    SharedAdd,
    /// Push duration nanos from Int/Float top-of-stack + unit tag (0=ms,1=us,2=s).
    MakeDuration(u8),
    /// Parallel each: top is collection; body between ParEachBegin and jump target.
    ParEachBegin(u32),
    /// Select multiplex: number of arms follows as packed metadata in consts — simplified:
    /// stack layout [arm0_kind, arm0_val, ...] then SelectWhen(n) leaves (index, value).
    SelectWhen(u8),
}

#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub ops: Vec<Op>,
    pub consts: Vec<Value>,
    pub functions: Vec<FunctionChunk>,
    pub tests: Vec<TestChunk>,
    /// Optional optimizer annotations for `super disasm`.
    pub opt_notes: Vec<OptNote>,
    /// Writeback lists for [`Op::CallMut`]: `(caller_local_slot, callee_param_idx)`.
    pub mut_writebacks: Vec<Vec<(u16, u16)>>,
    /// Bytecode PC → escape birth region for MakeMap/MakeStruct/MakeSum
    /// (`1` = request arena Stay, `2` = durable Promote/Conservative).
    /// Absent → runtime Auto (arena while a request is active).
    pub escape_birth: Vec<(u32, u8)>,
}

/// Human-readable optimizer breadcrumbs attached to a bytecode index.
#[derive(Clone, Debug)]
pub struct OptNote {
    pub at: u32,
    pub kind: OptNoteKind,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub enum OptNoteKind {
    Folded,
    DeadBranch,
    Sroa,
    LicmHoist,
    BoundsCheckElim,
    Fused,
    RegRecycle,
    /// A language trait selected a direct method instead of the builtin witness.
    TraitDispatch,
}

/// Static C value class of a parameter / local / return slot, derived from the
/// `TypedModule` (declared types) and refined by the AOT emitter's flow analysis.
///
/// `Dyn` means "no static type known" — the C emitter keeps such code on the boxed
/// `SuperVal` path unless whole-program monomorphization can prove a concrete tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CTag {
    #[default]
    Dyn,
    I64,
    F64,
    Bool,
    Str,
    ArrI64,
    ArrF64,
    /// `void` / `()` — functions that only produce `null`.
    Unit,
}

impl CTag {
    /// `true` for tags that lower to a plain C scalar (`int64_t` / `double` / `bool`).
    pub fn is_scalar(self) -> bool {
        matches!(self, CTag::I64 | CTag::F64 | CTag::Bool)
    }

    /// `true` for dense array tags (`T *restrict` + length in unboxed C).
    pub fn is_array(self) -> bool {
        matches!(self, CTag::ArrI64 | CTag::ArrF64)
    }

    /// Short stable mnemonic used by `super disasm` and tests.
    pub fn name(self) -> &'static str {
        match self {
            CTag::Dyn => "dyn",
            CTag::I64 => "i64",
            CTag::F64 => "f64",
            CTag::Bool => "bool",
            CTag::Str => "str",
            CTag::ArrI64 => "arr_i64",
            CTag::ArrF64 => "arr_f64",
            CTag::Unit => "unit",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FunctionChunk {
    /// Short name (`new`, `advance`). Methods from different types may share it.
    pub name: SymId,
    /// Owning type for nested methods (`App`); `None` for free functions.
    pub owner: Option<SymId>,
    pub arity: u8,
    pub local_count: u16,
    pub entry: u32,
    pub param_names: Vec<SymId>,
    /// Per-param `@` mutability (downward borrow); parallel to `param_names`.
    pub mut_params: Vec<bool>,
    /// `//# inline` / `=>` body — emit `always_inline` in C17 AOT.
    pub always_inline: bool,
    /// Live intervals `[start_op, end_op)` per local slot (for disasm).
    pub live_intervals: Vec<(u32, u32)>,
    /// Declared C value class per parameter (parallel to `param_names`); `Dyn` when untyped.
    pub param_tags: Vec<CTag>,
    /// Declared C value class per local slot (`local_count` entries once compiled).
    /// Parameter slots mirror `param_tags`; other slots are `Dyn` unless the binding
    /// carried an explicit type annotation.
    pub local_tags: Vec<CTag>,
    /// Declared return class (`Dyn` when the source left it to inference).
    pub ret_tag: CTag,
}

impl FunctionChunk {
    /// Chunk with no type metadata — every tag is [`CTag::Dyn`] (synthetic thunks, tests).
    pub fn untyped(
        name: SymId,
        owner: Option<SymId>,
        arity: u8,
        local_count: u16,
        entry: u32,
    ) -> Self {
        Self {
            name,
            owner,
            arity,
            local_count,
            entry,
            param_names: Vec::new(),
            mut_params: Vec::new(),
            always_inline: false,
            live_intervals: Vec::new(),
            param_tags: Vec::new(),
            local_tags: Vec::new(),
            ret_tag: CTag::Dyn,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestChunk {
    pub name: SymId,
    pub entry: u32,
}

impl Chunk {
    pub fn push(&mut self, op: Op) -> u32 {
        let i = self.ops.len() as u32;
        self.ops.push(op);
        i
    }

    pub fn add_const(&mut self, v: Value) -> u16 {
        let i = self.consts.len() as u16;
        self.consts.push(v);
        i
    }

    pub fn note(&mut self, at: u32, kind: OptNoteKind, detail: impl Into<String>) {
        self.opt_notes.push(OptNote {
            at,
            kind,
            detail: detail.into(),
        });
    }

    pub fn patch_jump(&mut self, at: u32, target: u32) {
        match &mut self.ops[at as usize] {
            Op::Jump(t)
            | Op::JumpIfFalse(t)
            | Op::JumpIfTrue(t)
            | Op::JumpIfLt(t)
            | Op::JumpIfGe(t)
            | Op::JumpIfEq(t)
            | Op::JumpIfNe(t)
            | Op::IterNext(t)
            | Op::ParEachBegin(t) => {
                *t = target;
            }
            Op::LoopStep { body, .. } => {
                *body = target;
            }
            _ => {}
        }
    }
}
