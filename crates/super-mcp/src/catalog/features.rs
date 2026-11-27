//! Language feature catalog for Super 0.1.0.

use super::Feature;

pub const ALL: &[Feature] = &[
    Feature {
        id: "bindings",
        title: "Bindings & inferred literals",
        category: "syntax",
        summary: "Locals are immutable by default; no semicolons; post-nominal types; multi-binding; inferred struct literals.",
        body_md: r#"# Bindings

```super
+TaxRate = 0.14
host = "localhost"
port: u16 = 8080
+tasks, +next_id, name = [], 1, "Ahmad"
```

- `+Name` at module scope exports; bare names are file-private.
- Type after the name (`port: u16`) when you need a width; otherwise infer `i64` / `f64` / `str` / `bool`.
- Module multi-binding: several names and values on one line; each name may carry its own `+`.
- Statement multi-assign: `a, b = 1, 2` introduces locals; `a, b = b, a` swaps (RHS first).
- Inferred struct literals when the expected type is known:

```super
make_user(id: i64, name: str) : User = .{id, name, balance: 0.0}
```
"#,
        tags: &[
            "let",
            "const",
            "infer",
            "export",
            "struct-literal",
            "multi-binding",
        ],
    },
    Feature {
        id: "collections",
        title: "Arrays, maps, anonymous records",
        category: "syntax",
        summary: "0-indexed arrays, unquoted symbol-key maps, and C-ABI anonymous records with punning.",
        body_md: r#"# Collections

```super
scores = [10, 20, 30, 40]
ports = {http: 80, https: 443, ssh: 22}
headers = {accept: "json", "content-type": "text/plain", [custom_key]: "100"}
config = {host, port, timeout: 30, tls: true}
```

Maps intern unquoted keys as symbols. Use quoted or `[expr]` keys when dynamic.
"#,
        tags: &["array", "map", "record", "punning"],
    },
    Feature {
        id: "structs-sums",
        title: "Named structs & sum types",
        category: "types",
        summary: "Flat C-ABI structs with +/- visibility; variable-sized sum types.",
        body_md: r#"# Structs and sums

```super
+User = {
    id: i64
    name: str
    balance: f64 = 0.0
    -secret_token: str = "internal"

    discount(@self, percent: f64) : f64 {
        cut = self.balance * (percent / 100.0)
        self.balance -= cut
        self.balance
    }
}

Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)
}
```

Inside a `+Struct`, members are public by default; prefix `-` for file-private.
"#,
        tags: &["struct", "enum", "sum", "method", "visibility"],
    },
    Feature {
        id: "functions-ufcs",
        title: "Functions, UFCS, @ mutation, it",
        category: "syntax",
        summary: "No fn keyword; UFCS dots; @ for mutable receivers; implicit it in pipelines.",
        body_md: r#"# Functions

```super
+add_tax(amount: f64) = amount * (1.0 + TaxRate)

final_amount = @u.discount(10.0).add_tax().round(2)
doubled = scores.map(it * 2).keep(it >= 25)
```

`@` on a call receiver is the mutable / in-place path. Pipelines and `when`/`for` bind `it` (and `index` in `for`).
"#,
        tags: &["function", "ufcs", "mutation", "it", "pipeline"],
    },
    Feature {
        id: "control-flow",
        title: "when, ensure, for, if",
        category: "control",
        summary: "Pattern-friendly when, ensure guards, for over arrays/ranges with @ mutation.",
        body_md: r#"# Control flow

```super
when final_amount {
    it > 1000.0 => print("Tier 1: {it}")
    it > 100.0 => print("Tier 2: {it}")
    else => print("Tier 3: {it}")
}

process(target: User) {
    ensure target.active else { return }
    print("Processing {target.name}")
}

for scores { print("{index}: {it}") }
for @scores { it += 5 }
for 0..100 { print(it) }
```
"#,
        tags: &["when", "ensure", "for", "break", "match"],
    },
    Feature {
        id: "optionals-errors",
        title: "Optionals (?), value errors (!), or",
        category: "types",
        summary: "Zero-cost optionals and value errors with or / or err handlers.",
        body_md: r#"# Optionals and errors

```super
find_user(id: i64) : User? = if id == 1 { u } else { none }
active_user = find_user(2) or .{id: 0, name: "Guest"}

read_port(path: str) : i64! {
    ensure path != "" else { fail "Empty path" }
    return 8080
}
p = read_port("cfg") or 3000
p_checked = read_port("") or err { print("Failed: {err}"); return }
```
"#,
        tags: &["optional", "error", "or", "fail", "none"],
    },
    Feature {
        id: "tests",
        title: "First-class test blocks",
        category: "testing",
        summary: "test blocks with private access; stripped in release builds.",
        body_md: r#"# Tests

```super
test "discount reduces balance accurately" {
    u = User{id: 1, name: "Amina", balance: 200.0}
    new_bal = @u.discount(25.0)
    assert_eq(new_bal, 150.0)
}
```

Run with `super test [path]`. Release builds strip `test` blocks.
"#,
        tags: &["test", "assert", "release"],
    },
    Feature {
        id: "http-routes",
        title: "HTTP routes & JSON APIs",
        category: "backend",
        summary: "post/get route macros with req.json, or err validation, http.Response!.",
        body_md: r#"# Backend routes

```super
#post("/orders")
handle_orders(req: http.Request) : http.Response! {
    body = req.json(CreateOrder) or err { return http.bad_request(.{error: "{err}"}) }
    user = await db.query_one(User, "SELECT * FROM users WHERE id = $1", [body.user_id])
        or return http.not_found(.{error: "User not found"})
    return http.ok(OrderResponse.{order_id: 9001, total: 120.5, status: "confirmed"})
}
```

Use `std.net.http` for typed routes, cookies, middleware, and `App.serve`.
"#,
        tags: &["http", "json", "route", "api", "await"],
    },
    Feature {
        id: "ecs-state",
        title: "ECS & hot-reloadable state",
        category: "game",
        summary: "Persistent state across reloads; for @(pos: Pos), (vel: Vel) in world.",
        body_md: r#"# Game ECS

```super
state world = ecs.World{}

update(dt: f32) {
    for @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}
```

`std.game.ecs` is a sparse-set ECS; `state` survives sub-millisecond module reloads.
"#,
        tags: &["ecs", "state", "game", "hot-reload"],
    },
    Feature {
        id: "metaprogramming",
        title: "Compile-time metaprogramming",
        category: "meta",
        summary: "Comptime bindings, loops, quotes, and splices evaluated during analysis.",
        body_md: r#"# Metaprogramming

Compile-time bindings use `#Name`. Quotes and splices expand before codegen. Macro Pods (`kind = "macro"`) ship reusable comptime helpers. See SPECIFICATION §3.9 and §7.
"#,
        tags: &["comptime", "macro", "quote", "splice"],
    },
    Feature {
        id: "pods",
        title: "Pods, manifests, reproducibility",
        category: "toolchain",
        summary: "Pod.toml lib/bin/macro packages with lockfiles and dependency fetch.",
        body_md: r#"# Pods

```toml
[pod]
name = "my_api"
version = "0.1.0"
kind = "bin"

[build]
backend = "c23"
mode = "aot"
# targets = ["native", "wasm32-wasi"]
```

- `super new` / `super init` scaffold Pods.
- `Pod.lock` pins dependencies; `super fetch` / `super vendor`.
- `[build] target` / `targets` and `[target.<triple>]` select compile destinations (`super targets`).
- `.pod` archives are tar+zstd (`super pod pack` / `extract` / `verify`); `Pod.lock` records `content_hash`.
- Vendor SaaS clients live under `pods/` as first-party lib Pods (openai, neon, …), not in stdlib.
"#,
        tags: &[
            "pod",
            "package",
            "lock",
            "dependency",
            "target",
            "wasm",
            "archive",
        ],
    },
    Feature {
        id: "backends",
        title: "VM, JIT, and C23 AOT backends",
        category: "runtime",
        summary: "Default c23/aot native builds; bytecode VM; native JIT; Stage-1 AOT composite #1 / JIT Cycle #1.",
        body_md: r#"# Execution backends

- Default: `super build` → C23 AOT → native binary under `target/<profile>/`.
- `--mode bytecode` → portable `.superb` module.
- `--backend jit` / `--jit` for hot paths (Cycle category leader under Protocol 2.0.0).
- Scoped data stacks (`mmap` arenas) with O(1) request/frame reset.
- Cross / wasm: `--target <TRIPLE>` or `[build] target` / `targets = […]` (see `super targets`).
  Host stays at `target/<profile>/`; others nest under `target/<triple>/<profile>/` (`.wasm` for WASI / freestanding).
  Drivers: `zig cc -target …` preferred, else `clang --target=…`. `[target.<triple>]` overrides `cc` / link flags.

Profiles: `dev`, `test`, `release`, `size`, `fast`. Release strips tests and enables fat LTO / `-O3`.

Stage-1 (Protocol 2.0.0): Super AOT composite **89.2 #1**; Zig wins Speed; Super JIT **Cycle #1**. See feature `stage1-benchmarks`.
"#,
        tags: &[
            "c23",
            "aot",
            "vm",
            "jit",
            "wasm",
            "cross",
            "performance",
            "stage1",
        ],
    },
    Feature {
        id: "stage1-benchmarks",
        title: "Stage-1 suite standings (Protocol 2.0.0)",
        category: "performance",
        summary: "Super AOT composite 89.2 #1; Zig wins Speed; Mem/Rel/LoC → SuperAot; Cycle → Super JIT; LAW-BENCH-001.",
        body_md: r#"# Stage-1 verified standings

Equal-algorithm suite under `benchmarks/` (20 kernels). **LAW-BENCH-001**: gated Super sources must not `use host` or call host FFI.

| Metric | Result |
|---|---|
| Protocol | **2.0.0** (2 warm-ups discarded, 10 interleaved timed runs, median wall) |
| Composite #1 | Super C23 AOT **89.2** (89.23 raw) |
| Speed | **Zig 100.0** vs SuperAot **97.87** |
| Memory / LoC | Super AOT **100** each (LoC **2198** vs Bun **2200**) |
| Reliability | **All** (100% pass every language) |
| Cycle #1 | Super JIT **100** |
| Site | top-level `docs/` (GitHub Pages) — `python3 benchmarks/report_page.py` |
| Stdlib vs host | stdlib = `stdlib/**/*.sr` only; builtins in `crates/super-host` |

Integrity: equal algorithms; no Super bench host kernels; no Body-alias AOT regressions. Details: `docs/REPORT.md`, `docs/stage1_speed_walls.md`, `SPECIFICATION.md` §11.
"#,
        tags: &[
            "stage1",
            "benchmark",
            "aot",
            "jit",
            "loc",
            "law-bench",
            "docs",
        ],
    },
    Feature {
        id: "ffi-interop",
        title: "Host FFI & C/C++/Rust interop",
        category: "interop",
        summary: "Bind host functions into Super; embed Super in C/C++/Rust hosts.",
        body_md: r#"# FFI

From Super, call host-registered functions as `host.name(...)`.

Embed Super:
- C: `super.h` — `super_engine_new`, `super_bind_fn`, `super_load_file`, `super_call_sig`
- C++: `super.hpp` — `super::Engine` with lambda bindings
- Rust: `super_ffi::{Engine, Value}` (never `use super::` — `super` is a Rust keyword)

See `examples/interop_c`, `interop_cpp`, `interop_rust`.
"#,
        tags: &["ffi", "c", "cpp", "rust", "embed"],
    },
    Feature {
        id: "option-pipeline",
        title: "Iterable reductions (trait pipelines)",
        category: "stdlib",
        summary: "AssociativeReducer + Iterable/ParIterable: fused keep/map/sum/mean without keyword reductions.",
        body_md: r#"# Iterable reductions

Keyword `sum for` is gone. Arrays and ranges witness `Iterable` / `ParIterable`
(`std.core.traits`). Reductions are ordinary UFCS methods; the compiler fuses
`.keep` / `.map` into a single counted loop (O(n) time, O(1) aux space).

```super
total = scores.keep(it >= 25).map(it * 2).sum()
mu = samples.mean()
var = samples.variance()
prod = (1..6).product()
hi = xs.max()
any_neg = xs.any(it < 0)
```

Custom monoids use `AssociativeReducer` (`identity` / `accumulate` / `merge` /
`finalize`) from `std.core.reduce`. Parallel form: partition with `par`, then
`.sum()` / `.par_sum()` / `merge` across workers.

Order statistics (`median`, `quantile`) are slice-only in `std.math.stats`
(introselect). Element-wise `exp` / `sqrt` / trig live in `std.math.core`.
"#,
        tags: &[
            "map",
            "keep",
            "sum",
            "mean",
            "Iterable",
            "AssociativeReducer",
            "pipeline",
            "simd",
        ],
    },
    Feature {
        id: "visibility",
        title: "+/− visibility rules",
        category: "modules",
        summary: "Top-level private by default; + exports; struct members public by default; - private.",
        body_md: r#"# Visibility

- Module items: private unless prefixed with `+`.
- Inside `+Struct`: fields/methods public by default; `-` keeps file-private (visible to `test` blocks).
- Leading `+` on a struct member is redundant.
"#,
        tags: &["visibility", "export", "private", "module"],
    },
    Feature {
        id: "string-interp",
        title: "String interpolation",
        category: "syntax",
        summary: "\"Hello {name}\" interpolation in string literals.",
        body_md: r#"# Interpolation

```super
print("Processing {target.name} with balance {target.balance}")
```
"#,
        tags: &["string", "interpolation", "format"],
    },
    Feature {
        id: "concurrency",
        title: "Structured concurrency & async",
        category: "runtime",
        summary: "Stackless async/await for I/O; structured spawn; no escaping @ borrows across spawn.",
        body_md: r#"# Concurrency

`await` on I/O futures (DB, HTTP). Structured concurrency rejects `@` borrows that would escape a spawn boundary (root-lifetime tracking). Prefer request-scoped stacks that rewind at the end of each handler.

Channel multiplexing uses `select { recv => … after dur => … }` (A3). Subject-less `when { … }` remains a legacy alias for the same construct.
"#,
        tags: &["async", "await", "spawn", "lifetime", "select", "channel"],
    },
    Feature {
        id: "diagnostics",
        title: "Diagnostic codes & check",
        category: "toolchain",
        summary: "super check / LSP publish E01xx–E05xx for syntax, types, mutation, lifetimes.",
        body_md: r#"# Diagnostics

`super check` runs lex/parse/sema and syntax-checks emitted C23. Editors use `super lsp` for the same frontend with hover, completion, semantic tokens, and `@` / lifetime diagnostics.
"#,
        tags: &["check", "lsp", "error", "diagnostic"],
    },
    Feature {
        id: "mcp-server",
        title: "Embedded MCP server",
        category: "toolchain",
        summary: "super mcp exposes language features, examples, algorithms, and stdlib to coding agents.",
        body_md: r#"# MCP

```bash
super mcp
```

stdio MCP (JSON-RPC 2.0, newline-delimited). Tools: `super_overview`, `super_search`, `super_get_feature`, `super_list_stdlib`, `super_get_algorithm`, `super_get_example`, and more. Shim binary: `super-mcp`.
"#,
        tags: &["mcp", "agent", "docs", "cli"],
    },
    Feature {
        id: "stdlib-layout",
        title: "Stdlib 126-module layout",
        category: "stdlib",
        summary: "17 layers, 126 importable modules + mod.sr; vendor SaaS in pods/.",
        body_md: r#"# Stdlib layout

Layers: core, fmt, sys, datetime, io, net, db, sec, math, data, game, cli, ai, tui, gui, html, diag.

Notable placements:
- Raw polling: `std.sys.epoll`, `std.sys.io_uring`
- App readiness: `std.io.poll`
- Windowing: `std.gui.window`
- Game netcode: `std.game.netcode`
- Columnar analytics: `std.data.frame`, `std.data.plan`, `std.fmt.xlsx`
- Scientific math: `std.math.linalg`, `std.math.ad`, `std.math.special`
- Vendor AI/DB: `pods/openai`, `pods/neon`, …
"#,
        tags: &["stdlib", "layers", "pods", "data", "math"],
    },
    Feature {
        id: "multiple-dispatch",
        title: "Static & sum-tag multiple dispatch",
        category: "language",
        summary: "FuncFamily overloads, trait-constrained params, #symmetric mirrors, and when (a, b) dispatch tables.",
        body_md: r#"# Multiple dispatch

```super
#symmetric
+add(a: i64, b: f64) : f64 => a.to_float() + b

when (x, y) {
    (0, 0) => 0
    (LinOp.Dense, LinOp.Diag) => dense_diag(x, y)
    else => fallback(x, y)
}
```

- Overloads differ by arity, types, trait bounds, or `@`.
- Specificity: Concrete > Variant > Trait > `_`; `E0205` on ambiguity, `E0206` on duplicates.
- `#symmetric f(a: T1, b: T2)` synthesizes `f(b, a) => f(a, b)`.
- `when (a, b)` also dispatches on plain integer tags, not only sum variants. Stdlib example
  (`std.game.physics2d`): `when (a.shape, b.shape) { (0, 0) => circle_circle(a, b)  (1, 0) => p2_flip(circle_box(b, a)) ... }`
  keeps the mixed pair symmetric with one mirrored arm and a negated normal.
- Bytecode, JIT, and C23 AOT each keep a distinct chunk slot per overload (AST param tags) and
  rank call sites by argument tags, so `#symmetric` mirrors and same-name overloads run everywhere.
  `std.math.linalg` still uses tuple `when` for `LinOp` kind matrices (dense/diag/CSR) because that
  dispatch is on runtime tags, not static types.
- Ambiguous Dyn UFCS (same short name on ≥2 type owners, e.g. `Cell.text` vs `StringBuilder.text`)
  does not silently pick an arbitrary owner; prefer a typed receiver or a free helper such as
  `cell_text(c)` / field access. Heavy coverage: `tests/dispatch_family.sr`.
"#,
        tags: &[
            "dispatch",
            "symmetric",
            "when",
            "overload",
            "FuncFamily",
            "tuple-when",
            "physics2d",
        ],
    },
    Feature {
        id: "stdlib-performance-patterns",
        title: "Stdlib performance patterns",
        category: "stdlib",
        summary: "General-purpose hot-path idioms used across std: preallocation, in-place @ helpers, #simd inner loops, spatial hash, prefilters, popcount/ctz.",
        body_md: r#"# Stdlib performance patterns

Patterns already used in `stdlib/` (none are benchmark-specific):

- **Preallocate**: `out = [0; n]` then `out[i] = v` instead of `arr_push` growth (`sec.sha2` `sha2_pad`, `sha2_bytes_of`; `ai.tensor` `tn_matmul`).
- **In-place `@` helpers**: `sha2_schedule_into(padded, at, @w)` and `sha2_block_into(@hv, w)` reuse one schedule buffer across blocks.
- **Loop order + hoisted strides**: `tn_matmul` runs i-k-j with the inner row streamed under `#simd`; per-element sums keep k-ascending order, so results are bit-identical to the naive form.
- **Spatial hash broadphase**: `game.physics2d` `p2_solve_grid` hashes padded AABBs into a uniform grid and visits candidate pairs in the same (i, j) order as the O(n^2) sweep (`p2_step_brute` is the reference).
- **No-allocation fast paths**: `net.http` `http_ieq` compares bytes in place, `canonical_header` returns its input when already canonical, `req_header` stops at the first hit, `pattern_capture` allocates a result only when it returns one.
- **Prefilters**: `core.regex` records the required first literal (`Regex.first`) so scanners reject most offsets with one comparison.
- **Word tricks**: `core.bitset` uses `bs_popcount32`, `bs_ctz32` (`v & -v`, popcount of the bits below) and `bs_msb32` to skip empty words and peel set bits.
"#,
        tags: &[
            "performance",
            "simd",
            "preallocation",
            "spatial-hash",
            "popcount",
            "prefilter",
            "restrict",
        ],
    },
    Feature {
        id: "scientific-math",
        title: "Scientific computing (std.math)",
        category: "stdlib",
        summary: "Algebra traits, special functions, AD, optimization, ODE/quadrature, FFT, stats, and structured linalg.",
        body_md: r#"# std.math

Import surfaces: `algebra`, `special`, `ad`, `optimize`, `integrate`, `interp`, `poly`, `linalg`, `fft`, `stats`, `rand`, `geom`.

```super
use std.math.ad
use std.math.special
d = Dual64.{v: 1.2, d: 1.0}
g = gamma(d.v)
```
"#,
        tags: &["math", "ad", "linalg", "fft", "optimize"],
    },
    Feature {
        id: "polars-data-pipeline",
        title: "Polars-grade columnar data pipeline (std.data)",
        category: "stdlib",
        summary: "LazyFrame/Frame/Expr, joins/group_by/windows, query optimizer, CSV/JSON/xlsx I/O via std.fmt.*",
        body_md: r#"# std.data (Polars-grade)

```super
use std.data.frame
use std.data.plan
use std.data.io
lf = LazyFrame.scan("events.csv").filter(col("amount").gt(lit(0.0)))
f = lf.collect() or Frame.new()
g = f.group_by(["region"]).agg([agg_sum("amount", "total")])
```

Layers: `column` → `expr` → `frame` → `plan` (lazy + `collect_streaming`) → `io` (csv/json/xlsx/…).
Windows: `.over(partition_by, order_by)` · `join_asof` · `group_by_dynamic`.

Hot paths: typed column cmp filters use `sys_arr_truncate` after dense scans; streaming
`collect_streaming` skips empty post-filter batches so partial `group_by` frames keep a stable schema
under `fr_concat`. Stringify Dyn cells with `cell_text` (not ambiguous `.text`).
"#,
        tags: &[
            "data",
            "dataframe",
            "polars",
            "csv",
            "xlsx",
            "lazy",
            "window",
        ],
    },
];

pub fn find(id: &str) -> Option<&'static Feature> {
    ALL.iter()
        .find(|f| f.id == id || f.title.eq_ignore_ascii_case(id))
}

pub fn search(query: &str) -> Vec<&'static Feature> {
    ALL.iter()
        .filter(|f| {
            let blob = format!(
                "{} {} {} {} {} {}",
                f.id,
                f.title,
                f.category,
                f.summary,
                f.body_md,
                f.tags.join(" ")
            );
            super::matches_query(&blob, query)
        })
        .collect()
}
