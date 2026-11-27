# Flat-RSS soak gate (0.1.0)

Authoritative thresholds live in `scripts/soak_http.sh` (header + evaluator)
and `.github/workflows/memory-gates.yml`. This document explains what the gate
measures and why a ~1 KiB / 10k-request leak cannot hide.

## What the soak drives

The soak binary is an AOT `App.serve_with` server (not a stub):

| Shape | Method / path | Why it is on the real path |
|---|---|---|
| `GET` | `/ping` | Router match + handler + response write |
| `POST` | `/echo` | Varied `Content-Length` bodies (64 / 256 / 1024 / 4096 bytes) |
| `TICK` | `/tick` | Same keep-alive Conn; `conn.served` / App COW + `conn_req_revive` |

Default soak cycles keep-alive on one connection (reconnect on error). The
`--churn` variant opens and closes one TCP connection per request (CSV
`soak_aot_churn_*.csv`) and requires `--min-connections` (CI: ≥ 1 000 000)
plus the same RSS spread/slope gates.

`alloc_probe` (`crates/super-cli/tests/alloc_probe.rs`) counts `malloc`/`free`
via `LD_PRELOAD` on fresh connections. After the connection arena, that counter
is **0.00 alloc / 0.00 free** per `GET /ping` — arena bumps are invisible to
`free()`. That is **not** zero-alloc; see `docs/MEMORY_MODEL.md`.

## Thresholds

| Gate | Value | Where |
|---|---|---|
| Warm-up | first **60 s** discarded | `scripts/soak_http.sh` |
| Sample interval | **≥ 5.0 s** between RSS samples | same |
| Spread | post-warmup `(max−min)/mid × 100 ≤ 1.0%` | same |
| Slope | least-squares RSS(KiB) vs request count; **slope × 10 000 < 1.0** KiB | same |
| Full 30 m run | also requires **≥ 1 000 000** requests | same |

CI runs the same script for **5 minutes** (`memory-gates` / `soak-5m`).

## Why ~1 KiB / 10k cannot hide

The slope gate fails when the fitted growth is **≥ 1 KiB per 10 000 requests**.
A durable per-request leak of size *B* bytes yields approximately
`(B / 1024) × 10 000` KiB per 10k requests in the slope estimate once samples
span the post-warmup window. For *B* = 1024 (1 KiB / request) the expected
slope is ~10 000 KiB / 10k — four orders of magnitude above the gate. Even a
steady **1 KiB every 10k requests** sits on the fail boundary (`slope_kib_per_10k ≥ 1`).

Spread ≤ 1% alone is not enough: a slow linear climb can stay inside a small
percent window on a large baseline. The **slope** term catches that. Negative
CI jobs (`scripts/soak_negative.sh`) plant a throwaway durable `malloc` per
request (never shipped in product sources) and require the soak evaluator to
print `FAIL`.

## Sampling honesty

- Spread and slope are computed only on samples with `t ≥ 60`.
- Exact zeros are possible (and have been measured) when RSS does not move
  across samples; report them as measured (`spread_pct=0.0000`,
  `slope_kib_per_10k=0.000000`) — do not round a non-zero to zero.
- No `malloc_trim` and no per-object freelist: arena death is bulk
  (`MADV_DONTNEED` / cursor rewind) on `sup_req_leave` (see `docs/MEMORY_MODEL.md`).

## Negative gates (must FAIL)

| Script / job | Plant | Expected |
|---|---|---|
| `scripts/soak_negative.sh durable-leak` | patch throwaway `.c`: `malloc(4096)` in `sup_req_leave` | soak prints `FAIL` |
| `scripts/soak_negative.sh slow-leak-keepalive` | `malloc(1024)+memset` every **10 000** requests in `sup_req_leave` | spread and/or slope gate `FAIL` |
| `scripts/soak_negative.sh slow-leak-churn` | same plant every **10 000** `sup_conn_enter` + `Connection: close` | churn spread and/or slope `FAIL` |
| `scripts/soak_negative.sh arena-free` | `free()` a `SUP_REG_REQ` pointer under ASan | sanitizer job fails |
| `scripts/soak_negative.sh conn-leak` | `malloc(4096)` in `sup_conn_enter` + churn (`Connection: close`) | churn gate FAIL |
| CI `memory-gates` / `negative-gates` | all of the above | job succeeds only if the gates fail |
| CI `memory-gates` / `soak-churn-1m` | real churn ≥1M connections | PASS on spread/slope |

Product trees never contain the planted leak.

### Slow-leak negatives (~1 KiB / 10k)

Harness: `scripts/soak_negative.sh slow-leak-keepalive` or `slow-leak-churn`
(also `slow-leak-all`). Tunables: `SLOW_LEAK_BYTES` (default **1024**),
`SLOW_LEAK_PERIOD` (default **10000**), `SLOW_SOAK_SECONDS` (default **300**).
Plants live only in a temp `--keep-c` tree; product sources are untouched.

Run (requires built `target/release/super` and `cc`):

```bash
scripts/soak_negative.sh slow-leak-keepalive   # keep-alive reuse
scripts/soak_negative.sh slow-leak-churn       # one connection per request
```

Measured on this repo (Linux, `SLOW_SOAK_SECONDS=300`, plant **1024 B / 10 000**):

| Mode | Requests / connections | `spread_pct` | `slope_kib_per_10k` | Gate |
|---|---:|---:|---:|---|
| keep-alive | 764 085 | 1.3066 | 1.013781 | **FAIL** (EXPECTED_FAIL) |
| churn | 587 129 | 2.3402 | 1.000988 | **FAIL** (EXPECTED_FAIL) |

Log artifacts land under gitignored `benchmarks-memory/` (local / CI
workspace only; not committed).

### Minimum detectable durable leak (measured, not theoretical)

Same harness and **300 s** window; only `SLOW_LEAK_BYTES` changed (period
10 000). Post-warm-up evaluator unchanged (`spread_pct ≤ 1%`,
`slope_kib_per_10k < 1`).

| Plant (B / 10k reqs, keep-alive) | `spread_pct` | `slope_kib_per_10k` | Soak gate |
|---:|---:|---:|---|
| 512 | 0.8873 | 0.521723 | **PASS** (below gate) |
| 513 | 1.4886 | 0.515814 | **FAIL** |

At this sample duration and ~1.5–1.6 M keep-alive requests, the smallest
**measured** failing plant was **513 B every 10 000 requests** (~0.50 KiB / 10k).
The Round 9 negative uses **1024 B / 10 000** (~1 KiB / 10k) so failure is
comfortably on the boundary (`slope_kib_per_10k ≥ 1` and/or `spread_pct > 1%`).
Longer `SLOW_SOAK_SECONDS` or tighter sampling does not weaken thresholds; it
only increases confidence on sub-threshold plants.
