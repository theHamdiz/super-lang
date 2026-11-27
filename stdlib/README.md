# Super standard library

Pure-Super modules (`stdlib/**/*.sr`), each with its own `test` blocks. Vendor SaaS
clients (OpenAI, Claude, Supabase, Neon, …) live as first-party **Pods** under
[`../pods/`](../pods/), not in this tree.

```sh
cargo build
./target/debug/super test stdlib/          # whole library (127 module `.sr` files incl. mod.sr, plus `*.law.sr` / `*.proof.sr` sidecars)
./target/debug/super test stdlib/game/ecs.sr
```

`stdlib/mod.sr` is the manifest (`stdlib_table()`, `stdlib_find`, `stdlib_layer_count`).
It lists **126** importable modules; `find stdlib -name '*.sr' ! -name '*.law.sr' ! -name '*.proof.sr' | wc -l`
is **127** with the manifest itself. `scripts/count_stdlib.py` is the acceptance spine (179 total `.sr` files including law sidecars). `./target/debug/super verify stdlib/data/expr.sr` discharges the data laws.

## Layout (v0.1.0)

| Layer | Modules | Notes |
| --- | --- | --- |
| core | `str`, `traits`, `bytes`, `collections`, `reduce`, `regex`, `uuid`, `unicode`, `decimal`, `bitset`, `int`, `macros` | strings, traits, reducers, bytes, containers, Unicode, fixed-point, fixed-width ints |
| fmt | `json`, `csv`, `toml`, `bin`, `compress`, `mime`, `yaml`, `msgpack`, `xml`, `xlsx`, `protobuf` | text/binary formats; `xlsx` reads and writes Excel workbooks (with DEFLATE) |
| sys | `path`, `cli`, `env`, `fs`, `proc`, `time`, `signal`, `epoll`, `io_uring`, `sync`, `ffi`, `mmap` | OS primitives; **raw** epoll/io_uring live here |
| datetime | `core`, `cron` | civil dates / IANA TZ / ISO 8601; schedules (distinct from `sys.time`) |
| io | `poll`, `buf`, `fs`, `stdio`, `console`, `socket`, `archive` | high-level I/O; `io.poll` is the unified readiness API |
| net | `http`, `ws`, `url`, `ip`, `tcp`, `udp`, `dns`, `sse`, `retry`, `tls`, `smtp`, `grpc`, `quic`, `oauth2`, `radix` | general-purpose networking |
| db | `sql`, `postgres`, `mysql`, `sqlite`, `redis`, `migrate`, `clickhouse` | wire protocols only (no vendor BaaS) |
| sec | `crypto`, `encoding`, `jwt`, `sha2`, `hash`, `pq`, `vault`, `x509`, `otp`, `argon2`, `csprng`, `secret` | crypto + certs + OTP + CSPRNG + secrets (see secret vs vault table) |
| math | `core`, `linalg`, `rand`, `noise`, `complex`, `quat`, `stats`, `geom`, `fft`, `algebra`, `special`, `ad`, `optimize`, `integrate`, `interp`, `poly` | numerics: matrices and factorizations, special functions, autodiff, optimizers, quadrature/ODEs, interpolation, polynomials |
| data | `column`, `expr`, `frame`, `plan`, `io` | typed data frames, filter/group_by/join, lazy query plans with an optimizer, CSV and xlsx I/O |
| game | `ecs`, `physics2d`, `ai`, `app`, `assets`, `tilemap`, `fx`, `netcode`, `audio` | engines; netcode moved from `net/` |
| cli | `cmd` (`std.cli` / `std.clap`) | app-facing CLI parser; argv tokenizer is `std.sys.cli` |
| ai | `client`, `tensor`, `embed`, `mcp_client`, `tokenizer` | generative client + local tensors + MCP + BPE |
| tui | `ui` | terminal UI |
| gui | `ui`, `widgets`, `window`, `alias` | desktop UI; **window** moved from `sys/` |
| html | `core`, `parse`, `template` | node builders; HTML→text/Markdown; Html/Text string templates (T001–T004) |
| diag | `test`, `log`, `metrics`, `bench` | assertions, logs, metrics, micro-benchmarks |

### Dedup / placement rules

- **Polling:** `std.sys.epoll` / `std.sys.io_uring` (legacy aliases `std.io.epoll` / `std.io.uring` still resolve). Apps prefer `std.io.poll`.
- **Filesystem:** `std.sys.fs` = raw descriptors; `std.io.fs` = buffered readers/writers.
- **CLI:** apps import `std.cli.cmd` only; `std.sys.cli` is the argv tokenizer.
- **Game UDP netcode:** `std.game.netcode` (was `std.net.udp_netcode`).
- **Windowing:** `std.gui.window` (was `std.sys.window`).

### `std.sec.secret` vs `std.sec.vault`

| Module | Role |
| --- | --- |
| `std.sec.secret` | In-memory `Secret` (redacted Display/Debug/log, CT compare, `clear`/`drop` zeroize). Optional local AEAD `FileKeyring`. |
| `std.sec.vault` | HashiCorp Vault-shaped named storage (KV/auth/transit). Prefer `put_secret`/`get_secret` so values stay as `Secret`. |

`scripts/assert_secret_vault_names.py` fails CI if both modules export the same public `+Name`.

### First-party vendor Pods (`pods/`)

| Pod | Replaces |
| --- | --- |
| `openai`, `claude`, `gemini`, `openrouter`, `deepseek`, `elevenlabs`, `seedance` | former `std.ai.*` vendor modules |
| `supabase`, `neon`, `cockroachdb`, `mariadb`, `meilisearch` | former `std.db.*` vendor modules |

Each has `Pod.toml` (`kind = "lib"`), `src/lib.sr`, and inline `test` blocks.

## Conventions

- Every public helper is prefixed by its module (`ecs_`, `p2_`, …) because modules merge flat.
- Values are copied. Mutating helpers take `@x` and are called as `@x.helper(...)`.
- Arrays grow with `arr = arr + [x]` or `host.sys_arr_push`.
- Offline by default: tests never touch the network.

## Language gotchas when writing stdlib code

- Parenthesise a loop or `if` header that ends in `.field` before `{`.
- `state`, `err`, `scope`, and `shared` are keywords.
- No short-circuit `&&` / `||`; guard with nested `if`.
- Do not `return` inside `for`; set a flag instead.
- A statement beginning with `[` or `(` after another expression can be parsed as index/call — prefer `return [...]`.
- Use `\{` and `\}` for literal braces inside strings.
- `Struct.{}` does not fill defaults; constructors list every field.
