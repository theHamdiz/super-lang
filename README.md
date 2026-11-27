<p align="center">
  <img src="assets/icons/super.png" alt="Super" width="96" height="96"/>
</p>

<h1 align="center">Super</h1>

<p align="center">
  <strong>.sr</strong> — a stack-oriented language for high-throughput services and data-heavy programs, with C23 AOT, a bytecode VM, and an optional numeric JIT.
</p>

<p align="center">
  <a href="https://github.com/theHamdiz/super-lang/actions/workflows/site-integrity.yml"><img src="https://img.shields.io/github/actions/workflow/status/theHamdiz/super-lang/site-integrity.yml?branch=main&amp;label=CI&amp;style=for-the-badge&amp;labelColor=050f16" alt="CI"/></a>
  <a href="https://thehamdiz.github.io/super-lang/install/"><img src="https://img.shields.io/badge/install-one--liner-ff7a1f?style=for-the-badge&amp;labelColor=050f16" alt="Install"/></a>
  <a href="https://thehamdiz.github.io/super-lang/"><img src="https://img.shields.io/badge/site-docs-050f16?style=for-the-badge&amp;labelColor=ff7a1f" alt="Site"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-22c55e?style=for-the-badge&amp;labelColor=050f16" alt="MIT"/></a>
  <a href="SPECIFICATION.md"><img src="https://img.shields.io/badge/version-0.1.0-38bdf8?style=for-the-badge&amp;labelColor=050f16" alt="0.1.0"/></a>
</p>

<p align="center">
  <a href="https://thehamdiz.github.io/super-lang/">Site</a> ·
  <a href="https://thehamdiz.github.io/super-lang/quickstart/">Quickstart</a> ·
  <a href="SPECIFICATION.md">Spec</a> ·
  <a href="https://thehamdiz.github.io/super-lang/stdlib-ref/">Stdlib</a> ·
  <a href="https://discord.gg/ZvgmreZsuY">Discord</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

---

## What is Super

Super (`.sr`) is an embeddable and standalone language with expression-oriented syntax, scoped data stacks, compile-time evaluation, and law/proof hooks beside ordinary tests. Today it ships a Rust-hosted CLI (`super run`, `test`, `check`, `build`, `verify`, `lsp`, `mcp`) and compiles hot paths to **C23 AOT** or runs them on the **register VM / kernel JIT**.

**Goal (not a benchmark claim):** keep call-site syntax compact and iteration latency low without sacrificing typed, checkable programs.

---

## Results at a glance

Stage-1 equal-algorithm suite, **Protocol 2.0.0** (frozen — see [docs/REPORT.md](docs/REPORT.md) and [live dashboard](https://thehamdiz.github.io/super-lang/benchmarks/)). All numbers come from one run on a single machine (Linux x86_64, 32 hardware threads); other hardware will give different results.

| Axis | Winner | Notes |
|---|---|---|
| **Composite** | SuperAot **89.2** | Overall weighted score (display rounds 89.23). |
| **Speed** | **Zig** (100.0 vs SuperAot 97.87) | Super did **not** win Speed. For `01_nbody`, `03_astar_pathfinding`, and `04_json_serde`, overlapping CIs ⇒ **within noise** vs Zig — not a hard ordering. |
| **Memory** | SuperAot | |
| **Reliability** | **All** | Every language 100% pass on gated checksums (no failures in frozen data). |
| **Cycle** | SuperJit | |
| **LoC** | SuperAot | Narrow margin: **2198** Super lines vs **2200** Bun (see LoC methodology below). |

Weights: Speed 40% · Memory 25% · Reliability 15% · Cycle 10% · LoC 10%.

**Single source of truth:** published tables come from [`docs/REPORT.md`](docs/REPORT.md), [`docs/results.json`](docs/results.json), and the frozen checksum fields inside them. Site HTML is generated from that data. CI runs [`scripts/check_numbers.py`](scripts/check_numbers.py) so README and site cannot drift.

---

## Install

```bash
sh -c 'f=$(mktemp) && trap "rm -f -- \"$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh -o "$f" && sh "$f" "$@"' sh
```

Lands in **`~/.super/`**:

```text
~/.super/bin/super
~/.super/lib/stdlib/
```

From a checkout: `./scripts/install.sh` or `make install`. Override with `SUPER_HOME` / `SUPER_STD`. Details: [install page](https://thehamdiz.github.io/super-lang/install/). Prebuilt 0.1.0 bundles: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-unknown-freebsd` (via `install.sh`), and `x86_64-pc-windows-gnu` (zip, via `install.ps1`). **Windows C23 AOT sockets: Winsock2 (WSAPoll) in 0.1.0.** (`crates/super-runtime/src/aot_host/net_host_c.rs` → emitted C). **Unsupported on Windows AOT:** TLS, QUIC, epoll, and IOCP evented I/O — see [`docs/platform.md`](docs/platform.md) and [`docs/SAFETY_CLAIMS.md`](docs/SAFETY_CLAIMS.md).

---

## Hello world

```super
+main() {
  print("hello from Super")
}
```

```bash
super run examples/doc_hello/hello.sr
super test examples/doc_test/
super verify examples/doc_law/
```

Syntax in one glance: no `fn` / `let` / semicolons; `@` mutates in place; `?` / `!` / `or` for errors; `test` blocks are first-class. Tutorials: [`QUICKSTART.md`](QUICKSTART.md) · [site quickstart](https://thehamdiz.github.io/super-lang/quickstart/).

---

## An API and its tests in 42 lines

[`examples/00_hello_api`](examples/00_hello_api/) is a complete JSON API—including
its built-in `TestClient` tests—in **42 non-blank, non-comment source lines**:

```super
+build_app() : App {
    #routes {
        get "/health" { ok({ok: true}) }
        get "/ping" { ok("pong") }
    }
}

+main() {
    app = build_app()
    @app.serve("127.0.0.1:3011") or 1
}

test "health" {
    tc = TestClient.new(build_app())
    res = @tc.get("/health")
    assert_eq(res.status, 200)
}
```

Run `super test examples/00_hello_api` to execute the tests, or set
`SUPER_ADDR` and run the server. The line count is reproducible with
`awk 'NF && $1 !~ /^\/\//' examples/00_hello_api/src/main.sr | wc -l`.

---

## Links

| Topic | Where |
|---|---|
| Specification | [`SPECIFICATION.md`](SPECIFICATION.md) · [spec](https://thehamdiz.github.io/super-lang/spec/) |
| Book | [`BOOK.md`](BOOK.md) · [PDF](https://thehamdiz.github.io/super-lang/downloads/SUPER_BOOK.pdf) |
| Stdlib (179 `.sr` files, 127 modules) | [`stdlib/`](stdlib/) · [reference](https://thehamdiz.github.io/super-lang/stdlib-ref/) |
| HTTP server | [`examples/00_hello_api`](examples/00_hello_api/) · [guide](https://thehamdiz.github.io/super-lang/http-server/) |
| Examples | [`examples/`](examples/) · [`scripts/run_examples.sh`](scripts/run_examples.sh) |
| Benchmarks & reproduction | [`benchmarks/`](benchmarks/) · [`benchmarks/REPRODUCE.md`](benchmarks/REPRODUCE.md) · [`docs/REPRODUCE_BENCHMARKS.md`](docs/REPRODUCE_BENCHMARKS.md) |
| Roadmap & security | [`ROADMAP.md`](ROADMAP.md) · [`SECURITY.md`](SECURITY.md) · [`docs/SAFETY_CLAIMS.md`](docs/SAFETY_CLAIMS.md) |
| Agent-oriented index | [`docs/llms.txt`](docs/llms.txt) |

---

## LoC methodology and caveats

Suite **LoC** counts **non-blank, non-comment** lines in each language's **primary algorithm source** (one file per algorithm; SuperAot and SuperJit share `algo.sr`). The published table is in [`docs/REPORT.md`](docs/REPORT.md).

| Language | Suite LoC | Caveat |
|---|---:|---|
| Super | **2198** | Shared across AOT/JIT. |
| Bun | **2200** | **Within 2 lines** of Super — LoC axis win is narrow. |
| C | **29,758** | Ports are **outline-expanded C23** emitted from the same algorithm specs; line count includes generated `leaf_*` helpers and verbose struct plumbing — **not** a hand-minimized reference implementation. Compare on equal algorithms, not on “half the lines of C” marketing. |

Do not infer language size from a single ratio without this counting rule.

---

## Toolchain

| Command | Purpose |
|---|---|
| `super run` / `test` / `check` / `build` | Execute, test blocks, typecheck, AOT |
| `super verify` | Law/proof discharge (`*.law.sr`, `*.proof.sr`) |
| `super lsp` · `super mcp` | Editor LSP · agent knowledge (stdio) |
| `super pod pack` / `verify` | Deterministic `.pod` archives (`Pod.toml` manifest) |

| Editor | Path |
|---|---|
| VS Code | [`editors/vscode/`](editors/vscode/) |
| Zed | [`editors/zed/`](editors/zed/) |

---

## Repository layout

```text
crates/       # lexer … sema, VM, JIT, CLI, LSP, MCP, host
stdlib/       # Super standard library (*.sr, *.law.sr, *.proof.sr)
examples/     # runnable demos (see scripts/run_examples.sh)
benchmarks/   # Stage-1 harness + frozen raw data (do not re-score)
docs/         # GitHub Pages site + REPORT.md + results.json
install.sh    # release installer (mirrored at docs/install.sh)
scripts/      # check_numbers, run_examples, site helpers
```

Host-only kernels (`sys_*`, crypto, IO) live in `crates/super-host`, not in `stdlib/`.

---

## Status and stability

**v0.1.0** — specification and tooling are public; breaking changes are still possible before a 1.0 stability promise. See [versioning](https://thehamdiz.github.io/super-lang/versioning/) and [`CHANGELOG.md`](CHANGELOG.md).

---

## License

[MIT](LICENSE) © 2026 Ahmad Hamdi