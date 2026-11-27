# Stage-1 absolute Speed (Protocol 2.0.0)

Equal algorithms; AOT / stdlib only. No Body-alias host kernels. Remeasured
2026-11-26T11:37:27+0200 (full suite, warm=2 / timed=10 / median).

## Absolute walls (selected algos)

Primary wall = median seconds → ms below. Per-algo chip = **fastest wall**
(same as `is-best`); suite composite is separate.

| Algo | Fastest wall | Super AOT | Super JIT | Notes |
|------|-------------:|----------:|----------:|-------|
| 02 Mandelbrot | Rust **10.4 ms** | 16.7 ms | — | Super AOT not wall #1 this run |
| 03 A* | Super AOT **1.10 ms** | **1.10 ms** | — | Hard order vs Zig/C |
| 14 Router | Zig **16.5 ms** | 17.1 ms | 18.9 ms | Within noise of Super AOT |
| 15 Physics2D | Julia **58.5 ms** | 100.4 ms | 98.3 ms | Zig 62.7 / Rust 70.4 / C 75.4; JIT not wall winner |
| 20 Lakehouse | Zig **3.8 ms** | 15.0 ms | 17.1 ms | C 4.4 ms |

Under heavy machine load, Protocol medians inflate; this table is one clean
interleaved suite on `hq` (32 CPUs).

## LoC (category)

| | Super | Bun |
|--|------:|----:|
| Suite code LoC | **2198** | 2200 |
| LoC score | **100** | ~99.9 |

## Composite (this run)

| | Score | Speed | Memory | Reliability | Cycle | LoC |
|--|------:|------:|-------:|------------:|------:|----:|
| **Super AOT** | **89.23** | 97.87 | **100** | **100** | 0.78 | **100** |
| Zig | 79.97 | **100** | 73.10 | 100 | 0.05 | 66.87 |
| Super JIT | 65.65 | 56.36 | 32.45 | 100 | **100** | 100 |

Overall: **Super AOT**. Category Speed → **Zig** (geo-median wall). Cycle →
**Super JIT**. Memory / LoC → Super AOT. Reliability → All (100% pass).

Honesty note: earlier site chips for algo 15 showed Super JIT because a
per-algo mini-composite (wall+RSS+cycle) rewarded tiny JIT cycle cost. Chips
now match fastest primary wall; suite composite still multi-metric.
