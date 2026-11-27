# LAW-BENCH-001 — Benchmark purity (structural)

**Id:** `LAW-BENCH-001`  
**Verify name:** `law_bench_001_no_host`  
**Code on cheat:** `E0504`  
**Discharge tier:** `structural` (verify engine — not a Super `#assert` body)

## Why this is not a `#law { #assert(true) }` file

Real Super laws constrain *programs*. Examples:

```super
#law("transfer preserves total balance across all valid amounts")
transfer_conserves_total(src: Account, dst: Account, amount: i64) {
    ensure amount >= 0 && src.balance >= amount else { return }
    s = src
    d = dst
    ok = transfer(@s, @d, amount)
    #assert(ok && (s.balance + d.balance == src.balance + dst.balance))
}
```

```super
#law("adjacent limits fuse into the smaller limit")
limit_fusion(n: i64, a: i64, b: i64) {
    ensure n >= 0 && a >= 0 && b >= 0 else { return }
    smaller = a
    if b < a { smaller = b }
    #assert(pl_limit(pl_limit(n, a), b) == pl_limit(n, smaller))
}
```

Those predicates are discharged by SMT / comptime / fuzz over Super semantics.
They **cannot** scan `benchmarks/**/*.sr`. A `#assert(true)` stub would be theater.

## Invariant

Every gated Super source under `benchmarks/` (all `*.sr` except
`benchmarks/tests/**` fixtures) must not:

- `use host` / braced `use { … host … }`
- call `host.` / host FFI

Benchmarks express algorithms in Super stdlib only. Stdlib may still use hosts
internally.

## Proof object (`prove_law_bench_001`)

Implemented in `crates/super-frontend/src/bench_purity.rs`, invoked from
`verify_module` whenever `VerifyOptions.bench_root` is set (`super verify`
auto-discovers `benchmarks/`):

1. **Source scan** — walk gated `.sr` files; first host import/call → **E0504**
   with counterexample bindings `{path, line, col, violation}`.
2. **Gate B evidence** — `benchmarks/runner.py` must still contain
   `LAW_BENCH_001`, `check_algo_bans_host`, and
   `host_err = check_algo_bans_host` inside `verify_super`. If an agent deletes
   the harness check, the proof fails even when sources are currently clean.

## Complementary gates (not substitutes)

| Gate | Where | Role |
|---|---|---|
| E0504 per-file | `super check` / `super verify` on a path under `benchmarks/` | Fail fast on the edited file |
| Gate B | `runner.py::verify_super` | Reject host *before* AOT build / timing |
| Structural proof | `prove_law_bench_001` | Suite-wide law + harness-evidence check |

## Tests

- Rust: `law_bench_001_passes_clean_tree`, `law_bench_001_fails_on_injected_host_algo`,
  `law_bench_001_fails_when_runner_gate_missing`, `law_bench_001_passes_on_repo_benchmarks_if_present`
- Python: `test_bench_purity.py` (`HostBanCheckerTests`, `VerifySuperGateTests`,
  `SuiteAlgoSourcesTests`)
