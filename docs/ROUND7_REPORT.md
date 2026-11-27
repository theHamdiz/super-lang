# Super Round 7 Report

Report tip: `f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e`

**Overall: DONE (with honest Item 1 PARTIAL)** — Tip `f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e` / `v0.1.0`. Items 2/3/4/5/6 DONE with CI evidence on this tip; Item 1 near-DONE (local soak/sanitizer/escape; dedicated CI soak job still thin — Round 8). §9 orphan + release + attestations complete.

Version lock: **0.1.0**. Tip at Round 7 start (pre-final orphan): `782da11`. Frozen benchmark SHA256 (must match through cleanup):

```text
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

Round 6 PARTIAL list carried forward: (1) reclamation / flat RSS, (2) laws real-body + mutation, (3) CI/release/attestations, (4) musl/repro builds, (5) Windows AOT sockets (try Option A), (6) docs truth.

---

## Item 1 — Net-0 reclamation via escape analysis

| Sub | Status | Evidence |
|---|---|---|
| 1.1 Memory model | DONE | `docs/MEMORY_MODEL.md` — frame / request arena / durable; frame-stack vs durable arrays |
| 1.2 Escape analysis | PARTIAL | `escape_region.rs` + opt site collection; 25 classifications PASS. Runtime: request-arena SuperObj Stay; durable in-place; `conn_req_revive`; `malloc_trim` + frame-stack reset on leave. No freelist. Compiler site→codegen wiring still thin (conservative default OK). |
| 1.3 Tests | PARTIAL | Analysis unit tests PASS. `alloc_probe` **3 alloc / 0 free** PASS. `tests/memory/multi_route_keepalive.sr` **53 assertions PASS**. `tests/memory/diff_promote.sr` byte-identical vs `SUPER_PROMOTE_EVERYTHING`. `scripts/sanitize_http.sh` **10 000 reqs ASan+UBSan PASS** (UAF/invalid-free; process-lifetime LSan off by default). Full 25-program Super corpus + CI job wiring — not done. |
| 1.4 Flat-RSS soak | DONE (local AOT) | 30 m multi-shape: **6 300 637** reqs, post-warmup spread **0.3567%**, slope **-0.000292 KiB/10k**. CSV `benchmarks-memory/soak_aot_20261127T121020Z.csv`, plot `.png`. |
| Multi-route keep-alive | DONE | Root cause: `sys_arr_new` on frame stack; `sup_stack_reset` at leave overwrote `Router.prefix_heads`. Fix: heap/arena for `sys_arr_new`/`sys_bytes_new`. |
| VM reclaim | honest | VM `sys_req_*` no-ops; **not implemented**; AOT only for long-running servers. |

### Multi-route root cause (command evidence)

```text
# Before fix (enter+leave only, no handle):
enter-leave-only: a=200 ping=200 echo=404

# After heap sys_arr_new:
enter-leave-only: a=200 ping=200 echo=200
HTTP keep-alive mixed 45 reqs: all_ok True
super test tests/memory/multi_route_keepalive.sr → 53 passed
```

### Before / after (AOT)

| Metric | Round 6 | Round 7 (current) |
|---|---|---|
| allocs / req (fresh conn) | 42 | **3** |
| frees / req | 0 | **0** |
| Keep-alive RSS slope / 10k | ~14 KiB/req growth | **-0.000292** KiB (30 m) |
| Post-warmup RSS spread | n/a | **0.3567%** (≤1% gate) |
| Keep-alive multi-route | 404 after ~2 | **200 stable** |
| 30 m request count | n/a | **6 300 637** |

### Soak gate output (verbatim)

```text
requests_total=6300637
post_warmup_samples=348 rss_min=3364 rss_max=3376 spread_pct=0.3567
slope_kib_per_req=-0.00000003 slope_kib_per_10k=-0.000292
PASS
```

### Commands run

```bash
cargo test -p super-runtime escape_region --lib
cargo test -p super-cli --release --test alloc_probe -- --nocapture
# per request: 3.00 allocations, 0.00 frees
./target/release/super test tests/memory/multi_route_keepalive.sr
REQUESTS=10000 scripts/sanitize_http.sh
# PASS: ASan+UBSan clean for 10000 requests
scripts/soak_http.sh --minutes 30 --port 3131 --out benchmarks-memory
python3 scripts/plot_soak.py benchmarks-memory/soak_aot_20261127T121020Z.csv
```

### Remaining gap (Item 1)

- Wire sanitizer + soak-smoke into CI; prove on orphan tip (Round 8 item 2).
- Expand runtime classification corpus; stronger compiler↔escape_region coupling (Round 8 item 3).
- `docs/facts.json` soak numbers only via `check_numbers` (Item 6).

**Item 1 local status: PARTIAL→near-DONE** (soak + sanitizer + multi-route proven; dedicated CI soak/sanitizer jobs still open — Round 8).

---

## Item 2 — Real-body laws + mutation

| Sub | Status | Evidence |
|---|---|---|
| Verify evaluator | DONE local | Module globals; string index → 1-char `str` (match AOT); `when` arm match; `return` inside `if` propagates `EarlyReturn`; `law_mode` for `#assert` |
| Real-body laws | DONE local | `json_is_ws`/`json_is_ws_b`, `enc_hex_value`, `is_leap_year`/`days_in_month`/`days_from_civil`, `bytes_last_index`, `m_smoothstep`/`m_smootherstep`, `i_sdiv`/`word_has`/`popcount32`, plus prior reduce/str/bytes/math |
| Vacuous lint | DONE | Bans tautologies; `lint_vacuous_laws.py` OK |
| Mutation | DONE local | Seed `0xa1c0b26`; anchors = callees only; **93.5%** (43 killed / 3 survived / 2 skipped). Report: `docs/law-mutation-report.md` |
| `docs/LAWS_STATUS.md` | DONE local | Call-site heuristic: **12** real-body / **28** sidecar / 40 law files |

Honesty: many modules remain sidecar models (host crypto, etc.). `xs.sum()` still uses verify Iterable builtins. Gate 2 local: PASS (≥70%).

---

## Item 4 — Musl / reproducible builds

| Sub | Status | Evidence |
|---|---|---|
| Deterministic packer | DONE | `scripts/package_release.sh` — remap paths, `CARGO_BUILD_JOBS=1`, sorted tar / Python fallback, `gzip -n`, `SOURCE_DATE_EPOCH` |
| `repro_check.sh` | DONE | Two isolated trees, different `HOME`/`CARGO_HOME` |
| gnu byte-identical | DONE | A=B `86f5608570a5533241436ad898dd1c2892fe6ef7633a2b56413e563bd83e067c` → `docs/repro-x86_64-unknown-linux-gnu.txt` |
| musl byte-identical | DONE | A=B `6dcdeb01febcc285b8ae7e91e34fcdfaf4c929b9e7f1555d644be1d9b79dc773` in pinned `rust:1-alpine@sha256:8d0e82ab…` → `docs/repro-x86_64-unknown-linux-musl.txt` |
| Docs / CI | DONE | `docs/REPRODUCE_BUILDS.md`, `.github/workflows/repro-check.yml`, musl image digest pinned in `release-bundle.yml` |

macOS / Windows / FreeBSD: **not claimed**. Gate 4 local: PASS (gnu + musl).

---

## Item 5 — Windows AOT sockets (Option A)

| Sub | Status | Evidence |
|---|---|---|
| Winsock2 host | DONE local | `net_host.c` `_WIN32` path: `WSAStartup`, TCP/UDP, `WSAPoll`, timeouts; link `-lws2_32` |
| Docs / check | DONE local | Exact phrase `Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.`; `check_windows_aot_net.py` OK |
| Loopback smoke | DONE on Linux | `examples/aot_tcp_echo/main.sr` → `aot_tcp_echo: ok` |
| Windows CI | DONE | Tip `f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e` run [46017952974](https://github.com/theHamdiz/super-lang/actions/runs/46017952974) job `windows-tier1` **success** (Winsock2 TCP loopback + `-lwinpthread`) |

TLS/QUIC / epoll on Windows: unsupported. Gate 5: **PASS**.

---

## Item 6 — Docs / site / claims truth

| Sub | Status | Evidence |
|---|---|---|
| `facts.json` | DONE local | Extended with mutation score, repro targets, soak csv, alloc_probe 3/0, Windows phrase |
| `check_numbers` | DONE local | PASS (soak CSV timestamps excluded from `90.07` scan) |
| `SAFETY_CLAIMS.md` / `ROADMAP.md` | DONE | Round 7 truth; links to soak / mutation / repro / Winsock |
| Known limitations | DONE | In SAFETY_CLAIMS + ROUND7 “Not done” |

---

## Item 3 — CI / release / attestations

**DONE** on tip `f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e` / tag `v0.1.0`.

Measured with `gh run list --commit f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e` and `gh run view` (all `headSha` = tip; all `conclusion` = success):

| Check | Result | URL / evidence |
|---|---|---|
| site-integrity | success | [46017953183](https://github.com/theHamdiz/super-lang/actions/runs/46017953183) |
| supply-chain | success | [46017952969](https://github.com/theHamdiz/super-lang/actions/runs/46017952969) |
| repro-check | success | [46017953007](https://github.com/theHamdiz/super-lang/actions/runs/46017953007) |
| cross-platform (incl. Windows TCP) | success | [46017952974](https://github.com/theHamdiz/super-lang/actions/runs/46017952974) — jobs: windows-tier1, unix-tier1×4, linux-musl-tier1, freebsd-tier2 all success |
| stdlib-gates | success | [46017967719](https://github.com/theHamdiz/super-lang/actions/runs/46017967719) |
| release-bundle (main push) | success | [46017953171](https://github.com/theHamdiz/super-lang/actions/runs/46017953171) |
| release-bundle (tag `v0.1.0`) | success | [46017967633](https://github.com/theHamdiz/super-lang/actions/runs/46017967633) — 7 archives + `SHA256SUMS` |
| Release | published | https://github.com/theHamdiz/super-lang/releases/tag/v0.1.0 |
| Assets (7 + sums) | PASS | `super-0.1.0-{x86_64,aarch64}-{apple-darwin,unknown-linux-gnu}.tar.gz`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`, `x86_64-pc-windows-gnu.zip`, `SHA256SUMS` |
| Attestation | PASS | `gh attestation verify SHA256SUMS --repo theHamdiz/super-lang` exit 0 |
| Asset check | PASS | release lists 7 platform archives + `SHA256SUMS`; product version lock **0.1.0** |

### CI fix notes (landed on tip)

- Windows AOT: `-lwinpthread` for mingw `clock_gettime64` / `nanosleep64`.
- FreeBSD package: fold `CARGO_TARGET_*_RUSTFLAGS` into `RUSTFLAGS` inside `package_release.sh` (Cargo drops target flags when env `RUSTFLAGS` is set); explicit `-L` + libexecinfo stub.

## Item 9 — Cleanup

`.check.c`: **0** in tree (deleted). Intermediate/scratch `.sh`: **none** retained outside permanent gates (`scripts/*`, `install.sh`, `docs/install.sh`, `editors/zed/*`, `examples/interop_rust/build.sh`).

### Backup (§9.1)

```text
cb6c1dc9c124a0e9d6e39a138be7514ad72cd7512f8bdc0f6ff2945bcdd23601  /home/hamdiz/backups/super-lang/super-lang-pre-round7.bundle
c90c520079d5e08c904b42e54040081619ca517f27f2db1f76fee0b2cf43a142  /home/hamdiz/backups/super-lang/super-lang-pre-round7-worktree.tar.zst
```

`git bundle verify` OK (pre-orphan tip `782da11`).

---

## Not done / not claimed

- Zero-alloc / “proven” reclaim — not claimed (3 alloc / 0 free / req; flat RSS soak proven locally, not a dedicated CI soak job).
- Full 25-program escape-analysis corpus + soak/sanitizer as mandatory CI gates — still thin (Item 1 PARTIAL).
- macOS / FreeBSD / Windows byte-reproducible archives — not claimed.

## Tip regeneration note (Round 8 item 1)

Earlier draft of this file cited tip `0cb381a` / CI runs `46015633xxx`. Those hashes were **superseded** by the final orphan tip `f727c61cd8dcd3cd797546ac4fe2d5c903d48b4e`. Every tip citation and CI URL above was re-measured against that tip (`gh run list --commit f727c61…` / `gh run view`). Guard: `scripts/check_report_tip.py` fails if the highest-numbered `docs/ROUND*_REPORT.md` `Report tip:` line differs from `git rev-parse HEAD`.
