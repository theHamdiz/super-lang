# LAW-BENCH-002 — No bench-shaped host/C APIs

**Id:** `LAW-BENCH-002`  
**Verify name:** `law_bench_002_no_bench_host_apis`  
**Code on cheat:** `E0505`  
**Discharge tier:** `structural` (verify engine — not a Super `#assert` body)

## Invariant

`crates/super-host/**` and `crates/super-runtime/src/c_emit*` must not expose
APIs shaped after specific Stage-1 benchmarks. Forbidden identifier patterns
include (non-exhaustive):

- `polars13`, `http14`, `raft19`, `hnsw17`, `sparse12`, `physics15`, `core18`,
  `lake20`, `lakehouse20`
- `http_req_scan`, `filter_join_f64_ge_i64_gt_unique`, `group_score_i64_f64`,
  `f64_nn_floor_scale_sum`, `polars13_fuse`
- whole-algo `sys_*_run` / `sup_*_run` / `algo_1N_*` fuses for those benches

Benchmarks prove Super by expressing the algorithm in Super (stdlib `.sr` may
use **general** host primitives: alloc, gather, join, crypto, IO). They must
never win by calling a Rust/C function that *is* the benchmark.

## Proof object (`prove_law_bench_002`)

Implemented in `crates/super-frontend/src/bench_purity.rs`:

1. **Surface scan** — walk host + c_emit Rust sources; first forbidden ident →
   **E0505**.
2. **Gate C evidence** — `benchmarks/runner.py` must contain `LAW_BENCH_002`,
   `check_host_surface_bans_bench`, and
   `surface_err = check_host_surface_bans_bench` (called from `verify_super`).

## Complementary gates

| Gate | Where | Role |
|---|---|---|
| LAW-BENCH-001 / E0504 | algo `.sr` + Gate B | No `host` in benchmarks |
| LAW-BENCH-002 / E0505 | host/C + Gate C | No bench-named host APIs |
