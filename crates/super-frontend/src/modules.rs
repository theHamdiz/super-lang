//! Ultra-simple module system.
//!
//! ```super
//! use auth                 // ./auth.sr — all +exports
//! use models.{User, Task}  // selective exports from one module
//! use std.data.{column, frame, plan}  // sibling modules (≡ three `use` lines)
//! use std.gui as App       // local module name
//! use std.gui.{Gui as Desk}
//! use std.net.http         // standard library (embedded `stdlib/`)
//! use std.game.ecs
//! use std.db.postgres
//! ```

use crate::ast::*;
use crate::cfg::{CfgContext, strip_cfg_items};
use crate::lexer::Lexer;
use crate::meta::{MetaCache, MetaEngine};
use crate::parser::Parser;
use crate::sema::{SemanticAnalyzer, TypedModule};
use crate::token::{Token, TokenKind};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use super_core::{Diagnostics, Interner, SourceFile, Span, SymId};

include!(concat!(env!("OUT_DIR"), "/embedded_stdlib.rs"));

/// Map `std.<segs>` → relative path under `stdlib/` (and legacy `std/`).
fn std_alias(segs: &[String]) -> Option<&'static str> {
    if segs.first().map(|s| s.as_str()) != Some("std") {
        return None;
    }
    let rest: Vec<&str> = segs.iter().skip(1).map(|s| s.as_str()).collect();
    let key = rest.join(".");
    Some(match key.as_str() {
        "" | "mod" => "mod.sr",
        // Domain A
        "str" => "core/str.sr",
        "traits" | "core.traits" => "core/traits.sr",
        "bytes" => "core/bytes.sr",
        "collections" => "core/collections.sr",
        "reduce" | "core.reduce" => "core/reduce.sr",
        "regex" => "core/regex.sr",
        "uuid" | "core.uuid" => "core/uuid.sr",
        // Domain B
        "fs" => "sys/fs.sr",
        "path" => "sys/path.sr",
        // Application entry: `std.cli.cmd` commands. Low-level argv stays at `sys.cli`.
        "cli" | "cli.cmd" | "clap" => "cli/cmd.sr",
        "sys.cli" | "cli.args" => "sys/cli.sr",
        "tui" | "tui.ui" => "tui/ui.sr",
        "gui" | "gui.ui" => "gui/ui.sr",
        "gui.widgets" => "gui/widgets.sr",
        "gui.window" | "window" => "gui/window.sr",
        "gui.alias" => "gui/alias.sr",
        "game.fx" | "fx" => "game/fx.sr",
        "env" => "sys/env.sr",
        "proc" => "sys/proc.sr",
        "time" | "sys.time" => "sys/time.sr",
        // Legacy `std.sys.window` → gui.window
        "sys.window" => "gui/window.sr",
        "sync" | "sys.sync" => "sys/sync.sr",
        "epoll" | "sys.epoll" | "io.epoll" => "sys/epoll.sr",
        "io_uring" | "sys.io_uring" | "io.uring" => "sys/io_uring.sr",
        "sys.ffi" | "ffi" => "sys/ffi.sr",
        "sys.mmap" | "mmap" => "sys/mmap.sr",
        "io.poll" | "poll" | "net.poll" => "io/poll.sr",
        "io.buf" => "io/buf.sr",
        "io.fs" => "io/fs.sr",
        "io.stdio" => "io/stdio.sr",
        "io.console" | "console" => "io/console.sr",
        "io.socket" => "io/socket.sr",
        "io.archive" | "archive" => "io/archive.sr",
        // Domain C
        "json" => "fmt/json.sr",
        "bin" => "fmt/bin.sr",
        "toml" => "fmt/toml.sr",
        "csv" => "fmt/csv.sr",
        "compress" | "fmt.compress" => "fmt/compress.sr",
        "mime" | "fmt.mime" => "fmt/mime.sr",
        "yaml" | "fmt.yaml" => "fmt/yaml.sr",
        "msgpack" | "fmt.msgpack" => "fmt/msgpack.sr",
        "xml" | "fmt.xml" => "fmt/xml.sr",
        // Domain D — networking
        "http" | "net.http" => "net/http.sr",
        "ws" | "net.ws" => "net/ws.sr",
        "net.game" | "netcode" | "game.netcode" | "net.udp_netcode" => "game/netcode.sr",
        "url" | "net.url" => "net/url.sr",
        "ip" | "net.ip" => "net/ip.sr",
        "tcp" | "net.tcp" => "net/tcp.sr",
        "udp" | "net.udp" => "net/udp.sr",
        "dns" | "net.dns" => "net/dns.sr",
        "sse" | "net.sse" => "net/sse.sr",
        "retry" | "net.retry" => "net/retry.sr",
        "tls" | "net.tls" => "net/tls.sr",
        "smtp" | "net.smtp" => "net/smtp.sr",
        "grpc" | "net.grpc" => "net/grpc.sr",
        "quic" | "net.quic" => "net/quic.sr",
        "oauth2" | "net.oauth2" => "net/oauth2.sr",
        // Domain E — generic DB wire protocols (vendor BaaS lives in pods/)
        "db" => "db/sql.sr",
        "db.pg" | "db.postgres" | "postgres" => "db/postgres.sr",
        "db.redis" | "redis" => "db/redis.sr",
        "db.sqlite" | "sqlite" => "db/sqlite.sr",
        "db.migrate" | "migrate" => "db/migrate.sr",
        "db.clickhouse" | "clickhouse" => "db/clickhouse.sr",
        "db.mysql" | "mysql" => "db/mysql.sr",
        // Domain F
        "crypto" => "sec/crypto.sr",
        "encoding" => "sec/encoding.sr",
        "jwt" => "sec/jwt.sr",
        "sha2" => "sec/sha2.sr",
        "hash" => "sec/hash.sr",
        "pq" => "sec/pq.sr",
        "vault" | "sec.vault" => "sec/vault.sr",
        "x509" | "sec.x509" => "sec/x509.sr",
        "otp" | "sec.otp" => "sec/otp.sr",
        "argon2" | "sec.argon2" => "sec/argon2.sr",
        "signal" | "sys.signal" => "sys/signal.sr",
        // Domain G — mathematics
        "math" | "math.core" => "math/core.sr",
        "linalg" | "simd" | "math.linalg" => "math/linalg.sr",
        "rand" | "math.rand" => "math/rand.sr",
        "noise" | "math.noise" => "math/noise.sr",
        "complex" | "math.complex" => "math/complex.sr",
        "quat" | "math.quat" => "math/quat.sr",
        "stats" | "math.stats" => "math/stats.sr",
        "geom" | "math.geom" => "math/geom.sr",
        "fft" | "math.fft" => "math/fft.sr",
        "ecs" => "game/ecs.sr",
        "physics2d" => "game/physics2d.sr",
        "ai" | "game.ai" => "game/ai.sr",
        "app" | "input" => "game/app.sr",
        "audio" | "game.audio" => "game/audio.sr",
        "assets" | "game.assets" => "game/assets.sr",
        "tilemap" | "game.tilemap" => "game/tilemap.sr",
        // Domain H
        "test" | "diag.test" => "diag/test.sr",
        "log" | "diag.log" => "diag/log.sr",
        "metrics" | "diag.metrics" => "diag/metrics.sr",
        "bench" | "diag.bench" => "diag/bench.sr",
        // Domain I — generative AI (vendor pods: openai/claude/…; std keeps client + tensor)
        "ai.core" | "ai.client" => "ai/client.sr",
        "ai.tensor" | "tensor" => "ai/tensor.sr",
        "ai.embed" | "embed" => "ai/embed.sr",
        "unicode" | "core.unicode" => "core/unicode.sr",
        "decimal" | "core.decimal" => "core/decimal.sr",
        "bitset" | "core.bitset" => "core/bitset.sr",
        "int" | "core.int" => "core/int.sr",
        "datetime" | "datetime.core" => "datetime/core.sr",
        "datetime.cron" | "cron" => "datetime/cron.sr",
        "html" | "html.core" => "html/core.sr",
        "html.parse" => "html/parse.sr",
        "html.template" | "template" => "html/template.sr",
        "protobuf" | "fmt.protobuf" => "fmt/protobuf.sr",
        "csprng" | "sec.csprng" => "sec/csprng.sr",
        "secret" | "sec.secret" | "keyring" => "sec/secret.sr",
        "mcp_client" | "ai.mcp_client" | "mcp.client" => "ai/mcp_client.sr",
        "tokenizer" | "ai.tokenizer" => "ai/tokenizer.sr",
        // Fallback: dotted path → nested file under stdlib/
        other => return legacy_rel_path(other),
    })
}

fn legacy_rel_path(dotted: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(v) = guard.get(dotted) {
        return Some(*v);
    }
    let path = format!("{}.sr", dotted.replace('.', "/"));
    let leaked: &'static str = Box::leak(path.into_boxed_str());
    guard.insert(dotted.to_string(), leaked);
    Some(leaked)
}

/// Resolved target of a top-level `use` in the entry file (for go-to-definition).
#[derive(Clone, Debug)]
pub struct ImportTarget {
    pub span: Span,
    pub path: PathBuf,
}

/// Definition site for a symbol merged from another module (stdlib or sibling).
#[derive(Clone, Debug)]
pub struct SymbolOrigin {
    pub path: PathBuf,
    pub span: Span,
}

/// Result of analyzing an entry file with its `use` graph resolved.
pub struct ProjectAnalyze {
    pub module: Module,
    pub typed: TypedModule,
    pub interner: Interner,
    pub diagnostics: Diagnostics,
    pub tokens: Vec<Token>,
    /// Local names brought in via `use std.net.http` → `"http"`, etc.
    pub std_imports: HashSet<String>,
    pub entry_dir: PathBuf,
    pub meta_cache: MetaCache,
    pub std_root: Option<PathBuf>,
    pub import_targets: Vec<ImportTarget>,
    /// `//!` / `/*!` documentation for each loaded module file.
    pub module_docs: HashMap<PathBuf, SymId>,
    pub symbol_origins: HashMap<SymId, SymbolOrigin>,
}

fn is_stdlib_root(p: &Path) -> bool {
    p.join("mod.sr").is_file() || p.join("net/http.sr").is_file() || p.join("http.sr").is_file()
}

/// `SUPER_STDLIB` / `SUPER_STD` from the environment when it points at a valid stdlib tree.
pub fn find_std_root_from_env() -> Option<PathBuf> {
    for key in ["SUPER_STDLIB", "SUPER_STD"] {
        let Ok(p) = std::env::var(key) else { continue };
        let pb = PathBuf::from(p);
        if is_stdlib_root(&pb) {
            return Some(pb);
        }
    }
    None
}

/// Walk upward from `start` (file or directory) for a project `stdlib/` root.
pub fn find_std_root_near(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    for _ in 0..16 {
        let stdlib = dir.join("stdlib");
        if is_stdlib_root(&stdlib) {
            return Some(stdlib);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// `$SUPER_HOME` / `~/.super` / `$XDG_DATA_HOME/super` toolchain root.
fn super_home_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("SUPER_HOME")
        && !p.is_empty()
    {
        out.push(PathBuf::from(p));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        out.push(PathBuf::from(home).join(".super"));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        out.push(PathBuf::from(xdg).join("super"));
    }
    out
}

/// Locate an installed toolchain stdlib (`$SUPER_HOME/lib/stdlib`, next to `super`, legacy share).
fn find_installed_std_root() -> Option<PathBuf> {
    for home in super_home_candidates() {
        for rel in ["lib/stdlib", "share/super/stdlib", "stdlib"] {
            let cand = home.join(rel);
            let resolved = cand.canonicalize().unwrap_or(cand);
            if is_stdlib_root(&resolved) {
                return Some(resolved);
            }
        }
    }
    let exe = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;
    for rel in [
        "../lib/stdlib",
        "lib/stdlib",
        "../share/super/stdlib",
        "share/super/stdlib",
        "../lib/super/stdlib",
        "lib/super/stdlib",
    ] {
        let cand = bin_dir.join(rel);
        let resolved = cand.canonicalize().unwrap_or(cand);
        if is_stdlib_root(&resolved) {
            return Some(resolved);
        }
    }
    None
}

/// Walk upward from the process cwd for a project-local `stdlib/`.
fn find_std_root_from_cwd() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..8 {
        let stdlib = dir.join("stdlib");
        if is_stdlib_root(&stdlib) {
            return Some(stdlib);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Locate the Super standard library root.
///
/// Search order (first hit wins) — matches `install.sh` / `scripts/install.sh`:
/// 1. `SUPER_STDLIB` / `SUPER_STD`
/// 2. Project-local `stdlib/` near cwd (so a Super checkout never loses to a stale install)
/// 3. `$SUPER_HOME/lib/stdlib`, `~/.super/lib/stdlib`, next-to-exe / legacy share layouts
/// 4. `None` — callers fall back to the embedded copy via `stdlib://…`
pub fn find_std_root() -> Option<PathBuf> {
    find_std_root_from_env()
        .or_else(find_std_root_from_cwd)
        .or_else(find_installed_std_root)
}

/// Like [`find_std_root`], but prefers a `stdlib/` next to the open file (LSP / `super check path`).
///
/// Order: env → entry-adjacent project → cwd project → installed share → embedded.
/// Walk upward from `start` looking for a directory that contains `Pod.toml`.
fn find_pod_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join("Pod.toml").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Read an LSP/buffer overlay for `path`, trying the raw and canonicalized keys.
fn overlay_text(overlays: &HashMap<PathBuf, String>, path: &Path) -> Option<String> {
    if let Some(t) = overlays.get(path) {
        return Some(t.clone());
    }
    let canon = path.canonicalize().ok()?;
    overlays.get(&canon).cloned()
}

pub fn find_std_root_for_entry(entry: &Path) -> Option<PathBuf> {
    find_std_root_from_env()
        .or_else(|| find_std_root_near(entry))
        .or_else(find_std_root_from_cwd)
        .or_else(find_installed_std_root)
}

/// One row from `stdlib/mod.sr` `+stdlib_table()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdlibModuleMeta {
    /// Dotted manifest id, e.g. `core.str` or `net.http`.
    pub manifest_path: String,
    pub layer: String,
    pub summary: String,
    /// Canonical `use` line, e.g. `use std.net.http`.
    pub use_line: String,
    /// Relative path under `stdlib/`, e.g. `net/http.sr`.
    pub source_rel: String,
}

/// Build the canonical `use std.…` import for a manifest path.
///
/// Always uses the fully qualified manifest id (`use std.net.http`, `use std.core.str`).
/// Shorter aliases (`use std.http`, `use std.str`) still resolve via [`std_alias`] for
/// backward compatibility, but tooling and docs treat the manifest path as canonical.
pub fn stdlib_use_line_for_manifest(manifest: &str) -> String {
    format!("use std.{manifest}")
}

fn parse_stdlib_manifest_triples(src: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut in_table = false;
    for line in src.lines() {
        if line.contains("+stdlib_table()") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line.trim_start().starts_with(']') {
            break;
        }
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() < 7 {
            continue;
        }
        let path = parts[1];
        let layer = parts[3];
        let summary = parts[5];
        if !path.contains('.') {
            continue;
        }
        out.push((path.to_string(), layer.to_string(), summary.to_string()));
    }
    out
}

fn load_stdlib_manifest_text(std_root: Option<&Path>) -> Option<String> {
    if let Some(root) = std_root {
        let manifest = root.join("mod.sr");
        if let Ok(text) = fs::read_to_string(&manifest) {
            return Some(text);
        }
    }
    embedded_stdlib_get("mod.sr").map(str::to_owned)
}

fn catalog_from_manifest(src: &str) -> Vec<StdlibModuleMeta> {
    parse_stdlib_manifest_triples(src)
        .into_iter()
        .map(|(manifest_path, layer, summary)| {
            let use_line = stdlib_use_line_for_manifest(&manifest_path);
            let segs: Vec<String> = use_line
                .strip_prefix("use ")
                .unwrap_or(&use_line)
                .split('.')
                .map(|s| s.to_string())
                .collect();
            let source_rel = std_alias(&segs).unwrap_or("mod.sr").to_string();
            StdlibModuleMeta {
                manifest_path,
                layer,
                summary,
                use_line,
                source_rel: source_rel.to_string(),
            }
        })
        .collect()
}

/// Shipped stdlib modules for editor completion (`stdlib/mod.sr` manifest).
pub fn stdlib_module_catalog() -> Vec<StdlibModuleMeta> {
    stdlib_module_catalog_for(find_std_root().as_deref())
}

/// Like [`stdlib_module_catalog`], but reads `mod.sr` from `std_root` when present.
pub fn stdlib_module_catalog_for(std_root: Option<&Path>) -> Vec<StdlibModuleMeta> {
    load_stdlib_manifest_text(std_root)
        .as_deref()
        .map(catalog_from_manifest)
        .unwrap_or_default()
}

fn matches_import_filter(m: &StdlibModuleMeta, prefix: &str) -> bool {
    let prefix = prefix.trim().trim_end_matches('.');
    if prefix.is_empty() {
        return true;
    }
    let canonical = m.manifest_path.as_str();
    if canonical == prefix || canonical.starts_with(&format!("{prefix}.")) {
        return true;
    }
    let parts: Vec<&str> = prefix.split('.').collect();
    let segs: Vec<&str> = canonical.split('.').collect();
    // `use std.net.h` → `net.http` (exact parents, prefix on the active segment).
    if !parts.is_empty() && parts.len() <= segs.len() {
        let parents_ok = parts[..parts.len() - 1]
            .iter()
            .zip(segs.iter())
            .all(|(want, got)| want == got);
        if parents_ok
            && let Some(got) = segs.get(parts.len() - 1)
            && got.starts_with(parts[parts.len() - 1])
        {
            return true;
        }
    }
    // `use std.h` / `use std.http` — match any path segment.
    if parts.len() == 1 {
        let p = parts[0];
        if segs.iter().any(|seg| *seg == p || seg.starts_with(p)) {
            return true;
        }
    }
    let suffix = m.use_line.strip_prefix("use std.").unwrap_or(&m.use_line);
    suffix == prefix || suffix.starts_with(&format!("{prefix}."))
}

/// Next path segments to offer for `use std.{prefix}` (e.g. `net.` → `http`, `tcp`).
pub fn stdlib_import_next_segments(prefix: &str) -> Vec<String> {
    let prefix = prefix.trim().trim_end_matches('.');
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for m in stdlib_module_catalog() {
        let path = m.manifest_path.as_str();
        let rest = if prefix.is_empty() {
            path
        } else if path == prefix {
            continue;
        } else if let Some(r) = path.strip_prefix(&format!("{prefix}.")) {
            r
        } else {
            continue;
        };
        let seg = rest.split('.').next().unwrap_or(rest);
        if seen.insert(seg.to_string()) {
            out.push(seg.to_string());
        }
    }
    out
}

/// `use std.…` completion rows, optionally filtered by path suffix (`"http"`, `"io.u"`).
pub fn stdlib_use_completions(import_suffix: Option<&str>) -> Vec<StdlibModuleMeta> {
    stdlib_use_completions_for(find_std_root().as_deref(), import_suffix)
}

/// Workspace-aware variant for LSP (respects `SUPER_STD` / entry-adjacent `stdlib/`).
pub fn stdlib_use_completions_for(
    std_root: Option<&Path>,
    import_suffix: Option<&str>,
) -> Vec<StdlibModuleMeta> {
    stdlib_module_catalog_for(std_root)
        .into_iter()
        .filter(|m| import_suffix.is_none_or(|p| matches_import_filter(m, p)))
        .collect()
}

/// Read module source for editor navigation (`stdlib://…` or on-disk `.sr`).
pub fn module_source_text(path: &Path, std_root: Option<&Path>) -> Option<String> {
    if let Some(s) = path.to_str()
        && let Some(rel) = s.strip_prefix("stdlib://")
    {
        if let Some(root) = std_root {
            let on_disk = root.join(rel);
            if let Ok(text) = fs::read_to_string(&on_disk) {
                return Some(text);
            }
        }
        if let Some(embed) = embedded_stdlib_get(rel) {
            return Some(embed.to_owned());
        }
    }
    fs::read_to_string(path).ok()
}

/// URI string for an LSP location pointing at a resolved module path.
pub fn module_path_to_file_uri(path: &Path, std_root: Option<&Path>) -> Option<String> {
    if let Some(s) = path.to_str()
        && let Some(rel) = s.strip_prefix("stdlib://")
        && let Some(root) = std_root
    {
        let on_disk = root.join(rel);
        if on_disk.is_file() {
            return Some(format!("file://{}", on_disk.display()));
        }
    }
    if path.is_file() {
        return Some(format!("file://{}", path.display()));
    }
    None
}

/// Resolve `use std.*` to embedded source text when available.
pub fn std_embedded_source(path_segs: &[String]) -> Option<&'static str> {
    let rel = std_alias(path_segs)?;
    embedded_stdlib_get(rel)
}

fn parse_source(
    source: &SourceFile,
    interner: &mut Interner,
    diagnostics: &mut Diagnostics,
    file: u32,
) -> (Module, Vec<Token>) {
    let tokens = Lexer::new(&source.text, interner, diagnostics)
        .with_file(file)
        .tokenize();
    let mut parser = Parser::new(&tokens, &source.text, interner, diagnostics);
    let module = parser.parse_module();
    (module, tokens)
}

fn resolve_module_path(
    from_dir: &Path,
    path_segs: &[String],
    std_root: Option<&Path>,
    extra_search_paths: &[(String, PathBuf)],
) -> Option<PathBuf> {
    if path_segs.is_empty() {
        return None;
    }
    if path_segs[0] == "std" {
        // Prefer aliased stdlib layout; fall back to flat legacy paths.
        if let Some(rel) = std_alias(path_segs) {
            if let Some(root) = std_root {
                let p = root.join(rel);
                if p.is_file() {
                    return Some(p);
                }
                // Legacy `std/http.sr` when root is old `std/`.
                if let Some(name) = path_segs.get(1) {
                    let flat = root.join(format!("{name}.sr"));
                    if flat.is_file() {
                        return Some(flat);
                    }
                }
            }
            // Virtual path for embedded modules (loaded via `std_embedded_source`).
            return Some(PathBuf::from(format!("stdlib://{rel}")));
        }
        let root = std_root?;
        if path_segs.len() == 1 {
            let p = root.join("mod.sr");
            return if p.is_file() {
                Some(p)
            } else {
                Some(PathBuf::from("stdlib://mod.sr"))
            };
        }
        let mut p = root.to_path_buf();
        for (i, seg) in path_segs[1..].iter().enumerate() {
            if i + 1 == path_segs[1..].len() {
                p.push(format!("{seg}.sr"));
            } else {
                p.push(seg);
            }
        }
        return if p.is_file() { Some(p) } else { None };
    }
    // Relative to importer: auth → ./auth.sr (canonical), ./auth.sup (legacy alias).
    let mut p = from_dir.to_path_buf();
    for (i, seg) in path_segs.iter().enumerate() {
        if i + 1 == path_segs.len() {
            p.push(format!("{seg}.sr"));
        } else {
            p.push(seg);
        }
    }
    if p.is_file() {
        return Some(p);
    }
    // Legacy `.sup` alias when the canonical `.sr` sibling is absent.
    let mut alt = from_dir.to_path_buf();
    for (i, seg) in path_segs.iter().enumerate() {
        if i + 1 == path_segs.len() {
            alt.push(format!("{seg}.sup"));
        } else {
            alt.push(seg);
        }
    }
    if alt.is_file() {
        return Some(alt);
    }
    for (prefix, root) in extra_search_paths {
        if path_segs.first() != Some(prefix) {
            continue;
        }
        let remainder = &path_segs[1..];
        if remainder.is_empty() {
            // Library Pods use `lib.sr`; macro Pods use `macros.sr`.
            for name in ["lib.sr", "macros.sr"] {
                let candidate = root.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        } else {
            let mut candidate = root.clone();
            for (index, segment) in remainder.iter().enumerate() {
                if index + 1 == remainder.len() {
                    candidate.push(format!("{segment}.sr"));
                } else {
                    candidate.push(segment);
                }
            }
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn load_module_text(path: &Path, path_segs: &[String]) -> Option<String> {
    if let Some(s) = path.to_str()
        && let Some(rel) = s.strip_prefix("stdlib://")
        && let Some(src) = embedded_stdlib_get(rel)
    {
        return Some(src.to_owned());
    }
    if let Ok(t) = fs::read_to_string(path) {
        return Some(t);
    }
    // Last resort: embedded alias even if virtual path differed.
    std_embedded_source(path_segs).map(|s| s.to_owned())
}

fn register_item_origins(origins: &mut HashMap<SymId, SymbolOrigin>, path: &Path, items: &[Item]) {
    for item in items {
        let (name, span) = match item {
            Item::Binding(b) if b.exported => (b.name, b.span),
            Item::Struct(s) if s.exported => (s.name, s.span),
            Item::Sum(s) if s.exported => (s.name, s.span),
            Item::Function(f) if f.exported => (f.name, f.span),
            Item::Trait(t) if t.exported => (t.name, t.span),
            _ => continue,
        };
        origins.entry(name).or_insert(SymbolOrigin {
            path: path.to_path_buf(),
            span,
        });
    }
}

fn is_exported_item(item: &Item) -> bool {
    match item {
        Item::Binding(b) => b.exported,
        Item::Struct(s) => s.exported,
        Item::Sum(s) => s.exported,
        Item::Function(f) => f.exported,
        Item::Trait(t) => t.exported,
        Item::Macro(m) => m.exported,
        Item::State(_) | Item::Test(_) | Item::Use(_) | Item::Rewrite(_) => false,
    }
}

fn item_name(item: &Item, interner: &Interner) -> Option<String> {
    let sym = match item {
        Item::Binding(b) => b.name,
        Item::Struct(s) => s.name,
        Item::Sum(s) => s.name,
        Item::Function(f) => f.name,
        Item::Trait(t) => t.name,
        Item::Macro(m) => return Some(format!("#{}", interner.resolve(m.name))),
        _ => return None,
    };
    Some(interner.resolve(sym).to_string())
}

/// Merge import bindings for one source `use` span (braced multi-module expands
/// to several loads that share the original span).
fn merge_entry_bindings(
    map: &mut HashMap<(u32, u32), Vec<SymId>>,
    key: (u32, u32),
    incoming: Vec<SymId>,
) {
    let slot = map.entry(key).or_default();
    for sym in incoming {
        if !slot.contains(&sym) {
            slot.push(sym);
        }
    }
}

/// A later `use` of a module that was first imported with a selector still
/// needs the exports that selector left out. Append those, and the private
/// helpers they call, without duplicating names already merged.
/// An entry-file `use`: binding key, resolved module path, and `{selectors}`.
type EntryUsePath = ((u32, u32), PathBuf, Option<Vec<SymId>>);

#[allow(clippy::too_many_arguments)]
fn append_missing_exports(
    path: &Path,
    segs: &[String],
    selectors: Option<&[SymId]>,
    merged: &mut Vec<Item>,
    interner: &mut Interner,
    diagnostics: &mut Diagnostics,
    origins: &mut HashMap<SymId, SymbolOrigin>,
    file: u32,
) -> Vec<SymId> {
    let Some(text) = load_module_text(path, segs) else {
        return Vec::new();
    };
    let src = SourceFile::new(path.display().to_string(), text);
    let (dep_mod, _) = parse_source(&src, interner, diagnostics, file);
    let exports = filter_exports(dep_mod.items, selectors, interner);
    let mut have: HashSet<String> = merged
        .iter()
        .filter_map(|item| item_name(item, interner))
        .collect();
    let mut added = Vec::new();
    let mut fresh = Vec::new();
    for item in exports {
        let Some(name) = item_name(&item, interner) else {
            continue;
        };
        if !have.insert(name) {
            continue;
        }
        if is_exported_item(&item) {
            match &item {
                Item::Binding(binding) => added.push(binding.name),
                Item::Struct(def) => added.push(def.name),
                Item::Sum(def) => added.push(def.name),
                Item::Function(function) => added.push(function.name),
                _ => {}
            }
        }
        fresh.push(item);
    }
    register_item_origins(origins, path, &fresh);
    merged.extend(fresh);
    added
}

fn filter_exports(items: Vec<Item>, selectors: Option<&[SymId]>, interner: &Interner) -> Vec<Item> {
    // Keep every item so private helpers / comptime tables stay available for
    // the transitive closure pass; selection happens after closure.
    let all = items;
    let mut keep: HashSet<String> = HashSet::new();
    for it in &all {
        if is_exported_item(it)
            && let Some(n) = item_name(it, interner)
        {
            if let Some(sels) = selectors {
                let want: HashSet<String> = sels
                    .iter()
                    .map(|s| interner.resolve(*s).to_string())
                    .collect();
                if want.contains(&n) {
                    keep.insert(n);
                }
            } else {
                keep.insert(n);
            }
        }
    }
    // Transitive private dependency closure (P0-3): exported `crc32_bytes_lut`
    // needs private `#CRC32_LUT`, private helpers, etc.
    let by_name: std::collections::HashMap<String, &Item> = all
        .iter()
        .filter_map(|it| item_name(it, interner).map(|n| (n, it)))
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        let frontier: Vec<String> = keep.iter().cloned().collect();
        for name in frontier {
            let Some(it) = by_name.get(&name) else {
                continue;
            };
            let mut refs = HashSet::new();
            collect_item_refs(it, interner, &mut refs);
            for r in refs {
                if by_name.contains_key(&r) && keep.insert(r) {
                    changed = true;
                }
            }
        }
    }
    all.into_iter()
        .filter(|it| {
            item_name(it, interner)
                .map(|n| keep.contains(&n))
                .unwrap_or(false)
        })
        .collect()
}

fn collect_item_refs(item: &Item, interner: &Interner, out: &mut HashSet<String>) {
    match item {
        Item::Binding(b) => collect_expr_refs(&b.value, interner, out),
        Item::Function(f) => collect_fn_refs(f, interner, out),
        Item::Struct(s) => {
            for m in &s.methods {
                collect_fn_refs(m, interner, out);
            }
            for f in &s.fields {
                collect_type_refs(&f.ty, interner, out);
                if let Some(d) = &f.default {
                    collect_expr_refs(d, interner, out);
                }
            }
        }
        Item::Sum(s) => {
            for m in &s.methods {
                collect_fn_refs(m, interner, out);
            }
            for v in &s.variants {
                for (_, ty) in &v.fields {
                    collect_type_refs(ty, interner, out);
                }
            }
        }
        Item::Macro(m) => {
            for rule in &m.rules {
                collect_token_refs(&rule.template, interner, out);
            }
        }
        _ => {}
    }
}

/// Identifiers and `#name` macro references inside a raw token stream (macro
/// templates and unexpanded call input). Over-approximates, which only keeps
/// extra private helpers alive.
fn collect_token_refs(stream: &TokenStream, interner: &Interner, out: &mut HashSet<String>) {
    let mut after_hash = false;
    for tok in &stream.tokens {
        if let Some(sym) = tok.sym.filter(|_| tok.kind == TokenKind::Ident) {
            let name = interner.resolve(sym);
            if after_hash {
                out.insert(format!("#{name}"));
            }
            out.insert(name.to_string());
        } else if tok.kind == TokenKind::String
            && let Some(sym) = tok.sym
        {
            collect_string_hole_refs(interner.resolve(sym), out);
        }
        after_hash = tok.kind == TokenKind::Hash;
    }
}

/// Body plus signature: a private struct named only in `: T` / `p: T` (and built with
/// `.{…}`) must still travel with the function that uses it.
fn collect_fn_refs(f: &Function, interner: &Interner, out: &mut HashSet<String>) {
    for p in &f.params {
        collect_type_refs(&p.ty, interner, out);
    }
    if let Some(ret) = &f.ret {
        collect_type_refs(ret, interner, out);
    }
    match &f.body {
        FnBody::Expr(e) => collect_expr_refs(e, interner, out),
        FnBody::Block(b) => {
            for s in &b.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
    }
}

fn collect_type_refs(ty: &TypeExpr, interner: &Interner, out: &mut HashSet<String>) {
    match ty {
        TypeExpr::Named(name, _) => {
            out.insert(interner.resolve(*name).to_string());
        }
        TypeExpr::Optional(inner, _)
        | TypeExpr::Error(inner, _)
        | TypeExpr::Chan(inner, _)
        | TypeExpr::Code(inner, _) => collect_type_refs(inner, interner, out),
        TypeExpr::Apply { name, args, .. } => {
            out.insert(interner.resolve(*name).to_string());
            for arg in args {
                collect_type_refs(arg, interner, out);
            }
        }
        TypeExpr::Path(..) | TypeExpr::Type(_) => {}
    }
}

fn collect_stmt_refs(stmt: &Stmt, interner: &Interner, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Binding(b) => collect_expr_refs(&b.value, interner, out),
        Stmt::Destructure { value, .. } => collect_expr_refs(value, interner, out),
        Stmt::Expr(e) | Stmt::Return(Some(e), _) | Stmt::Fail(e, _) => {
            collect_expr_refs(e, interner, out)
        }
        Stmt::Panic(Some(e), _) => collect_expr_refs(e, interner, out),
        Stmt::Panic(None, _) => {}
        Stmt::Return(None, _) => {}
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Ensure {
            cond, else_block, ..
        } => {
            collect_expr_refs(cond, interner, out);
            for s in &else_block.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Stmt::Each { iter, body, .. } => {
            collect_expr_refs(iter, interner, out);
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_refs(cond, interner, out);
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Stmt::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                collect_expr_refs(t, interner, out);
            }
            for v in values {
                collect_expr_refs(v, interner, out);
            }
        }
        Stmt::Guarded { stmt, cond, .. } => {
            collect_expr_refs(cond, interner, out);
            collect_stmt_refs(stmt, interner, out);
        }
        Stmt::When { subjects, arms, .. } => {
            for s in subjects {
                collect_expr_refs(s, interner, out);
            }
            for arm in arms {
                collect_when_pattern_refs(&arm.pattern, interner, out);
                collect_expr_refs(&arm.body, interner, out);
            }
        }
        Stmt::Scope { body, .. } => {
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Stmt::Comptime(cs) => match cs {
            ComptimeStmt::Block(b, _) => {
                for s in &b.stmts {
                    collect_stmt_refs(s, interner, out);
                }
            }
            ComptimeStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                collect_expr_refs(cond, interner, out);
                for s in &then_block.stmts {
                    collect_stmt_refs(s, interner, out);
                }
                if let Some(eb) = else_block {
                    for s in &eb.stmts {
                        collect_stmt_refs(s, interner, out);
                    }
                }
            }
            ComptimeStmt::Each { iter, body, .. } => {
                collect_expr_refs(iter, interner, out);
                for s in &body.stmts {
                    collect_stmt_refs(s, interner, out);
                }
            }
        },
    }
}

fn collect_when_pattern_refs(pat: &WhenPattern, interner: &Interner, out: &mut HashSet<String>) {
    match pat {
        WhenPattern::Expr(p) | WhenPattern::After(p) => collect_expr_refs(p, interner, out),
        WhenPattern::Else | WhenPattern::VariantBind { .. } => {}
        WhenPattern::Tuple { elems, .. } => {
            for e in elems {
                collect_when_pattern_refs(e, interner, out);
            }
        }
    }
}

fn collect_expr_refs(expr: &Expr, interner: &Interner, out: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            out.insert(interner.resolve(*name).to_string());
        }
        Expr::ComptimeRef { name, .. } => {
            let name = interner.resolve(*name);
            out.insert(format!("#{name}"));
            out.insert(name.to_string());
        }
        Expr::Binary { left, right, .. }
        | Expr::Or { left, right, .. }
        | Expr::Assign {
            target: left,
            value: right,
            ..
        } => {
            collect_expr_refs(left, interner, out);
            collect_expr_refs(right, interner, out);
        }
        Expr::Unary { expr: e, .. }
        | Expr::Await(e, _)
        | Expr::Spawn { body: e, .. }
        | Expr::Shared { value: e, .. }
        | Expr::Duration { value: e, .. }
        | Expr::Quote { body: e, .. }
        | Expr::Splice { code: e, .. } => collect_expr_refs(e, interner, out),
        Expr::Call { callee, args, .. } => {
            collect_expr_refs(callee, interner, out);
            for a in args {
                collect_expr_refs(a, interner, out);
            }
        }
        Expr::Pipeline { base, args, .. } => {
            collect_expr_refs(base, interner, out);
            for a in args {
                collect_expr_refs(a, interner, out);
            }
        }
        Expr::Field { base, .. } => collect_expr_refs(base, interner, out),
        Expr::Index { base, index, .. } => {
            collect_expr_refs(base, interner, out);
            collect_expr_refs(index, interner, out);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            collect_expr_refs(base, interner, out);
            collect_expr_refs(start, interner, out);
            collect_expr_refs(end, interner, out);
        }
        Expr::Array(elems, _) => {
            for e in elems {
                collect_expr_refs(e, interner, out);
            }
        }
        Expr::ArrayRepeat { value, len, .. } => {
            collect_expr_refs(value, interner, out);
            collect_expr_refs(len, interner, out);
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr_refs(cond, interner, out);
            collect_expr_refs(then_expr, interner, out);
            collect_expr_refs(else_expr, interner, out);
        }
        Expr::Map(entries, _) => {
            for e in entries {
                if let MapKey::Expr(k) = &e.key {
                    collect_expr_refs(k, interner, out);
                }
                collect_expr_refs(&e.value, interner, out);
            }
        }
        Expr::Record(fields, _)
        | Expr::StructLit { fields, .. }
        | Expr::InferredStruct { fields, .. } => {
            if let Expr::StructLit { name: Some(n), .. } = expr {
                out.insert(interner.resolve(*n).to_string());
            }
            for f in fields {
                collect_expr_refs(&f.value, interner, out);
            }
        }
        Expr::Range { start, end, .. } => {
            collect_expr_refs(start, interner, out);
            collect_expr_refs(end, interner, out);
        }
        Expr::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_expr_refs(cond, interner, out);
            for s in &then_block.stmts {
                collect_stmt_refs(s, interner, out);
            }
            if let Some(eb) = else_block {
                for s in &eb.stmts {
                    collect_stmt_refs(s, interner, out);
                }
            }
        }
        Expr::Block(b) | Expr::ComptimeBlock { body: b, .. } => {
            for s in &b.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Expr::OrErr { left, body, .. } => {
            collect_expr_refs(left, interner, out);
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Expr::Chan { capacity, .. } => collect_expr_refs(capacity, interner, out),
        Expr::NamedVariant { sum, args, .. } => {
            out.insert(interner.resolve(*sum).to_string());
            for a in args {
                collect_expr_refs(a, interner, out);
            }
        }
        Expr::ComptimeIf {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_expr_refs(cond, interner, out);
            for s in &then_block.stmts {
                collect_stmt_refs(s, interner, out);
            }
            if let Some(eb) = else_block {
                for s in &eb.stmts {
                    collect_stmt_refs(s, interner, out);
                }
            }
        }
        Expr::ComptimeEach { iter, body, .. } => {
            collect_expr_refs(iter, interner, out);
            for s in &body.stmts {
                collect_stmt_refs(s, interner, out);
            }
        }
        Expr::ComptimeField { base, field, .. } => {
            collect_expr_refs(base, interner, out);
            collect_expr_refs(field, interner, out);
        }
        Expr::Lambda { body, .. } => match body.as_ref() {
            FnBody::Expr(e) => collect_expr_refs(e, interner, out),
            FnBody::Block(b) => {
                for s in &b.stmts {
                    collect_stmt_refs(s, interner, out);
                }
            }
        },
        Expr::Panic { message, .. } => {
            if let Some(m) = message {
                collect_expr_refs(m, interner, out);
            }
        }
        Expr::MacroCall(call) => {
            out.insert(format!("#{}", interner.resolve(call.name)));
            collect_token_refs(&call.input, interner, out);
        }
        Expr::String(sym, _) => collect_string_hole_refs(interner.resolve(*sym), out),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::None(_)
        | Expr::It(_)
        | Expr::IndexIdent(_)
        | Expr::ErrIdent(_)
        | Expr::Embed { .. }
        | Expr::GoalType(_) => {}
    }
}

/// Names used inside `"...{expr}..."` holes. Interpolation is still a string at
/// this pass, so a private helper called only from a hole (`gui_paint_node(...)`)
/// would otherwise be dropped when the module is imported.
fn collect_string_hole_refs(src: &str, out: &mut HashSet<String>) {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 1;
            if i < b.len() {
                i += 1;
            }
            continue;
        }
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        if i + 1 < b.len() && b[i + 1] == b'{' {
            i += 2;
            continue;
        }
        let start = i + 1;
        let mut depth = 1i32;
        let mut j = start;
        while j < b.len() && depth > 0 {
            if b[j] == b'{' {
                depth += 1;
            } else if b[j] == b'}' {
                depth -= 1;
            }
            if depth > 0 {
                j += 1;
            }
        }
        if depth != 0 {
            break;
        }
        if let Ok(hole) = std::str::from_utf8(&b[start..j]) {
            collect_hole_idents(hole, out);
        }
        i = j + 1;
    }
}

fn collect_hole_idents(hole: &str, out: &mut HashSet<String>) {
    let b = hole.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if let Ok(name) = std::str::from_utf8(&b[s..i]) {
                out.insert(name.to_string());
            }
        } else {
            i += 1;
        }
    }
}

/// Bindings a shim module re-exports: everything its `use` items bring in, transitively.
fn collect_reexported(
    path: &Path,
    reexports: &HashMap<PathBuf, Vec<PathBuf>>,
    loaded: &HashMap<PathBuf, Vec<SymId>>,
    out: &mut Vec<SymId>,
    seen: &mut HashSet<PathBuf>,
) {
    if !seen.insert(path.to_path_buf()) {
        return;
    }
    for child in reexports.get(path).into_iter().flatten() {
        if let Some(bindings) = loaded.get(child) {
            out.extend(bindings.iter().copied());
        }
        collect_reexported(child, reexports, loaded, out, seen);
    }
}

/// Analyze `entry` resolving `use` graph. Std modules require `use std.*`.
pub fn analyze_project(entry: &Path) -> ProjectAnalyze {
    analyze_project_with_paths(entry, &[])
}

/// Analyze `entry` with named package roots used after sibling-module lookup.
///
/// A search path `("router", "/cache/router/src")` resolves `use router` to
/// `src/lib.sr` and `use router.http` to `src/http.sr`.
pub fn analyze_project_with_paths(
    entry: &Path,
    extra_search_paths: &[(String, PathBuf)],
) -> ProjectAnalyze {
    analyze_project_with_paths_and_cfg(entry, extra_search_paths, &CfgContext::host())
}

/// Analyze a project with an explicit target and feature configuration.
pub fn analyze_project_with_paths_and_cfg(
    entry: &Path,
    extra_search_paths: &[(String, PathBuf)],
    cfg: &CfgContext,
) -> ProjectAnalyze {
    let entry = entry.canonicalize().unwrap_or_else(|_| entry.to_path_buf());
    match fs::read_to_string(&entry) {
        Ok(text) => analyze_project_text_with_paths_and_cfg(&entry, text, extra_search_paths, cfg),
        Err(e) => {
            let mut interner = Interner::with_capacity(64);
            let mut diagnostics = Diagnostics::new();
            diagnostics.error(
                super_core::DiagnosticKind::Syntax,
                super_core::Span::default(),
                format!("cannot read {}: {e}", entry.display()),
            );
            let entry_dir = entry
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            let empty_docs = interner.intern("");
            let sema = SemanticAnalyzer::new(&mut interner, &mut diagnostics);
            let typed = sema.analyze(&Module {
                items: vec![],
                span: super_core::Span::default(),
                docs: empty_docs,
                macros: Vec::new(),
            });
            ProjectAnalyze {
                module: Module {
                    items: vec![],
                    span: super_core::Span::default(),
                    docs: empty_docs,
                    macros: Vec::new(),
                },
                typed,
                interner,
                diagnostics,
                tokens: vec![],
                std_imports: HashSet::new(),
                entry_dir,
                meta_cache: MetaCache::default(),
                std_root: find_std_root_for_entry(&entry),
                import_targets: Vec::new(),
                module_docs: HashMap::new(),
                symbol_origins: HashMap::new(),
            }
        }
    }
}

/// Like [`analyze_project`], but uses `text` for the entry file (LSP buffer overlay).
pub fn analyze_project_text(entry: &Path, text: impl Into<String>) -> ProjectAnalyze {
    analyze_project_text_with_paths(entry, text, &[])
}

/// Buffer-overlay variant of [`analyze_project_with_paths`].
pub fn analyze_project_text_with_paths(
    entry: &Path,
    text: impl Into<String>,
    extra_search_paths: &[(String, PathBuf)],
) -> ProjectAnalyze {
    analyze_project_text_with_paths_and_cfg(entry, text, extra_search_paths, &CfgContext::host())
}

pub fn analyze_project_text_with_paths_and_cfg(
    entry: &Path,
    text: impl Into<String>,
    extra_search_paths: &[(String, PathBuf)],
    cfg: &CfgContext,
) -> ProjectAnalyze {
    analyze_project_text_with_overlays(entry, text, extra_search_paths, HashMap::new(), cfg)
}

/// Like [`analyze_project_text_with_paths`], with optional path→buffer overlays
/// (used when a `*.law.sr` / `*.proof.sr` sidecar is open in the LSP).
fn analyze_project_text_with_overlays(
    entry: &Path,
    text: impl Into<String>,
    extra_search_paths: &[(String, PathBuf)],
    mut overlays: HashMap<PathBuf, String>,
    cfg: &CfgContext,
) -> ProjectAnalyze {
    let text = text.into();
    let entry = entry.canonicalize().unwrap_or_else(|_| entry.to_path_buf());

    // Opening a verify sidecar alone must analyze the primary module so `#law`
    // bodies see `Account` / `transfer` / etc. Overlay the sidecar buffer.
    if crate::verify::is_verify_sidecar(&entry)
        && let Some(primary) = crate::verify::resolve_sidecar_primary(&entry)
    {
        let primary = primary.canonicalize().unwrap_or(primary);
        if primary != entry {
            overlays.insert(entry.clone(), text);
            let primary_text = overlays
                .get(&primary)
                .cloned()
                .or_else(|| fs::read_to_string(&primary).ok())
                .unwrap_or_default();
            return analyze_project_text_with_overlays(
                &primary,
                primary_text,
                extra_search_paths,
                overlays,
                cfg,
            );
        }
    }

    let mut interner = Interner::with_capacity(512);
    let mut diagnostics = Diagnostics::new();
    let std_root = find_std_root_for_entry(&entry);
    let entry_dir = entry
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let source = SourceFile::new(entry.display().to_string(), text);
    let (mut module, tokens) = parse_source(&source, &mut interner, &mut diagnostics, 0);
    strip_cfg_items(&mut module, &interner, &mut diagnostics, cfg);
    let mut next_file: u32 = 1;

    let mut std_imports = HashSet::new();
    let mut loaded: HashSet<PathBuf> = HashSet::new();
    let mut loaded_bindings: HashMap<PathBuf, Vec<SymId>> = HashMap::new();
    loaded.insert(entry.clone());
    let mut merged_items: Vec<Item> = Vec::new();
    let mut pending_uses: Vec<(PathBuf, UseItem, bool)> = Vec::new();
    let mut resolved_entry_bindings: HashMap<(u32, u32), Vec<SymId>> = HashMap::new();
    // Pure `use`-only modules (deprecated shims such as `std.sys.epoll`) re-export what
    // they import: shim path -> the module paths it pulls in.
    let mut reexports: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut entry_use_paths: Vec<EntryUsePath> = Vec::new();
    let mut import_targets: Vec<ImportTarget> = Vec::new();
    let mut module_docs: HashMap<PathBuf, SymId> = HashMap::new();
    let mut symbol_origins: HashMap<SymId, SymbolOrigin> = HashMap::new();
    if !interner.resolve(module.docs).is_empty() {
        module_docs.insert(entry.clone(), module.docs);
    }

    // Bend-style companions: `*.law.sr` / `*.proof.sr` beside the entry (and under
    // `laws/` / `src/` when a Pod root is discoverable).
    let pod_root = find_pod_root(&entry_dir);
    for companion in crate::verify::discover_law_proof_files(&entry, pod_root.as_deref()) {
        let companion = companion.canonicalize().unwrap_or(companion);
        if !loaded.insert(companion.clone()) {
            continue;
        }
        let Some(comp_text) = overlay_text(&overlays, &companion)
            .or_else(|| std::fs::read_to_string(&companion).ok())
        else {
            continue;
        };
        let file = next_file;
        next_file = next_file.saturating_add(1);
        let comp_src = SourceFile::new(companion.display().to_string(), comp_text);
        let (mut comp_mod, _) = parse_source(&comp_src, &mut interner, &mut diagnostics, file);
        strip_cfg_items(&mut comp_mod, &interner, &mut diagnostics, cfg);
        if !interner.resolve(comp_mod.docs).is_empty() {
            module_docs.insert(companion.clone(), comp_mod.docs);
        }
        let comp_dir = companion
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        for item in &comp_mod.items {
            if let Item::Use(u) = item {
                pending_uses.push((comp_dir.clone(), u.clone(), false));
            }
        }
        module.items.extend(comp_mod.items);
    }

    // Collect uses from entry first
    for item in &module.items {
        if let Item::Use(u) = item {
            pending_uses.push((entry_dir.clone(), u.clone(), true));
        }
    }

    while let Some((from_dir, use_item, is_entry_use)) = pending_uses.pop() {
        let segs: Vec<String> = use_item
            .path
            .iter()
            .map(|s| interner.resolve(*s).to_string())
            .collect();
        // `use std.data.{column, frame, plan}` → sibling modules when selectors are
        // *loadable* modules under the path prefix. Otherwise keep export-selector
        // semantics (`use models.{User}` / `use std.poll.{POLL_READ}`).
        // Note: `resolve_module_path` may return a virtual `stdlib://…` for any dotted
        // std path via `legacy_rel_path` — require `load_module_text` success.
        // If *any* selector is a loadable sibling module, treat the brace as a
        // multi-module import (error on the rest) instead of falling through to
        // export-select on the parent path.
        if let Some(sels) = &use_item.selectors
            && !sels.is_empty()
            && use_item.alias.is_none()
        {
            let mut child_uses = Vec::with_capacity(sels.len());
            let mut missing: Vec<String> = Vec::new();
            for sel in sels {
                let name = interner.resolve(*sel).to_string();
                let mut child_segs = segs.clone();
                child_segs.push(name.clone());
                let loadable = resolve_module_path(
                    &from_dir,
                    &child_segs,
                    std_root.as_deref(),
                    extra_search_paths,
                )
                .and_then(|p| load_module_text(&p, &child_segs))
                .is_some();
                if loadable {
                    let mut path = use_item.path.clone();
                    path.push(*sel);
                    child_uses.push(UseItem {
                        path,
                        alias: None,
                        selectors: None,
                        renames: Vec::new(),
                        bindings: Vec::new(),
                        span: use_item.span,
                        docs: use_item.docs,
                    });
                } else {
                    missing.push(name);
                }
            }
            if !child_uses.is_empty() {
                for name in &missing {
                    let full = format!("{}.{}", segs.join("."), name);
                    diagnostics.error(
                        super_core::DiagnosticKind::Syntax,
                        use_item.span,
                        format!("cannot resolve module `{full}` in braced multi-import"),
                    );
                }
                // Enqueue loadable children (same entry/dep flag).
                for child in child_uses.into_iter().rev() {
                    pending_uses.push((from_dir.clone(), child, is_entry_use));
                }
                continue;
            }
        }
        if is_entry_use && let Some(alias) = use_item.alias {
            std_imports.insert(interner.resolve(alias).to_string());
        }
        if use_item.alias.is_none() && segs.first().map(|s| s.as_str()) == Some("std") {
            match segs.len() {
                1 => {
                    std_imports.insert("*".into());
                }
                2 => {
                    std_imports.insert(segs[1].clone());
                }
                _ => {
                    if let Some(last) = segs.last() {
                        std_imports.insert(last.clone());
                    }
                }
            }
        }
        let Some(path) =
            resolve_module_path(&from_dir, &segs, std_root.as_deref(), extra_search_paths)
        else {
            diagnostics.error(
                super_core::DiagnosticKind::Syntax,
                use_item.span,
                format!(
                    "cannot resolve module `{}` (use std.* from the Super stdlib, or a sibling .sr file)",
                    segs.join(".")
                ),
            );
            continue;
        };
        if is_entry_use {
            import_targets.push(ImportTarget {
                span: use_item.span,
                path: path.clone(),
            });
            entry_use_paths.push((
                (use_item.span.lo.0, use_item.span.hi.0),
                path.clone(),
                use_item.selectors.clone(),
            ));
        }
        if !loaded.insert(path.clone()) {
            let file = next_file;
            next_file = next_file.saturating_add(1);
            let added = append_missing_exports(
                &path,
                &segs,
                use_item.selectors.as_deref(),
                &mut merged_items,
                &mut interner,
                &mut diagnostics,
                &mut symbol_origins,
                file,
            );
            if let Some(bindings) = loaded_bindings.get_mut(&path) {
                for sym in added {
                    if !bindings.contains(&sym) {
                        bindings.push(sym);
                    }
                }
            }
            if is_entry_use && let Some(bindings) = loaded_bindings.get(&path) {
                let bindings = if let Some(selectors) = &use_item.selectors {
                    bindings
                        .iter()
                        .copied()
                        .filter(|binding| selectors.contains(binding))
                        .collect()
                } else {
                    bindings.clone()
                };
                merge_entry_bindings(
                    &mut resolved_entry_bindings,
                    (use_item.span.lo.0, use_item.span.hi.0),
                    bindings,
                );
            }
            continue;
        }
        let Some(dep_text) = load_module_text(&path, &segs) else {
            diagnostics.error(
                super_core::DiagnosticKind::Syntax,
                use_item.span,
                format!("cannot read {}", path.display()),
            );
            continue;
        };
        let dep_src = SourceFile::new(path.display().to_string(), dep_text);
        let file = next_file;
        next_file = next_file.saturating_add(1);
        let (mut dep_mod, _) = parse_source(&dep_src, &mut interner, &mut diagnostics, file);
        strip_cfg_items(&mut dep_mod, &interner, &mut diagnostics, cfg);
        if !interner.resolve(dep_mod.docs).is_empty() {
            module_docs.insert(path.clone(), dep_mod.docs);
        }
        // Merge `*.law.sr` / `*.proof.sr` companions beside each dependency module.
        for companion in crate::verify::discover_law_proof_files(&path, None) {
            let companion = companion.canonicalize().unwrap_or(companion);
            if !loaded.insert(companion.clone()) {
                continue;
            }
            let Some(comp_text) = overlay_text(&overlays, &companion)
                .or_else(|| std::fs::read_to_string(&companion).ok())
            else {
                continue;
            };
            let cfile = next_file;
            next_file = next_file.saturating_add(1);
            let comp_src = SourceFile::new(companion.display().to_string(), comp_text);
            let (mut comp_mod, _) = parse_source(&comp_src, &mut interner, &mut diagnostics, cfile);
            strip_cfg_items(&mut comp_mod, &interner, &mut diagnostics, cfg);
            dep_mod.items.extend(comp_mod.items);
        }
        let dep_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        for item in &dep_mod.items {
            if let Item::Use(u) = item {
                pending_uses.push((dep_dir.clone(), u.clone(), false));
            }
        }
        if !dep_mod.items.iter().any(is_exported_item) {
            for item in &dep_mod.items {
                let Item::Use(u) = item else { continue };
                let child_segs: Vec<String> = u
                    .path
                    .iter()
                    .map(|s| interner.resolve(*s).to_string())
                    .collect();
                if let Some(child) = resolve_module_path(
                    &dep_dir,
                    &child_segs,
                    std_root.as_deref(),
                    extra_search_paths,
                ) {
                    reexports.entry(path.clone()).or_default().push(child);
                }
            }
        }
        let exports = filter_exports(dep_mod.items, use_item.selectors.as_deref(), &interner);
        let exported_bindings: Vec<SymId> = exports
            .iter()
            .filter(|item| is_exported_item(item))
            .filter_map(|item| match item {
                Item::Binding(binding) => Some(binding.name),
                Item::Struct(def) => Some(def.name),
                Item::Sum(def) => Some(def.name),
                Item::Function(function) => Some(function.name),
                Item::Trait(def) => Some(def.name),
                _ => None,
            })
            .collect();
        loaded_bindings.insert(path.clone(), exported_bindings.clone());
        if is_entry_use {
            merge_entry_bindings(
                &mut resolved_entry_bindings,
                (use_item.span.lo.0, use_item.span.hi.0),
                exported_bindings,
            );
        }
        register_item_origins(&mut symbol_origins, &path, &exports);
        merged_items.extend(exports);
    }

    for (key, path, selectors) in entry_use_paths {
        let mut inherited = Vec::new();
        collect_reexported(
            &path,
            &reexports,
            &loaded_bindings,
            &mut inherited,
            &mut HashSet::new(),
        );
        if inherited.is_empty() {
            continue;
        }
        let slot = resolved_entry_bindings.entry(key).or_default();
        for binding in inherited {
            let wanted = selectors.as_ref().is_none_or(|sel| sel.contains(&binding));
            if wanted && !slot.contains(&binding) {
                slot.push(binding);
            }
        }
    }

    // Entry items last so they can override. Keep Use items so semantic analysis
    // can report unused imports after module resolution.
    let local_item_start = merged_items.len();
    for mut item in module.items.drain(..) {
        if let Item::Use(import) = &mut item
            && let Some(bindings) =
                resolved_entry_bindings.remove(&(import.span.lo.0, import.span.hi.0))
        {
            import.bindings = bindings;
        }
        merged_items.push(item);
    }
    module.items = merged_items;

    crate::macros::expand_module(&mut module, &mut interner, &mut diagnostics);
    let mut meta = MetaEngine::new(&mut interner, &mut diagnostics);
    meta.expand_module(&mut module);
    let meta_cache = meta.cache;
    crate::template::check_module_templates(&module, &interner, &mut diagnostics);

    let mut sema = SemanticAnalyzer::new(&mut interner, &mut diagnostics);
    sema.set_std_imports(std_imports.clone());
    sema.set_local_item_start(local_item_start);
    // A7: release / SUPER_DENY_UNUSED promotes E0303–E0305 to errors.
    if std::env::var_os("SUPER_DENY_UNUSED").is_some() {
        sema.set_unused_as_error(true);
    }
    let typed = sema.analyze(&module);

    ProjectAnalyze {
        module,
        typed,
        interner,
        diagnostics,
        tokens,
        std_imports,
        entry_dir,
        meta_cache,
        std_root,
        import_targets,
        module_docs,
        symbol_origins,
    }
}

/// Convenience: analyze a single in-memory buffer (tests) without file graph.
pub fn analyze_source(source: &SourceFile) -> crate::AnalyzeResult {
    crate::analyze(source)
}

#[cfg(test)]
mod reexport_tests {
    use super::*;

    fn errors_for(text: &str) -> Vec<String> {
        let dir = std::env::temp_dir();
        let analyzed = analyze_project_text(&dir.join("reexport_probe.sr"), text);
        analyzed
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn private_types_travel_with_exported_users() {
        let text = "Span = {\n    lo: i64 = -1\n}\nRet = {\n    v: i64 = 0\n}\nArg = {\n    w: i64 = 0\n}\nBox2 = {\n    inner: Ret\n}\nLonely = {\n    z: i64 = 0\n}\nhelper() => 1\n+mk() => Span.{}\n+ret() : Ret => .{v: 1}\n+arg(a: Arg) : i64 => a.w\n+boxed(b: Box2) : i64 => 0\n";
        let src = SourceFile::new("dep.sr", text.to_string());
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let (dep, _) = parse_source(&src, &mut interner, &mut diags, 0);
        assert!(!diags.has_errors(), "{diags:?}");
        let kept: HashSet<String> = filter_exports(dep.items, None, &interner)
            .iter()
            .filter_map(|it| item_name(it, &interner))
            .collect();
        for name in ["Span", "Ret", "Arg", "Box2", "mk", "ret", "arg", "boxed"] {
            assert!(kept.contains(name), "{name} dropped: {kept:?}");
        }
        assert!(
            !kept.contains("Lonely") && !kept.contains("helper"),
            "{kept:?}"
        );
    }

    #[test]
    fn private_struct_defaults_survive_import() {
        let dir = std::env::temp_dir().join(format!("super_priv_defaults_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dep.sr"),
            "Span = {\n    lo: i64 = -1\n}\n+first_lo() : i64 {\n    s = Span.{}\n    s.lo\n}\n",
        )
        .unwrap();
        let main = dir.join("main.sr");
        let text = "use dep\n\ntest \"t\" {\n    assert_eq(first_lo(), -1)\n}\n";
        let analyzed = analyze_project_text(&main, text);
        let _ = std::fs::remove_dir_all(&dir);
        let errs: Vec<String> = analyzed
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert_eq!(errs, Vec::<String>::new());
        let has_span = analyzed
            .module
            .items
            .iter()
            .any(|it| item_name(it, &analyzed.interner).as_deref() == Some("Span"));
        assert!(
            has_span,
            "private struct `Span` was not merged with its user"
        );
    }

    #[test]
    fn deprecated_io_shims_re_export_new_modules() {
        for (path, usage) in [
            ("std.epoll", "epoll_readable(EPOLL_IN)"),
            ("std.sys.epoll", "epoll_readable(EPOLL_IN)"),
            ("std.io_uring", "IORING_POLL_IN"),
            ("std.sys.io_uring", "IORING_POLL_IN"),
            ("std.poll", "poll_can_read(POLL_READ)"),
            ("std.net.poll", "poll_can_read(POLL_READ)"),
            ("std.poll.{POLL_READ}", "POLL_READ"),
        ] {
            let src = format!("use {path}\n\ntest \"t\" {{\n    assert_eq({usage}, {usage})\n}}\n");
            assert_eq!(errors_for(&src), Vec::<String>::new(), "legacy path {path}");
        }
    }

    #[test]
    fn manifest_catalog_maps_use_lines() {
        let catalog = stdlib_module_catalog();
        assert!(
            catalog.len() >= 60,
            "expected full manifest, got {}",
            catalog.len()
        );
        assert!(catalog.iter().any(|m| m.use_line == "use std.net.http"));
        assert!(catalog.iter().any(|m| m.use_line == "use std.sys.io_uring"));
        assert!(catalog.iter().any(|m| m.use_line == "use std.io.poll"));
        assert_eq!(stdlib_use_line_for_manifest("net.http"), "use std.net.http");
        assert_eq!(stdlib_use_line_for_manifest("game.ecs"), "use std.game.ecs");
        let filtered = stdlib_use_completions(Some("io."));
        assert!(filtered.iter().any(|m| m.use_line == "use std.io.poll"));
        assert!(!filtered.iter().any(|m| m.use_line == "use std.net.http"));
        let httpish = stdlib_use_completions(Some("http"));
        assert!(httpish.iter().any(|m| m.manifest_path == "net.http"));
        let short = stdlib_use_completions(Some("h"));
        assert!(short.iter().any(|m| m.manifest_path == "net.http"));
        let net_segs = stdlib_import_next_segments("net");
        assert!(net_segs.iter().any(|s| s == "http"), "{net_segs:?}");
        assert!(!net_segs.iter().any(|s| s == "App"), "{net_segs:?}");
    }

    #[test]
    fn find_std_root_near_repo_entry() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo = manifest_dir.join("../..");
        let example = repo.join("examples/02_task_api/src/main.sr");
        if example.is_file() {
            let root = find_std_root_for_entry(&example);
            let expect = repo.join("stdlib");
            assert_eq!(
                root.as_deref().and_then(|p| p.canonicalize().ok()),
                expect.canonicalize().ok(),
                "stdlib should resolve from open file path"
            );
        }
    }

    #[test]
    fn super_home_lib_stdlib_is_discovered() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_stdlib = manifest_dir.join("../../stdlib");
        if !repo_stdlib.join("mod.sr").is_file() {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("super-home-test-{}", std::process::id()));
        let lib_std = tmp.join("lib/stdlib");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&lib_std).expect("mkdir SUPER_HOME/lib/stdlib");
        // Minimal marker tree so is_stdlib_root accepts it.
        std::fs::write(lib_std.join("mod.sr"), "// test\n").unwrap();
        let prev_home = std::env::var_os("SUPER_HOME");
        let prev_std = std::env::var_os("SUPER_STD");
        let prev_stdlib = std::env::var_os("SUPER_STDLIB");
        unsafe {
            std::env::set_var("SUPER_HOME", &tmp);
            std::env::remove_var("SUPER_STD");
            std::env::remove_var("SUPER_STDLIB");
        }
        // Bypass project-local cwd discovery by checking the installed helper directly.
        let found = find_installed_std_root();
        assert_eq!(
            found.as_deref().and_then(|p| p.canonicalize().ok()),
            lib_std.canonicalize().ok(),
            "SUPER_HOME/lib/stdlib must win for installed discovery"
        );
        match prev_home {
            Some(v) => unsafe { std::env::set_var("SUPER_HOME", v) },
            None => unsafe { std::env::remove_var("SUPER_HOME") },
        }
        match prev_std {
            Some(v) => unsafe { std::env::set_var("SUPER_STD", v) },
            None => unsafe { std::env::remove_var("SUPER_STD") },
        }
        match prev_stdlib {
            Some(v) => unsafe { std::env::set_var("SUPER_STDLIB", v) },
            None => unsafe { std::env::remove_var("SUPER_STDLIB") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn io_socket_imports_bind_symbols() {
        let src = "use std.io.socket\n\ntest \"t\" { assert_eq(socket_text_len(\"abc\"), 3) }\n";
        assert_eq!(
            errors_for(src),
            Vec::<String>::new(),
            "io.socket imports must expose socket helpers"
        );
    }

    #[test]
    fn braced_sibling_modules_expand_like_separate_uses() {
        // `use std.data.{column, frame, plan}` ≡ three module imports.
        let braced = "use std.data.{column, frame, plan}\n\ntest \"t\" {\n    f = Frame.new()\n    assert_eq(f.height(), 0)\n}\n";
        let separate = "use std.data.column\nuse std.data.frame\nuse std.data.plan\n\ntest \"t\" {\n    f = Frame.new()\n    assert_eq(f.height(), 0)\n}\n";
        assert_eq!(
            errors_for(braced),
            Vec::<String>::new(),
            "braced sibling modules must resolve"
        );
        assert_eq!(
            errors_for(separate),
            Vec::<String>::new(),
            "separate uses (control)"
        );
        let a = analyze_project_text(&std::env::temp_dir().join("brace_mods.sr"), braced);
        let use_item = a
            .module
            .items
            .iter()
            .find_map(|it| match it {
                Item::Use(u) => Some(u),
                _ => None,
            })
            .expect("use item");
        assert!(
            use_item.selectors.as_ref().is_some_and(|s| s.len() == 3),
            "AST keeps braced selectors"
        );
        assert!(
            !use_item.bindings.is_empty(),
            "bindings merged from sibling modules"
        );
    }

    #[test]
    fn braced_export_selectors_still_work() {
        // Export select (not sibling modules): `std.poll.{POLL_READ}` names an export.
        let src =
            "use std.poll.{POLL_READ}\n\ntest \"t\" {\n    assert_eq(POLL_READ, POLL_READ)\n}\n";
        assert_eq!(errors_for(src), Vec::<String>::new());
    }

    #[test]
    fn braced_sibling_rejects_unknown_module_name() {
        let src = "use std.data.{column, no_such_mod, frame}\n\ntest \"t\" { assert_eq(1, 1) }\n";
        let errs = errors_for(src);
        assert!(
            errs.iter()
                .any(|m| { m.contains("no_such_mod") && m.contains("braced multi-import") }),
            "expected braced multi-import resolve error, got {errs:?}"
        );
    }

    #[test]
    fn braced_math_siblings_bind_exports() {
        let src = "use std.math.{core, linalg}\n\ntest \"t\" {\n    assert_eq(1 + 1, 2)\n}\n";
        assert_eq!(errors_for(src), Vec::<String>::new());
    }

    #[test]
    fn braced_core_siblings_bind_str_helpers() {
        let src = concat!(
            "use std.core.{str, collections}\n\n",
            "test \"t\" {\n",
            "    assert_eq(byte_len(\"ab\"), 2)\n",
            "}\n",
        );
        assert_eq!(errors_for(src), Vec::<String>::new());
    }

    #[test]
    fn braced_single_module_selector_expands() {
        // One loadable selector still takes the multi-module path.
        let src = "use std.data.{frame}\n\ntest \"t\" {\n    f = Frame.new()\n    assert_eq(f.height(), 0)\n}\n";
        assert_eq!(errors_for(src), Vec::<String>::new());
    }

    #[test]
    fn braced_alias_skips_sibling_module_expand() {
        // `as` on a braced use disables sibling-module expansion (export-select / alias path).
        let src =
            "use std.poll.{POLL_READ} as PR\n\ntest \"t\" {\n    assert_eq(PR, POLL_READ)\n}\n";
        // Accept either alias-bound PR or an intentional parse/sema error — never a
        // "cannot read stdlib://poll/POLL_READ.sr" sibling-module miss.
        let errs = errors_for(src);
        assert!(
            errs.iter().all(|m| !m.contains("stdlib://poll/POLL_READ")),
            "alias+brace must not treat export as sibling module file: {errs:?}"
        );
    }

    #[test]
    fn new_io_paths_resolve() {
        for (path, usage) in [
            ("std.sys.io_uring", "IORING_POLL_IN"),
            ("std.io.uring", "IORING_POLL_IN"),
            ("std.sys.epoll", "epoll_readable(EPOLL_IN)"),
            ("std.io.epoll", "epoll_readable(EPOLL_IN)"),
            ("std.io.poll", "poll_can_read(POLL_READ)"),
            ("std.io.buf", "BUF_WAIT_ROUNDS"),
            ("std.io.fs", "IO_CHUNK"),
            ("std.io.socket", "socket_text_len(\"a\")"),
            ("std.net.quic", "QUIC_V1"),
            ("std.sec.x509", "DER_SEQUENCE"),
            ("std.game.netcode", "NET_ACK_WINDOW"),
        ] {
            let src = format!("use {path}\n\ntest \"t\" {{\n    assert_eq({usage}, {usage})\n}}\n");
            assert_eq!(errors_for(&src), Vec::<String>::new(), "new path {path}");
        }
    }
}
