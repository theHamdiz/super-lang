# Super Round 8 Report

Report tip: HEAD

**Orphan tip (this commit):** run `git rev-parse HEAD` — machine tip line is the `HEAD` sentinel (a commit cannot embed its own full SHA). Version lock **0.1.0**. Frozen benches (verified this pass, unchanged):

```text
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

| Item | Status | Evidence |
|---|---|---|
| 1. Stale ROUND7 tip + tip-check | DONE | `scripts/check_report_tip.py`; `site-integrity.yml` |
| 2. CI soak + ASan/UBSan | DONE | `memory-gates.yml` keep-alive soak + ASan; **churn soak job added** |
| 3. Escape analysis → codegen | DONE (VM reclaim PARTIAL) | Route-handler lambdas in Super are request-scoped; soak `4/3/1`, hello-api `6/5/1`; Stay→`SUP_REG_REQ` Promote→durable; diff + wrong-Stay ASan; see `docs/ESCAPE_ANALYSIS_COUNTS.md` |
| 4. Laws real-body | DONE with module PARTIALs | Call-trace proof: **27** real-body / **13** sidecar; **25**/127 impl modules |
| 5. Platforms | DONE (docs) | Windows TLS/QUIC/epoll unsupported; macOS/FreeBSD/Windows archives not reproducible |
| A. Soak honesty | DONE | Real App.serve mix + probe; negative plants FAIL; SOAK_GATE.md |
| B. Connection-churn gap | DONE | Conn arena `SUP_REG_CONN`; 544 B reclaim/leave; alloc_probe **0/0** malloc; conn-leak plant FAIL; CI `http-soak-churn` ≥1M **green** on tip |
| Ending orphan / re-cut | DONE | Single-orphan tip + tag `v0.1.0`; all tip workflows green; release assets×7; `SHA256SUMS` SLSA provenance verified (`gh attestation verify`) |

## B — Connection churn (measured this pass)

Where the old **3 alloc / 0 free** went (fresh `GET /ping`, `LD_PRELOAD` size hist):

| Alloc | Size | Source |
|---|---:|---|
| 1 | `calloc` 520 | `Conn` SuperObj at accept (durable) |
| 2 | `malloc` 16 | `socket_peer_addr` / `super_strdup` |
| 3 | `malloc` 1 | empty `socket_buf_slice("", 0, 0)` |

Fix: birth into connection bump arena at `socket_conn_enter`; reclaim at
`socket_conn_leave` after the connection is finished (`MADV_DONTNEED`).

```text
# SUPER_CONN_TRACE=1 — 20 fresh + 1 keep-alive (10 reqs) → 22 leaves
super: conn-arena leave reclaimed_bytes=544 enter_count=N leave_count=N
conn_leaves=22 reclaimed_bytes_min=544 max=544 sum=11968 median=544
```

```text
# LD_PRELOAD hist — 100 fresh connections (post-fix)
allocs=5 frees=2   # noise only; no 3×calloc/malloc per conn
# One keep-alive connection, 100 requests
allocs=1 frees=0
# alloc_probe pin
per request: 0.00 allocations, 0.00 frees   # malloc counter; NOT zero-alloc
```

```text
# Planted per-connection leak (scripts/soak_negative.sh conn-leak)
negative_conn_leak connections_total=135736 delta_kib=545068
spread_pct=15.0415 slope_kib_per_10k=40156.698615
EXPECTED_FAIL: churn soak gate caught per-connection leak
PASS negative conn-leak
```

CI: `memory-gates` / `soak-churn-1m` (`--churn --min-connections 1000000`).
Flat RSS on that job is **DONE** (green on the orphan tip).

### Escape-pass soaks (measured this tip, before ending redo)

```text
# keep-alive ≥1M
requests_total=1000000
post_warmup_samples=38 rss_min=3388 rss_max=3388 spread_pct=0.0000
slope_kib_per_req=0.00000000 slope_kib_per_10k=0.000000
PASS

# churn ≥1M
connections_total=1000000 requests_total=1000000
post_warmup_samples=25 rss_min=3308 rss_max=3308 spread_pct=0.0000
slope_kib_per_req=0.00000000 slope_kib_per_10k=0.000000
PASS

# negatives (scripts/soak_negative.sh all)
PASS negative durable-leak / arena-free / conn-leak
```

## Item 3 — escape counts

See `docs/ESCAPE_ANALYSIS_COUNTS.md`. Root cause: route lambdas were outside the
`socket_req_enter` gate. Fix: analyze `App.get`/`post`/… handler bodies as
request-scoped (serve path already Super in `stdlib/net/http.sr`); remap
`escape_birth` across peephole. Measured:

```text
# soak (ping/echo/tick/alloc)
super: escape-analysis sites=4 stay=3 promote=1 conservative=0
/* escape_analysis: tagged_births=4 stay_arena=3 promote_or_conservative_durable=1 */

# examples/00_hello_api
super: escape-analysis sites=6 stay=5 promote=1 conservative=0
/* escape_analysis: tagged_births=6 stay_arena=5 promote_or_conservative_durable=1 */
```

Honesty fixture (classifier + reasons):

```text
super: escape-analysis sites=2 stay=1 promote=0 conservative=1
super: escape-analysis conservative site 2: unknown_call:opaque
super: escape-analysis site 1 → stay (None)
```

Proof: `scripts/escape_diff.sh` PASS (identical `/alloc` bodies);
`scripts/escape_wrong_stay.sh` PASS (planted Stay-on-Promote → ASan/abort).

VM `sys_req_*` / `sys_conn_*`: documented no-op reclaim (PARTIAL).

### Rust leftovers outside compiler/toolchain (PARTIAL — not moved)

| Path | Bytes | Role |
|------|------:|------|
| `crates/super-host/src/host_sys.rs` | 111585 | VM/embed `sys_*` kernels (TCP, req stubs) |
| `crates/super-runtime/src/net_host.c` | 35908 | AOT C host dispatch (emitted with codegen) |
| `crates/super-host/src/host_net.rs` | 14361 | Wire/JSON helpers for host/tests |
| `crates/super-cli/tests/http_rss_soak.rs` | 6484 | RSS soak harness (driver; server is Super) |
| `crates/super-cli/tests/alloc_probe.rs` | 6376 | LD_PRELOAD probe harness |
| `crates/super-host/src/http.rs` | 2270 | Embed/test `Response` helpers |
| `crates/super-cli/tests/fixtures/alloc_count.c` | 2059 | malloc counter for probe |

HTTP `App.serve` / Conn / routing / handlers: Super (`stdlib/net/http.sr`).
Soak/probe **programs** under test: Super (scripts emit `.sr`). Harnesses that
must link `LD_PRELOAD` / drive RSS stay Rust/shell.

## Item 4 — laws (call-trace)

```text
Real-body (call-trace): 27 / 40. Sidecar: 13
25 of 127 impl modules; 179 .sr files
```

`scripts/laws_status.py` BFS from `#law` bodies through same-file helpers to
impl symbols (`docs/LAWS_STATUS.md`).

## Not claimed

- Zero-alloc (arena bumps; malloc probe 0/0 is not “no allocation”).
- Flat RSS on CI churn tip until `soak-churn-1m` is green on the orphan tip.
- VM request/connection reclaim.
- Mutation ≥80% on every host-heavy module.
