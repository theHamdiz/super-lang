//! Arena-friendly AST kinds for Super (nodes stored densely; IDs used at call sites).

use crate::token::Token;
use super_core::{Span, SymId};

#[derive(Clone, Debug)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
    /// Module documentation from `//!` and `/*! … */`.
    pub docs: SymId,
    /// Macros visible in this module after expansion (for "write `#name`" hints).
    pub macros: Vec<SymId>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Use(UseItem),
    Binding(Binding),
    Struct(StructDef),
    Sum(SumDef),
    Function(Function),
    State(StateDef),
    Test(TestBlock),
    /// Typed algebraic rewrite rule: `rewrite "name" (...) { lhs => rhs }`.
    Rewrite(RewriteRule),
    /// `+Name = trait { signatures }` — a method contract declared inline on a type.
    Trait(TraitDef),
    /// `+#name { (pattern) => { template } … }` — a pattern macro.
    Macro(MacroDef),
}

/// Raw tokens kept for macro matching, plus which tokens begin a source line.
#[derive(Clone, Debug, Default)]
pub struct TokenStream {
    pub tokens: Vec<Token>,
    pub line_starts: Vec<bool>,
}

impl TokenStream {
    pub fn push(&mut self, token: Token, line_start: bool) {
        self.tokens.push(token);
        self.line_starts.push(line_start);
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct MacroDef {
    pub exported: bool,
    pub name: SymId,
    pub rules: Vec<MacroRule>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct MacroRule {
    pub pattern: TokenStream,
    pub template: TokenStream,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroDelim {
    /// `#name(…)`
    Paren,
    /// `#name { … }`
    Brace,
    /// `#name` with no input.
    Bare,
}

/// An unexpanded `#name(…)` / `#name { … }` invocation.
#[derive(Clone, Debug)]
pub struct MacroCall {
    pub name: SymId,
    pub delim: MacroDelim,
    pub input: TokenStream,
    pub span: Span,
    /// One-based line and column of the `#`.
    pub line: u32,
    pub column: u32,
    /// Source text covered by `span`, used for `#stringify` of call-site tokens.
    pub text: SymId,
}

/// Compile-time attribute attached via `//# …` or `/*# … #*/`.
#[derive(Clone, Debug)]
pub struct MetaAttr {
    pub kind: MetaAttrKind,
    pub span: Span,
    /// Raw directive text for diagnostics / LSP hover.
    pub raw: SymId,
}

#[derive(Clone, Debug)]
pub enum MetaAttrKind {
    Unroll(u32),
    Simd,
    Inline,
    Route {
        method: SymId,
        path: SymId,
    },
    System {
        stage: SymId,
    },
    AssertSize {
        ty: SymId,
        bytes: u32,
    },
    Inspect(SymId),
    /// `#law("description")` — human-locked behavioral invariant (verify-only).
    Law {
        description: SymId,
    },
    /// `#proof(law_ident)` — AI-authored discharge / lemma for a `#law` (verify-only).
    Proof {
        law: SymId,
    },
    /// `#symmetric` — synthesize the swapped-arity-2 mirror `f(b, a) => f(a, b)`.
    Symmetric,
    /// `#cfg(predicate)` — item is retained only when the build context matches.
    Cfg(SymId),
    /// Free-form / inline comptime statement payload (parsed separately).
    InlineComptime(SymId),
    Unknown,
}

impl Function {
    /// True when this function is a `#law` or `#proof` item (never emitted to runtime).
    pub fn is_verify_only(&self) -> bool {
        self.attrs.iter().any(|a| {
            matches!(
                a.kind,
                MetaAttrKind::Law { .. } | MetaAttrKind::Proof { .. }
            )
        })
    }

    pub fn law_description(&self) -> Option<SymId> {
        self.attrs.iter().find_map(|a| match &a.kind {
            MetaAttrKind::Law { description } => Some(*description),
            _ => None,
        })
    }

    pub fn proof_target(&self) -> Option<SymId> {
        self.attrs.iter().find_map(|a| match &a.kind {
            MetaAttrKind::Proof { law } => Some(*law),
            _ => None,
        })
    }

    pub fn is_law(&self) -> bool {
        self.law_description().is_some()
    }

    pub fn is_proof(&self) -> bool {
        self.proof_target().is_some()
    }

    pub fn is_symmetric(&self) -> bool {
        self.attrs
            .iter()
            .any(|a| matches!(a.kind, MetaAttrKind::Symmetric))
    }
}

#[derive(Clone, Debug)]
pub struct RewriteRule {
    pub name: SymId,
    pub params: Vec<Param>,
    pub lhs: Expr,
    pub rhs: Expr,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct UseItem {
    /// Module path segments: `auth` → `auth.sr`, `std.net.http` → `net/http.sr`.
    pub path: Vec<SymId>,
    /// `use std.gui as App` — local name of the module. `None` keeps the last segment.
    pub alias: Option<SymId>,
    /// Braced names after the path. Two meanings (resolved in `modules`):
    /// - Sibling modules: `use std.data.{column, frame}` ≡ two `use` lines when
    ///   every name resolves as a module under the path prefix.
    /// - Export selectors: `use models.{User, Task}` when names are `+` exports
    ///   of the path module (`None` means import every `+` export).
    pub selectors: Option<Vec<SymId>>,
    /// `use std.gui.{Gui as Desk}` — exported name, then the local name.
    pub renames: Vec<(SymId, SymId)>,
    /// Names made available by module resolution. Empty before project loading.
    pub bindings: Vec<SymId>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub exported: bool,
    pub mutable: bool,
    /// `#Name = …` compile-time binding.
    pub comptime: bool,
    pub name: SymId,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
    pub attrs: Vec<MetaAttr>,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub exported: bool,
    pub name: SymId,
    pub fields: Vec<FieldDef>,
    /// Methods attached to the type (UFCS / chaining). First param is typically `@self`.
    pub methods: Vec<Function>,
    /// Traits declared inline on the type (`+Toggle: Widget = { … }`).
    pub traits: Vec<SymId>,
    pub span: Span,
    pub attrs: Vec<MetaAttr>,
    pub docs: SymId,
}

/// A method contract. Methods are signatures; the body is written on each type.
#[derive(Clone, Debug)]
pub struct TraitDef {
    pub exported: bool,
    pub name: SymId,
    pub methods: Vec<TraitMethod>,
    pub span: Span,
    pub docs: SymId,
}

/// One required method. `params` includes the receiver.
#[derive(Clone, Debug)]
pub struct TraitMethod {
    pub name: SymId,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct FieldDef {
    pub private: bool,
    pub name: SymId,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct SumDef {
    pub exported: bool,
    pub name: SymId,
    pub variants: Vec<VariantDef>,
    /// Methods attached to the sum type (UFCS / chaining), same as [`StructDef::methods`].
    pub methods: Vec<Function>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct VariantDef {
    pub name: SymId,
    pub fields: Vec<(SymId, TypeExpr)>,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub exported: bool,
    pub name: SymId,
    /// When set, this is a method on that type (receiver is params[0]).
    pub owner: Option<SymId>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: FnBody,
    pub span: Span,
    pub attrs: Vec<MetaAttr>,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub mutable: bool, // @param
    /// `#N: usize` compile-time value parameter.
    pub comptime: bool,
    pub name: SymId,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum FnBody {
    Expr(Expr),
    Block(Block),
}

/// Capture recorded by sema for a lambda (`name` by value, or `@name` mutable).
#[derive(Clone, Debug)]
pub struct LambdaCapture {
    pub name: SymId,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct StateDef {
    pub name: SymId,
    pub value: Expr,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct TestBlock {
    pub name: SymId,
    pub body: Block,
    pub span: Span,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Binding(Binding),
    /// Destructuring: `{a, b} = expr` or `{a, b: c} = expr`
    Destructure {
        fields: Vec<(SymId, SymId)>, // (source_field, local_name)
        value: Expr,
        span: Span,
    },
    Expr(Expr),
    Return(Option<Expr>, Span),
    Ensure {
        cond: Expr,
        else_block: Block,
        span: Span,
    },
    Each {
        mutable: bool,
        /// `par …` / `for parallel …` — split across M:N workers with SIMD-friendly non-aliasing.
        parallel: bool,
        /// Optional reduction op for `par y in 0..H (+) { expr }` (currently `+` only).
        reduction: Option<BinOp>,
        iter: Expr,
        bindings: Vec<EachBinding>,
        body: Block,
        span: Span,
        attrs: Vec<MetaAttr>,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    /// `loop { … }` — infinite loop (desugars like `while true`).
    Loop {
        body: Block,
        span: Span,
    },
    /// Exit the innermost `for`/`while`/`loop`.
    Break(Span),
    /// Continue the innermost loop.
    Continue(Span),
    /// `a, b = b, a` / `x, y, z = 1, 2, 3` — RHS evaluated fully before LHS stores.
    MultiAssign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
        span: Span,
    },
    /// Postfix guard: `stmt if cond` (break/continue/return/assign forms).
    Guarded {
        stmt: Box<Stmt>,
        cond: Expr,
        span: Span,
    },
    When {
        /// Empty → multiplex form: `select { recv => … after 16.ms => … }`
        /// (legacy alias: subject-less `when { … }`).
        /// One subject → classic `when s { … }`. Two or more → multi-method
        /// `when (e1, e2, …)` static/sum-tag dispatch.
        subjects: Vec<Expr>,
        arms: Vec<WhenArm>,
        span: Span,
    },
    /// Structured concurrency: all `spawn`s inside finish before exit.
    Scope {
        body: Block,
        span: Span,
    },
    Fail(Expr, Span),
    /// `panic` / `panic <msg>` — unrecoverable abort (not a `T!` error).
    Panic(Option<Expr>, Span),
    /// `#{ … }` / `#if` / `#for` as statements (after expansion may be empty).
    Comptime(ComptimeStmt),
}

#[derive(Clone, Debug)]
pub enum ComptimeStmt {
    Block(Block, Span),
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    Each {
        name: SymId,
        iter: Expr,
        body: Block,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub struct EachBinding {
    pub mutable: bool,
    pub name: SymId,
    pub ty: Option<SymId>,
}

#[derive(Clone, Debug)]
pub struct WhenArm {
    pub pattern: WhenPattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum WhenPattern {
    Else,
    Expr(Expr),
    /// `after 16.ms` arm in a multiplex `when` block.
    After(Expr),
    /// `Status.Paid { amount }` / `Status.Paid { amount: a }` — tag match + payload binders.
    VariantBind {
        sum: SymId,
        variant: SymId,
        /// `(payload_field, local_name)` — punning uses the same id twice.
        fields: Vec<(SymId, SymId)>,
        span: Span,
    },
    /// `(P1, P2, …)` multi-method arm paired with `when (e1, e2, …)`.
    Tuple {
        elems: Vec<WhenPattern>,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    Named(SymId, Span),
    Optional(Box<TypeExpr>, Span),
    Error(Box<TypeExpr>, Span),
    Path(Vec<SymId>, Span),
    /// `chan[T]` element type wrapper used by `chan[T](cap)`.
    Chan(Box<TypeExpr>, Span),
    /// Meta type parameter: `T: type`.
    Type(Span),
    /// Quoted AST payload: `Code[T]`.
    Code(Box<TypeExpr>, Span),
    /// `Name[Arg, …]` — a type constructor applied to arguments, resolved by name
    /// in sema (the builtin `Fn[Params…, Ret]` is the callable type).
    Apply {
        name: SymId,
        args: Vec<TypeExpr>,
        span: Span,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurationUnit {
    Ms,
    Us,
    S,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Ident(SymId, Span),
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(SymId, Span),
    None(Span),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        /// When true, trailing `.` punning fills args from same-named locals.
        punning: bool,
        span: Span,
    },
    /// Slice / range index: `msg[lo..hi]`
    Slice {
        base: Box<Expr>,
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: SymId,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    Array(Vec<Expr>, Span),
    /// `[val; N]` — compile-time-sized stack array filled with `val`.
    ArrayRepeat {
        value: Box<Expr>,
        len: Box<Expr>,
        span: Span,
    },
    /// `cond ? then_expr : else_expr`
    Ternary {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        span: Span,
    },
    Map(Vec<MapEntry>, Span),
    Record(Vec<RecordField>, Span),
    StructLit {
        name: Option<SymId>,
        fields: Vec<RecordField>,
        span: Span,
    },
    InferredStruct {
        fields: Vec<RecordField>,
        span: Span,
    },
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    Block(Block),
    Or {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    OrErr {
        left: Box<Expr>,
        body: Block,
        span: Span,
    },
    Await(Box<Expr>, Span),
    /// `spawn expr` / `spawn { … }` — returns a task handle.
    Spawn {
        body: Box<Expr>,
        span: Span,
    },
    /// `chan[T](capacity)` — bounded ownership-transferring channel.
    Chan {
        elem_ty: TypeExpr,
        capacity: Box<Expr>,
        span: Span,
    },
    /// `shared 0` / `shared .{…}` — atomic or futex-backed shared cell.
    Shared {
        value: Box<Expr>,
        span: Span,
    },
    /// `16.ms` / `2.s` / `500.us` duration literal (lowered from field sugar).
    Duration {
        value: Box<Expr>,
        unit: DurationUnit,
        span: Span,
    },
    Pipeline {
        base: Box<Expr>,
        method: SymId,
        args: Vec<Expr>,
        span: Span,
    },
    /// Compile-time file embedding: `embed "assets/logo.png"`.
    Embed {
        path: SymId,
        span: Span,
    },
    It(Span),
    IndexIdent(Span),
    ErrIdent(Span),
    NamedVariant {
        sum: SymId,
        variant: SymId,
        args: Vec<Expr>,
        span: Span,
    },
    /// Hygienic quote: `'{ expr }` → `Code[T]`.
    Quote {
        body: Box<Expr>,
        span: Span,
    },
    /// Splice: `${ code_expr }` — expand `Code[T]` into enclosing AST.
    Splice {
        code: Box<Expr>,
        span: Span,
    },
    /// `#ident` / `#expr` compile-time value reference.
    ComptimeRef {
        name: SymId,
        span: Span,
    },
    /// `#{ stmts… }` compile-time block expression (result = last value / array builder).
    ComptimeBlock {
        body: Block,
        span: Span,
    },
    /// `#if cond { … } else { … }`
    ComptimeIf {
        cond: Box<Expr>,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    /// `#for item in list { … }`
    ComptimeEach {
        name: SymId,
        iter: Box<Expr>,
        body: Block,
        span: Span,
    },
    /// `#goal_type` — inferred goal / expected type reflection.
    GoalType(Span),
    /// Dynamic field via comptime symbol: `val.(field_expr)`.
    ComptimeField {
        base: Box<Expr>,
        field: Box<Expr>,
        span: Span,
    },
    /// Anonymous function: `|x: i64| => x * 2` / `|@acc, x| { … }`.
    /// `captures` is filled by sema (by-value; `@` = mutable).
    Lambda {
        params: Vec<Param>,
        ret: Option<TypeExpr>,
        body: Box<FnBody>,
        captures: Vec<LambdaCapture>,
        span: Span,
    },
    /// Diverging panic expression (`panic`, `#todo`, …). Never returns.
    Panic {
        message: Option<Box<Expr>>,
        span: Span,
    },
    /// `#name(…)` / `#name { … }` before macro expansion. Never reaches sema.
    MacroCall(Box<MacroCall>),
}

#[derive(Clone, Debug)]
pub struct MapEntry {
    pub key: MapKey,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub enum MapKey {
    Ident(SymId),
    String(SymId),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct RecordField {
    pub name: Option<SymId>, // None = punning from value Ident
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
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
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    MutRef, // @expr
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Ident(_, s)
            | Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Bool(_, s)
            | Expr::String(_, s)
            | Expr::None(s)
            | Expr::It(s)
            | Expr::IndexIdent(s)
            | Expr::ErrIdent(s)
            | Expr::Await(_, s) => *s,
            Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Slice { span, .. }
            | Expr::Field { span, .. }
            | Expr::Index { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Array(_, span)
            | Expr::ArrayRepeat { span, .. }
            | Expr::Ternary { span, .. }
            | Expr::Map(_, span)
            | Expr::Record(_, span)
            | Expr::StructLit { span, .. }
            | Expr::InferredStruct { span, .. }
            | Expr::Range { span, .. }
            | Expr::If { span, .. }
            | Expr::Or { span, .. }
            | Expr::OrErr { span, .. }
            | Expr::Pipeline { span, .. }
            | Expr::Embed { span, .. }
            | Expr::Spawn { span, .. }
            | Expr::Chan { span, .. }
            | Expr::Shared { span, .. }
            | Expr::Duration { span, .. }
            | Expr::NamedVariant { span, .. }
            | Expr::Quote { span, .. }
            | Expr::Splice { span, .. }
            | Expr::ComptimeRef { span, .. }
            | Expr::ComptimeBlock { span, .. }
            | Expr::ComptimeIf { span, .. }
            | Expr::ComptimeEach { span, .. }
            | Expr::GoalType(span)
            | Expr::ComptimeField { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Panic { span, .. } => *span,
            Expr::MacroCall(call) => call.span,
            Expr::Block(b) => b.span,
        }
    }
}

/// Marker type for future arena-backed AST storage.
pub struct Ast;
