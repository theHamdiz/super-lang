# The Super Programming Language Specification

Language specification for Super 0.1.0 (Round 5 language revision)

## 1. Status, scope, and conformance

This document specifies the source language accepted by the Super 0.1.0
toolchain, including the Round 5 revision (block and assertion macros, the
§3.9 design rule, `super fmt`, and `super lint`). The product, crates, CLI,
installer, site, and release tag are 0.1.0; the Round 5 revision is not a
separate product version. Published benchmark measurements remain the frozen
0.1.0 measurement, taken on a single machine. This
document is normative where it uses “shall”, “must”, or grammar productions,
and explanatory where it describes implementation strategy. Super source files
use the `.sr` extension. Every source file is a module; there is no `mod`
declaration keyword. The reference implementation is the Rust workspace in
this repository.

A conforming implementation shall:

1. tokenize and parse the grammar in this document;
2. diagnose violations before execution where the static rules require it;
3. preserve observable evaluation order, integer/float results, mutation, and control flow across the VM, JIT, and C23 AOT paths;
4. implement module privacy, `@` mutation rules, optional/error handling, and root-lifetime checks;
5. accept `Pod.toml` Pods (with removed legacy `Super.toml`) and produce reproducible `Pod.lock` Git pins.

The implementation may optimize, fuse, specialize, vectorize, or erase constructs when the resulting behavior is unchanged. Tests are compilation units for `super test`; release builds may strip them.

## 2. Source text and lexical structure

### 2.1 Encoding, layout, and statement boundaries

Source text is UTF-8. The lexer recognizes ASCII letters in identifiers; non-ASCII text is valid inside strings and comments. Horizontal and vertical whitespace separate tokens. Newlines normally have no token, but a newline before binary `+` or `-` ends the preceding expression, preventing the next statement from being consumed as an infix continuation. Statements are separated by newlines. The semicolon appears only inside array-repeat literals (`[0; n]`); using `;` as a statement separator or terminator is a syntax error.

An identifier starts with `A`–`Z`, `a`–`z`, or `_`, followed by zero or more ASCII letters, digits, or `_`. Names are case-sensitive. By convention, types and sum variants use `UpperCamelCase`, exported constants may use capitals, and values use `snake_case`.

```ebnf
letter        = "A"…"Z" | "a"…"z" | "_" ;
digit         = "0"…"9" ;
hex-digit     = digit | "A"…"F" | "a"…"f" ;
oct-digit     = "0"…"7" ;
bin-digit     = "0" | "1" ;
identifier    = letter, { letter | digit } ;
```

### 2.2 Keywords

The reserved keywords in 0.1.0 are:

```text
after await break chan Code continue each else embed ensure err fail false
for if in loop none or panic par parallel return rewrite scope select shared
spawn state test trait true type use when while
```

The contextual keywords are `as`, `it`, `index`, `self`, and `_`. Each has
meaning in one grammatical position only (import renames; implicit loop and
closure names; method receivers; wildcards and inferred types) and is an
ordinary identifier everywhere else. No other identifier is special: names
such as `sum`, `len`, `routes`, `get`, `post`, `put`, and `delete` are ordinary
names resolved as values, functions, or methods.

The rule that governs this list is stated in §3.9.

### 2.3 Literals

Decimal integers and floats permit `_` separators. Integer literals also support hexadecimal `0x`, binary `0b`, and octal `0o` prefixes. Separators do not affect value. A leading `-` is folded into a numeric token when it cannot be an infix subtraction; otherwise it is unary negation.

```ebnf
dec-int       = [ "-" ], digit, { digit | "_" } ;
hex-int       = [ "-" ], "0", ("x" | "X"), hex-digit, { hex-digit | "_" } ;
bin-int       = [ "-" ], "0", ("b" | "B"), bin-digit, { bin-digit | "_" } ;
oct-int       = [ "-" ], "0", ("o" | "O"), oct-digit, { oct-digit | "_" } ;
exponent      = ("e" | "E"), [ "+" | "-" ], digit, { digit | "_" } ;
float         = [ "-" ], digit, { digit | "_" },
                (".", digit, { digit | "_" }, [ exponent ] | exponent) ;
integer       = dec-int | hex-int | bin-int | oct-int ;
boolean       = "true" | "false" ;
none-literal  = "none" ;
string        = '"', { escaped-char | interpolation | string-char }, '"' | raw-string ;
interpolation = "{", expression, "}" ;
```

Examples:

```super
mask = 0xff_00_ff
limit = 1_000_000
ratio = 6.022e23
message = "entity {id} at {pos.x}"
```

Strings are immutable language values. Interpolation is retained in the string token and lowered by the compiler. Each hole is rendered with `Display`: arrays, maps, records, structs, and commands have a witness, and a type that declares `Display` or `ToString` replaces it with a direct call. A hole that is not an expression (`{}`, `{1, 2}`, `\{`) stays literal text. Raw strings do not interpolate.

#### 2.3.1 Escape sequences

Inside an ordinary string literal a backslash begins an escape sequence, which the lexer validates and the compiler decodes. An unrecognized or malformed escape is a syntax error (E0001).

```ebnf
escaped-char  = "\", ( "n" | "r" | "t" | "0" | "\" | '"' | "'" | "{" | "}"
                     | "x", hex-digit, hex-digit
                     | "u", "{", hex-digit, { hex-digit }, "}" ) ;
```

| Escape | Value |
| --- | --- |
| `\n`, `\r`, `\t` | U+000A, U+000D, U+0009 |
| `\0` | U+0000 |
| `\\`, `\"`, `\'` | backslash, double quote, single quote |
| `\{`, `\}` | literal braces; they never start an interpolation |
| `\xNN` | the code point `NN`, which must be at most `7F` |
| `\u{H…}` | a Unicode scalar value written with 1–6 hex digits |

#### 2.3.2 Raw strings

A raw string literal is `r`, zero or more `#`, `"`, a body, `"`, and the same number of `#`. The body ends at the first `"` followed by that many `#`. It contains no escapes and no interpolation (`\` , `{`, and `}` are ordinary bytes), may span lines, and has type `str` like any other string. `r` not followed by `#*"` is an ordinary identifier.

```ebnf
raw-string    = "r", { "#" }, '"', { any-char }, '"', { "#" } ;   (* hash counts equal *)
```

```super
path    = r"C:\Users\admin\notes.txt"
pattern = r"^\d+\.\d+$"
quoted  = r#"say "hi" \d+"#
```

Lexically a raw string is stored as the equivalent escaped string (`\` → `\\`, `"` → `\"`, `{` → `\{`, `}` → `\}`), so every later phase treats it as an ordinary string whose decoded value equals the source bytes.

### 2.4 Comments and documentation

`//` begins an inert line comment. `/* … */` is an inert block comment. Block comments are not nested.

Documentation comments are tokens, not trivia. The parser attaches them to the next declaration (module item, field, method, variant, or local binding) and `super doc` renders that text as Markdown. The language server shows the same text on hover, completion, signature help, and outline detail. Editors highlight the forms below as documentation rather than as ordinary comments.

| Form | Attached to |
|---|---|
| `/// text` | the next declaration |
| `/** text */` | the next declaration |
| `//! text` | the enclosing module, or the enclosing type when written inside its body |
| `/*! text */` | the enclosing module, or the enclosing type when written inside its body |

`////` and `/***` stay ordinary comments. A documentation comment that is not followed by a declaration is a syntax error.

Two legacy executable meta-comment forms are tokens rather than trivia: `//# directive` and `/*# directive #*/`. First-class `#inline`, `#simd`, and related attributes are preferred.

```super
/// Adds two values.
+add(a: i64, b: i64) => a + b

//# unroll(4)
for values { consume(it) }
```

### 2.5 Operators and punctuation

The lexical operators are:

```text
+  -  *  /  %  =  ==  !=  <  <=  >  >=
+= -= *= /=  .  ..  ,  ;  :  ?  !  @  #
$  '{  ${  =>  |  &  ^  ~  &&  ||  <<  >>  >>>
( ) { } [ ]
```

`+` has two declaration roles in addition to addition: at module scope it exports the following declaration, and in a public struct redundant `+field` explicitly marks a public field. `-field` marks a private struct field. `@` creates or forwards an in-place mutable borrow and is also required at mutation-capable parameters and selected loop targets. `#` introduces compile-time declarations, references, blocks, conditions, loops, and recognized attributes.

Expression precedence, from strongest to weakest, is postfix call/field/index, unary (`-`, `!`, `~`, `@`, `await`, `spawn`, `shared`), multiplicative, additive, shifts, comparisons/equality, bitwise, logical `&&`, logical `||`, range `..`, ternary `?:`, assignment, then `or`. Assignments associate right-to-left; ordinary binary operators associate left-to-right.

## 3. Complete syntactic grammar

The EBNF is descriptive of the accepted 0.1.0 surface. `NL` denotes a source newline where statement separation is needed. Braced bodies remove most newline ambiguity.

### 3.1 Modules and items

```ebnf
module          = { item } ;
item            = { meta-attribute },
                  ( use-item | test-item | state-item | route-item
                  | rewrite-item | comptime-item | declaration ) ;

visibility      = [ "+" ] ;
declaration     = visibility,
                  ( function-def | type-def | binding | multi-binding )
                | multi-binding ;

use-item        = "use", module-path, [ ".", "{", use-name,
                  { ",", use-name }, "}" ], [ "as", identifier ] ;
use-name        = identifier, [ "as", identifier ] ;
module-path     = identifier, { ".", identifier } ;

binding         = identifier, [ ":", type-expr ], "=", expression ;
multi-binding   = multi-bind-target, { ",", multi-bind-target }, "=",
                  expression, { ",", expression } ;
multi-bind-target = [ "+" ], identifier, [ ":", type-expr ] ;
comptime-item   = "#", ( binding | comptime-if | comptime-for
                  | comptime-block ) ;

state-item      = "state", identifier, "=", expression ;
test-item       = "test", string, block ;

route-item      = route-method, string, "|", identifier, "|",
                  [ ":", type-expr ], block ;
route-method    = "get" | "post" | "put" | "delete" ;

rewrite-item    = "rewrite", string, "(", [ parameter-list ], ")",
                  "{", expression, "=>", expression, "}" ;
```

Every `.sr` file is a module. Items are private unless prefixed by `+`. `use auth` resolves a sibling `auth.sr`; `use models.{User, Task}` imports selected exports; `use std.net.http` resolves an embedded or installed standard-library module (canonical manifest path `layer.module`; short aliases like `use std.http` still resolve but are deprecated in new code). **Braced multi-import:** when every name in `use std.data.{column, frame, plan}` resolves as a *loadable sibling module* under the path prefix, the brace expands to the same bindings as separate `use std.data.column` / `use std.data.frame` / `use std.data.plan` lines (LoC-friendly). If no selector is a loadable module, braces keep export-select semantics (`use models.{User}`, `use std.poll.{POLL_READ}`). A mix of loadable modules and unknown names is an error (`braced multi-import`). `use std.gui as App` binds that module under the local name `App`, so `App.Gui.new(...)` and `App.byte_len(...)` resolve through the alias. `use std.gui.{Gui as Desk}` adds `Desk` as another name for the exported type or function; `Desk.new(...)` calls `Gui.new`. `as` is contextual and is not a reserved word. The original export stays available. An unused import is still `E0305`, and using either the alias or an imported name counts as a use. `state` is module-root mutable storage intended to survive hot reload where the selected runtime supports persistence.

### 3.2 Types, structs, and sums

```ebnf
type-expr       = primary-type, { "?" | "!" } ;
primary-type    = module-path | "type" | "Code", "[", type-expr, "]"
                | "chan", "[", type-expr, "]" ;

type-def        = identifier, [ ":", identifier, { ",", identifier } ],
                  "=", "{", ( struct-body | sum-body ), "}" ;
struct-body     = { struct-member, [ "," ] } ;
struct-member   = field-def | method-def ;
field-def       = [ "+" | "-" ], identifier, ":", type-expr,
                  [ "=", expression ] ;

sum-body        = variant-def, { [ "," ], variant-def } ;
variant-def     = identifier,
                  [ "(", identifier, ":", type-expr,
                    { ",", identifier, ":", type-expr }, ")" ] ;

method-def      = [ "+" | "-" ], function-def ;
```

The built-in scalar names are `bool`, `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64`, and `str`. Unannotated integer literals default to `i64`; unannotated floats default to `f64`. Fixed-width integers coerce among themselves and with `i64` at assignment and call boundaries (numeric widen/narrow). Overflow of signed `i64` arithmetic wraps with two's-complement semantics (`-fwrapv` / VM wrap) in all profiles; division by zero is a runtime error. Explicit wrapping, checked, and saturating helpers for narrower widths live in `std.core.int`. C23 mapping: `u8`→`uint8_t`, `u16`→`uint16_t`, `u32`→`uint32_t`, `u64`→`uint64_t`, `i8`→`int8_t`, `i16`→`int16_t`, `i32`→`int32_t`, `i64`→`int64_t`, `i128`→`__int128`. `_` in a parameter type position requests inference/specialization. A named struct uses flat C-compatible field layout. In an exported struct, fields and methods are public by default; prefix a member with `-` to keep it file-private (this module and `test` blocks). A leading `+` on a field or method is redundant and ignored — only top-level declarations use `+` to export. Sum variants have a discriminant and an inline payload sized to the largest variant.

```super
+User = {
    id: i64
    name: str
    balance: f64 = 0.0
    -secret: str = ""

    credit(@self, amount: f64) {
        self.balance += amount
    }

    -recompute(@self) {
        // file-private helper
    }
}

Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)
}
```

#### Traits, declared inline

```ebnf
trait-def       = identifier, "=", "trait", "{", { trait-method }, "}" ;
trait-method    = [ "+" | "-" ], identifier, "(", [ parameter-list ], ")",
                  [ ":", type-expr ] ;
conforming-type = identifier, ":", identifier, { ",", identifier }, "=",
                  "{", struct-body, "}" ;
```

A trait is a method contract. It has signatures and no bodies. A struct declares the traits it satisfies in the type header, in the same place a binding declares its type: `+Toggle: Widget = { … }`. Several traits are a comma list: `+Sprite: Widget, Anim = { … }`. The methods are written once, inline, on that type. A sum type does not declare traits.

Conformance is static and nominal. For each declared trait the struct must provide every method. A match requires the same parameter count (the receiver counts), the same `@` on each parameter, the same parameter types after the receiver, and the same return type when the trait names one. The receiver's type is the owner on each side and is not compared. A trait parameter written `_`, or left untyped, accepts any type on the struct. A header that names an unknown trait, omits a method, or disagrees on that signature is `E0203`.

There is no vtable and no separate `impl` block. A call stays a direct call to the method written on the type when that type declares the trait. Otherwise a language trait uses the builtin witness. `Display` renders arrays as `[a, b]`, maps and anonymous records as `{k: v}` with sorted keys, and named structs as `Name{k: v}`. `Eq`, `Ord`, `Hash`, `Add`, and the other core traits follow the same split. The VM, the fiber interpreter, and the C23 backend implement one algorithm. A method whose signature matches a language trait, but whose type does not declare it, is `E0204`. A definition of `Display`, `Eq`, or any other name in that table that drifts from `std.core.traits` is also `E0204`. Missing or mismatched methods on a trait the type did declare stay `E0203`.

A widget tree adopts a custom value by calling `paint` and mounting the display list with `custom`. `std.gui.widgets` defines `Widget` (`paint`, `hit`, `measure`) and the controls that declare it. `std.game.fx` sprites declare the same trait. Import the module that defines the trait; `use std.gui` also sees `Widget` because `std.gui` imports `std.gui.widgets`. The language traits are ambient: `: Display` is valid without importing `std.core.traits`.

```super
+Widget = trait {
    paint(self) : str
    hit(self, x: i64, y: i64) : bool
    measure(self, max_w: i64) : i64
}

+Badge: Widget = {
    label: str = ""
    w: i64 = 72
    paint(self) : str  => "text 0 0 230 230 236 |{self.label}\n"
    hit(self, x: i64, y: i64) : bool  => x >= 0 && y >= 0 && self.w > 0
    measure(self, max_w: i64) : i64 {
        w = self.w
        if max_w < w { return max_w }
        w
    }
}
```

### 3.3 Functions and calls

```ebnf
function-def    = identifier, "(", [ parameter-list ], ")",
                  [ ":", type-expr ], function-body ;
parameter-list  = parameter, { ",", parameter } ;
parameter       = [ "#" ], [ "@" ], identifier,
                  [ ":", type-expr ] ;
function-body   = "=", expression | "=>", expression | block ;

call            = postfix-expr, "(", [ argument-list ], ")" ;
argument-list   = argument, { ",", argument } ;
argument        = expression | "." ;
field-access    = postfix-expr, ".", identifier ;
ufcs-call       = postfix-expr, ".", identifier,
                  "(", [ argument-list ], ")" ;
```

`=>` and `=` both define expression-bodied functions; `=>` additionally communicates an inline preference to the backend. An omitted parameter type is inferred and monomorphized. A `#` parameter is a compile-time parameter. `@` marks a mutable, non-escaping parameter.

Uniform Function Call Syntax rewrites `receiver.method(args…)` to the applicable method or free function with `receiver` as the first argument. The receiver form and the nested-call form are **first-class and equivalent**: `recv.f(a, b)` ≡ `f(recv, a, b)`, for a receiver of any type (numeric, `str`, array, struct, optional, …). UFCS is left-associative and chainable: in `a.f().g(x).h()` each stage's result is the next stage's receiver, so the chain denotes `h(g(f(a), x))`.

Resolution of `recv.f(args)`: (1) if `recv` is a struct, a method `f` declared on that struct; (2) a free function `f` of arity `1 + len(args)` whose first parameter accepts `recv` (an exactly matching parameter type is preferred over an inferred `_` parameter); (3) a language builtin (`print`, `println`, `assert_eq`, `abs`). The chain's static type is the declared return type of each resolved function. `keep`, `map`, `sum`, `group_by` and channel verbs are lowered by the compiler and are not resolved as free functions.

#### Method families (`FuncFamily`) and trait-constrained parameters

Multiple top-level (and `+`-exported) functions may share an identifier when their parameter lists differ in arity, concrete types, `#` compile-time parameters, trait constraints, or `@` mutability. UFCS resolves against the unified `FuncFamily` in scope. A parameter written `x: TraitName` (where `TraitName` names a `trait`) is a statically monomorphized generic constrained to types declaring `: TraitName` (`E0203` on missing conformance).

Specificity ranking per parameter position: **Concrete** (4) > **narrowed sum variant** (3) > **trait constraint** (2) > **unconstrained `_`** (1). Candidate $A$ dominates $B$ iff every rank of $A$ is ≥ the corresponding rank of $B$ and at least one is strictly greater. Residual ties emit `E0205` (ambiguous dispatch) at the call site; identical canonical signatures in one module emit `E0206` (duplicate overload).

`#symmetric` on a 2-parameter function with distinct parameter types or `@` flags synthesizes the swapped mirror `f(b, a) => f(a, b)` with matching visibility. Invalid `#symmetric` (arity ≠ 2 or identical types/mutability) is `E0501`. C23 AOT mangles specialized family members as `mod__fn__T1__T2` (param tags) while preserving `restrict` on `@` parameters.

```super
print("{49.0.sqrt().floor().to_int()}")   // UFCS chain
print("{to_int(floor(sqrt(49.0)))}")      // identical nested calls
n = "Hi".byte_len()                         // str receiver
m = [1, 2, 3].arr_push(4).len()         // array receiver, two stages
``` `@receiver.method(…)` forwards a mutable borrow. A lone `.` argument requests call punning: remaining arguments are filled from same-named locals.

```super
+add_tax(amount: f64) => amount * 1.14
final = @user.discount(10.0).add_tax()
spawn_wave(.) // fills remaining named parameters from scope
```

#### Output builtins

`print(x)` and `println(x)` are always in scope. Both write the display form of `x` followed by a single newline; `println` is the explicit name and is an alias of `print`. With no argument either writes a bare newline. Both are usable in receiver form (`x.println()`). A user-defined free function `println` takes precedence over the builtin. Output without a trailing newline, stderr, and binary-safe writes are provided by `std.io.stdio`.

### 3.4 Statements and control flow

```ebnf
block           = "{", { statement }, "}" ;
statement       = { meta-attribute },
                  ( binding | destructure | expression
                  | return-stmt | fail-stmt | panic-stmt | ensure-stmt
                  | when-stmt | select-stmt | loop-stmt | sum-into-stmt
                  | scope-stmt | comptime-stmt | multi-assign ),
                  [ guard ] ;

return-stmt     = "return", [ expression ] ;
fail-stmt       = "fail", expression ;
panic-stmt      = "panic", [ expression ] ;
guard           = "if", expression ;
ensure-stmt     = "ensure", expression, "else", block ;

loop-stmt       = for-stmt | par-stmt
                | "while", expression, block
                | "loop", block | "break" | "continue" ;

for-stmt        = "for", [ "parallel" ], [ "@" ],
                  ( ecs-binders, "in", expression
                  | [ binder, "in" ], expression ),
                  [ reduction-marker ], block ;
par-stmt        = "par", [ "@" ], [ binder, "in" ], expression,
                  [ reduction-marker ], block ;
binder          = identifier, [ ":", identifier ] ;
ecs-binders     = "(", binder, ")", { ",", [ "@" ], "(", binder, ")" } ;
reduction-marker = "(", "+", ")" ;

sum-into-stmt   = "sum", identifier, sum-clause ;
sum-clause      = ("for" | "par"),
                  [ binder, "in" ], expression, [ "if", expression ] ;

scope-stmt      = "scope", block ;
destructure     = "{", destruct-field, { ",", destruct-field }, "}",
                  "=", expression ;
destruct-field  = identifier, [ ":", identifier ] ;
multi-assign    = assign-target, { ",", assign-target }, "=",
                  expression, { ",", expression } ;
assign-target   = identifier | postfix-expr ;
```

`for` installs implicit `it` and `index`. `it` is the current element or range value; `index` is the zero-based iteration index. `for @array` permits in-place element mutation. `par` is the preferred parallel form; `for parallel` remains accepted. The `(+ )` reduction marker is represented without the space as `(+)`.

Multi-assignment evaluates every right-hand side before any left-hand store. A bare unknown identifier on the left introduces a local. At module scope, a multi-binding declares several bindings in one line; each name may carry its own `+` export (unprefixed names stay private), and optional `: T` annotations are allowed per target. Postfix guards are allowed on `break`, `continue`, `return`, `fail`, assignment, and binding forms:

```super
+tasks, +next_id, name = [], 1, "Ahmad"
return 0 if n == 0
a, b = b, a
for i in 0..n { continue if blocked[i] }
```

### 3.5 `when`, conditionals, and matching

```ebnf
when-stmt       = "when", [ when-subject ], "{", { when-arm }, "}" ;
select-stmt     = "select", "{", { when-arm }, "}" ;
when-subject    = expression
                | "(", expression, ",", expression, { ",", expression }, ")" ;
when-arm        = ("else" | "after", expression
                | "(", when-pat, ",", when-pat, { ",", when-pat }, ")"
                | expression),
                  "=>", expression ;
when-pat        = "_" | expression | variant-bind ;
if-expr         = "if", expression, block, [ "else", block ] ;
ternary-expr    = expression, "?", expression, ":", expression ;
```

With a subject, `when subject` binds the subject to implicit `it` in arm expressions. Arms are considered in source order; `else` is the fallback. Channel / structured-concurrency multiplexing uses **`select { … }`** (A3) and may contain channel/receive expressions and `after duration` timeout arms. Subject-less `when { … }` remains accepted as a legacy alias for `select` and lowers to the same AST.

Multi-subject `when (e1, e2, …)` (arity ≥ 2) pairs with tuple arms `(P1, P2, …) => expr`:

- **Mode 1 — static type/trait tables:** subjects typed `_` or as traits desugar to a ranked `FuncFamily` of monomorphized specializations; open subjects require an `else` arm (`E0201`). Ambiguous incomparable arms emit `E0205`.
- **Mode 2 — closed sum-tag matrices:** subjects that are sum types lower to packed tag-index dispatch (`((uint64_t)a.tag << 32) | (uint64_t)b.tag`) with O(1) VM / JIT / C23 AOT jumps and zero heap allocation. Cartesian exhaustiveness is required unless `else` is present.

```super
when score {
    it >= 1000 => print("gold")
    it >= 100 => print("silver")
    else => print("bronze")
}

select {
    events.recv() => handle(it)
    after 16.ms => tick()
}
```

Named variants use `Sum.Variant` (unit) or `Sum.Variant(args…)` (payload). Construction, equality, and `when` tag matching are fully supported at compile time and run time. Payload destructuring binders are allowed: `when s { Status.Paid { amount } => … }` / `{ amount: a }`. Methods may be declared in the sum body after variants (same UFCS rules as structs); a `when` used as the method’s last expression returns the matched arm’s value. A `when` on a sum subject that omits variants and has no `else` is an error. Unit variants are tiny tagged values and coerce to their declaration-order `i64` discriminant when passed to integer parameters or compared with integers (HTTP `Mw` / route `Handler` ids). Stdlib middleware is the `Mw` sum (`Mw.Logger`, `Mw.Bearer(token)`, …) with default `before` / `after` methods; apps override by `App.replace` / `App.remove` (slot replace, no callbacks).

### 3.6 Expressions and collections

```ebnf
expression      = assignment-expr ;
assignment-expr = conditional-expr,
                  [ assign-op, assignment-expr ] ;
assign-op       = "=" | "+=" | "-=" | "*=" | "/=" ;

conditional-expr = logical-expr,
                   [ "?", expression, ":", conditional-expr ] ;
logical-expr    = range-expr, { ("&&" | "||"), range-expr } ;
range-expr      = binary-expr, [ "..", binary-expr ] ;
binary-expr     = unary-expr, { binary-op, unary-expr } ;
binary-op       = "+" | "-" | "*" | "/" | "%"
                | "==" | "!=" | "<" | "<=" | ">" | ">="
                | "&" | "|" | "^" | "<<" | ">>" | ">>>" ;

unary-expr      = [ "-" | "!" | "~" | "@" | "await" | "spawn" | "shared" ],
                  postfix-expr ;
postfix-expr    = primary-expr,
                  { call-suffix | field-suffix | index-suffix | slice-suffix } ;
call-suffix     = "(", [ argument-list ], ")" ;
field-suffix    = ".", identifier ;
index-suffix    = "[", expression, "]" ;
slice-suffix    = "[", expression, "..", expression, "]" ;

primary-expr    = identifier | integer | float | boolean | string | "none"
                | array | map-or-record | struct-literal | inferred-struct
                | if-expr | channel-expr | embed-expr
                | quote-expr | splice-expr | comptime-expr
                | "(", expression, ")" | block ;

array           = "[", [ expression,
                  ( ";", expression | { ",", expression } ) ], "]" ;
map-or-record   = "{", [ brace-entry, { ",", brace-entry } ], "}" ;
brace-entry     = identifier, [ ":", expression ]
                | string, ":", expression
                | "[", expression, "]", ":", expression ;
struct-literal  = module-path, [ "." ], "{",
                  [ record-field, { ",", record-field } ], "}" ;
inferred-struct = ".", "{", [ record-field, { ",", record-field } ], "}" ;
record-field    = identifier, [ ":", expression ] ;

channel-expr    = "chan", "[", type-expr, "]",
                  [ "(", expression, ")" ] ;
embed-expr      = "embed", string ;
```

`[value; N]` is a compile-time-sized repeat array. In the static model it is written `[T; N]`, where `T` is inferred from `value`; its storage is a dense scoped-stack allocation. Array literals are contiguous and zero-indexed. A brace with quoted or dynamic keys is a map. A brace with identifier fields is an anonymous record. Punning `{host, port}` means `{host: host, port: port}`.

Named `Type{…}` and `Type.{…}` forms construct that type. `.{…}` requires an expected struct type from a return type, parameter, typed binding, or other context.

### 3.7 Iterable reductions

Builtin arrays and ranges witness `Iterable` and `ParIterable` (`std.core.traits`). Reductions are UFCS pipeline methods — for example `xs.sum()`, `xs.map(…).sum()`, `xs.keep(…).sum()` — not keyword loops. The compiler fuses these chains without heap materialization. Custom associative folds use `reduce(reducer)` with `AssociativeReducer` witnesses from `std.core.reduce` (`identity`, `accumulate`, `merge`, `finalize`).

Element-wise math (`exp`, `sqrt`, …) lives in `std.math.core`, not as collection reductions. Order statistics (`median`, `quantile`) are slice-only via `std.math.stats` (introselect).

```super
total = values.sum()
energy = values.map(it * it).sum()
positive = values.keep(it > 0).sum()
weighted = (0..n).map(weights[it] * values[it]).sum()
checksum = rows.keep(it.valid).sum()
checksum = values.map(hash(it)).par_sum()
```

Parallel partition style (documented in `std.core.reduce`):

```super
partials = [0; workers]
par w in 0..workers { partials[w] = chunk(w).sum() }
total = partials.sum()
```

Pure reducers may lower to OpenMP chunking on the C23 path; the VM runs them sequentially with the same result.

### 3.8 Optionals and value errors

```ebnf
or-expr         = expression, "or",
                  ( expression | "err", block ) ;
optional-type   = type-expr, "?" ;
error-type      = type-expr, "!" ;
```

`T?` contains either a `T` or `none`. `T!` contains either `T` or an error value. Both must be discharged with `or`, propagated through a compatible return, or otherwise handled by a recognized control-flow form. In `left or fallback`, fallback is evaluated only if `left` has no value. In `left or err { … }`, implicit `err` is bound to the error payload inside the block.

```super
user = find_user(id) or .{id: 0, name: "Guest"}
port = read_port("Pod.toml") or err {
    log.error("configuration: {err}")
    return 3000
}
```

### 3.9 Keywords, macros, and compile-time metaprogramming

Keywords are the only bare syntax in Super. Every compile-time, user-extensible construct is a `#`-prefixed macro. Everything else is a function or a method. The compiler contains no identifier-name special cases; any construct not in the keyword list is resolved through ordinary name resolution: macro, function, or method.

A `#name` in source is therefore always one of: a pattern macro defined in
Super source, a compiler intrinsic in the `#` namespace, an item attribute, or
a compile-time binding (`#Name = expression`). None of these names is reserved
as a bare identifier, so a value named `routes`, `assert`, or `len` is an
ordinary binding.

#### 3.9.1 Pattern macros

```ebnf
macro-def     = [ "+" ], "#", identifier, "{", { macro-rule }, "}" ;
macro-rule    = "(", pattern, ")", "=>", "{", template, "}" ;
pattern       = { token | fragment | repetition } ;
fragment      = "$", identifier, ":", fragment-kind ;
fragment-kind = "ident" | "path" | "str" | "lit" | "operand" | "term"
              | "expr" | "block" | "closure" | "tt" | "type" ;
repetition    = "$(", pattern, ")", [ separator ], ( "*" | "+" | "?" ) ;
macro-call    = "#", identifier, ( "(", tokens, ")" | "{", tokens, "}" | ε ) ;
```

A module defines a macro with `#name { … }` and exports it with `+#name { … }`.
Exported macros travel with `use`, exactly like exported functions. The macros
in `std.core.macros` are in scope in every module without `use`.

A call `#name(…)`, `#name { … }`, or bare `#name` captures its input as
tokens. Rules are tried in order; the first rule whose pattern consumes the
entire input is expanded. Matching is greedy and does not backtrack. A call
that matches no rule is a compile error listing the accepted forms. Expansion
is recursive up to a depth of 64.

| Fragment | Matches |
|----------|---------|
| `ident` | one identifier |
| `path` | `a.b.c` |
| `str` | one string literal |
| `lit` | one literal (number, string, `true`, `false`, `none`) |
| `operand` | a path or a literal other than `none` |
| `term` | a unary/postfix expression without binary operators |
| `expr` | a full expression |
| `block` | `{ … }` |
| `closure` | `{ … }` or a `\|params\| body` lambda |
| `tt` | one token or one balanced `(…)`, `[…]`, `{…}` group |
| `type` | a type expression |

In a template, `$x` substitutes the captured fragment. A multi-token `expr` or
`term` is substituted as one parenthesized unit, so operator precedence of the
call site is preserved. `$( … ) sep* ` repeats once per captured iteration.
Any `$name` that the pattern does not bind is a hygienic local: each expansion
gets a fresh name that cannot collide with caller names or with another
expansion. Tokens copied from the call keep their source spans; tokens written
by the template carry the call-site span, so diagnostics in expanded code point
at the call. An expansion in statement position is spliced into the enclosing
block; in expression position it is a block expression whose value is its last
expression.

#### 3.9.2 Intrinsics

The compiler provides token intrinsics usable in templates and directly in
source: `#stringify(tokens)` (source text of the tokens as a string),
`#concat(lit, …)`, `#location` (`"line:column"` of the outermost call),
`#upper(token)`, `#lower(token)`, `#error([anchor,] message)` (compile error,
expands to nothing), and `#ensure_distinct(message, (key…) …)` (compile error on
the first repeated key group). `#goal_type` reflects the expected type.

#### 3.9.3 Core and library macros

| Macro | Defined in | Expansion |
|-------|------------|-----------|
| `#assert(cond)` / `#assert(cond, msg)` | `std.core.macros` | evaluates `cond` once; on failure prints the call site, the source of `cond`, and operand values, and records the failure |
| `#assert_err(value)` / `#assert_err(value, text)` | `std.core.macros` | checks that `value` is a value error (with exact text) |
| `#todo`, `#unimplemented`, `#unreachable` | `std.core.macros` | `panic` with the standard message or a given one |
| `#dbg(expr)` | `std.core.macros` | prints `expr`, then yields it |
| `#routes { verb "/path" handler … }` | `std.net.http` | `App.new()` followed by one typed `App.<verb>` call per entry |

#### 3.9.4 Attributes and compile-time bindings

```ebnf
meta-attribute  = "#", meta-name, [ "(", meta-args, ")" ]
                | meta-comment ;
meta-name       = "assert_size" | "blocking" | "cfg" | "delete" | "get"
                | "inline" | "inspect" | "law" | "post" | "proof" | "put"
                | "route" | "simd" | "symmetric" | "system"
                | "template_html" | "template_text" | "unroll" ;
meta-args       = { identifier | integer | string | "," } ;
meta-comment    = "//#", meta-text | "/*#", meta-text, "#*/" ;

comptime-expr   = "#", identifier
                | comptime-block | comptime-if | comptime-for ;
comptime-block  = "#{", { statement }, "}" ;
comptime-if     = "#if", expression, block, [ "else", block ] ;
comptime-for    = "#for", identifier, "in", expression, block ;
quote-expr      = "'{", expression, "}" ;
splice-expr     = "${", expression, "}" ;
```

An attribute is a `#name` from this list written on the line before an item or
loop.

`#Name = expression` evaluates at compile time and creates an immutable compile-time binding. `#Name` reads it. `+#Name = expression` (and `+#name(#param: T, …) : Ret = body`) exports that surface from a **macro** Pod so importers can `use` it. `#if`, `#for`, and `#{…}` execute in the meta engine. `'{ expression }` creates hygienic `Code[T]`; `${code}` splices it into the surrounding quoted or generated syntax. `#goal_type` exposes the expected type in metaprogramming contexts. `rewrite` declares a typed rewrite rule.

`#law("description") name(params) { … }` declares a human-locked behavioral invariant. `#proof(law_ident) name(params) { … }` attaches an AI-authored lemma or witness. Prefer Bend-style file separation: `*.law.sr` for locked laws and `*.proof.sr` for proofs (auto-discovered beside modules and under `laws/` / `src/`). Neither form adds reserved keywords; both are `meta-name` attributes. After static/symbolic verification they are completely stripped — zero bytes in C23 AOT, JIT, and `.superb` release bytecode (same erasure class as `test` blocks under `release`/`size`/`fast`).

Compile-time evaluation is deterministic with respect to declared source inputs. `embed "path"` records and embeds file bytes; C23 output may represent sufficiently large assets using the C23 `#embed` facility.

### 3.10 Tests, routes, state, and structured concurrency

Tests have full access to file-private declarations and private fields in their defining module:

```super
test "discount mutates balance" {
    user = User{id: 1, name: "A", balance: 200.0}
    assert_eq(@user.discount(25.0), 150.0)
}
```

Routes are declarations, not function calls:

```super
#post("/orders")
handle_orders(req: http.Request) : http.Response! {
    body = req.json(CreateOrder) or err {
        return http.bad_request(.{error: "{err}"})
    }
    http.ok(.{accepted: true, user_id: body.user_id})
}
```

The `#routes` macro exported by `std.net.http` is an optional route-table
builder:

```super
app = #routes {
    get "/health" { ok("up") }
    post "/echo" |req| { ok(req.body) }
}
```

It expands to `App.new()` followed by the corresponding typed `App.get`,
`App.post`, `App.put`, `App.patch`, `App.delete`, `App.head`, `App.options`, or
`App.any` calls; the expansion is the same code as writing those calls by hand.
Duplicate method/path pairs, unknown methods, and handlers that are neither a
block nor a lambda are compile errors raised by the macro. The route table and
handler checks are known at compile time; diagnostics retain the source route
span. A handler has type `Fn[Request, Response]`. Wrong arity, unit, strings,
integers, and every other non-`Response` return are compile errors; values are
never silently discarded.

`#assert(expression)` is a source-capturing macro from `std.core.macros`. It
evaluates each captured operand exactly once and, on failure, reports the
source expression, location, and captured operand values, then records the
failure so the test runner continues with the next assertion.
Assertions remain active in tests and release programs. `assert_eq(a, b)` is
reserved for meaningful two-value comparisons. `#assert_err(expression)` checks
that a value error occurred; an optional second string requires the exact error
text.

`scope { … }` is a structured-concurrency region. Every `spawn` created in the region shall complete before scope exit. `spawn expression` returns `Task[T]`; `await` obtains its result. `chan[T](capacity)` creates a bounded ownership-transferring channel; omitted capacity defaults to 64. `shared value` creates an atomic or futex-backed shared cell for supported values. A mutable `@` capture may not be used by two sibling spawns.

### 3.11 Conditional compilation

`#cfg(predicate)` is an item attribute. The compiler evaluates it before import
resolution, macro expansion, semantic analysis, and code generation. A false
predicate removes the complete annotated item; code in that item cannot create
name-resolution, type, lifetime, or backend diagnostics. Multiple `#cfg`
attributes on one item are combined with logical AND.

```super
#cfg(target_os = "linux")
use std.sys.io_uring

#cfg(all(unix, feature = "tls"))
+secure_server() => start_tls()

#cfg(not(feature = "tls"))
+secure_server() => fail "TLS feature disabled"
```

```ebnf
cfg-attribute = "#", "cfg", "(", cfg-predicate, ")" ;
cfg-predicate = cfg-atom
              | "all", "(", cfg-predicate, { ",", cfg-predicate }, ")"
              | "any", "(", cfg-predicate, { ",", cfg-predicate }, ")"
              | "not", "(", cfg-predicate, ")" ;
cfg-atom      = "unix" | "windows" | "debug_assertions" | "test"
              | cfg-key, "=", string ;
cfg-key       = "target_os" | "target_arch" | "target_env"
              | "target_family" | "feature" ;
```

Canonical `target_os` values include `linux`, `macos`, `freebsd`, `windows`,
and `wasi`; `target_arch` follows target-triple architecture spelling.
`target_env` includes `gnu`, `musl`, and `msvc`. Unknown keys, unknown bare
predicates, malformed combinators, and unbalanced delimiters are compile
errors. `all` and `any` require at least one operand; `not` requires exactly
one. Feature values are exact, case-sensitive names from `Pod.toml`.

Conditional stripping applies independently to primary modules, imported
modules, and `.law.sr` / `.proof.sr` companions. It occurs before macro
expansion, so a removed item cannot execute compile-time code or introduce a
macro-generated item. Laws and proofs may be gated, but an enabled proof still
follows the ordinary law-target and verification rules.

## 4. Static semantics

### 4.1 Name resolution and visibility

Resolution proceeds from the innermost lexical scope outward, then through module declarations and imports. An unknown name is E0101. An unresolved or invalid import/scope is E0102. An unknown field is E0103. Access to a private field outside its permitted module/test context is E0104.

The `+` prefix exports a top-level binding, function, struct, sum, or trait. Unprefixed top-level declarations are module-private. Selective imports bind only requested exported names. Importing a standard module also binds its namespace (`http`, `ecs`, and so on).

### 4.2 Types and inference

Super uses static types with local inference. Literal defaults are `i64` for integers and `f64` for floating-point values unless an expected type selects another representable built-in type. `none` requires optional context. `.{…}` requires a known struct context. Array elements unify to one element type. An `if` expression’s value-producing branches must unify.

The analyzer registers type names, computes struct/sum layouts, declares callable signatures, checks bindings/routes/tests, and then checks function bodies. This order permits forward function references and recursively visible type names.

Expected types flow from:

- explicit binding annotations;
- function and route return types;
- parameter types at calls;
- struct fields;
- optional/error wrappers;
- the destination of inferred struct literals.

Actual types flow outward from literals, identifiers, operators, calls, indexing, and blocks. Failure to unify is E0201. Using `T?` or `T!` where `T` is required without `or` is E0202.

After signatures are known, each struct header of the form `Name: Trait, …` is checked against the trait items visible in the module, including traits brought in by `use`. A missing method or a signature that disagrees in arity, `@`, parameter type, or return type is E0203. The check does not build a vtable. A later call still resolves to the method written on that struct.

Callable parameters use `Fn[P1, …, Return]`. Expected parameter and return
types flow into lambdas before their bodies are checked. HTTP registration
requires `Fn[Request, Response]`; route diagnostics name the method/path,
expected and actual return types, and point at the offending closure body.

### 4.3 Monomorphization

An untyped parameter or `_` parameter is an inference parameter. Calls are specialized by the tuple of argument types and compile-time arguments. Specialization produces a concrete callable for VM/C23/JIT lowering. `#` parameters are compile-time values and participate in specialization identity. A conforming implementation may share machine code where representations and behavior are equivalent.

There is no runtime boxed “any” requirement for inferred source functions. Dynamic runtime host values are an implementation ABI and do not weaken source-level static checks.

### 4.4 Implicit identifiers

`it` and `index` are introduced only by a construct that defines them:

- `for` and `par`: current value and zero-based index;
- subject `when`: `it` is the subject while evaluating arms;
- pipeline/closure-like stdlib operations: `it` is the current element where that operation’s contract establishes it.

`err` exists only in an `or err { … }` handler. A named loop binder coexists with `it`; both denote the current element. Implementations should diagnose accidental use outside these regions as E0101.

### 4.5 Mutation and aliasing

Bindings are immutable by default in the semantic model, while assignment syntax may update an existing mutable-capable local according to its storage and origin. In-place mutation across a call boundary requires `@`:

```super
move(@position, velocity)
@entity.damage(10)
for @scores { it += 5 }
```

A mutation-capable parameter is written `@name: Type`. Passing a value without the required mutable borrow is E0301. `@` borrows are exclusive for the call/lexical operation, are non-escaping, and lower to C `restrict`-qualified access where sound. Concurrent sibling spawns may not capture the same mutable root; this is E0302.

### 4.6 Usage diagnostics

Unused names are compile-time errors unless deliberately prefixed with `_`:

- E0303: unused local variable;
- E0304: unused parameter;
- E0305: unused import or imported binding.

These codes are distinct from mutation errors: E0301 is missing `@`; E0302 is a cross-spawn mutable capture. The distinction is stable and tooling may key quick fixes to it.

### 4.7 Lifetime and root escape

A mutable root includes `state` and other module-root mutable storage. A function may mutate such a root in place but may not return an `@` reference or aggregate that lets a mutable root outlive the checked region. An escaping mutable root is E0401. Local scoped-stack values likewise may not escape the scope that owns their storage unless moved/copied into an enclosing destination selected by destination-driven construction.

## 5. Runtime and memory model

### 5.1 Values and layout

Scalars are passed in registers where practical. Named structs use deterministic C-ABI-compatible offsets, alignment, and size. A sum consists of a 32-bit tag plus inline payload storage sufficient for its largest variant. Strings and collection descriptors refer to bytes owned by an appropriate data stack, static pool, embedded asset, or host binding.

The abstract model does not expose GC. Super programs should assume no tracing collector and no implicit heap lifetime extension.

### 5.2 Scoped data stacks

Temporary aggregates, strings, records, and arrays are allocated on virtual-memory-backed bump arenas called data stacks. Entering a lexical, request, game-frame, or task scope records a mark. Exiting rewinds the cursor to that mark in O(1); individual objects are not freed.

**Implementation status (0.1.0).** The typed C23 AOT path places strings and frame-local arrays on data stacks and rewinds them at region exits. Functions the AOT planner cannot type fall back to a boxed runtime. Closures are the common case, and route registration on `App` uses them. The boxed runtime allocates SuperObj values on the durable heap; strings/arrays on the App.serve path use a request bump arena. HTTP request scopes reclaim strings/arrays via a per-request bump arena; durable SuperObj calloc remains (net 42). The AOT hello server performs 42 heap allocations and 0 frees per request, so its memory grows with traffic; `crates/super-cli/tests/alloc_probe.rs` pins these counts.

The runtime supports:

- a primary stack for ordinary destination-driven values;
- additional stacks for simultaneously growing collections and variable payloads;
- task micro-stacks reset on task/scope exit;
- per-worker stacks for M:N parallel execution;
- 32-bit relative offsets inside a stack;
- an implicit return destination (`ret_ptr`) for constructing results directly where the caller needs them.

Stack overflow is a checked allocation failure at the safe runtime boundary, not permission to overwrite memory.

### 5.3 Arrays

`[a, b, c]` is a contiguous zero-indexed array. `[value; N]` is a dense compile-time-sized stack array, modeled as `[T; N]`, with `N` instances initialized from `value`. The compiler may prove bounds from `N`, ranges, and canonical loops and remove redundant checks. It may select specialized dense `i64` or `f64` storage.

Slices such as `msg[lo..hi]` borrow a contiguous subrange. A slice cannot outlive its backing stack value. Mutable array arguments use `@` and therefore satisfy the non-alias contract required by restrict/SIMD lowering.

### 5.4 `@` as a restrict borrow

For the dynamic extent of an `@` borrow, accesses that could mutate the borrowed region occur through that borrow and derived accesses. The borrow shall not escape, be stored into longer-lived state, or overlap another active mutable borrow. Read-only non-`@` arguments may alias unless the compiler proves otherwise.

The C23 backend lowers eligible mutable array/component parameters to `T * restrict`. The native JIT uses the same no-alias fact when selecting packed loops. Violating the source rule is diagnosed rather than becoming C undefined behavior.

## 6. Execution and lowering

### 6.1 Evaluation

Expressions are evaluated left-to-right except that short-circuit operators evaluate their right operand only when required. `or` evaluates its fallback only on `none`/error. Multi-assignment evaluates all right sides before stores. `ensure condition else { … }` runs the else block only when false; well-structured use exits or otherwise establishes the condition afterward.

Integer arithmetic uses the implementation’s signed 64-bit runtime representation for `i64`. Floating arithmetic follows IEEE-754 `f32`/`f64` behavior. Division/modulo by zero is a runtime error unless removed from a statically unreachable branch.

### 6.2 C23 AOT backend

C23 is the default source-to-native AOT language standard (`--std c23`; `--c23` is an alias for C compilation with C23). Generated units include `<stdbit.h>` and use C23 features where they improve representation or optimization:

- `static constexpr` for compile-time integer/byte tables;
- C23 `#embed` as an available representation for large embedded files;
- `<stdbit.h>` primitives for bit operations where selected;
- `[[unsequenced]]` on pure runtime constructors/helpers;
- typed C structs for source structs and compact tag/payload layouts for sums;
- `restrict` on proven exclusive `@` array/component pointers;
- OpenMP lowering for recognized pure `par` kernels and `+` reductions when compiled with OpenMP support.

The emitter first builds typed bytecode/IR, emits self-contained C, then invokes the configured host compiler at the requested optimization level. Unsupported optimization patterns retain correct scalar lowering. OpenMP is an optimization contract, not a change to source semantics.

Every Super name that becomes a C identifier is checked against the ISO C23 §6.4.1 keyword list, including the obsolescent underscore spellings and the interchange-float keywords. A match is emitted with a trailing `_`, so a field named `short` is the C member `short_`. Field lookup still uses the source spelling.

#### Build profiles

`super build` (short form `super b`) and `super emit-c` select a named profile, then apply any explicit overrides. `super run` has the short form `super r`. With no source-file argument those commands walk up to `Pod.toml` (then removed legacy `Super.toml`) and compile the Pod entry for its `kind`: `src/main.sr` (`bin`), `src/lib.sr` (`lib`), or `src/macros.sr` (`macro`). The default profile is `dev` unless `[build] profile` names another one. `super test` always selects `test`, then applies `[profile.test]`.

The default compilation backend is `c23` and the default mode is `aot`. `super build` therefore emits C23 and links a native artifact at `target/<profile>/<pod>` (`<pod>.exe` on Windows for bin Pods). Lib and macro Pods build without a process `main` (shared/`omit_main`). `[build] backend` accepts `c23`, `jit`, or `vm`. `[build] mode` accepts `aot`, `jit`, or `bytecode`. A missing key uses those defaults, so an older manifest still builds. `--backend` and `--mode` override the file. `--mode bytecode` writes the portable `target/<profile>/<pod>.superb` module (`.superb-dev` when tests are kept) and does not link. `jit` still links a native image through the C23 stack, which is the executable this toolchain can run as a JIT image. The CLI prints each build step as it starts. The current step name is cyan-green. The closing line is `Finished` in cyan-green, `Failed` in cyan-red, or `Finished with warnings` in cyan-yellow.

| Profile | Selection | Host compile contract | `test` blocks |
|---|---|---|---|
| `dev` | default, `-d`, `--debug`, `--profile dev` | `-O0 -g`, no LTO, generic CPU | kept |
| `test` | `super test`, `--profile test` | `-O2 -g -march=native`, no LTO, no fast-math | kept |
| `release` | `-r`, `--release`, `--profile release` | `-O3 -flto -march=native -ffast-math -funroll-loops -fno-plt -fno-semantic-interposition -fomit-frame-pointer -s` | stripped |
| `size` | `--profile size` | `-Os -flto -ffunction-sections -fdata-sections -Wl,--gc-sections -s` | stripped |
| `fast` | `--profile fast` | same peak as `release` | stripped |

`-r`/`--release` and `-d`/`--debug` cannot be combined. An explicit level replaces the profile's optimization level and does not change the rest of the profile: `-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-Oz`, `-Og`, `-Ofast` (also `--opt <LEVEL>`). Link controls are `--lto off|thin|fat` (`--lto` alone means thin), `-s`/`--strip`, `--no-strip`, `-g`/`--debug-info`, `--static`, `--shared`, `--target <TRIPLE>`, `--target-cpu native|generic|<cpu>`, `--fast-math` / `--no-fast-math`, and repeated `--link-arg`. Thin LTO is `-flto=auto` for GCC and `-flto=thin` for Clang. Fat LTO is `-flto` for GCC and `-flto=full` for Clang. The choice follows the compiler binary name.

#### Compile targets

`[build] target`, `[build] targets = […]`, `[profile.<name>] target`, and `--target <TRIPLE>` select the destination architecture and OS. `super targets` prints the catalog. Accepted spellings are canonical triples (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-gnu`, …), short `arch-os-abi` (`x86_64-linux-musl`), and the aliases `native` / `host`. WebAssembly is first-class: `wasm32-wasi` (WASI CLI), `wasm32-unknown-unknown` (freestanding), and the `wasm64-*` counterparts. Host builds keep `target/<profile>/<pod>`; every other triple nests as `target/<triple>/<profile>/<pod>` (`.wasm` for WebAssembly, `.exe` on Windows). Cross and wasm builds prefer `zig cc -target …` when `zig` is on `PATH`, else `clang --target=…`. `[target.<triple>]` may override `cc`, `std`, `target-cpu`, `static`, `shared`, `out`, and `link-arg` for one destination. Cross builds drop `-march=native`; wasm builds omit OpenMP and host-only POSIX headers while keeping stdio stubs.

These host flags apply to every native build. `super build` uses the C23 stack unless `mode` is `bytecode`. The profile still decides whether `test` blocks are stripped. Profile and link flags do not change the observable result of a conforming program, aside from test stripping and the host compiler's own `-Ofast` / `-ffast-math` contraction rules. `-fwrapv` and `-ffp-contract=off` stay on so signed overflow and fused multiply-add match the bytecode VM. `release` is the fastest profile the toolchain emits: fat LTO, `-O3`, the host CPU, fast-math, unrolling, and direct calls. `fast` uses that same peak so an older `--profile fast` command is not slower than `--release`.

`Pod.toml` may patch a preset. `[profile.<name>]` wins over `[build]` for the same key, and an explicit CLI flag wins over both. Unset CLI booleans stay unset, so a value in the file still applies. `super build`, `super run`, `super check`, `super test`, and `super verify` resolve and fetch the manifest's dependencies before compiling.

`super verify` (short form `super v`) discharges `#law` / `#proof` items (§7.1) and never emits them into C23, JIT, or bytecode. The same erasure guarantee applies whenever a release/`size`/`fast` profile strips tests: verify-only items produce **0 bytes / 0 lines** in `super emit-c --profile release`, JIT images, and `.superb` release modules. Optional `[verify] require-on-release = true` gates `super build --release` and `super install` on a successful verify (typically with `--locked-laws`).

#### Installing, querying, and cleaning

`super install` (short form `super i`) builds the current **bin** Pod at the maximum release profile and copies the native binary onto `PATH`. The build is C23 AOT with the release contract forced, whatever `[build] profile` says: `-O3`, fat LTO, `-march=native`, `-ffast-math`, unroll, direct calls, and strip. Test blocks are stripped. The executable name is `[pod].name` (`name.exe` on Windows). Lib and macro Pods are skipped.

The destination is chosen in this order:

1. `--bin-dir <DIR>`
2. `SUPER_INSTALL_DIR`
3. the first of `~/.local/bin`, `~/bin`, and `~/.cargo/bin` that is already on `PATH` (the same three directories under the user profile on Windows)
4. `~/.local/bin`, which Super creates

When step 4 is not already on `PATH`, Super adds it to the user environment so a new terminal can run the program: the shell startup file on Unix (`~/.zshenv`, `~/.bashrc` and `~/.profile`, `~/.config/fish/config.fish`, `~/.config/nushell/env.nu`, or `~/.profile`) and the user `Path` on Windows. An explicit `--bin-dir` or `SUPER_INSTALL_DIR` is used as given and is not written into the shell startup file.

The command walks upward to `Pod.toml`. A workspace is either a manifest with `[workspace] members = ["app", "tool"]` or a directory of child Pods. Every **bin** member with `src/main.sr` is installed. Lib and macro Pods are skipped. Re-running replaces a binary this command installed. `--force` replaces an executable it did not install. Two members with the same Pod name are an error.

`super uninstall` (`super u`) removes one recorded binary. With no name it uninstalls the single bin Pod around the current directory. It does not delete a file Super did not install, and it leaves the `PATH` entry in place. `super list` (`super installed`) prints name, version, path, and Pod root. `super which [name]` prints only the executable path.

Receipts are `installed.toml` under `SUPER_DATA_DIR`, else `$XDG_DATA_HOME/super`, else `~/.local/share/super`. On Windows the fallback is `%LOCALAPPDATA%\super`.

`super clean` removes binary artifacts under the Pod `target/` directory. `-d` / `--debug` removes `target/dev` and `target/debug`. `-r` / `--release` removes `target/release`. `-a` / `--all` removes the whole `target/` directory. With no flag, `-a` is used. The three flags cannot be combined. A workspace cleans every member. `super help` and `super help <command>` print the same usage text as `--help`.

### 6.3 VM and native JIT

The portable execution path is a register-oriented bytecode VM. The JIT owns writable memory through an RAII executable buffer, emits bytes, and changes the mapping to read/execute before calling it. Supported architectures include x86-64 and AArch64, with a safe fallback elsewhere.

The SIMD kernel model recognizes canonical no-alias loops over dense numeric arrays or C-ABI fields. On x86-64 it emits packed SSE2 operations such as `mulpd`/`addpd` plus a scalar tail. On AArch64 it emits two-lane NEON `fmul`/`fadd` plus a scalar tail. A kernel that is not recognized executes through general bytecode or C23 AOT; semantics remain identical.

### 6.4 Parallelism and I/O

`par` divides independent iteration space across worker tasks. Every worker uses its own data stack. Reductions merge worker-local accumulators in a defined reduction step. Source that writes overlapping memory without an ownership-safe construct is rejected or remains serial.

The Linux system layer exposes non-blocking sockets and epoll readiness through `std.sys.epoll`/host bindings. `epoll_create`, control, wait, and close operations own descriptors in runtime state. Higher-level HTTP, WebSocket, TCP, UDP, SSE, retry, and DNS modules build on the system contracts. Other platforms may implement equivalent readiness APIs without exposing epoll itself.

## 7. Metaprogram execution model

Compile-time values are separate from runtime values even when their scalar representation matches. The meta engine expands `#` constructs before ordinary semantic analysis/lowering of the generated program. Expansion is hygienic: quoted local names do not capture unrelated caller locals, and explicit splice points are the route by which generated code enters its context.

Recognized attributes include:

- `#inline`: prefer inline lowering;
- `#simd`: assert/request SIMD-friendly loop lowering;
- `#unroll(N)`: request fixed-factor unrolling;
- `#blocking`: mark a host operation as blocking for scheduling;
- `#inspect`: expose compile-time inspection;
- `#route`, `#get`, `#post`, `#put`, `#delete`, `#system`: attach domain metadata;
- `#assert_size(N)`: assert a computed layout size;
- `#law("…")`: human-locked behavioral invariant (verify-only; see §7.1);
- `#proof(law)`: AI-authored discharge / lemma for a `#law` (verify-only).

#### 7.1 Law & Proof verification (`super verify`)

`super verify` (short form `super v`) discharges every `#law` in the Pod/entry using a three-tier engine that preserves Super execution semantics (`-fwrapv` signed `i64` wrap, IEEE-754 with `-ffp-contract=off`, `@` non-escaping restrict borrows, Goose-style scoped stacks):

| Tier | Solver flag | Role |
|---|---|---|
| 1 | `smt` | Symbolic / bitvector exploration of pure integer/boolean/`@`/struct updates |
| 2 | `comptime` | Equational rewrite / algebraic identity discharge (e.g. `decode(encode(x)) == x`, reducer associativity) |
| 3 | `fuzz` | Bounded deterministic property fuzzing for residual / host-boundary laws |
| 4 | `structural` | Verify-engine laws that cannot be Super `#assert` bodies (e.g. LAW-BENCH-001 filesystem scan + Gate B evidence) |

`auto` (default) tries tiers in order. CLI flags: `--locked-laws` (fail if `#law` bodies drift from `laws.lock`), `--solver`, `--timeout <ms>`, `--json`. Pod.toml `[verify]` may set `laws`, `proofs`, `require-on-release`, and `solver`. When `require-on-release = true`, `super build --release` and `super install` gate on a green verify.

Falsified laws emit **E0502** with counterexample bindings. Missing / `#todo` proofs emit **E0503**.

**LAW-BENCH-001** (structural — see `benchmarks/laws/LAW-BENCH-001.md`): every gated Super source under `benchmarks/` (except `benchmarks/tests/**`) must never `use host` or call `host.` / host FFI. Unlike behavioral `#law` bodies (e.g. transfer conservation, `pl_limit` fusion), this invariant cannot be an `#assert` over program state — the verify engine discharges it via `prove_law_bench_001` (**tier `structural`**, code **E0504**): suite source scan + Gate B evidence check in `runner.py`. Complementary: per-file E0504 from `super check`, and Gate B in `verify_super` before timing. A `#assert(true)` stub is not a valid law or proof.

Builtin diagnostic macros (always in scope, no `use`; documented in `std.core.macros`):

- `#todo` / `#todo("…")`: expand to `panic` with a not-yet-implemented message;
- `#unimplemented` / `#unimplemented("…")`: expand to `panic` for a deliberate unfinished path;
- `#unreachable` / `#unreachable("…")`: expand to `panic` for a branch that must never run;
- `#assert(cond)` / `#assert(cond, "…")`: panic when `cond` is false;
- `#dbg(expr)`: print `expr` then yield it.

`panic` / `panic <msg>` is an unrecoverable abort. It is distinct from `fail`, which produces a recoverable `T!` error for `or` / `or err`.

An invalid compile-time operation is E0501. Since the requested stable reference range in this release is E01xx–E04xx, E0501 is documented as an extension code and does not alter those ranges.

## 8. Pods and reproducibility

A **Pod** is a Super project unit. The canonical manifest is `Pod.toml` with a `[pod]` table. `Pod.toml` with `[pod]` is the only supported manifest. `kind` selects the project type:

| `kind` | Entry | Role |
|---|---|---|
| `bin` (default) | `src/main.sr` | Executable. `super install` ships it. |
| `lib` | `src/lib.sr` | Reusable library. Build emits without a process `main`. |
| `macro` | `src/macros.sr` (or `src/lib.sr`) | Distributable comptime macros and `+#` exports. |

```toml
[pod]
name = "orders"
version = "0.1.0"
kind = "bin"

[build]
profile = "dev"
backend = "c23"
mode = "aot"
# codegen-units = "incremental"   # default: per-TU .c + shared super_rt; use "single" for one giant .c
# keep-c = false                  # default: delete intermediate .c/.o after a successful link
# target = "native"
# targets = ["native", "wasm32-wasi", "aarch64-unknown-linux-gnu"]

# [registry]
# path = "~/.super/registry"      # local filesystem registry for `super publish` / version deps

[profile.test]
opt = "2"
lto = "off"
debug-info = true
fast-math = false

[profile.release]
opt = "3"
lto = "fat"
strip = true
fast-math = true
target-cpu = "native"

[target.wasm32-wasi]
cc = "zig"

[features]
default = ["tls"]
tls = []
full-io = ["tls"]

[dependencies]
geo = { path = "../geo" }
router = { git = "https://github.com/acme/router.git", tag = "v1.4.0" }
codec = { git = "https://gitlab.com/acme/codec.git", rev = "abc123" }
```

`super new` / `super init` write `Pod.toml` with `dev`, `test`, `release`, `size`, and `fast` tables. Flags `--bin` (default), `--lib`, and `--macro` set `kind` and scaffold the matching entry file. Commands that compile a Pod (`build`, `run`, `check`, `test`, `fetch`, `install`) walk upward for `Pod.toml`, apply it, and fetch every dependency it lists. A CLI flag overrides the matching key. `super install` forces the §6.2 release contract and copies only **bin** Pods onto `PATH`.

`[features]` maps each feature to the features it enables. `default` is the set
enabled when no feature-selection flag is present. `super build --features
tls,full-io` enables named features and their transitive entries;
`--all-features` enables every declared non-`default` feature; and
`--no-default-features` suppresses only the default set. Unknown feature names
are errors. Feature selection feeds `#cfg(feature = "name")` before imports and
macros are processed.

A workspace manifest names members and may omit `[pod]`:

```toml
[workspace]
members = ["app", "tool"]
```

`super install` and `super clean` from that directory act on every member. A directory of child Pods with no root manifest is the same kind of workspace. From inside one member, the nearest manifest wins.

Dependencies are path or Git dependencies. Git entries may select exactly one operational reference through `tag`, `branch`, or `rev`; the resolver also accepts `github:owner/repo@tag` and `gitlab:owner/repo@tag` shorthand. Sources are cached under `~/.super/cache/git`.

Resolution is transitive and rejects dependency cycles. `Pod.lock` format version 1 records every Pod source and the resolved Git commit. Normal resolution honors locked commits; update fetches and repins. `super vendor` copies resolved Pods into `vendor/` without `.git` directories. Pod source is loaded from `src/` when present, otherwise from the dependency root. Macro Pods export comptime bindings with `+#NAME` and specialization functions such as `+add_n(#n: i64, x: i64)`; importers resolve them through ordinary `use`.

## 9. Standard library contracts

The Super standard library is **only** `stdlib/**/*.sr`. Runtime host builtins (`sys_*`, crypto, and IO kernels that `.sr` reaches via `host`) live in `crates/super-host` — there is no Rust crate named `super-std`. Embedding FFI stays in `crates/super-ffi`.

The 0.1.0 toolchain ships **179** .sr files under `stdlib/` (127 implementation modules, `stdlib/mod.sr`, plus law/proof sidecars). Vendor SaaS clients are **first-party Pods** under `pods/` (`kind = "lib"`), not stdlib modules. Individual implementation details may grow compatibly, but the following domain contracts are stable:

- `len(value)` and `value.len()` are the canonical zero-cost length operation.
  Arrays and collection values report element count; UTF-8 strings report byte
  count. `char_count(text)` reports Unicode scalar count. Public
  Type-prefixed or container-specific length aliases do not exist.
- HTTP's common path is typed route registration followed by
  `@app.serve(address)`. `ServeConfig` is a plain value that sets timeout,
  header/body, and keep-alive limits. The serving path is not allocation-free:
  on the C23 AOT build the hello server performs 256 heap allocations and 7
  frees per request (pinned by `crates/super-cli/tests/alloc_probe.rs`). `App.serve` owns listener setup,
  SIGINT/SIGTERM notification, serving, cleanup, and graceful stop. Direct
  `Server`/`Notify` wiring remains the advanced path.

- `std.core`: strings, bytes, collections, regex, UUID, Unicode (UTF-8/graphemes/`wcwidth`), fixed-point `Decimal`, bitsets/Bloom filters, language traits (`Iterable` / `ParIterable` / `AssociativeReducer`), and `std.core.reduce` witnesses; operations preserve explicit ownership and return optionals for searches that may fail.
- `std.fmt`: JSON, CSV, TOML, YAML 1.2, MessagePack, XML, binary encoding, compression/checksums, MIME; JSON decoding supports validation and path extraction without requiring a heap object tree.
- `std.sys`: path, CLI argv tokenizer, environment, raw filesystem/descriptors, processes, time, sync, signals, **epoll**, **io_uring**, dynamic `ffi` (`dlopen`/`dlsym`), and `mmap`. Resource-owning operations expose explicit close/lifetime behavior.
- `std.io`: unified readiness (`std.io.poll`), ring buffers, buffered file readers/writers, stdio/console, sockets, and tar/zip archive helpers. Prefer `std.io.poll` over raw epoll/uring.
- `std.net`: HTTP request/response/router/middleware, WebSocket framing, URL/IP, TCP/UDP, DNS, SSE, retry/circuit breaking, TLS 1.3 framing/ALPN/SNI, SMTP+STARTTLS, gRPC/HTTP2+Protobuf, QUIC framing, and OAuth2/OIDC PKCE. Game-oriented UDP netcode lives in `std.game.netcode` (not `std.net`).
- `std.db`: SQL builders/in-memory tables, PostgreSQL / MySQL / SQLite / Redis / ClickHouse wire helpers, and transactional migrations. Hosted vendor wrappers (Supabase, Neon, CockroachDB, MariaDB dialect extras, Meilisearch) are Pods under `pods/`.
- `std.sec`: cryptographic helpers, encodings, HS256 JWT, pure-Super SHA-2, broader hashes, post-quantum/hybrid primitives, X.509/ASN.1, HOTP/TOTP/WebAuthn helpers, and Argon2id PHC strings. Contracts distinguish encoding from cryptographic authentication.
- `std.sec.secret` vs `std.sec.vault`: **secret** is process-local `Secret[T]` (Display/Debug redaction, zeroization, constant-time compare) plus an optional local AEAD **FileKeyring** for named Secrets — not a remote vault. **vault** is HashiCorp Vault-*shaped* named storage (`put_secret` / `get_secret`, KV/auth surface). Public export names must not overlap (CI: `scripts/assert_secret_vault_names.py`).
- `std.datetime`: civil date/time, IANA TZ offsets, ISO 8601, calendar arithmetic (distinct from `std.sys.time` instants); `std.datetime.cron` for TZ-aware schedules.
- `std.html`: XSS-aware HTML builders (`std.html.core`), tolerant parse (`std.html.parse`), and **`std.html.template`** — Html/Text string templates with comptime diagnostics **T001–T004** and destination `TplWriter`.
- `std.math`: scalar math, algebraic traits, special functions, forward/reverse AD, optimization, ODE/quadrature, interpolation/polynomials, structured/sparse linear algebra (`when (a,b)` / `#symmetric`), random generators, noise, complex numbers, quaternions, statistics, geometry, and FFT.
- `std.data`: typed null-aware columns, Frame/LazyFrame analytics (filter, join, group_by, sort), expression/plan layers, and CSV/xlsx I/O via `std.fmt.xlsx`.
- `std.game`: sparse-set ECS, 2D physics, AI/pathfinding, app loop/input, assets, tilemaps, procedural audio, and UDP netcode. `std.game.fx` adds `Sprite` (a `Widget`), `Clip`, `Cue`, and `Mixer`. A sprite's `paint` is a `sprite x y w h name frame` display-list line. A cue's `play` is a `sound name gain` line. Neither opens a device.
- `std.ai`: generic OpenAI-compatible HTTP/SSE client (`std.ai.client`), local tensors (`std.ai.tensor`), and embedding/search helpers (`std.ai.embed`). Named vendor SDKs (OpenAI, Claude, Gemini, OpenRouter, DeepSeek, ElevenLabs, Seedance) are Pods under `pods/`.
- `std.diag`: test assertions, structured/redacted logging, counters/gauges/histograms, and a micro-benchmark harness (`std.diag.bench`).
- `std.cli.cmd` (also `std.cli`, `std.clap`): structured commands, subcommands, help, and typed options. **Application code imports only this module** — it re-exports `cli_args`, `cli_parse`, and `CliArgs`. Parsing helpers use `std.core.str`, `std.core.collections`, and `std.sys.env`. The argv tokenizer implementation lives at `std.sys.cli` (alias `std.cli.args`) and is not an application import. Commands also cover count flags, subcommand aliases, conflict and requirement checks, help groups, author and after-help text, closest-name suggestions, and a bash completion word list.
- `std.tui`: a cell screen, layout, and widgets (block, paragraph, list, gauge, tabs, sparkline, table, word wrap, aligned text, scrollbar, bar chart). `Style` and `theme` name ink and paper. `layout_grid`, `tier`, and `reflow` place cells, including a stack when the terminal is narrow. `mouse_parse` reads SGR mouse events. Cells are dense integer planes from `std.core.collections`; text measurement uses `std.core.str` / `std.core.unicode`; gauges use `std.math.core`. The host diffs the previous frame into ANSI and owns the terminal.
- `std.gui`: a retained widget tree (column, row, panel, grid, scroll, label, text, button, input, check, list, slider, progress, tabs, and the controls in `std.gui.widgets`: toggle, checkbox, radio, dialog, field, toast, flash, shape, titlebar, chrome, badge). `custom` mounts a display list from `Widget.paint`. Each node carries color, type size, weight, alignment, tracking, padding, gap, grid span, and opacity. `theme` applies a palette. `tier` is compact below 480 pixels and wide at 960; a row stacks and a grid becomes one column on a compact window. `note` / `take` are the event queue. Escape closes dialogs. Choosing a radio clears the other buttons in its group. `animate` / `frame` ease `x`, `y`, `w`, and `opacity`. `measure_text` is the calligraphy width. Layout stays in Super. Hits use `std.math.geom`. `paint` is the display list used by tests and by the canvas. `describe` is the widget document a desktop provider turns into real controls: GTK 4 on Linux (Wayland, then X11), Win32 on Windows, and Cocoa on macOS. `SUPER_GUI` may name `gtk`, `wayland`, `x11`, `win32`, `cocoa`, or `macos`; the host tries the remaining providers when the named one cannot open. `mirror` draws the same tree through `std.tui`. `run` paces frames with `std.sys.time` and polls `std.gui.window`. `SUPER_GUI=0`, `headless`, or `quiet` keeps tests off-screen. `SUPER_GUI=ppm` writes a PPM. `SUPER_GUI=1` or `native` opens the platform provider.
- `std.gui.widgets`: the `Widget` trait (`paint(self) : str`, `hit(self, x: i64, y: i64) : bool`, `measure(self, max_w: i64) : i64`) and the controls that declare it inline. `widget_hit` and `widget_measure` are the shared rectangle helpers. A program may declare `+Name: Widget` and pass `name.paint()` to `Gui.custom`.
- `std.gui.window`: native window open/poll host bindings (moved out of `std.sys` so `sys/` stays headless).

Short aliases such as `std.http`, `std.ecs`, `std.json`, `std.metrics`, and `std.linalg` resolve to their layered modules. Standard-library functions are ordinary Super declarations over host primitives, so they are visible to type checking, UFCS completion, tests, and C/JIT lowering.

### 9.1 First-party vendor Pods

The workspace root `pods/` directory holds distributable `lib` Pods that wrap hosted APIs while depending on `std.*`:

| Pod | Depends on (representative) |
| --- | --- |
| `openai`, `claude`, `gemini`, `openrouter`, `deepseek`, `elevenlabs`, `seedance` | `std.ai.client`, `std.net.http` |
| `supabase`, `neon`, `cockroachdb` | `std.db.postgres`, `std.db.sql` |
| `mariadb` | `std.db.mysql`, `std.db.sql` |
| `meilisearch` | `std.net.http`, `std.fmt.json` |

Each Pod has `Pod.toml` with `kind = "lib"`, entry `src/lib.sr`, and inline `test` blocks. A workspace `pods/Pod.toml` lists all members under `[workspace] members`.

### 9.2 Embedded MCP knowledge server

The toolchain ships an embedded Model Context Protocol server for coding agents:

```bash
super mcp
# shim: super-mcp  →  exec super mcp
```

Transport is **stdio**, newline-delimited JSON-RPC 2.0 (MCP protocol version `2024-11-05`). Content-Length framed requests are also accepted. The server advertises tools, resources, and prompts that describe the **current** Super version:

| Surface | Purpose |
| --- | --- |
| Tools | `super_overview`, `super_search`, `super_list_features` / `super_get_feature`, `super_list_algorithms` / `super_get_algorithm`, `super_list_examples` / `super_get_example`, `super_list_stdlib` / `super_get_stdlib`, `super_cheatsheet`, `super_cli_help`, `super_verify` |
| Resources | `super://overview`, `super://laws`, `super://feature/{id}`, `super://algorithm/{id}`, `super://example/{id}`, `super://stdlib/{path}` |
| Prompts | `learn-super`, `write-super`, `explore-stdlib` (law-guarded workflow: edit `*.proof.sr`, never `*.law.sr`; call `super_verify`) |

`super_get_stdlib` reads live `.sr` source and export lists when `SUPER_STDLIB` / `SUPER_STD` (or the installed `$SUPER_HOME/lib/stdlib`, default `~/.super/lib/stdlib`) resolves. This server is the authoritative in-tree synopsis of language features, example projects, algorithm sketches, and stdlib coverage for the installed toolchain. Implementation: `crates/super-mcp`.

### 9.3 Toolchain install layout

The public one-liner (`install.sh` on the GitHub Pages site / repository root) and `scripts/install.sh` / `make install` place the **language toolchain** outside `~/.cargo`:

| Path | Contents |
| --- | --- |
| `$SUPER_HOME/bin/super` | CLI (`super-lsp` / `super-mcp` are shims) |
| `$SUPER_HOME/lib/stdlib/` | Standard library tree (`use std.*`) |
| `$SUPER_HOME/registry/` | Local Pod registry (`super publish`) |
| `$SUPER_HOME/cache/` | Git dependency cache |

`SUPER_HOME` defaults to `~/.super`. With `SUPER_USE_XDG=1`, it uses `$XDG_DATA_HOME/super`. Stdlib resolution order: `SUPER_STDLIB` / `SUPER_STD` → project `./stdlib` → `$SUPER_HOME/lib/stdlib` (and next-to-exe / legacy share layouts) → embedded. Separately, `super install` for **application** bin Pods still prefers a user PATH directory (`~/.local/bin`, `~/bin`, or `~/.cargo/bin` when already on `PATH`).

## 10. Diagnostics reference

Diagnostics are rendered in rustc-like form with a stable code, primary span, labels, and notes. The stable E01xx–E04xx reference is:

| Code | Class | Meaning and required correction |
|---|---|---|
| E0101 | Name | Undefined name. Declare it, import it, or use the intended spelling. |
| E0102 | Name/module | Invalid import, unresolved module, or lexical-scope violation. |
| E0103 | Member | Unknown field or method for the receiver’s inferred type. |
| E0104 | Visibility | Private field accessed outside its permitted module/test context. |
| E0201 | Type | Type mismatch or failure to unify expected and actual types. |
| E0202 | Type/control | Optional or value error used without `or`, propagation, or handling. |
| E0203 | Type | A struct header names an unknown trait, or a method is missing or does not match the trait signature (arity, `@`, parameter type, or return type). |
| E0204 | Type | A method matches a language trait (`Display`, `Eq`, `Add`, …) but the type does not declare it, or a language-trait definition drifts from `std.core.traits`. |
| E0205 | Type | Ambiguous multiple dispatch: two or more `FuncFamily` / multi-`when` candidates remain and none dominates. |
| E0206 | Type | Duplicate overload: two definitions share identical canonical parameter types and `@` flags. |
| E0301 | Mutation | In-place mutation requires an `@` mutable borrow. |
| E0302 | Concurrency | Mutable root captured across sibling spawns or unsafe spawn boundary. |
| E0303 | Usage | Unused local variable; consume it or prefix an intentional placeholder with `_`. |
| E0304 | Usage | Unused parameter; use it or prefix it with `_`. |
| E0305 | Usage | Unused import/imported binding; remove or use it. |
| E0401 | Lifetime | An `@` borrow or mutable root escapes its permitted lifetime. |
| E0402 | Lifetime | A non-`@` stack borrow (array slice / nested array element) escapes via return, record store, channel send, or spawn capture. |

For completeness, E0001 denotes lexer/parser syntax errors and E0501 denotes compile-time/meta failures. Extension codes for law verification:

| Code | Class | Meaning |
|---|---|---|
| E0502 | Verify | `#law` invariant falsified; diagnostic includes counterexample parameter bindings. |
| E0503 | Verify | Incomplete or invalid proof: missing discharge, `#todo`/`#unimplemented` on a proof path, orphan `#proof`, or `--locked-laws` drift. |
| E0504 | Verify | **LAW-BENCH-001**: `use host` / `host.` / host FFI found in a gated `benchmarks/**/*.sr` source. |

`std.html.template` stable codes (never renumber once released):

| Code | Class | Meaning |
|---|---|---|
| T001 | Template | Template syntax error (unclosed tag, bad `#if`/`#for`, unknown filter, …). |
| T002 | Template | Unknown interpolation name or partial (when a known-name list is provided). |
| T003 | Template | Value is not renderable (needs `Display`, `Escaped`, `Raw`, `Js`, or `Css`). |
| T004 | Template | Unsafe Html context (script/style interp without `js`/`css`/`raw`, or unquoted attr). |

They sit outside the requested E01xx–E04xx semantic range.

## 11. Stage-1 verified standings (Protocol 2.0.0)

The in-tree multi-language suite under `benchmarks/` (twenty equal-algorithm kernels) is gated by **LAW-BENCH-001**: Super sources must not `use host` or call host FFI. Timing uses Protocol **2.0.0** (2 warm-ups discarded, 10 interleaved timed runs, median wall). Composite weights: Speed 40% · Memory 25% · Reliability 15% · Cycle 10% · LoC 10%.

**Verified standings (frozen 2026-11-26, Protocol 2.0.0):** Super C23 AOT is overall composite **#1 at 89.23** (display **89.2**). **Speed** category: **Zig 100.0** vs SuperAot **97.87** (overlapping CIs on `01_nbody`, `03_astar_pathfinding`, `04_json_serde` ⇒ within noise). SuperAot wins **Memory** and **LoC**; **Reliability → All** (100% pass for every language in the frozen table); SuperJit wins **Cycle**. All twenty algorithms pass on Super AOT/JIT and peer ports. Suite Super code LoC is **2198** (Bun **2200**, within two lines). C port **29,758** lines (outline-expanded C23 — see README LoC methodology). Absolute Speed walls: `docs/stage1_speed_walls.md`. Source of truth: **`docs/REPORT.md`** / **`docs/results.json`**; site via `scripts/publish_site.sh` (do not re-run or rescale scores).

Toolchain upgrades landing with this remesure: static/`FuncFamily`/`#symmetric`/tuple-`when` multiple dispatch (VM/JIT/AOT), `std.math` scientific suite, Polars-grade `std.data` + `std.fmt.xlsx`, host `sys_arr_truncate`, typed `Lcg` fill paths (no Dyn `roll` CallHost), and Super preference on exact category ties. Integrity: equal algorithms; no Super bench host kernels; no Body-alias AOT regressions.

## Appendix A. Representative program

```super
use std.net.http
use std.diag.metrics

+User = {
    id: i64
    name: str
    balance: f64 = 0.0

    discount(@self, percent: f64) : f64 {
        cut = self.balance * percent / 100.0
        self.balance -= cut
        self.balance
    }
}

#DEFAULT_PORT = 0x1f90
state requests = shared 0

+total_active(users) : f64 {
    sum user.balance for user in users if user.balance > 0.0
}

#post("/discount")
discount(req: http.Request) : http.Response! {
    body = req.json(User) or err {
        return http.bad_request(.{error: "{err}"})
    }
    remaining = @body.discount(10.0)
    @requests += 1
    http.ok(.{id: body.id, balance: remaining})
}

test "discount is in-place" {
    user = User{id: 7, name: "Amina", balance: 200.0}
    assert_eq(@user.discount(25.0), 150.0)
    assert_eq(user.balance, 150.0)
    assert_eq(#DEFAULT_PORT, 8080)
}
```

## Appendix B. Conformance checklist

A conformance suite should cover every radix and separator form; comments and doc comments; exports/private fields; structs, sums, inferred literals, and layouts; inline trait declarations and `E0203` for a missing method, a wrong arity, a wrong `@`, a wrong parameter type, and a wrong return type; `E0204` for an undeclared language-trait method and for a drifted core trait; `E0205`/`E0206` for ambiguous / duplicate `FuncFamily` overloads; `#symmetric` mirror synthesis; multi-subject `when (e1, e2, …)` Mode 1/2 dispatch; equivalent display, equality, and ordering of arrays, maps, and structs on the VM and the C23 backend; expression and block functions; UFCS and call punning; every loop and reduction spelling; `when` subject and multiplex forms; `ensure`; optionals/errors; tests/routes/state; compile-time bindings/loops/quotes/splices; Pod lock pinning (`Pod.toml` / `Pod.lock` (legacy `Super.toml` removed)); all stable diagnostic codes; scoped-stack rewind; `@` non-escape and cross-spawn rejection; equivalent checksums through VM, JIT, and C23 AOT; and scalar fallback equivalence for every SIMD/OpenMP optimization.

Stdlib conformance additionally requires:

- `find stdlib -name '*.sr' | wc -l` equals **179** (127 implementation modules + `mod.sr` + law/proof sidecars), with `stdlib/mod.sr` listing every importable module. `scripts/count_stdlib.py` must agree with this paragraph and the live tree (CI fails on mismatch).
- Raw OS polling resolves under `std.sys.epoll` / `std.sys.io_uring`; `std.io.poll` is the app-facing readiness loop.
- Windowing resolves under `std.gui.window`; game UDP netcode under `std.game.netcode`.
- Vendor AI/DB modules are absent from `stdlib/` and present as `pods/*/Pod.toml` lib Pods that `super check` / `super test` / `super build --release`.
- New open-standard modules (`std.net.tls`, `std.core.decimal`, `std.ai.tensor`, `std.fmt.yaml`, …) pass the same three backends without E01xx–E05xx diagnostics.
- `super mcp` (and the `super-mcp` shim) answers MCP `initialize` / `tools/list` / `tools/call` for overview, search, features, algorithms, examples, stdlib coverage, and `super_verify` / `super://laws` without leaving the installed toolchain.
- `#law` / `#proof` parse as meta-name attributes; `super verify` discharges them; E0502/E0503 fire on broken laws and incomplete proofs; release `emit-c` / JIT / `.superb` contain **zero** law/proof symbols or bodies.
- Stdlib ships foundational `*.law.sr` / `*.proof.sr` for core reducer and JSON round-trip invariants.

## Module tiers (A2)

| Tier | Meaning | Examples |
|------|---------|----------|
| **kernel** | Required to build and run 0.1.0 programs | language + `std.core` / `std.fmt` / `std.io` / `std.net` (HTTP/TCP/UDP/URL) |
| **stable** | Supported stdlib with tests on VM+JIT+C23 | `std.sys`, `std.data`, `std.math` (non-crypto), `std.db.sqlite`, `std.datetime`, `std.html`, `std.sec.csprng`, `std.fmt.protobuf` |
| **experimental** | Shipped in-tree; API may change; not required for 0.1.0 | self-rolled `std.net.tls` / QUIC, `std.sec.pq`, Argon2id, WebAuthn helpers, SMT/law-proof **checker**, five-provider GUI, game engine |
| **out-of-tree** | First-party Pods under `pods/`, not stdlib | vendor SaaS clients |

Self-rolled cryptography and TLS framing are **never** labeled stable. Prefer OS CSPRNG (`std.sec.csprng`) and vetted TLS binds for harness HTTPS (see `docs/reports/C6-tls-verification.md`).

## Non-`@` reference lifetimes (A1)

Immutable references (`str`, slices, and record fields that borrow stack bytes) are valid only while their owning scoped data stack frame has not rewound. A non-`@` reference **must not escape** its declaring scope by:

| Escape path | Allowed? |
|-------------|----------|
| Return from the declaring function | **No** (E0402) when the referent is stack-owned |
| Store into a heap/module `state` record | **No** (E0402) |
| Send on a channel | **No** (E0402) |
| Capture into a `spawn` closure | **No** (E0402) |
| Capture into a same-scope lambda that does not outlive the frame | **Yes** |
| Copy into an owned `str` / array / struct by value | **Yes** |

`str` slices materialize owned `str` values (destination-driven copy) and may escape; array slices and nested-array element slots remain stack borrows and trigger E0402. `@` mutable roots retain the existing E0401 non-escape rules. Positive (≥5) and negative (≥10) tests live under `crates/super-frontend` lifetime suites (`sema::tests::*e0402*` / `*_ok`).

## SIMD baseline (A6)

On x86-64 the JIT SIMD kernel baseline is **SSE2** (`mulpd`/`addpd` + scalar tail). Performance claims in README and the bench report name this baseline. Raising to AVX2/AVX-512 with runtime feature detection remains optional future work; until then every published claim is SSE2-scoped.

## Platform scope (B5)

Applications should prefer the portable `std.io.poll` façade. Its 0.1.0
platform layer selects Linux epoll with optional io_uring acceleration,
macOS/FreeBSD kqueue, or the documented Windows WSAPoll fallback.
`std.sys.epoll` and `std.sys.io_uring` remain Linux-specific modules and are
selected with `#cfg(target_os = "linux")`.

**Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.**
Networking on Windows goes through the Rust host's WSAPoll fallback (VM and
JIT). The Windows C23 host produces console-safe binaries; an AOT
`App.serve` binary on Windows is not a supported configuration in 0.1.0.

## Laws and tests policy (B8)

Laws/proofs and tests are complementary. Every module that ships a `.law.sr` file must also keep at least one example `test` block exercising the same property on the real artifact (VM, JIT, and C23). CI fails when a law module lacks a paired example test. The proof **checker** remains experimental (A2); law/proof **files** stay in-tree.

## Law/proof coverage (B6)

| Module | Laws | Proofs | Notes |
|--------|------|--------|-------|
| `std.core.reduce` / JSON fmt | yes | yes | foundational round-trip / reducer invariants |
| `std.datetime` | yes | yes | Gregorian leap identity |
| `std.html` / `std.html.core` | yes | yes | escape growth witness |
| `std.sec.csprng` | yes | yes | length non-neg |
| `std.fmt.protobuf` | yes | yes | zigzag inverse |
| `std.core.int` | N/A | N/A | pure numeric helpers; covered by tests |
| `std.ai.mcp_client` / `tokenizer` | N/A | N/A | harness modules; tests only |
| `std.html.parse` / `datetime.cron` | N/A | N/A | Stage 8 modules; tests only |
| `std.html.template` | yes | yes | untrusted Html input escaped except `raw` |
| `std.sec.secret` | yes | yes | Display redaction / zeroize invariant |
| Remaining shipped `stdlib/**` | partial | partial | extend in priority order: fmt parsers → collections → math → data → net |

## UFCS coherence (A9)

When a receiver type defines a method `m`, UFCS `recv.m(...)` binds that method even if a free function `m` is in scope. A library that later adds a method can change the meaning of existing call sites — the hazard example is `points.len` resolving to a new `Vec.len` method instead of a free `len`. Free functions remain reachable via qualified or bare calls without a receiver. Tooling may warn when a newly added method shadows a previously selected free UFCS target.

## Context-sensitive features (A10)

| Feature | Rationale |
|---------|-----------|
| Implicit `it` / `index` | Keeps `for` / `when` / pipeline bodies one expression; always rebound per construct |
| Implicit `err` in `or err` | Names the failed payload without a temporary |
| UFCS | Method discovery without importing every free function |
| Multiple dispatch ranks | Lets stdlib overload on sum tags / traits without open recursion |
| `sum` is an ordinary method | `rows.sum()` is a pipeline method; there is no `sum` keyword |
| Call punning with `.{}` | Inferred struct literals cut boilerplate when the type is known |
| Newline before `+`/`-` ends expressions | Keeps statement boundaries readable; continuation requires the operator at end of line |

## Pod source distribution and `.pod` format (D1–D5)

Pods are distributed as **source**, not binaries: monomorphization, comptime, and statically resolved traits require compiler-visible source (D1).

A `.pod` file is a **tar + zstd** archive (no custom magic; Zstandard magic `0xFD2FB528` identifies compression). The first tar entry must be `pod.manifest` (format version, name, version, Super version requirements). Archives are deterministic (sorted entries, normalized mtimes/owners), reject `..` / absolute paths / escaping symlinks, and enforce documented size limits (D2). CLI: `super pod pack` / `super pod extract` / `super pod verify`.

`Pod.lock` records `content_hash = "sha256:…"` for packed archives; `super pod pack` writes it and `super pod verify` checks it (D3). `super pod pack` builds the archive, extracts into a clean tree, compiles, and runs the VM test suite plus C23 emit/`cc -fsyntax-only` plus JIT compile/run before succeeding unless `--skip-gate` (D4). Naming is consistently `.pod` / `Pod.lock` (D5).
