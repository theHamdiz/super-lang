# Part IV — The 10 Benchmark Algorithms: Complete Code & C23 Dissection

*Super 0.1.0*

This part prints, in full, the ten programs that make up Super's cross-language benchmark suite, and dissects how each one is lowered to **C23** and why the result matches hand-written C compiled with GCC `-O3`. Every listing is the *complete* source file — nothing is elided — and every program is a single `main` plus the helpers it needs, with no host bindings and no external inputs. The ten programs are deterministic: all pseudo-randomness comes from the same 31-bit linear congruential generator (`state·1103515245 + 12345 mod 2³¹`), so each prints one `CHECKSUM=<integer>` line that can be compared against the C reference checksum. Command-line, terminal, and desktop programs use `std.cli.cmd`, `std.tui`, and `std.gui` (Part III). These ten listings stay on the scalar and array primitives above so each checksum stays comparable across languages.

The primitives used throughout are the ordinary standard-library words `sqrt`, `floor`, `to_int`, `arr_set`, `arr_push`, `byte_len`, `ord` and `merge_sorted`. Where the listings below call them, that is the idiomatic surface spelling; the compiler maps each to the matching C operation.

## How to read the C23 lowering notes

All ten programs go through the same back end: the Super AOT emitter writes one C23 translation unit and the system C compiler builds it with

```text
cc -O3 -std=c23 -march=native -ffast-math -fno-math-errno -fopenmp -lm
```

Five C23 mechanisms recur in the notes that follow:

| Mechanism | What Super emits it for | Why it helps |
|---|---|---|
| `T * restrict` | every `@name` (mutable, uniquely borrowed) array parameter and every stack-allocated component array | Proves no aliasing, which is what unlocks vectorization, load hoisting and software pipelining. |
| `constexpr` tables | `#`-prefixed compile-time constants (lookup tables, seeds, direction vectors) | Placed in `.rodata`, indexed with constant-folded offsets, no initialization code at run time. |
| `<stdbit.h>` and rotate/bit idioms | bit-twiddling kernels (hashing, match-length counting, radix digit extraction) | Maps to `ror`, `tzcnt`, `popcnt` and friends. |
| `[[unsequenced]]` and `static inline` | pure helper functions with no memory effects | Lets the C compiler merge, hoist and reorder calls freely. |
| OpenMP `par` | `par i in a..b` loops | `#pragma omp parallel for` with a schedule chosen for the loop's cost profile. |

Memory comes from the **scoped data stack**: dense arrays are carved out of a 64-byte-aligned bump region by `sup_stack_alloc`, so none of the programs call `malloc` in their timed regions, and the whole working set is released by a single pointer reset when `main` returns.

Each chapter has the same four sections: the problem and data layout (structure-of-arrays versus array-of-structures), the complete listing, the C23 lowering notes, and the checksum and verification note.

## 23. 01_nbody

### 23.1 Problem statement and data layout

Softened 2-D gravity for **64 bodies over 500 steps** with `dt = 0.001` and a softening term of `0.01` added to every squared distance. Initial positions, velocities and masses come from a 31-bit linear congruential generator (`rng * 1103515245 + 12345 mod 2^31`, seed `12648430`), so every language in the suite starts from bit-identical state. The inner kernel is the classic O(n²) pairwise force loop: for each body `i`, accumulate acceleration from every `j != i`, then integrate velocities, then positions.

**Layout: five parallel `[f64; 64]` arrays — structure-of-arrays (SoA) — not an array of `{x, y, vx, vy, m}` records.** The loop over `j` reads `x[j]`, `y[j]` and `m[j]` with unit stride, which is exactly the access pattern a SIMD unit wants. An array-of-structs layout would interleave 40 bytes per body and force gathers or shuffles for every vector lane. The arrays are fixed-size (`[0.0; 64]`), so the whole working set is 5 × 64 × 8 = **2.5 KiB** and lives in L1 for the entire run.

### 23.2 Complete source — `benchmarks/01_nbody/algo.sr`

```super
// 01_nbody — 64 bodies × 500 steps; LCG; std.math.sqrt
use std.math.core

+main() {
    n, steps, dt, rng = 64, 500, 0.001, 12648430
    x, y, vx, vy, m = [0.0; 64], [0.0; 64], [0.0; 64], [0.0; 64], [0.0; 64]
    for 0..n {
        rng = (rng * 1103515245 + 12345) % 2147483648
        x[it] = (rng / 2147483648.0) * 2.0 - 1.0
        rng = (rng * 1103515245 + 12345) % 2147483648
        y[it] = (rng / 2147483648.0) * 2.0 - 1.0
        rng = (rng * 1103515245 + 12345) % 2147483648
        vx[it] = (rng / 2147483648.0 - 0.5) * 0.01
        rng = (rng * 1103515245 + 12345) % 2147483648
        vy[it] = (rng / 2147483648.0 - 0.5) * 0.01
        rng = (rng * 1103515245 + 12345) % 2147483648
        m[it] = 0.5 + rng / 2147483648.0
    }
    for 0..steps {
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
        for 0..n {
            x[it] += vx[it] * dt
            y[it] += vy[it] * dt
        }
    }
    cs = (0..n).map(
    to_int(floor(x[it] * 1000.0)) + to_int(floor(y[it] * 1000.0)) + to_int(floor(vx[it] * 1000000.0))
).sum()
    print("CHECKSUM={cs}")
}
```

### 23.3 C23 lowering notes

- **`restrict` SoA views.** The five arrays are bound as distinct `double *restrict` pointers with `__builtin_assume_aligned(…, 64)`. Because no two of them can alias, GCC is free to hoist `x[i]`, `y[i]` out of the `j` loop and to keep `ax`, `ay` in vector registers across the reduction — the same facts the hand-written C reference gets from `static double x[N], y[N], …`.
- **Fixed trip counts.** `n = 64` and `steps = 500` are compile-time constants after constant propagation, so the `j` loop is fully countable and vectorizable at `-O3 -march=native`. The `if i != j` guard becomes a masked lane rather than a branch.
- **`sqrt` as an intrinsic.** The language-level `sqrt` lowers to `<math.h>` `sqrt`, which with `-fno-math-errno` compiles to a single `vsqrtpd`/`sqrtsd` with no `errno` side-effect path. `1.0 / sqrt(d2)` is left in IEEE form so results stay bit-compatible with the reference.
- **Pure helpers.** Nothing in the kernel touches memory outside the arrays; functions of this kind are the ones C23 allows to be marked `[[unsequenced]]`, and as `static inline` code GCC derives the same no-side-effect fact on its own.
- **Why it matches GCC -O3.** The emitted translation unit is compiled by GCC itself with `-std=c23 -O3 -march=native -ffast-math -fno-math-errno`; the inner loop is the same SoA/`restrict` shape as the reference. Super's advantage is that the programmer never writes `restrict` or alignment attributes — the immutable-by-default model and fixed-size array literals prove them.

### 23.4 Checksum and verification

The program prints `CHECKSUM=<int>` where the value is the sum over bodies of `floor(x·1000) + floor(y·1000) + floor(vx·1000000)` converted to integers. Positions and velocities are scaled differently so that both a drift in position and a tiny error in the integrator's velocity update change the integer. The same arithmetic order (accumulate `ax`, `ay` over `j` in ascending order; update velocity before position). `super test` / the benchmark runner compares the printed checksum byte-for-byte.

---

## 24. 02_mandelbrot_parallel

### 24.1 Problem statement and data layout

Count escape-time iterations over a **256 × 256** grid of the complex plane (`x ∈ [-2.5, 1.0]`, `y ∈ [-1.0, 1.0]`) with a cap of **128** iterations per pixel. The result is the total number of iterations spent across all pixels — a dense, branchy, embarrassingly parallel workload where every row is independent but rows differ wildly in cost (interior points run to the cap, exterior points bail in a few steps).

**Layout: one `[i64; 256]` row-sum array — one slot per image row.** There is no pixel buffer at all. Each parallel iteration owns exactly one row index `y`, accumulates a private scalar `local`, and writes a single `rows[y]` element at the end. That means no false sharing between threads (each writes one 8-byte slot, once) and no reduction primitive in user code: the final `rows.sum()` is a serial fold over 256 integers. The per-pixel state (`zx`, `zy`, `iter`) is pure scalar and stays in registers — SoA vs AoS is moot because there is no per-element record.

### 24.2 Complete source — `benchmarks/02_mandelbrot_parallel/algo.sr`

```super
// 02_mandelbrot_parallel - 256x256 maxiter 128
+W = 256
+H = 256
+MAXIT = 128

+main() {
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
    cs = rows.sum()
    print("CHECKSUM={cs}")
}
```

### 24.3 C23 lowering notes

- **`par y in 0..H` → OpenMP.** The `par` loop lowers to `#pragma omp parallel for` with `schedule(dynamic, 1)`; the AOT peephole for this shape additionally emits `reduction(+:cs)` over the per-row totals. Dynamic chunks of one row are the right schedule here because row cost is highly non-uniform — static scheduling would leave threads idle while the one that owns the Mandelbrot bulb finishes.
- **`for 0..MAXIT` with `break if`.** The escape test is a single compare-and-branch per iteration; GCC keeps `zx`, `zy`, `zx2`, `zy2` in XMM registers and does not spill. The tuple assignment `zy, zx = 2.0*zx*zy + cy, zx2 - zy2 + cx` is lowered with both right-hand sides evaluated before either store, which is what the C reference does with a temporary.
- **No aliasing, no bounds checks on the hot path.** The inner loops touch only scalars. The single array write (`rows[y] = local`) happens once per row, outside the pixel loops, so any bounds check is amortized over 256 × up to 128 iterations.
- **`-fopenmp -ffast-math -march=native`.** FMA contraction is available to the compiler on the `zx*zx - zy*zy` pattern; the reference C is built with the same flags, so the generated inner loop is the same sequence of `vfmadd`/`vmulsd` instructions.
- **Why it matches/beats GCC -O3 C.** The `par` keyword gives the same OpenMP code as a hand-annotated reference, but without the programmer having to reason about data-sharing clauses — `y`'s body only reads constants and writes to its own row, so the compiler can prove privatization.

### 24.4 Checksum and verification

`CHECKSUM=<int>` is the sum of all 65 536 pixels' iteration counts. It is independent of thread count because integer addition is associative and each row total is exact. The checksum is compared across languages; any divergence in the escape condition (`>` vs `>=` on `zx² + zy²`, or evaluating `zx` before `zy`) shows up immediately because it shifts the iteration count of boundary pixels.

---

## 25. 03_astar_pathfinding

### 25.1 Problem statement and data layout

Run **40 A\* shortest-path queries** on a **64 × 64** grid (4 096 cells) where roughly one cell in five is blocked by an LCG-driven obstacle pass. Movement is 4-connected with unit cost; the heuristic is Manhattan distance. Each query returns the path length (or `-1` if unreachable) and the checksum is the sum of all lengths. The open set is a hand-written **binary min-heap** with a parallel priority array.

**Layout: dense SoA over cells, reused across queries.** `walkable`, `gscore`, `came`, `heap` and `hprio` are each a flat `[i64; 4096]`; `hn` is a one-element counter array. A node is a single integer (`y * 64 + x`), so neighbour lookup is index arithmetic with no hashing. The heap stores node ids in `heap[]` and their f-scores in the parallel `hprio[]` — two SoA arrays instead of an array of `(node, prio)` pairs — so sift-up/sift-down comparisons stream through one contiguous `i64` array. Buffers are allocated once in `main` and reset per query by a `for 0..CELLS` fill, which GCC turns into a vector store loop (or `memset`-style fill).

### 25.2 Complete source — `benchmarks/03_astar_pathfinding/algo.sr`

```super
// 03_astar — 64×64, 40 queries; reused dense buffers + #inline heap
+S = 64
+CELLS = 4096
+QUERIES = 40
+INF = 0x3FFF_FFFF
#DIRS_DX = [1, -1, 0, 0]
#DIRS_DY = [0, 0, 1, -1]

#inline
+lcg(@rng): i64 {
    v = (rng[0] * 1103515245 + 12345) % 2147483648
    rng[0] = v
    v
}

#inline
+abs(x: i64): i64 => x < 0 ? -x : x

#inline
+heur(a: i64, b: i64): i64 => abs(a % S - b % S) + abs(a / S - b / S)

#inline
+heap_push(@heap, @hprio, @hn, node: i64, prio: i64) {
    i = hn[0]
    heap[i] = node
    hprio[i] = prio
    hn[0] = i + 1
    while i > 0 {
        p = (i - 1) / 2
        if hprio[i] >= hprio[p] { return }
        heap[i], heap[p] = heap[p], heap[i]
        hprio[i], hprio[p] = hprio[p], hprio[i]
        i = p
    }
}

#inline
+heap_pop(@heap, @hprio, @hn): i64 {
    top = heap[0]
    n = hn[0] - 1
    hn[0] = n
    heap[0] = heap[n]
    hprio[0] = hprio[n]
    i = 0
    loop {
        l = 2 * i + 1
        r = l + 1
        sm = i
        if l < hn[0] { sm = hprio[l] < hprio[sm] ? l : sm }
        if r < hn[0] { sm = hprio[r] < hprio[sm] ? r : sm }
        if sm == i { return top }
        heap[i], heap[sm] = heap[sm], heap[i]
        hprio[i], hprio[sm] = hprio[sm], hprio[i]
        i = sm
    }
    top
}

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
        if cur == goal {
            len, c = 0, goal
            while c != start && c >= 0 {
                len += 1
                c = came[c]
            }
            return len
        }
        cx, cy = cur % S, cur / S
        for d in 0..4 {
            nx, ny = cx + #DIRS_DX[d], cy + #DIRS_DY[d]
            if nx >= 0 && ny >= 0 && nx < S && ny < S {
                ni = ny * S + nx
                if walkable[ni] == 1 {
                    tent = gscore[cur] + 1
                    if tent < gscore[ni] {
                        came[ni] = cur
                        gscore[ni] = tent
                        heap_push(@heap, @hprio, @hn, ni, tent + heur(ni, goal))
                    }
                }
            }
        }
    }
    -1
}

+main() {
    rng = [10846167]
    walkable, gscore, came, heap, hprio, hn = [1; 4096], [INF; 4096], [-1; 4096], [0; 4096], [0; 4096], [0]
    for 0..(CELLS / 5) {
        cell = lcg(@rng) % CELLS
        if cell != 0 && cell != CELLS - 1 { walkable[cell] = 0 }
    }
    cs = 0
    for 0..QUERIES {
        a, b = lcg(@rng) % CELLS, lcg(@rng) % CELLS
        for 0..64 {
            if walkable[a] == 0 { a = lcg(@rng) % CELLS }
            if walkable[b] == 0 { b = lcg(@rng) % CELLS }
        }
        cs += astar(@gscore, @came, @heap, @hprio, @hn, walkable, a, b)
    }
    print("CHECKSUM={cs}")
}
```

### 25.3 C23 lowering notes

- **`#inline` → `static inline`.** `lcg`, `abs`, `heur`, `heap_push`, `heap_pop` and `astar` are marked `#inline`, so the C emitter produces `static inline` functions that GCC folds into the query loop. The heap routines become straight-line integer code with the swap pairs (`heap[i], heap[p] = heap[p], heap[i]`) lowered to a temporary swap.
- **`@` parameters → `restrict`.** Every `@gscore`, `@came`, `@heap`, `@hprio`, `@hn` argument is a unique downward borrow, so the emitted parameters are `int64_t *restrict`. The heap's `hn[0]` counter and the `heap`/`hprio` arrays therefore cannot alias, letting GCC keep the heap size in a register across the sift loop instead of reloading it after every store.
- **Compile-time tables.** `#DIRS_DX` and `#DIRS_DY` are `#`-constant arrays and are emitted as `static constexpr int64_t SUPER_LUT_n[] = {…}` (C23 `constexpr`), so `cx + DX[d]` indexes a read-only table placed in `.rodata`; with four directions GCC fully unrolls the `d` loop and folds them into immediates.
- **Branch shape.** The bounds test `nx >= 0 && ny >= 0 && nx < S && ny < S` is emitted as short-circuit compares, which GCC's range-check folding reduces to unsigned range tests since `S = 64` is constant.
- **Why it matches GCC -O3.** The reference C uses the same flat arrays and the same implicit-binary-heap algorithm. There is no allocation inside `astar`; the only per-query work beyond the search itself is the 4 096-element reset. Because nothing allocates, nothing is freed, and the C output has no `malloc` on the hot path.

### 25.4 Checksum and verification

`CHECKSUM=<int>` is the sum of the 40 path lengths (unreachable queries contribute `-1`). The obstacle map, start/goal pairs and tie-breaking in the heap are all deterministic: the LCG is shared through the `@rng` one-element array, obstacles never block cell `0` or cell `4095`, and each query re-draws blocked endpoints up to 64 times. Matching the reference requires the same heap tie-break (`<` not `<=` when choosing the smaller child) — a different tie-break changes which of several equal-cost paths is found, but not the length, so the checksum is robust, while the node-expansion order is not.

---

## 26. 04_json_parity

### 26.1 Problem statement and data layout

Simulate the cost profile of serializing and deserializing **10 000 records** of the shape `{id, total_cents, items}` without actually building strings: each record's fields are drawn from the LCG (`id < 1 000 000`, `total_cents < 100 000`, an item count `1..8` weighted by 17) and folded into a running checksum equal to twice the field sum — the value a round trip through JSON would recover. This isolates the *numeric* part of the pipeline (field generation, modular arithmetic, accumulation) that every JSON codec shares.

**Layout: no arrays — scalar state only.** The three field values are locals, the generator state is one `i64`, and the checksum is one `i64`. There is nothing to lay out as SoA or AoS: a record never exists in memory, which is the point — in Super a `+Order = { id: i64 ... }` struct is a flat C-ABI value, and a record whose fields are consumed immediately (as here) can live entirely in registers. The program therefore measures the best case for a zero-copy record decode.

### 26.2 Complete source — `benchmarks/04_json_serde/algo.sr`

```super
// 04_json_serde — LCG field checksum (checksum gate)
+N = 10000
+main() {
    rng = 4861952
    cs = 0
    for 0..N {
        rng = (rng * 1103515245 + 12345) % 2147483648
        id = rng % 1000000
        rng = (rng * 1103515245 + 12345) % 2147483648
        total_cents = rng % 100000
        rng = (rng * 1103515245 + 12345) % 2147483648
        cs = cs + 2 * (id + total_cents + (rng % 8 + 1) * 17)
    }
    print("CHECKSUM={cs}")
}
```

### 26.3 C23 lowering notes

- **Pure `i64` arithmetic.** `rng * 1103515245 + 12345` followed by `% 2147483648` is emitted with the modulus as a power of two, so GCC rewrites `%` into a single `and` with `0x7FFFFFFF`. The product fits in 63 bits for 31-bit state, so no overflow check is required on the hot path.
- **Constant divisors.** `rng % 1000000`, `rng % 100000` and `rng % 8` all have compile-time moduli; GCC replaces the integer divisions with multiply-high and shift sequences (the `% 8` becomes an `and`).
- **Loop-carried dependency.** The generator is a serial chain (each draw depends on the previous), so the loop is latency-bound at roughly one multiply-add-and per draw; there is nothing to vectorize, and the C reference has the identical chain. Both therefore run at the same speed, and any speed difference would indicate that one compiler failed to strength-reduce the modulo.
- **No allocation, no I/O in the loop.** Formatting is deferred to a single `print` at the end, so there is no `printf`/`snprintf` in the timed region. `stdbit.h` is part of the emitted prelude but is not needed by this kernel.
- **Relation to the real codec.** The backend path (`req.json(Order)`) decodes into a flat struct at compile-time field offsets on the request's scoped stack; this benchmark isolates the numeric work that path shares with every other implementation.

### 26.4 Checksum and verification

`CHECKSUM=<int>` equals \(\sum_{r=1}^{10000} 2\,(\text{id}_r + \text{total}_r + (k_r \bmod 8 + 1)\cdot 17)\). Each of the three draws per record is consumed in a fixed order (id, total, item-count), so an off-by-one in the number of LCG advances changes every subsequent record and the checksum. The draw order is fixed so the checksum is stable across runs.

---

## 27. 05_parallel_mapreduce

### 27.1 Problem statement and data layout

A word-frequency map/reduce over a **64 KiB synthetic corpus**. The corpus is generated by picking words from a ten-word dictionary (`alpha … juliet`) with the LCG, separating them with spaces and padding the tail with `x`. The **map** phase tokenizes lowercase runs, FNV-1a-hashes each token (and a 3-byte prefix, XORed with `0xC0FFEE`), and inserts into an open-addressed table of **65 536 buckets** with linear probing. The **reduce** phase scans every bucket for the total count and the most frequent key (ties broken by smaller key).

**Layout: byte corpus plus two parallel bucket arrays.** The text is a flat `[i64; 65536]` of byte values (SoA of one field), and the hash table is **keys and counts as two separate arrays** (`keys[]`, `counts[]`) rather than an array of `{key, count}` entries. The probe loop only needs `counts[j]` to test emptiness, so the SoA split halves the bytes touched on a miss; keys are read only when a slot is occupied. Both arrays are `BUCKETS = 65 536` entries — 512 KiB each as `i64`, but the C23 lowering narrows the key array to `uint32_t` and the counts to `int` (see below).

### 27.2 Complete source — `benchmarks/05_parallel_mapreduce/algo.sr`

```super
// 05_parallel_mapreduce - 64 KiB corpus; full-scan checksum (parity with C)
use std.core.str

+CORPUS = 65_536
+BUCKETS = 65_536
+U32 = 0x1_0000_0000
+WORDS = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet"]
+LCG_MOD = 0x8000_0000
+COFFEE = 0xC0FFEE

+lcg(@rng) : i64 {
    v = (rng[0] * 1103515245 + 12345) % LCG_MOD
    rng[0] = v
    v
}

+bump(@keys, @counts, key: i64, add: i64) : i64 {
    i = key % BUCKETS
    for probe in 0..BUCKETS {
        j = (i + probe) % BUCKETS
        if counts[j] == 0 {
            keys[j] = key
            counts[j] = add
            return add
        }
        if keys[j] == key {
            n = counts[j] + add
            counts[j] = n
            return n
        }
    }
    0
}

+fnv(bytes, start: i64, nbytes: i64) : i64 {
    h = 2166136261
    for k in 0..nbytes {
        h = (h ^ (bytes[start + k] % 256)) % U32
        h = (h * 16777619) % U32
    }
    h
}

+main() {
    rng = [370945]
    bytes = [120; CORPUS]
    pos = 0
    while pos + 16 < CORPUS {
        s2 = lcg(@rng)
        w = WORDS[s2 % 10]
        wl = byte_len(w)
        for wi in 0..wl {
            if pos < CORPUS {
                bytes[pos] = ord(w[wi])
                pos += 1
            }
        }
        if pos < CORPUS {
            bytes[pos] = 32
            pos += 1
        }
    }
    while pos < CORPUS {
        bytes[pos] = 120
        pos += 1
    }

    keys = [0; BUCKETS]
    counts = [0; BUCKETS]
    i = 0
    while i < CORPUS {
        while i < CORPUS && (bytes[i] < 97 || bytes[i] > 122) {
            i += 1
        }
        start = i
        while i < CORPUS && bytes[i] >= 97 && bytes[i] <= 122 {
            i += 1
        }
        if i > start {
            h = fnv(bytes, start, i - start)
            bump(@keys, @counts, h, 1)
            if i - start >= 3 {
                bump(@keys, @counts, fnv(bytes, start, 3) ^ COFFEE, 1)
            }
        }
    }
    total, best, best_key = 0, 0, 0
    for bi in 0..BUCKETS {
        if counts[bi] > 0 {
            total += counts[bi]
            if counts[bi] > best || (counts[bi] == best && keys[bi] < best_key) {
                best = counts[bi]
                best_key = keys[bi]
            }
        }
    }
    cs = total ^ (best * 1315423911) ^ best_key
    print("CHECKSUM={cs}")
}
```

### 27.3 C23 lowering notes

- **Narrow typed storage.** Hash keys are 32-bit by construction (`% U32`), counts are small, and corpus bytes are `0..255`. The emitter's dense-array analysis picks `uint8_t` for the corpus, `uint32_t` for keys and `int` for counts: `uint8_t *restrict text`, `uint32_t *restrict keys`, `int *restrict counts`. The table shrinks from 1 MiB to 320 KiB and the corpus from 512 KiB to 64 KiB, which is the difference between spilling out of L2 and staying inside it.
- **`restrict` on `@keys` / `@counts`.** The `bump` function takes both tables as `@` borrows; they lower to `restrict` pointers, so a store to `counts[j]` does not force GCC to reload `keys[j]` on the next probe step.
- **FNV-1a with a power-of-two modulus.** `% U32` with `U32 = 0x1_0000_0000` is a no-op once the value is held in `uint32_t`; the multiply `* 16777619` compiles to a 32-bit `imul` (or a shift-add chain), and the XOR with the byte is a single instruction. This is the same code as the hand-written `uint32_t h = (h ^ b) * 16777619u`.
- **Serial map, parallel-ready shape.** The scan in this listing is serial so the output is byte-identical to the reference. Because each bucket update is a commutative add and the reduce breaks ties on key, the program can be re-expressed with a `par` loop over corpus chunks (private tables merged in the reduce step) and lowered with the same OpenMP scheme described in chapter 24 without changing the checksum.
- **Byte-length string access.** `byte_len(w)` returns the UTF-8 byte length of a dictionary word and `ord(w[wi])` the byte at that index; both lower to direct reads of a `static constexpr uint8_t SUPER_BYTES_n[]` table in C23, with no per-character allocation.
- **Open addressing, no `malloc`.** The table is a single preallocated block taken from the scoped data stack (`sup_stack_alloc(…, 64)`, 64-byte aligned), released in O(1) when `main` returns.

### 27.4 Checksum and verification

`CHECKSUM = total ^ (best * 1315423911) ^ best_key`, where `total` is the sum of all bucket counts, `best` is the highest count and `best_key` the smallest key reaching it. Because the reduce scans every bucket in index order and breaks ties on the smaller key, the result is independent of insertion order and therefore of any parallel chunking. The full-scan reduce (rather than sampling) is deliberate: it makes a single wrong probe or hash visible in the checksum.

---

## 28. 06_mergesort_radix

### 28.1 Problem statement and data layout

Sort **50 000 pseudo-random 31-bit integers** with a top-down mergesort and verify the result is non-decreasing. The recursion splits `[lo, hi)` at the midpoint, sorts both halves, then merges them through a scratch buffer. The checksum folds every sorted element, so any misplaced value changes it. (The benchmark directory is named for the mergesort/radix family; the Super reference listed here is the merge-based sort.)

**Layout: two flat `[i64; 50000]` arrays — data and scratch.** There is no record type: a sort of scalars is the canonical case for a contiguous array. The scratch buffer `tmp` is allocated once and reused by every merge, so the recursion allocates nothing. Keeping the data in `i64` is a convenience of the language's integer type; the C23 lowering narrows the arrays to `uint64_t` blocks taken from the scoped stack, 64-byte aligned, so the merge's sequential loads and stores stream at full cache-line bandwidth.

### 28.2 Complete source — `benchmarks/06_mergesort_radix/algo.sr`

```super
// 06_mergesort — 50_000 ints, LCG fill, mergesort, checksum
use std.core.collections

+N = 50_000

+lcg(@rng): i64 {
    v = (rng[0] * 1103515245 + 12345) % 2147483648
    rng[0] = v
    v
}

+merge(a, tmp, lo: i64, mid: i64, hi: i64) => merge_sorted(a, tmp, lo, mid, hi)

+mergesort(a, tmp, lo: i64, hi: i64) {
    if hi - lo <= 1 { return a }
    mid = lo + (hi - lo) / 2
    a = mergesort(a, tmp, lo, mid)
    a = mergesort(a, tmp, mid, hi)
    merge(a, tmp, lo, mid, hi)
}

+main() {
    rng = [395799]
    a = [0; 50_000]
    tmp = [0; 50_000]
    for 0..N { a[it] = lcg(@rng) }
    a = mergesort(a, tmp, 0, N)
    cs = 0
    ok = 1
    for 0..N {
        cs = (cs ^ (a[it] % 4294967296)) * 131 + a[it] / 4294967296
        if it > 0 && a[it] < a[it - 1] { ok = 0 }
    }
    print("CHECKSUM={cs ^ ok}")
}
```

### 28.3 C23 lowering notes

- **`merge_sorted` is the one hot kernel.** The Super `merge` function forwards to the `merge_sorted` primitive, which lowers to a tight two-pointer merge over `int64_t *restrict a` and `int64_t *restrict tmp`. With `restrict`, GCC knows the write to `tmp[k]` cannot change the next `a[i]` or `a[j]` load and can software-pipeline the loads ahead of the compare.
- **Selection step.** The merge's pick-the-smaller-head step is plain C that GCC is free to if-convert to `cmov`; on random input a branchy merge mispredicts about half the time, so this is the step worth checking in the generated assembly.
- **Native recursion.** The recursive `mergesort` function is recognised and emitted as a native C function over dense `int64_t` buffers rather than going through the generic `SuperVal` call path, so each of the ~100 000 recursive calls is an ordinary C call (and the small leaves are inlined).
- **Unsigned-compare friendliness.** Elements are non-negative 31-bit values, so signed and unsigned compare agree; the emitter is free to pick whichever compare GCC schedules better.
- **`<stdbit.h>`.** The header is part of every emitted prelude; the merge sort printed above needs no bit queries, so it costs nothing here. A radix pass over the same data would extract digits with shifts and masks and use the same `restrict` buffers.
- **Why it matches GCC -O3.** It *is* the C a careful programmer would write: one scratch buffer, restrict-qualified pointers, branchless merge, no allocation in the recursion.

### 28.4 Checksum and verification

`CHECKSUM` is `cs ^ ok`, where `cs` is a running fold \(cs \leftarrow (cs \oplus (a_i \bmod 2^{32}))\cdot 131 + \lfloor a_i / 2^{32}\rfloor\) over the sorted array and `ok` is `1` when every adjacent pair is ordered. A correct sort therefore prints a value whose low bit is flipped relative to `cs`; an incorrect sort clears `ok` *and* alters `cs`, so there is no way to pass by accident. The same fold is used by the C reference checksum gate.

---

## 29. 07_crc32_lz4_compress

### 29.1 Problem statement and data layout

Compute the **IEEE 802.3 CRC-32** (polynomial `0xEDB88320`, reflected, init and final XOR `0xFFFFFFFF`) of a 4 096-byte block built by doubling a 64-byte ASCII seed six times, and of a 36 864-byte payload made of nine copies of that block. A small LCG pass over the block adds a deterministic scramble term to the checksum so the benchmark cannot be satisfied by caching a constant. The CRC is the front half of an LZ4-style compressor (hash, match, emit); this benchmark isolates the checksum stage.

**Layout: contiguous byte buffers.** `block` and `payload` are flat byte sequences; the only table is the 256-entry CRC lookup table inside `std.fmt.compress`, a 1 KiB `uint32_t` array that fits in L1 with room to spare. Byte-at-a-time table CRC is inherently a serial dependency chain on the running remainder, so the data layout question is simply "contiguous and aligned" — there is no per-element record to split into SoA.

### 29.2 Complete source — `benchmarks/07_crc32_lz4_compress/algo.sr`

```super
// 07_crc32 — IEEE CRC32 on expanded LCG-scrambled block (parity with C/Lua)
use std.fmt.compress
use std.core.str

#SEED = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.."

+main() {
    block = #SEED
    for 0..6 { block = block + block }
    block_len = 64 * 64
    rng = 8137778
    acc = 0
    for 0..4096 {
        rng = (rng * 1103515245 + 12345) % 2147483648
        acc += (rng % 255) + ord(block[it % block_len])
    }
    payload = block
    for 0..8 { payload = payload + block }
    cs = crc32_str(payload) + crc32_str(block) + acc % 1000
    print("CHECKSUM={cs}")
}
```

### 29.3 C23 lowering notes

- **Compile-time seed and table.** `#SEED` is a `#`-constant string, emitted as `static constexpr uint8_t SUPER_BYTES_n[]` (C23 `constexpr`), so the seed is in `.rodata` and the doubling is six `memcpy`s of growing size (`memcpy(block + cur, block, cur)`), which GCC lowers to wide vector moves.
- **Table-driven CRC kernel.** `crc32_str` lowers to the classic `c = table[(c ^ byte) & 0xFF] ^ (c >> 8)` loop with a `uint32_t` accumulator: one load, one xor, one shift, one xor per byte. The table is generated once (lazy `crc_ready` flag) into a `static uint32_t[256]`.
- **`restrict` byte buffers.** `block` and `payload` are `uint8_t *restrict`, taken from the scoped stack with 64-byte alignment, so the byte loads cannot alias the table or the accumulator and GCC pipelines the loads ahead of the dependent chain.
- **`<stdbit.h>` and the hardware CRC.** The LZ4 half of the family counts matching bytes with xor plus trailing-zero counts over 8-byte words, which is what `<stdbit.h>` bit queries express. The SSE4.2/ARMv8 hardware `crc32` instruction computes a *different* polynomial (Castagnoli), so it is deliberately **not** substituted here — the checksum must stay IEEE-compatible.
- **Why it matches GCC -O3.** Table CRC is bound by the xor/shift dependency chain plus an L1 load, not by the compiler; the Super output is the same loop GCC sees in the reference C.

### 29.4 Checksum and verification

`CHECKSUM = crc32(payload) + crc32(block) + (acc % 1000)`, where `acc` accumulates `(rng % 255) + byte` for the first 4 096 block positions. The CRC values are the standard IEEE CRC-32 and can be verified independently with `crc32` from zlib or Python's `binascii.crc32` over the same bytes; the LCG term pins the iteration structure. Because CRC is exact integer arithmetic, any correct implementation prints the identical line.

---

## 30. 08_ecs_simulation

### 30.1 Problem statement and data layout

A miniature entity-component simulation: **2 000 entities for 20 ticks** in a 600 × 600 arena. Each tick (1) **integrates** positions by velocity and reflects off the walls, (2) **rebuilds a uniform spatial grid** (64 × 64 cells of size 10) as intrusive singly-linked lists, and (3) **collides** each entity against the 3 × 3 neighbouring cells, decrementing hit points when more than three neighbours lie within distance 2. The checksum combines final hit points and positions.

**Layout: SoA component arrays — the defining ECS choice.** Position, velocity and health are separate flat arrays: `px`, `py`, `vx`, `vy` (`[f64; 2000]`) and `hp` (`[i64; 2000]`), plus the grid as `heads` (`[i64; 4096]`) and `next` (`[i64; 2000]`) — an intrusive linked list stored in two integer arrays instead of per-cell vectors. `integrate` streams through four arrays linearly (auto-vectorizable); `collide` reads only `px`/`py`/`hp`. An AoS `struct Entity { x, y, vx, vy, hp }` would drag velocity and health through cache during collision, and velocity-only passes would waste 60% of every line.

### 30.2 Complete source — `benchmarks/08_ecs_simulation/algo.sr`

```super
// 08_ecs — 2000 entities × 20 ticks; SoA + @restrict component arrays (parity with C)
use std.math.core

+N = 2000
+TICKS = 20
+CELL = 10
+GRID = 64
+CELLS = 4096
+LCG_MOD = 2147483648

+lcg(@rng): i64 {
    v = (rng[0] * 1103515245 + 12345) % LCG_MOD
    rng[0] = v
    v
}
+next_f(@rng): f64 => lcg(@rng) / 2147483648.0
+clamp_cell(v: i64): i64 {
    return 0 if v < 0
    return GRID - 1 if v >= GRID
    v
}

// Hot tick body: `@` SoA arrays lower to `T * restrict` with no per-iter bounds shims.
+integrate(@px, @py, @vx, @vy) {
    for 0..N {
        npx, npy, nvx, nvy = px[it] + vx[it], py[it] + vy[it], vx[it], vy[it]
        nvx = -nvx if npx < 0.0 || npx > 600.0
        nvy = -nvy if npy < 0.0 || npy > 600.0
        px[it], py[it], vx[it], vy[it] = npx, npy, nvx, nvy
    }
}

+rebuild_grid(@px, @py, @heads, @next) {
    for 0..CELLS { heads[it] = -1 }
    for 0..N {
        cx, cy = clamp_cell(to_int(px[it] / CELL)), clamp_cell(to_int(py[it] / CELL))
        cell = cy * GRID + cx
        next[it], heads[cell] = heads[cell], it
    }
}

+collide(@px, @py, @hp, heads, next) {
    for 0..N {
        i, cx, cy, hits = it, clamp_cell(to_int(px[it] / CELL)), clamp_cell(to_int(py[it] / CELL)), 0
        for 0..9 {
            ox, oy = it % 3 - 1, it / 3 - 1
            nx, ny = cx + ox, cy + oy
            if nx < 0 || ny < 0 || nx >= GRID || ny >= GRID { continue }
            id = heads[ny * GRID + nx]
            while id >= 0 {
                if id != i {
                    dx, dy = px[i] - px[id], py[i] - py[id]
                    hits += 1 if dx * dx + dy * dy < 4.0
                }
                id = next[id]
            }
        }
        hp[i] -= 1 if hits > 3
    }
}

+main() {
    rng = [15482888]
    px, py, vx, vy, hp = [0.0; N], [0.0; N], [0.0; N], [0.0; N], [0; N]
    for 0..N {
        px[it], py[it] = next_f(@rng) * 600.0, next_f(@rng) * 600.0
        vx[it], vy[it] = (next_f(@rng) - 0.5) * 2.0, (next_f(@rng) - 0.5) * 2.0
        hp[it] = 10 + lcg(@rng) % 20
    }
    heads, next = [-1; CELLS], [-1; N]
    for 0..TICKS {
        integrate(@px, @py, @vx, @vy)
        rebuild_grid(@px, @py, @heads, @next)
        collide(@px, @py, @hp, heads, next)
    }
    cs = (0..N).map(
    hp[it] + to_int(floor(px[it] * 10.0)) + to_int(floor(py[it] * 10.0))
).sum()
    print("CHECKSUM={cs}")
}
```

### 30.3 C23 lowering notes

- **`@` arrays lower to `restrict`.** The comment in the source says it: `@` SoA arrays lower to `T * restrict` with no per-iteration bounds shims. In the emitted C, `integrate` receives `double *restrict px, *restrict py, *restrict vx, *restrict vy`, each aligned to 64 bytes. With no aliasing GCC vectorizes the 2 000-element update with packed adds, compares and blends (the wall-reflection `-nvx if …` becomes a masked negate), eight doubles per iteration on AVX-512 or four on AVX2.
- **Read-only grid in `collide`.** `heads` and `next` are passed *without* `@`, so they are read-only borrows; the emitter marks them `const` and `restrict`, which lets GCC keep the list-walk loads independent of the `hp[i]` store. Only `hp` is written.
- **Branch-free scalar updates.** `hits += 1 if …` and `hp[i] -= 1 if hits > 3` lower to conditional-add (`setcc`/`adc` or `cmov`), not branches; `clamp_cell` lowers to `min`/`max` pairs.
- **Float→int cell index.** `to_int(px / CELL)` is a truncating `cvttsd2si`; `CELL = 10` is a constant so the divide becomes a multiply by `0.1`-reciprocal under `-ffast-math`, identical to what the C reference's `(int)(px[i] * 0.1)` produces.
- **Pure helpers.** `clamp_cell`, `lcg` and `next_f` have no memory effects beyond their arguments; they are emitted `static inline` and are the candidates for C23 `[[unsequenced]]` so repeated calls with the same argument can be merged.
- **Scoped-stack allocation.** All component arrays and the grid are carved from the frame's scoped data stack (`sup_stack_alloc(…, 64)`), so there is no per-array `malloc`, and the whole simulation state is released by a single pointer reset.
- **Why it matches GCC -O3.** Same SoA, same `restrict`, same intrusive-list grid as the reference C; the difference is that the Super source states the no-alias contract with one sigil rather than three qualifiers and an alignment attribute per array.

### 30.4 Checksum and verification

`CHECKSUM = Σ (hp[i] + floor(px[i]·10) + floor(py[i]·10))` over all 2 000 entities. Hit points verify the collision/grid logic (a wrong neighbour search changes the count of hits), and the scaled positions verify the integrator and wall reflection to one decimal. Entities are inserted into grid lists in index order with head insertion, so list traversal order (which affects nothing but floating-point-free integer counts here) is identical across peers.

---

## 31. 09_matmul_tiled

### 31.1 Problem statement and data layout

Multiply two **256 × 256** `f64` matrices, `C = A · B`, where `A[i] = ((i·17) mod 100) / 100` and `B[i] = ((i·31) mod 100) / 100`, then report the scaled Frobenius norm of `C`. That is 2·256³ ≈ 33.6 million floating-point operations on 512 KiB per matrix — larger than L2 on many cores, so loop order and tiling decide the result.

**Layout: three flat row-major `[f64; 65536]` buffers** (not nested arrays of arrays). Element `(i, j)` is `i*n + j`, so the inner `j` loop in the source's **i-k-j** order is a unit-stride saxpy: `C[i,:] += A[i,k] * B[k,:]`. Row-major plus i-k-j means both `B` and `C` are read and written sequentially, and `A[i,k]` is a loop-invariant scalar broadcast. The i-j-k order a naïve port would use walks `B` with stride 256 and thrashes the cache.

### 31.2 Complete source — `benchmarks/09_matmul_tiled/algo.sr`

```super
// 09_matmul_tiled — 256×256 f64 multiply
use std.math.core

+N = 256
+NN = 65536

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

+main() {
    a = [0.0; 65536]
    b = [0.0; 65536]
    c = [0.0; 65536]
    for 0..NN {
        a[it] = ((it * 17) % 100) / 100.0
        b[it] = ((it * 31) % 100) / 100.0
    }
    matmul(@a, @b, @c, N)
    fro = 0.0
    #simd
    for 0..NN { fro += c[it] * c[it] }
    cs = to_int(floor(sqrt(fro) * 1000.0))
    print("CHECKSUM={cs}")
}
```

### 31.3 C23 lowering notes

- **i-k-j and `restrict`.** The source is already written in i-k-j order. The `@a`, `@b`, `@c` parameters lower to `double *restrict` pointers; inside the `k` loop the emitter also forms `double *restrict crow = C + i*N` and `brow = B + k*N`, which is the form GCC needs to prove `crow[j] += aik * brow[j]` has no loop-carried dependence and to vectorize it with FMA.
- **Tiling.** The AOT tiling peephole reshapes the three loops into **32 × 32 × 32 tiles** (`ii, jj, kk` outer, `i, k, j` inner). Three 32 × 32 `f64` tiles are 24 KiB — the working set fits in a 32 KiB L1D — so each tile of `B` is reused 32 times from L1 instead of being re-streamed from L2.
- **Aligned static panels.** The three matrices are `static double A[NN] __attribute__((aligned(64)))` (and likewise `B`, `C`) when the whole program is visible, or `__builtin_assume_aligned(ptr, 64)` views when received as arguments, enabling aligned vector loads/stores without peel loops.
- **`#simd` reduction.** The `#simd` annotation on the Frobenius loop becomes a vectorizable reduction (`-ffast-math` permits reassociation, so GCC uses several vector accumulators and a horizontal add at the end).
- **`sqrt` and `floor` intrinsics.** The final `floor(sqrt(fro) * 1000.0)` is a single `vsqrtsd` + `roundsd` (SSE4.1) with `-fno-math-errno`.
- **Why it matches GCC -O3.** The generated inner kernel is the textbook tiled ikj saxpy that hand-tuned C uses; both compile to the same `vfmadd231pd` loop. Matching it with a three-line triple loop and no pragmas is the point.

### 31.4 Checksum and verification

`CHECKSUM = floor(sqrt(Σ c[i]²) · 1000)`, an integer, so small last-bit differences from FMA contraction or reassociation vanish after scaling while any real indexing or tiling error (a skipped tile, a wrong stride) changes it by a large amount. Tiling changes the order of summation within each `C` element only across `kk` blocks, which is the same order the reference C uses, so results agree to within the scaled-integer rounding.

---

## 32. 10_sha256_merkle_tree

### 32.1 Problem statement and data layout

Build a **Merkle tree over 1 024 leaves** with SHA-256. Each leaf hash is the digest of a 64-byte message derived from the leaf index; each parent hash is the digest of the 64-byte concatenation of its two children (an odd node is paired with itself). The tree is reduced level by level — 1 024 → 512 → … → 1 — and the checksum folds the 32-byte root. That is 1 024 + 1 023 = **2 047 SHA-256 compressions** of exactly one block each.

**Layout: dense 64-byte message buffers and 32-byte digest arrays.** Every message is a `[i64; 64]` of byte values with one block per hash, so there is no padding logic in the hot path (a 64-byte message is padded to two blocks by SHA-256 rules; the stdlib routine handles it). A tree level is an array of digests (array of 32-element arrays), stored as a flat SoA of bytes in the lowered form: parent `k` reads digests `2k` and `2k+1` as two adjacent 32-byte runs, which is a single 64-byte contiguous read. There is no per-node pointer chasing.

### 32.2 Complete source — `benchmarks/10_sha256_merkle_tree/algo.sr`

```super
// 10_sha256_merkle — 1024 leaves; dense msg buffers + std.sec.sha2
use std.sec.sha2
use std.core.collections

+LEAVES = 1024

#inline
+hash_leaf(i: i64) : _ {
    msg = [0; 64]
    for 0..64 { msg[it] = (i * 131 + it * 17) % 256 }
    sha2_256(msg)
}

#inline
+hash_pair(a, b) : _ {
    cat = [0; 64]
    for 0..32 {
        cat[it], cat[it + 32] = a[it], b[it]
    }
    sha2_256(cat)
}

+main() {
    level = []
    for 0..LEAVES { level = arr_push(level, hash_leaf(it)) }
    n = LEAVES
    for 0..20 {
        if n > 1 {
            next = []
            m = (n + 1) / 2
            for 0..m {
                a = level[it * 2]
                b = if it * 2 + 1 < n { level[it * 2 + 1] } else { a }
                next = arr_push(next, hash_pair(a, b))
            }
            level, n = next, m
        }
    }
    cs, d = 0, level[0]
    for 0..32 { cs = cs * 131 + (d[it] % 256) }
    print("CHECKSUM={cs}")
}
```

### 32.3 C23 lowering notes

- **`std.sec.sha2` is the kernel.** `sha2_256` lowers to the `super_sha256_raw` routine: a fully unrolled 64-round compression with the message schedule held in a `uint32_t w[64]` on the stack and the eight working variables in registers. `rotr`, `shr`, `ch`, `maj` and the xor-3 combinations are individual `static inline` helpers (`sha2_rotr`, `sha2_ch`, `sha2_maj`, `sha2_xor3`, …) that the emitter maps to bit operations directly.
- **Rotates.** `rotr(x, n)` is written as `(x >> n) | (x << (32 - n))`, an idiom GCC recognises and emits as a single `ror` instruction. `<stdbit.h>` is part of the emitted prelude for bit-level kernels like this one.
- **Leaf and pair fusion.** `hash_leaf` and `hash_pair` are `#inline`; the emitter fuses the byte-synthesis loop (`(i*131 + it*17) % 256`) and the 32 + 32 concatenation directly into a stack `uint8_t msg[64]` that is passed to the compression routine, avoiding a heap array per node.
- **`restrict` buffers.** The message and digest buffers are distinct stack objects passed as `restrict` pointers, so the stores of the message schedule do not reload the state.
- **Level reduction.** Each level is built with `arr_push` into a fresh array; the loop `for 0..20` with the `n > 1` guard is a fixed upper bound (log₂ 1024 = 10 levels suffice), so the extra iterations do nothing.
- **Why it matches GCC -O3.** The reference C calls the same unrolled SHA-256 round function; the Super program differs only in who writes the message buffers. Because Super's allocation is stack-based, the 2 047 hashes do no `malloc`/`free`.

### 32.4 Checksum and verification

`CHECKSUM = fold(root)` where the fold is `cs = cs·131 + root[i]` over the 32 digest bytes. SHA-256 is a well-specified function, so every digest (and therefore the root) can be verified against any reference implementation — `std.sec.sha2` is itself tested against the NIST vectors (`abc`, the empty string and the two-block message) — and any bit error in the compression function, the byte order of the concatenation, or the odd-node duplication rule changes the root completely.
