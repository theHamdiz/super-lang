# Super memory model (0.1.0)

This document defines the three lifetime classes used by the C23 AOT runtime and
the request-scoped escape analysis pass. It is the design contract for
request-arena reclamation on `App.serve` / `Server.serve`.

## Lifetime classes

Every Super value has exactly one **birth region** and one **death point**.

| Class | Birth | Death | Typical contents |
|---|---|---|---|
| **(a) Frame / stack** | Function entry or `scope` mark | Scope / function exit (cursor rewind); also reset at request leave | Locals, SROA scalars, **scratch-only** typed numeric arrays from unboxed kernels |
| **(b) Request arena** | Allocation while a request scope is active (`sup_req_enter` … `sup_req_leave`) | Request boundary (`sup_req_leave`): bump cursor rewind + `MADV_DONTNEED` | Request/Response trees, temporary COW copies, wire strings, dense arrays |
| **(c) Durable heap** | `calloc` / promote outside a request, or an explicit promote at the boundary | Process exit, or replacement when a durable binding is overwritten and the old object is unreachable | `App`, `Router`, `Server`, long-lived caches, the `Conn` rebuilt after revive |

Region tags (debug / ASan builds): every heap object header carries
`SUP_REG_FRAME`, `SUP_REG_REQ`, or `SUP_REG_DURABLE`. A `free()` of a
`SUP_REG_REQ` pointer aborts with a clear message; arena pointers are never
individually freed.

## Request boundary

The **request boundary** is the point in `Server.serve` (and equivalent host
loops) after the handler and response write have finished, and before the next
`read_request`:

1. Promote every value that must outlive the request (see below).
2. Call `sup_req_leave(mark)` — all non-promoted request-arena memory dies at once.
3. Resume with only durable (or frame) pointers live.

Crossing the boundary is allowed only for:

- Integers, floats, bools, and other immediate values.
- Durable-heap pointers (including freshly promoted graphs).
- Host handles (fds, timers) that are not arena pointers.

A durable object **must not** retain a pointer into the request arena after
`sup_req_leave`. The compiler’s escape analysis inserts promote steps so this
is true by construction; the runtime debug check aborts if a tagged REQ
pointer is observed from a DURABLE slot after leave.

## Escape analysis (compiler)

Intraprocedural dataflow (`opt::collect_request_alloc_sites` +
`escape_region::classify_*`) fills facts for every struct/record birth inside a
request-enter function. There is no blanket `has_boundary_promote = in_req`
heuristic: Stay / Promote / Conservative come from real uses (stores, returns,
closures, globals, channels, COW Conn/App updates). Set `SUPER_ESCAPE_VERBOSE=1`
(or `SUPER_ESCAPE_SUMMARY=1`) to print per-class counts and every Conservative
site with its reason.

| Class | Meaning | Codegen |
|---|---|---|
| **Stay** | Proven not to escape the request | Allocate in the request arena; no individual free |
| **Promote** | Proven to escape (stored into durable state, returned across the boundary, captured by a longer-lived closure, sent to another task, etc.) | Durable birth (`super_obj_alloc_durable`) / boundary deep-promote |
| **Conservative** | Analysis cannot prove non-escape (unknown call, complex lvalue, …) | Same as Promote (safe). Listed with reason in verbose output |

Escape sources handled:

- Store into a durable object (COW update of `App` / `Router` / caches).
- Store into a global / `state` / static.
- Return from the handler as part of durable state.
- Capture by a closure that outlives the request.
- Send to another task / thread.
- Insert into a durable collection.
- Unknown callee / complex store → **Conservative** (reason recorded).

**Graph promote** preserves sharing: one durable copy per arena object identity
in the reachable subgraph (including cycles via a promote memo table).

Conservative fallback never leaves a dangling arena pointer in durable state.

## Conn copy-on-write / connection arena

`Conn` is born in the **connection bump arena** at `accept` (`socket_conn_enter`;
region `SUP_REG_CONN`), together with peer-addr / empty-buf strings that used to
be three durable `malloc`/`calloc`s per connection. During a request, COW
updates may allocate in the request arena (**Stay** for intermediates). Before
`sup_req_leave`, `conn_req_revive` deep-promotes connection-lifetime fields into
the connection arena (or leaves an already-`SUP_REG_CONN` Conn in place). After
the connection is finished, `socket_conn_leave` rewinds the conn arena with
`MADV_DONTNEED` (bytes reported as `reclaimed_bytes`; set `SUPER_CONN_TRACE=1`
to print each leave). A regression test covers keep-alive routing after revive.

This closes the connection-churn gap for the historical **3 alloc / 0 free** per
fresh connection under `LD_PRELOAD`. It is **not** a zero-allocation claim:
request/connection arenas still bump; only durable heap growth per connect is
reclaimed.

`App` / `Router` COW that escapes the request is classified **Promote** (or
conservative promote): the updated aggregate is durable before leave so the
next request still sees routes.

## Compiler → codegen wiring (Round 8)

`optimize_module` classifies request-scoped struct/record births and stores
`SiteClass` values on `OptMeta`. The bytecode compiler tags matching
`MakeStruct` / `MakeSum` ops with a birth region (`Chunk.escape_birth`):

| Class | Birth region emitted |
|---|---|
| Stay | `super_obj_alloc_region(..., SUP_REG_REQ)` |
| Promote | `super_obj_alloc_durable(...)` |
| Conservative | `super_obj_alloc_durable(...)` — **never** the request arena |

Untagged Make* ops keep runtime Auto (arena while a request is active). Emitted
C includes an `escape_analysis:` banner. Request leave resets the frame stack and
rewinds the arena with `MADV_DONTNEED`; **`malloc_trim` is not used**.

## VM status (Round 9 Item 5)

Bytecode VM implements `sys_req_*` / `sys_conn_*` via `super_runtime::vm_req`
(thread-local pending **Stay** drops + mmap bump for `keep_str`). **`Make*`
births** consult bytecode `escape_birth` (region `1` = Stay, `2` = durable);
runtime **Auto** (`None`) births during an active request are tracked and
bulk-dropped at `socket_req_leave`.

**PARTIAL vs AOT:** host-dense kernels (`sys_arr_new`, `Bytes`, frame stack) and
untracked value paths can still allocate on the Rust heap; RSS on `super run`
servers may grow until the limit above is widened in a soak gate. Production
long-running servers should still prefer C23 AOT (`sup_req` mmap +
`MADV_DONTNEED`). Regression: `crates/super-cli/tests/vm_req_reclaim_soak.rs`,
`cargo test -p super-runtime vm_req::tests`.

## Frame stack vs durable arrays

The frame scratch stack (`g_sup_stack`) is reset at every request leave
(`sup_stack_reset`). **Anything stored into a durable object must not live on
that stack.** Host `sys_arr_new` / `sys_bytes_new` therefore allocate on the
durable heap (or the request arena when a request is active), never on the
frame stack. Unboxed numeric kernels may still call
`super_arr_alloc_*_stack` for true scratch that dies with the frame.

Round 7 root cause: `Router.prefix_heads = arr_new(64, -1)` was previously
stack-backed; after `sup_req_leave` reset the cursor, the next match overwrote
the heads table → keep-alive multi-route 404s while `/ping` often survived.

## What the probes measure (Round 9 Item 3)

| Probe | Measures | Does **not** measure |
|---|---|---|
| `scripts/alloc_probe.sh` / `LD_PRELOAD` `alloc_count.c` | Counts of `malloc` + `calloc` + `realloc` (and matching `free`) per fresh `GET /ping` | Arena bumps, mmap, stack |
| `socket_req_stats().arena_bytes` | Bytes bumped in the request arena **during the current request** (reset to 0 on `sup_req_leave`) | Process RSS; durable heap |
| `socket_conn_stats().reclaimed_bytes` | Bytes rewound on the **last** `sup_conn_leave` (`MADV_DONTNEED`) | Per-request malloc |

**Measured (this tip, Linux AOT, keep-alive `/ping` that returns `socket_req_stats`):**

```text
# Mid-request (includes building the JSON stats response itself)
arena_bytes=78568  stay_objs=24  promoted_objs=0

# Connection leave (SUPER_CONN_TRACE=1, fresh Connection: close)
super: conn-arena leave reclaimed_bytes=544 enter_count=N leave_count=N
```

**Public claim wording:** **zero malloc per request on the request path**
(`alloc_probe` pin `0.00` allocations / `0.00` frees). Never say **zero-alloc** —
request and connection arenas still bump and reclaim in bulk.

## What this is not

- Not a tracing GC.
- Not a claim of zero heap allocations / “zero-alloc” (see `alloc_probe` and `docs/facts.json`).
- Not a freelist of individually freed request objects — arena death is bulk.
