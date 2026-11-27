# Escape-analysis counts (measured)

Generated with `SUPER_ESCAPE_VERBOSE=1` on the tip toolchain. Numbers below are
copied from command output — not invented.

## Why sites were zero (root cause)

`collect_request_alloc_sites` only classified struct/record births inside
functions that mention `socket_req_enter` / promote / revive. Soak and hello-api
**entry modules** register route lambdas and call `serve` / `serve_with`; the
handler bodies are `Expr::Lambda` args to `App.get` / `#routes`, so they never
entered the request-scope gate. `Server.serve` (stdlib, already Super) does call
`socket_req_enter` but has no inline record births.

**Fix (no new syntax):** route-handler lambdas registered via `get`/`post`/… are
analyzed as request-scoped units; the bytecode compiler tags matching `Make*`
ops; peephole Nop compaction / LICM IP bumps remap `escape_birth` PCs so AOT
emission keeps Stay → `SUP_REG_REQ` and Promote → `super_obj_alloc_durable`.

## Soak binary (`App.serve_with` ping/echo/tick/alloc)

```text
super: escape-analysis sites=4 stay=3 promote=1 conservative=0
super: escape-analysis site 1 → stay (None)
super: escape-analysis site 2 → stay (None)
super: escape-analysis site 3 → stay (None)
super: escape-analysis site 4 → promote (StoreGlobal)
```

Emitted C banner (same build, `--keep-c`):

```text
/* escape_analysis: tagged_births=4 stay_arena=3 promote_or_conservative_durable=1 */
```

`/alloc` handler births: two item records + payload Stay; `{note: …}` stored into
`state g_escape` → Promote (`StoreGlobal`).

## hello-api (`examples/00_hello_api`)

```text
super: escape-analysis sites=6 stay=5 promote=1 conservative=0
super: escape-analysis site 1 → stay (None)
super: escape-analysis site 2 → stay (None)
super: escape-analysis site 3 → stay (None)
super: escape-analysis site 4 → promote (StoreGlobal)
super: escape-analysis site 5 → stay (None)
super: escape-analysis site 6 → stay (None)
```

```text
/* escape_analysis: tagged_births=6 stay_arena=5 promote_or_conservative_durable=1 */
```

## escape-service (`examples/31_escape_service`)

Measured from the tip `target/release/super` binary (Round 9 Item 2 fixture):
middleware (`.use`), `req.json` / `std.fmt.json`, lists/maps, durable `state`
cache roots, and many inline route lambdas with record births. Durable stores use
`state g_*` names so `StoreGlobal` edges match the classifier (`g_` prefix rule
in `opt.rs`).

```bash
SUPER_ESCAPE_VERBOSE=1 SUPER_STD=stdlib ./target/release/super build --release \
  examples/31_escape_service/src/main.sr
```

```text
super: escape-analysis sites=28 stay=24 promote=4 conservative=0
super: escape-analysis site 1 → stay (None)
super: escape-analysis site 2 → stay (None)
super: escape-analysis site 3 → stay (None)
super: escape-analysis site 4 → stay (None)
super: escape-analysis site 5 → stay (None)
super: escape-analysis site 6 → stay (None)
super: escape-analysis site 7 → promote (StoreGlobal)
super: escape-analysis site 8 → stay (None)
super: escape-analysis site 9 → stay (None)
super: escape-analysis site 10 → stay (None)
super: escape-analysis site 11 → stay (None)
super: escape-analysis site 12 → stay (None)
super: escape-analysis site 13 → stay (None)
super: escape-analysis site 14 → promote (StoreGlobal)
super: escape-analysis site 15 → stay (None)
super: escape-analysis site 16 → promote (StoreGlobal)
super: escape-analysis site 17 → stay (None)
super: escape-analysis site 18 → stay (None)
super: escape-analysis site 19 → promote (StoreGlobal)
super: escape-analysis site 20 → stay (None)
super: escape-analysis site 21 → stay (None)
super: escape-analysis site 22 → stay (None)
super: escape-analysis site 23 → stay (None)
super: escape-analysis site 24 → stay (None)
super: escape-analysis site 25 → stay (None)
super: escape-analysis site 26 → stay (None)
super: escape-analysis site 27 → stay (None)
super: escape-analysis site 28 → stay (None)
```

```text
/* escape_analysis: tagged_births=28 stay_arena=24 promote_or_conservative_durable=4 */
```

Promote sites correspond to request records written into `g_promote_sink`,
`g_cache_stats`, or `g_last_item` (e.g. `/items`, `/ingest`, `/alloc`,
`/lookup` cache bump). **Conservative:** none on this build (`conservative=0`);
no `SUPER_ESCAPE_VERBOSE` conservative reason lines.

### What escape analysis does not see

1. **Interprocedural (surgical, Round 9)** — module `+foo` functions **reachable**
   from `App.get`/`post`/… lambdas (BFS on `Ident` calls) are analyzed with
   `force_in_req` and get bytecode `escape_birth` tags; foreign / UFCS /
   indirect calls still miss callees.
2. **Named handlers** — births inside `named_handler_mirror` are counted when
   the route calls it by name (`|req| { named_handler_mirror(req) }`); inline
   `|req| { … }` bodies still get separate site IDs.
3. **`unknown_call`** — values flowing into un-modeled host/stdlib calls become
   **Conservative** with a logged reason (see honesty fixture below).
4. **Module `state` without `g_` name** — `StoreGlobal` promotion is keyed on
   durable root *identifiers* (`g_*`, `app`, …); `state foo = …` assignments
   do not match unless the binding name is recognized.

## Honesty fixture (classifier unit test)

```text
super: escape-analysis sites=2 stay=1 promote=0 conservative=1
super: escape-analysis conservative site 2: unknown_call:opaque
super: escape-analysis site 1 → stay (None)
```

(Re-run: `cargo test -p super-runtime --release print_honesty_fixture_escape_summary -- --nocapture`.)

## Proof scripts

| Script | What it proves |
|--------|----------------|
| `scripts/escape_diff.sh` | Analysis on vs `SUPER_PROMOTE_EVERYTHING=1` → identical `/alloc` bodies (`ESCAPE_FIXTURE=examples/31_escape_service/src/main.sr` for service) |
| `scripts/escape_wrong_stay.sh` | Plant Stay over a Promote durable birth → ASan/abort FAIL |
| `scripts/soak_http.sh` | Flat RSS + optional `--probe` (arena bumps ≠ zero-alloc) |
| `scripts/alloc_probe.sh`, `crates/super-cli/tests/alloc_probe.rs` | LD_PRELOAD malloc/free pin (shell + thin Rust wrapper; server under test is Super) |

## Connection-arena reclaim (related, not EA)

`SUPER_CONN_TRACE=1` on the hello-server path (20 fresh connections + 1
keep-alive with 10 requests): median **544** bytes reclaimed per
`socket_conn_leave` (see prior tip measurement).
