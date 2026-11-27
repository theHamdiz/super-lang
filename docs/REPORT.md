# Super Multi-Language Benchmark Report

Generated: 2026-11-26T11:37:27+0200
Host: hq · 32 CPUs
Julia: julia version 1.13.4

## Timing protocol `2.0.0`

- Warm-ups: **2** (timed but excluded from aggregation)
- Timed runs: **10** interleaved round-robin across languages
- Estimator: **median** (primary wall); mean retained in `stats.*`
- Spread: stdev, min/max, and 1.96σ CI on the mean
- Overlapping CIs ⇒ ranking **within noise** (no hard order)

## Composite Language Score (0–100)

Weights: Speed 40% · Memory 25% · Reliability 15% · Cycle 10% · LoC 10%.

| Language | Score | Speed | Memory | Reliability | Cycle | LoC | Geo wall | Geo RSS | Geo cycle | Pass | Code LoC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `super_aot` | **89.23** | 97.87 | 100.0 | 100.0 | 0.78 | 100.0 | 0.0046s | 1776 | 1.0387s | 100% | 2198 |
| `zig` | **79.97** | 100.0 | 73.1 | 100.0 | 0.05 | 66.87 | 0.0045s | 2430 | 15.2433s | 100% | 3287 |
| `rust` | **67.24** | 81.07 | 52.75 | 100.0 | 0.33 | 65.87 | 0.0056s | 3368 | 2.4449s | 100% | 3337 |
| `super_jit` | **65.65** | 56.36 | 32.45 | 100.0 | 100.0 | 100.0 | 0.0080s | 5475 | 0.0081s | 100% | 2198 |
| `c` | **49.73** | 60.63 | 37.43 | 100.0 | 3.75 | 7.39 | 0.0075s | 4747 | 0.2152s | 100% | 29758 |
| `lua` | **43.46** | 20.67 | 33.69 | 100.0 | 37.83 | 79.87 | 0.0219s | 5274 | 0.0213s | 100% | 2752 |
| `bun` | **33.39** | 12.52 | 4.85 | 100.0 | 21.75 | 99.91 | 0.0362s | 36667 | 0.0371s | 100% | 2200 |
| `python` | **28.36** | 2.14 | 10.94 | 100.0 | 3.72 | 93.97 | 0.2119s | 16244 | 0.2169s | 100% | 2339 |
| `julia` | **28.12** | 14.1 | 0.52 | 100.0 | 1.38 | 72.07 | 0.0321s | 344660 | 0.5831s | 100% | 3050 |
| `kotlin` | **26.93** | 5.48 | 2.95 | 100.0 | 0.19 | 89.82 | 0.0827s | 60149 | 4.3544s | 100% | 2447 |

**Overall winner: `super_aot`** (89.23)

### Category winners
- **speed**: `zig`
- **memory**: `super_aot`
- **reliability**: **All** / tie (every language 100% Pass; `category_winners.reliability` = `all` — no sole winner)
- **cycle**: `super_jit`
- **loc**: `super_aot`
