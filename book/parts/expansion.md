# Expanded Tutorials

## Tutorial 1: Bindings deep dive

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    port = 8080
    print("{port}")
}
```

## Tutorial 2: Interpolation patterns

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    name = "super"
    scores = [10, 20, 30]
    print("hi {name}")
    print("scores {scores}")
}
```

`hi super` then `scores [10, 20, 30]`. The array has no method of its own. `Display` renders it.

## Tutorial 3: Dense arrays

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    a = [0; 16]
    a[3] = 9
    print("{a[3]}")
}
```

## Tutorial 4: Records and maps

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    m = {a: 1, b: 2}
    print("{m.a}")
    print("{m}")
}
```

`1` then `{a: 1, b: 2}`. A record interpolates through the same witness as an array. Keys are sorted.

## Tutorial 5: Methods and @self

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    /* see ch 9 */
    print("ok")
}
```

## Tutorial 6: when matching

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    /* see ch 10 */
    print("ok")
}
```

## Tutorial 7: Inline => helpers

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+dbl(x: i64) => x + x
+main() {
    print("{dbl(21)}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 8: Iterable reductions

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    xs = [1, 2, 3, 4, 5]
    s = xs.sum()
    squares = xs.map(it * it).sum()
    range_sum = (0..50).sum()
    print("sum={s} squares={squares} range={range_sum}")
}
```

## Tutorial 9: par loops

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    /* AOT OpenMP */
    print("par")
}
```

## Tutorial 10: ensure guards

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    ensure true else { return }
    print("ok")
}
```

## Tutorial 11: or defaults

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    x = none or 3
    print("{x}")
}
```

## Tutorial 12: comptime #for

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
#T = #for i in 0..4 { i }
+main() {
    print("{#T[2]}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 13: std.math.core surface

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.math.core
v = sqrt(16.0)
+main() {
    print("{to_int(v)}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 14: std.core.str surface

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.core.str
n = byte_len("ab")
+main() {
    print("{n}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 15: std.core.collections

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.core.collections
a = arr_new(2, 0)
+main() {
    print("{len(a)}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 16: std.io.buf

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.io.buf
w = RingWriter.new(4)
+main() {
    print("{@w.push(1)}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 17: std.sys.io_uring

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.sys.io_uring
+main() {
    print("uring")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 18: std.net.ws rooms

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
use std.net.ws
r = WsRoom.new(2)
+main() {
    print("{@r.join(1)}")
}
```

Performance note: prefer stack arrays, `@` mutation, and fused `.sum()` Iterable pipelines on hot paths.

Compiler diagnostic tip: unused bindings must be prefixed with `_` (`E0303`).

## Tutorial 19: SoA layout

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    px = [0.0; 8]
    print("{px[0]}")
}
```

## Tutorial 20: Checksum style

Read this section as a self-contained drill. Type it, run `super check`, then `super run`.

```super
+main() {
    cs = (0..10).sum()
    print("CHECKSUM={cs}")
}
```

## Tutorial 21: A trait on the type

Read this section as a self-contained drill. Type it, run `super check`, then `super run`. The header `Pin: Mark` is the conformance. `paint` has to return `str` and take `self`, or the compiler reports `E0203`.

```super
+Mark = trait {
    paint(self) : str
}

+Pin: Mark = {
    label: str = "ok"
    paint(self) : str  => self.label
}

+main() {
    pin = Pin{label: "ok"}
    print(pin.paint())
}
```

## Tutorial 22: Install onto PATH

This drill is the toolchain, not a new language form. From the package directory:

```bash
super install
super which
super list
super uninstall
super clean -d
super clean -r
super clean
```

`super i` is `super install`. It builds `src/main.sr` as a native C23 binary at the maximum release profile (`-O3`, fat LTO, the host CPU, fast-math, unroll, strip) and copies the executable onto `PATH`. The name is the package name. `super u` removes that binary and leaves `PATH` itself alone. `super which` prints only the path. `super clean -d` removes `target/dev`. `super clean -r` removes `target/release`. `super clean`, the same as `super clean -a`, removes the whole `target/` directory.

`super help` lists every command. `super help install` and `super clean --help` print the flags for one command.

## Design essay 1

Universal UFCS applies to every receiver type (arrays, strings, floats — not only numbers): `recv.f(args…)` desugars to `f(recv, args…)` and chains left-to-right.

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 1 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 2

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 2 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 3

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 3 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 4

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 4 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 5

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 5 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 6

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 6 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 7

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 7 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 8

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 8 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 9

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 9 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 10

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 10 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 11

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 11 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 12

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 12 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 13

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 13 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 14

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 14 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 15

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 15 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 16

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 16 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 17

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 17 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 18

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 18 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 19

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 19 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 20

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 20 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 21

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 21 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 22

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 22 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 23

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 23 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 24

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 24 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 25

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 25 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 26

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 26 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 27

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 27 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 28

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 28 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 29

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 29 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```

## Design essay 30

Super 0.1.0 keeps the mental model small: stacks, `@` borrows, and a C23-shaped AOT path. Essay 30 revisits that idea from a different angle — latency budgets, SoA layouts, checksum-gated benchmarks, and idiomatic stdlib wrappers that hide host intrinsics. When you write servers, start with `std.io.poll` or `std.sys.io_uring`. When you write games, start with SoA columns and fixed timesteps. When you write pipelines, start with `.sum()` / fused Iterable pipelines and comptime tables. The same language should feel at home in all three.

```super
use std.math.core
+main() {
    print("{to_int(floor(sqrt(49.0)))}")         // 7
    print("{49.0.sqrt().floor().to_int()}")      // identical UFCS chain
    hello = "Hello"
    print("{hello.len()}")                       // universal string byte length
}
```
