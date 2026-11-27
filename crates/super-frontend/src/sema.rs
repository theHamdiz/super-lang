//! Single-pass type inference and C-ABI layout resolution.

use crate::ast::*;
use std::collections::{HashMap, HashSet};
use super_core::{Arena, DiagnosticKind, Diagnostics, Interner, Span, SymId, TypeId, err};

#[derive(Clone, Debug)]
pub enum TypeKind {
    Void,
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    Optional(TypeId),
    Error(TypeId),
    Array(TypeId),
    Map {
        key: TypeId,
        value: TypeId,
    },
    Struct {
        name: SymId,
        fields: Vec<FieldLayout>,
        size: u32,
        align: u32,
    },
    Sum {
        name: SymId,
        variants: Vec<VariantLayout>,
        size: u32,
    },
    Func {
        params: Vec<TypeId>,
        ret: TypeId,
    },
    Named(SymId),
    /// Imported module namespace (`http` after `use std.net.http`).
    Module {
        name: SymId,
    },
    Infer,
    Never,
    /// Bounded channel of element type.
    Chan(TypeId),
    /// Task / fiber handle (result of `spawn`).
    Task(TypeId),
    /// `shared T` atomic/futex cell.
    Shared(TypeId),
    /// Duration (nanoseconds internally).
    Duration,
    /// Meta type parameter (`T: type`).
    TypeMeta,
    /// Quoted AST `Code[T]`.
    Code(TypeId),
    /// Trait-constrained generic parameter (`x: Matrix`) — monomorphized at call sites.
    TraitBound {
        name: SymId,
    },
}

#[derive(Clone, Debug)]
pub struct FieldLayout {
    pub name: SymId,
    pub ty: TypeId,
    pub offset: u32,
    pub private: bool,
    pub docs: SymId,
}

#[derive(Clone, Debug)]
pub struct VariantLayout {
    pub name: SymId,
    pub tag: u32,
    pub fields: Vec<(SymId, TypeId)>,
    pub docs: SymId,
}

#[derive(Clone, Debug, Default)]
pub struct TypedModule {
    pub types: Arena<TypeKind>,
    pub expr_types: HashMap<u64, TypeId>, // keyed by span lo|hi
    pub symbols: Vec<SymbolInfo>,
    pub symbol_references: Vec<SymbolReference>,
    pub bindings: HashMap<SymId, BindingInfo>,
    /// Documentation for module-level bindings and `state` items.
    pub binding_docs: HashMap<SymId, SymId>,
    /// Documentation for struct and sum types.
    pub type_docs: HashMap<SymId, SymId>,
    pub functions: HashMap<SymId, FunctionInfo>,
    /// All overloads sharing a function key (`FuncFamily`). Includes the primary in
    /// [`Self::functions`] plus any additional arity/type specializations.
    pub families: HashMap<SymId, Vec<FunctionInfo>>,
    pub structs: HashMap<SymId, TypeId>,
    pub sums: HashMap<SymId, TypeId>,
    /// `use path.{Name as Local}` and function renames. Local name → canonical name.
    pub aliases: HashMap<SymId, SymId>,
    /// Trait name → required method names, in declaration order.
    pub traits: HashMap<SymId, Vec<SymId>>,
    /// Trait name → one signature line per required method (`paint(self) : str`).
    pub trait_sigs: HashMap<SymId, Vec<String>>,
    /// Type name → traits it implements.
    pub impls: HashMap<SymId, Vec<SymId>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageClass {
    Register,
    StackLocal,
    StackArray,
    ConstExpr,
    Param,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Local,
    Param,
    Binding,
    Import,
    LoopBinder,
    Const,
}

#[derive(Clone, Debug)]
pub struct SymbolInfo {
    pub name: SymId,
    pub ty: TypeId,
    pub mutable: bool,
    pub storage: StorageClass,
    pub def_span: Span,
    pub kind: SymbolKind,
}

#[derive(Clone, Copy, Debug)]
pub struct SymbolReference {
    pub name: SymId,
    pub span: Span,
    pub def_span: Span,
}

#[derive(Clone, Debug)]
pub struct BindingInfo {
    pub ty: TypeId,
    pub exported: bool,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FunctionInfo {
    pub ty: TypeId,
    pub exported: bool,
    pub params: Vec<(SymId, TypeId, bool)>,
    /// Per-parameter trait constraint when the annotation named a `trait` (else `None`).
    pub trait_params: Vec<Option<SymId>>,
    /// Specificity ranks per parameter: 4 concrete, 3 variant, 2 trait, 1 unconstrained.
    pub param_ranks: Vec<u8>,
    pub span: Span,
    /// Owning type for nested methods; `None` for free functions.
    pub owner: Option<SymId>,
    pub docs: SymId,
}

/// Specificity ranking for multiple dispatch (Concrete > Variant > Trait > _).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SpecRank {
    Unconstrained = 1,
    Trait = 2,
    Variant = 3,
    Concrete = 4,
}

/// One column of a tuple `when` arm, as precisely as overlap analysis can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArmCol {
    /// `_`: matches every value.
    Any,
    /// `Sum.Variant` (with or without payload bindings): matches exactly that variant.
    Variant(SymId, SymId),
    /// Literals, guards, types, traits: overlap is not decided statically.
    Opaque,
}

/// Where two tuple arms can both match.
enum Overlap {
    Disjoint,
    At(Vec<ArmCol>),
    Unknown,
}

impl ArmCol {
    fn intersect(a: &[ArmCol], b: &[ArmCol]) -> Overlap {
        if a.len() != b.len() {
            return Overlap::Unknown;
        }
        let mut meet = Vec::with_capacity(a.len());
        for (&x, &y) in a.iter().zip(b) {
            meet.push(match (x, y) {
                (ArmCol::Opaque, _) | (_, ArmCol::Opaque) => return Overlap::Unknown,
                (ArmCol::Any, c) | (c, ArmCol::Any) => c,
                (x, y) if x == y => x,
                _ => return Overlap::Disjoint,
            });
        }
        Overlap::At(meet)
    }

    /// Is `arm` exactly the overlap `meet` (an explicit arm claiming the shared cases)?
    fn owns(arm: &[ArmCol], meet: &[ArmCol]) -> bool {
        arm == meet
    }
}

/// Pipeline stages the compiler lowers itself (never resolved as free-function UFCS).
fn is_pipeline_builtin(name: &str) -> bool {
    matches!(
        name,
        "keep"
            | "map"
            | "sum"
            | "product"
            | "min"
            | "max"
            | "any"
            | "all"
            | "mean"
            | "variance"
            | "reduce"
            | "par_sum"
            | "par_product"
            | "par_min"
            | "par_max"
            | "par_mean"
            | "par_variance"
            | "par_reduce"
            | "group_by"
            | "send"
            | "recv"
            | "close"
            | "load"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageKind {
    Local,
    Parameter,
    Import,
}

#[derive(Clone, Copy, Debug)]
struct UsageInfo {
    span: Span,
    kind: UsageKind,
    used: bool,
}

pub struct SemanticAnalyzer<'a> {
    interner: &'a mut Interner,
    diagnostics: &'a mut Diagnostics,
    typed: TypedModule,
    scopes: Vec<HashMap<SymId, BindingInfo>>,
    usage_scopes: Vec<HashMap<SymId, UsageInfo>>,
    builtins: HashMap<&'static str, TypeId>,
    in_test: bool,
    /// Names imported from `use std.net.http` / `use std.game.ecs` / …
    std_imports: HashSet<String>,
    /// Imported symbol → import declaration group used for E0305 accounting.
    import_uses: HashMap<SymId, Vec<SymId>>,
    /// Nested `scope { … }` depth — enables cross-spawn `@` race checks.
    scope_depth: u32,
    /// Mutable `@` captures taken by `spawn` bodies inside the current scope.
    scope_mut_captures: Vec<HashSet<SymId>>,
    /// `state` bindings and other module-root mutables (lifetime checked on return).
    root_mutable: HashSet<SymId>,
    /// True while type-checking under a leading `@` (pipeline receiver already mut-ref).
    under_mut_ref: bool,
    /// First module item originating in the entry file after dependency merging.
    local_item_start: usize,
    /// Macros visible in the analyzed module, for `#name` migration hints.
    macro_names: Vec<SymId>,
    tracking_enabled: bool,
    /// When true (release / `--deny-unused`), E0303–E0305 are hard errors; else warnings (A7).
    unused_as_error: bool,
}

impl<'a> SemanticAnalyzer<'a> {
    fn arm_col(&self, pat: &WhenPattern) -> ArmCol {
        match pat {
            WhenPattern::Expr(Expr::Ident(n, _)) if self.interner.resolve(*n) == "_" => ArmCol::Any,
            WhenPattern::Expr(Expr::NamedVariant {
                sum, variant, args, ..
            }) if args.is_empty() => ArmCol::Variant(*sum, *variant),
            WhenPattern::Expr(Expr::Field { base, field, .. }) => match base.as_ref() {
                Expr::Ident(sum, _) if self.typed.sums.contains_key(sum) => {
                    ArmCol::Variant(*sum, *field)
                }
                _ => ArmCol::Opaque,
            },
            WhenPattern::VariantBind { sum, variant, .. } => ArmCol::Variant(*sum, *variant),
            _ => ArmCol::Opaque,
        }
    }

    pub fn new(interner: &'a mut Interner, diagnostics: &'a mut Diagnostics) -> Self {
        super_core::traits::intern_core(interner);
        let mut typed = TypedModule::default();
        let mut builtins = HashMap::new();
        let void = typed.types.alloc_type(TypeKind::Void);
        let bool_t = typed.types.alloc_type(TypeKind::Bool);
        let i8_t = typed.types.alloc_type(TypeKind::I8);
        let i16_t = typed.types.alloc_type(TypeKind::I16);
        let i32_t = typed.types.alloc_type(TypeKind::I32);
        let i64_t = typed.types.alloc_type(TypeKind::I64);
        let i128_t = typed.types.alloc_type(TypeKind::I128);
        let u8_t = typed.types.alloc_type(TypeKind::U8);
        let u16_t = typed.types.alloc_type(TypeKind::U16);
        let u32_t = typed.types.alloc_type(TypeKind::U32);
        let u64_t = typed.types.alloc_type(TypeKind::U64);
        let f32_t = typed.types.alloc_type(TypeKind::F32);
        let f64_t = typed.types.alloc_type(TypeKind::F64);
        let str_t = typed.types.alloc_type(TypeKind::Str);
        let infer = typed.types.alloc_type(TypeKind::Infer);
        let _ = void;
        builtins.insert("bool", bool_t);
        builtins.insert("i8", i8_t);
        builtins.insert("i16", i16_t);
        builtins.insert("i32", i32_t);
        builtins.insert("i64", i64_t);
        builtins.insert("int", i64_t);
        builtins.insert("i128", i128_t);
        builtins.insert("u8", u8_t);
        builtins.insert("u16", u16_t);
        builtins.insert("u32", u32_t);
        builtins.insert("u64", u64_t);
        builtins.insert("f32", f32_t);
        builtins.insert("f64", f64_t);
        builtins.insert("str", str_t);
        builtins.insert("_", infer);
        builtins.insert("duration", typed.types.alloc_type(TypeKind::Duration));
        Self {
            interner,
            diagnostics,
            typed,
            scopes: vec![HashMap::new()],
            usage_scopes: vec![HashMap::new()],
            builtins,
            in_test: false,
            std_imports: HashSet::new(),
            import_uses: HashMap::new(),
            scope_depth: 0,
            scope_mut_captures: Vec::new(),
            root_mutable: HashSet::new(),
            under_mut_ref: false,
            local_item_start: 0,
            macro_names: Vec::new(),
            tracking_enabled: true,
            unused_as_error: false,
        }
    }

    /// Release builds promote unused locals/params/imports to errors (A7).
    pub fn set_unused_as_error(&mut self, yes: bool) {
        self.unused_as_error = yes;
    }

    pub fn set_std_imports(&mut self, imports: HashSet<String>) {
        self.std_imports = imports;
    }

    pub fn set_local_item_start(&mut self, start: usize) {
        self.local_item_start = start;
    }

    pub fn analyze(mut self, module: &Module) -> TypedModule {
        self.macro_names.clone_from(&module.macros);
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            let Item::Use(import) = item else { continue };
            if let Some(&group) = import.path.last() {
                let local = import.alias.unwrap_or(group);
                self.track_definition(local, import.span, UsageKind::Import);
                let ty = self.builtin("_");
                self.record_symbol(
                    local,
                    ty,
                    false,
                    StorageClass::ConstExpr,
                    import.span,
                    SymbolKind::Import,
                );
                self.import_uses.entry(local).or_default().push(local);
                let brought: Vec<SymId> = if !import.bindings.is_empty() {
                    import.bindings.clone()
                } else {
                    import.selectors.clone().unwrap_or_default()
                };
                for binding in brought {
                    let visible = import
                        .renames
                        .iter()
                        .find(|(from, _)| *from == binding)
                        .map(|(_, to)| *to)
                        .unwrap_or(binding);
                    self.record_symbol(
                        visible,
                        ty,
                        false,
                        StorageClass::ConstExpr,
                        import.span,
                        SymbolKind::Import,
                    );
                    self.import_uses.entry(binding).or_default().push(local);
                    if visible != binding {
                        self.import_uses.entry(visible).or_default().push(local);
                    }
                }
            }
        }
        // Bind imported std namespaces (`http`, `ecs`, …) before name resolution.
        for ns in self.std_imports.clone() {
            if ns == "*" {
                continue;
            }
            let sym = self.interner.intern(&ns);
            let tid = self.typed.types.alloc_type(TypeKind::Module { name: sym });
            let span = self
                .typed
                .symbols
                .iter()
                .find(|symbol| symbol.name == sym && symbol.kind == SymbolKind::Import)
                .map_or_else(Span::default, |symbol| symbol.def_span);
            if let Some(symbol) = self
                .typed
                .symbols
                .iter_mut()
                .find(|symbol| symbol.name == sym && symbol.kind == SymbolKind::Import)
            {
                symbol.ty = tid;
            }
            let info = BindingInfo {
                ty: tid,
                exported: false,
                mutable: false,
                span,
            };
            self.typed.bindings.insert(sym, info.clone());
            self.scopes[0].insert(sym, info);
        }
        // Pass 1: register type names
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            match item {
                Item::Struct(s) => {
                    let tid = self.alloc_struct_placeholder(s);
                    self.typed.structs.insert(s.name, tid);
                }
                Item::Sum(s) => {
                    let tid = self.alloc_sum_placeholder(s);
                    self.typed.sums.insert(s.name, tid);
                }
                _ => {}
            }
        }
        // Pass 2: fill layouts
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            match item {
                Item::Struct(s) => self.define_struct(s),
                Item::Sum(s) => self.define_sum(s),
                _ => {}
            }
        }
        self.bind_import_renames(module);
        // Pass 2b: register trait names so `x: Trait` params resolve during declare.
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            if let Item::Trait(tr) = item {
                self.typed.traits.insert(
                    tr.name,
                    tr.methods.iter().map(|method| method.name).collect(),
                );
                self.typed.trait_sigs.insert(
                    tr.name,
                    tr.methods
                        .iter()
                        .map(|method| self.trait_method_line(method))
                        .collect(),
                );
            }
        }
        // Pass 3: functions & bindings
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            match item {
                Item::Function(f) => self.declare_function(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.declare_function(m);
                    }
                }
                Item::Binding(b) => {
                    let ty = self.check_binding(b);
                    self.record_symbol(
                        b.name,
                        ty,
                        b.mutable,
                        if b.comptime {
                            StorageClass::ConstExpr
                        } else {
                            self.storage_for_type(ty, false)
                        },
                        b.span,
                        if b.comptime {
                            SymbolKind::Const
                        } else {
                            SymbolKind::Binding
                        },
                    );
                    self.note_binding_docs(b.name, b.docs);
                    self.typed.bindings.insert(
                        b.name,
                        BindingInfo {
                            ty,
                            exported: b.exported,
                            mutable: b.mutable,
                            span: b.span,
                        },
                    );
                    self.scopes[0].insert(
                        b.name,
                        BindingInfo {
                            ty,
                            exported: b.exported,
                            mutable: b.mutable,
                            span: b.span,
                        },
                    );
                }
                Item::State(s) => {
                    let ty = self.check_expr(&s.value, None);
                    self.note_binding_docs(s.name, s.docs);
                    self.root_mutable.insert(s.name);
                    self.record_symbol(
                        s.name,
                        ty,
                        true,
                        StorageClass::StackLocal,
                        s.span,
                        SymbolKind::Binding,
                    );
                    self.scopes[0].insert(
                        s.name,
                        BindingInfo {
                            ty,
                            exported: true,
                            mutable: true,
                            span: s.span,
                        },
                    );
                }
                Item::Test(t) => {
                    self.in_test = true;
                    self.check_block(&t.body);
                    self.in_test = false;
                }
                Item::Rewrite(r) => {
                    // Type-check rewrite pattern expressions in a fresh scope.
                    self.push_scope();
                    for p in &r.params {
                        let ty = self.resolve_type_expr(&p.ty);
                        self.record_symbol(
                            p.name,
                            ty,
                            p.mutable,
                            if p.comptime {
                                StorageClass::ConstExpr
                            } else {
                                StorageClass::Param
                            },
                            p.span,
                            if p.comptime {
                                SymbolKind::Const
                            } else {
                                SymbolKind::Param
                            },
                        );
                        self.scopes.last_mut().unwrap().insert(
                            p.name,
                            BindingInfo {
                                ty,
                                exported: false,
                                mutable: p.mutable,
                                span: p.span,
                            },
                        );
                        self.track_definition(p.name, p.span, UsageKind::Parameter);
                    }
                    let _ = self.check_expr(&r.lhs, None);
                    let _ = self.check_expr(&r.rhs, None);
                    self.pop_scope();
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.declare_function(m);
                    }
                }
                Item::Use(_) | Item::Trait(_) | Item::Macro(_) => {}
            }
        }
        // Pass 4: function / method bodies
        for (index, item) in module.items.iter().enumerate() {
            self.tracking_enabled = index >= self.local_item_start;
            match item {
                Item::Function(f) => self.check_function_body(f),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.check_function_body(m);
                    }
                }
                Item::Sum(s) => {
                    for m in &s.methods {
                        self.check_function_body(m);
                    }
                }
                _ => {}
            }
        }
        self.tracking_enabled = true;
        self.check_trait_impls(module);
        self.check_language_traits(module);
        self.finish_usage_scope(0);
        self.typed
    }

    /// Every inline `: Trait` must name a trait and supply each required method.
    ///
    /// Arity, receiver mutability, parameter types, and the return type all
    /// have to match. The receiver's type is the owner on each side, so it is
    /// not compared. A trait parameter typed `_` accepts any implementation type.
    fn check_trait_impls(&mut self, module: &Module) {
        let mut required: HashMap<SymId, Vec<TraitMethod>> = HashMap::new();
        for item in &module.items {
            if let Item::Trait(tr) = item {
                self.typed.traits.insert(
                    tr.name,
                    tr.methods.iter().map(|method| method.name).collect(),
                );
                self.typed.trait_sigs.insert(
                    tr.name,
                    tr.methods
                        .iter()
                        .map(|method| self.trait_method_line(method))
                        .collect(),
                );
                required.insert(tr.name, tr.methods.clone());
            }
        }
        for item in &module.items {
            let Item::Struct(struct_def) = item else {
                continue;
            };
            if struct_def.traits.is_empty() {
                continue;
            }
            self.typed
                .impls
                .insert(struct_def.name, struct_def.traits.clone());
            for trait_name in &struct_def.traits {
                let Some(methods) = required.get(trait_name) else {
                    let trait_label = self.interner.resolve(*trait_name).to_string();
                    if super_core::traits::core_trait(&trait_label).is_some() {
                        self.check_core_impl(struct_def, &trait_label);
                        continue;
                    }
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Type,
                        struct_def.span,
                        err::E0203,
                        format!("unknown trait `{trait_label}`"),
                        "declare it with `+Name = trait { … }` before the type",
                    );
                    continue;
                };
                let type_label = self.interner.resolve(struct_def.name).to_string();
                let trait_label = self.interner.resolve(*trait_name).to_string();
                for need in methods {
                    let method_label = self.interner.resolve(need.name).to_string();
                    let Some(method) = struct_def
                        .methods
                        .iter()
                        .find(|method| method.name == need.name)
                    else {
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            struct_def.span,
                            err::E0203,
                            format!(
                                "`{type_label}` does not satisfy `{trait_label}`: missing `{method_label}`"
                            ),
                            format!(
                                "add `+{}`",
                                self.trait_method_line(need)
                            ),
                        );
                        continue;
                    };
                    if method.params.len() != need.params.len() {
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            method.span,
                            err::E0203,
                            format!(
                                "`{type_label}.{method_label}` has {} parameter(s); `{trait_label}` requires {}",
                                method.params.len(),
                                need.params.len()
                            ),
                            format!("write `+{}`", self.trait_method_line(need)),
                        );
                        continue;
                    }
                    self.check_trait_signature(
                        &type_label,
                        &trait_label,
                        &method_label,
                        need,
                        method,
                    );
                }
            }
        }
    }

    /// Language traits are ambient. A definition that drifts from the compiler
    /// table, or a method that matches one and is not declared, is `E0204`.
    fn check_language_traits(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Trait(tr) = item {
                let name = self.interner.resolve(tr.name).to_string();
                if let Some(expected) = super_core::traits::core_trait(&name) {
                    self.check_core_definition(tr, expected);
                }
            }
        }
        for item in &module.items {
            let Item::Struct(struct_def) = item else {
                continue;
            };
            for method in &struct_def.methods {
                let method_name = self.interner.resolve(method.name).to_string();
                let Some((expected_trait, expected)) =
                    super_core::traits::core_method(&method_name)
                else {
                    continue;
                };
                if !self.method_matches_core(method, expected) {
                    continue;
                }
                let declared = struct_def
                    .traits
                    .iter()
                    .any(|name| self.interner.resolve(*name) == expected_trait.name);
                if declared {
                    continue;
                }
                let type_label = self.interner.resolve(struct_def.name).to_string();
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    method.span,
                    err::E0204,
                    format!(
                        "`{type_label}.{method_name}` implements `{0}` but `{type_label}` does not declare it",
                        expected_trait.name
                    ),
                    format!(
                        "write `+{type_label}: {0}` so `+`, `==`, `\"{{…}}\"`, and the other witnesses call this method",
                        expected_trait.name
                    ),
                );
            }
        }
    }

    fn check_core_definition(
        &mut self,
        tr: &crate::ast::TraitDef,
        expected: &super_core::traits::CoreTrait,
    ) {
        if tr.methods.len() != expected.methods.len() {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                tr.span,
                err::E0204,
                format!(
                    "`{}` is a language trait and must declare exactly {} method(s)",
                    expected.name,
                    expected.methods.len()
                ),
                "match `std.core.traits`; the compiler witness and the source must agree",
            );
            return;
        }
        for need in expected.methods {
            let Some(got) = tr
                .methods
                .iter()
                .find(|method| self.interner.resolve(method.name) == need.name)
            else {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    tr.span,
                    err::E0204,
                    format!("`{}` is missing `{}`", expected.name, need.name),
                    "the language trait table is the witness used by every backend",
                );
                continue;
            };
            if !self.trait_method_matches_core(got, need) {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    got.span,
                    err::E0204,
                    format!(
                        "`{}.{}` does not match the language witness",
                        expected.name, need.name
                    ),
                    self.core_method_hint(need),
                );
            }
        }
    }

    fn check_core_impl(&mut self, struct_def: &StructDef, trait_label: &str) {
        let Some(expected) = super_core::traits::core_trait(trait_label) else {
            return;
        };
        let type_label = self.interner.resolve(struct_def.name);
        for need in expected.methods {
            let Some(method) = struct_def
                .methods
                .iter()
                .find(|method| self.interner.resolve(method.name) == need.name)
            else {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    struct_def.span,
                    err::E0203,
                    format!(
                        "`{type_label}` does not satisfy `{trait_label}`: missing `{}`",
                        need.name
                    ),
                    self.core_method_hint(need),
                );
                continue;
            };
            if !self.method_matches_core(method, need) {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    method.span,
                    err::E0203,
                    format!(
                        "`{type_label}.{}` does not match `{trait_label}`",
                        need.name
                    ),
                    self.core_method_hint(need),
                );
            }
        }
    }

    fn method_matches_core(
        &self,
        method: &Function,
        expected: &super_core::traits::CoreMethod,
    ) -> bool {
        if method.params.len() != expected.params as usize {
            return false;
        }
        let receiver = method.params.first();
        let is_self = receiver.is_some_and(|param| self.interner.resolve(param.name) == "self");
        if expected.has_self != is_self {
            return false;
        }
        if expected.has_self && receiver.is_some_and(|param| param.mutable) != expected.mutable_self
        {
            return false;
        }
        match expected.ret {
            Some(ret) => method
                .ret
                .as_ref()
                .is_some_and(|got| self.format_type_expr(got) == ret),
            None => true,
        }
    }

    fn trait_method_matches_core(
        &self,
        method: &crate::ast::TraitMethod,
        expected: &super_core::traits::CoreMethod,
    ) -> bool {
        if method.params.len() != expected.params as usize {
            return false;
        }
        let receiver = method.params.first();
        let is_self = receiver.is_some_and(|param| self.interner.resolve(param.name) == "self");
        if expected.has_self != is_self {
            return false;
        }
        if expected.has_self && receiver.is_some_and(|param| param.mutable) != expected.mutable_self
        {
            return false;
        }
        match expected.ret {
            Some(ret) => method
                .ret
                .as_ref()
                .is_some_and(|got| self.format_type_expr(got) == ret),
            None => true,
        }
    }

    fn core_method_hint(&self, method: &super_core::traits::CoreMethod) -> String {
        let mut line = String::new();
        if method.mutable_self {
            line.push('@');
        }
        if method.has_self {
            line.push_str("self");
            if method.params > 1 {
                line.push_str(", ");
                line.push_str("other");
            }
        } else if method.params == 1 {
            line.push_str("value");
        }
        let mut text = format!("+{}({line})", method.name);
        if let Some(ret) = method.ret {
            text.push_str(" : ");
            text.push_str(ret);
        }
        format!("write `{text}`")
    }

    fn check_trait_signature(
        &mut self,
        type_label: &str,
        trait_label: &str,
        method_label: &str,
        need: &TraitMethod,
        method: &Function,
    ) {
        for (need_param, got_param) in need.params.iter().zip(method.params.iter()) {
            if need_param.mutable != got_param.mutable {
                let expected = if need_param.mutable { "@self" } else { "self" };
                let found = if got_param.mutable { "@self" } else { "self" };
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    got_param.span,
                    err::E0203,
                    format!(
                        "`{type_label}.{method_label}` takes `{found}`; `{trait_label}` requires `{expected}`"
                    ),
                    format!("write `+{}`", self.trait_method_line(need)),
                );
            }
            let receiver = self.interner.resolve(need_param.name) == "self";
            if !receiver && !self.type_expr_satisfies(&need_param.ty, &got_param.ty) {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    got_param.span,
                    err::E0203,
                    format!(
                        "`{type_label}.{method_label}` parameter `{}` is `{}`; `{trait_label}` requires `{}`",
                        self.interner.resolve(need_param.name),
                        self.format_type_expr(&got_param.ty),
                        self.format_type_expr(&need_param.ty)
                    ),
                    format!("write `+{}`", self.trait_method_line(need)),
                );
            }
        }
        if let Some(need_ret) = &need.ret {
            let matches = method
                .ret
                .as_ref()
                .is_some_and(|got| self.type_expr_satisfies(need_ret, got));
            if !matches {
                let found = method
                    .ret
                    .as_ref()
                    .map(|ty| self.format_type_expr(ty))
                    .unwrap_or_else(|| "no return type".to_string());
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    method.span,
                    err::E0203,
                    format!(
                        "`{type_label}.{method_label}` returns `{found}`; `{trait_label}` requires `{}`",
                        self.format_type_expr(need_ret)
                    ),
                    format!("write `+{}`", self.trait_method_line(need)),
                );
            }
        }
    }

    fn trait_method_line(&self, method: &TraitMethod) -> String {
        let params: Vec<String> = method
            .params
            .iter()
            .map(|param| {
                let mut piece = String::new();
                if param.mutable {
                    piece.push('@');
                }
                let name = self.interner.resolve(param.name);
                piece.push_str(name);
                if name != "self" {
                    let ty = self.format_type_expr(&param.ty);
                    if ty != "_" {
                        piece.push_str(": ");
                        piece.push_str(&ty);
                    }
                }
                piece
            })
            .collect();
        let mut line = format!(
            "{}({})",
            self.interner.resolve(method.name),
            params.join(", ")
        );
        if let Some(ret) = &method.ret {
            line.push_str(" : ");
            line.push_str(&self.format_type_expr(ret));
        }
        line
    }

    fn format_type_expr(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Named(name, _) => self.interner.resolve(*name).to_string(),
            TypeExpr::Optional(inner, _) => format!("{}?", self.format_type_expr(inner)),
            TypeExpr::Error(inner, _) => format!("{}!", self.format_type_expr(inner)),
            TypeExpr::Path(path, _) => path
                .iter()
                .map(|name| self.interner.resolve(*name))
                .collect::<Vec<_>>()
                .join("."),
            TypeExpr::Chan(inner, _) => format!("chan[{}]", self.format_type_expr(inner)),
            TypeExpr::Type(_) => "type".to_string(),
            TypeExpr::Code(inner, _) => format!("Code[{}]", self.format_type_expr(inner)),
            TypeExpr::Apply { name, args, .. } => {
                let args = args
                    .iter()
                    .map(|arg| self.format_type_expr(arg))
                    .collect::<Vec<_>>();
                format!("{}[{}]", self.interner.resolve(*name), args.join(", "))
            }
        }
    }

    /// A trait type of `_` accepts whatever the method wrote. Every other form must match.
    fn type_expr_satisfies(&self, required: &TypeExpr, got: &TypeExpr) -> bool {
        if let TypeExpr::Named(name, _) = required
            && self.interner.resolve(*name) == "_"
        {
            return true;
        }
        self.type_exprs_equal(required, got)
    }

    fn type_exprs_equal(&self, left: &TypeExpr, right: &TypeExpr) -> bool {
        match (left, right) {
            (TypeExpr::Named(left, _), TypeExpr::Named(right, _)) => left == right,
            (TypeExpr::Optional(left, _), TypeExpr::Optional(right, _)) => {
                self.type_exprs_equal(left, right)
            }
            (TypeExpr::Error(left, _), TypeExpr::Error(right, _)) => {
                self.type_exprs_equal(left, right)
            }
            (TypeExpr::Path(left, _), TypeExpr::Path(right, _)) => left == right,
            (TypeExpr::Chan(left, _), TypeExpr::Chan(right, _)) => {
                self.type_exprs_equal(left, right)
            }
            (TypeExpr::Type(_), TypeExpr::Type(_)) => true,
            (TypeExpr::Code(left, _), TypeExpr::Code(right, _)) => {
                self.type_exprs_equal(left, right)
            }
            (
                TypeExpr::Apply {
                    name: left_name,
                    args: left_args,
                    ..
                },
                TypeExpr::Apply {
                    name: right_name,
                    args: right_args,
                    ..
                },
            ) => {
                left_name == right_name
                    && left_args.len() == right_args.len()
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.type_exprs_equal(left, right))
            }
            _ => false,
        }
    }

    fn alloc_struct_placeholder(&mut self, s: &StructDef) -> TypeId {
        self.typed.types.alloc_type(TypeKind::Struct {
            name: s.name,
            fields: vec![],
            size: 0,
            align: 8,
        })
    }

    fn alloc_sum_placeholder(&mut self, s: &SumDef) -> TypeId {
        self.typed.types.alloc_type(TypeKind::Sum {
            name: s.name,
            variants: vec![],
            size: 16,
        })
    }

    fn define_struct(&mut self, s: &StructDef) {
        let Some(&tid) = self.typed.structs.get(&s.name) else {
            return;
        };
        let mut fields = Vec::new();
        let mut offset = 0u32;
        let align = 8u32;
        for f in &s.fields {
            let ty = self.resolve_type_expr(&f.ty);
            offset = align_up(offset, align_of(&self.typed.types, ty));
            fields.push(FieldLayout {
                name: f.name,
                ty,
                offset,
                private: f.private,
                docs: f.docs,
            });
            offset += size_of_ty(&self.typed.types, ty);
            if let Some(default) = &f.default {
                let _ = self.check_expr(default, Some(ty));
            }
        }
        let size = align_up(offset, align);
        self.note_type_docs(s.name, s.docs);
        *self.typed.types.get_type_mut(tid) = TypeKind::Struct {
            name: s.name,
            fields,
            size,
            align,
        };
    }

    fn define_sum(&mut self, s: &SumDef) {
        let Some(&tid) = self.typed.sums.get(&s.name) else {
            return;
        };
        let mut variants = Vec::new();
        let mut max_payload = 0u32;
        for (i, v) in s.variants.iter().enumerate() {
            let mut fields = Vec::new();
            let mut payload = 0u32;
            for (n, te) in &v.fields {
                let ty = self.resolve_type_expr(te);
                fields.push((*n, ty));
                payload += size_of_ty(&self.typed.types, ty);
            }
            max_payload = max_payload.max(payload);
            variants.push(VariantLayout {
                name: v.name,
                tag: i as u32,
                fields,
                docs: v.docs,
            });
        }
        self.note_type_docs(s.name, s.docs);
        *self.typed.types.get_type_mut(tid) = TypeKind::Sum {
            name: s.name,
            variants,
            size: 4 + max_payload,
        };
    }

    /// Free functions key by short name; methods by `Type.method`.
    fn function_key(&mut self, f: &Function) -> SymId {
        match f.owner {
            Some(owner) => {
                let q = format!(
                    "{}.{}",
                    self.interner.resolve(owner),
                    self.interner.resolve(f.name)
                );
                self.interner.intern(&q)
            }
            None => f.name,
        }
    }

    fn bind_import_renames(&mut self, module: &Module) {
        let uses: Vec<UseItem> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Use(import) => Some(import.clone()),
                _ => None,
            })
            .collect();
        for import in uses {
            for (from, to) in import.renames {
                if let Some(tid) = self.typed.structs.get(&from).copied() {
                    self.typed.structs.insert(to, tid);
                    self.typed.aliases.insert(to, from);
                } else if let Some(tid) = self.typed.sums.get(&from).copied() {
                    self.typed.sums.insert(to, tid);
                    self.typed.aliases.insert(to, from);
                } else {
                    self.typed.aliases.insert(to, from);
                }
            }
        }
    }

    fn canonical(&self, name: SymId) -> SymId {
        self.typed.aliases.get(&name).copied().unwrap_or(name)
    }

    fn is_module_binding(&self, name: SymId) -> bool {
        self.typed.bindings.get(&name).is_some_and(|info| {
            matches!(self.typed.types.get_type(info.ty), TypeKind::Module { .. })
        })
    }

    /// `Type.method(...)` or `Alias.method(...)` where Alias names the same struct.
    fn named_type_method_ret(
        &mut self,
        owner_name: SymId,
        field: SymId,
        args: &[Expr],
        punning: bool,
        expected: Option<TypeId>,
    ) -> Option<TypeId> {
        if !self.typed.structs.contains_key(&owner_name)
            && !self.typed.sums.contains_key(&owner_name)
            && !self.typed.aliases.contains_key(&owner_name)
        {
            return None;
        }
        self.mark_used(owner_name);
        let owner = self.canonical(owner_name);
        let q = format!(
            "{}.{}",
            self.interner.resolve(owner),
            self.interner.resolve(field)
        );
        let key = self.interner.intern(&q);
        let info = self.typed.functions.get(&key).cloned()?;
        if punning {
            for (name, _, _) in info.params.iter().skip(args.len()) {
                self.mark_used(*name);
            }
        }
        for (i, arg) in args.iter().enumerate() {
            let expect_ty = info.params.get(i).map(|(_, ty, _)| *ty);
            let _ = self.check_expr(arg, expect_ty);
        }
        Some(match self.typed.types.get_type(info.ty) {
            TypeKind::Func { ret, .. } => *ret,
            _ => expected.unwrap_or_else(|| self.builtin("i64")),
        })
    }

    fn declare_function(&mut self, f: &Function) {
        let mut params = Vec::new();
        let mut param_tys = Vec::new();
        let mut trait_params = Vec::new();
        let mut param_ranks = Vec::new();
        for p in &f.params {
            let (ty, trait_bound, rank) = self.resolve_param_type(&p.ty);
            params.push((p.name, ty, p.mutable));
            param_tys.push(ty);
            trait_params.push(trait_bound);
            param_ranks.push(rank as u8);
        }
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| self.builtin("i64"));
        let fty = self.typed.types.alloc_type(TypeKind::Func {
            params: param_tys.clone(),
            ret,
        });
        let key = self.function_key(f);
        let info = FunctionInfo {
            ty: fty,
            exported: f.exported,
            params,
            trait_params,
            param_ranks: param_ranks.clone(),
            span: f.span,
            owner: f.owner,
            docs: f.docs,
        };
        // E0206: identical canonical parameter types + @ flags in the same family.
        // Cross-module imports often export the same host wrapper (`sha256_hex` in both
        // `std.sec.hash` and `std.sec.crypto`); keep the first binding and skip the
        // duplicate. Same-file / local redefinitions still error (`tracking_enabled`).
        if let Some(family) = self.typed.families.get(&key) {
            for prior in family {
                if Self::same_overload_signature(prior, &info) {
                    if !self.tracking_enabled {
                        return;
                    }
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Type,
                        f.span,
                        err::E0206,
                        format!(
                            "duplicate overload `{}` with identical parameter types",
                            self.interner.resolve(f.name)
                        ),
                        "change a parameter type, arity, trait bound, or `@` mutability",
                    );
                    // Still record secondary span on the prior definition.
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Type,
                        prior.span,
                        err::E0206,
                        format!(
                            "previous definition of `{}` here",
                            self.interner.resolve(f.name)
                        ),
                        "overloads must differ in arity, types, traits, or `@`",
                    );
                    return;
                }
            }
        }
        self.typed
            .families
            .entry(key)
            .or_default()
            .push(info.clone());
        // Primary slot: keep the first overload for hover / single-lookup paths.
        self.typed.functions.entry(key).or_insert(info);
    }

    fn same_overload_signature(a: &FunctionInfo, b: &FunctionInfo) -> bool {
        if a.params.len() != b.params.len() {
            return false;
        }
        a.params
            .iter()
            .zip(b.params.iter())
            .all(|((_, ta, ma), (_, tb, mb))| ta == tb && ma == mb)
            && a.trait_params == b.trait_params
    }

    /// Resolve a parameter type annotation, detecting trait constraints.
    fn resolve_param_type(&mut self, te: &TypeExpr) -> (TypeId, Option<SymId>, SpecRank) {
        match te {
            TypeExpr::Named(name, _) => {
                let s = self.interner.resolve(*name);
                if s == "_" {
                    let ty = self.typed.types.alloc_type(TypeKind::Infer);
                    return (ty, None, SpecRank::Unconstrained);
                }
                if self.typed.traits.contains_key(name) {
                    let ty = self
                        .typed
                        .types
                        .alloc_type(TypeKind::TraitBound { name: *name });
                    return (ty, Some(*name), SpecRank::Trait);
                }
                if let Some(ty) = self.lookup_named_type(*name) {
                    let rank = match self.typed.types.get_type(ty) {
                        TypeKind::Sum { .. } => SpecRank::Variant,
                        TypeKind::Infer => SpecRank::Unconstrained,
                        _ => SpecRank::Concrete,
                    };
                    return (ty, None, rank);
                }
                (self.builtin("i64"), None, SpecRank::Concrete)
            }
            _ => {
                let ty = self.resolve_type_expr(te);
                let rank = match self.typed.types.get_type(ty) {
                    TypeKind::Infer => SpecRank::Unconstrained,
                    TypeKind::TraitBound { .. } => SpecRank::Trait,
                    TypeKind::Sum { .. } => SpecRank::Variant,
                    _ => SpecRank::Concrete,
                };
                (ty, None, rank)
            }
        }
    }

    fn note_type_docs(&mut self, name: SymId, docs: SymId) {
        if !self.interner.resolve(docs).is_empty() {
            self.typed.type_docs.insert(name, docs);
        }
    }

    fn note_binding_docs(&mut self, name: SymId, docs: SymId) {
        if !self.interner.resolve(docs).is_empty() {
            self.typed.binding_docs.insert(name, docs);
        }
    }

    fn check_function_body(&mut self, f: &Function) {
        self.push_scope();
        let key = self.function_key(f);
        let body_info = self
            .typed
            .families
            .get(&key)
            .and_then(|fam| fam.iter().find(|i| i.span == f.span).cloned())
            .or_else(|| self.typed.functions.get(&key).cloned());
        if let Some(info) = body_info {
            for (name, ty, mutable) in info.params {
                self.scopes.last_mut().unwrap().insert(
                    name,
                    BindingInfo {
                        ty,
                        exported: false,
                        mutable,
                        span: f.span,
                    },
                );
                let span = f
                    .params
                    .iter()
                    .find(|p| p.name == name)
                    .map_or(f.span, |p| p.span);
                let comptime = f.params.iter().any(|p| p.name == name && p.comptime);
                self.record_symbol(
                    name,
                    ty,
                    mutable,
                    if comptime {
                        StorageClass::ConstExpr
                    } else {
                        StorageClass::Param
                    },
                    span,
                    if comptime {
                        SymbolKind::Const
                    } else {
                        SymbolKind::Param
                    },
                );
                self.track_definition(name, span, UsageKind::Parameter);
            }
            let ret = match self.typed.types.get_type(info.ty) {
                TypeKind::Func { ret, .. } => *ret,
                _ => self.builtin("i64"),
            };
            match &f.body {
                FnBody::Expr(e) => {
                    let _ = self.check_expr(e, Some(ret));
                    self.check_root_escape(e, e.span());
                }
                FnBody::Block(b) => self.check_block_tail(b, Some(ret)),
            }
        }
        self.pop_scope();
    }

    fn check_binding(&mut self, b: &Binding) -> TypeId {
        let expected = b.ty.as_ref().map(|t| self.resolve_type_expr(t));
        self.check_expr(&b.value, expected)
    }

    fn check_block(&mut self, block: &Block) {
        self.push_scope();
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.pop_scope();
    }

    /// Like `check_block`, but the last expression/`return` is checked against `expected_ret`.
    fn check_block_tail(&mut self, block: &Block, expected_ret: Option<TypeId>) {
        self.push_scope();
        match block.stmts.split_last() {
            None => {}
            Some((last, rest)) => {
                for stmt in rest {
                    self.check_stmt(stmt);
                }
                match last {
                    Stmt::Expr(e) => {
                        let _ = self.check_expr(e, expected_ret);
                        // Tail expression is an implicit return — same escape rules.
                        self.check_root_escape(e, e.span());
                    }
                    Stmt::Return(Some(e), span) => {
                        let _ = self.check_expr(e, expected_ret);
                        self.check_root_escape(e, *span);
                    }
                    Stmt::Return(None, _) => {}
                    Stmt::Scope { body, .. } => {
                        self.check_block_tail(body, expected_ret);
                    }
                    other => self.check_stmt(other),
                }
            }
        }
        self.pop_scope();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Binding(b) => {
                let ty = self.check_binding(b);
                // `name = value` updates a local already in this block. A module
                // import lives in an outer scope (`use std.cli.cmd`); `cmd = …`
                // still introduces a local that shadows the module.
                let in_this_scope = self.scopes.last().is_some_and(|s| s.contains_key(&b.name));
                let module = self.find_binding(b.name).is_some_and(|info| {
                    matches!(self.typed.types.get_type(info.ty), TypeKind::Module { .. })
                });
                if !in_this_scope && (self.find_binding(b.name).is_none() || module) {
                    self.record_symbol(
                        b.name,
                        ty,
                        b.mutable,
                        if b.comptime {
                            StorageClass::ConstExpr
                        } else {
                            self.storage_for_type(ty, false)
                        },
                        b.span,
                        if b.comptime {
                            SymbolKind::Const
                        } else {
                            SymbolKind::Local
                        },
                    );
                    self.scopes.last_mut().unwrap().insert(
                        b.name,
                        BindingInfo {
                            ty,
                            exported: b.exported,
                            mutable: b.mutable,
                            span: b.span,
                        },
                    );
                    self.track_definition(b.name, b.span, UsageKind::Local);
                }
            }
            Stmt::Destructure {
                fields,
                value,
                span,
            } => {
                let ty = self.check_expr(value, None);
                for (src, local) in fields {
                    let fty = self.field_type(ty, *src, *span);
                    self.record_symbol(
                        *local,
                        fty,
                        false,
                        self.storage_for_type(fty, false),
                        *span,
                        SymbolKind::Local,
                    );
                    self.scopes.last_mut().unwrap().insert(
                        *local,
                        BindingInfo {
                            ty: fty,
                            exported: false,
                            mutable: false,
                            span: *span,
                        },
                    );
                    self.track_definition(*local, *span, UsageKind::Local);
                }
            }
            Stmt::Expr(e) => {
                let _ = self.check_expr(e, None);
            }
            Stmt::Return(v, span) => {
                if let Some(e) = v {
                    let _ = self.check_expr(e, None);
                    self.check_root_escape(e, *span);
                }
            }
            Stmt::Ensure {
                cond, else_block, ..
            } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                self.check_block(else_block);
            }
            Stmt::While { cond, body, .. } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                self.check_block(body);
            }
            Stmt::Loop { body, .. } => {
                self.check_block(body);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::MultiAssign {
                targets, values, ..
            } => {
                // RHS first (all evaluated before any store).
                let mut rhs_tys = Vec::with_capacity(values.len());
                for v in values {
                    rhs_tys.push(self.check_expr(v, None));
                }
                // `a, b, c = 0, 0, 0` may *introduce* locals (like a binding) when
                // the target is a bare Ident not yet in scope.
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        Expr::Ident(name, span) if self.find_binding(*name).is_none() => {
                            let ty = rhs_tys
                                .get(i)
                                .copied()
                                .unwrap_or_else(|| self.builtin("i64"));
                            self.record_symbol(
                                *name,
                                ty,
                                true,
                                self.storage_for_type(ty, false),
                                *span,
                                SymbolKind::Local,
                            );
                            self.scopes.last_mut().unwrap().insert(
                                *name,
                                BindingInfo {
                                    ty,
                                    exported: false,
                                    mutable: true,
                                    span: *span,
                                },
                            );
                            self.track_definition(*name, *span, UsageKind::Local);
                        }
                        _ => {
                            let _ = self.check_assignment_target(t, rhs_tys.get(i).copied());
                        }
                    }
                }
            }
            Stmt::Guarded { stmt, cond, .. } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                self.check_stmt(stmt);
            }
            Stmt::Each {
                iter,
                body,
                bindings,
                span,
                ..
            } => {
                let iter_ty = self.check_expr(iter, None);
                self.push_scope();
                let i64t = self.builtin("i64");
                let elem_ty = match self.typed.types.get_type(iter_ty) {
                    TypeKind::Chan(e) | TypeKind::Array(e) => *e,
                    _ => i64t,
                };
                let it = self.interner.intern("it");
                let index = self.interner.intern("index");
                self.scopes.last_mut().unwrap().insert(
                    it,
                    BindingInfo {
                        ty: elem_ty,
                        exported: false,
                        mutable: true,
                        span: iter.span(),
                    },
                );
                self.scopes.last_mut().unwrap().insert(
                    index,
                    BindingInfo {
                        ty: i64t,
                        exported: false,
                        mutable: false,
                        span: iter.span(),
                    },
                );
                for b in bindings {
                    let ty =
                        b.ty.and_then(|n| self.lookup_named_type(n))
                            .unwrap_or(elem_ty);
                    self.record_symbol(
                        b.name,
                        ty,
                        b.mutable,
                        self.storage_for_type(ty, false),
                        *span,
                        SymbolKind::LoopBinder,
                    );
                    self.scopes.last_mut().unwrap().insert(
                        b.name,
                        BindingInfo {
                            ty,
                            exported: false,
                            mutable: b.mutable,
                            span: *span,
                        },
                    );
                    self.track_definition(b.name, iter.span(), UsageKind::Local);
                }
                self.check_block(body);
                self.pop_scope();
            }
            Stmt::When {
                subjects,
                arms,
                span,
            } => {
                if subjects.len() >= 2 {
                    self.check_multi_when(subjects, arms, *span);
                    return;
                }
                let subject = subjects.first();
                let subj_ty = subject.map(|subj| self.check_expr(subj, None));
                let sum_variants = subj_ty.and_then(|tid| match self.typed.types.get_type(tid) {
                    TypeKind::Sum { variants, .. } => Some(variants.clone()),
                    _ => None,
                });
                let mut covered: HashSet<SymId> = HashSet::new();
                let mut has_else = false;
                for arm in arms {
                    match &arm.pattern {
                        WhenPattern::Expr(e) => {
                            if let Some(variants) = &sum_variants {
                                // Value-match arm: `Status.Paid` / `Status.Paid(x)`.
                                let _ = self.check_expr(e, subj_ty);
                                if let Expr::NamedVariant { variant, .. } = e {
                                    covered.insert(*variant);
                                    if !variants.iter().any(|v| v.name == *variant) {
                                        // check_named_variant already diagnosed unknown variants.
                                    }
                                }
                            } else if subject.is_some() {
                                let _ = self.check_expr(e, Some(self.builtin("bool")));
                            } else {
                                // Multiplex arm: binding or recv expression.
                                let _ = self.check_expr(e, None);
                            }
                            let _ = self.check_expr(&arm.body, None);
                        }
                        WhenPattern::After(d) => {
                            let _ = self.check_expr(d, Some(self.builtin("duration")));
                            let _ = self.check_expr(&arm.body, None);
                        }
                        WhenPattern::Else => {
                            has_else = true;
                            let _ = self.check_expr(&arm.body, None);
                        }
                        WhenPattern::Tuple { span: pat_span, .. } => {
                            self.diagnostics.error_code_help(
                                DiagnosticKind::Type,
                                *pat_span,
                                err::E0201,
                                "tuple when-arm requires `when (e1, e2, …)` with matching arity",
                                "write `when (a, b) { (P1, P2) => … }`",
                            );
                            let _ = self.check_expr(&arm.body, None);
                        }
                        WhenPattern::VariantBind {
                            sum,
                            variant,
                            fields,
                            span: pat_span,
                        } => {
                            covered.insert(*variant);
                            self.push_scope();
                            if let Some(variants) = &sum_variants {
                                let Some(vlayout) = variants.iter().find(|v| v.name == *variant)
                                else {
                                    self.diagnostics.error_code_help(
                                        DiagnosticKind::Type,
                                        *pat_span,
                                        err::E0202,
                                        format!(
                                            "unknown variant `{}` on sum `{}`",
                                            self.interner.resolve(*variant),
                                            self.interner.resolve(*sum)
                                        ),
                                        "hover the sum type to see its variants",
                                    );
                                    let _ = self.check_expr(&arm.body, None);
                                    self.pop_scope();
                                    continue;
                                };
                                for (payload, local) in fields {
                                    let Some((_, fty)) =
                                        vlayout.fields.iter().find(|(n, _)| n == payload)
                                    else {
                                        self.diagnostics.error_code_help(
                                            DiagnosticKind::Type,
                                            *pat_span,
                                            err::E0202,
                                            format!(
                                                "variant `{}` has no payload field `{}`",
                                                self.interner.resolve(*variant),
                                                self.interner.resolve(*payload)
                                            ),
                                            "use a field declared on that variant",
                                        );
                                        continue;
                                    };
                                    self.record_symbol(
                                        *local,
                                        *fty,
                                        false,
                                        StorageClass::StackLocal,
                                        *pat_span,
                                        SymbolKind::Local,
                                    );
                                    self.scopes.last_mut().unwrap().insert(
                                        *local,
                                        BindingInfo {
                                            ty: *fty,
                                            exported: false,
                                            mutable: false,
                                            span: *pat_span,
                                        },
                                    );
                                    self.track_definition(*local, *pat_span, UsageKind::Local);
                                }
                            } else {
                                self.diagnostics.error_code_help(
                                    DiagnosticKind::Type,
                                    *pat_span,
                                    err::E0202,
                                    "payload destructuring requires a sum-typed `when` subject",
                                    "match on a sum value, e.g. `when status { Status.Paid { amount } => … }`",
                                );
                            }
                            let _ = self.check_expr(&arm.body, None);
                            self.pop_scope();
                        }
                    }
                }
                if let Some(variants) = &sum_variants
                    && !has_else
                    && !variants.is_empty()
                {
                    let missing: Vec<_> = variants
                        .iter()
                        .filter(|v| !covered.contains(&v.name))
                        .map(|v| self.interner.resolve(v.name).to_owned())
                        .collect();
                    if !missing.is_empty() {
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            *span,
                            err::E0202,
                            format!(
                                "non-exhaustive `when` on sum type; missing {}",
                                missing
                                    .iter()
                                    .map(|m| format!("`{m}`"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            "cover every variant, or add an `else => …` arm",
                        );
                    }
                }
            }
            Stmt::Scope { body, .. } => {
                self.scope_depth += 1;
                self.scope_mut_captures.push(HashSet::new());
                self.check_block(body);
                self.scope_mut_captures.pop();
                self.scope_depth -= 1;
            }
            Stmt::Fail(e, _) => {
                let _ = self.check_expr(e, Some(self.builtin("str")));
            }
            Stmt::Panic(msg, _) => {
                if let Some(e) = msg {
                    let _ = self.check_expr(e, Some(self.builtin("str")));
                }
            }
            Stmt::Comptime(cs) => match cs {
                ComptimeStmt::Block(b, _) => self.check_block(b),
                ComptimeStmt::If {
                    cond,
                    then_block,
                    else_block,
                    ..
                } => {
                    let _ = self.check_expr(cond, Some(self.builtin("bool")));
                    self.check_block(then_block);
                    if let Some(eb) = else_block {
                        self.check_block(eb);
                    }
                }
                ComptimeStmt::Each {
                    name,
                    iter,
                    body,
                    span,
                } => {
                    let _ = self.check_expr(iter, None);
                    let elem_ty = self.builtin("i64");
                    self.push_scope();
                    self.scopes.last_mut().unwrap().insert(
                        *name,
                        BindingInfo {
                            ty: elem_ty,
                            exported: false,
                            mutable: false,
                            span: *span,
                        },
                    );
                    self.record_symbol(
                        *name,
                        elem_ty,
                        false,
                        StorageClass::ConstExpr,
                        *span,
                        SymbolKind::LoopBinder,
                    );
                    self.track_definition(*name, *span, UsageKind::Local);
                    self.check_block(body);
                    self.pop_scope();
                }
            },
        }
    }

    fn check_expr(&mut self, expr: &Expr, expected: Option<TypeId>) -> TypeId {
        let ty = match expr {
            Expr::Int(_, _) => self.builtin("i64"),
            Expr::Float(_, _) => self.builtin("f64"),
            Expr::Bool(_, _) => self.builtin("bool"),
            Expr::String(value, span) => {
                self.check_string_interpolations(*value, *span);
                self.builtin("str")
            }
            Expr::None(_) => {
                if let Some(exp) = expected {
                    exp
                } else {
                    let inner = self.builtin("i64");
                    self.typed.types.alloc_type(TypeKind::Optional(inner))
                }
            }
            Expr::Ident(name, span) => self.lookup_binding(*name, *span),
            Expr::It(span) | Expr::IndexIdent(span) | Expr::ErrIdent(span) => {
                let name = match expr {
                    Expr::It(_) => "it",
                    Expr::IndexIdent(_) => "index",
                    _ => "err",
                };
                let sym = self.interner.intern(name);
                self.lookup_binding(sym, *span)
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let lt = self.check_expr(left, None);
                // Comparisons stay soft on both sides so loosely-typed `each`
                // element `it` (often i64) can still be compared to `str` keys.
                // Arithmetic nudges the RHS toward the LHS for numeric widen.
                let rt = match op {
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => self.check_expr(right, None),
                    _ => self.check_expr(right, Some(lt)),
                };
                let _ = rt;
                match op {
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => self.builtin("bool"),
                    _ => lt,
                }
            }
            Expr::Unary {
                op, expr: inner, ..
            } => match op {
                UnOp::Not => {
                    let _ = self.check_expr(inner, None);
                    self.builtin("bool")
                }
                UnOp::Neg | UnOp::BitNot => self.check_expr(inner, None),
                UnOp::MutRef => {
                    self.under_mut_ref = true;
                    let t = self.check_expr(inner, None);
                    self.under_mut_ref = false;
                    t
                }
            },
            Expr::Call {
                callee,
                args,
                punning,
                span,
            } => {
                let free_len = matches!(
                    callee.as_ref(),
                    Expr::Ident(name, _) if self.interner.resolve(*name) == "len"
                        && !self.typed.functions.contains_key(name)
                );
                if free_len && args.len() == 1 {
                    let _ = self.check_expr(&args[0], None);
                    return self.builtin("i64");
                }
                if let Expr::Field { base, field, .. } = callee.as_ref()
                    && self.interner.resolve(*field) == "len"
                    && args.is_empty()
                {
                    let receiver = self.check_expr(base, None);
                    if !self.receiver_defines_method(receiver, *field) {
                        return self.builtin("i64");
                    }
                }
                if let Expr::Field { field, .. } = callee.as_ref()
                    && self.interner.resolve(*field) == "sys_arr_len"
                    && args.len() == 1
                {
                    let _ = self.check_expr(&args[0], None);
                    return self.builtin("i64");
                }
                // `Type.method(...)` — static or explicit UFCS on a known struct type.
                let mut type_method_ret: Option<TypeId> = None;
                let callee_span = *span;
                if let Expr::Field { base, field, .. } = callee.as_ref() {
                    type_method_ret = if let Expr::Ident(ns, _) = base.as_ref() {
                        self.named_type_method_ret(*ns, *field, args, *punning, expected)
                    } else if let Expr::Field {
                        base: mid,
                        field: type_name,
                        ..
                    } = base.as_ref()
                    {
                        if let Expr::Ident(ns, _) = mid.as_ref() {
                            if self.is_module_binding(*ns) {
                                self.mark_used(*ns);
                                self.named_type_method_ret(
                                    *type_name, *field, args, *punning, expected,
                                )
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                }
                if let Some(ret) = type_method_ret {
                    if self.expr_names_send(callee) {
                        for a in args {
                            self.check_root_escape(a, a.span());
                        }
                    }
                    ret
                } else {
                    let ct = self.check_expr(callee, None);
                    // Soft-type args first so FuncFamily ranking sees concrete TypeIds.
                    let arg_tys: Vec<TypeId> =
                        args.iter().map(|a| self.check_expr(a, None)).collect();
                    if let Some(info) = self.resolve_callable_ranked(callee, &arg_tys, None) {
                        if *punning {
                            for (name, _, _) in info.params.iter().skip(args.len()) {
                                self.mark_used(*name);
                            }
                        }
                        for (i, a) in args.iter().enumerate() {
                            if let Some((_, et, _)) = info.params.get(i) {
                                // Trait-bound params accept any conforming impl.
                                if info.trait_params.get(i).copied().flatten().is_none() {
                                    if matches!(a, Expr::Lambda { .. }) {
                                        let registration = match callee.as_ref() {
                                            Expr::Field { field, .. } => {
                                                self.interner.resolve(*field).to_string()
                                            }
                                            _ => String::new(),
                                        };
                                        self.check_route_handler_arg(&registration, args, a, *et);
                                        let _ = self.check_expr(a, Some(*et));
                                    } else {
                                        self.check_call_arg_literal(a, *et);
                                    }
                                }
                            }
                        }
                        if !self.is_static_type_method(callee) {
                            let receiver = if let Expr::Field { base, .. } = callee.as_ref() {
                                Some(base.as_ref())
                            } else {
                                None
                            };
                            self.check_inout_call(&info, receiver, args, callee_span);
                        }
                        if self.expr_names_send(callee) {
                            for a in args {
                                self.check_root_escape(a, a.span());
                            }
                        }
                        match self.typed.types.get_type(info.ty) {
                            TypeKind::Func { ret, .. } => *ret,
                            _ => expected.unwrap_or_else(|| self.builtin("i64")),
                        }
                    } else if let TypeKind::Func { params, ret, .. } =
                        self.typed.types.get_type(ct).clone()
                    {
                        for (a, pty) in args.iter().zip(params.iter()) {
                            let _ = self.check_expr(a, Some(*pty));
                        }
                        if self.expr_names_send(callee) {
                            for a in args {
                                self.check_root_escape(a, a.span());
                            }
                        }
                        ret
                    } else {
                        if self.expr_names_send(callee) {
                            for a in args {
                                self.check_root_escape(a, a.span());
                            }
                        }
                        expected.unwrap_or_else(|| self.builtin("i64"))
                    }
                }
            }
            Expr::Slice {
                base, start, end, ..
            } => {
                let bt = self.check_expr(base, None);
                let _ = self.check_expr(start, Some(self.builtin("i64")));
                let _ = self.check_expr(end, Some(self.builtin("i64")));
                match self.typed.types.get_type(bt) {
                    TypeKind::Str => self.builtin("str"),
                    TypeKind::Array(e) => self.typed.types.alloc_type(TypeKind::Array(*e)),
                    _ => self.builtin("str"),
                }
            }
            Expr::Field { base, field, span } => {
                let bt = self.check_expr(base, None);
                self.field_type(bt, *field, *span)
            }
            Expr::Pipeline {
                base,
                method,
                args,
                span,
            } => {
                if self.interner.resolve(*method) == "sys_arr_len" && args.len() == 1 {
                    let _ = self.check_expr(&args[0], None);
                    return self.builtin("i64");
                }
                let bt = self.check_expr(base, None);
                if self.interner.resolve(*method) == "len"
                    && args.is_empty()
                    && !self.receiver_defines_method(bt, *method)
                {
                    return self.builtin("i64");
                }
                // Implicit `it` inside pipeline closures / predicates.
                self.push_scope();
                let it = self.interner.intern("it");
                let index = self.interner.intern("index");
                let i64t = self.builtin("i64");
                let pipe_span = *span;
                self.scopes.last_mut().unwrap().insert(
                    it,
                    BindingInfo {
                        ty: i64t,
                        exported: false,
                        mutable: false,
                        span: pipe_span,
                    },
                );
                self.scopes.last_mut().unwrap().insert(
                    index,
                    BindingInfo {
                        ty: i64t,
                        exported: false,
                        mutable: false,
                        span: pipe_span,
                    },
                );
                let route_handler_index = match self.interner.resolve(*method) {
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "any" => {
                        Some(1)
                    }
                    "mount" => Some(2),
                    _ => None,
                };
                for (index, arg) in args.iter().enumerate() {
                    if route_handler_index == Some(index) && matches!(arg, Expr::Lambda { .. }) {
                        continue;
                    }
                    let _ = self.check_expr(arg, None);
                }
                if self.interner.resolve(*method) == "send" {
                    for a in args {
                        self.check_root_escape(a, a.span());
                    }
                }
                self.pop_scope();
                self.check_known_route_pipeline(*method, args);
                // Type method on a known struct/sum receiver: `a.advance(dt)` / `s.is_ok()`.
                let method_owner = match self.typed.types.get_type(bt).clone() {
                    TypeKind::Struct { name: owner, .. } | TypeKind::Sum { name: owner, .. } => {
                        Some(owner)
                    }
                    _ => None,
                };
                let method_ret = method_owner.and_then(|owner| {
                    let q = format!(
                        "{}.{}",
                        self.interner.resolve(owner),
                        self.interner.resolve(*method)
                    );
                    let key = self.interner.intern(&q);
                    self.typed.functions.get(&key).and_then(|info| {
                        match self.typed.types.get_type(info.ty) {
                            TypeKind::Func { ret, .. } => Some(*ret),
                            _ => None,
                        }
                    })
                });
                if let Some(ret) = method_ret {
                    if let Some(owner) = method_owner {
                        let q = format!(
                            "{}.{}",
                            self.interner.resolve(owner),
                            self.interner.resolve(*method)
                        );
                        let key = self.interner.intern(&q);
                        if let Some(info) = self.typed.functions.get(&key).cloned() {
                            let parameter_offset =
                                usize::from(!self.is_type_name_receiver(base.as_ref()));
                            for (index, arg) in args.iter().enumerate() {
                                let Some((_, expected_ty, _)) =
                                    info.params.get(index + parameter_offset)
                                else {
                                    continue;
                                };
                                if matches!(arg, Expr::Lambda { .. }) {
                                    let registration = self.interner.resolve(*method);
                                    if !matches!(
                                        registration,
                                        "get"
                                            | "post"
                                            | "put"
                                            | "patch"
                                            | "delete"
                                            | "head"
                                            | "options"
                                            | "any"
                                            | "mount"
                                    ) {
                                        let _ = self.check_expr(arg, Some(*expected_ty));
                                    }
                                } else {
                                    self.check_call_arg_literal(arg, *expected_ty);
                                }
                            }
                            if !self.under_mut_ref && !self.is_type_name_receiver(base.as_ref()) {
                                self.check_inout_call(&info, Some(base.as_ref()), args, *span);
                            }
                        }
                    }
                    ret
                } else {
                    // Universal UFCS: `recv.f(a…)` ≡ `f(recv, a…)` for a free function
                    // `f` of arity `1 + args`. The call's type is `f`'s declared return.
                    let name = self.interner.resolve(*method);
                    let ufcs_ret = if is_pipeline_builtin(name) {
                        None
                    } else {
                        self.typed.functions.get(method).and_then(|info| {
                            if info.owner.is_some() || info.params.len() != args.len() + 1 {
                                return None;
                            }
                            match self.typed.types.get_type(info.ty) {
                                TypeKind::Func { ret, .. }
                                    if !matches!(
                                        self.typed.types.get_type(*ret),
                                        TypeKind::Infer
                                    ) =>
                                {
                                    Some(*ret)
                                }
                                _ => None,
                            }
                        })
                    };
                    // Built-in collection methods return inferred collections
                    match name {
                        _ if ufcs_ret.is_some() => ufcs_ret.unwrap_or_else(|| self.builtin("i64")),
                        "map" | "keep" | "group_by" => {
                            expected.unwrap_or_else(|| self.builtin("i64"))
                        }
                        "sum" | "round" => expected.unwrap_or_else(|| self.builtin("f64")),
                        _ => {
                            let _ = span;
                            expected.unwrap_or_else(|| self.builtin("i64"))
                        }
                    }
                }
            }
            Expr::Index { base, index, .. } => {
                let bt = self.check_expr(base, None);
                let _ = self.check_expr(index, Some(self.builtin("i64")));
                match self.typed.types.get_type(bt) {
                    TypeKind::Str => self.builtin("str"),
                    TypeKind::Array(e) => *e,
                    TypeKind::Map { value, .. } => *value,
                    _ => expected.unwrap_or_else(|| self.builtin("i64")),
                }
            }
            Expr::Assign {
                op, target, value, ..
            } => {
                let tt = if *op == AssignOp::Assign {
                    self.check_assignment_target(target, None)
                } else {
                    self.check_expr(target, None)
                };
                let _ = self.check_expr(value, Some(tt));
                tt
            }
            Expr::Array(elems, _) => {
                let mut elem_ty = expected
                    .and_then(|t| match self.typed.types.get_type(t) {
                        TypeKind::Array(e) => Some(*e),
                        _ => None,
                    })
                    .unwrap_or_else(|| self.builtin("i64"));
                for (i, e) in elems.iter().enumerate() {
                    let t = self.check_expr(e, if i == 0 { None } else { Some(elem_ty) });
                    if i == 0 {
                        elem_ty = t;
                    }
                }
                self.typed.types.alloc_type(TypeKind::Array(elem_ty))
            }
            Expr::ArrayRepeat { value, len, .. } => {
                let elem_ty = self.check_expr(
                    value,
                    expected.and_then(|t| match self.typed.types.get_type(t) {
                        TypeKind::Array(e) => Some(*e),
                        _ => None,
                    }),
                );
                let _ = self.check_expr(len, Some(self.builtin("i64")));
                self.typed.types.alloc_type(TypeKind::Array(elem_ty))
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                let tt = self.check_expr(then_expr, expected);
                let _ = self.check_expr(else_expr, Some(tt));
                tt
            }
            Expr::Map(entries, _) => {
                for e in entries {
                    if let MapKey::Expr(k) = &e.key {
                        let _ = self.check_expr(k, None);
                    }
                    let _ = self.check_expr(&e.value, None);
                }
                let k = self.builtin("str");
                let v = self.builtin("i64");
                self.typed
                    .types
                    .alloc_type(TypeKind::Map { key: k, value: v })
            }
            Expr::Record(fields, _) | Expr::InferredStruct { fields, .. } => {
                for f in fields {
                    let _ = self.check_expr(&f.value, None);
                    // Storing a stack borrow into a longer-lived record escapes.
                    self.check_root_escape(&f.value, f.span);
                }
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::StructLit { name, fields, span } => {
                if let Some(name) = name {
                    self.mark_used(*name);
                }
                let tid = name.and_then(|n| self.typed.structs.get(&n).copied());
                if let Some(tid) = tid {
                    if let TypeKind::Struct { fields: layout, .. } =
                        self.typed.types.get_type(tid).clone()
                    {
                        for f in fields {
                            if let Some(fname) = f.name {
                                if let Some(fl) = layout.iter().find(|x| x.name == fname) {
                                    if fl.private && !self.in_test {
                                        let struct_name = name
                                            .map(|n| self.interner.resolve(n).to_string())
                                            .unwrap_or_else(|| "struct".into());
                                        let fname_s = self.interner.resolve(fname);
                                        self.diagnostics.error_code_help(
                                            DiagnosticKind::Type,
                                            f.span,
                                            err::E0104,
                                            format!(
                                                "field '{fname_s}' on '{struct_name}' is file-private (marked with '-')"
                                            ),
                                            format!(
                                                "remove `-{fname_s}` from the literal or access it only from this module / `test` blocks"
                                            ),
                                        );
                                    }
                                    self.check_call_arg_literal(&f.value, fl.ty);
                                    let _ = self.check_expr(&f.value, Some(fl.ty));
                                    self.check_root_escape(&f.value, f.span);
                                }
                            } else {
                                let _ = self.check_expr(&f.value, None);
                                self.check_root_escape(&f.value, f.span);
                            }
                        }
                    }
                    tid
                } else {
                    for f in fields {
                        let _ = self.check_expr(&f.value, None);
                    }
                    let _ = span;
                    expected.unwrap_or_else(|| self.builtin("i64"))
                }
            }
            Expr::Range { start, end, .. } => {
                let _ = self.check_expr(start, Some(self.builtin("i64")));
                let _ = self.check_expr(end, Some(self.builtin("i64")));
                self.typed
                    .types
                    .alloc_type(TypeKind::Array(self.builtin("i64")))
            }
            Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                self.check_block(then_block);
                if let Some(e) = else_block {
                    self.check_block(e);
                }
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::Block(b) => {
                self.check_block(b);
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::Or { left, right, .. } => {
                // Neither side of `or` / chained `a or b or c` should be checked
                // against the unwrapped expected type up-front — both may be T!.
                let lt = self.check_expr(left, None);
                let rt = self.check_expr(right, None);
                let unwrap = |tid: TypeId, sema: &Self| match sema.typed.types.get_type(tid) {
                    TypeKind::Optional(inner) | TypeKind::Error(inner) => *inner,
                    _ => tid,
                };
                let lu = unwrap(lt, self);
                let _ru = unwrap(rt, self);
                let _ = (lu, _ru);
                lu
            }
            Expr::OrErr { left, body, .. } => {
                let lt = self.check_expr(left, None);
                self.push_scope();
                let err = self.interner.intern("err");
                let err_ty = self.builtin("str");
                let err_span = left.span();
                self.scopes.last_mut().unwrap().insert(
                    err,
                    BindingInfo {
                        ty: err_ty,
                        exported: false,
                        mutable: false,
                        span: err_span,
                    },
                );
                self.record_symbol(
                    err,
                    err_ty,
                    false,
                    StorageClass::Register,
                    err_span,
                    SymbolKind::Local,
                );
                self.check_block(body);
                self.pop_scope();
                match self.typed.types.get_type(lt) {
                    TypeKind::Optional(inner) | TypeKind::Error(inner) => *inner,
                    _ => lt,
                }
            }
            Expr::Await(inner, _) => {
                let t = self.check_expr(inner, None);
                match self.typed.types.get_type(t) {
                    TypeKind::Task(inner) => *inner,
                    TypeKind::Optional(inner) | TypeKind::Error(inner) => *inner,
                    _ => t,
                }
            }
            Expr::Spawn { body, span } => {
                let mut_caps = self.collect_mut_captures(body);
                if let Some(set) = self.scope_mut_captures.last_mut() {
                    for name in &mut_caps {
                        if set.contains(name) {
                            let n = self.interner.resolve(*name).to_owned();
                            self.diagnostics.error_code_help(
                                DiagnosticKind::Mutation,
                                *span,
                                err::E0302,
                                format!(
                                    "cannot capture `@{n}` mutably in multiple spawned tasks in the same scope"
                                ),
                                format!(
                                    "wrap `@{n}` with `shared(...)` for cross-task mutation, or pass it immutably"
                                ),
                            );
                        } else {
                            set.insert(*name);
                        }
                    }
                }
                let ret = self.check_expr(body, None);
                // Spawn must not capture a stack borrow from the parent frame.
                self.check_spawn_escape(body, *span);
                self.typed.types.alloc_type(TypeKind::Task(ret))
            }
            Expr::Chan {
                elem_ty, capacity, ..
            } => {
                let _ = self.check_expr(capacity, Some(self.builtin("i64")));
                let et = self.resolve_type_expr(elem_ty);
                self.typed.types.alloc_type(TypeKind::Chan(et))
            }
            Expr::Shared { value, .. } => {
                let vt = self.check_expr(value, None);
                self.typed.types.alloc_type(TypeKind::Shared(vt))
            }
            Expr::Duration { value, .. } => {
                let _ = self.check_expr(value, Some(self.builtin("i64")));
                self.builtin("duration")
            }
            Expr::Embed { .. } => {
                // Embedded files are `str` when UTF-8, otherwise opaque bytes → use str for now.
                expected.unwrap_or_else(|| self.builtin("str"))
            }
            Expr::NamedVariant {
                sum,
                variant,
                args,
                span,
            } => self.check_named_variant(*sum, *variant, args, *span, expected),
            Expr::Quote { body, .. } => {
                let inner = self.check_expr(body, expected);
                self.typed.types.alloc_type(TypeKind::Code(inner))
            }
            Expr::Splice { code, .. } => {
                let ct = self.check_expr(code, None);
                match self.typed.types.get_type(ct) {
                    TypeKind::Code(inner) => *inner,
                    _ => expected.unwrap_or(ct),
                }
            }
            Expr::ComptimeRef { name, span } => self.lookup_binding(*name, *span),
            Expr::ComptimeBlock { body, .. } => {
                self.check_block(body);
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::ComptimeIf {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let _ = self.check_expr(cond, Some(self.builtin("bool")));
                self.check_block(then_block);
                if let Some(eb) = else_block {
                    self.check_block(eb);
                }
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::ComptimeEach {
                name,
                iter,
                body,
                span,
            } => {
                let _ = self.check_expr(iter, None);
                let elem_ty = self.builtin("i64");
                self.push_scope();
                self.scopes.last_mut().unwrap().insert(
                    *name,
                    BindingInfo {
                        ty: elem_ty,
                        exported: false,
                        mutable: false,
                        span: *span,
                    },
                );
                self.record_symbol(
                    *name,
                    elem_ty,
                    false,
                    StorageClass::ConstExpr,
                    *span,
                    SymbolKind::LoopBinder,
                );
                self.track_definition(*name, *span, UsageKind::Local);
                self.check_block(body);
                self.pop_scope();
                expected.unwrap_or_else(|| {
                    self.typed
                        .types
                        .alloc_type(TypeKind::Array(self.builtin("i64")))
                })
            }
            Expr::GoalType(_) => self.typed.types.alloc_type(TypeKind::TypeMeta),
            Expr::ComptimeField { base, field, .. } => {
                let _ = self.check_expr(base, None);
                let _ = self.check_expr(field, Some(self.builtin("str")));
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::Lambda {
                params,
                ret,
                body,
                captures: _,
                span,
            } => {
                let expected_sig = expected.and_then(|ty| match self.typed.types.get_type(ty) {
                    TypeKind::Func { params, ret } => Some((params.clone(), *ret)),
                    _ => None,
                });
                if let Some((expected_params, _)) = &expected_sig
                    && expected_params.len() != params.len()
                {
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Type,
                        *span,
                        err::E0202,
                        format!(
                            "expected handler with {} parameter(s), found {}",
                            expected_params.len(),
                            params.len()
                        ),
                        "route handlers take exactly one request parameter",
                    );
                }
                self.push_scope();
                let mut param_tys = Vec::new();
                for (index, p) in params.iter().enumerate() {
                    let inferred = matches!(
                        &p.ty,
                        TypeExpr::Named(name, _) if self.interner.resolve(*name) == "_"
                    );
                    let ty = if inferred {
                        expected_sig
                            .as_ref()
                            .and_then(|(expected_params, _)| expected_params.get(index).copied())
                            .unwrap_or_else(|| self.resolve_type_expr(&p.ty))
                    } else {
                        let ty = self.resolve_type_expr(&p.ty);
                        if let Some(expected_ty) = expected_sig
                            .as_ref()
                            .and_then(|(expected_params, _)| expected_params.get(index))
                        {
                            let _ = self.unify_soft(ty, *expected_ty, p.span);
                        }
                        ty
                    };
                    param_tys.push(ty);
                    self.scopes.last_mut().unwrap().insert(
                        p.name,
                        BindingInfo {
                            ty,
                            exported: false,
                            mutable: p.mutable,
                            span: p.span,
                        },
                    );
                }
                let declared_ret = ret
                    .as_ref()
                    .map(|ty| self.resolve_type_expr(ty))
                    .or_else(|| expected_sig.as_ref().map(|(_, ret)| *ret));
                let ret_ty = match body.as_ref() {
                    FnBody::Expr(expr) => self.check_expr(expr, declared_ret),
                    FnBody::Block(block) => {
                        self.check_block_tail(block, declared_ret);
                        declared_ret.unwrap_or_else(|| self.builtin("i64"))
                    }
                };
                let ret_ty = declared_ret.unwrap_or(ret_ty);
                self.pop_scope();
                self.typed.types.alloc_type(TypeKind::Func {
                    params: param_tys,
                    ret: ret_ty,
                })
            }
            Expr::Panic { message, .. } => {
                if let Some(m) = message {
                    let _ = self.check_expr(m, Some(self.builtin("str")));
                }
                // Diverges — accept whatever the site expects.
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
            Expr::MacroCall(call) => {
                let name = self.interner.resolve(call.name).to_string();
                self.diagnostics.error(
                    DiagnosticKind::Syntax,
                    call.span,
                    format!("macro `#{name}` was not expanded before type checking"),
                );
                expected.unwrap_or_else(|| self.builtin("i64"))
            }
        };

        // Soft expected-type nudge: only hard-fail on clear optional/error unwrap
        // misses and concrete scalar mismatches. Collection / Infer shapes stay
        // advisory so pipelines and `#` expansions do not flood diagnostics.
        if let Some(exp) = expected {
            let _ = self.unify_soft(ty, exp, expr.span());
        }
        self.typed.expr_types.insert(span_key(expr.span()), ty);
        ty
    }

    fn check_route_handler_arg(
        &mut self,
        registration: &str,
        args: &[Expr],
        handler: &Expr,
        expected: TypeId,
    ) {
        let path_index = match registration {
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "any" => 0,
            "mount" => 1,
            _ => return,
        };
        let path = args
            .get(path_index)
            .and_then(|arg| match arg {
                Expr::String(value, _) => Some(self.interner.resolve(*value)),
                _ => None,
            })
            .unwrap_or("<dynamic path>");
        let TypeKind::Func {
            params: expected_params,
            ret: expected_ret,
        } = self.typed.types.get_type(expected).clone()
        else {
            return;
        };
        let Expr::Lambda {
            params, body, span, ..
        } = handler
        else {
            return;
        };
        if params.len() != expected_params.len() {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                *span,
                err::E0202,
                format!(
                    "handler for route \"{path}\" (App.{registration}) must accept {} request parameter(s), found {}",
                    expected_params.len(),
                    params.len()
                ),
                "use `|req| { ... }` and return an `http.Response`",
            );
            return;
        }
        let tail = match body.as_ref() {
            FnBody::Expr(expr) => Some(expr),
            FnBody::Block(block) => block.stmts.last().and_then(|stmt| match stmt {
                Stmt::Expr(expr) => Some(expr),
                _ => None,
            }),
        };
        let Some(tail) = tail else {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                *span,
                err::E0202,
                format!(
                    "handler for route \"{path}\" (App.{registration}) must return `{}`, found `void`",
                    self.type_name(expected_ret)
                ),
                "return `http.ok(...)`, `http.json(...)`, or another `http.Response` constructor",
            );
            return;
        };
        let Some(&actual_ret) = self.typed.expr_types.get(&span_key(tail.span())) else {
            return;
        };
        if actual_ret == expected_ret
            || matches!(self.typed.types.get_type(actual_ret), TypeKind::Never)
        {
            return;
        }
        let expected_name = self.type_name(expected_ret);
        let actual_name = self.type_name(actual_ret);
        self.diagnostics.error_code_help(
            DiagnosticKind::Type,
            tail.span(),
            err::E0202,
            format!(
                "handler for route \"{path}\" (App.{registration}) must return `{expected_name}`, found `{actual_name}`"
            ),
            "return `http.ok(...)`, `http.json(...)`, or another `http.Response` constructor",
        );
    }

    fn check_known_route_pipeline(&mut self, method: SymId, args: &[Expr]) {
        let registration = self.interner.resolve(method).to_string();
        let handler_index = match registration.as_str() {
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "any" => 1,
            "mount" => 2,
            _ => return,
        };
        let Some(handler @ Expr::Lambda { .. }) = args.get(handler_index) else {
            return;
        };
        let request = self.interner.intern("Request");
        let response = self.interner.intern("Response");
        let Some(request_ty) = self.lookup_named_type(request) else {
            return;
        };
        let Some(response_ty) = self.lookup_named_type(response) else {
            return;
        };
        let expected = self.typed.types.alloc_type(TypeKind::Func {
            params: vec![request_ty],
            ret: response_ty,
        });
        let _ = self.check_expr(handler, Some(expected));
        self.check_route_handler_arg(&registration, args, handler, expected);
    }

    fn field_type(&mut self, base: TypeId, field: SymId, span: Span) -> TypeId {
        match self.typed.types.get_type(base).clone() {
            TypeKind::Struct { fields, .. } => {
                if let Some(f) = fields.iter().find(|f| f.name == field) {
                    if f.private && !self.in_test {
                        let fname = self.interner.resolve(field);
                        let owner = match self.typed.types.get_type(base) {
                            TypeKind::Struct { name, .. } => self.interner.resolve(*name),
                            _ => "type",
                        };
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            span,
                            err::E0104,
                            format!(
                                "field '{fname}' on '{owner}' is file-private (marked with '-')"
                            ),
                            "private fields are visible only in this module and `test` blocks; see the struct declaration",
                        );
                    }
                    return f.ty;
                }
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0103,
                    format!("unknown field `{}`", self.interner.resolve(field)),
                    "hover the struct name to see its fields and C-ABI layout",
                );
                self.builtin("i64")
            }
            TypeKind::Sum { name, variants, .. } => {
                // Payload field on a sum value, or (legacy) Type.Variant via Field.
                if let Some(v) = variants.iter().find(|v| v.name == field) {
                    let _ = (name, v);
                    return base;
                }
                for v in &variants {
                    if let Some((_, ty)) = v.fields.iter().find(|(n, _)| *n == field) {
                        return *ty;
                    }
                }
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0103,
                    format!(
                        "unknown variant or field `{}` on `{}`",
                        self.interner.resolve(field),
                        self.interner.resolve(name)
                    ),
                    "use `Sum.Variant` / `Sum.Variant(args…)` constructors, or payload field names",
                );
                self.builtin("i64")
            }
            // `http.ok` / `ecs.World` — members of an imported std module.
            TypeKind::Module { .. } => {
                if let Some(tid) = self.lookup_named_type(field) {
                    return tid;
                }
                // Host function on the module (ok, simulate_post, …)
                self.typed.types.alloc_type(TypeKind::Func {
                    params: vec![],
                    ret: self.builtin("i64"),
                })
            }
            _ => self.builtin("i64"),
        }
    }

    fn check_named_variant(
        &mut self,
        sum: SymId,
        variant: SymId,
        args: &[Expr],
        span: Span,
        expected: Option<TypeId>,
    ) -> TypeId {
        let Some(&tid) = self.typed.sums.get(&sum) else {
            let sname = self.interner.resolve(sum).to_owned();
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0101,
                format!("unknown sum type `{sname}`"),
                "declare the sum type before constructing its variants",
            );
            return expected.unwrap_or_else(|| self.builtin("i64"));
        };
        let TypeKind::Sum { variants, .. } = self.typed.types.get_type(tid).clone() else {
            return tid;
        };
        let Some(v) = variants.iter().find(|v| v.name == variant) else {
            let sname = self.interner.resolve(sum).to_owned();
            let vname = self.interner.resolve(variant).to_owned();
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0103,
                format!("unknown variant `{vname}` on `{sname}`"),
                "hover the sum type to see its variants",
            );
            return tid;
        };
        // Empty-arg form is valid as a `when` tag pattern even for payload variants.
        if !args.is_empty() {
            if args.len() != v.fields.len() {
                let vname = self.interner.resolve(variant).to_owned();
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0202,
                    format!(
                        "variant `{vname}` expects {} field(s), found {}",
                        v.fields.len(),
                        args.len()
                    ),
                    "pass one argument per payload field, in declaration order",
                );
            }
            for (arg, (_, fty)) in args.iter().zip(v.fields.iter()) {
                let _ = self.check_expr(arg, Some(*fty));
            }
            for arg in args.iter().skip(v.fields.len()) {
                let _ = self.check_expr(arg, None);
            }
        } else if !v.fields.is_empty() {
            // Unit construction of a payload variant is only legal as a when pattern;
            // as a value it would be incomplete. Soft-allow when expected is the sum
            // (pattern context) — construction sites typically pass no expected.
            if expected.is_none() {
                let vname = self.interner.resolve(variant).to_owned();
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0202,
                    format!(
                        "variant `{vname}` requires {} payload field(s)",
                        v.fields.len()
                    ),
                    "write `Sum.Variant(args…)` or use the bare name only inside `when` arms",
                );
            }
        }
        tid
    }

    fn check_assignment_target(&mut self, target: &Expr, expected: Option<TypeId>) -> TypeId {
        match target {
            Expr::Ident(name, span) => {
                if let Some(binding) = self.find_binding(*name) {
                    self.record_reference(*name, *span, binding.span);
                    binding.ty
                } else {
                    expected.unwrap_or_else(|| self.builtin("i64"))
                }
            }
            Expr::Field { base, field, span } => {
                let base_ty = self.check_expr(base, None);
                self.field_type(base_ty, *field, *span)
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.check_expr(base, None);
                let _ = self.check_expr(index, Some(self.builtin("i64")));
                match self.typed.types.get_type(base_ty) {
                    TypeKind::Array(elem) => *elem,
                    TypeKind::Map { value, .. } => *value,
                    _ => expected.unwrap_or_else(|| self.builtin("i64")),
                }
            }
            _ => self.check_expr(target, expected),
        }
    }

    fn resolve_type_expr(&mut self, te: &TypeExpr) -> TypeId {
        match te {
            TypeExpr::Named(name, _) => self
                .lookup_named_type(*name)
                .unwrap_or_else(|| self.builtin("i64")),
            TypeExpr::Path(path, _) => {
                if let Some(last) = path.last() {
                    self.lookup_named_type(*last)
                        .unwrap_or_else(|| self.builtin("i64"))
                } else {
                    self.builtin("i64")
                }
            }
            TypeExpr::Optional(inner, _) => {
                let t = self.resolve_type_expr(inner);
                self.typed.types.alloc_type(TypeKind::Optional(t))
            }
            TypeExpr::Error(inner, _) => {
                let t = self.resolve_type_expr(inner);
                self.typed.types.alloc_type(TypeKind::Error(t))
            }
            TypeExpr::Chan(inner, _) => {
                let t = self.resolve_type_expr(inner);
                self.typed.types.alloc_type(TypeKind::Chan(t))
            }
            TypeExpr::Type(_) => self.typed.types.alloc_type(TypeKind::TypeMeta),
            TypeExpr::Code(inner, _) => {
                let t = self.resolve_type_expr(inner);
                self.typed.types.alloc_type(TypeKind::Code(t))
            }
            TypeExpr::Apply { name, args, span } => {
                let label = self.interner.resolve(*name).to_string();
                let user_type =
                    self.typed.structs.contains_key(name) || self.typed.sums.contains_key(name);
                match BuiltinTypeConstructor::from_name(&label).filter(|_| !user_type) {
                    Some(BuiltinTypeConstructor::Fn) => {
                        let Some((ret, params)) = args.split_last() else {
                            self.diagnostics.error(
                                DiagnosticKind::Type,
                                *span,
                                "`Fn[...]` requires at least a return type",
                            );
                            return self.builtin("i64");
                        };
                        let params = params
                            .iter()
                            .map(|param| self.resolve_type_expr(param))
                            .collect();
                        let ret = self.resolve_type_expr(ret);
                        self.typed.types.alloc_type(TypeKind::Func { params, ret })
                    }
                    None => {
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            *span,
                            err::E0203,
                            format!("`{label}` is not a type constructor"),
                            "only `Fn[Params…, Ret]` and `chan[T]` take type arguments",
                        );
                        self.builtin("i64")
                    }
                }
            }
        }
    }

    /// Collect mutable `@ident` captures inside a spawn body for race analysis.
    fn collect_mut_captures(&self, expr: &Expr) -> HashSet<SymId> {
        let mut out = HashSet::new();
        collect_mut_captures_expr(expr, &mut out);
        out
    }

    fn lookup_named_type(&mut self, name: SymId) -> Option<TypeId> {
        self.mark_used(name);
        let s = self.interner.resolve(name);
        if let Some(t) = self.builtins.get(s) {
            return Some(*t);
        }
        if let Some(t) = self.typed.structs.get(&name) {
            return Some(*t);
        }
        if let Some(t) = self.typed.sums.get(&name) {
            return Some(*t);
        }
        None
    }

    fn lookup_binding(&mut self, name: SymId, span: Span) -> TypeId {
        // Discard binding `_` is always in scope (when arms, destructuring, etc.).
        if self.interner.resolve(name) == "_" {
            return self.builtin("i64");
        }
        self.mark_used(name);
        if let Some(info) = self.find_binding(name) {
            self.record_reference(name, span, info.span);
            return info.ty;
        }
        if let Some(f) = self.typed.functions.get(&name) {
            let ty = f.ty;
            let def_span = f.span;
            self.record_reference(name, span, def_span);
            return ty;
        }
        // Type names are first-class for `req.json(CreateTask)` etc.
        if let Some(tid) = self.typed.structs.get(&name) {
            return *tid;
        }
        if let Some(tid) = self.typed.sums.get(&name) {
            return *tid;
        }
        if let Some(canon) = self.typed.aliases.get(&name).copied()
            && let Some(func) = self.typed.functions.get(&canon)
        {
            let ty = func.ty;
            let def_span = func.span;
            self.record_reference(name, span, def_span);
            return ty;
        }
        // Language prelude only — http/ecs/db require `use std.*`.
        let n = self.interner.resolve(name);
        if matches!(
            n,
            "print" | "println" | "assert_eq" | "round" | "map" | "keep" | "group_by" | "sum"
        ) {
            return self.builtin("i64");
        }
        if let Some(manifest) = match n {
            "http" => Some("std.net.http"),
            "ecs" => Some("std.game.ecs"),
            "db" => Some("std.db.sql"),
            "collections" => Some("std.core.collections"),
            _ => None,
        } {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0102,
                format!("`{n}` is not in scope"),
                format!("add `use {manifest}` at the top of this file"),
            );
            return self.builtin("i64");
        }
        // Embed host namespace is always available for `host.fn(...)` FFI calls.
        if n == "host" {
            let name = self.interner.intern("host");
            return self.typed.types.alloc_type(TypeKind::Module { name });
        }
        if self.macro_names.contains(&name) {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0101,
                format!("undefined name `{n}`"),
                format!("`{n}` is a macro: write `#{n}(…)` or `#{n} {{ … }}`"),
            );
            return self.builtin("i64");
        }
        self.diagnostics.error_code_help(
            DiagnosticKind::Type,
            span,
            err::E0101,
            format!("undefined name `{n}`"),
            "check spelling, or import it with `use std.…` if it lives in the standard library",
        );
        self.builtin("i64")
    }

    fn find_binding(&self, name: SymId) -> Option<BindingInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(&name) {
                return Some(b.clone());
            }
        }
        self.typed.bindings.get(&name).cloned()
    }

    fn unify_soft(&mut self, got: TypeId, expected: TypeId, span: Span) -> TypeId {
        if got == expected {
            return got;
        }
        if matches!(self.typed.types.get_type(got), TypeKind::Infer)
            || matches!(self.typed.types.get_type(expected), TypeKind::Infer)
        {
            return expected;
        }
        // Structural T? / T! equality (distinct TypeIds, same shape) and T → T?/T!.
        match (
            self.typed.types.get_type(got),
            self.typed.types.get_type(expected),
        ) {
            (TypeKind::Optional(a), TypeKind::Optional(b)) if *a == *b => return expected,
            (TypeKind::Error(a), TypeKind::Error(b)) if *a == *b => return expected,
            (_, TypeKind::Optional(a)) if *a == got => return expected,
            (_, TypeKind::Error(a)) if *a == got => return expected,
            (TypeKind::Array(_), TypeKind::Array(_)) => return expected,
            (TypeKind::Map { .. }, TypeKind::Map { .. }) => return expected,
            (TypeKind::Never, _)
            | (_, TypeKind::Never)
            | (TypeKind::Void, _)
            | (_, TypeKind::Void) => {
                return expected;
            }
            (TypeKind::TypeMeta, _) | (_, TypeKind::TypeMeta) => return expected,
            (TypeKind::Module { .. }, _) | (_, TypeKind::Module { .. }) => return expected,
            // Unit-sum / any sum discriminant coerces to i64 for HTTP route ids.
            (
                TypeKind::Sum { .. },
                TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::I128
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64,
            ) => return expected,
            (
                TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::I128
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64,
                TypeKind::Sum { .. },
            ) => return got,
            _ => {}
        }
        if self.is_numeric(got) && self.is_numeric(expected) {
            return expected;
        }
        let got_s = self.type_name(got);
        let exp_s = self.type_name(expected);
        // Real missing-`or` case: got T?/T! while expected is the unwrapped T.
        match self.typed.types.get_type(got) {
            TypeKind::Optional(inner) if *inner == expected => {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0202,
                    format!("expected `{exp_s}`, found `{got_s}` (optional value)"),
                    format!(
                        "unwrap with `or <default>` or `or err {{ … }}` before using the `{exp_s}` value"
                    ),
                );
                return expected;
            }
            TypeKind::Error(inner) if *inner == expected => {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    span,
                    err::E0202,
                    format!("expected `{exp_s}`, found `{got_s}` (fallible value)"),
                    format!(
                        "unwrap with `or <default>` or `or err {{ … }}` before using the `{exp_s}` value"
                    ),
                );
                return expected;
            }
            _ => {}
        }
        // Hard fail on concrete scalar / named mismatches — never silently accept.
        if self.is_hard_type_mismatch(got, expected) {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0202,
                format!("expected `{exp_s}`, found `{got_s}`"),
                format!(
                    "types must match; convert `{got_s}` to `{exp_s}`, or change the binding / parameter type"
                ),
            );
        }
        expected
    }

    /// True when `got` and `expected` are distinct concrete types that must not unify.
    fn is_hard_type_mismatch(&self, got: TypeId, expected: TypeId) -> bool {
        use TypeKind::*;
        match (
            self.typed.types.get_type(got),
            self.typed.types.get_type(expected),
        ) {
            (Infer, _) | (_, Infer) | (Never, _) | (_, Never) | (Void, _) | (_, Void) => false,
            (Optional(_), _) | (Error(_), _) | (_, Optional(_)) | (_, Error(_)) => false,
            (Array(_), _) | (_, Array(_)) | (Map { .. }, _) | (_, Map { .. }) => false,
            (TypeMeta, _) | (_, TypeMeta) | (Module { .. }, _) | (_, Module { .. }) => false,
            (Code(_), _) | (_, Code(_)) => false,
            (Duration, Duration) => false,
            (a, b) if self.is_numeric(got) && self.is_numeric(expected) => {
                let _ = (a, b);
                false
            }
            (Sum { .. }, I8 | I16 | I32 | I64 | I128 | U8 | U16 | U32 | U64)
            | (I8 | I16 | I32 | I64 | I128 | U8 | U16 | U32 | U64, Sum { .. }) => false,
            (Bool, Bool)
            | (I8, I8)
            | (I16, I16)
            | (I32, I32)
            | (I64, I64)
            | (I128, I128)
            | (U8, U8)
            | (U16, U16)
            | (U32, U32)
            | (U64, U64)
            | (F32, F32)
            | (F64, F64)
            | (Str, Str) => false,
            (
                Struct { name: a, .. } | Sum { name: a, .. } | Named(a),
                Struct { name: b, .. } | Sum { name: b, .. } | Named(b),
            ) if a == b => false,
            (Func { .. }, Func { .. })
            | (Chan(_), Chan(_))
            | (Task(_), Task(_))
            | (Shared(_), Shared(_)) => false,
            (
                Struct { name: a, .. } | Sum { name: a, .. } | Named(a),
                Struct { name: b, .. } | Sum { name: b, .. } | Named(b),
            ) if a != b => true,
            // Struct / sum identity mismatches stay hard. Scalar str↔i64 from
            // loosely-typed `_` / host paths stays soft here; call sites still
            // reject string *literals* passed to integer parameters via
            // `check_call_arg_literal`.
            (Bool, Str) | (Bool, F32) | (Bool, F64) => true,
            (Str, Struct { .. })
            | (Str, Sum { .. })
            | (Str, Named(_))
            | (Struct { .. }, Str)
            | (Sum { .. }, Str)
            | (Named(_), Str) => true,
            _ => false,
        }
    }

    /// Reject obvious literal mismatches at call boundaries (e.g. `take("nope")`).
    fn check_call_arg_literal(&mut self, arg: &Expr, expected: TypeId) {
        let exp = self.typed.types.get_type(expected);
        match (arg, exp) {
            (
                Expr::String(_, span),
                TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::I128
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64
                | TypeKind::Bool,
            ) => {
                let exp_s = self.type_name(expected);
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    *span,
                    err::E0202,
                    format!("expected `{exp_s}`, found `str`"),
                    format!(
                        "types must match; convert `str` to `{exp_s}`, or change the binding / parameter type"
                    ),
                );
            }
            (
                Expr::Bool(_, span),
                TypeKind::Str | TypeKind::I64 | TypeKind::F64 | TypeKind::F32,
            ) => {
                let exp_s = self.type_name(expected);
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    *span,
                    err::E0202,
                    format!("expected `{exp_s}`, found `bool`"),
                    format!("types must match; pass a `{exp_s}` value"),
                );
            }
            _ => {}
        }
    }

    fn type_name(&self, tid: TypeId) -> String {
        match self.typed.types.get_type(tid) {
            TypeKind::Void => "void".into(),
            TypeKind::Bool => "bool".into(),
            TypeKind::I8 => "i8".into(),
            TypeKind::I16 => "i16".into(),
            TypeKind::I32 => "i32".into(),
            TypeKind::I64 => "i64".into(),
            TypeKind::I128 => "i128".into(),
            TypeKind::U8 => "u8".into(),
            TypeKind::U16 => "u16".into(),
            TypeKind::U32 => "u32".into(),
            TypeKind::U64 => "u64".into(),
            TypeKind::F32 => "f32".into(),
            TypeKind::F64 => "f64".into(),
            TypeKind::Str => "str".into(),
            TypeKind::Optional(t) => format!("{}?", self.type_name(*t)),
            TypeKind::Error(t) => format!("{}!", self.type_name(*t)),
            TypeKind::Array(t) => format!("[{}]", self.type_name(*t)),
            TypeKind::Map { key, value } => {
                format!("{{{}: {}}}", self.type_name(*key), self.type_name(*value))
            }
            TypeKind::Struct { name, .. } => {
                let n = self.interner.resolve(*name);
                if n.is_empty() {
                    "{…}".into()
                } else {
                    n.into()
                }
            }
            TypeKind::Sum { name, .. } => self.interner.resolve(*name).into(),
            TypeKind::Named(n) => {
                let name = self.interner.resolve(*n);
                if let Some(tid) = self.typed.structs.get(n).or_else(|| self.typed.sums.get(n)) {
                    return self.type_name(*tid);
                }
                name.into()
            }
            TypeKind::Func { params, ret } => {
                let ps: Vec<_> = params.iter().map(|p| self.type_name(*p)).collect();
                format!("({}) → {}", ps.join(", "), self.type_name(*ret))
            }
            TypeKind::Module { name } => format!("module {}", self.interner.resolve(*name)),
            TypeKind::Chan(t) => format!("chan[{}]", self.type_name(*t)),
            TypeKind::Task(t) => format!("task[{}]", self.type_name(*t)),
            TypeKind::Shared(t) => format!("shared {}", self.type_name(*t)),
            TypeKind::Duration => "duration".into(),
            TypeKind::TypeMeta => "type".into(),
            TypeKind::Code(t) => format!("Code[{}]", self.type_name(*t)),
            TypeKind::Infer => "_".into(),
            TypeKind::Never => "never".into(),
            TypeKind::TraitBound { name } => {
                format!("trait {}", self.interner.resolve(*name))
            }
        }
    }

    fn is_static_type_method(&self, callee: &Expr) -> bool {
        matches!(
            callee,
            Expr::Field {
                base,
                ..
            } if self.is_type_name_receiver(base.as_ref())
        )
    }

    fn is_type_name_receiver(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Ident(ns, _) if self.typed.structs.contains_key(ns))
    }

    /// Resolve a callable, ranking `FuncFamily` overloads when argument types are known.
    fn resolve_callable_ranked(
        &mut self,
        callee: &Expr,
        arg_tys: &[TypeId],
        receiver_ty: Option<TypeId>,
    ) -> Option<FunctionInfo> {
        let key = match callee {
            Expr::Ident(name, _) => Some(*name),
            Expr::Field { base, field, .. } => {
                if let Expr::Ident(ns, _) = base.as_ref()
                    && self.is_module_binding(*ns)
                {
                    let fname = self.canonical(*field);
                    if (self.typed.families.contains_key(&fname)
                        || self.typed.functions.contains_key(&fname))
                        && let Some(picked) = self.pick_overload(fname, arg_tys, receiver_ty, true)
                    {
                        return Some(picked);
                    }
                }
                let q = if let Expr::Ident(ns, _) = base.as_ref() {
                    let owner = self.canonical(*ns);
                    format!(
                        "{}.{}",
                        self.interner.resolve(owner),
                        self.interner.resolve(*field)
                    )
                } else {
                    format!(".{}", self.interner.resolve(*field))
                };
                Some(self.interner.intern(&q))
            }
            _ => None,
        }?;
        self.pick_overload(key, arg_tys, receiver_ty, false)
    }

    fn pick_overload(
        &mut self,
        key: SymId,
        arg_tys: &[TypeId],
        receiver_ty: Option<TypeId>,
        free_only: bool,
    ) -> Option<FunctionInfo> {
        let family: Vec<FunctionInfo> = self
            .typed
            .families
            .get(&key)
            .cloned()
            .or_else(|| self.typed.functions.get(&key).cloned().map(|i| vec![i]))?;
        let mut call_tys = Vec::new();
        if let Some(rt) = receiver_ty {
            call_tys.push(rt);
        }
        call_tys.extend_from_slice(arg_tys);

        let applicable: Vec<&FunctionInfo> = family
            .iter()
            .filter(|info| {
                if free_only && info.owner.is_some() {
                    return false;
                }
                if call_tys.is_empty() {
                    return true;
                }
                if info.params.len() != call_tys.len() {
                    return false;
                }
                info.params.iter().zip(call_tys.iter()).enumerate().all(
                    |(i, ((_, pty, _), aty))| {
                        self.arg_compatible(*pty, info.trait_params.get(i).copied().flatten(), *aty)
                    },
                )
            })
            .collect();

        if applicable.is_empty() {
            return self.typed.functions.get(&key).cloned();
        }
        if applicable.len() == 1 {
            return Some(applicable[0].clone());
        }

        // Dominance: A dominates B iff ∀i rank(A)≥rank(B) and ∃j rank(A)>rank(B).
        let mut champions: Vec<&FunctionInfo> = Vec::new();
        for cand in &applicable {
            let dominated = applicable.iter().any(|other| {
                if std::ptr::eq(*other, *cand) {
                    return false;
                }
                Self::dominates(other, cand)
            });
            if !dominated {
                champions.push(*cand);
            }
        }
        if champions.len() == 1 {
            return Some(champions[0].clone());
        }
        if champions.len() > 1 && !call_tys.is_empty() {
            let span = champions[0].span;
            let name = self.interner.resolve(key).to_string();
            let mut sigs = String::new();
            for (i, c) in champions.iter().enumerate() {
                if i > 0 {
                    sigs.push_str("; ");
                }
                sigs.push_str(&self.format_fn_sig(c));
            }
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0205,
                format!("ambiguous call to `{name}`: candidates [{sigs}]"),
                "add a more specific overload or cast arguments to disambiguate",
            );
            for c in champions.iter().skip(1) {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Type,
                    c.span,
                    err::E0205,
                    format!("competing overload of `{name}`",),
                    "narrow a parameter type or remove the duplicate candidate",
                );
            }
        }
        Some(champions.first().copied().unwrap_or(applicable[0]).clone())
    }

    fn dominates(a: &FunctionInfo, b: &FunctionInfo) -> bool {
        if a.param_ranks.len() != b.param_ranks.len() {
            return false;
        }
        let mut strict = false;
        for (ra, rb) in a.param_ranks.iter().zip(b.param_ranks.iter()) {
            if ra < rb {
                return false;
            }
            if ra > rb {
                strict = true;
            }
        }
        strict
    }

    fn arg_compatible(&self, param_ty: TypeId, trait_bound: Option<SymId>, arg_ty: TypeId) -> bool {
        if let Some(tr) = trait_bound {
            return self.type_implements(arg_ty, tr);
        }
        match self.typed.types.get_type(param_ty) {
            TypeKind::Infer | TypeKind::Named(_) => true,
            TypeKind::TraitBound { name } => self.type_implements(arg_ty, *name),
            _ => {
                // Exact or structurally equal TypeId, or both numeric/infer.
                param_ty == arg_ty
                    || matches!(
                        (
                            self.typed.types.get_type(param_ty),
                            self.typed.types.get_type(arg_ty)
                        ),
                        (TypeKind::Infer, _) | (_, TypeKind::Infer)
                    )
            }
        }
    }

    fn type_implements(&self, ty: TypeId, trait_name: SymId) -> bool {
        let type_name = match self.typed.types.get_type(ty) {
            TypeKind::Struct { name, .. } | TypeKind::Sum { name, .. } => *name,
            TypeKind::Named(n) => *n,
            _ => return false,
        };
        self.typed
            .impls
            .get(&type_name)
            .is_some_and(|ts| ts.contains(&trait_name))
    }

    fn format_fn_sig(&self, info: &FunctionInfo) -> String {
        let params: Vec<String> = info
            .params
            .iter()
            .zip(info.trait_params.iter())
            .map(|((name, ty, mutable), tr)| {
                let n = self.interner.resolve(*name);
                let at = if *mutable { "@" } else { "" };
                let t = if let Some(tr) = tr {
                    self.interner.resolve(*tr).to_string()
                } else {
                    self.type_label(*ty)
                };
                format!("{at}{n}: {t}")
            })
            .collect();
        format!("({})", params.join(", "))
    }

    fn type_label(&self, ty: TypeId) -> String {
        match self.typed.types.get_type(ty) {
            TypeKind::I8 => "i8".into(),
            TypeKind::I16 => "i16".into(),
            TypeKind::I32 => "i32".into(),
            TypeKind::I64 => "i64".into(),
            TypeKind::I128 => "i128".into(),
            TypeKind::U8 => "u8".into(),
            TypeKind::U16 => "u16".into(),
            TypeKind::U32 => "u32".into(),
            TypeKind::U64 => "u64".into(),
            TypeKind::F64 => "f64".into(),
            TypeKind::F32 => "f32".into(),
            TypeKind::Bool => "bool".into(),
            TypeKind::Str => "str".into(),
            TypeKind::Infer => "_".into(),
            TypeKind::Struct { name, .. }
            | TypeKind::Sum { name, .. }
            | TypeKind::Named(name)
            | TypeKind::TraitBound { name } => self.interner.resolve(*name).to_string(),
            TypeKind::Optional(inner) => format!("{}?", self.type_label(*inner)),
            TypeKind::Error(inner) => format!("{}!", self.type_label(*inner)),
            TypeKind::Array(inner) => format!("[{}]", self.type_label(*inner)),
            TypeKind::Func { .. } => "fn".into(),
            _ => "any".into(),
        }
    }

    /// Multi-subject `when (e1, e2, …)` — Mode 1 type/trait tables and Mode 2 sum-tag matrices.
    fn check_multi_when(&mut self, subjects: &[Expr], arms: &[WhenArm], span: Span) {
        let subj_tys: Vec<TypeId> = subjects.iter().map(|s| self.check_expr(s, None)).collect();
        let all_sums = subj_tys
            .iter()
            .all(|tid| matches!(self.typed.types.get_type(*tid), TypeKind::Sum { .. }));
        let open_subjects = subj_tys.iter().any(|tid| {
            matches!(
                self.typed.types.get_type(*tid),
                TypeKind::Infer | TypeKind::TraitBound { .. } | TypeKind::Named(_)
            )
        });
        let mut has_else = false;
        let mut seen_rank_sigs: Vec<(Vec<u8>, Vec<ArmCol>, Span)> = Vec::new();
        for arm in arms {
            match &arm.pattern {
                WhenPattern::Else => {
                    has_else = true;
                    let _ = self.check_expr(&arm.body, None);
                }
                WhenPattern::Tuple {
                    elems,
                    span: pat_span,
                } => {
                    if elems.len() != subjects.len() {
                        self.diagnostics.error_code_help(
                            DiagnosticKind::Type,
                            *pat_span,
                            err::E0201,
                            format!(
                                "tuple when-arm has {} pattern(s) but subject has {}",
                                elems.len(),
                                subjects.len()
                            ),
                            "match the subject arity: `when (a, b) { (P1, P2) => … }`",
                        );
                    }
                    let mut ranks = Vec::with_capacity(elems.len());
                    self.push_scope();
                    for (i, elem) in elems.iter().enumerate() {
                        let subj_ty = subj_tys.get(i).copied();
                        match elem {
                            WhenPattern::Expr(e) => {
                                if let Expr::Ident(n, _) = e {
                                    let name = self.interner.resolve(*n);
                                    if name == "_" {
                                        ranks.push(SpecRank::Unconstrained as u8);
                                    } else if self.typed.traits.contains_key(n) {
                                        ranks.push(SpecRank::Trait as u8);
                                    } else if self.typed.structs.contains_key(n)
                                        || self.typed.sums.contains_key(n)
                                    {
                                        ranks.push(SpecRank::Concrete as u8);
                                    } else if let Some(tid) = subj_ty {
                                        let _ = self.check_expr(e, Some(tid));
                                        ranks.push(SpecRank::Concrete as u8);
                                    } else {
                                        let _ = self.check_expr(e, None);
                                        ranks.push(SpecRank::Concrete as u8);
                                    }
                                } else if let Expr::NamedVariant { variant, .. } = e {
                                    ranks.push(SpecRank::Variant as u8);
                                    let _ = variant;
                                    if let Some(tid) = subj_ty {
                                        let _ = self.check_expr(e, Some(tid));
                                    }
                                } else {
                                    ranks.push(SpecRank::Concrete as u8);
                                    let _ = self.check_expr(e, subj_ty);
                                }
                            }
                            WhenPattern::VariantBind {
                                sum,
                                variant,
                                fields,
                                span: vspan,
                            } => {
                                ranks.push(SpecRank::Variant as u8);
                                if let Some(tid) = subj_ty
                                    && let TypeKind::Sum { variants, .. } =
                                        self.typed.types.get_type(tid).clone()
                                    && let Some(vlayout) =
                                        variants.iter().find(|v| v.name == *variant)
                                {
                                    for (payload, local) in fields {
                                        if let Some((_, fty)) =
                                            vlayout.fields.iter().find(|(n, _)| n == payload)
                                        {
                                            self.scopes.last_mut().unwrap().insert(
                                                *local,
                                                BindingInfo {
                                                    ty: *fty,
                                                    exported: false,
                                                    mutable: false,
                                                    span: *vspan,
                                                },
                                            );
                                        }
                                    }
                                }
                                let _ = (sum, variant);
                            }
                            _ => ranks.push(SpecRank::Unconstrained as u8),
                        }
                    }
                    // Ambiguous arm ranking (E0205) when two arms have incomparable ranks, unless
                    // they provably never meet or an earlier arm already claims their overlap.
                    let cols: Vec<ArmCol> = elems.iter().map(|e| self.arm_col(e)).collect();
                    for (prior_ranks, prior_cols, prior_span) in &seen_rank_sigs {
                        if prior_ranks.len() == ranks.len() {
                            let a_dom = prior_ranks.iter().zip(ranks.iter()).all(|(a, b)| a >= b)
                                && prior_ranks.iter().zip(ranks.iter()).any(|(a, b)| a > b);
                            let b_dom = ranks.iter().zip(prior_ranks.iter()).all(|(a, b)| a >= b)
                                && ranks.iter().zip(prior_ranks.iter()).any(|(a, b)| a > b);
                            let resolved = match ArmCol::intersect(prior_cols, &cols) {
                                Overlap::Disjoint => true,
                                Overlap::At(meet) => seen_rank_sigs
                                    .iter()
                                    .any(|(_, c, _)| ArmCol::owns(c, &meet)),
                                Overlap::Unknown => false,
                            };
                            if !a_dom && !b_dom && prior_ranks != &ranks && !resolved {
                                self.diagnostics.error_code_help(
                                    DiagnosticKind::Type,
                                    *pat_span,
                                    err::E0205,
                                    "ambiguous multi-method `when` arms (neither dominates)",
                                    "order arms by specificity or add an `else` fallback",
                                );
                                self.diagnostics.error_code_help(
                                    DiagnosticKind::Type,
                                    *prior_span,
                                    err::E0205,
                                    "competing multi-method `when` arm",
                                    "narrow one arm's patterns so it dominates the other",
                                );
                            }
                        }
                    }
                    seen_rank_sigs.push((ranks, cols, *pat_span));
                    let _ = self.check_expr(&arm.body, None);
                    self.pop_scope();
                }
                other => {
                    let _ = other;
                    self.diagnostics.error_code_help(
                        DiagnosticKind::Type,
                        arm.span,
                        err::E0201,
                        "multi-subject `when` arms must be tuple patterns `(P1, P2, …)`",
                        "write `(VariantA, VariantB) => …` or `(T1, T2) => …`",
                    );
                    let _ = self.check_expr(&arm.body, None);
                }
            }
        }
        if open_subjects && !has_else {
            self.diagnostics.error_code_help(
                DiagnosticKind::Type,
                span,
                err::E0201,
                "open multi-method `when` (trait/`_` subjects) requires an `else` arm",
                "add `else => …` as the catch-all specialization",
            );
        }
        if all_sums && !has_else {
            // Mode 2: cartesian exhaustiveness is checked lightly — prefer else for large matrices.
            let _ = all_sums;
        }
    }

    fn check_inout_call(
        &mut self,
        info: &FunctionInfo,
        receiver: Option<&Expr>,
        args: &[Expr],
        span: Span,
    ) {
        for (i, (_pname, _pty, mut_ref)) in info.params.iter().enumerate() {
            if !*mut_ref {
                continue;
            }
            let arg = if i == 0 {
                receiver.or_else(|| args.first())
            } else if receiver.is_some() {
                args.get(i - 1)
            } else {
                args.get(i)
            };
            let Some(arg) = arg else { continue };
            if !self.arg_is_mut_ref(arg) {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Mutation,
                    arg.span(),
                    err::E0301,
                    "cannot mutate without `@`",
                    "prefix the in-place argument with `@`, e.g. `@u.discount(...)`",
                );
                let _ = span;
            }
        }
    }

    fn arg_is_mut_ref(&self, expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Unary {
                op: UnOp::MutRef,
                ..
            }
        )
    }

    fn root_mut_sym(expr: &Expr) -> Option<SymId> {
        match expr {
            Expr::Unary {
                op: UnOp::MutRef,
                expr: inner,
                ..
            } => match inner.as_ref() {
                Expr::Ident(name, _) => Some(*name),
                _ => None,
            },
            Expr::Ident(name, _) => Some(*name),
            _ => None,
        }
    }

    fn check_root_escape(&mut self, expr: &Expr, span: Span) {
        let Some(name) = Self::root_mut_sym(expr) else {
            self.check_borrow_escape(expr, span);
            return;
        };
        if !self.root_mutable.contains(&name) {
            self.check_borrow_escape(expr, span);
            return;
        }
        let owner = self.interner.resolve(name);
        self.diagnostics.error_code_help(
            DiagnosticKind::Lifetime,
            span,
            err::E0401,
            format!("mutable root `@{owner}` cannot escape its declaring module"),
            format!(
                "`state {owner}` and other roots must stay module-scoped; return an owned copy instead of `@{owner}`"
            ),
        );
    }

    /// True when `ty` is a struct or sum that declares a method named `method`.
    fn receiver_defines_method(&mut self, ty: TypeId, method: SymId) -> bool {
        let (TypeKind::Struct { name: owner, .. } | TypeKind::Sum { name: owner, .. }) =
            self.typed.types.get_type(ty).clone()
        else {
            return false;
        };
        let key = format!(
            "{}.{}",
            self.interner.resolve(owner),
            self.interner.resolve(method)
        );
        let key = self.interner.intern(&key);
        self.typed.functions.contains_key(&key)
    }

    fn expr_names_send(&self, callee: &Expr) -> bool {
        match callee {
            Expr::Ident(name, _) => self.interner.resolve(*name) == "send",
            Expr::Field { field, .. } | Expr::Pipeline { method: field, .. } => {
                self.interner.resolve(*field) == "send"
            }
            _ => false,
        }
    }

    fn check_spawn_escape(&mut self, expr: &Expr, span: Span) {
        match expr {
            Expr::Block(b) => {
                for stmt in &b.stmts {
                    match stmt {
                        Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => {
                            self.check_spawn_escape(e, span);
                        }
                        Stmt::Binding(b) => self.check_spawn_escape(&b.value, span),
                        _ => {}
                    }
                }
            }
            other => self.check_root_escape(other, span),
        }
    }

    /// Non-`@` stack borrows (array slices / field projections of locals) must not
    /// escape their declaring scope via return, record store, channel send, or spawn.
    /// `str` slices are destination-built owned `str` values and may escape.
    fn check_borrow_escape(&mut self, expr: &Expr, span: Span) {
        if !self.expr_is_stack_borrow(expr) {
            return;
        }
        self.diagnostics.error_code_help(
            DiagnosticKind::Lifetime,
            span,
            err::E0402,
            "stack-owned borrow cannot escape its declaring scope",
            "copy into an owned `str` / array / struct before returning, storing, sending, or spawning",
        );
    }

    fn expr_is_stack_borrow(&self, expr: &Expr) -> bool {
        match expr {
            // Array slices borrow parent storage; `str` slices materialize owned `str`.
            Expr::Slice { base, .. } => {
                let bt = self.infer_escape_base_ty(base);
                matches!(self.typed.types.get_type(bt), TypeKind::Array(_))
            }
            Expr::Field { base, .. } => self.expr_is_stack_borrow(base),
            Expr::Index { base, .. } => {
                // Scalar element loads copy; only nested-array element slots borrow.
                let bt = self.infer_escape_base_ty(base);
                match self.typed.types.get_type(bt) {
                    TypeKind::Array(elem) => {
                        matches!(self.typed.types.get_type(*elem), TypeKind::Array(_))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn infer_escape_base_ty(&self, expr: &Expr) -> TypeId {
        match expr {
            Expr::Ident(name, _) => self
                .find_binding(*name)
                .map(|b| b.ty)
                .unwrap_or_else(|| self.builtin("_")),
            Expr::Field { base, field, .. } => {
                let bt = self.infer_escape_base_ty(base);
                // Best-effort: reuse recorded field layout when available.
                match self.typed.types.get_type(bt) {
                    TypeKind::Struct { fields, .. } => fields
                        .iter()
                        .find(|f| f.name == *field)
                        .map(|f| f.ty)
                        .unwrap_or_else(|| self.builtin("_")),
                    _ => self.builtin("_"),
                }
            }
            Expr::Slice { base, .. } | Expr::Index { base, .. } => self.infer_escape_base_ty(base),
            _ => self.builtin("_"),
        }
    }

    fn is_numeric(&self, t: TypeId) -> bool {
        matches!(
            self.typed.types.get_type(t),
            TypeKind::I8
                | TypeKind::I16
                | TypeKind::I32
                | TypeKind::I64
                | TypeKind::I128
                | TypeKind::U8
                | TypeKind::U16
                | TypeKind::U32
                | TypeKind::U64
                | TypeKind::F32
                | TypeKind::F64
        )
    }

    fn builtin(&self, name: &str) -> TypeId {
        *self.builtins.get(name).expect("builtin")
    }

    fn storage_for_type(&self, ty: TypeId, parameter: bool) -> StorageClass {
        if parameter {
            return StorageClass::Param;
        }
        match self.typed.types.get_type(ty) {
            TypeKind::Array(_) | TypeKind::Map { .. } => StorageClass::StackArray,
            TypeKind::Struct { .. } | TypeKind::Sum { .. } => StorageClass::StackLocal,
            _ => StorageClass::Register,
        }
    }

    fn record_symbol(
        &mut self,
        name: SymId,
        ty: TypeId,
        mutable: bool,
        storage: StorageClass,
        def_span: Span,
        kind: SymbolKind,
    ) {
        if !self.tracking_enabled {
            return;
        }
        if self
            .typed
            .symbols
            .iter()
            .any(|symbol| symbol.name == name && symbol.def_span == def_span && symbol.kind == kind)
        {
            return;
        }
        self.typed.symbols.push(SymbolInfo {
            name,
            ty,
            mutable,
            storage,
            def_span,
            kind,
        });
    }

    fn record_reference(&mut self, name: SymId, span: Span, def_span: Span) {
        if self.tracking_enabled {
            self.typed.symbol_references.push(SymbolReference {
                name,
                span,
                def_span,
            });
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.usage_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        let index = self.usage_scopes.len() - 1;
        self.finish_usage_scope(index);
        self.usage_scopes.pop();
        self.scopes.pop();
    }

    fn track_definition(&mut self, name: SymId, span: Span, kind: UsageKind) {
        if !self.tracking_enabled || self.interner.resolve(name).starts_with('_') {
            return;
        }
        self.usage_scopes.last_mut().expect("usage scope").insert(
            name,
            UsageInfo {
                span,
                kind,
                used: false,
            },
        );
    }

    fn mark_used(&mut self, name: SymId) {
        let mut names = self.import_uses.get(&name).cloned().unwrap_or_default();
        if names.is_empty() {
            names.push(name);
        }
        for name in names {
            self.mark_usage_name(name);
        }
    }

    fn mark_usage_name(&mut self, name: SymId) {
        for scope in self.usage_scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(&name) {
                info.used = true;
                return;
            }
        }
    }

    /// Resolve `"…{name}…"` / `"…{obj.field}…"` holes so undefined names are errors
    /// (route patterns like `"/{id}"` must use raw `r"/{id}"` instead).
    fn check_string_interpolations(&mut self, value: SymId, span: Span) {
        let text = self.interner.resolve(value).to_owned();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                i += 2;
                if chars[i - 1] == 'u' && chars.get(i) == Some(&'{') {
                    i += chars[i..]
                        .iter()
                        .position(|&c| c == '}')
                        .map_or(0, |p| p + 1);
                }
                continue;
            }
            if chars[i] != '{' {
                i += 1;
                continue;
            }
            let Some(end) = chars[i + 1..]
                .iter()
                .position(|&c| c == '}')
                .map(|p| i + 1 + p)
            else {
                break;
            };
            let inner: String = chars[i + 1..end].iter().collect();
            let inner = inner.trim();
            if Self::is_interp_path(inner) {
                let head = inner.split('.').next().unwrap_or(inner);
                let name = self.interner.intern(head);
                let _ = self.lookup_binding(name, span);
            } else {
                // Code hole (`{foo(bar)}`): mark free idents as used best-effort.
                let bytes = inner.as_bytes();
                let mut at = 0;
                while at < bytes.len() {
                    if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
                        let start = at;
                        at += 1;
                        while at < bytes.len()
                            && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_')
                        {
                            at += 1;
                        }
                        let name = self.interner.intern(&inner[start..at]);
                        self.mark_used(name);
                    } else {
                        at += 1;
                    }
                }
            }
            i = end + 1;
        }
    }

    fn is_interp_path(inner: &str) -> bool {
        if inner.is_empty() {
            return false;
        }
        inner.split('.').all(|seg| {
            let mut cs = seg.chars();
            matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
                && cs.all(|c| c.is_alphanumeric() || c == '_')
        })
    }

    fn finish_usage_scope(&mut self, index: usize) {
        let unused: Vec<_> = self.usage_scopes[index]
            .iter()
            .filter_map(|(&name, info)| (!info.used).then_some((name, *info)))
            .collect();
        for (name, info) in unused {
            let name = self.interner.resolve(name);
            let (code, noun) = match info.kind {
                UsageKind::Local => (err::E0303, "variable"),
                UsageKind::Parameter => (err::E0304, "parameter"),
                UsageKind::Import => (err::E0305, "import"),
            };
            let message = format!("unused {noun} '{name}'");
            let help = format!("prefix it with an underscore: `_{name}`");
            if self.unused_as_error {
                self.diagnostics.error_code_help(
                    DiagnosticKind::Other,
                    info.span,
                    code,
                    message,
                    help,
                );
            } else {
                self.diagnostics.warn_code_help(
                    DiagnosticKind::Other,
                    info.span,
                    code,
                    message,
                    help,
                );
            }
        }
    }
}

/// Builtin type constructors: names in the type namespace that take `[…]`
/// arguments. A user type of the same name shadows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinTypeConstructor {
    /// `Fn[Params…, Ret]` — callable signature.
    Fn,
}

impl BuiltinTypeConstructor {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "Fn" => Some(Self::Fn),
            _ => None,
        }
    }
}

fn span_key(span: Span) -> u64 {
    crate::meta::span_key(span)
}

fn align_up(offset: u32, align: u32) -> u32 {
    if align == 0 {
        return offset;
    }
    (offset + align - 1) & !(align - 1)
}

fn collect_mut_captures_expr(expr: &Expr, out: &mut HashSet<SymId>) {
    match expr {
        Expr::Unary {
            op: UnOp::MutRef,
            expr: inner,
            ..
        } => {
            if let Expr::Ident(name, _) = inner.as_ref() {
                out.insert(*name);
            }
            collect_mut_captures_expr(inner, out);
        }
        Expr::Binary { left, right, .. }
        | Expr::Assign {
            target: left,
            value: right,
            ..
        }
        | Expr::Or { left, right, .. }
        | Expr::Index {
            base: left,
            index: right,
            ..
        } => {
            collect_mut_captures_expr(left, out);
            collect_mut_captures_expr(right, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_mut_captures_expr(callee, out);
            for a in args {
                collect_mut_captures_expr(a, out);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            collect_mut_captures_expr(base, out);
            for a in args {
                collect_mut_captures_expr(a, out);
            }
        }
        Expr::Field { base, .. } => collect_mut_captures_expr(base, out),
        Expr::Unary { expr, .. }
        | Expr::Await(expr, _)
        | Expr::Spawn { body: expr, .. }
        | Expr::Shared { value: expr, .. }
        | Expr::Duration { value: expr, .. }
        | Expr::Chan { capacity: expr, .. } => collect_mut_captures_expr(expr, out),
        Expr::Array(elems, _) => {
            for e in elems {
                collect_mut_captures_expr(e, out);
            }
        }
        Expr::ArrayRepeat { value, len, .. } => {
            collect_mut_captures_expr(value, out);
            collect_mut_captures_expr(len, out);
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            collect_mut_captures_expr(cond, out);
            collect_mut_captures_expr(then_expr, out);
            collect_mut_captures_expr(else_expr, out);
        }
        Expr::Block(b) | Expr::If { then_block: b, .. } => {
            for s in &b.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Expr::OrErr { left, body, .. } => {
            collect_mut_captures_expr(left, out);
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Expr::Quote { body, .. }
        | Expr::Splice { code: body, .. }
        | Expr::ComptimeField { base: body, .. } => collect_mut_captures_expr(body, out),
        Expr::ComptimeBlock { body, .. }
        | Expr::ComptimeIf {
            then_block: body, ..
        }
        | Expr::ComptimeEach { body, .. } => {
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        _ => {}
    }
}

fn collect_when_pattern_mut_captures(pat: &WhenPattern, out: &mut HashSet<SymId>) {
    match pat {
        WhenPattern::Expr(e) | WhenPattern::After(e) => collect_mut_captures_expr(e, out),
        WhenPattern::Else | WhenPattern::VariantBind { .. } => {}
        WhenPattern::Tuple { elems, .. } => {
            for e in elems {
                collect_when_pattern_mut_captures(e, out);
            }
        }
    }
}

fn collect_mut_captures_stmt(stmt: &Stmt, out: &mut HashSet<SymId>) {
    match stmt {
        Stmt::Binding(b) => collect_mut_captures_expr(&b.value, out),
        Stmt::Expr(e) | Stmt::Fail(e, _) => collect_mut_captures_expr(e, out),
        Stmt::Panic(Some(e), _) => collect_mut_captures_expr(e, out),
        Stmt::Panic(None, _) => {}
        Stmt::Return(Some(e), _) => collect_mut_captures_expr(e, out),
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            collect_mut_captures_expr(cond, out);
            for s in &else_block.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Stmt::Each { iter, body, .. } => {
            collect_mut_captures_expr(iter, out);
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_mut_captures_expr(cond, out);
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                collect_mut_captures_expr(t, out);
            }
            for v in values {
                collect_mut_captures_expr(v, out);
            }
        }
        Stmt::Guarded { stmt, cond, .. } => {
            collect_mut_captures_expr(cond, out);
            collect_mut_captures_stmt(stmt, out);
        }
        Stmt::Scope { body, .. } => {
            for s in &body.stmts {
                collect_mut_captures_stmt(s, out);
            }
        }
        Stmt::Comptime(cs) => match cs {
            ComptimeStmt::Block(b, _)
            | ComptimeStmt::If { then_block: b, .. }
            | ComptimeStmt::Each { body: b, .. } => {
                for s in &b.stmts {
                    collect_mut_captures_stmt(s, out);
                }
            }
        },
        Stmt::When { subjects, arms, .. } => {
            for s in subjects {
                collect_mut_captures_expr(s, out);
            }
            for arm in arms {
                collect_when_pattern_mut_captures(&arm.pattern, out);
                collect_mut_captures_expr(&arm.body, out);
            }
        }
        _ => {}
    }
}

fn align_of(types: &Arena<TypeKind>, ty: TypeId) -> u32 {
    match types.get_type(ty) {
        TypeKind::Bool | TypeKind::I8 | TypeKind::U8 => 1,
        TypeKind::I16 | TypeKind::U16 => 2,
        TypeKind::I32 | TypeKind::U32 | TypeKind::F32 => 4,
        TypeKind::I64
        | TypeKind::U64
        | TypeKind::F64
        | TypeKind::Str
        | TypeKind::Duration
        | TypeKind::Chan(_)
        | TypeKind::Task(_)
        | TypeKind::Shared(_) => 8,
        TypeKind::I128 => 16,
        TypeKind::Struct { align, .. } => *align,
        _ => 8,
    }
}

fn size_of_ty(types: &Arena<TypeKind>, ty: TypeId) -> u32 {
    match types.get_type(ty) {
        TypeKind::Void | TypeKind::Never => 0,
        TypeKind::Bool | TypeKind::I8 | TypeKind::U8 => 1,
        TypeKind::I16 | TypeKind::U16 => 2,
        TypeKind::I32 | TypeKind::U32 | TypeKind::F32 => 4,
        TypeKind::I64
        | TypeKind::U64
        | TypeKind::F64
        | TypeKind::Str
        | TypeKind::Duration
        | TypeKind::Chan(_)
        | TypeKind::Task(_)
        | TypeKind::Shared(_) => 8,
        TypeKind::I128 => 16,
        TypeKind::Optional(inner) | TypeKind::Error(inner) => 1 + size_of_ty(types, *inner),
        TypeKind::Struct { size, .. } | TypeKind::Sum { size, .. } => *size,
        TypeKind::Array(_)
        | TypeKind::Map { .. }
        | TypeKind::Func { .. }
        | TypeKind::Module { .. } => 16,
        TypeKind::Named(_)
        | TypeKind::Infer
        | TypeKind::TypeMeta
        | TypeKind::Code(_)
        | TypeKind::TraitBound { .. } => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn analyze_src(src: &str) -> (TypedModule, Diagnostics) {
        analyze_src_opts(src, false)
    }

    fn analyze_src_opts(src: &str, unused_as_error: bool) -> (TypedModule, Diagnostics) {
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, src, &mut interner, &mut diags);
        let module = parser.parse_module();
        let mut sema = SemanticAnalyzer::new(&mut interner, &mut diags);
        sema.set_unused_as_error(unused_as_error);
        let typed = sema.analyze(&module);
        (typed, diags)
    }

    #[test]
    fn callable_context_infers_lambda_and_rejects_bad_route_return() {
        let (_, diagnostics) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
App = { get(@self, pattern: str, handler: Fn[Request, Response]) : App { self } }
main() {
    app = App{}
    app.get("/ping", |_req| => "pong")
}
"#,
        );
        assert!(diagnostics.has_errors());
        let messages = diagnostics
            .into_vec()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect::<Vec<_>>();
        assert!(
            messages.iter().any(|message| {
                message.contains("route \"/ping\" (App.get)")
                    && message.contains("Response")
                    && message.contains("str")
            }),
            "{messages:?}"
        );
    }

    #[test]
    fn callable_context_accepts_typed_route_response() {
        let (_, diagnostics) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
App = { get(@self, pattern: str, handler: Fn[Request, Response]) : App { self } }
main() {
    app = App{}
    @app.get("/ping", |_req| => Response.{status: 200})
}
"#,
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
    }

    #[test]
    fn callable_context_rejects_wrong_route_arity() {
        let (_, diagnostics) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
App = { get(@self, pattern: str, handler: Fn[Request, Response]) : App { self } }
main() {
    app = App{}
    app.get("/ping", |_req, extra| => Response.{status: 200})
}
"#,
        );
        let messages = diagnostics
            .into_vec()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect::<Vec<_>>();
        assert!(
            messages.iter().any(|message| {
                message.contains("route \"/ping\" (App.get)") && message.contains("parameter")
            }),
            "{messages:?}"
        );
    }

    #[test]
    fn every_callable_route_registration_enforces_response_return() {
        let cases = [
            ("get", "GET", false),
            ("post", "POST", false),
            ("put", "PUT", false),
            ("patch", "PATCH", false),
            ("delete", "DELETE", false),
            ("head", "HEAD", false),
            ("options", "OPTIONS", false),
            ("any", "ANY", false),
            ("mount", "CUSTOM", true),
        ];
        for (registration, _method, mounted) in cases {
            let params = if mounted {
                "method: str, pattern: str, handler: Fn[Request, Response]"
            } else {
                "pattern: str, handler: Fn[Request, Response]"
            };
            let args = if mounted {
                "\"CUSTOM\", \"/probe\", |_req| => 7"
            } else {
                "\"/probe\", |_req| => 7"
            };
            let source = format!(
                r#"
Request = {{ path: str = "" }}
Response = {{ status: i64 = 200 }}
App = {{
    {registration}(@self, {params}) : App {{ self }}
}}
main() {{
    app = App{{}}
    app.{registration}({args})
}}
"#
            );
            let (_, diagnostics) = analyze_src(&source);
            let messages = diagnostics
                .into_vec()
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>();
            assert!(
                messages.iter().any(|message| {
                    message.contains(&format!("route \"/probe\" (App.{registration})"))
                        && message.contains("Response")
                        && message.contains("i64")
                }),
                "{registration}: {messages:?}"
            );
        }
    }

    #[test]
    fn route_handler_rejects_unit_and_accepts_response_twin() {
        let (_, bad) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
App = { post(@self, pattern: str, handler: Fn[Request, Response]) : App { self } }
main() {
    app = App{}
    @app.post("/unit", |_req| {})
}
"#,
        );
        assert!(
            bad.into_vec()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("found `void`"))
        );

        let (_, good) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
App = { post(@self, pattern: str, handler: Fn[Request, Response]) : App { self } }
main() {
    app = App{}
    @app.post("/unit", |_req| => Response.{status: 201})
}
"#,
        );
        assert!(!good.has_errors(), "{:?}", good.into_vec());
    }

    #[test]
    fn app_get_and_map_get_coexist() {
        let (_, diagnostics) = analyze_src(
            r#"
Request = { path: str = "" }
Response = { status: i64 = 200 }
Map = {
    value: i64 = 0
    get(self, key: str) : i64 => self.value
}
App = { get(@self, path: str, handler: Fn[Request, Response]) : App { self } }
main() {
    values = Map.{value: 7}
    found = values.get("answer")
    app = App{}
    @app.get("/ping", |_req| => Response.{status: 200})
}
"#,
        );
        assert!(!diagnostics.has_errors(), "{:?}", diagnostics.into_vec());
    }

    #[test]
    fn inline_trait_requires_every_method() {
        let (typed, d) = analyze_src(
            r#"
+Widget = trait {
    +paint(self) : str
}
+Badge: Widget = {
    label: str = ""
    +paint(self) : str => self.label
}
+Bare: Widget = {
    label: str = ""
}
"#,
        );
        assert!(d.has_errors());
        let messages: Vec<_> = d.into_vec().into_iter().map(|diag| diag.message).collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("E0203") || message.contains("Bare")),
            "{messages:?}"
        );
        assert!(typed.impls.values().any(|names| !names.is_empty()));
        assert!(
            typed
                .trait_sigs
                .values()
                .any(|lines| lines.iter().any(|line| line.contains("paint(self) : str")))
        );
    }

    #[test]
    fn undeclared_add_is_a_language_trait_witness_error() {
        let (_typed, d) = analyze_src(
            r#"
+Vec2 = {
    x: f64 = 0.0
    +add(self, other: Vec2) : Vec2 => self
}
"#,
        );
        assert!(d.has_errors());
        let codes: Vec<_> = d
            .into_vec()
            .into_iter()
            .filter_map(|diag| diag.code)
            .collect();
        assert!(codes.contains(&"E0204"), "{codes:?}");
    }

    #[test]
    fn drifted_core_trait_is_rejected() {
        let (_typed, d) = analyze_src(
            r#"
+Eq = trait { +eq(self) : str }
"#,
        );
        assert!(d.has_errors());
        let codes: Vec<_> = d
            .into_vec()
            .into_iter()
            .filter_map(|diag| diag.code)
            .collect();
        assert!(codes.contains(&"E0204"), "{codes:?}");
    }

    #[test]
    fn undefined_string_interpolation_is_an_error() {
        // `"/{id}"` interpolates `id`; route patterns must use raw `r"/{id}"`.
        let (_typed, d) = analyze_src("+pattern() : str => \"/tasks/{id}\"\n");
        assert!(d.has_errors());
        let joined: String = d
            .into_vec()
            .into_iter()
            .map(|diag| diag.message)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("undefined name `id`"), "{joined}");
        let (_typed, d2) = analyze_src("+pattern() : str => r\"/tasks/{id}\"\n");
        assert!(!d2.has_errors(), "{:?}", d2.into_vec());
    }

    #[test]
    fn declared_add_matches_the_language_witness() {
        let (_typed, d) = analyze_src(
            r#"
+Vec2: Add = {
    x: f64 = 0.0
    +add(self, other: Vec2) : Vec2 => Vec2.{x: self.x + other.x}
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
    }

    #[test]
    fn inline_trait_rejects_a_different_signature() {
        let (_typed, d) = analyze_src(
            r#"
+Widget = trait {
    +paint(self) : str
    +nudge(@self, by: i64) : i64
}
+Badge: Widget = {
    label: str = ""
    +paint(@self) : i64 => 1
    +nudge(self, by: str) : str => by
}
"#,
        );
        assert!(d.has_errors());
        let messages: Vec<_> = d.into_vec().into_iter().map(|diag| diag.message).collect();
        let joined = messages.join("\n");
        assert!(joined.contains("requires `self`"), "{messages:?}");
        assert!(joined.contains("requires `str`"), "{messages:?}");
        assert!(joined.contains("requires `i64`"), "{messages:?}");
    }

    #[test]
    fn layouts_struct_fields() {
        let (typed, d) = analyze_src(
            r#"
+User = {
    id: i64
    name: str
    balance: f64 = 0.0
}
"#,
        );
        assert!(!d.has_errors());
        assert!(!typed.structs.is_empty());
    }

    #[test]
    fn http_requires_std_import() {
        let (_typed, d) = analyze_src(
            r#"
main() {
    x = http.ok("hi")
}
"#,
        );
        assert!(d.has_errors());
        let msg = format!("{}", d.iter().next().unwrap());
        assert!(
            msg.contains("use std.net.http"),
            "expected import hint, got: {msg}"
        );
    }

    #[test]
    fn type_methods_declare() {
        let (typed, d) = analyze_src(
            r#"
+Enemy = {
    hp: i64 = 3
    damage(@self, amount: i64) : i64 {
        self.hp = self.hp - amount
        return self.hp
    }
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
        assert!(!typed.functions.is_empty());
    }

    #[test]
    fn type_methods_static_and_instance_typecheck() {
        let (_typed, d) = analyze_src(
            r#"
+App = {
    name: str = ""
    n: i64 = 0
    +new(name: str, x: i64) : App => App.{name, n: x}
    value(self) : i64 => self.n
    bump(@self, d: i64) : i64 {
        self.n = self.n + d
        self.n
    }
}
main() {
    a = App.new("api", 10)
    _g = a.value()
    _b = @a.bump(1)
    _c = App.bump(a, 2)
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
    }

    #[test]
    fn missing_at_diagnostic_has_code() {
        let (_typed, d) = analyze_src(
            r#"
+User = {
    balance: f64 = 0.0
    discount(@self, percent: f64) : f64 {
        self.balance + percent
    }
}
main() {
    u = User.{}
    u.discount(10.0)
}
"#,
        );
        assert!(d.has_errors());
        let msg = format!("{}", d.iter().next().unwrap());
        assert!(msg.contains("E0301"), "got: {msg}");
        assert!(msg.contains("cannot mutate without `@`"), "got: {msg}");
    }

    #[test]
    fn private_field_diagnostic_is_friendly() {
        let (_typed, d) = analyze_src(
            r#"
+User = { -secret: str = "x" }
main() {
    u = User.{}
    x = u.secret
}
"#,
        );
        assert!(d.has_errors());
        let msg = format!("{}", d.iter().next().unwrap());
        assert!(msg.contains("E0104"), "got: {msg}");
        assert!(msg.contains("file-private"), "got: {msg}");
    }

    #[test]
    fn e0202_rejects_str_passed_as_i64() {
        let (_typed, d) = analyze_src(
            r#"
take(n: i64) : i64 => n
main() {
    take("nope")
}
"#,
        );
        assert!(d.has_errors(), "str→i64 must be a hard type error");
        let msgs: Vec<String> = d.iter().map(|e| format!("{e}")).collect();
        assert!(
            msgs.iter().any(|m| m.contains("E0202")),
            "expected E0202, got: {msgs:?}"
        );
    }

    #[test]
    fn e0202_rejects_optional_without_or() {
        let (_typed, d) = analyze_src(
            r#"
find() : i64? => none
main() {
    x: i64 = find()
}
"#,
        );
        assert!(d.has_errors(), "T? assigned to T must error");
        let msgs: Vec<String> = d.iter().map(|e| format!("{e}")).collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("E0202") && m.contains("optional")),
            "expected optional E0202, got: {msgs:?}"
        );
    }

    #[test]
    fn e0202_rejects_wrong_struct_field_type() {
        let (_typed, d) = analyze_src(
            r#"
+Pt = { x: i64 = 0, y: i64 = 0 }
main() {
    p: Pt = Pt.{x: "bad", y: 1}
}
"#,
        );
        assert!(d.has_errors(), "str field into i64 must error");
        let msgs: Vec<String> = d.iter().map(|e| format!("{e}")).collect();
        assert!(
            msgs.iter().any(|m| m.contains("E0202")),
            "expected E0202, got: {msgs:?}"
        );
    }

    #[test]
    fn unused_local_warns_in_dev() {
        let (_typed, d) = analyze_src(
            r#"
main() {
    x = 1
}
"#,
        );
        let messages: Vec<_> = d.iter().map(ToString::to_string).collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("E0303") && message.contains("unused variable 'x'")),
            "expected unused-local diagnostic, got: {messages:?}"
        );
        assert!(!d.has_errors(), "dev unused should be warning-only");
    }

    #[test]
    fn unused_local_errors_in_release() {
        let (_typed, d) = analyze_src_opts(
            r#"
main() {
    x = 1
}
"#,
            true,
        );
        assert!(d.has_errors(), "release unused must be a hard error");
    }

    #[test]
    fn unused_local_fails() {
        let (_typed, d) = analyze_src_opts(
            r#"
main() {
    x = 1
}
"#,
            true,
        );
        let messages: Vec<_> = d.iter().map(ToString::to_string).collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("E0303") && message.contains("unused variable 'x'")),
            "expected unused-local diagnostic, got: {messages:?}"
        );
    }

    fn assert_e0402(src: &str, label: &str) {
        let (_typed, d) = analyze_src(src);
        let messages: Vec<_> = d.iter().map(ToString::to_string).collect();
        assert!(
            messages.iter().any(|m| m.contains("E0402")),
            "{label}: expected E0402, got: {messages:?}"
        );
    }

    fn assert_no_e0402(src: &str, label: &str) {
        let (_typed, d) = analyze_src(src);
        let messages: Vec<_> = d.iter().map(ToString::to_string).collect();
        assert!(
            !messages.iter().any(|m| m.contains("E0402")),
            "{label}: unexpected E0402: {messages:?}"
        );
    }

    #[test]
    fn slice_return_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2, 3]
    return xs[0..1]
}
"#,
            "return array slice",
        );
    }

    #[test]
    fn str_slice_return_ok() {
        assert_no_e0402(
            r#"
+take(s: str) : str {
    return s[0..1]
}
"#,
            "owned str slice",
        );
    }

    #[test]
    fn owned_str_return_ok() {
        assert_no_e0402(
            r#"
+take(s: str) : str {
    return s
}
"#,
            "owned str return",
        );
    }

    #[test]
    fn array_slice_tail_expr_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2, 3]
    xs[0..1]
}
"#,
            "tail array slice",
        );
    }

    #[test]
    fn array_index_nested_return_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [[1, 2], [3, 4]]
    return xs[0]
}
"#,
            "nested array index",
        );
    }

    #[test]
    fn record_store_slice_is_e0402() {
        assert_e0402(
            r#"
+Wrap = { data: _ }
+take() {
    xs = [1, 2, 3]
    Wrap.{data: xs[0..2]}
}
"#,
            "record store of slice",
        );
    }

    #[test]
    fn spawn_slice_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2, 3]
    spawn { xs[0..1] }
}
"#,
            "spawn capturing slice",
        );
    }

    #[test]
    fn send_slice_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2, 3]
    send(xs[0..1])
}
"#,
            "channel send of slice",
        );
    }

    #[test]
    fn open_end_slice_return_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2, 3, 4]
    n = 4
    return xs[2..n]
}
"#,
            "open-end array slice return",
        );
    }

    #[test]
    fn anonymous_record_slice_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [10, 20, 30]
    {view: xs[0..2]}
}
"#,
            "anonymous record store of slice",
        );
    }

    #[test]
    fn mid_return_slice_is_e0402() {
        assert_e0402(
            r#"
+take(flag: bool) {
    xs = [1, 2, 3]
    if flag {
        return xs[0..1]
    }
    0
}
"#,
            "mid-block return slice",
        );
    }

    #[test]
    fn inferred_struct_slice_is_e0402() {
        assert_e0402(
            r#"
+take() {
    xs = [1, 2]
    .{data: xs[0..1]}
}
"#,
            "inferred struct store",
        );
    }

    #[test]
    fn owned_array_literal_return_ok() {
        assert_no_e0402(
            r#"
+take() {
    return [1, 2, 3]
}
"#,
            "owned array literal",
        );
    }

    #[test]
    fn scalar_index_return_ok() {
        assert_no_e0402(
            r#"
+take() : i64 {
    xs = [1, 2, 3]
    return xs[0]
}
"#,
            "scalar i64 index copies",
        );
    }

    #[test]
    fn local_slice_binding_ok() {
        assert_no_e0402(
            r#"
+take() : i64 {
    xs = [1, 2, 3]
    view = xs[0..2]
    view[0]
}
"#,
            "same-scope slice binding",
        );
    }

    #[test]
    fn str_field_return_ok() {
        assert_no_e0402(
            r#"
+Box = { s: str }
+take(b: Box) : str {
    return b.s
}
"#,
            "str field is owned fat ptr",
        );
    }

    #[test]
    fn underscore_local_silences_unused() {
        let (_typed, d) = analyze_src(
            r#"
main() {
    _x = 1
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
    }

    #[test]
    fn unused_parameter_fails() {
        let (_typed, d) = analyze_src_opts("+constant(value: i64) : i64 => 1", true);
        let messages: Vec<_> = d.iter().map(ToString::to_string).collect();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("E0304")
                    && message.contains("unused parameter 'value'")),
            "expected unused-parameter diagnostic, got: {messages:?}"
        );
    }

    #[test]
    fn used_variable_passes() {
        let (_typed, d) = analyze_src(
            r#"
main() : i64 {
    x = 1
    return x
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
    }

    #[test]
    fn symbol_index_covers_declarations_and_reads() {
        let (typed, d) = analyze_src(
            r#"
#Limit = 4
compute(@seed: i64) : i64 {
    local = seed + Limit
    return local
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
        assert!(
            typed
                .symbols
                .iter()
                .any(|symbol| symbol.kind == SymbolKind::Const
                    && symbol.storage == StorageClass::ConstExpr)
        );
        assert!(typed.symbols.iter().any(|symbol| {
            symbol.kind == SymbolKind::Param
                && symbol.storage == StorageClass::Param
                && symbol.mutable
        }));
        assert!(
            typed
                .symbols
                .iter()
                .any(|symbol| symbol.kind == SymbolKind::Local
                    && symbol.storage == StorageClass::Register)
        );
        assert!(
            typed.symbol_references.len() >= 3,
            "expected indexed reads, got {:?}",
            typed.symbol_references
        );
    }

    #[test]
    fn loop_and_closure_reads_count_as_uses() {
        let (_typed, d) = analyze_src(
            r#"
count_up(n: i64) : i64 {
    count = 0
    for 0..n {
        count += 1
    }
    count
}
capture(n: i64) : i64 {
    value = n
    task = spawn value
    await task
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.iter().collect::<Vec<_>>());
    }

    #[test]
    fn ufcs_chain_types_follow_free_function_returns() {
        // Each stage's declared return type becomes the next receiver's type.
        let (_t, d) = analyze_src(
            r#"
half(x: f64) : f64 => x / 2.0
byte_len(s: str) : i64 => s.len
label(_n: i64) : str => "n"
main() {
    a: f64 = 8.0.half().half()
    n: i64 = "Hi".byte_len()
    s: str = "Hi".byte_len().label()
    println("{a} {n} {s}")
}
"#,
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
    }

    #[test]
    fn ufcs_chain_expression_types_are_recorded() {
        let src = r#"
half(x: f64) : f64 => x / 2.0
byte_len(s: str) : i64 => s.len
label(_n: i64) : str => "n"
fa() : f64 {
    x = 8.0.half().half()
    x
}
fb() : i64 {
    x = "Hi".byte_len()
    x
}
fc() : str {
    x = "Hi".byte_len().label()
    x
}
main() {
    println("{fa()} {fb()} {fc()}")
}
"#;
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let module = Parser::new(&tokens, src, &mut interner, &mut diags).parse_module();
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let body_ty = |name: &str| {
            let f = module
                .items
                .iter()
                .find_map(|i| match i {
                    Item::Function(f) if interner.resolve(f.name) == name => Some(f),
                    _ => None,
                })
                .expect("function");
            let FnBody::Block(b) = &f.body else {
                panic!("block body")
            };
            let Stmt::Binding(bind) = &b.stmts[0] else {
                panic!("binding stmt")
            };
            let tid = typed.expr_types[&span_key(bind.value.span())];
            typed.types.get_type(tid).clone()
        };
        assert!(matches!(body_ty("fa"), TypeKind::F64));
        assert!(matches!(body_ty("fb"), TypeKind::I64));
        assert!(matches!(body_ty("fc"), TypeKind::Str));
    }

    #[test]
    fn a_declared_len_method_keeps_its_own_return_type() {
        let src = r#"
V = {
    x: f64
    len(self) : f64 => self.x
}
fa(v: V) : f64 {
    n = v.len()
    n
}
fb(s: str) : i64 {
    n = s.len()
    n
}
main() {
    println(fa(V{x: 1.5}))
    println(fb("ab"))
}
"#;
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(src, &mut interner, &mut diags).tokenize();
        let module = Parser::new(&tokens, src, &mut interner, &mut diags).parse_module();
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let first_binding_ty = |name: &str| {
            let f = module
                .items
                .iter()
                .find_map(|i| match i {
                    Item::Function(f) if interner.resolve(f.name) == name => Some(f),
                    _ => None,
                })
                .expect("function");
            let FnBody::Block(b) = &f.body else {
                panic!("block body")
            };
            let Stmt::Binding(bind) = &b.stmts[0] else {
                panic!("binding stmt")
            };
            typed
                .types
                .get_type(typed.expr_types[&span_key(bind.value.span())])
                .clone()
        };
        assert!(matches!(first_binding_ty("fa"), TypeKind::F64));
    }

    #[test]
    fn println_is_in_the_language_prelude() {
        let (_t, d) =
            analyze_src("main() {\n    println(\"hi\")\n    println()\n    \"x\".println()\n}\n");
        assert!(!d.has_errors(), "{:?}", d.into_vec());
    }

    const KIND_SUM: &str = "Kind = {\n    Var\n    Con\n    Arrow\n}\n";

    fn tuple_when(arms: &str) -> Diagnostics {
        let src = format!(
            "{KIND_SUM}pick(a: i64, b: i64) : i64 {{\n    when (a, b) {{\n{arms}        else => {{ return 0 }}\n    }}\n}}\n"
        );
        analyze_src(&src).1
    }

    fn has_e0205(d: Diagnostics) -> bool {
        d.into_vec()
            .iter()
            .any(|x| x.message.contains("neither dominates"))
    }

    #[test]
    fn mirrored_wildcard_arms_are_fine_when_an_earlier_arm_owns_the_overlap() {
        let d = tuple_when(
            "        (Kind.Var, Kind.Var) => { return 1 }\n        (Kind.Var, _) => { return 2 }\n        (_, Kind.Var) => { return 3 }\n",
        );
        assert!(!has_e0205(d));
    }

    #[test]
    fn mirrored_wildcard_arms_without_an_owner_stay_ambiguous() {
        let d = tuple_when(
            "        (Kind.Var, _) => { return 2 }\n        (_, Kind.Var) => { return 3 }\n",
        );
        assert!(has_e0205(d));
    }

    #[test]
    fn wildcard_arms_on_distinct_variants_never_meet() {
        let d = tuple_when(
            "        (Kind.Con, _) => { return 2 }\n        (_, Kind.Var) => { return 3 }\n",
        );
        assert!(has_e0205(d), "Con/_ and _/Var meet at (Con, Var)");
        let d = tuple_when(
            "        (Kind.Con, Kind.Arrow) => { return 1 }\n        (Kind.Var, _) => { return 2 }\n",
        );
        assert!(
            !has_e0205(d),
            "rank-incomparable but disjoint arms are not ambiguous"
        );
    }
}
