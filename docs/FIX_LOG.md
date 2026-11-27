# Super site & repo fix log

Frozen benchmark rule: **no re-run, no score/methodology edits** (R1).  
Round 2 Item 4 may change Reliability *winner encoding* only (not numeric scores).

```text
# Round 1 / pre–Item-4 bytes:
51b37aab7a45bd9b6137ae182891e7a48356bcd6a274e5a23323f95b10c69c7b  docs/results.json
# After Item 4 (reliability winner → all):
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

## Item 1 — README / published numbers

| Sub | Change | Acceptance |
|---|---|---|
| 1.1 | Replaced stale **90.07** across README, BOOK, SPEC, MCP, site JSON labels; composite **89.2** (raw 89.23). | `grep -rn 90.07` (excl. check_numbers sentinel) → empty |
| 1.2 | Speed → **Zig**; within-noise noted for nbody/astar/json. | `python3 scripts/check_numbers.py` |
| 1.3 | LoC methodology: Bun +2 lines, C **29,758** caveat. | README § LoC methodology |
| 1.4 | No unqualified cross-language superiority claims. | grep → none |
| 1.5 | Results at a glance table + REPORT/site links. | README |
| 1.6 | `scripts/check_numbers.py` + `.sh`; deliberate fail: append `90.07` → `FAIL: stale composite…` then reverted. | see command output in session |
| **+user** | **Reliability axis winner → All** (100% pass all languages); README, REPORT category note, site tiles/JSON display id `all`, noscript table, SPEC/BOOK/AGENTS/MCP. | `check_numbers.py` + visual `index.html` |

```bash
python3 scripts/check_numbers.py
# OK: no 90.07 … OK: canonical composite 89.23 (display 89.2)
```

## Item 2 — Publish promised tree

| Sub | Status | Notes |
|---|---|---|
| 2.1 | HISTORICAL | This round shipped `stdlib/` with 147 files / 126 modules. The current cross-platform ergonomics tree has 150 files / 127 modules; `python3 scripts/count_stdlib.py` verifies the live count. |
| 2.2 | DONE | `examples/doc_*` + `scripts/run_examples.sh` |
| 2.3 | DONE | `benchmarks/` harness + `data/results.json` + `REPRODUCE.md` + `SHA256SUMS` (copy, not regenerated) |
| 2.4 | DONE | `install.sh` + `docs/install.sh` (sha256 `727e4809…`) |
| 2.5 | DONE | README layout tree updated |

```bash
./scripts/run_examples.sh
# OK: all doc examples passed
```

## Item 3 — Site structure

| Sub | Status |
|---|---|
| 3.1 | DONE — `index.html` “What is Super” before charts + noscript axis table |
| 3.2 | DONE — `quickstart.html` from `scripts/generate_site_docs.py` |
| 3.3 | DONE — Prove it / harness links on `index.html` + `language.html` |
| 3.4 | DONE — Read the book → spec.html TOC links |
| 3.5 | DONE — `<noscript>` composite/axis table on index |
| 3.6 | PARTIAL — nav updated on index/language; legacy pages retain extended footer links |
| 3.7 | DONE — title/meta/canonical on generated doc pages |

```bash
python3 scripts/check_links.sh
# OK: all internal doc links resolve
```

## Item 4 — Missing doc pages

DONE — generated: `quickstart`, `docs-hub`, `comptime`, `macros-pods`, `law-proof`, `testing`, `data-stacks`, `stdlib-ref`, `versioning`, `install-security`, `syntax`, `platform` (+ `.md` twins).

```bash
./scripts/doc_examples.sh
```

## Item 5 — Hygiene

| Sub | Status |
|---|---|
| 5.1 | **SUPERSEDED CLAIM** — this line originally overstated the `v0.1.0` release matrix; the verified release contained 4 tarballs + `SHA256SUMS` (see Round-1 honesty corrections and Round 2). |
| 5.2 | DONE — `CHANGELOG.md` |
| 5.3 | DONE — `.github/workflows/site-integrity.yml` + CI badge in README |
| 5.4 | DONE — `gh repo edit` description, homepage, topics (programming-language, compiler, comptime, c23, jit, stdlib, benchmarks) |
| 5.5 | DONE — `docs/llms.txt`, `docs/llms-full.txt` |
| 5.6 | DONE — LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, CHANGELOG, issue templates |
| 5.7 | DONE — README section order |
| 5.8 | DONE — `docs/sitemap.xml`, `docs/robots.txt` |

## Final verification (F1–F4)

```bash
python3 scripts/check_numbers.py && python3 scripts/check_links.sh && ./scripts/run_examples.sh
```

**Historical status: SUPERSEDED** (the round was not fully done; see the corrected overall status below).

**Benchmark data:** not modified (SHA256 above).

## Follow-up — Reliability display on charts + benchmarks/report

| Sub | Change | Acceptance |
|---|---|---|
| R-display | `docs/charts.html` and `benchmarks/report.html` embedded `#report-data` `category.reliability` → `all`; display map adds `all` → **All**; benchmarks cat-tile matches `index` / `report` / `winners`. **Unchanged:** `docs/results.json` `category_winners.reliability` remains `super_aot` (machine tie-break per REPORT). | `python3 scripts/check_numbers.py` |

```bash
python3 scripts/check_numbers.py
# OK: no 90.07 in repo text files
# OK: README numbers and honesty
# OK: site index headline numbers
# OK: charts + benchmarks/report Reliability → All
# OK: stdlib counts
# OK: canonical composite 89.23 (display 89.2)
```

## Follow-up — GitHub Pages default landing (language home)

| Sub | Change | Acceptance |
|---|---|---|
| P-landing | `docs/index.html` is the language home (`What is Super` intro, canonical `/`). Benchmark dashboard moved to `docs/benchmarks.html` + `docs/report.html`; **removed** `what-is-super` from benchmark pages. Nav Home → `index.html`; Benchmarks → `benchmarks.html`. Updated `sitemap.xml`, `llms.txt`, `llms-full.txt`, `_includes/nav.html`, `scripts/check_numbers.py`, `benchmarks/report_page.py`, `pages.yml`. | `curl` root shows home; `benchmarks.html` has 89.2 + noscript table |
| P-deploy | Prior GitHub Pages deployments listed/deleted via `gh api` (see session log below). Fresh publish from orphan `main` tip. | Pages build succeeds |

```bash
python3 scripts/check_numbers.py
# OK: site home + benchmarks headline numbers
```

### GitHub Pages deployment cleanup (record)

Deleted via `gh api -X DELETE repos/theHamdiz/super-lang/deployments/{id}`: **8082496899, 8082467944, 8081982638, 8081908710, 8081717610, 8081600148, 8081500564, 8081392107, 8081379755, 8080959629, 8080933190** (environment `github-pages`). Active tip **8082579768** could not be deleted until superseded (GitHub API rule).

### Pretty URLs (no `.html`)

| Sub | Change |
|---|---|
| P-pretty | Flat `docs/*.html` → `docs/<slug>/index.html`; internal links use trailing-slash paths (`quickstart/`, `benchmarks/`). Root `docs/index.html` remains language home. Updated `scripts/site_urls.py`, `check_links.sh`, generators, sitemap, llms, README. |

## Follow-up — site-integrity `check_numbers` (CI 2026-11-26)

| Item | Cause | Fix |
|---|---|---|
| site-integrity / verify (push) run `45803176871` | `check_reliability_display_pages()` required `benchmarks/report.html`, which is gitignored (`benchmarks/*`) and not produced by `scripts/generate_site_docs.py` on Actions — only exists after a local `benchmarks/report_page.py` run. | Point the third path at committed `docs/report/index.html` (same `#report-data` as legacy report page). |

```bash
python3 scripts/generate_site_docs.py
python3 scripts/check_numbers.py
# OK: charts + benchmarks + report Reliability → All
./scripts/check_links.sh
# OK: all internal doc links resolve
./scripts/run_examples.sh
# OK: all doc examples passed
./scripts/doc_examples.sh
# OK: doc_examples (same as run_examples for 0.1.0)
python3 scripts/count_stdlib.py
# OK: claim matches tree
```

## Follow-up — multi-OS release bundles + installer matrix (2026-11-26; historical)

This snapshot is superseded by the cross-platform cleanup entry below. Its
planned matrix was seven archives: six `.tar.gz` files plus one Windows `.zip`,
not seven tarballs.

| Item | Notes |
|---|---|
| CI | `release-bundle.yml`: gnu (4), musl Alpine (2), FreeBSD cross (1); `scripts/verify_release_assets.sh` in smoke + package jobs |
| Installer | Historical plan used six tarballs plus one Windows zip; current installers and platform docs use those exact archive formats. |
| BLOCKED | Historical state: Windows had no bundle. Superseded below by `install.ps1` and the Windows zip release lane. WASM remains a build target, not a release bundle. |
| Local API | `~/projects/Personal/super-hello-api` — `super test .` with installed toolchain |
| macOS CI (resolved) | Historical: `gui_dispatch.c` failed to compile on GitHub macOS runners. Resolved before v0.1.0: that release shipped `x86_64-apple-darwin` and `aarch64-apple-darwin` tarballs. |
| Julia race | Score race capped at `ranked().slice(0,8)` — Julia (#9 at 28.12) omitted; show full `ranked()` + dynamic canvas height; `check_numbers.py` gate |
| Nav home | Removed duplicate Language page from nav; `docs/language/index.html` redirects to `/`; sitemap drops `/language/` |

---

## Round-1 honesty corrections (2026-11-26, Round 2 Item 5)

Preserved original Round-1 wording above. Corrections:

| Round-1 claim | Corrected status | Note |
|---|---|---|
| Item 2.3 DONE — benchmarks harness published | **PARTIAL → DONE in Round 2 Item 1** | Harness existed locally but `benchmarks/*` was gitignored; only `README.md` was tracked until commit `7a74cff`. |
| Item 3.1 DONE — noscript axis table on index | **PARTIAL** | Language home has intro; `<noscript>` score tables were missing on `/benchmarks/` until Round 2 Item 6. |
| Item 3.5 DONE — noscript on index | **PARTIAL** | Same gap: live dashboard noscript added in Round 2. |
| Item 5.1 DONE — multi-arch 7 tarballs | **PARTIAL** | v0.1.0 release only had 4 assets (linux gnu×2, musl x86_64, freebsd). Darwin/aarch64-musl were claimed but missing. |
| Follow-up R-display: results.json reliability stays `super_aot` | **SUPERSEDED** | Round 2 Item 4 sets `category_winners.reliability` = `all` (numeric scores unchanged). |
| **Overall: DONE** | **PARTIAL** | Overstated; harness unpublished + release matrix honesty gaps remained. |

**Corrected overall (end of Round 1):** PARTIAL.



---

## Round 2 — 2026-11-26

Numeric scores frozen. Reliability *winner field* only changed (Item 4).

| Item | Status | Evidence |
|---|---|---|
| 1 Publish harness | DONE | Commit `7a74cff` — `benchmarks/` runners, 20×10 ports, `data/results.json`, `REPRODUCE.md`, `SHA256SUMS`. `sha256sum -c` + `python3 -m unittest discover` OK. |
| 2 Home + /benchmarks/ | DONE | `docs/index.html` = What is Super (low-boilerplate expression syntax, stacks, comptime, law/proof, AOT+JIT). Dashboard at `docs/benchmarks/` with intro paragraph. Nav Home→`/`, Benchmarks→`/benchmarks/`. |
| 3 Link hygiene | DONE | Flat `docs/*.html` meta-refresh redirects; README pretty URLs; `scripts/check_links.sh` crawls docs+README. |
| 4 Reliability consistency | DONE | `category_winners.reliability` → `all` in docs+benchmarks results.json; tiles/README/REPORT agree. Scores remain 100%. SHA256 before `51b37aab…` → after `714f530a…`. |
| 5 FIX_LOG honesty | DONE | Round-1 corrections above; this Round 2 section. |
| 6 Small bugs | DONE | `/testing/` doubled-brace literal → single `{ ... }`; quickstart BOOK/SPEC GitHub+site links; `/benchmarks/` noscript tables; Prove it / Read the book filled. |
| 7 CI + release honesty | DONE | release-bundle: drop darwin/aarch64-musl matrix; smoke glob fix; `install.sh` fails unsupported platforms; platform docs list exact 4 shipped triples. |

### SHA256 pins (Item 4 / V4 baseline)

```text
PRE  (numeric-identical bytes before winner-field edit):
51b37aab7a45bd9b6137ae182891e7a48356bcd6a274e5a23323f95b10c69c7b  docs/results.json

POST (after category_winners.reliability=all only):
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

**Round 2 Items 1–7 overall:** DONE (pending live Verification Gate + Phase D).

### Follow-up A — static-link ld warnings (`super b -r`)

| Status | Evidence |
|---|---|
| DONE | Default release/size/fast presets no longer emit `-static` on glibc (opt-in via `--static` / Pod `static = true`; prefer musl for true static). **Verified unit tests (both exit 0, correct filters):** `cargo test -p super-runtime --lib --locked profiles_match_cargo_and_gcc_defaults`; `cargo test -p super-runtime --lib --locked size_profile_cc_args_request_gc_and_lto`. Fresh release CLI + `super b -r` in `super-hello-api`: no `-static` in Optimizing line; no `dlopen`/`getaddrinfo` ld warnings. Tip CI site-integrity + release-bundle green on `a87fadc`. |

### Follow-up B — Pod packaging E2E

| Status | Evidence |
|---|---|
| DONE | `package_entry` accepts flat `main.sr` beside `Pod.toml` (examples/doc_pod). `super pod pack .` gate: VM + JIT + C23 green. Unit test `package_entry_accepts_flat_main_beside_manifest`. |

### Follow-up C — post Phase D

| Status | Notes |
|---|---|
| PENDING | Reinstall via published `install.sh`, convert/test `super-hello-api` as pod, confirm clean `super b -r` after reinstall. |


### Verification Gate (pre–Phase D)

- V1 Items 1–7 DONE (logged above).
- V2 site-integrity green on tip `a87fadc` (and Round 2 commits).
- V3 live site verified: root intro, `/benchmarks/` + noscript, README links, old `.html` redirects.
- V4 SHA256 `docs/results.json` = `714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb` (matches `benchmarks/data/results.json`). Pre–Item-4 bytes were `51b37aab…`.

### Phase D

D1 backup: `/home/hamdiz/backups/super-round2/super-backup-202611261442.bundle` (verified) + `super-workdir-202611261442.tar.zst`.

### Phase D release honesty — musl packaging (2026-11-26)

| Item | Status | Evidence |
|---|---|---|
| release-bundle `45822839904` package-musl | FAILED | Alpine image lacked `git` (`sh: git: not found`, exit 127) before `git config safe.directory`. |
| Published assets after that run | PRESENT (4 tarballs + SHA256SUMS) | `gh release view v0.1.0` listed musl; downloaded tarball SHA matched SHA256SUMS; `bin/super` is static-pie and prints `super 0.1.0`. Asset likely attached from a concurrent/prior artifact path while publish only required `package-gnu` success. |
| Fix | DONE (this pass) | `apk add … git …` in musl Docker step; publish now requires gnu+musl+freebsd success and asserts exactly the four `install.sh` SHIPPED_TARGETS tarballs before attach. Re-orphan + re-cut `v0.1.0` so tip workflow and release assets are consistent. |


### Round 2 closing (Phase D complete)

| Step | Status | Evidence |
|---|---|---|
| D1 Backup | DONE | `/home/hamdiz/backups/super-round2/super-backup-202611261442.bundle` (verify OK) + `super-workdir-202611261442.tar.zst` |
| D2 Releases/tags deleted | DONE | Prior `v0.1.0` removed then re-cut |
| D3 Pages deployments/envs | DONE | Deployments cleared; `github-pages` env recreated by redeploy |
| D4 Orphan squash | DONE | Single commit `48aeeaa` “Initial public release of Super 0.1.0” on `main` (no Co-Authored-By) |
| D5 Re-cut release | DONE | https://github.com/theHamdiz/super-lang/releases/tag/v0.1.0 — 4 tarballs + SHA256SUMS |
| D6 Pages redeploy | DONE | Live root intro + `/benchmarks/` verified |
| D7 Checklist | DONE | See parent report |

**SHA256 (numeric scores frozen; Reliability winner field = all):**

```text
pre-Item4:  51b37aab7a45bd9b6137ae182891e7a48356bcd6a274e5a23323f95b10c69c7b
post-Item4: 714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
post-Item4: 714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
```

**Round 2 overall: DONE**


### Follow-up — `super pod pack` no longer hangs on server mains

**Root cause:** pack JIT gate called `JitEngine::compile_and_run`, which executed `main`. Apps that `listen`/`serve` (e.g. super-hello-api) printed `listening on …` and never returned.

**Fix:** `JitEngine::compile_kernels` + pack `gate_jit` compiles kernels only (no `main` execution). Regression: `pod_pack_gate_does_not_execute_blocking_main`.

**Verify:** `timeout 30s super pod pack .` and separate `timeout 180s super b -r` in `~/projects/Personal/super-hello-api` both exit 0; no glibc static/dlopen warnings on build.


### Re-orphan (D7 single-commit)

Folded pack-hang fix (`gate_jit` compile-kernels-only; hello-api pack no longer listens) into a fresh single orphan tip so `git rev-list --count HEAD` = 1. Prior two-commit tip (`ff7aa6f` + `195a146`) superseded. Backup: `/home/hamdiz/backups/super-round2/super-reorphan-202611261506.bundle`.

---

## Cross-platform release cleanup — 2026-11-26

Benchmark measurements remain frozen. Baseline SHA256 values captured before this
round:

```text
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
00931ddd51718f69f16b2af63b6cda36b7cc9d16061a05f7717272ec958bca50  benchmarks/REPORT.md
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
```

| Item | Status | Evidence |
|---|---|---|
| A1 Reliability label | DONE | `benchmarks/REPORT.md` label changed from sole `super_aot` to `all`; no score, table, chart, or methodology value changed. |
| A2 stale log claims | DONE | Historical release count and premature overall status above are explicitly superseded; the verified v0.1.0 release matrix was four tarballs. |
| A3 site 404 | DONE | `docs/404.html` provides styled recovery links to Home, Benchmarks, and the docs index. |
| B1 comparison wording | DONE | Public language copy describes Super on its own terms; historical comparison wording was removed from all rendered documentation and metadata. |
| B2 cross-platform implementation | IMPLEMENTED — CI PENDING | PAL selects epoll/io_uring on Linux, kqueue on macOS/FreeBSD, and documented WSAPoll fallback on Windows. C23 target guards, JIT→VM fallback, seven-archive packaging, POSIX/PowerShell verified installers, and native CI jobs are present. Local Linux, Windows GNU cross, FreeBSD cross, Windows C23, installer corruption, and release-bundle smokes pass; native hosted CI is still required. |
| B4 conditional compilation | IMPLEMENTED — CI PENDING | The 0.1.0 language specification defines `all`/`any`/`not`, target/mode/feature predicates, and stripping before imports/macros. Compiler and stdlib platform law/proof tests pass locally, including `#cfg(test)`. Product/toolchain remains 0.1.0. |
| B5 Pod features | IMPLEMENTED — CI PENDING | `[features]` resolves defaults, explicit names, transitive edges, all/no-default selection, and unknown-feature errors. Build/emit CLI flags and four-configuration integration tests pass locally; hosted tier-1 CI remains required. |
| GNU glibc release link | DONE | `/home/hamdiz/projects/Personal/super-hello-api`: current `super b -r` completed with no `-static`, `dlopen`, or `getaddrinfo` warning. Pod pack gate also completed. Musl remains the static release target. |

Post-A1 SHA256 values:

```text
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  benchmarks/data/results.json
192d1b601b506a1a4f7bcb0454142ab63465d750aa651a918b26082f7f7f67e5  benchmarks/REPORT.md
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
```

The only frozen-file byte change is the permitted A1 Reliability category label.

**Current overall: PARTIAL — Phase D is prohibited until all Phase C gates pass.**

---

## Ergonomics and documentation round — 2026-11-26

Product, crates, CLI, installers, and release tag remain **0.1.0**. The language
document is a **0.2.0 draft** for `#cfg`, Pod features, and the API changes in
this section.

### Typed route-handler root cause and fix

The route builder previously accepted an unconstrained callable parameter.
Lambda inference therefore had no required `Response` result type; a `str`,
`i64`, or unit tail could compile and later be discarded by dispatch. Route
registration now uses `Fn[Request, Response]`, applies that expected signature
while checking the lambda, and reports the route path, `App` method, expected
type, actual type, source span, and response-constructor hint.

| API | Required callable | Constrained before | Current |
|---|---|---:|---|
| `App.get/post/put/patch/delete/head/options/any` | `Fn[Request, Response]` | No | Enforced |
| `App.mount` | `Fn[Request, Response]` | No | Enforced |
| `App.crud` | `CrudHandlers` of typed route callables | Partial | Enforced by field types |
| `routes {}` (now the `#routes` macro, Round 5) | same callables as the plain builder | N/A | Same checker and source spans |
| middleware `before` / `after` | declared `Request`/`Response` methods | Yes | Retained |
| `spawn` and test hooks | inferred result retained by task/test semantics | Yes | No discarded-value hole found |

### Canonical-name audit

| Previous surface | Canonical surface | Result |
|---|---|---|
| route `*_fn` methods | `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `any` | Removed and migrated |
| array/container length aliases | `len(value)` / `value.len()` | Removed and migrated |
| Unicode scalar length | `char_count(text)` | Kept distinct from UTF-8 byte `len` |
| quaternion `len` | `norm` | Renamed; magnitude is not collection length |
| HTTP `Server.serve_until` wrapper | `Server.serve` | Removed |

Names such as substring `count(text, needle)`, database row counts, non-empty
cell counts, bit-set cardinality, and filesystem line counts are not aliases
for storage length and remain explicit. `byte_len`/`byte_at`/`byte_slice`
remain the explicit UTF-8 wire-index family; `len(str)` is byte length while
`char_count` is scalar count. The frozen
`benchmarks/15_physics2d_broadphase_solver/algo.sr` source still contains its
measurement-era length spelling and is not presented as current API example
content; changing it would break the byte-for-byte benchmark freeze.

### Assertions

(Round 5 renamed these to the `#assert` / `#assert_err` macros; the old call
forms are now compile errors that name the new form.)
`assert(expr)` captures immediate comparison/call operands into temporaries,
evaluates each once, and prints source text plus captured values on failure.
`assert_err(expr[, exact_text])` covers value-error paths. The source migration
replaced 3,655 boolean `assert_eq` calls in stdlib/examples/Pods; book and docs
were migrated separately. Meaningful two-value comparisons retain
`assert_eq`.

### Server builder and measurement

The common path is typed registration followed by `@app.serve(address)`;
listener creation, SIGINT/SIGTERM notification, serving, and cleanup are
integrated. `serve_with(address, ServeConfig)` exposes stack-only timeout,
header/body, and keep-alive limits. Direct `Server`/`Notify` wiring is the
advanced path.

The separate `benchmarks/server-builder/` measurement (Linux x86_64 release
AOT, multiple runs; see its README and SHA256SUMS) recorded:

| Variant | Median RPS | p50 | p99 | Peak RSS | Startup median |
|---|---:|---:|---:|---:|---:|
| `App.serve` | 15,335.12 | 1.022 ms | 1.214 ms | 7,632 KiB | 0.387 ms |
| verbose wiring | 14,984.20 | 1.037 ms | 1.338 ms | 7,672 KiB | 0.401 ms |

Full source was 12 versus 8 lines, so the “half the total LOC” target was not
met. Setup ceremony was 7 versus 3 lines. Generated behavior uses the same
`Server.serve` loop. A counting-allocator measurement and automated
zero-heap-allocation assertion are still **UNVERIFIED**; no allocation claim
is made from RSS alone. Task A therefore remains **PARTIAL**. (Resolved later:
Phase D ran and v0.1.0 was published. Round 5 added the counting-allocator
gate; see the Round 5 entry.)

---

## Cross-platform ergonomics completion — 2026-11-26

| Item | Status | Evidence |
|---|---|---|
| Handler and route ergonomics | DONE | Callable route handlers are type-checked; clean HTTP verb methods, `routes {}` expansion, and compile-fail diagnostics have tests. |
| Server builder | DONE | `App.new().get(...).serve(...)` and graceful serving are implemented. `examples/00_hello_api` and the external hello API contain the API plus five built-in assertions in 42 nonblank, non-comment lines. |
| Assertion ergonomics | DONE | `assert` captures safe operands once and `assert_err` checks value errors with optional text; CLI regression tests cover single evaluation. |
| Universal length | DONE | `len(value)` / `value.len()` covers collections and UTF-8 byte length; `char_count` is the explicit Unicode scalar count. Compiler, runtime, stdlib, and documentation were migrated without removing domain-specific measures such as vector magnitude. |
| Documentation parity | DONE | Specification, MDX editions, book sources, generated PDF, web book pages, site pages, `llms` files, README, and contributor guidance cover the cross-platform and ergonomics features. Complete documentation modules are compiled in CI. |
| Server load probe | PARTIAL | Three 5-second, concurrency-32 runs measured a median 14,993.65 requests/s (p50 2.112 ms, p99 2.511 ms, zero HTTP errors); a 128-client probe completed 78,342 requests with zero errors and p99 10.402 ms. RSS grew from 2,104,708 KiB to 6,372,592 KiB, so no release performance claim was added to README. Raw commands, host details, and results are in `benchmarks/server-builder/`. |

Product, toolchain, installers, and release remain **0.1.0**. The language
specification is a **0.2.0 draft** describing the new source features; this is
not a product release version. Frozen composite benchmark hashes remain:

```text
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
192d1b601b506a1a4f7bcb0454142ab63465d750aa651a918b26082f7f7f67e5  benchmarks/REPORT.md
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
```

**Current overall:** content and Phase C verification are complete. The clean
v0.1.0 release is produced from this tree; destructive cleanup evidence is
recorded in the final release report rather than embedded before publication.

---

## Round 5 — 0.1.0 (2026-11-26)

Product, crates, CLI, installers, editors, site, and the release tag are all
**0.1.0**; the specification documents this round as the Round 5 language
revision of 0.1.0, not a new product version. The frozen benchmark measurement is
unchanged; the placeholder `benchmarks/REPORT.md` listed in the hash block
above was a fake fixture (no generation time, invented scores) and was
deleted. The published measurement is only `docs/REPORT.md` plus
`docs/results.json`.

| Work item | Status | Evidence |
|---|---|---|
| WI1 identifier-name special cases | PARTIAL | `routes {}`, `assert`, `assert_err` are `#routes` / `#assert` / `#assert_err` macros; `len` parses as an ordinary call; spec §3.9 carries the design rule. Remaining sema-level name heuristics are listed below. |
| WI2 migration and clean break | DONE | Old forms are compile errors that name the new form (`cli.rs` old-form test, parser test `name_followed_by_brace_points_to_the_macro_form`). Grep proofs below return no matches outside history files and the intentional old-form tests. |
| WI3 `super fmt` / `super lint` | DONE | 15 formatter tests (idempotent over the embedded stdlib, examples, and pods), 21 lint tests, CLI and LSP tests, CI `style` job. `super fmt --check stdlib examples pods`: 219 files formatted. Lint: 0 warnings on stdlib (115 info, S005), clean on examples and pods. |
| WI4 allocations on the server path | PARTIAL | `crates/super-cli/tests/alloc_probe.rs` pins 256 allocations and 7 frees per `GET /ping`. Root cause below; not fixed in 0.1.0. |
| WI5 docs truthfulness | DONE | Fake fixture removed, stale BLOCKED rows annotated, zero-allocation wording removed where unmeasured, single-machine and Windows caveats added, semicolon rule corrected in the spec. |

### Remaining name heuristics (WI1)

These are not syntax: each name still resolves as an ordinary function,
method, or binding, and a user definition with the same name wins. They are
typing shortcuts in `sema.rs` that a later release should move into typed
prelude signatures.

| Name(s) | Where | What it does |
|---|---|---|
| `len`, `.len()`, `host.sys_arr_len` | `sema.rs` call/method typing | Types the call as `i64` when no user `len` exists. A struct or sum that declares `len` now keeps its own return type (test `a_declared_len_method_keeps_its_own_return_type`). |
| `print`, `println`, `assert_eq`, `round`, `map`, `keep`, `group_by`, `sum` | `sema.rs` name lookup | Language prelude names typed without a declaration. |
| `map`, `keep`, `group_by`, `sum`, `round` | `sema.rs` pipeline typing | Result type comes from the expected type. |
| `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, `any`, `mount` | `sema.rs` route registration | Applies the `Fn[Request, Response]` signature to the handler lambda. |
| `send` | `sema.rs` | Root-lifetime escape check on arguments. |
| `http`, `ecs`, `db`, `collections` | `sema.rs` | Missing-`use` hint only. |
| `host` | `sema.rs` | The embedding namespace. |
| `self`, `_`, `it`, `index` | parser/sema | Contextual keywords. |

### Allocation root cause (WI4)

`main` registers routes with closures. The typed (unboxed) AOT planner does
not lower `MakeFn`, so `main` and the handlers fall back to the boxed
`SuperVal` runtime ("boxed fallback: main — unsupported op MakeFn"). Boxed
values are never freed: host string results (substrings, concatenation, the
64 KiB `sys_tcp_read` buffer per request), `super_add`, `super_obj_alloc`
(about 2 KiB per Request/Response object), and arrays. Region allocation
(`sup_region_enter` / `sup_ralloc`) is only used by the unboxed planner; with
no region open, `sup_ralloc` calls `malloc`. Measured RSS grew from 3 MB to
1.3 GB over 47k requests (concurrency 32, 3 s). A fix needs request-scoped
reclamation for boxed values or closure support in the unboxed planner.
Docs no longer describe the server path as zero-allocation.

### GCC 15 empty-reply bug (C23 AOT server)

On the Ubuntu 26.04 CI image (GCC 15.2) every `GET /ping` to a
`super build --release` server got an empty reply; GCC 14 and GCC 16.2 builds
were fine.
`-O1 -fno-tree-dse` restored replies. Bisecting with per-function
`optimize("no-tree-dse")` isolated `super_host_net` (`net_host.c`), and
branch-level reduction left three branches: `sys_signal_recv`,
`sys_bytes_concat`, and `sys_bytes_slice`. The trigger is the read of
`static volatile sig_atomic_t g_sig_last` in `sys_signal_recv`: with it
present, GCC 15 deletes the `memcpy` and NUL stores into the freshly
`malloc`'d result buffers of the other two branches, so request heads read back
empty and the server waits for more input until the socket timeout. A
60-line standalone C file reproduces it at `-O1` and above with GCC 15.2 and
not with GCC 14 or GCC 16.2. The flag is now `static _Atomic int` (lock-free, so still
async-signal-safe); `net_host_has_no_volatile_objects` guards the source and
`alloc_probe` exercises the server end to end. Allocation counts under GCC 15
are unchanged: 256 allocations and 7 frees per request.

### C equivalence against the pre-Round-5 tree (`scripts/c_equivalence.py`)

- `benchmarks/15_physics2d_broadphase_solver/algo.sr` does not compile
  before or after Round 5 (it uses the removed `arr_len`; the file is frozen).
  With `arr_len(` spelled `len(` in a temporary copy, the pre-Round-5 and
  Round 5 C output is equivalent.
- `super-hello-api` (migrated copy: `#routes`, `#assert`): the only
  difference is that 12 length calls go through the generic `__len` host
  instead of `host.sys_arr_len`, and 3 redundant dup/pop pairs are gone. Both
  return the array length for arrays. `super test .` passes (5 assertions).

### Lint exceptions

- `stdlib/lint.toml`: S002 stutter for 69 files (the `std.<module>.<module>_*`
  naming the stdlib uses for unqualified imports), and 13 S006 entries where
  `@` marks a handle's state change or keeps a frozen benchmark kernel's
  by-reference signature (`mcp_client`, `console.write_line`,
  `http.set_nonblock`, `http.app_run_after`, `tcp.accept`, `tcp.write`,
  `udp.send_to`, `udp.recv`, `collections.gd_lin_fit_i`).
- `examples/02_task_api/Pod.toml`: S002 `AuthToken`, S003 `password_hash`.

### Gate (this tree)

| Command | Result |
|---|---|
| `cargo test --workspace` | 644 passed, 0 failed |
| `super test stdlib` / `examples` / `pods` | 24,891 / 1,352 / 153 assertions passed |
| `super fmt --check stdlib examples pods` | 219 files formatted |
| `super lint --config stdlib/lint.toml stdlib`, `super lint examples pods` | 0 errors, 0 warnings |
| `scripts/check_numbers.py`, `scripts/check_links.sh`, `scripts/check_doc_code.py` | OK (264 doc modules) |
| benchmark and site unittests | OK |
| `cargo clippy --workspace --all-targets -- -D warnings` | Clean. 0.1.0 failed this gate under clippy 1.99; Round 5 fixed the findings, and the remaining style lints (index loops, wide emitter signatures, unboxed error types, the C ABI's raw-pointer entry points) are allowed at crate level with a stated reason. |

Frozen hashes, unchanged:

```text
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
```

---

## WI6 — orphan release cleanup (2026-11-26, Round 5 closeout)

| Step | Status | Evidence |
|---|---|---|
| Backup | DONE | `/home/hamdiz/backups/super-lang/super-wi6-pre-recut-20261126-215758.bundle` (verify OK, sha256 `81c110386938…`) + worktree `…tar.zst` (`084ed151…`) |
| Delete releases/tags/deployments | DONE | Releases empty; remote `v0.1.0` deleted then re-created; Pages deployment `8094562064` inactivated/removed; fresh deploy `8094740121` |
| Orphan tip | DONE | `git rev-list --count HEAD` = 1 on `main`; author Ahmad Hamdi; no Co-Authored-By / Cursor trailers. Prior content tip before this WI6 log fold-in: `6d81304f85555392a5b8a2366e40780400aef73f`. |
| Force-push + re-cut | DONE | Annotated tag `v0.1.0` → this tip; CI builds platform assets + `SHA256SUMS` on the tag push |
| Assets | DONE | 7 archives + `SHA256SUMS` (linux gnu×2, musl x86_64, freebsd, darwin×2, windows zip); `bin/super --version` → `super 0.1.0` |
| Pages | DONE | Fresh Pages deploy from tip; live `/` softwareVersion 0.1.0; benchmarks copy clean (no “Ultimate overall winner” / “press one to filter”) |

Release: https://github.com/theHamdiz/super-lang/releases/tag/v0.1.0

---

## Round 6 — 0.1.0 (2026-11-27)

Baseline (pre-edit): recorded in `/tmp/round6-baseline.txt` and summarized here.

| Field | Value |
|---|---|
| HEAD | `c38db23ad47199a6ff7269a4547a74d552534440` |
| `super --version` | `super 0.1.0` |
| Stdlib | 150 `.sr` files → **179** after new law sidecars; **127** implementation modules |
| Laws / proofs | 11 → **40** modules with laws; 11 proof pairs retained |
| Frozen `docs/REPORT.md` | `dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4` |
| Frozen `docs/results.json` | `714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb` |
| Releases | one `v0.1.0`; Pages deployments present |

### Item 1 — Request-scoped reclamation (`App.serve`)

| Sub | Status | Evidence |
|---|---|---|
| 1.1 Allocation profile | DONE | `docs/http-server.md` § Allocation profile |
| 1.2 Request arena | PARTIAL | AOT: `sup_req_*` bump for strings/arrays; `Server.serve` enter/leave + `conn_req_revive`. Objects stay durable calloc (bump under live `App` caused keep-alive 404). VM host: `sys_req_*` stubs only (no owned-value reclaim). Escape compile-fail for request-scoped refs: **PARTIAL** — rely on existing E0402 + revive/keep_str; no new request-scope escape diagnostic. Bare-string route compile-fail: already in `cli.rs` / `sema.rs`. |
| 1.3 `alloc_probe` | DONE (pin update) | Before: 256 alloc / 7 free (net 249). After: **42 alloc / 0 free (net 42)**. Pins updated in `alloc_probe.rs`. |
| 1.4 RSS soak | PARTIAL | `http_rss_soak.rs` + CI 10k path. Measured ~14 KiB RSS/request from remaining object calloc; not flat. Full 1M / net-0 soak **not** claimed. |
| 1.5 Microbench | DONE | Before median **10757.44** RPS; after **14129.23** RPS (faster; noise margin N/A). Same machine. Not frozen suite. |
| 1.6 Zero-alloc claim | DONE (honest) | **No** zero-alloc claim published. Docs state net 42 + test name. |

Commands:

```bash
# differential alloc measure (LD_PRELOAD)
# per request: allocs=42.00 frees=0.00 net=42.00
python3 benchmarks/server-builder/load.py --port 3022 --pid $SPID --concurrency 32 --duration 5 --runs 3
# before median rps=10757.44; after median rps=14129.23
```

### Item 2 — Windows AOT sockets

| Status | B — UNSUPPORTED |
|---|---|
| Phrase | **Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.** |
| CI | `scripts/check_windows_aot_net.py` in `site-integrity.yml` |
| Option A | Not implemented (no green Windows AOT HTTP request job) |

### Item 3 — Laws / proofs coverage

| Sub | Status | Evidence |
|---|---|---|
| 3.1 Coverage script | DONE | `scripts/law_coverage.py` → `docs/law_coverage.json`; floor `docs/law_coverage_floor.json` = 40 |
| 3.2 Expand laws | PARTIAL | **40** modules with `.law.sr` (target met). Many new laws are **pure sidecar models** (verifier cannot execute most stdlib bodies at comptime). Spot-check `super verify` green on 10 modules. |
| 3.3 Mutation + vacuous lint | PARTIAL | Vacuous tautology lint bans `x\|\|!x` / `x==x\|\|x!=x`. Mutation report: module injection often does not falsify pure sidecar models (documented). |
| 3.4 `super verify` CI | PARTIAL | Linux gates + coverage floor; not every platform runs full verify matrix. |
| 3.5 Published numbers | DONE | From coverage script / `docs/facts.json`, not hand-typed |

### Item 4 — Drift / verifiable releases

| Sub | Status | Evidence |
|---|---|---|
| 4.1 `facts.json` | DONE | `scripts/generate_facts.py`, `check_numbers.py` extended |
| 4.2 Attestations / repro | PARTIAL | `attest-build-provenance` on `SHA256SUMS`; per-archive attest PARTIAL; musl byte-identical **PARTIAL** (documented in `VERIFYING_RELEASES.md`) |
| 4.3 audit / fuzz | DONE/PARTIAL | `supply-chain.yml` cargo-audit + cargo-deny; bounded JSON fuzz — no cargo-fuzz harness |

### Item 5 — Adoption docs

DONE — `CONTRIBUTING.md` good-first issues; issue templates + PR template; `ROADMAP.md`; `docs/REPRODUCE_BENCHMARKS.md`; `SECURITY.md`; `docs/SAFETY_CLAIMS.md`; `docs/VERIFYING_RELEASES.md`; site links.

### Item 6 — Version discipline

DONE — product version **0.1.0** only; `scripts/check_version.py` passes.

### Gate / cleanup

Recorded as work proceeds; frozen hashes must match baseline through the gate.

### Round 6 final report

**Overall: PARTIAL** (core deliverables landed with honest gaps; cleanup/release re-cut follows this log).

| Item | Status | Evidence |
|---|---|---|
| 1 Reclamation | PARTIAL | AOT net **42** (was 249); bump reclaim strings/arrays; SuperObj durable; VM reclaim stubs; no zero-alloc claim. Microbench before 10757 → after 14129 RPS. Soak RSS still grows ~14 KiB/req. |
| 2 Windows AOT sockets | DONE (B) | Unsupported phrase + `check_windows_aot_net.py` |
| 3 Laws | PARTIAL | 11→**40** modules; pure sidecar models; vacuous lint; mutation PARTIAL |
| 4 Facts/releases | PARTIAL | `facts.json` + checks; SHA256SUMS attestation; musl repro PARTIAL |
| 5 Adoption | DONE | ROADMAP, SAFETY_CLAIMS, REPRODUCE_BENCHMARKS, VERIFYING_RELEASES, templates |
| 6 Version | DONE | 0.1.0 only; `check_version.py` |

Frozen hashes (must match baseline through cleanup):

```text
dcdb9441b279aaf0b4e4867d75af4c0103e7a2faccbc4f262ba89d992fe554f4  docs/REPORT.md
714f530adce2de7e63214684101bca703504df29a0c505f40378e11e79304edb  docs/results.json
```

alloc_probe: `app_serve_allocations_per_request_are_pinned` → 42.00 / 0.00 (PASS).

### Round 6 backup (§8.1)

```text
ed24865d235f4332e779b87e788effa8d3a1cd468ebd0b27e99e926b5bdcd298  /home/hamdiz/backups/super-lang/super-lang-pre-round6.bundle
7f03bce8842aa4db8d9fcbd271f352ab5187ea322346c741c42b4febe5480f61  /home/hamdiz/backups/super-lang/super-lang-pre-round6-worktree.tar.zst
```

Bundle clone HEAD matched pre-cleanup `c38db23…` / tree `bfdec0a…`. Worktree tarball holds uncommitted Round 6 edits.



---

## Round 7 — 0.1.0 (2026-11-27)

See `docs/ROUND7_REPORT.md` for full evidence. Summary:

| Item | Status |
|---|---|
| 1 Memory / flat RSS soak | PARTIAL→near-DONE (3 alloc/0 free; 30m soak PASS; CI soak job thin) |
| 2 Real-body laws + mutation | DONE (93.5% mutation; 12 real-body / 28 sidecar) |
| 4 Reproducible gnu+musl | DONE (identical SHA256 dual-build; pinned Alpine digest) |
| 5 Windows AOT Winsock2 | DONE (Option A + Windows CI TCP loopback green on tip) |
| 6 Docs/claims truth | DONE |
| 3 CI/release/attestations | DONE tip `0cb381a` / `v0.1.0` — full matrix green; 7 assets; `SHA256SUMS` attested |
| 9 Cleanup | `.check.c` 0; scratch `.sh` 0; permanent scripts kept |

Frozen benches unchanged: `dcdb9441…` / `714f530a…`.


## Round 8 (pre-orphan fold-in)

Items 1–5 local evidence in `docs/ROUND8_REPORT.md`. Escape analysis wired into AOT codegen; `malloc_trim` removed; `memory-gates` CI workflow added; laws **17/40** real-body; platforms honesty for TLS/QUIC/epoll + non-repro macOS/FreeBSD/Windows. Ending: bundle backup + orphan re-cut `v0.1.0`.
