# Changelog

## v0.1.0 — 2026-11-26

The product, crates, CLI, installers, site, and release tag are versioned together at 0.1.0.

### Round 6 (same product version)

- Request-scoped bump arena on the AOT `App.serve` path (`socket_req_enter` / `leave` / `keep_str`): strings and dense arrays reclaim; durable `SuperObj` calloc remains. `alloc_probe` pin: **42 alloc / 0 free** per `GET /ping` (was 256 / 7). Not zero-allocation. VM host `sys_req_*` API exists but does not reclaim owned values (PARTIAL).
- Windows C23 AOT sockets remain unsupported; wording + `scripts/check_windows_aot_net.py` enforce consistency.
- Law coverage: **40 / 127** modules have `.law.sr` (floor file); coverage/facts/version CI scripts; vacuous-tautology lint. Many new laws are pure sidecar models because comptime cannot execute full stdlib (mutation PARTIAL).
- Adoption: `ROADMAP.md`, `docs/SAFETY_CLAIMS.md`, `docs/REPRODUCE_BENCHMARKS.md`, `docs/VERIFYING_RELEASES.md`, issue/PR templates, supply-chain audit/deny + bounded fuzz workflow. Release attestations on `SHA256SUMS` (per-archive / byte-identical musl PARTIAL).

### Round 5 language revision

Breaking changes (clean break; each old form is a compile error that names its replacement):

- `routes { … }` is now the `#routes { … }` macro. A binding or function named `routes` is an ordinary value.
- `assert(expr)` and `assert_err(expr[, text])` are now the `#assert` and `#assert_err` macros. They still show the captured source expression and operand values on failure.
- `len` is an ordinary prelude function with no parser or sema special case.
- Specification §3.9 states the design rule: every construct is a keyword, a macro, a function, or a method.

Tooling:

- `super fmt [--check] [paths | -]`: a deterministic, idempotent, layout-only formatter. It refuses files that do not lex and checks that the token stream is unchanged. Style guide: `docs/formatting-style.md`.
- `super lint [--fix] [--rules] [--config FILE] [paths]`: rules S001–S008 with stable ids and severities, safe `--fix` for S007/S008, and `[lint]` allow/deny/exception config in `Pod.toml`. See `docs/lint.md`.
- The LSP publishes lint findings as diagnostics (source `super-lint`) and supports whole-document formatting.
- CI runs `super fmt --check` and `super lint` on `stdlib/`, `examples/`, and `pods/`.

Measurement and honesty:

- New counting-allocator test (`crates/super-cli/tests/alloc_probe.rs`, Linux) for the AOT `App.serve` path: 42 allocations and 0 frees per `GET /ping` request. Known issue: the route-registering `main` falls back to the boxed runtime, whose per-request values are never reclaimed, so RSS grows with request count. Request-scoped bump reclamation covers strings/arrays; durable SuperObj calloc remains at net 42 (not zero-allocation).
- The placeholder `benchmarks/REPORT.md` fixture (no generation time, invented scores) was removed. The published measurement is `docs/REPORT.md` and `docs/results.json`, unchanged: SuperAot composite 89.2 and Zig wins Speed. All numbers come from a single machine.
- Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.

Fixes:

- C23 AOT servers built with GCC 15 (Ubuntu 26.04) answered every request with an empty reply. GCC 15 deleted the buffer writes in `sys_bytes_concat` / `sys_bytes_slice` because the same host function read a `volatile sig_atomic_t` signal flag; GCC 14 and GCC 16 compile the same code correctly. The flag is now a lock-free `_Atomic int`.

### Initial release

- Public specification, CLI (`super run`, `test`, `check`, `build`, `verify`, `lsp`, `mcp`, `pod`).
- Standard library: **179** `.sr` files, **127** implementation modules with law/proof sidecars where noted (`docs/facts.json`).
- Stage-1 benchmark data frozen at composite **89.2** (SuperAot); **Zig** wins Speed axis — see `docs/REPORT.md`.
- GitHub Pages site with static noscript score tables and reproducible harness in `benchmarks/`.

Known gaps are called out per module tier in `SPECIFICATION.md` and on doc pages marked **planned**.