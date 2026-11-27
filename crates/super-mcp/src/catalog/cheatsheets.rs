//! Compact Super cheatsheets by topic.

#[derive(Clone, Copy)]
pub struct Cheatsheet {
    pub id: &'static str,
    pub title: &'static str,
    pub body_md: &'static str,
}

pub const ALL: &[Cheatsheet] = &[
    Cheatsheet {
        id: "syntax",
        title: "Syntax essentials",
        body_md: r#"# Syntax essentials

| Idea | Spelling |
|------|----------|
| Export | `+Name = ...` |
| Typed local | `port: u16 = 8080` |
| Struct literal | `.{id, name}` |
| Mutable call | `@obj.method()` |
| Optional | `T?`, `none`, `x or default` |
| Error | `T!`, `fail "msg"`, `x or err { ... }` |
| Panic | `panic`, `panic "msg"`, `#todo`, `#unimplemented`, `#unreachable`, `#assert`, `#dbg` |
| Test | `test "name" { ... }` |
| Range for | `for 0..n { ... }` |
| Iterable reduce | `xs.keep(...).map(...).sum()` / `.mean()` / `.par_sum()` |
| Interp | `"hi {name}"` |
"#,
    },
    Cheatsheet {
        id: "reductions",
        title: "Trait-based reductions",
        body_md: r#"# Trait-based reductions

| Method | Notes |
|--------|-------|
| `sum` / `product` / `min` / `max` | Associative; fuse with `map`/`keep` |
| `any` / `all` | Optional predicate `xs.any(it > 0)` |
| `mean` / `variance` | Welford single-pass |
| `reduce(init, acc + it)` | Custom fold; `acc` + `it` bound |
| `par_sum` / `par_mean` / … | ParIterable; OpenMP on C23 when pure |
| `median` / `quantile` | `std.math.stats` on contiguous slices only |

```super
use std.core.reduce
partials = [0; workers]
par w in 0..workers { partials[w] = chunk(w).sum() }
total = partials.sum()
```
"#,
    },
    Cheatsheet {
        id: "imports",
        title: "Common imports",
        body_md: r#"# Common imports

```super
use std.core.str
use std.fmt.json
use std.net.http
use std.game.ecs
use std.cli.cmd
use std.diag.test
use std.io.poll
use std.sys.time
```

Vendor Pods (not stdlib): `use openai`, `use neon`, … after declaring the Pod dependency.
"#,
    },
    Cheatsheet {
        id: "cli",
        title: "CLI commands",
        body_md: r#"# CLI

```
super new my_app [--bin|--lib|--macro]
super run [file.sr] [--jit]
super test [path]
super check [file.sr]
super build [--release] [--backend c23|jit|vm] [--mode aot|bytecode]
super build --target wasm32-wasi
super build --target aarch64-unknown-linux-gnu -r
super targets      # list linux/windows/macos/android/wasm triples
super lsp
super mcp          # agent knowledge server (stdio MCP)
super doc [path]
super fmt [path]
super install / uninstall / list / which
```

`[build] target` / `targets = […]` in Pod.toml select destinations; artifacts land under
`target/<triple>/<profile>/` (host keeps `target/<profile>/`). Cross/wasm prefer `zig cc`.
"#,
    },
    Cheatsheet {
        id: "types",
        title: "Types & visibility",
        body_md: r#"# Types

Primitives: `i64`, `u16`, `f64`, `f32`, `bool`, `str`, `none`.
Structs: `{ field: Type = default }`.
Enums / sums (no `enum` keyword): `Status = { Pending; Paid(amount: f64); is_ok(self) : bool { … } }`.
Construct with `Status.Pending` / `Status.Paid(49.95)`; match with `when s { Status.Paid { amount } => … }`.
Methods live on the sum like structs. Unit variants coerce to `i64` tags (HTTP `Mw` / route `Handler`).
Middleware is the `Mw` sum (`Mw.Logger`, `Mw.Bearer(token)`); defaults are `before`/`after`; override via `App.replace` / `remove`.
Visibility: module private default; `+` export; struct members public; `-` private.
"#,
    },
];

pub fn find(id: &str) -> Option<&'static Cheatsheet> {
    ALL.iter()
        .find(|c| c.id == id || c.title.eq_ignore_ascii_case(id))
}
