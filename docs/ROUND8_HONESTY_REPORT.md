# Round 8 honesty pass — soak gate + Items 3/4

Report tip: HEAD

Version **0.1.0**. Frozen benches (verified this pass, unchanged):

```text
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

| Item | Status | Evidence |
|---|---|---|
| A. Soak honesty | DONE | Real `App.serve_with` mix + alloc_probe; negative CI plants; `docs/SOAK_GATE.md`; 5m soak no trim/freelist |
| B. Escape → codegen (was PARTIAL) | DONE (VM reclaim PARTIAL) | Dataflow Stay/Promote/Conservative; counts + Conservative reasons; differential + wrong-Stay classifier test; VM reclaim still documented limit |
| C. Laws (was PARTIAL) | DONE with module PARTIALs | **27** real-body / **13** sidecar; **25** of **127** impl modules; mutation **77.8%** overall; PRIORITY pure ≥80%; modules below 80% listed (not dropped) |
| Ending | PENDING until CI green on orphan tip | |

## A — Soak gate honesty

### A1 Real App.serve path + probe

5-minute soak (`scripts/soak_http.sh --minutes 5 --port 3220 --out benchmarks-memory/ci --probe`):

```text
audit: emitted C has no malloc_trim / freelist (soak.c)
request_mix=GET:542546,POST1024:271272,POST256:271273,POST4096:271272,POST64:271273,TICK:542545
requests_total=2170181
post_warmup_samples=48 rss_min=3372 rss_max=3372 spread_pct=0.0000
slope_kib_per_req=0.00000000 slope_kib_per_10k=0.000000
PASS
alloc_probe_per_request: 3.00 alloc / 0.00 free (GET /ping fresh conn)
```

CSV: `benchmarks-memory/ci/soak_aot_20261127T161258Z.csv`. Spread and slope are **exactly 0.0000 / 0.000000** as measured (not rounded).

### A2 Negative gates (must FAIL; plants never in product)

```text
# durable leak (touched malloc(4096) in throwaway keep-c copy of leave)
negative_durable_leak … delta_kib=1873404 spread_pct=33.3383 slope_kib_per_10k=40156.147870
EXPECTED_FAIL: soak gate caught durable leak
PASS negative durable-leak

# arena free under ASan
EXPECTED_FAIL: sanitizer/arena-free plant detected (rc=134)
PASS negative arena-free
```

Wired permanently: `scripts/soak_negative.sh` + `.github/workflows/memory-gates.yml` job `negative-gates`.

### A3 Thresholds

Documented in `docs/SOAK_GATE.md` and `scripts/soak_http.sh`: warm-up **60 s**, sample interval **≥ 5 s**, spread **≤ 1%**, slope **< 1 KiB / 10k**. A ~1 KiB / 10k durable leak sits on the fail boundary; a 4 KiB/req touched leak measured ~40 156 KiB / 10k.

### A4 No malloc_trim / no freelist

Emitted-C audit on the soak binary + unit test `emit_c_wires_escape_analysis_and_omits_malloc_trim`. Re-run above is the no-trim / no-freelist measurement.

## B — Escape analysis

- Replaced blanket `has_boundary_promote = in_req` with intraprocedural dataflow in `opt.rs` (stores, returns, closures, globals/channels, unknown calls → Conservative with reason).
- `SUPER_ESCAPE_VERBOSE=1` prints per-class counts and every Conservative reason.
- Measured fixture: `sites=2 stay=1 promote=0 conservative=1` with `conservative site 2: unknown_call:opaque`.
- Classifier tests: summary counts; wrong Stay on durable store without boundary → Promote.
- Memory suite: **161** assertions PASS (`tests/memory/`).
- VM reclaim: still **documented limit** (`docs/MEMORY_MODEL.md`).

## C — Laws

```text
Real-body: 27 / 40 law files. Sidecar: 13
Stdlib: 179 .sr; 127 impl modules; 25 modules with real-body laws
mutation score=77.8% killed=105 survived=30 skipped=14  (seed 0xa1c0b26 --full)
```

PRIORITY pure modules ≥80% (or skipped with no sites): str/bytes/math/collections/json/encoding/datetime. Modules below 80% (host-heavy under verify, mutants kept): `datetime/cron`, `fmt/csv`, `html/core`, `net/http` — see `docs/law-mutation-report.md`. Remaining sidecars listed with reasons in `docs/LAWS_STATUS.md`. Vacuous lint OK.

## Not claimed

- Zero-alloc (probe is 3.00 / 0.00).
- Flat RSS on CI tip until `memory-gates` is green on the orphan tip.
- VM request reclaim.
- Mutation ≥80% on every non-PRIORITY module with host-limited laws.
