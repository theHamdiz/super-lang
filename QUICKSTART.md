# Super 0.1.0 in 60 Minutes

> Rapid field guide. Full book: [`BOOK.md`](https://github.com/theHamdiz/super-lang/blob/main/BOOK.md) / [`SUPER_BOOK.pdf`](https://thehamdiz.github.io/super-lang/downloads/SUPER_BOOK.pdf). Normative: [`SPECIFICATION.md`](https://github.com/theHamdiz/super-lang/blob/main/SPECIFICATION.md).

Install (toolchain → `~/.super`, not `~/.cargo`):

```bash
sh -c 'f=$(mktemp) && trap "rm -f -- \"$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh -o "$f" && sh "$f" "$@"' sh
# or from a checkout: ./scripts/install.sh
```

This guide teaches Super 0.1.0 as shipped: scoped data stacks, `@` mutation, VM + SIMD JIT, C23 AOT, traits (`E0203`), and the example suite (`examples/01`–`examples/30`).

```bash
super check examples/01_space_shooter/src/main.sr
super test examples/
super run examples/03_log_analyzer/src/main.sr
super build --release --c23 examples/08_kv_cache_engine/src/main.sr
```



# Focus — Conciseness, Sugar, Parallelism, Comptime

Super’s density is intentional. The print edition (`SUPER_BOOK.pdf`) is a token-compact language for people and coding agents: scoped stacks, memory-safe by construction, C and Rust interop, and native code aimed past idiomatic C. Four ideas organize the book, and the bullets under them are the rest of the surface.

1. **Conciseness** — no `fn`/`let`/`var`/semicolon tax; `+` exports; inference; `Iterable` pipeline reductions; postfix `if`.
2. **Syntactic sugar with teeth** — `@` means an in-place, `restrict`-style write; `[T; N]` means stack/SoA storage; `=>` marks pure leaves; `T?`/`T!` force handling.
3. **Parallelism** — `par` / `ParIterable.par_sum()` for independent work; `for @` updates elements; structured `spawn`/`scope` without a shared heap.
4. **Comptime** — `#NAME`, `#for` tables, `#inline` / `#unroll` specialize into C23 `constexpr` / `.rodata` with zero startup cost.

The forms around `for`, `for @`, and `par`. The rest of the surface is the same length:

- `for` walks a range, an array, or a pipeline.
- `for @xs` writes each element in place.
- `#for` repeats the loop at compile time.
- `par` states that the iterations are independent.
- `Iterable.sum()` and `ParIterable.par_sum()` (from `std.core.traits`) are the reductions.
- `?` is an optional and `!` is a value error.
- `or` recovers from either in line.
- `ensure` leaves through its `else` block.
- `when` matches a value against arms.
- `+` exports the next declaration. There is no export list.
- `value.method(args)` is `method(value, args)`.
- Pipelines read left to right, and the current value is `it`.
- `.{}` builds a value when the type is already known.
- Structs line up with C. Sums stay a tag plus a payload.
- `+Name = trait { … }` is a method contract. `+Name: Trait = { … }` declares it on the type and writes the methods there. `Display`, `Eq`, `Ord`, `Add`, and the other language traits are ambient: declaring one replaces the builtin witness for `"{…}"`, `==`, and `+`.
- `test` blocks see private items and are stripped from a release build.
- `#` names, `#unroll`, and `#assert_size` run before the program starts.
- `scope`, `spawn`, and `await` structure concurrent work on the same stacks.
- `"{expr}"` interpolates through `Display`. Arrays, maps, structs, and commands render by that witness. A type that declares `Display` or `ToString` replaces the builtin. Raw strings do not interpolate, and `\{` still forces a literal brace.
- Command-line parsing, a terminal UI, and a desktop UI sit on these same stacks.

```super
+N = 1024
+square(x: i64) => x * x

+main() {
    xs = [0; N]
    par i in 0..N { xs[i] = square(i) }
    print("CHECKSUM={xs.sum()}")
}
```

The chapters below expand these themes with full examples, benchmarks, and the thirteen sample projects.

# Part I — Foundations & Philosophy

## 1. Why Super exists

Super is a token-compact language for coding agents and for the people who review what those agents write. Ceremony is what was removed: no `fn`, no `let`, no semicolons, and a type annotation only when inference would guess wrong. An agent spends its tokens on the program.

Memory is a scoped data stack, in the same family as Goose. A block, a task, a request, or a frame owns what it creates. A bump cursor allocates, and one rewind releases the whole region. The ordinary path has no tracing collector and no free-list walk.

That stack is memory-safe by construction. The checks happen before the program runs:

1. Values are local and immutable until the program opts in. There is no hidden mutable alias.
2. `@` is the only in-place write, and it is visible on the parameter and at the call.
3. A mutable view cannot escape the scope that owns the bytes (`E0401`), so a dangling pointer is a compile error.
4. Running out of stack is a checked failure. It does not overwrite the neighboring region.
5. Structs are C layouts with no object header, arrays are contiguous, and sums are a tag plus an inline payload.

C and Rust meet that layout. Structs are C-ABI values. The AOT path emits C23, and a Rust host function registers in one line, including a shared slice. The VM, the native JIT, and the C binary agree on results. On dense kernels the native paths are built to meet and pass idiomatic C; the benchmark checksums are the comparison. The same minimal syntax is for command-line tools, backends and APIs, games, automation, and the programs in between.

Strings, arrays, maps, channels, and host handles still need storage. “Zero-heap” means the ordinary path uses scoped stacks, static storage, caller destinations, and explicit host resources.

## 2. The execution stack

A Super source file passes through a zero-copy-oriented lexer, an arena-shaped AST, semantic analysis, and lowering. It can then run in three related ways:

- The VM executes compact register bytecode and is the universal fallback.
- The native JIT recognizes useful kernels and emits executable x86-64 or AArch64 code.
- The C23 backend emits self-contained C and invokes the host compiler for optimized AOT binaries.

C23 is the default AOT standard. Generated code can use `constexpr`, `<stdbit.h>`, `[[unsequenced]]`, `restrict`, and recognized OpenMP parallel loops. This is not a “C syntax mode” exposed to Super programmers. It is a portable optimization and interoperability substrate.

The JIT does not attempt to be a second giant optimizing compiler. It has a deliberately focused model: find canonical dense loops with safe alias facts, emit packed instructions, handle the scalar tail, and fall back when a shape is not recognized. The C compiler remains the broad optimizer for release AOT.

## 3. Design commitments

Super keeps a small set of rules that show up everywhere:

- **Scoped data stacks** own temporary aggregates; leaving a scope rewinds storage in O(1).
- **`@` marks in-place mutation** at the call site and on mutable parameters; the compiler derives non-aliasing from that contract.
- **Flat C-ABI structs and compact sums** keep layouts deterministic for AOT and FFI.
- **`#` is compile time** — typed bindings, loops, quotes, and attributes, not token pasting.
- **Routes, tests, UFCS pipelines, and structured concurrency** (`scope` / `spawn` / `chan` / `shared`) are part of the language, not bolted on later.

C23 is the default AOT substrate (the host compiler sees `restrict`, OpenMP, and dense arrays). The VM and JIT are the other two execution paths. Performance claims should always be tied to a workload, machine, and checksum gate; the repository’s verified kernel suite is evidence for that run, not a universal law.

## 4. Your first module

Every `.sr` file is a module. There is no package declaration and no `fn` keyword.

```super
/// Exported greeting.
+greet(name: str) => "Hello, {name}!"

private_suffix = "from Super"

+main() {
    message = greet("Amina")
    print("{message} {private_suffix}")
}

test "greeting interpolates" {
    assert_eq(greet("Amina"), "Hello, Amina!")
}
```

The `+` exports a declaration. `private_suffix` is visible only inside the module and its tests. Run:

```bash
super check hello.sr
super test hello.sr
super run hello.sr
```

The toolchain’s stable diagnostics are part of the language experience. Undefined names are E0101, mismatched types E0201, missing mutation borrows E0301, and escaping mutable roots E0401. Read the code, not only the prose: the highlighted span normally identifies the shortest correction.

# Part II — Complete Language Syntax

## 5. Bindings, literals, and operators

Bindings infer their type:

```super
host = "127.0.0.1"
port: u16 = 8080
+MAX_CLIENTS = 65_536
flags = 0xff_00 | 0b1010
ratio = 1.5e2
```

The `+` on `MAX_CLIENTS` exports it; it is not unary plus. Ordinary statements need no semicolon. Decimal, hexadecimal, binary, and octal integers accept `_`. Strings support interpolation:

```super
print("listening on {host}:{port}, flags={flags}")
```

Escapes are `\n \r \t \0 \\ \" \' \{ \}`, `\xNN` (ASCII) and `\u{1F600}`; anything else is a syntax error. For backslash-heavy text use a raw string, `r"C:\Users\me"` or `r#"say "hi" \d+"#`, which has no escapes and no interpolation. `print` and `println` both end the line (`println` is the explicit name; `print()` / `println()` alone emit a blank line).

Arithmetic, comparison, bitwise, shift, logical, and assignment operators follow familiar precedence. `&&` and `||` short-circuit. `..` creates a half-open iteration range in canonical loops.

Use postfix guards when they make an early exit clearer:

```super
+clamp_nonnegative(x: i64) : i64 {
    return 0 if x < 0
    x
}
```

## 6. Arrays, maps, records, and slices

Arrays are contiguous and zero-indexed:

```super
primes = [2, 3, 5, 7]
pixels = [0; 1920 * 1080]
middle = primes[1..3]
```

`[value; N]` is the dense stack-array repeat form. The compiler knows the element kind and length and can remove checks in canonical loops.

Brace literals intentionally cover two data shapes:

```super
ports = {http: 80, https: 443}
headers = {"content-type": "application/json", [custom_name]: "yes"}
config = {host, port, timeout: 30}
```

Quoted or computed keys force a map. Identifier fields create an anonymous record and support punning. Named and inferred struct literals use:

```super
user = User{id: 7, name: "Amina"}

+make_user(id: i64, name: str) : User  => .{id, name, balance: 0.0}
```

The second form works because the return type supplies the expected `User`.

## 7. Structs, methods, and sum types

```super
+Account = {
    id: i64
    balance: f64 = 0.0
    -token: str = ""

    deposit(@self, amount: f64) : f64 {
        self.balance += amount
        self.balance
    }
}

Payment = {
    Pending
    Settled(amount: f64)
    Rejected(reason: str)

    amount_or(self, fallback: f64) : f64 {
        when self {
            Payment.Settled { amount } => amount
            else => fallback
        }
    }
}
```

Struct fields have deterministic offsets and flat layout. In a public struct, `-token` remains private. Tests in the defining module can inspect private fields. Sums may declare methods the same way; `when` arms can bind payloads with `Payment.Settled { amount }`.

Methods are ordinary statically resolved functions owned by a type. UFCS lets free functions read like methods too:

```super
balance = @account.deposit(20.0).round(2)
```

UFCS works on any receiver (numbers, strings, arrays, structs) and chains left to right. The chain and the nested call are the same program:

```super
print("{49.0.sqrt().floor().to_int()}")   // 7
print("{to_int(floor(sqrt(49.0)))}")      // 7, identical
n = "Hi".byte_len()                         // 2
```

The leading `@` is visible proof that the pipeline may mutate `account`. Missing it is E0301.

A trait is signatures only. The struct names it in the header and writes the bodies once:

```super
+Mark = trait {
    paint(self) : str
}

+Pin: Mark = {
    label: str = ""
    paint(self) : str  => self.label
}
```

`std.gui.widgets` defines `Widget` (`paint`, `hit`, `measure`). `Toggle`, `Dialog`, and `Sprite` in `std.game.fx` declare it the same way. A missing method, a wrong `@`, a wrong parameter type, or a wrong return type is E0203. Calls stay static.

Sum values use a tag and inline payload. They make state transitions explicit without allocating a class hierarchy:

```super
status = Payment.Settled(49.95)
when status {
    Payment.Pending => print("waiting")
    Payment.Settled => print("paid")
    else => print("failed")
}
```

## 8. Functions, inference, and monomorphization

Functions can be expressions or blocks:

```super
+square(x: f64) => x * x
+cube(x: f64) => x * x * x

+distance(x1: f64, y1: f64, x2: f64, y2: f64) : f64 {
    dx = x2 - x1
    dy = y2 - y1
    (dx * dx + dy * dy).sqrt()
}
```

`=>` carries an inline preference. Untyped or `_` parameters are specialized at call sites:

```super
+identity(value) => value
```

Calling `identity(3)` and `identity("x")` produces concrete specializations rather than requiring a runtime boxed union. Compile-time parameters start with `#` and also participate in specialization.

Call punning reduces boilerplate when scope names match parameter names:

```super
+spawn_wave(world, count, speed) { /* ... */ }
world = make_world()
count = 20
speed = 1.5
spawn_wave(.)
```

Use it only when the names communicate the call clearly.

## 9. Mutation and scoped lifetime

Super makes mutation a protocol:

```super
+Account = {
    balance: f64

    discount(@self, percent: f64) : f64 {
        self.balance -= self.balance * percent / 100.0
        self.balance
    }
}

acct = Account{balance: 200.0}
remaining = @acct.discount(10.0)
```

The method says this value receives exclusive mutable access. The call repeats the fact on the receiver. During that dynamic extent, the region cannot be mutably aliased. This supports both human review and `restrict`/SIMD lowering.

Most temporary aggregate data lives on a scoped data stack:

```super
scope {
    scratch = [0.0; 4096]
    for @scratch { it = index * 0.5 }
    consume(scratch)
} // one rewind releases scratch and other temporaries
```

Do not return a mutable borrow into `scratch` or persist it in `state`; the compiler rejects the escape. Destination-driven construction can place a returned aggregate directly in caller-owned storage, avoiding an intermediate copy.

## 10. Loops, implicit names, and reductions

The shortest loop uses implicit `it` and `index`:

```super
for scores { print("{index}: {it}") }
for @scores { it += 5 }
for 0..100 { consume(it) }
```

Named binders improve nested loops:

```super
for row in 0..height {
    for col in 0..width {
        pixels[row * width + col] = shade(row, col)
    }
}
```

`while`, `loop`, `break`, and `continue` cover stateful control:

```super
i = 0
while i < n {
    i += 1
    continue if disabled[i]
    break if ready(i)
}
```

Reductions make intent available to the optimizer:

```super
total = values.sum()
energy = sum v * v for v in velocities
positive = values.keep(it > 0).sum()
checksum = (0..n).map(data[it]).par_sum()
```

The destination form is useful when a name is consumed later:

```super
checksum = rows.keep(it.valid).sum()
```

The compiler can lower recognized parallel reductions to worker-local accumulators and OpenMP on the C23 path.

## 11. Parallel loops and structured concurrency

Use `par` for independent data parallelism:

```super
par y in 0..height {
    rows[y] = render_row(y, width)
}
```

Use `scope` and `spawn` for task parallelism:

```super
scope {
    left = spawn fetch("/left")
    right = spawn fetch("/right")
    combine(await left, await right)
}
```

All spawned work completes before scope exit. A bounded channel transfers ownership:

```super
events = chan[Event](256)
scope {
    producer = spawn produce(events)
    consumer = spawn consume(events)
    await producer
    events.close()
    await consumer
}
```

`shared 0` creates supported atomic/futex-backed shared state. Prefer channel ownership transfer; use shared cells for genuinely shared counters or coordination. Two sibling spawns may not capture the same `@` root; E0302 prevents a race from becoming an optimizer alias violation.

`when` can multiplex events and time:

```super
when {
    events.recv() => handle(it)
    after 16.ms => update()
}
```

## 12. Conditionals, `when`, and `ensure`

`if` is an expression:

```super
label = if score > 100 { "high" } else { "normal" }
compact = active ? "on" : "off"
```

`when` selects the first matching arm and exposes the subject as `it`:

```super
when latency_us {
    it < 100 => metrics.fast()
    it < 1000 => metrics.normal()
    else => metrics.slow()
}
```

`ensure` keeps the successful path unindented:

```super
+process(user: User) {
    ensure user.active else {
        log.warn("inactive {user.id}")
        return
    }
    charge(user)
}
```

## 13. Optionals and value errors

`T?` represents `T` or `none`; `T!` represents `T` or a value error.

```super
+find_port(name: str) : i64?  => if name == "http" { 80 } else { none }

+read_port(path: str) : i64! {
    ensure path != "" else { fail "empty path" }
    8080
}

port = find_port("custom") or 3000
checked = read_port("") or err {
    print("configuration failed: {err}")
    return 3000
}
```

An optional/error may not silently enter a context requiring its success type. E0202 points to the missing handling edge.

## 14. Modules, Pods, tests, routes, and state

Modules are files:

```super
use models
use models.{User, Task}
use std.net.http
use std.fmt.json
```

A **Pod** is a Super project. `Pod.toml` (removed legacy `Super.toml` still works) defines identity, `kind` (`bin` / `lib` / `macro`), path/Git dependencies, and build profiles. `super new --bin`, `--lib`, or `--macro` scaffolds the matching entry (`src/main.sr`, `src/lib.sr`, or `src/macros.sr`). `super build`, `super run`, and `super check` find that file from the working directory, fetch what it lists, and use `[build] profile` when no `-d`, `-r`, or `--profile` was passed. `super test` uses `[profile.test]`. Set `[build] target` / `targets = […]` or pass `--target <TRIPLE>` for cross and WebAssembly builds (`super targets` lists the catalog; wasm artifacts are `.wasm` under `target/<triple>/<profile>/`). `Pod.lock` / `Pod.lock` records resolved Git commits. Commit the lockfile for applications. `super vendor` produces a source snapshot under `vendor/`. Macro Pods distribute comptime with `+#NAME` exports and `#`-parameter specialization functions.

Tests are native grammar:

```super
test "private token is initialized" {
    a = Account{id: 1}
    assert_eq(a.token, "")
    #assert(a.id > 0)
}
```

`test` is a keyword. `#assert` is a macro from `std.core.macros`, in scope everywhere: keywords are the only bare syntax, every user-extensible compile-time construct is a `#` macro, and everything else is a function or a method. Write your own with `#name { (pattern) => { template } }` (see `examples/doc_macros/`).

Routes capture method, path, request name, return type, and body:

```super
#get("/health")
health(req: http.Request) : http.Response! {
    http.ok(.{ok: true})
}
```

`state` declares persistent module-root mutable state:

```super
state world = ecs.World{}
```

The hot-reload runtime may preserve its data stack while replacing code. Root lifetime checks prevent transient mutable borrows from being stored into it.

## 15. Compile-time Super

Compile-time bindings look like ordinary names with a visible `#`. Use `+#` to export them from a macro Pod:

```super
#WIDTH = 64
+#MASK = 0xff
pixels = [0; #WIDTH * #WIDTH]
```

Compile-time control can generate tables:

```super
#HIST_BOUNDS = #for i in 0..6 { i * 200 }

#if #WIDTH >= 64 {
    #simd
    for @pixels { it = index & #MASK }
}
```

Named comptime functions and quote/splice work with typed code. Scaffold a distributable macro Pod with `super new --macro tables`:

```super
+#make_add(#n: i64, x: i64) : i64 = x + #n
+#ANSWER = '{ 6 * 7 }
```

Attributes communicate constrained optimizer/domain facts:

```super
#inline
+mix(a: i64, b: i64) => (a ^ b) * 0x9e37

#unroll(4)
for 0..16 { accum += lanes[it] }
```

Use `embed "assets/logo.bin"` for compile-time file bytes. The dependency is tracked by the meta cache; C23 output can use constant byte tables or `#embed`.

## 16. Standard library map

The shipped library is **100** `.sr` files under `stdlib/` (see `stdlib/mod.sr`). Vendor SaaS clients live in first-party `pods/` lib Pods.

- `std.core.str`, `std.core.bytes`, `std.core.collections`, `std.core.regex`, `std.core.uuid`, `std.core.unicode`, `std.core.decimal`, `std.core.bitset`
- `std.fmt.json`, `std.fmt.csv`, `std.fmt.toml`, `std.fmt.yaml`, `std.fmt.msgpack`, `std.fmt.xml`, `std.fmt.bin`, `std.fmt.compress`, `std.fmt.mime`
- `std.sys.fs`, `std.sys.path`, `std.sys.env`, `std.sys.proc`, `std.sys.time`, `std.sys.epoll`, `std.sys.io_uring`, `std.sys.ffi`, `std.sys.mmap`; CLI apps use `std.cli.cmd` only (not `std.sys.cli`)
- `std.io.poll`, `std.io.buf`, `std.io.fs`, `std.io.stdio`, `std.io.console`, `std.io.socket`, `std.io.archive`
- `std.net.http`, `std.net.ws`, `std.net.url`, `std.net.ip`, `std.net.tcp`, `std.net.udp`, `std.net.dns`, `std.net.sse`, `std.net.retry`, `std.net.tls`, `std.net.smtp`, `std.net.grpc`, `std.net.quic`, `std.net.oauth2`
- `std.db.sql`, `std.db.postgres`, `std.db.mysql`, `std.db.redis`, `std.db.sqlite`, `std.db.migrate`, `std.db.clickhouse`
- `std.sec.crypto`, `std.sec.encoding`, `std.sec.jwt`, `std.sec.sha2`, `std.sec.hash`, `std.sec.pq`, `std.sec.x509`, `std.sec.otp`, `std.sec.argon2`, `std.sec.vault`
- `std.math.core`, `std.math.linalg`, `std.math.rand`, `std.math.noise`, `std.math.complex`, `std.math.quat`, `std.math.stats`, `std.math.geom`, `std.math.fft`
- `std.game.ecs`, `std.game.physics2d`, `std.game.ai`, `std.game.app`, `std.game.assets`, `std.game.tilemap`, `std.game.fx`, `std.game.netcode`, `std.game.audio`
- `std.ai.client`, `std.ai.tensor`, `std.ai.embed`
- `std.diag.test`, `std.diag.log`, `std.diag.metrics`, `std.diag.bench`
- `std.cli.cmd` (alias `std.clap`), `std.tui`, `std.gui`, `std.gui.widgets`, `std.gui.window`. `use std.gui as App` binds the module under `App`; `use std.gui.{Gui as Desk}` adds another name for that export. `Widget` is the inline contract for `paint`, `hit`, and `measure`.

Vendor Pods: `openai`, `claude`, `gemini`, `openrouter`, `deepseek`, `elevenlabs`, `seedance`, `supabase`, `neon`, `cockroachdb`, `mariadb`, `meilisearch`.

On Linux, epoll/io_uring-backed non-blocking primitives support the network modules via `std.io.poll`. JSON supports validation/encoding and zero-tree path extraction. ECS and linear algebra expose layouts suitable for dense kernels.

# Part III — Design Patterns

The syntax in Part II is small; the engineering consequences are not. This part
collects the arrangements that repeatedly produce clear, fast Super programs.
The patterns are deliberately concrete. They describe ownership, layout,
failure, and compilation boundaries rather than abstract class hierarchies.

## 17. Structure of arrays and `@restrict`

An array of records is often convenient at an API boundary:

```super
Body = {
    x: f64
    y: f64
    vx: f64
    vy: f64
}
```

A structure of arrays (SoA) is usually better for a hot integration pass. Values
used together occupy contiguous streams, a field can be traversed without
loading unrelated bytes, and mutable roots are visible in the signature:

```super
+integrate(@x, @y, @vx, @vy, dt: f64) {
    #simd
    for i in 0..x.len() {
        x[i] += vx[i] * dt
        y[i] += vy[i] * dt
    }
}

integrate(@x, @y, @vx, @vy, 1.0 / 60.0)
```

The four `@` parameters are exclusive, non-escaping borrows. When semantic
analysis proves that they do not overlap, C23 AOT can emit `restrict` pointers
and the JIT can select packed operations. `@` is therefore more than a visual
marker: it is a checked optimizer fact.

Use SoA when a system updates one or two fields over many entities. Prefer a
record when a value crosses an FFI boundary, is handled as a unit, or is rarely
scanned. A practical game world often uses both: stable component arrays in
storage and small records at commands, events, and serialization boundaries.

Do not manufacture alias claims. Passing the same array as two mutable
parameters would invalidate the optimization contract and is rejected. If two
logical views overlap, redesign the kernel around one root, split into disjoint
slices, or stage output in a separate destination.

### Double buffering

Neighborhood algorithms often read old state and write new state. Separate
input and output buffers remove loop-carried aliasing:

```super
+steer(px, py, vx, vy, @next_vx, @next_vy) {
    par i in 0..px.len() {
        force = compute_force(i, px, py, vx, vy)
        next_vx[i] = vx[i] + force.x
        next_vy[i] = vy[i] + force.y
    }
}
```

At module scope you can declare several bindings on one line with per-name
export: `+tasks, +next_id, name = [], 1, "Ahmad"`. Inside functions,
`a, b = 1, 2` introduces locals and `a, b = b, a` swaps safely because every
right-hand side is evaluated before any store.

Swap the roots after the pass. Multi-assignment evaluates right sides before
stores, so `vx, next_vx = next_vx, vx` has swap semantics.

## 18. Destination-driven construction with `.{...}`

Super constructs an aggregate into the destination selected by context whenever
possible. A return type, parameter type, field type, or typed binding can provide
that context:

```super
+User = {
    id: i64
    name: str
    active: bool = true
}

+new_user(id: i64, name: str) : User  => .{id, name}

+accept(user: User) => user.id

primary: User = .{id: 1, name: "Amina"}
code = accept(.{id: 2, name: "Noor"})
```

Field punning binds `id` to `id: id`. Defaults fill omitted fields. This is not
dynamic dictionary construction: semantic analysis knows the target layout and
field offsets. For larger results the hidden return destination lets the callee
write directly into caller-owned stack storage.

Use named `User{...}` when context is weak or the type name communicates domain
meaning. Use `.{...}` at typed boundaries where repeating the name adds no
information. If inference reports E0201, add a type annotation at the narrowest
boundary rather than annotating every intermediate expression.

Destination construction also clarifies optional fallback:

```super
user = find_user(id) or .{id: 0, name: "Guest", active: false}
```

The successful and fallback branches construct the same concrete type. No
boxed union or hash map is introduced.

## 19. Request, frame, lexical, and task stacks

Most backend values die with a request; most game temporaries die with a frame.
Super maps those facts to bump-stack marks:

```super
+handle(req: http.Request) : http.Response! {
    scope {
        body = req.json(CreateOrder) or err {
            return http.bad_request(.{error: "{err}"})
        }
        scratch = [0; 4096]
        order = create_order(body, @scratch)
        return http.ok(.{id: order.id})
    }
}
```

The request or explicit scope records a cursor. Arrays, temporary strings,
decoded records, and intermediate pipeline output advance the cursor. Scope exit
rewinds it once. There is no object-by-object free walk and no tracing pause.

That is the design. In 0.1.0 the HTTP request scope is not implemented yet. A
server whose routes are closures runs in the boxed runtime, which keeps every
per-request value on the heap: 42 allocations and 0 frees per request for the
hello server, pinned by `crates/super-cli/tests/alloc_probe.rs`.

The same shape applies to a frame:

```super
state world = ecs.World{}

+update(dt: f64) {
    scope {
        contacts = [Contact{}; MAX_CONTACTS]
        count = broad_phase(world, @contacts)
        solve(@world, contacts[0..count], dt)
    }
}
```

`world` persists across frames and may survive hot reload. `contacts` does not.
The compiler prevents a mutable view into `contacts` from being stored in
`world`. Persistent state should contain values, stable handles, or data copied
into its own stack, never an accidental pointer into request/frame scratch.

A spawned task receives a task-local micro-stack. Structured `scope` waits for
its tasks before reclaiming their storage:

```super
scope {
    left = spawn fetch("/left")
    right = spawn fetch("/right")
    merged = merge(await left, await right)
    consume(merged)
}
```

If data must outlive the task, return it into the awaiting destination, send an
owned value through a bounded channel, or explicitly copy it into persistent
state. Do not hide lifetime transfer behind a global registry.

### Choosing the right lifetime

Use a lexical stack for parsing helpers and algorithm scratch. Use a request
stack for decoded inputs and response assembly. Use a frame stack for contacts,
render commands, and visibility lists. Use `state` for world data, caches, and
hot-reload continuity. Use host resource handles for sockets, files, GPU
objects, and database connections whose cleanup cannot be represented by a
cursor rewind alone.

## 20. Error pipelines with `or` and `err`

Optionals and value errors keep the success path as an ordinary value:

```super
+load_config(path: str) : Config! {
    text = fs_read_str(path) or err {
        fail "cannot read {path}: {err}"
    }
    toml_decode(text, Config) or err {
        fail "invalid configuration: {err}"
    }
}
```

At a boundary, convert domain failure into the boundary protocol:

```super
#post("/orders")
handle_orders(req: http.Request) : http.Response! {
    body = req.json(CreateOrder) or err {
        return http.bad_request(.{error: "{err}"})
    }
    order = create_order(body) or err {
        return http.conflict(.{error: "{err}"})
    }
    http.ok(.{order_id: order.id})
}
```

`or` evaluates its fallback only on failure. `or err { ... }` binds the payload
for logging, mapping, recovery, or early return. A plain fallback is ideal for a
true default:

```super
port = env_port("PORT") or 8080
```

Do not use defaults to erase operational failures. Missing optional metadata can
reasonably default; invalid authentication, disk corruption, and database
timeouts normally deserve an error path. Keep logs at architectural boundaries
so low-level helpers remain reusable and a failure is not logged repeatedly.

For pipelines, handle failure as soon as policy is known:

```super
token = header(req, "authorization")
    .strip_prefix("Bearer ")
    .decode_token()
    or err { return http.unauthorized(.{error: "{err}"}) }
```

The exact available methods depend on imported modules, but the shape remains:
typed transformations on the left, one policy decision at the edge.

## 21. ECS systems and deterministic frames

An ECS world separates identity from component storage. A system should declare
the components it reads and mutates:

```super
state world = ecs.World{}

+movement(dt: f64) {
    #simd
    for @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}
```

The leading `@` applies to the mutable component stream. The velocity stream is
read-only. This gives the scheduler and optimizer a usable access set. Systems
that mutate disjoint component sets can run in parallel; systems with conflicts
need an order or a staged command buffer.

For deterministic simulation:

1. use a fixed timestep;
2. make iteration order stable where order affects results;
3. avoid wall-clock reads inside systems;
4. seed random generators explicitly;
5. queue structural changes until a defined synchronization point;
6. checksum authoritative fields in tests and network snapshots.

```super
+tick(@world, inputs) {
    apply_inputs(@world, inputs)
    movement(TICK_DT)
    broad_phase(@world)
    solve_contacts(@world)
    flush_commands(@world)
}
```

Use `par` only where iterations are independent or a supported reduction
combines worker-local results. Collision pair generation, damage ordering, and
entity creation can be order-sensitive; make their merge rule explicit.

## 22. Router and protocol patterns

Routes are declarations in the language:

```super
#get("/health")
health(req: http.Request) : http.Response! {
    http.ok(.{status: "ok"})
}

#post("/users")
create_user(req: http.Request) : http.Response! {
    input = req.json(CreateUser) or err {
        return http.bad_request(.{error: "{err}"})
    }
    http.created(create_user(input))
}
```

Keep transport records separate from domain records when validation or privacy
differs. A request DTO can accept fields that never enter the stored model; a
response DTO can omit private fields. Route handlers should coordinate parsing,
authorization, domain calls, and response mapping, not implement storage
algorithms inline.

For high-rate services, parse method and path once, use zero-copy slices for
parameters, and route through a precomputed tree or table. Bound request bodies,
header counts, connection queues, channel capacity, and timeouts. A scoped
request stack makes reclamation cheap but does not make unbounded input safe.

For frontend/backend communication, prefer a binary WebSocket protocol with
Protobuf schemas for sustained streams, then gRPC with a Tonic host where HTTP/2
RPC fits. Always retain a JSON endpoint or JSON WebSocket message form for
browsers, diagnostics, and operational fallback. The core domain function
should accept typed values so binary and JSON adapters share behavior.

```super
+apply_command(@state, command: Command) : Reply! {
    // transport-independent domain logic
}

#post("/v1/command")
command(req: http.Request) : http.Response! {
    command = req.json(Command) or err {
        return http.bad_request(.{error: "{err}"})
    }
    http.ok(apply_command(@state, command) or err {
        return http.conflict(.{error: "{err}"})
    })
}
```

## 23. Compile-time lookup tables with `#for`

A lookup table that depends only on constants belongs in compile-time storage:

```super
+CRC_POLY = 0xEDB88320

+CRC32_LUT = #for i in 0..256 {
    crc = i
    for 0..8 {
        crc = if (crc & 1) == 1 {
            (crc >> 1) ^ CRC_POLY
        } else {
            crc >> 1
        }
    }
    crc & 0xFFFF_FFFF
}
```

Runtime code becomes one indexed update per byte:

```super
+crc32(text: str) : i64 {
    crc = 0xFFFF_FFFF
    for i in 0..byte_len(text) {
        slot = (crc ^ ord(text[i])) & 255
        crc = ((crc >> 8) ^ CRC32_LUT[slot]) & 0xFFFF_FFFF
    }
    (crc ^ 0xFFFF_FFFF) & 0xFFFF_FFFF
}
```

The table is generated by the meta engine, validated during compilation, and
emitted into constant storage. There is no startup initializer. The same pattern
works for trigonometric samples, parser character classes, quantization curves,
small finite-state transitions, and protocol metadata.

Keep generated tables modest and deterministic. If generation depends on a
file, use `embed` so the compiler tracks the input. If the table is huge or
deployment-specific, an external generated asset may produce faster builds and
smaller debug artifacts.

## 24. Fused pipelines and explicit reductions

UFCS pipelines describe value flow:

```super
total = readings
    .keep(it.valid)
    .map(it.value * it.value)
    .sum()
```

The optimizer may fuse recognized operations into one traversal. Use pipelines
for transformations whose ordering and ownership are straightforward. Use an
explicit loop when you need multiple outputs, early exits, complex mutation, or
detailed diagnostics:

```super
sum = 0.0
rejected = 0
for reading in readings {
    if reading.valid {
        sum += reading.value * reading.value
    } else {
        rejected += 1
    }
}
```

`.sum()` and `.par_sum()` communicate an additive reduction directly:

```super
energy = velocities.map(it * it).sum()
checksum = (0..n).map(hash(values[it])).par_sum()
```

A parallel reduction requires an associative operation in practice. Floating
addition is not mathematically associative, so a parallel grouping can differ
in low bits from a serial grouping. Use fixed grouping, integer/fixed-point
accumulation, or an accepted tolerance when reproducibility matters.

## 25. Pattern review checklist

Before optimizing a subsystem, answer:

- What owns the data: lexical scope, request, frame, task, persistent state, or
  host resource?
- Is the hot layout AoS or SoA, and does it match actual access?
- Which roots mutate, and are all mutation boundaries marked with `@`?
- Can output be built directly in its final destination?
- Are failure values handled where policy becomes known?
- Does parallel work write disjoint destinations or use a supported reduction?
- Can a compile-time table replace repeated pure runtime setup?
- Is a JSON fallback preserved beside any binary transport?
- Do tests verify failure paths, deterministic checksums, and optimized/scalar
  equivalence?

# Part IV — Algorithms: Ten Benchmark Deep Dives

The benchmark contract matters: every language implementation must compute the same checksum, the harness performs two warmups and five timed runs, and a checksum mismatch fails the gate. Read the Super source at `benchmarks/*/algo.sr`; do not infer performance from synthetic snippets alone.

## 17. 01_nbody

Source: `benchmarks/01_nbody/algo.sr`

N-body integrates interacting particles and stresses floating-point arithmetic, nested loops, and mutable arrays. The Super version keeps body state in dense arrays and uses explicit `@` at mutation boundaries. The useful lesson is layout: positions, velocities, and masses should be arranged for repeated linear traversal rather than wrapped in dynamic objects.

When modifying the kernel:

1. preserve update order and checksum;
2. keep hot values local;
3. avoid changing force accumulation precision;
4. compare both AOT and JIT, since specialization shapes differ.

the latest verified results mean times were C 4.852 ms, Super AOT 4.703 ms, and Super JIT 5.460 ms.

## 18. 02_mandelbrot_parallel

Source: `benchmarks/02_mandelbrot_parallel/algo.sr`

Mandelbrot is a naturally independent row kernel. `par y in 0..H` exposes that independence, while every worker computes a local row result. The C23 emitter recognizes the pure-double row shape and can generate OpenMP with a reduction or independent stores.

The deep lesson is to parallelize at a granularity that amortizes scheduling. Pixels are too small as tasks; rows are useful partitions. the latest verified results mean times were C 3.237 ms, AOT 1.423 ms, and JIT 3.652 ms, with substantial variance visible in short runs.

## 19. 03_astar_pathfinding

Source: `benchmarks/03_astar_pathfinding/algo.sr`

A* combines dense fixed-capacity arrays, a binary heap, irregular branches, and sentinel values. `[1; 4096]`, `[INF; 4096]`, and related repeat arrays avoid per-node object allocation. This benchmark demonstrates that scoped dense storage is useful even when SIMD is not the main win.

Keep heap invariants explicit and use multi-assignment carefully: right sides are all evaluated before stores, making swaps reliable. the latest verified results means were C 0.663 ms, AOT 0.624 ms, and JIT 1.173 ms.

## 20. 04_json_parity

Source: `benchmarks/04_json_serde/algo.sr`

The JSON benchmark measures parsing/encoding work under a checksum gate. Super’s standard JSON design favors structural validation and path extraction rather than building a general heap tree. This is representative of backend APIs that decode known request shapes.

Avoid comparing only source line count: validation coverage and escaping rules must match. the latest verified results means were C 0.380 ms, AOT 0.346 ms, and JIT 0.723 ms.

## 21. 05_parallel_mapreduce

Source: `benchmarks/05_parallel_mapreduce/algo.sr`

This workload generates a 64 KiB corpus, hashes words and prefixes, and accumulates into fixed buckets. It exercises radix constants, array-repeat allocation, loops, mutation-capable helper parameters, and reduction-like scans.

The current benchmark uses explicit open addressing so all implementations perform comparable work. It is a good study in keeping the hot table contiguous and avoiding general map overhead. the latest verified results means were C 2.509 ms, AOT 0.638 ms, and JIT 1.121 ms.

## 22. 06_mergesort_radix

Source: `benchmarks/06_mergesort_radix/algo.sr`

Sorting exposes memory traffic, branch behavior, and temporary buffers. Dense arrays and scoped scratch storage fit the algorithm’s phase lifetime. Radix work rewards predictable passes; merge work rewards sequential accesses.

Preserve stability and checksum when experimenting with pass width. the latest verified results means were C 5.834 ms, AOT 5.431 ms, and JIT 6.342 ms.

## 23. 07_crc32_lz4_compress

Source: `benchmarks/07_crc32_lz4_compress/algo.sr`

Compression combines byte-oriented scanning, hash/checksum arithmetic, and irregular match decisions. It tests whether host/string/byte operations remain cheap after lowering. The standard library’s compression domain includes LZ4 block compression, Adler-32, and CRC-32 contracts.

This benchmark is sensitive to exact byte semantics. Check checksum agreement before trusting time. the latest verified results means were C 0.924 ms, AOT 1.097 ms, and JIT 1.550 ms.

## 24. 08_ecs_simulation

Source: `benchmarks/08_ecs_simulation/algo.sr`

The ECS workload uses 2,000 entities for 20 ticks, structure-of-arrays position/velocity/health data, a 64×64 spatial grid, and collision probes. `integrate(@px, @py, @vx, @vy)` is the canonical restrict-friendly kernel. Grid rebuild and collision remain branchier.

This split is instructive: vectorize integration, keep spatial indexing explicit, and do not demand that one optimization fit the whole frame. the latest verified results means were C 3.800 ms, AOT 3.922 ms, and JIT 4.095 ms.

## 25. 09_matmul_tiled

Source: `benchmarks/09_matmul_tiled/algo.sr`

Tiled matrix multiplication tests cache locality and repeated floating multiply-add work. Tile size is a compile-time tuning decision; arrays remain dense and indexing arithmetic should stay simple enough for strength reduction and bounds-check elimination.

Use `#unroll` or `#simd` only after preserving the scalar checksum. the latest verified results means were C 1.336 ms, AOT 1.288 ms, and JIT 1.846 ms.

## 26. 10_sha256_merkle_tree

Source: `benchmarks/10_sha256_merkle_tree/algo.sr`

SHA-256 plus Merkle construction stresses shifts, rotates, masks, fixed rounds, and repeated block hashing. Hex constants and explicit shift operators map cleanly through C23 `<stdbit.h>`-capable lowering. The Merkle phase tests compact intermediate arrays.

Cryptographic benchmark code must prioritize exactness: do not substitute non-equivalent “fast hashes.” the latest verified results means were C 0.999 ms, AOT 0.976 ms, and JIT 1.456 ms.

## 27. Source atlas: the ten kernels

The following excerpts are from the shipped `algo.sr` files. They are short on
purpose: use them to locate the hot shape, then read the complete source for
initialization, checksum, and edge conditions.

### 27.1 N-body: nested force accumulation

From `benchmarks/01_nbody/algo.sr`:

```super
for i in 0..n {
    ax, ay = 0.0, 0.0
    for j in 0..n {
        if i != j {
            dx = x[j] - x[i]
            dy = y[j] - y[i]
            inv = 1.0 / sqrt(dx * dx + dy * dy + 0.01)
            inv3 = inv * inv * inv
            ax += m[j] * dx * inv3
            ay += m[j] * dy * inv3
        }
    }
    vx[i] += ax * dt
    vy[i] += ay * dt
}
```

The source uses separate `x`, `y`, `vx`, `vy`, and `m` arrays. Force
accumulation stays local until the velocity store. The later position pass is a
simple dense traversal. This split makes update order explicit and avoids
allocating body objects.

### 27.2 Mandelbrot: row-level `par`

From `benchmarks/02_mandelbrot_parallel/algo.sr`:

```super
rows = [0; 256]
par y in 0..H {
    cy, local = (y * 1.0 / H) * 2.0 - 1.0, 0
    for x in 0..W {
        cx, zx, zy, iter = (x * 1.0 / W) * 3.5 - 2.5, 0.0, 0.0, 0
        for 0..MAXIT {
            zx2, zy2 = zx * zx, zy * zy
            break if zx2 + zy2 > 4.0
            zy, zx = 2.0 * zx * zy + cy, zx2 - zy2 + cx
            iter += 1
        }
        local += iter
    }
    rows[y] = local
}
```

Rows are independent destinations, so no lock is needed. Multi-assignment
preserves the old `zx` and `zy` values while calculating the next iteration.
Only the final row sum crosses worker boundaries.

### 27.3 A*: reusable dense state and inline heap

From `benchmarks/03_astar_pathfinding/algo.sr`:

```super
#inline
+astar(@gscore, @came, @heap, @hprio, @hn, walkable, start: i64, goal: i64): i64 {
    for 0..CELLS {
        gscore[it] = INF
        came[it] = -1
    }
    hn[0] = 0
    gscore[start] = 0
    heap_push(@heap, @hprio, @hn, start, heur(start, goal))
    while hn[0] > 0 {
        cur = heap_pop(@heap, @hprio, @hn)
        // neighbor relaxation follows
    }
    -1
}
```

The caller allocates all buffers once and reuses them for 40 queries. A
single-element `hn` array is an explicit mutable scalar root. Compile-time
direction tables remove per-query setup.

### 27.4 JSON parity: deterministic field workload

From `benchmarks/04_json_serde/algo.sr`:

```super
for 0..N {
    rng = (rng * 1103515245 + 12345) % 2147483648
    id = rng % 1000000
    rng = (rng * 1103515245 + 12345) % 2147483648
    total_cents = rng % 100000
    rng = (rng * 1103515245 + 12345) % 2147483648
    cs = cs + 2 * (id + total_cents + (rng % 8 + 1) * 17)
}
```

The current parity kernel is intentionally compact and deterministic. It
exercises the same generated field values and checksum as peer implementations;
it should not be described as a complete general JSON parser benchmark. Use the
`std.fmt.json` tests for parser correctness and a separately specified corpus
when measuring full encode/decode behavior.

### 27.5 Map/reduce: open-addressed fixed buckets

From `benchmarks/05_parallel_mapreduce/algo.sr`:

```super
+bump(@keys, @counts, key: i64, add: i64) : i64 {
    i = key % BUCKETS
    for probe in 0..BUCKETS {
        j = (i + probe) % BUCKETS
        if counts[j] == 0 {
            keys = arr_set(keys, j, key)
            counts = arr_set(counts, j, add)
            return add
        }
        if keys[j] == key {
            n = counts[j] + add
            counts = arr_set(counts, j, n)
            return n
        }
    }
    0
}
```

The table has explicit capacity and linear probing. It avoids a generic map so
peer implementations perform comparable hashing and collision work. Despite the
directory name, the current source's counting pass is explicit and primarily
serial; benchmark descriptions should not claim parallel execution that is not
present in the file.

### 27.6 Mergesort: one scoped scratch buffer

From `benchmarks/06_mergesort_radix/algo.sr`:

```super
+mergesort(a, tmp, lo: i64, hi: i64) {
    if hi - lo <= 1 { return a }
    mid = lo + (hi - lo) / 2
    a = mergesort(a, tmp, lo, mid)
    a = mergesort(a, tmp, mid, hi)
    merge(a, tmp, lo, mid, hi)
}
```

`main` allocates `a` and `tmp` once at length 50,000. Recursive calls pass the
same storage rather than allocating subarrays. The checksum also verifies
sorted order, preventing a fast but invalid no-op.

### 27.7 CRC-32: library table path

From `benchmarks/07_crc32_lz4_compress/algo.sr`:

```super
use std.fmt.compress

payload = block
for 0..8 { payload = payload + block }
cs = crc32_str(payload) + crc32_str(block) + acc % 1000
```

The benchmark builds deterministic bytes, then calls the standard compression
module's CRC path. The security hash module demonstrates the modern table
pattern with `CRC32_LUT = #for i in 0..256 { ... }`. Always verify exact unsigned
32-bit semantics when changing this kernel.

### 27.8 ECS: SoA and exclusive integration roots

From `benchmarks/08_ecs_simulation/algo.sr`:

```super
+integrate(@px, @py, @vx, @vy) {
    for 0..N {
        npx, npy = px[it] + vx[it], py[it] + vy[it]
        nvx, nvy = vx[it], vy[it]
        nvx = -nvx if npx < 0.0 || npx > 600.0
        nvy = -nvy if npy < 0.0 || npy > 600.0
        px[it], py[it], vx[it], vy[it] = npx, npy, nvx, nvy
    }
}
```

The signature communicates four non-overlapping mutable streams. Grid rebuild
uses `heads` and `next`; collision probes read the grid and update health. Those
phases have different access patterns and remain separate.

### 27.9 Matrix multiplication: loop order and dense buffers

From `benchmarks/09_matmul_tiled/algo.sr`:

```super
+matmul(@a, @b, @c, n: i64) {
    for i in 0..n {
        for k in 0..n {
            aik = a[i * n + k]
            for j in 0..n {
                c[i * n + j] += aik * b[k * n + j]
            }
        }
    }
}
```

The `i-k-j` order reuses `aik` and traverses contiguous rows of `b` and `c`.
The AOT backend may recognize and tile this named kernel. Three explicit
mutable roots expose alias facts; the checksum is the scaled Frobenius norm.

### 27.10 SHA-256 Merkle tree: compact level construction

From `benchmarks/10_sha256_merkle_tree/algo.sr`:

```super
+hash_pair(a, b) : _ {
    cat = [0; 64]
    for 0..32 {
        cat[it], cat[it + 32] = a[it], b[it]
    }
    sha2_256(cat)
}
```

Leaves are 64-byte deterministic messages. Parent input is a fixed 64-byte
stack array, and the level shrinks until one root remains. An odd final node is
paired with itself. The final byte fold checks the actual digest path.


# Part V — Standard Library

The standard library is organized by responsibility rather than by a single
prelude. Import the modules a source file uses. Tooling reads
`stdlib/mod.sr`, whose exported `STDLIB_VERSION` is `"0.1.0"` and whose manifest
lists 57 modules across nine layers.

```super
use std.core.str
use std.fmt.json
use std.net.http
use std.diag.metrics
```

Short aliases such as `std.str`, `std.json`, `std.http`, `std.ecs`, and
`std.diag.metrics` are supported where documented. Layered names are preferable in
libraries because they expose the dependency boundary.

## 28. `core`: values, bytes, and collections

The core layer contains:

- `core.str`: slicing, searching, case conversion, padding, splitting, and
  numeric parsing;
- `core.bytes`: byte arrays, hexadecimal conversion, endian reads/writes, and
  bit helpers;
- `core.collections`: stack, queue, ring buffer, set, priority queue, and
  array-backed map helpers;
- `core.regex`: compiled backtracking regular expressions;
- `core.uuid`: UUID v4/v7 generation, parsing, and formatting.

Core operations preserve explicit ownership. Searches that can miss return
optionals. Parsing does not invent sentinel values unless the API explicitly
documents one:

```super
use std.core.str

port = to_i64("8080") or err {
    print("invalid port: {err}")
    return
}
parts = split("api/v1/users", "/")
```

Use slices for views into an existing string or byte buffer. Copy into an
owning destination before the backing request/frame stack is released.
Collection types are value-oriented and expose `@` mutation:

```super
use std.core.collections

queue = RingBuf.new(128)
@queue.push(event)
next = @queue.pop() or none
```

Fixed arrays remain the lowest-overhead choice when capacity is naturally
bounded and known at compile time.

## 29. `fmt`: JSON, text formats, binary data, compression

The format layer contains JSON, CSV, TOML, binary packing, and compression:

- `fmt.json` validates, decodes, encodes, and extracts known paths without
  requiring a general heap tree;
- `fmt.csv` handles RFC 4180 quoting, commas, and line endings;
- `fmt.toml` parses the supported TOML subset used by package/configuration
  workflows;
- `fmt.bin` packs and unpacks explicit binary representations;
- `fmt.compress` supplies LZ4 blocks, Adler-32, and CRC-32.

At an HTTP boundary:

```super
#post("/events")
events(req: http.Request) : http.Response! {
    event = req.json(Event) or err {
        return http.bad_request(.{error: "{err}"})
    }
    http.ok(.{accepted: true, id: event.id})
}
```

JSON is the interoperability fallback even when a service also exposes
Protobuf over binary WebSocket or gRPC through a host. Binary codecs must define
endianness, field widths, length limits, and versioning. Never decode a claimed
length without comparing it to the remaining buffer and configured maximum.

Compression works best on owned byte blocks. Bound decompressed size before
allocation. Checksums detect accidental corruption; they are not authentication.
Use `sec` primitives for signatures or message authentication.

## 30. `sys`: operating-system boundaries

The system layer includes path, CLI, environment, filesystem, process, time,
epoll, and io_uring modules:

- `sys.path`, `sys.cli`, `sys.env`, `sys.fs`, `sys.proc`, `sys.time`;
- `sys.epoll` for Linux non-blocking descriptor readiness;
- `sys.io_uring` for Linux submission/completion integration, including the
  shipped NOP and socket-readiness bridge.

System APIs wrap narrow host primitives. File descriptors, processes, sockets,
and mappings are resources, not ordinary scoped bytes. Their wrappers track
ownership and explicit close behavior.

An epoll-style server separates readiness from protocol work:

```super
poll = epoll.open() or err { fail "epoll: {err}" }
@poll.add(listener, EPOLL_IN) or err { fail "register: {err}" }

loop {
    ready = @poll.wait(256, 50.ms) or err {
        log.error("poll: {err}")
        continue
    }
    for event in ready {
        handle_ready(event)
    }
}
```

The exact names in a module should be checked with completion or source docs;
the architecture is stable: readiness returns bounded events, protocol state
lives with the connection, and request scratch dies after dispatch.

io_uring is not automatically faster for every workload. Submission batching,
registered resources, kernel version, and operation mix matter. Keep an epoll
path where portability or simpler readiness semantics are valuable.

## 31. `net`: HTTP, WebSocket, polling, TCP, and UDP

The network layer contains:

- `net.http`: requests, responses, routers, cookies, and middleware;
- `net.ws`: handshake and frame encoding/decoding;
- `net.poll`: a higher-level TCP readiness event loop;
- `net.tcp`, `net.udp`, `net.dns`, `net.ip`, `net.url`;
- `net.udp_netcode`: packet helpers for game networking;
- `net.sse`: server-sent event framing;
- `net.retry`: exponential backoff and circuit breaking.

HTTP helpers return typed responses:

```super
#get("/health")
health(req: http.Request) : http.Response! {
    http.ok(.{version: "0.1.0", healthy: true})
}
```

WebSocket framing distinguishes text, binary, ping/pong, and close:

```super
wire = ws_encode(ws_binary(snapshot), 0) or err {
    fail "snapshot encode: {err}"
}
frame = ws_decode(wire) or ws_close(1002, "invalid frame")
```

For live game or telemetry streams, use binary WebSocket plus Protobuf as the
primary contract and offer JSON text messages as fallback. gRPC with a Tonic
host is a strong service-to-service alternative. MsgPack is useful when schema
tooling is less important, but version fields and limits remain mandatory.

TCP is a byte stream: applications must frame messages. A length prefix should
use a fixed endian representation, reject oversized values, and cope with
partial reads/writes. UDP preserves datagrams but may lose, duplicate, or
reorder them; the netcode layer's sequence/ack design must match the game.

## 32. `game`: ECS, physics, app loops, assets, and AI

The game layer supplies:

- `game.ecs`: sparse-set entity/component storage;
- `game.physics2d`: circles, boxes, broad-phase helpers, and impulses;
- `game.ai`: A*, steering, finite-state machines, utility scores, and behavior
  trees;
- `game.app`: fixed timestep, scenes, input actions, and audio-facing helpers;
- `game.assets`: hot-reload cache plus PNG/WAV loaders;
- `game.tilemap`: CSV/JSON tilemaps integrated with pathfinding;
- `game.fx`: `Sprite` (declares `Widget`), `Clip`, `Cue`, and `Mixer`. Paint and sound are display-list lines.

A fixed update loop should accumulate wall time but advance authoritative state
in constant steps:

```super
clock = [0.0]

+frame(frame_dt: f64) {
    clock[0] += frame_dt
    for 0..MAX_CATCHUP {
        break if clock[0] < TICK_DT
        update(TICK_DT)
        clock[0] -= TICK_DT
    }
    render(clock[0] / TICK_DT)
}
```

Separate broad phase, narrow phase, and solver passes. Broad phase should reduce
candidate pairs; it must not change collision meaning. Use stable pair ordering
when replay/network determinism matters. Assets persist beyond one frame, while
decoded scratch and draw-command lists fit frame stacks.

## 33. `sec`: hashes, authentication, encoding, and post-quantum tools

The security layer distinguishes representation, checksums, hashes, and
authentication:

- `sec.encoding`: base64/base32/hex and URL encoding;
- `sec.crypto`: hashes, HMAC, PBKDF2, and TOTP;
- `sec.jwt`: HS256 JWT helpers;
- `sec.sha2`: pure-Super SHA-256 and HMAC-SHA256;
- `sec.hash`: BLAKE2/3, SHA-3, SHAKE, Keccak, CRC-32, FNV, and DJB2;
- `sec.pq`: ML-KEM, ML-DSA, hybrid helpers, and hash-based signatures.

The CRC implementation demonstrates compile-time table generation:

```super
+CRC32_LUT = #for i in 0..256 {
    crc = i
    for 0..8 {
        crc = if (crc & 1) == 1 {
            (crc >> 1) ^ HASH_CRC_POLY
        } else {
            crc >> 1
        }
    }
    crc & 0xFFFF_FFFF
}
```

CRC-32 and FNV are not cryptographic. Use HMAC for message authentication.
Validate JWT algorithm, expiry, issuer/audience policy, and key rotation outside
the compact token primitive. Password derivation needs a deliberately expensive
KDF and deployment-specific parameters.

Post-quantum primitives are easy to compose incorrectly. Treat the module as
building blocks, preserve domain separation and transcript binding, and use
reviewed protocols rather than inventing a handshake.

## 34. `math`: scalar through geometric tools

The math layer contains scalar functions, linear algebra, random generation,
noise, complex values, quaternions, statistics, and geometry:

```super
use std.math.linalg
use std.math.rand
use std.math.geom

velocity = v2(3.0, 4.0)
speed = v2_len(velocity)
```

Random generators are deterministic under an explicit seed, which is essential
for tests and replay. Do not use simulation PRNGs for secrets. Linear algebra
types favor flat layout and dense traversal. Quaternions avoid many Euler-angle
composition problems but still require normalization policy. Statistics APIs
should be fed bounded, validated samples; percentile methods differ in
interpolation and must be chosen consistently.

## 35. `diag`: tests, logs, and metrics

The diagnostic layer includes:

- `diag.test`: assertions and soft reports;
- `diag.log`: leveled structured logs with redaction;
- `diag.metrics`: counters, gauges, histograms, and summary helpers.

Tests are language items, not runtime callbacks:

```super
test "parser rejects an oversized frame" {
    result = decode_frame(make_header(MAX_FRAME + 1))
    #assert(result.is_error())
}
```

Logs should carry stable field names and avoid secrets. Metrics should have
bounded label cardinality: status class and route template are useful; user IDs,
raw paths, and request IDs are not. Histograms need bounds chosen for the
service-level objective, preferably generated at compile time:

```super
#LATENCY_MS = #for i in 0..8 { i * 25 }
```

## 36. Standard-library selection guide

Start at the lowest layer that expresses the problem:

1. use `core` for in-memory values and containers;
2. use `fmt` to cross a representation boundary;
3. use `sys` for operating-system resources;
4. use `net`, `db`, `game`, or `sec` for domain protocols;
5. instrument behavior with `diag`.

Do not bypass a typed module for a host primitive merely to save a line. The
module supplies validation, ownership, diagnostics, and a place for optimized
lowering. Reach for the host boundary only when implementing or extending the
module itself.

# Part VI — Real-World Projects

## 28. 01_space_shooter

Path: `examples/01_space_shooter/`

This project introduces a frame loop, ECS-style position/velocity/collider/enemy data, wave spawning, collision math, score, health, and tests. Study it after the array and mutation chapters. The key mapping is frame lifetime to scoped-stack lifetime: temporary collision candidates die together while persistent world state remains under `state`.

For voxel meshing, 3D integer coordinates, ray visibility, and dense chunk-style indexing, continue with `examples/11_voxel_raycast/`.

## 29. 02_task_api

Path: `examples/02_task_api/`

The API is split into `main.sr`, `models.sr`, `auth.sr`, and `routes.sr`. It demonstrates `use`, exported/private declarations, typed records, route declarations, JSON handling, authentication, and endpoint tests. This is the best starting point for a multi-module backend.

Production work should add a concrete persistent database adapter, migration policy, TLS termination contract, and operational limits. Keep JSON as a supported fallback even if a deployment adds a binary WebSocket/Protobuf or gRPC transport.

## 30. 03_log_analyzer

Path: `examples/03_log_analyzer/`

The analyzer demonstrates high-volume parsing, filtering, grouping, sums, and compact status representation. It maps stream/batch lifetime to a scoped stack and makes reductions explicit. It is a useful template for telemetry preprocessing and offline data tools.

Extension gap: add bounded streaming input, backpressure, rotated-file handling, and malformed-record metrics without turning every record into a heap object.

## 31. 04_roguelike_dungeon

Path: `examples/04_roguelike_dungeon/`

This project applies grids, procedural generation, pathfinding, turns, and compact entities. It reinforces array-repeat maps, deterministic random input, and AI helpers. Tests should pin generation seeds and verify reachability rather than snapshots of incidental formatting.

Chunked 3D cells, face-style sampling, and deterministic grid indexing are demonstrated in `examples/11_voxel_raycast/`.

## 32. 05_tower_defense

Path: `examples/05_tower_defense/`

Tower defense combines path following, waves, targeting, cooldowns, and damage. It is a natural data-oriented workload: separate arrays for positions, target IDs, cooldowns, and health make hot updates direct. `.sum()` pipelines are appropriate for economy and damage summaries.

Swarm movement with neighborhood queries, separation/alignment/cohesion, and double-buffered steering is demonstrated in `examples/12_boids_flocking/`.

## 33. 06_physics_sandbox

Path: `examples/06_physics_sandbox/`

The sandbox exercises integration, collision detection, impulses, and the `std.physics2d` contract. Separate broad phase from narrow phase and solver iterations. `@` parameters make mutation sets reviewable and provide alias facts for integration kernels.

Extension gap: continuous collision detection, joints, sleeping islands, and deterministic cross-platform replay are not implied by a small 2D example.

## 34. 07_realtime_chat_broker

Path: `examples/07_realtime_chat_broker/`

The broker maps naturally to structured concurrency: listeners create tasks, channels transfer messages, and scopes own connection work. The network library provides HTTP/WebSocket framing and Linux epoll-backed readiness primitives.

Chat broadcast is not game-state replication. Continue with the authoritative fixed-timestep loop in `examples/13_multiplayer_tick/` for snapshots, sequence numbers, and deterministic tick checksums. Prefer a binary WebSocket + Protobuf interface for live state, with JSON fallback for browsers/tools.

## 35. 08_kv_cache_engine

Path: `examples/08_kv_cache_engine/`

The cache engine demonstrates maps/tables, expiry, metrics, and bounded storage behavior. It is useful for thinking about persistent versus request-scoped state. Define eviction and time semantics explicitly; a benchmark that never reaches capacity does not test a cache policy.

Extension gap: persistence snapshots, replication, sharding, admission policy, and crash recovery need separate contracts and tests.

## 36. 09_http_load_tester

Path: `examples/09_http_load_tester/`

This example combines `scope`, `spawn`, `chan[Sample]`, `shared` counters, compile-time histogram bounds, HTTP simulation, and metrics. It is the clearest concurrency tutorial in the examples.

The collector owns the stream of samples; workers send values rather than sharing an output array. That ownership shape scales better and avoids E0302. A production extension needs real sockets, connection reuse, rate shaping, coordinated omission correction, TLS, and percentile calculation over an actual histogram.

## 37. 10_mapreduce_kv

Path: `examples/10_mapreduce_kv/`

The project connects partitioned mapping, grouped/reduced values, and key/value state. It complements benchmark 05 by emphasizing architecture rather than only a timed kernel. Use bounded channels between stages to make backpressure visible.

Extension gap — multiplayer/cluster: distributed operation needs partition ownership, retry identity, idempotence, failure detection, and a wire protocol. Binary Protobuf over WebSocket or gRPC/Tonic hosts are suitable primary transports; preserve JSON for debugging and frontend compatibility.

## 38. 11_voxel_raycast

Path: `examples/11_voxel_raycast/`

Software DDA raycasting over a fixed `MAP³` voxel grid: dense indexing, 3D integer sampling along a ray, parallel scanline rendering, and solid-floor plus obstacle hits. It closes the voxel extension paths referenced from the 2D shooter and roguelike chapters—use it when learning chunk-local arrays, visibility probes, and the stepping blocks that precede face extraction or streaming chunk ownership.

## 39. 12_boids_flocking

Path: `examples/12_boids_flocking/`

Data-oriented flocking with fixed-capacity arrays for positions and velocities, perception-radius neighbor scans, separation/alignment/cohesion accumulation, and post-integration speed clamping. Steering forces are accumulated in separate buffers before mutating velocities, which keeps `@` updates from overlapping within a tick—matching the boids extension sketched in the tower-defense walkthrough.

## 40. 13_multiplayer_tick

Path: `examples/13_multiplayer_tick/`

Authoritative 60Hz fixed-timestep simulation with stack-resident player SoA arrays, tick accumulation, and a deterministic `CHECKSUM` over replicated state. Pair with `07_realtime_chat_broker` for transport; prefer binary WebSocket + Protobuf (or gRPC) on the wire with JSON for debugging and browser tools.

## 41. Complete project field guide

All projects are runnable modules with executable tests. From the repository
root, use:

```bash
super check examples/NN_name/src/main.sr
super test examples/NN_name/
super run examples/NN_name/src/main.sr
```

The first ten projects are the original application suite. Projects 11–20 form
a second, complete workload set: three data-oriented games, a binary TCP broker,
a radix HTTP core, a RESP/LRU cache, a log pipeline, a rigid-body simulation, a
multi-room WebSocket core, and an authoritative synchronized game server.

### 41.1 `examples/01_space_shooter`

Purpose: a compact arcade simulation with ECS components, collision tests,
parallel physics intent, compile-time trigonometric tables, channels, and
hot-reloadable world state.

Key features: `state`, named value structs, `@self` methods, ECS component
queries, `#for`, `#simd`, `for parallel`, channels, scoped tasks, and tests.

```super
state world = ecs.World{}

+update(dt: f64) {
    #simd
    for parallel @(pos: Pos), (vel: Vel) in world {
        pos.x += vel.vx * dt
        pos.y += vel.vy * dt
    }
}
```

Run `super test examples/01_space_shooter/`. Extend it by separating collision
candidate generation from damage commands so parallel movement cannot race
structural world changes.

### 41.2 `examples/02_task_api`

Purpose: a multi-module backend skeleton with models, authentication, routes,
concurrent cache/database work, an audit channel, HTTP helpers, JWTs, logging,
hashes, and post-quantum key examples.

Key features: selective `use`, typed DTOs, `scope`/`spawn`, bounded channels,
JSON responses, value errors, and module-private tests.

```super
scope {
    t_user = spawn fetch_user(id)
    t_cache = spawn fetch_cache(id)
    user = await t_user
    cached = await t_cache
    total = user.id + cached
}
```

Run `super test examples/02_task_api/`. For production, keep JSON routes as the
fallback and add Protobuf over binary WebSocket or a gRPC/Tonic host for the
primary typed transport.

### 41.3 `examples/03_log_analyzer`

Purpose: high-volume log parsing and aggregation with embedded fixtures,
compile-time character classes, slices, regex, metrics, pipelines, and chunk
distribution.

Key features: `embed`, sum types, `#for`, UFCS fusion, string slices, shared
counters, and a million-record scale test.

```super
+analyze(scores) : i64 {
    scores.keep(it >= 25).map(it * 2).sum()
}
```

Run `super test examples/03_log_analyzer/`. A production reader should add
bounded input chunks, malformed-line counters, backpressure, and rotation
handling without retaining whole files.

### 41.4 `examples/04_roguelike_dungeon`

Purpose: BSP-flavored room counting, grid movement, A* integration, FOV-style
visibility, ASCII dimensions, deterministic direction tables, and AI work
queues.

Key features: sum types, compile-time direction arrays, channels, shared values,
and `std.game.ai`.

```super
#DIR_DX = [0, 1, 0, -1]
#DIR_DY = [-1, 0, 1, 0]

+neighbor_steps() : i64 {
    (0..4).map(#DIR_DX[it] * #DIR_DX[it] + #DIR_DY[it] * #DIR_DY[it]).sum()
}
```

Run `super test examples/04_roguelike_dungeon/`. Pin random seeds and test
connectivity invariants instead of snapshots of decorative output.

### 41.5 `examples/05_tower_defense`

Purpose: a dense tower-defense workload with 50,000-creep scale tests, spatial
cell helpers, targeting, projectiles, economy, priority queues, and deterministic
noise.

Key features: dense records, `for parallel`, `#simd`, compile-time neighbor
offsets, channels, collections, and noise.

```super
+retarget(creeps: i64, towers: i64) : i64 {
    #simd
    for parallel 0..creeps {
        inspect_target_band(it, towers)
    }
    creeps * towers
}
```

Run `super test examples/05_tower_defense/`. Real targeting should partition
space and write one target slot per creep, avoiding a shared counter in the hot
parallel pass.

### 41.6 `examples/06_physics_sandbox`

Purpose: 2D rigid bodies, AABB and circle overlap, Euler integration, broad-phase
sizing, unrolled solver passes, contact channels, and `std.game.physics2d`.

Key features: struct methods, `@self`, `for parallel`, `#unroll`, sum variants,
and deterministic collision tests.

```super
+RigidBody = {
    x: f64
    y: f64
    vx: f64
    vy: f64

    integrate_euler(@self, dt: f64) {
        self.x += self.vx * dt
        self.y += self.vy * dt
    }
}

@body.integrate_euler(0.016)
```

Run `super test examples/06_physics_sandbox/`. Keep integration, candidate
generation, contact construction, and iterative solving as separate phases.

### 41.7 `examples/07_realtime_chat_broker`

Purpose: a multi-room message broker with bounded channels, heartbeat timeouts,
shared delivery counters, parallel room work, compile-time wire metadata, and
WebSocket frame tests.

Key features: `when` multiplexing, `after`, structured concurrency, sums, and
`std.net.ws`.

```super
when {
    stop.recv() => return
    after 1.ms => send_ping()
}
```

Run `super test examples/07_realtime_chat_broker/`. Add per-connection output
limits so a slow client cannot retain unbounded messages.

### 41.8 `examples/08_kv_cache_engine`

Purpose: a sharded in-memory key/value core with command channels, TTL sweeps,
compile-time shard masks/seeds, Redis-like behavior, and collection tests.

Key features: `#{...}`, `#for`, `#inline`, shared counters, channels, and
`std.db.redis`.

```super
#SHARD_BITS = 2
#SHARD_MASK = #{ 4 - 1 }

+shard(key: i64) : i64  => key & #SHARD_MASK
```

Run `super test examples/08_kv_cache_engine/`. Production cache design also
needs capacity pressure, eviction policy, snapshot/replication semantics, and
crash tests.

### 41.9 `examples/09_http_load_tester`

Purpose: concurrent synthetic HTTP generation, sample streaming, histogram
bounds, percentile indexes, and metrics.

Key features: spawned workers, bounded sample channels, shared counters,
compile-time histogram tables, HTTP simulation, and metrics.

```super
#HIST_BOUNDS = #for i in 0..6 { i * 200 }

for bound in #HIST_BOUNDS {
    if latency_us >= bound { bucket += 1 }
}
```

Run `super test examples/09_http_load_tester/`. Real load generation needs
connection reuse, rate shaping, TLS, timeout policy, and correction for
coordinated omission.

### 41.10 `examples/10_mapreduce_kv`

Purpose: partitioned mapping, bounded batches, a reducer task, n-gram counting,
ring buffers, and stage logging.

Key features: channels as ownership transfer, shared reduction state,
compile-time constants, collections, and deterministic tests.

```super
scope {
    reducer = spawn reduce_batches(batches, @reduced)
    for partition in partitions {
        batches.send(map_partition(partition))
    }
    batches.close()
    await reducer
}
```

Run `super test examples/10_mapreduce_kv/`. Distributed evolution requires
partition identity, retry identity, idempotence, explicit wire schemas, and
backpressure.

### 41.11 `examples/11_voxel_raycast`

Purpose: software grid ray marching over a fixed 16³ voxel field with
parallel scanlines and a checksum.

What Super does well: `[0; 4096]` creates dense map storage, `sample` performs
bounded integer indexing, `par y` gives workers independent row destinations,
and `rows.sum()` closes the workload with a deterministic gate.

```super
rows = [0; H]
par y in 0..H {
    local = 0
    for x in 0..W {
        local += cast(map, 8.0, 3.0, 8.0, ray_x(x), -0.02, ray_z(y))
    }
    rows[y] = local
}
```

Run:

```bash
super run examples/11_voxel_raycast/src/main.sr
super test examples/11_voxel_raycast/
```

The next step is chunk ownership: keep chunk voxels persistent, allocate visible
faces per frame, and schedule scanlines or tiles with disjoint outputs.

### 41.12 `examples/12_boids_flocking`

Purpose: fixed-capacity flocking with SoA positions/velocities, neighborhood
scans, steering accumulation, speed limits, wraparound, and deterministic seeds.

What Super does well: acceleration arrays are frame scratch; old position and
velocity arrays remain read-only during neighbor scans; updates happen in a
separate pass through exclusive roots.

```super
+step(@px, @py, @vx, @vy) {
    ax, ay = [0.0; N], [0.0; N]
    for i in 0..N { accumulate_neighbors(i, px, py, vx, vy, @ax, @ay) }
    for i in 0..N {
        vx[i] += ax[i]
        vy[i] += ay[i]
    }
}
```

Run `super run examples/12_boids_flocking/src/main.sr` and
`super test examples/12_boids_flocking/`. A spatial grid can reduce the
quadratic neighbor scan while preserving the double-buffered update pattern.

### 41.13 `examples/13_multiplayer_tick`

Purpose: authoritative fixed-60-Hz simulation, accumulated fractional frame
time, bounded catch-up, SoA player state, and deterministic snapshot checksums.

What Super does well: one-element array state makes mutation explicit, fixed
steps separate simulation from frame arrival, and checksum output makes replay
tests cheap.

```super
+simulate_tick(@x, @y, @vx, @vy) {
    for player in 0..PLAYER_CAPACITY {
        x[player] += vx[player] * TICK_DT
        y[player] += vy[player] * TICK_DT
    }
}
```

Run `super run examples/13_multiplayer_tick/src/main.sr` and
`super test examples/13_multiplayer_tick/`. Pair it with binary WebSocket plus
Protobuf snapshots and retain JSON messages for debugging and browser fallback.

### 41.14 `examples/14_tcp_message_broker`

Purpose: length-prefixed binary frames, topic subscriptions, flat membership
tables, fanout, deterministic sequence checks, and 2,000-message workload output.

What Super does well: wire buffers have fixed capacity; `encode_frame` mutates
only its destination; subscriptions flatten to
`subscriber * TOPIC_COUNT + topic`; fanout writes explicit mailbox roots.

```super
+fanout(subscriptions, topic: i64, payload_sum: i64, @mailboxes) : i64 {
    delivered = 0
    for subscriber in 0..SUBSCRIBER_COUNT {
        if subscriptions[subscriber * TOPIC_COUNT + topic] == 1 {
            mailboxes[subscriber] += payload_sum
            delivered += 1
        }
    }
    delivered
}
```

Run `super run examples/14_tcp_message_broker/src/main.sr` and
`super test examples/14_tcp_message_broker/`. A socket adapter must handle
partial frames, enforce `MAX_FRAME`, and apply per-subscriber backpressure.

### 41.15 `examples/15_http_radix_framework`

Purpose: method-aware radix-style routing, zero-copy path parameter slices,
typed route matches, static and parameter routes, and a 10,000-request checksum.

What Super does well: early returns keep matching linear to read, slices avoid
allocating path segments, and a flat handler ID supports direct dispatch.

```super
if method == METHOD_GET && starts_with(path, "/users/") {
    return RouteMatch{
        handler: 4,
        parameter: parse_decimal(path[7..byte_len(path)]),
        matched: true,
    }
}
```

Run `super run examples/15_http_radix_framework/src/main.sr` and
`super test examples/15_http_radix_framework/`. A generated radix tree can move
route-shape decisions to compile time while leaving parameter validation in the
request stack.

### 41.16 `examples/16_resp_lru_cache`

Purpose: RESP-like command parsing, fixed-capacity O(1)-style table slots, LRU
stamps, TTL expiry, deletion, deterministic pressure, and 5,000 operations.

What Super does well: keys, values, expiries, and stamps are SoA arrays; all
capacity is explicit; cache mutations name their roots; no node allocation is
required.

```super
+cache_set(@keys, @values, @expires, @stamps, key: i64, value: i64, now: i64) {
    slot = find_slot(keys, key)
    if slot < 0 { slot = choose_slot(keys, stamps) }
    keys[slot], values[slot] = key, value
    expires[slot], stamps[slot] = 0, now
}
```

Run `super run examples/16_resp_lru_cache/src/main.sr` and
`super test examples/16_resp_lru_cache/`. A production implementation should
replace scans with an indexed table/list while retaining fixed bounds and expiry
tests.

### 41.17 `examples/17_log_ingest_pipeline`

Purpose: 200,000 synthetic records, chunked ingestion, service/status/latency
aggregates, sum variants, fused filtering/mapping, and deterministic byte totals.

What Super does well: records are generated and consumed inside the chunk
lifetime; aggregate arrays persist; only compact counters cross chunk
boundaries; UFCS expresses a secondary pipeline.

```super
+ingest_chunk(start: i64, end: i64, @services, @statuses, @latencies) : i64 {
    bytes = 0
    for index in start..end {
        record = make_record(index)
        services[record.service] += 1
        statuses[record.status] += 1
        latencies[classify_latency(record.latency_ms)] += 1
        bytes += record.bytes
    }
    bytes
}
```

Run `super run examples/17_log_ingest_pipeline/src/main.sr` and
`super test examples/17_log_ingest_pipeline/`. Parallel chunks should produce
worker-local counters, followed by a bounded merge.

### 41.18 `examples/18_rigidbody_sandbox`

Purpose: 96-body fixed-step integration, gravity, boundary restitution,
pairwise circle contacts, two solver iterations, and 180-tick checksums.

What Super does well: SoA roots expose mutation and layout, radius is read-only,
solver passes are compile-time-unrolled, and deterministic tests isolate gravity
and contact behavior.

```super
+step(@x, @y, @vx, @vy, radius) : i64 {
    integrate(@x, @y, @vx, @vy, radius, DT)
    contacts = 0
    #unroll(2)
    for 0..2 {
        contacts += solve_contacts(@x, @y, @vx, @vy, radius)
    }
    contacts
}
```

Run `super run examples/18_rigidbody_sandbox/src/main.sr` and
`super test examples/18_rigidbody_sandbox/`. Add broad-phase cells before
raising body count; preserve a scalar reference for solver verification.

### 41.19 `examples/19_ws_multiroom_chat`

Purpose: WebSocket-shaped frames, multi-room membership, sender exclusion,
fanout into client inboxes, standard WebSocket round trips, and 4,000 messages.

What Super does well: membership is a flat bit table, frame decoding produces a
small value record, and publish mutates one explicit inbox root.

```super
+publish(memberships, frame: ChatFrame, @inboxes) : i64 {
    delivered = 0
    for client in 0..CLIENT_COUNT {
        if client != frame.sender && memberships[client * ROOM_COUNT + frame.room] == 1 {
            inboxes[client] += frame.payload_sum + frame.sequence
            delivered += 1
        }
    }
    delivered
}
```

Run `super run examples/19_ws_multiroom_chat/src/main.sr` and
`super test examples/19_ws_multiroom_chat/`. Use Protobuf in binary frames for
the primary contract and JSON text frames as a fallback.

### 41.20 `examples/20_game_server_ws_sync`

Purpose: a 60-Hz authoritative server, 48-player SoA state, input application,
world wrapping, quantized snapshots, WebSocket binary framing, and periodic
state synchronization.

What Super does well: input records are values, velocity and position mutation
are separate, snapshot output is destination-driven into a fixed packet, and
checksums cover synchronization output.

```super
+encode_snapshot(@packet, tick: i64, x, y) : i64 {
    packet[0] = 130
    cursor = 4
    for player in 0..PLAYER_CAPACITY {
        write_quantized(@packet, cursor, player, x[player], y[player], tick)
        cursor += 6
    }
    cursor
}
```

Run `super run examples/20_game_server_ws_sync/src/main.sr` and
`super test examples/20_game_server_ws_sync/`. Evolve this toward delta
snapshots, acknowledgements, input sequence validation, interest management,
and JSON diagnostics.

### 41.27 `examples/27_gui_todo`

Purpose: a todo desk whose rows live in `std.db.sqlite` and whose window
declares `Widget`. Add refuses an empty title, toggle flips `done`, and the
filter is `all`, `open`, or `done`.

What Super does well: `+TodoApp: Widget` is checked at compile time. `paint`
builds a `std.gui` tree (title bar, field, badge, toast) and returns the
display list. Tests read that string and the sqlite rows. Running the program
opens the platform window: GTK 4 on Linux (Wayland, then X11), Win32 on
Windows, Cocoa on macOS. `SUPER_GUI=win32` or `SUPER_GUI=cocoa` selects those
providers; `SUPER_GUI=gtk` keeps GTK, including on Windows and macOS.

Run `super test examples/27_gui_todo`. Nineteen assertions cover insert,
validation, filter, delete, hit testing, and measure.

### 41.28 `examples/28_gui_breakout`

Purpose: a paddle, a sprite ball, a row of bricks, and bounce or break cues.

What Super does well: `Breakout` declares `Widget`. `tick` mutates the ball.
`paint` asks `Sprite` and `Cue` for display-list lines (`sprite …`, `sound bounce 80`).

Run `super test examples/28_gui_breakout`.

### 41.29 `examples/29_gui_invaders`

Purpose: a player sprite, an alien row whose width tracks how many remain, one
shot, and `shot` / `hit` cues.

What Super does well: the same `Widget` header as the desk, with `std.game.fx`
for the cels. `fire` and `tick` are ordinary methods beside `paint`.

Run `super test examples/29_gui_invaders`.

### 41.30 `examples/30_gui_snake`

Purpose: a head that moves, food that grows `body`, and a wall that ends the run.

What Super does well: the length field is `body`, because `len` is the array
intrinsic. Eating writes `sound eat 60`. A later tick on a dead snake returns
`dead` and does not move.

Run `super test examples/30_gui_snake`.

## 42. A staged learning project

Build a small telemetry service:

1. Define `Event` as a struct and `Severity` as a sum.
2. Accept a JSON route with `or err`.
3. Send events over `chan[Event]` to a collector task.
4. Aggregate counts with dense arrays and `.sum()`.
5. Publish metrics and a JSON health endpoint.
6. Add a binary transport in the host while retaining JSON.
7. Compile with C23 and compare VM/JIT/AOT checksums.

Skeleton:

```super
use std.net.http
use std.diag.metrics

+Event = {
    service: str
    latency_us: i64
    ok: bool = true
}

state metrics = Metrics.new()
state events = chan[Event](1024)

#post("/event")
event(req: http.Request) : http.Response! {
    event = req.json(Event) or err {
        return http.bad_request(.{error: "{err}"})
    }
    events.send(event)
    http.ok(.{accepted: true})
}

+collector() {
    for event in events {
        @metrics.inc("events", 1)
        @metrics.observe("latency_us", event.latency_us)
    }
}

test "event route rejects malformed JSON" {
    response = http.simulate_post("/event", "{bad")
    assert_eq(response.status, 400)
}
```

## 42. Engineering checklist

Before calling a Super program complete:

- run `super check` on every entry module;
- run `super test` at the package or repository root;
- treat E0303–E0305 as design feedback, not noise;
- verify every `@` parameter is truly mutation-capable and non-escaping;
- checksum optimized kernels against a simple reference;
- test optional/error failure paths;
- bound channels, arrays, request sizes, and resource counts;
- separate persistent `state` from request/frame/task temporaries;
- test both scalar fallback and optimized C23/JIT paths;
- record benchmark machine, compiler, warmups, runs, and variance;
- preserve JSON transport fallbacks when adding binary protocols;
- commit `Pod.lock` for reproducible applications.

# Part VII — Tooling & Performance

Super is a toolchain, not only a parser. The CLI, diagnostics, language server,
editor grammars, benchmark harness, VM/JIT, and C23 backend are designed to
agree on one program model.

## 43. CLI workflow

The primary commands are:

```bash
super check path/to/main.sr
super r path/to/main.sr
super test path/or/package
super b -d path/to/main.sr
super b -r --c23 path/to/main.sr -o dist/app
super b --profile size --c23 -Oz path/to/main.sr -o dist/app
super install
super which
super list
super uninstall
super clean -d
super clean -r
super clean
super help
super lsp
super mcp
```

`check` lexes, parses, resolves, infers, validates mutation/lifetimes, and emits
diagnostics without running the program. Use it as the fastest feedback loop.
`super r` is `super run`. `super b` is `super build`. Either command with no
file uses the Pod's `Pod.toml` and the entry for its `kind`
(`src/main.sr`, `src/lib.sr`, or `src/macros.sr`). `-d` is the dev profile
(`-O0 -g`, tests kept). `-r` is the release profile, and it is the fastest
setting the toolchain has: `-O3`, fat LTO, `-march=native`, `-ffast-math`,
unrolling, and direct calls, with tests stripped. `--profile test` is `-O2 -g`
with tests kept (this is what `super test` selects). `--profile size` is the
small binary. `--profile fast` matches release. `-O0` through `-O3`, `-Os`,
`-Oz`, `-Og`, and `-Ofast` override just the optimization level. A flag on the
command line wins over the Pod manifest. The default backend is `c23` and the
default mode is `aot`, so `super b -r <pod>` writes the native executable at
`target/release/<pod>` for bin Pods. `--c23` still selects that stack explicitly.
`--mode bytecode` writes a portable `.superb` module instead. While the build
runs, the CLI prints each step. The current step name is cyan-green, with the
step's text and progress beside it. The last line is `Finished` in cyan-green,
`Failed` in cyan-red, or `Finished with warnings` in cyan-yellow.

`super install` (`super i`) is the ship step for **bin** Pods. From the Pod
directory, or any subdirectory, it builds `src/main.sr` as a native C23 binary
at the maximum release profile and copies it onto `PATH`. The executable name
is the Pod name. Lib and macro Pods are skipped. The directory is `--bin-dir`,
`SUPER_INSTALL_DIR`, a user bin already on `PATH` (`~/.local/bin`, `~/bin`, or
`~/.cargo/bin`), or `~/.local/bin` after Super adds that directory to the shell
startup file. On Windows the same directories live under the user profile and
Super updates the user `Path`. `super uninstall` (`super u`) removes the
recorded binary and leaves `PATH` in place. `super list` shows every receipt.
`super which` prints one path.

`super clean -d` removes debug binaries (`target/dev`). `super clean -r`
removes release binaries (`target/release`). `super clean` and `super clean -a`
remove the whole `target/` directory. A workspace is a directory of Pods, or a
manifest with `[workspace] members`; install and clean act on every member.
`super help` lists every command, and `super help install` prints one.

Pod commands resolve `Pod.toml`, honor `Pod.lock` / `Pod.lock`,
and can vendor source. Applications should commit the lockfile. Library and
macro Pods should document supported version ranges and test their public
modules through a small consumer Pod.

A productive loop is:

```bash
super check examples/17_log_ingest_pipeline/src/main.sr
super test examples/17_log_ingest_pipeline/
super run examples/17_log_ingest_pipeline/src/main.sr
super build --release --c23 examples/17_log_ingest_pipeline/src/main.sr
```

Do not skip `check` merely because one runtime path succeeds. Static diagnostics
are part of the contract, and release lowering may exercise stricter layout or
alias information.

## 44. Diagnostics as design feedback

Stable semantic codes distinguish categories:

- E0101–E0104: names, modules, members, and visibility;
- E0201–E0203: type unification, unhandled optional/error values, and trait conformance;
- E0301–E0302: missing mutation borrow and unsafe concurrent capture;
- E0303–E0305: unused local, parameter, or import;
- E0401: an escaping mutable root or scoped value.

Fix the primary span first. A missing `@` is not solved by suppressing a
warning; it asks whether the call really mutates and whether exclusive access is
available. An escape error asks which lifetime should own the result. An unused
import often means a refactor is incomplete or a module boundary is too broad.

CI should run `super check` and `super test` over package roots, then build the
release target used in deployment. Optimized kernels should have checksum or
reference tests that run through scalar and optimized paths.

## 45. Language server

Start the language server with `super lsp` or the dedicated `super-lsp` binary.
It speaks JSON-RPC 2.0 over standard input/output and supports:

- open/change synchronization and live diagnostics;
- inferred-type, layout, offset, stack-behavior, and trait-signature hover;
- definition and reference navigation through interned symbols/spans;
- completion for visible fields, imports, implicit `it`/`index`, and UFCS
  methods valid for the receiver type;
- semantic tokens that distinguish exported declarations, private fields,
  mutable borrows, types, and implicit names.

Editor clients must not write protocol logs to standard output because that
corrupts JSON-RPC framing. Use standard error or a configured log file.

Incremental updates re-run the smallest practical frontend region, but semantic
truth remains the same as `super check`. If editor output disagrees with the
CLI, save the file and treat CLI diagnostics as authoritative while reporting
the mismatch with a minimal source case.

## 45.1 Agent MCP server

Coding agents should use the embedded MCP knowledge server rather than scraping
docs by hand:

```bash
super mcp
# or the shim:
super-mcp
```

It speaks Model Context Protocol over stdio (newline-delimited JSON-RPC 2.0;
protocol `2024-11-05`). Point an MCP client at the `super mcp` command. Tools
cover:

- language features and cheatsheets (`super_get_feature`, `super_cheatsheet`);
- algorithm sketches in Super (`super_get_algorithm`);
- `examples/` project index (`super_list_examples`, `super_get_example`);
- stdlib coverage and live module source (`super_list_stdlib`, `super_get_stdlib`);
- unified search (`super_search`).

Resources use `super://feature/…`, `super://algorithm/…`, `super://example/…`,
and `super://stdlib/…`. Prompts `learn-super`, `write-super`, and
`explore-stdlib` give agents a guided workflow. Set `SUPER_STDLIB` / `SUPER_STD`
(or install via `make install` → `~/.super/lib/stdlib`) so stdlib tools can open the `.sr` files next to the
compiler.

## 46. Editor support

The repository ships:

- a VS Code TextMate grammar and language configuration under
  `editors/vscode/`;
- a Tree-sitter grammar under `editors/tree-sitter-super/`;
- Zed integration under `editors/zed/`.

Syntax highlighting recognizes exports, private fields, mutation sigils,
compile-time forms, tests, routes, the `trait` keyword, interpolation, and control-flow keywords.
Semantic tokens from the LSP improve on lexical highlighting because they know
whether a name is a type, field, mutable root, or implicit binding.

Editor grammars are not the language specification. A highlighted construct can
still be invalid, and a temporarily unhighlighted valid construct remains valid.
When syntax evolves, update lexer, parser, semantic analysis, formatter/tooling,
TextMate, Tree-sitter, Zed, tests, the specification, and this book together.

## 47. Benchmark methodology

Performance numbers without a contract are stories. A credible Super benchmark
records:

1. exact source revision and Super version;
2. OS, CPU model, core count, memory, and power policy;
3. host C compiler and flags for C23 AOT;
4. VM, JIT, and AOT modes measured separately;
5. warmup count, timed run count, and full distribution;
6. input size and deterministic seed;
7. checksum or output validation against a reference;
8. excluded workloads and the reason for exclusion.

The repository benchmark suite uses ten algorithms and checksum gates. Run the
harness from a quiet machine, allow warmups, and compare medians plus variance.
For very short programs, process startup and timer resolution can dominate;
increase work per process or use an in-process harness while retaining the same
algorithm and checksum.

Report ratios in an unambiguous direction, such as “Super time / C time.” A
value below 1 then means Super was faster for that workload. A geometric mean is
useful across ratios but must list exclusions. It does not replace per-workload
results; a strong Mandelbrot result cannot explain an A* regression.

### Avoiding benchmark traps

- Do not remove required work because a checksum happens not to observe it.
- Do not compare a host intrinsic in one language with a source implementation
  in another without labeling the difference.
- Do not time file generation, compilation, or startup unless the workload is
  specifically about those costs.
- Do not use a parallel implementation against a forced single-thread baseline
  without reporting CPU utilization and both serial versions.
- Do not assume source brevity implies speed.
- Do not tune only the checksum path.

Use profilers after correctness gates pass. Inspect allocation/stack growth,
branch misses, cache behavior, worker balance, generated C, and final assembly.

## 48. C23 AOT

C23 is Super's broad optimizing backend. The frontend lowers checked source to
typed IR/bytecode, then emits a self-contained C translation unit. The host
compiler handles mature scalar optimization, vectorization, register allocation,
and linking.

The emitter can use:

- `static constexpr` data for compile-time values and lookup tables;
- `#embed` as a representation option for large embedded assets;
- `<stdbit.h>` operations for recognized bit primitives;
- `[[unsequenced]]` for eligible pure helpers;
- typed structs and compact tag/payload sums;
- `restrict` on proven exclusive mutable arrays/components;
- OpenMP for recognized independent `par` kernels and reductions.

Generated C is an implementation artifact, but reading it is valuable. Verify
that hot loops have simple bounds, roots carry the intended qualifiers,
compile-time tables are static, and no unexpected runtime constructor appears.
Then inspect compiler optimization remarks or assembly.

C compatibility does not expose arbitrary C undefined behavior to Super source.
The frontend must establish bounds, aliasing, initialization, and lifetime facts
before emitting aggressive forms. Unsupported patterns lower to correct scalar
code.

### Interoperability notes

Named value structs have deterministic C-compatible layout. Validate size and
offset assumptions with compile-time assertions and host-side static assertions.
Do not pass stack-relative pointers beyond their lifetime. Strings and arrays
need explicit ABI descriptors or host binding types, not assumptions about a C
NUL-terminated pointer.

## 49. VM and JIT measurement

The register-oriented VM is the universal execution model and correctness
fallback. The focused JIT recognizes profitable dense numeric kernels on
x86-64 and AArch64, emits packed operations plus scalar tails, and protects
executable memory through an audited RAII buffer.

Measure VM and JIT separately. A JIT run may include compilation overhead unless
the harness warms it. A kernel can be too small to repay compilation. A shape
the JIT does not recognize should remain correct through general bytecode.

When diagnosing a JIT/AOT difference:

1. compare checksums;
2. reduce to the smallest kernel;
3. confirm static types and `@` alias facts;
4. inspect bounds and scalar tail behavior;
5. test lengths around vector width: 0, 1, width−1, width, width+1;
6. compare NaN, infinity, overflow, and signed shift cases where relevant.

Never “fix” a mismatch by weakening the checksum. Optimized and scalar paths are
the same language.

## 50. Release checklist

Before shipping a Super 0.1.0 application:

- run `super check` for every entry module;
- run package and integration tests, including failure paths;
- build the deployment target in release mode;
- verify lockfile and vendored dependency policy;
- bound requests, frames, channels, arrays, and resource counts;
- ensure request/frame data cannot escape into persistent state;
- validate binary protocols and keep a JSON fallback;
- test reconnect, timeout, cancellation, and partial I/O behavior;
- compare optimized kernels against scalar references;
- record performance methodology and checksums;
- verify logs redact secrets and metric labels are bounded;
- validate C ABI layout at every FFI boundary;
- exercise clean shutdown and resource closure;
- retain deterministic seeds and replay fixtures for game/network simulations.

## 51. Where to continue

Use [SPECIFICATION.md](SPECIFICATION.md) for normative grammar and diagnostics. Read `stdlib/mod.sr` for the shipped module catalog, `benchmarks/the latest verified results/results.json` for measurement detail, `docs/the latest verified results` for syntax refinements, and the example tests for executable usage.

Super 0.1.0 is intentionally compact, but it is not magical. Its performance comes from visible layout, bounded lifetime, static specialization, and giving the backend honest alias and parallelism facts. Write the clearest value-oriented program first; then use checksums, diagnostics, and profiles to decide which loops deserve sharper hints.
