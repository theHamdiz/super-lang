# Safety claims (0.1.0)

Independent review: **none**. Use the **Security claim review** GitHub issue template to challenge or extend this list.

## Claimed (with evidence in-tree)

| Claim | Where tested / documented | Not verified |
|---|---|---|
| Scoped stack references do not escape non-`@` borrows (E0402) | `crates/super-frontend` sema lifetime suites | Formal proof of entire language |
| `@` root lifetimes do not escape (E0401) | Same frontend suites | All dynamic patterns |
| Installer aborts on SHA256 mismatch | `install.sh` / `install.ps1` tests, `install-security` doc | Signing keys beyond GitHub attestations |
| Stage-1 benchmark checksums frozen | `benchmarks/REPRODUCE.md`, `scripts/check_numbers.py` | Statistical significance across hardware |
| HTTP route arity checked before serve | `examples/00_hello_api`, sema tests | Every host embedding |
| AOT hello-server allocation probe (**0 malloc / 0 free** per GET; conn arena, not zero-alloc) | `scripts/alloc_probe.sh`, `crates/super-cli/tests/alloc_probe.rs` (Linux); CI `memory-gates` | Zero-alloc (not claimed); VM RSS soak parity with AOT |
| VM request Stay bulk reclaim at `sys_req_leave` (Make* + Auto births) | `crates/super-runtime/src/vm_req.rs`, `crates/super-cli/tests/vm_req_reclaim_soak.rs` | Full parity with AOT mmap + all host allocs |
| Flat-RSS AOT soak (±1% spread, near-0 slope) | `scripts/soak_http.sh`, `benchmarks-memory/`, `docs/ROUND8_REPORT.md`; CI job `memory-gates` / `http-soak-5m` | Claim “flat RSS” only after that CI job is green on the tip |
| Law mutation ≥70% on priority modules | `scripts/mutate.py`, `docs/law-mutation-report.md`, `docs/law-mutants.json` | Every stdlib module |
| gnu + musl release archives byte-identical across two trees | `scripts/repro_check.sh`, `docs/repro-*.txt`, `docs/REPRODUCE_BUILDS.md` | **macOS / FreeBSD / Windows are not reproducible** |
| **Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.** | `crates/super-runtime/src/aot_host/net_host_c.rs` (emitted C), `-lws2_32`/`-lwinpthread`, `examples/aot_tcp_echo/main.sr`, CI `windows-tier1` | TLS / QUIC / epoll on Windows AOT (unsupported) |
| JIT SIMD baseline SSE2 on x86-64 | SPEC §A6, README | AVX2/AVX-512 paths |

## Explicit non-claims

- Law/proof discharge is **not** a theorem prover; many `.law.sr` files remain sidecar models (`docs/LAWS_STATUS.md`).
- Self-rolled TLS remains experimental (`docs/reports/C6-tls-verification.md`).
- Memory axis leadership does **not** imply zero allocation on all server paths.
- Benchmark composite **89.2** is one machine snapshot — not a universal speed ranking.
- VM `sys_req_*` reclaim is **PARTIAL** (Make* Stay tracking; not AOT-complete); production servers prefer AOT.
- Epoll / io_uring / TLS / QUIC on Windows AOT are **unsupported**.

## Known limitations (Round 9)

- Sidecar laws remain **9/40**; real-body **31/40** (~29 of 127 impl modules). See `docs/LAWS_STATUS.md`.
- macOS / FreeBSD / Windows release archives are **not reproducible** (`docs/REPRODUCE_BUILDS.md`).
- Windows AOT: TCP via Winsock2/`WSAPoll` only; **TLS / QUIC / epoll / IOCP evented I/O unsupported**.
- Escape analysis is module-local with route-lambda + in-module callee BFS; cross-module / UFCS / unknown host calls stay Conservative (`docs/ESCAPE_ANALYSIS_COUNTS.md`).
- AOT OS wrappers are still C emitted from `aot_host/net_host_c.rs` (no standalone `net_host.c`); HTTP serve logic is Super.
- Release attestations: `SHA256SUMS` **and** each archive (workflow `release-bundle.yml`); verify after the ending re-cut.

## Invite

If a public doc overstates safety or platform support, open **Security claim review** or a normal bug report with file paths and expected wording.
