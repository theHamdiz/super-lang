//! Embedded stdlib module table (mirrors stdlib/mod.sr) + live source helpers.

use super::StdlibModule;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const LAYERS: &[&str] = &[
    "core", "fmt", "sys", "datetime", "io", "net", "db", "sec", "math", "data", "game", "cli",
    "ai", "tui", "gui", "html", "diag",
];

pub const MODULES: &[StdlibModule] = &[
    StdlibModule {
        path: "core.bitset",
        layer: "core",
        summary: "dense bitsets (popcount/ctz word skipping), Bloom filters, sparse bitmaps",
    },
    StdlibModule {
        path: "core.bytes",
        layer: "core",
        summary: "byte arrays, hex, endianness, bit helpers",
    },
    StdlibModule {
        path: "core.collections",
        layer: "core",
        summary: "stack, queue, set, map-like helpers over arrays",
    },
    StdlibModule {
        path: "core.decimal",
        layer: "core",
        summary: "fixed-point Decimal with Display/Eq/Ord/Hash/Add",
    },
    StdlibModule {
        path: "core.macros",
        layer: "core",
        summary: "docs for global #todo/#unimplemented/#assert/#dbg and panic",
    },
    StdlibModule {
        path: "core.reduce",
        layer: "core",
        summary: "AssociativeReducer witnesses, Welford merge, reduce_with",
    },
    StdlibModule {
        path: "core.regex",
        layer: "core",
        summary: "backtracking regular expressions with first-literal prefilter",
    },
    StdlibModule {
        path: "core.str",
        layer: "core",
        summary: "string slicing, search, case, padding, splitting",
    },
    StdlibModule {
        path: "core.traits",
        layer: "core",
        summary: "Display, Debug, Eq, Ord, Hash, Iterable, AssociativeReducer",
    },
    StdlibModule {
        path: "core.unicode",
        layer: "core",
        summary: "UTF-8 validation, scalars/graphemes, case fold, wcwidth",
    },
    StdlibModule {
        path: "core.uuid",
        layer: "core",
        summary: "UUID v4 / v7, parse and format",
    },
    StdlibModule {
        path: "core.int",
        layer: "core",
        summary: "fixed-width u8/u16/u32/u64/i8/i16/i32/i128 conversions, wrap/checked/saturating",
    },
    StdlibModule {
        path: "datetime.core",
        layer: "datetime",
        summary: "civil date/time, IANA TZ offsets, ISO 8601, calendar arithmetic (distinct from sys.time)",
    },
    StdlibModule {
        path: "datetime.cron",
        layer: "datetime",
        summary: "cron schedules with TZ + fake clock hooks",
    },
    StdlibModule {
        path: "html.core",
        layer: "html",
        summary: "context-aware XSS-safe HTML node builders; unsafe_raw greppable",
    },
    StdlibModule {
        path: "html.parse",
        layer: "html",
        summary: "tolerant HTML to text/Markdown parser with selectors",
    },
    StdlibModule {
        path: "html.template",
        layer: "html",
        summary: "Html/Text string templates; comptime T001–T004; destination TplWriter",
    },
    StdlibModule {
        path: "fmt.protobuf",
        layer: "fmt",
        summary: "Protobuf wire codec (varint, zigzag, unknown fields)",
    },
    StdlibModule {
        path: "fmt.bin",
        layer: "fmt",
        summary: "binary pack and unpack",
    },
    StdlibModule {
        path: "fmt.compress",
        layer: "fmt",
        summary: "LZ4 block compress plus Adler-32 and CRC-32",
    },
    StdlibModule {
        path: "fmt.csv",
        layer: "fmt",
        summary: "CSV rows with quoting",
    },
    StdlibModule {
        path: "fmt.json",
        layer: "fmt",
        summary: "JSON parse and stringify",
    },
    StdlibModule {
        path: "fmt.mime",
        layer: "fmt",
        summary: "media types, TypeByExtension, ParseMediaType",
    },
    StdlibModule {
        path: "fmt.msgpack",
        layer: "fmt",
        summary: "MessagePack encoder/decoder on scoped stacks",
    },
    StdlibModule {
        path: "fmt.toml",
        layer: "fmt",
        summary: "TOML subset parser",
    },
    StdlibModule {
        path: "fmt.xlsx",
        layer: "fmt",
        summary: "Office Open XML workbooks: reader, writer, DEFLATE inflate",
    },
    StdlibModule {
        path: "fmt.xml",
        layer: "fmt",
        summary: "streaming XML/SVG/RSS pull-parser and builder",
    },
    StdlibModule {
        path: "fmt.yaml",
        layer: "fmt",
        summary: "YAML 1.2 parser and serializer",
    },
    StdlibModule {
        path: "sys.cli",
        layer: "sys",
        summary: "argv tokenizer implementation (apps: use std.cli.cmd)",
    },
    StdlibModule {
        path: "sys.env",
        layer: "sys",
        summary: "environment variables",
    },
    StdlibModule {
        path: "sys.epoll",
        layer: "sys",
        summary: "Linux epoll readiness (raw OS polling)",
    },
    StdlibModule {
        path: "sys.ffi",
        layer: "sys",
        summary: "dlopen/dlsym helpers, CStr views, C23 ABI pointers",
    },
    StdlibModule {
        path: "sys.fs",
        layer: "sys",
        summary: "raw OS filesystem operations and descriptors",
    },
    StdlibModule {
        path: "sys.io_uring",
        layer: "sys",
        summary: "Linux io_uring SQ/CQ rings with epoll fallback",
    },
    StdlibModule {
        path: "sys.mmap",
        layer: "sys",
        summary: "memory-mapped files and shared memory regions",
    },
    StdlibModule {
        path: "sys.path",
        layer: "sys",
        summary: "path join, split, normalise",
    },
    StdlibModule {
        path: "sys.proc",
        layer: "sys",
        summary: "child processes",
    },
    StdlibModule {
        path: "sys.signal",
        layer: "sys",
        summary: "SIGINT/SIGTERM notify pipe for graceful shutdown",
    },
    StdlibModule {
        path: "sys.sync",
        layer: "sys",
        summary: "spin mutex, once, shared atomics",
    },
    StdlibModule {
        path: "sys.time",
        layer: "sys",
        summary: "Instant, Duration, DateTime, sleep",
    },
    StdlibModule {
        path: "io.archive",
        layer: "io",
        summary: "tar/zip archive header helpers",
    },
    StdlibModule {
        path: "io.buf",
        layer: "io",
        summary: "fixed-capacity zero-copy ring reader/writer buffers",
    },
    StdlibModule {
        path: "io.console",
        layer: "io",
        summary: "CLI-matching colored stdout/stderr",
    },
    StdlibModule {
        path: "io.fs",
        layer: "io",
        summary: "buffered high-level file readers/writers",
    },
    StdlibModule {
        path: "io.poll",
        layer: "io",
        summary: "unified cross-platform readiness abstraction",
    },
    StdlibModule {
        path: "io.socket",
        layer: "io",
        summary: "TCP/UDP/DNS socket host primitives",
    },
    StdlibModule {
        path: "io.stdio",
        layer: "io",
        summary: "process stdin, stdout, stderr and line I/O",
    },
    StdlibModule {
        path: "net.dns",
        layer: "net",
        summary: "hostname DNS lookup",
    },
    StdlibModule {
        path: "net.grpc",
        layer: "net",
        summary: "HTTP/2 framing and Protobuf wire codec for RPCs",
    },
    StdlibModule {
        path: "net.http",
        layer: "net",
        summary: "responses, typed routes, allocation-light header parsing, App.serve, cookies, middleware",
    },
    StdlibModule {
        path: "net.ip",
        layer: "net",
        summary: "IPv4/IPv6 parse, CIDR, private ranges",
    },
    StdlibModule {
        path: "net.oauth2",
        layer: "net",
        summary: "OAuth2/OIDC PKCE flows",
    },
    StdlibModule {
        path: "net.quic",
        layer: "net",
        summary: "multiplexed low-latency UDP/QUIC streams",
    },
    StdlibModule {
        path: "net.radix",
        layer: "net",
        summary: "compact radix path router (static/param/wildcard)",
    },
    StdlibModule {
        path: "net.retry",
        layer: "net",
        summary: "exponential backoff and circuit breaker",
    },
    StdlibModule {
        path: "net.smtp",
        layer: "net",
        summary: "SMTP + STARTTLS email client helpers",
    },
    StdlibModule {
        path: "net.sse",
        layer: "net",
        summary: "Server-Sent Events framing",
    },
    StdlibModule {
        path: "net.tcp",
        layer: "net",
        summary: "TCP listen, connect, read, write",
    },
    StdlibModule {
        path: "net.tls",
        layer: "net",
        summary: "TLS 1.3 client/server streams, ALPN, SNI, PEM",
    },
    StdlibModule {
        path: "net.udp",
        layer: "net",
        summary: "UDP bind, send_to, recv_from",
    },
    StdlibModule {
        path: "net.url",
        layer: "net",
        summary: "URL parse, build, query encode/decode",
    },
    StdlibModule {
        path: "net.ws",
        layer: "net",
        summary: "WebSocket framing and handshake",
    },
    StdlibModule {
        path: "db.clickhouse",
        layer: "db",
        summary: "ClickHouse HTTP client and offline columnar store",
    },
    StdlibModule {
        path: "db.migrate",
        layer: "db",
        summary: "transactional schema migration runner",
    },
    StdlibModule {
        path: "db.mysql",
        layer: "db",
        summary: "MySQL pool, ? params, AUTO_INCREMENT, REPLACE",
    },
    StdlibModule {
        path: "db.postgres",
        layer: "db",
        summary: "PostgreSQL pool, SQLSTATE, schemas, COPY, LISTEN/NOTIFY",
    },
    StdlibModule {
        path: "db.redis",
        layer: "db",
        summary: "RESP protocol and in-memory store",
    },
    StdlibModule {
        path: "db.sql",
        layer: "db",
        summary: "SQL builder and in-memory tables",
    },
    StdlibModule {
        path: "db.sqlite",
        layer: "db",
        summary: "SQLite pragmas, FTS, savepoints, attach, dump",
    },
    StdlibModule {
        path: "sec.argon2",
        layer: "sec",
        summary: "Argon2id password hashing (PHC strings)",
    },
    StdlibModule {
        path: "sec.csprng",
        layer: "sec",
        summary: "OS-backed CSPRNG; hard-fail on entropy failure (not math.rand)",
    },
    StdlibModule {
        path: "sec.secret",
        layer: "sec",
        summary: "in-memory Secret (redact/zeroize/CT) + local AEAD FileKeyring",
    },
    StdlibModule {
        path: "sec.crypto",
        layer: "sec",
        summary: "hashes, HMAC, PBKDF2, TOTP",
    },
    StdlibModule {
        path: "sec.encoding",
        layer: "sec",
        summary: "base64, base32, hex, url encoding",
    },
    StdlibModule {
        path: "sec.hash",
        layer: "sec",
        summary: "BLAKE2/3, SHA-3, SHAKE, Keccak, CRC32, FNV; host digests",
    },
    StdlibModule {
        path: "sec.jwt",
        layer: "sec",
        summary: "HS256 JSON Web Tokens",
    },
    StdlibModule {
        path: "sec.otp",
        layer: "sec",
        summary: "HOTP/TOTP and WebAuthn/Passkey helpers",
    },
    StdlibModule {
        path: "sec.pq",
        layer: "sec",
        summary: "ML-KEM, ML-DSA, hybrid X25519+ML-KEM, HBS-OTS",
    },
    StdlibModule {
        path: "sec.sha2",
        layer: "sec",
        summary: "pure-Super SHA-256 and HMAC-SHA256 (offline twin, in-place schedule)",
    },
    StdlibModule {
        path: "sec.vault",
        layer: "sec",
        summary: "HashiCorp Vault-shaped named storage; put_secret/get_secret",
    },
    StdlibModule {
        path: "sec.x509",
        layer: "sec",
        summary: "X.509 parsing, CSR helpers, ASN.1 DER",
    },
    StdlibModule {
        path: "math.ad",
        layer: "math",
        summary: "dual, hyper-dual and reverse-mode automatic differentiation",
    },
    StdlibModule {
        path: "math.algebra",
        layer: "math",
        summary: "Ring/AlgField/Matrix traits, compensated sums, ModP, distributions",
    },
    StdlibModule {
        path: "math.complex",
        layer: "math",
        summary: "complex arithmetic",
    },
    StdlibModule {
        path: "math.core",
        layer: "math",
        summary: "scalar math, trig, logs, number theory",
    },
    StdlibModule {
        path: "math.fft",
        layer: "math",
        summary: "radix-2 and Bluestein FFT (any length), real FFT, convolution",
    },
    StdlibModule {
        path: "math.geom",
        layer: "math",
        summary: "2D AABB, circles, segments",
    },
    StdlibModule {
        path: "math.integrate",
        layer: "math",
        summary: "Gauss-Kronrod, tanh-sinh, Simpson quadrature; RK4, DOPRI5, Rosenbrock4 and BDF2 ODEs",
    },
    StdlibModule {
        path: "math.interp",
        layer: "math",
        summary: "linear, cubic spline, PCHIP, Hermite and barycentric interpolation",
    },
    StdlibModule {
        path: "math.linalg",
        layer: "math",
        summary: "dense/diagonal/CSR matrices, LU, Cholesky, QR, CG, GMRES, BiCGSTAB",
    },
    StdlibModule {
        path: "math.noise",
        layer: "math",
        summary: "value, Perlin, fractal and Worley noise",
    },
    StdlibModule {
        path: "math.optimize",
        layer: "math",
        summary: "Brent, Newton, Nelder-Mead, BFGS, L-BFGS, AdamW, simplex LP, active-set QP",
    },
    StdlibModule {
        path: "math.poly",
        layer: "math",
        summary: "polynomial arithmetic, roots and Chebyshev series fitting",
    },
    StdlibModule {
        path: "math.quat",
        layer: "math",
        summary: "unit quaternions for 3D orientation",
    },
    StdlibModule {
        path: "math.rand",
        layer: "math",
        summary: "seeded MRG32k3a, counter-based Philox4x32 streams, Halton and Sobol sequences",
    },
    StdlibModule {
        path: "math.special",
        layer: "math",
        summary: "gamma, beta, erf, Bessel J/Y/I/K, zeta, Lambert W",
    },
    StdlibModule {
        path: "math.stats",
        layer: "math",
        summary: "mean, variance, percentiles, correlation",
    },
    StdlibModule {
        path: "data.column",
        layer: "data",
        summary: "typed, null-aware int/float/bool/str columns",
    },
    StdlibModule {
        path: "data.expr",
        layer: "data",
        summary: "filter/projection expression parser and Kleene evaluator",
    },
    StdlibModule {
        path: "data.frame",
        layer: "data",
        summary: "in-memory data frames: filter, group_by, join, sort",
    },
    StdlibModule {
        path: "data.io",
        layer: "data",
        summary: "frames from and to CSV and xlsx files",
    },
    StdlibModule {
        path: "data.plan",
        layer: "data",
        summary: "lazy query plans with a result-preserving optimizer",
    },
    StdlibModule {
        path: "game.ai",
        layer: "game",
        summary: "A*, steering, state machines, behavior trees",
    },
    StdlibModule {
        path: "game.app",
        layer: "game",
        summary: "fixed timestep, scenes, input actions",
    },
    StdlibModule {
        path: "game.assets",
        layer: "game",
        summary: "hot-reload asset cache, PNG/WAV loaders",
    },
    StdlibModule {
        path: "game.audio",
        layer: "game",
        summary: "PCM synthesis, WAV/OGG helpers, spatial mixer",
    },
    StdlibModule {
        path: "game.ecs",
        layer: "game",
        summary: "sparse-set entity component system",
    },
    StdlibModule {
        path: "game.fx",
        layer: "game",
        summary: "sprites, frame clips, and sound cues",
    },
    StdlibModule {
        path: "game.netcode",
        layer: "game",
        summary: "UDP game netcode sequence/ACK packets",
    },
    StdlibModule {
        path: "game.physics2d",
        layer: "game",
        summary: "circles, boxes, spatial-hash broadphase, tuple-when shape dispatch, impulse resolution",
    },
    StdlibModule {
        path: "game.tilemap",
        layer: "game",
        summary: "CSV/JSON tilemaps with AI pathfinding",
    },
    StdlibModule {
        path: "cli.cmd",
        layer: "cli",
        summary: "structured commands; re-exports cli_args/cli_parse",
    },
    StdlibModule {
        path: "ai.client",
        layer: "ai",
        summary: "shared offline/live generative AI HTTP/SSE client",
    },
    StdlibModule {
        path: "ai.mcp_client",
        layer: "ai",
        summary: "MCP client (stdio + HTTP/SSE): init, tools, resources, prompts",
    },
    StdlibModule {
        path: "ai.tokenizer",
        layer: "ai",
        summary: "tiktoken-style BPE plus documented approximate token counter",
    },
    StdlibModule {
        path: "ai.embed",
        layer: "ai",
        summary: "local vector similarity, flat/HNSW search, BPE helpers",
    },
    StdlibModule {
        path: "ai.tensor",
        layer: "ai",
        summary: "strided N-D tensor views, INT8/FP16 quant, cache-friendly i-k-j matmul",
    },
    StdlibModule {
        path: "tui.ui",
        layer: "tui",
        summary: "cell buffer, layout, widgets, and terminal keys",
    },
    StdlibModule {
        path: "gui.alias",
        layer: "gui",
        summary: "short aliases for gui surface",
    },
    StdlibModule {
        path: "gui.ui",
        layer: "gui",
        summary: "retained desktop widgets, layout, and a window presenter",
    },
    StdlibModule {
        path: "gui.widgets",
        layer: "gui",
        summary: "Widget trait and controls: toggle, dialog, toast, chrome",
    },
    StdlibModule {
        path: "gui.window",
        layer: "gui",
        summary: "native window open and event poll",
    },
    StdlibModule {
        path: "diag.bench",
        layer: "diag",
        summary: "micro-benchmark harness with warmup and timing",
    },
    StdlibModule {
        path: "diag.log",
        layer: "diag",
        summary: "structured logging with color, sinks, remote ingest",
    },
    StdlibModule {
        path: "diag.metrics",
        layer: "diag",
        summary: "counters, gauges, histograms, stats",
    },
    StdlibModule {
        path: "diag.test",
        layer: "diag",
        summary: "assertion helpers and soft reports",
    },
];

pub fn find(path: &str) -> Option<&'static StdlibModule> {
    let p = path.trim().trim_start_matches("std.");
    MODULES.iter().find(|m| m.path == p || m.path == path)
}

pub fn search(query: &str) -> Vec<&'static StdlibModule> {
    MODULES
        .iter()
        .filter(|m| {
            let blob = format!("{} {} {}", m.path, m.layer, m.summary);
            super::matches_query(&blob, query)
        })
        .collect()
}

pub fn by_layer(layer: &str) -> Vec<&'static StdlibModule> {
    MODULES.iter().filter(|m| m.layer == layer).collect()
}

/// Map `core.str` → `core/str.sr`.
pub fn path_to_rel_file(module_path: &str) -> PathBuf {
    let parts: Vec<&str> = module_path.split('.').collect();
    match parts.as_slice() {
        [a, b] => PathBuf::from(a).join(format!("{b}.sr")),
        _ => PathBuf::from(format!("{module_path}.sr")),
    }
}

pub fn resolve_stdlib_root() -> Option<PathBuf> {
    super_frontend::find_std_root()
}

pub fn read_module_source(module_path: &str) -> Option<String> {
    let root = resolve_stdlib_root()?;
    let file = root.join(path_to_rel_file(module_path));
    fs::read_to_string(file).ok()
}

pub fn extract_exports(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix('+') {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !out.iter().any(|x| x == &name) {
                out.push(name);
            }
        }
    }
    out
}

pub fn module_detail(path: &str, include_source: bool, max_source_chars: usize) -> Option<Value> {
    let m = find(path)?;
    let mut obj = json!({
        "path": format!("std.{}", m.path),
        "import": format!("use std.{}", m.path),
        "layer": m.layer,
        "summary": m.summary,
        "file": path_to_rel_file(m.path).display().to_string(),
    });
    if let Some(src) = read_module_source(m.path) {
        let exports = extract_exports(&src);
        obj["exports"] = json!(exports);
        obj["export_count"] = json!(exports.len());
        obj["source_available"] = json!(true);
        if include_source {
            let truncated = if src.len() > max_source_chars {
                let mut s = src[..max_source_chars].to_string();
                s.push_str("\n\n… source truncated; open the .sr file for the rest.");
                s
            } else {
                src
            };
            obj["source"] = json!(truncated);
        }
    } else {
        obj["source_available"] = json!(false);
        obj["hint"] = json!(
            "Set SUPER_STDLIB / SUPER_STD, or install to ~/.super/lib/stdlib (SUPER_HOME), for live source."
        );
    }
    Some(obj)
}

pub fn coverage_summary() -> Value {
    let mut layers_map = serde_json::Map::new();
    for layer in LAYERS {
        let mods: Vec<_> = by_layer(layer)
            .into_iter()
            .map(|m| json!({"path": m.path, "summary": m.summary}))
            .collect();
        layers_map.insert((*layer).to_string(), json!(mods));
    }
    let root = resolve_stdlib_root();
    json!({
        "version": "0.1.0",
        "module_count": MODULES.len(),
        "file_count_including_mod": 127,
        "file_count_all_sr": 147,
        "layers": LAYERS,
        "by_layer": layers_map,
        "stdlib_root": root.as_ref().map(|p| p.display().to_string()),
        "stdlib_root_ok": root.as_ref().map(|p| Path::new(p).join("mod.sr").is_file()).unwrap_or(false),
        "vendor_pods": [
            "pods/claude", "pods/deepseek", "pods/elevenlabs", "pods/gemini",
            "pods/openai", "pods/openrouter", "pods/seedance",
            "pods/supabase", "pods/neon", "pods/cockroachdb", "pods/mariadb", "pods/meilisearch"
        ]
    })
}
