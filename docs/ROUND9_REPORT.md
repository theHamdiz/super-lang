# Super Round 9 Report

Report tip: HEAD

**Orphan tip (this commit):** run `git rev-parse HEAD` — machine tip line is the `HEAD` sentinel (a commit cannot embed its own full SHA). Version lock **0.1.0**. Frozen benches (verified this pass, unchanged):

```text
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

| Item | Status | Evidence |
|---|---|---|
| 1. Rust/C leftovers → Super | PARTIAL | `net_host.c` **deleted**; AOT host is `aot_host/net_host_c.rs` (36553 B Rust-owned C emission). Harnesses → `scripts/alloc_probe.sh` / `http_rss_soak.sh` + thin Rust. HTTP serve remains Super. Irreducible OS/VM kernels listed in `docs/RUST_LEFTOVERS.md` (C23 AOT still needs emitted syscall wrappers). |
| 2. Escape analysis on real service | DONE (limits documented) | `examples/31_escape_service`: **sites=28 stay=24 promote=4 conservative=0**. In-module callee BFS. Diff PASS; wrong-Stay ASan PASS. Cross-module/UFCS/unknown_call still Conservative (honesty fixture `unknown_call:opaque` → 1). |
| 3. Memory claims | DONE | Probe = malloc counts; arena = bump bytes. Claim: **zero malloc per request on the request path**. Never zero-alloc. Measured mid-req `arena_bytes=78568`; conn leave `reclaimed_bytes=544`. |
| 4. Gate sensitivity ~1 KiB/10k | DONE | `slow-leak-keepalive` + `slow-leak-churn` EXPECTED_FAIL; min detectable ~513 B/10k @ 300 s (`docs/SOAK_GATE.md`). |
| 5. VM reclaim | DONE (PARTIAL vs AOT) | `vm_req.rs` + soak test; host/sys still not full mmap parity — production prefers AOT. |
| 6. Per-archive attestations | DONE (workflow; verify on re-cut) | `release-bundle.yml` attests `SHA256SUMS` **and** each of 7 archives. macOS/Windows/FreeBSD **not** reproducible. |
| 7. Laws + mutation | DONE | Real-body **31/40**, modules **29/127**. Mutation **92.9%** overall; cron **87.5%**, csv **100%**, html/core **87.5%**, net/http **100%**. |
| 8. Windows evented I/O | DONE (docs) | TCP/`WSAPoll` only. TLS/QUIC/epoll/IOCP **unsupported** (`docs/platform.md`, README, SAFETY_CLAIMS). |
| 9. Docs truth pass | DONE | This report + LAWS_STATUS + MEMORY_MODEL + README/SAFETY; tip-check uses `HEAD` sentinel. |
| Ending orphan / re-cut | (after green tip) | Bundle → wipe releases/tags/Pages → orphan `Super 0.1.0` → `v0.1.0` workflow → CI + attest verify. |

## Item 1 — before / after bytes (`wc -c`)

| Path | Before | After |
|------|-------:|------:|
| `crates/super-runtime/src/net_host.c` | 35908 | **0 (deleted)** |
| `crates/super-runtime/src/aot_host/net_host_c.rs` | — | 36553 |
| `crates/super-host/src/host_sys.rs` | 111585 | 112691 |
| `crates/super-host/src/host_net.rs` | 14361 | 14641 |
| `crates/super-cli/tests/alloc_probe.rs` | 6376 | 663 |
| `crates/super-cli/tests/http_rss_soak.rs` | 6345 | 1275 |
| `scripts/alloc_probe.sh` | — | 3335 |
| `scripts/http_rss_soak.sh` | — | 4320 |

## Item 2 — escape-service

```text
SUPER_ESCAPE_VERBOSE=1 … build --release examples/31_escape_service/src/main.sr
super: escape-analysis sites=28 stay=24 promote=4 conservative=0
```

```text
ESCAPE_FIXTURE=examples/31_escape_service/src/main.sr scripts/escape_diff.sh
→ PASS escape differential: identical /alloc bodies

PORT=3183 scripts/escape_wrong_stay.sh
→ PASS escape wrong-Stay ASan
```

**unknown_call Conservatives:** honesty fixture forces **1** (`unknown_call:opaque`); escape-service build **0**.

## Item 3 — memory wording

| Probe | Result |
|---|---|
| `alloc_probe` | `0.00` malloc / `0.00` free per fresh GET |
| Req mid-handler `arena_bytes` | `78568` (includes stats JSON tree) |
| Conn leave | `reclaimed_bytes=544` |

Public claim: **zero malloc per request on the request path**. Not zero-alloc.

## Item 4 — planted slow leaks

| Mode | Plant | Gate |
|---|---|---|
| keep-alive | 1024 B / 10k req | FAIL (spread 1.3066%, slope 1.013781) |
| churn | 1024 B / 10k conn | FAIL (spread 2.3402%, slope 1.000988) |

Min detectable (300 s keep-alive): **512 B/10k PASS**, **513 B/10k FAIL**.

## Item 7 — laws

```text
real-body=31 sidecar=9 / 40
impl modules with real-body ≈29 / 127
mutation score=92.9% killed=145 survived=11
```

## Not claimed

- Zero-alloc / zero arena bumps.
- Full Super rewrite of every `sys_*` kernel (see `RUST_LEFTOVERS.md`).
- Windows TLS/QUIC/epoll/IOCP.
- Byte-reproducible macOS/Windows/FreeBSD archives.
- Whole-program interprocedural escape across modules.
