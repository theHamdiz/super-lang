# Rust / native leftovers (Round 9 Item 1)

Non-compiler artifacts that stay in Rust or C because they are **host OS
binding**, **LD_PRELOAD instrumentation**, **VM wire encoding**, **AOT C
templates**, or **frozen bench snapshots**. Sizes are from `wc -c` at
documentation time (do not hand-edit).

**Moved in Round 9:** standalone `crates/super-runtime/src/net_host.c` deleted;
AOT socket/HTTP dispatch lives in `crates/super-runtime/src/aot_host/net_host_c.rs`
(Rust-owned string emission appended to generated `.c`).

**HTTP `App.serve`:** Super (`stdlib/net/http.sr`). Programs under soak/probe
gates are Super; drivers are shell (+ thin Rust `#[test]` wrappers on Linux).

---

## Before / after (Round 9 Item 1)

| Path | Before (B) | After (B) | Notes |
|------|-----------:|----------:|-------|
| `crates/super-runtime/src/net_host.c` | 35908 | **0 (deleted)** | Replaced by `aot_host/net_host_c.rs` |
| `crates/super-runtime/src/aot_host/net_host_c.rs` | — | 36553 | Rust source of truth for emitted AOT host |
| `crates/super-host/src/host_sys.rs` | 111585 | 112691 | VM `sys_*` kernels (TCP, req, embed) |
| `crates/super-host/src/host_net.rs` | 14361 | 14641 | VM `Value`→JSON toolchain (`sys_json_*`); bench kernel retained (+ module doc) |
| `crates/super-cli/tests/http_rss_soak.rs` | 6345 | 1275 | Thin wrapper → `scripts/http_rss_soak.sh` |
| `crates/super-cli/tests/alloc_probe.rs` | 6376 | 663 | Thin wrapper → `scripts/alloc_probe.sh` |
| `scripts/http_rss_soak.sh` | — | 4320 | CI short RSS soak (AOT `new.sr`) |
| `scripts/alloc_probe.sh` | — | 3335 | Pinned 0/0 malloc probe |
| `scripts/soak_http.sh` | (unchanged) | 11151 | Full flat-RSS gate; Super soak server inline |

---

## Remaining irreducible pieces (inventory)

### AOT host C template (toolchain output, not user `.sr`)

| Path | Bytes | Reason |
|------|------:|--------|
| `crates/super-runtime/src/aot_host/net_host_c.rs` | 36553 | Epoll/Winsock/TCP/HTTP dispatch appended to emitted C23; must stay Rust-audited |

### VM / embed host (not in Super stdlib)

| Path | Bytes | Reason |
|------|------:|--------|
| `crates/super-host/src/host_sys.rs` | 112691 | `sys_*` VM/embed: sockets, signals, stdio, request stubs |
| `crates/super-host/src/host_net.rs` | 14641 | `sys_json_*`: boxed `Value`→JSON + Stage-1 HTTP JSON micro-bench kernel |
| `crates/super-host/src/http.rs` | 2270 | Embed/test `Response` helpers for VM harnesses |
| `crates/super-runtime/src/vm_req.rs` | 10131 | VM request-scope Stay/Promote reclaim (`sys_req_*`) |

### Runtime VM / stacks / JIT (audited `unsafe`, not frontend)

| Path | Bytes | Reason |
|------|------:|--------|
| `crates/super-runtime/src/stacks.rs` | 7716 | `mmap`/`VirtualAlloc` bump arenas |
| `crates/super-runtime/src/vm.rs` | 121592 | Register VM + opcode dispatch |
| `crates/super-runtime/src/jit.rs` | 30467 | Native JIT emitter + executable pages |

### Platform GUI / window providers (optional embed)

| Path | Bytes | Reason |
|------|------:|--------|
| `crates/super-runtime/src/win32_provider.c` | 17786 | Win32 message loop |
| `crates/super-runtime/src/gtk_provider.c` | 36178 | GTK window provider |
| `crates/super-runtime/src/cocoa_provider.c` | 19928 | macOS Cocoa provider |
| `crates/super-runtime/src/gui_dispatch.c` | 4009 | Shared GUI dispatch glue |

### Stdlib AOT check artifact

| Path | Bytes | Reason |
|------|------:|--------|
| `*.check.c` (e.g. `stdlib/net/http.check.c`) | (gitignored) | AOT `--keep-c` / check emit; never committed (`**/*.check.c` in `.gitignore`) |

### CI / memory harness (C + shell; server under test is Super)

| Path | Bytes | Reason |
|------|------:|--------|
| `crates/super-cli/tests/fixtures/alloc_count.c` | 2059 | LD_PRELOAD `malloc`/`free` counter (irreducible instrumentation) |
| `scripts/alloc_probe.sh` | 3335 | Pinned 0/0 alloc gate; builds `benchmarks/server-builder/new.sr` |
| `scripts/http_rss_soak.sh` | 4320 | Short RSS soak for release-bundle CI |
| `scripts/soak_http.sh` | 11151 | Full flat-RSS + optional `--probe` / `--churn`; Super `soak.sr` driver |
| `scripts/soak_negative.sh` | 23950 | Negative plants (must fail gates) |
| `scripts/sanitize_http.sh` | 6240 | ASan+UBSan on App.serve path |
| `crates/super-cli/tests/alloc_probe.rs` | 663 | Linux `cargo test` → `alloc_probe.sh` |
| `crates/super-cli/tests/http_rss_soak.rs` | 1275 | Linux `cargo test` → `http_rss_soak.sh` |
| `crates/super-cli/tests/vm_req_reclaim_soak.rs` | 972 | VM `sys_req_*` reclaim integration |
| `crates/super-cli/tests/fixtures/vm_req_soak.sr` | 886 | Super program for VM reclaim soak |

### C interop examples (documentation, not toolchain)

| Path | Bytes | Reason |
|------|------:|--------|
| `examples/interop_c/main.c` | 4497 | Sample C main calling Super AOT |
| `examples/interop_c/native/mathlib.c` | 242 | Tiny native helper for interop demo |

### Frozen Stage-1 bench `algo.c` trees (not runtime)

Under `benchmarks/*/algo.c` (01–20): composite snapshot kernels for
`docs/REPORT.md` / `results.json`. **Do not edit** for Round 9 host migration.

### Editor Tree-sitter C parser (generated)

| Path | Bytes | Reason |
|------|------:|--------|
| `editors/tree-sitter-super/src/parser.c` | 93137 | Tree-sitter runtime; regen from `grammar.js` |

---

## Explicitly not listed here

- Compiler frontend/middle (`lexer`, `parser`, `sema`, `c_emit`, LSP, MCP): intentional Rust toolchain.
- Super stdlib (`stdlib/**/*.sr`): language surface; HTTP server logic is Super.

---

## Related docs

- `docs/SAFETY_CLAIMS.md` — claims and gates
- `docs/SOAK_GATE.md` — soak thresholds (`soak_http.sh`)
- `docs/MEMORY_MODEL.md` — conn arena vs zero-alloc wording
- `docs/ROUND8_REPORT.md` — pre-Round-9 leftover table (historical byte counts)