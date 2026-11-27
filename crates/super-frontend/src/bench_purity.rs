//! LAW-BENCH-001 — Benchmark purity: no `host` in `benchmarks/**/*.sr`.
//! LAW-BENCH-002 — Host surface purity: no bench-shaped APIs in `super-host` / `c_emit`.
//!
//! Super `#law` bodies constrain *programs* (see `examples/04_law_verify`,
//! `stdlib/data/plan.law.sr`): they `#assert` predicates over parameters and
//! helpers the SMT/comptime/fuzz tiers can discharge. They cannot scan the
//! filesystem, so these are **first-class structural laws** owned by
//! the verify engine — same `LawResult` / E05xx surface as behavioral laws.
//!
//! Discharge (`prove_law_bench_001`):
//! 1. Walk every gated `benchmarks/**/*.sr` (exclude `benchmarks/tests/`)
//! 2. Fail **E0504** on any `use host` / braced host import / `host.` call
//! 3. Require complementary Gate B evidence in `benchmarks/runner.py`
//!    (`LAW_BENCH_001`, `check_algo_bans_host`, call from `verify_super`)
//!
//! Discharge (`prove_law_bench_002`):
//! 1. Walk `crates/super-host/**/*.rs` and `crates/super-runtime/src/c_emit*`
//! 2. Fail **E0505** on bench-shaped symbol names (`polars13`, `http14`,
//!    `raft19`, `*_run` whole-algo fuses, score helpers, …)
//! 3. Require Gate C markers in `runner.py` (`check_host_surface_bans_bench`)
//!
//! Complementary (not substitutes): per-file E0504 from `super check`, and
//! Gate B/C in `runner.py` before timing.

use crate::verify::{Counterexample, DischargeTier, LawResult};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use super_core::{
    Span,
    err::{E0504, E0505},
};

/// Stable law id — also the Super-facing law name in verify reports.
pub const LAW_BENCH_001: &str = "LAW-BENCH-001";
/// Function-style name used in `LawResult.name` (matches `#law` naming).
pub const LAW_BENCH_001_NAME: &str = "law_bench_001_no_host";
pub const LAW_BENCH_001_TITLE: &str = "benchmarks never import or call host";

pub const LAW_BENCH_002: &str = "LAW-BENCH-002";
pub const LAW_BENCH_002_NAME: &str = "law_bench_002_no_bench_host_apis";
pub const LAW_BENCH_002_TITLE: &str =
    "super-host / c_emit must not expose bench-shaped whole-algo or fuse APIs";

/// Markers that must appear in `benchmarks/runner.py` for Gate B to count as
/// live complementary evidence. If an agent deletes the harness check, the
/// structural proof fails even when every `.sr` file is currently clean.
const GATE_B_MARKERS: &[&str] = &[
    "LAW_BENCH_001",
    "check_algo_bans_host",
    "host_err = check_algo_bans_host",
];

/// Gate C — host-surface ban must stay wired in the suite harness.
const GATE_C_MARKERS: &[&str] = &[
    "LAW_BENCH_002",
    "check_host_surface_bans_bench",
    "surface_err = check_host_surface_bans_bench",
];

/// Forbidden substrings that identify bench-cheating host/C APIs.
/// Matched as bare identifiers (alphanumeric/`_` boundaries).
const BENCH_HOST_FORBIDDEN_IDENTS: &[&str] = &[
    "polars13",
    "http14",
    "raft19",
    "hnsw17",
    "sparse12",
    "physics15",
    "core18",
    "lake20",
    "lakehouse20",
    "http_req_scan",
    "filter_join_f64_ge_i64_gt_unique",
    "group_score_i64_f64",
    "f64_nn_floor_scale_sum",
    "polars13_fuse",
    "sys_polars13_run",
    "sys_http14_run",
    "sys_sparse12_run",
    "sys_raft19_run",
    "sys_core18_run",
    "sys_hnsw17_run",
    "sys_lake20_run",
    "sys_physics15_run",
    "sup_polars13_run",
    "sup_http14_run",
    "sup_sparse12_run",
    "sup_raft19_run",
    "sup_core18_run",
    "sup_hnsw17_run",
    "sup_lake20_run",
    "sup_physics15_run",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBanViolation {
    pub law_id: &'static str,
    pub path: Option<PathBuf>,
    pub message: String,
    /// 1-based line in the source text.
    pub line: usize,
    /// 1-based column in the source text.
    pub col: usize,
    pub code: &'static str,
}

impl HostBanViolation {
    pub fn diagnostic_message(&self) -> String {
        let loc = self
            .path
            .as_ref()
            .map(|p| format!("{}:{}:{}", p.display(), self.line, self.col))
            .unwrap_or_else(|| format!("{}:{}", self.line, self.col));
        format!(
            "{} ({}) at {}: {}",
            self.law_id, self.code, loc, self.message
        )
    }
}

/// Evidence collected while discharging LAW-BENCH-001.
#[derive(Clone, Debug, Default)]
pub struct BenchHostBanProof {
    pub files_scanned: usize,
    pub violations: Vec<HostBanViolation>,
    pub gate_b_runner: PathBuf,
    pub gate_b_present: bool,
    pub missing_gate_markers: Vec<String>,
    /// Always `E0504` — the diagnostic this proof cites on cheat.
    pub cited_code: &'static str,
}

impl BenchHostBanProof {
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.gate_b_present
    }
}

/// True when `path` resolves under a `benchmarks/` tree and is a Super source
/// that must obey LAW-BENCH-001. Intentional fail fixtures under
/// `benchmarks/tests/` are excluded.
pub fn path_is_benchmark_source(path: &Path) -> bool {
    let mut under_benchmarks = false;
    let mut under_tests = false;
    for c in path.components() {
        if let Component::Normal(s) = c {
            let s = s.to_string_lossy();
            if s == "benchmarks" {
                under_benchmarks = true;
            }
            if under_benchmarks && s == "tests" {
                under_tests = true;
            }
        }
    }
    if !under_benchmarks || under_tests {
        return false;
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.ends_with(".sr")
}

/// Locate `benchmarks/` from a Pod root, entry file, or cwd walk.
pub fn find_benchmarks_root(start: Option<&Path>) -> Option<PathBuf> {
    let mut cur = start
        .map(|p| {
            if p.is_file() {
                p.parent().unwrap_or(p).to_path_buf()
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    loop {
        let candidate = cur.join("benchmarks");
        if candidate.is_dir() && candidate.join("runner.py").is_file() {
            return Some(candidate);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

/// Strip `//` line comments and `/* */` block comments.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                out.push(' ');
                out.push(' ');
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                out.push(' ');
                out.push(' ');
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                }
                if i + 1 < bytes.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                }
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn line_col_at(src: &str, byte_idx: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in src.char_indices() {
        if i >= byte_idx {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn contains_bare_ident(hay: &str, ident: &str) -> bool {
    let bytes = hay.as_bytes();
    let needle = ident.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && (i + needle.len() >= bytes.len() || !is_ident_byte(bytes[i + needle.len()]))
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Scan Super source text for banned host imports / calls.
pub fn check_source_bans_host(src: &str) -> Result<(), HostBanViolation> {
    let cleaned = strip_comments(src);
    let bytes = cleaned.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'u'
            && cleaned[i..].starts_with("use")
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && i + 3 < n
            && !is_ident_byte(bytes[i + 3])
        {
            let after = i + 3;
            let mut j = after;
            while j < n && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < n && bytes[j] == b'{' {
                let close = cleaned[j..].find('}').map(|k| j + k).unwrap_or(n);
                let inner = &cleaned[j..close];
                if contains_bare_ident(inner, "host") {
                    let (line, col) = line_col_at(src, i);
                    return Err(HostBanViolation {
                        law_id: LAW_BENCH_001,
                        path: None,
                        message: format!(
                            "{LAW_BENCH_001_TITLE}: braced `use` imports `host` (benchmarks must call Super stdlib only)"
                        ),
                        line,
                        col,
                        code: E0504,
                    });
                }
                i = close.saturating_add(1);
                continue;
            }
            if cleaned[j..].starts_with("host") && (j + 4 >= n || !is_ident_byte(bytes[j + 4])) {
                let (line, col) = line_col_at(src, j);
                return Err(HostBanViolation {
                    law_id: LAW_BENCH_001,
                    path: None,
                    message: format!(
                        "{LAW_BENCH_001_TITLE}: `use host` is forbidden in benchmark Super sources"
                    ),
                    line,
                    col,
                    code: E0504,
                });
            }
            i = j;
            continue;
        }

        if bytes[i] == b'h'
            && cleaned[i..].starts_with("host")
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && i + 4 < n
            && !is_ident_byte(bytes[i + 4])
        {
            let mut j = i + 4;
            while j < n && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < n && (bytes[j] == b'.' || bytes[j] == b'[') {
                let (line, col) = line_col_at(src, i);
                return Err(HostBanViolation {
                    law_id: LAW_BENCH_001,
                    path: None,
                    message: format!(
                        "{LAW_BENCH_001_TITLE}: `host.` / host FFI call is forbidden in benchmark Super sources"
                    ),
                    line,
                    col,
                    code: E0504,
                });
            }
        }
        i += 1;
    }
    Ok(())
}

/// Check a benchmark source file path + text. No-op (Ok) when the path is not
/// a gated benchmark source.
pub fn check_benchmark_file(path: &Path, src: &str) -> Result<(), HostBanViolation> {
    if !path_is_benchmark_source(path) {
        return Ok(());
    }
    check_source_bans_host(src).map_err(|mut v| {
        v.path = Some(path.to_path_buf());
        v
    })
}

fn collect_gated_sources(bench_root: &Path) -> Vec<PathBuf> {
    // Everything under `bench_root` except `bench_root/tests/**` — works for the
    // real `…/benchmarks/` tree and for unit-test temp trees that are not
    // literally named `benchmarks`.
    let mut out = Vec::new();
    let mut stack = vec![bench_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                // Skip only the top-level `tests/` fixture tree.
                if p.strip_prefix(bench_root).ok() == Some(Path::new("tests")) {
                    continue;
                }
                stack.push(p);
            } else {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.ends_with(".sr") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn check_gate_b_evidence(runner_py: &Path) -> (bool, Vec<String>) {
    let Ok(text) = std::fs::read_to_string(runner_py) else {
        return (false, vec![format!("cannot read {}", runner_py.display())]);
    };
    let mut missing = Vec::new();
    for marker in GATE_B_MARKERS {
        if !text.contains(marker) {
            missing.push((*marker).to_string());
        }
    }
    (missing.is_empty(), missing)
}

/// Mechanically discharge LAW-BENCH-001 over `bench_root`.
///
/// This is the real proof object: source scan + Gate B evidence, citing E0504.
pub fn prove_law_bench_001(bench_root: &Path) -> (BenchHostBanProof, LawResult) {
    let runner = bench_root.join("runner.py");
    let (gate_b_present, missing_gate_markers) = check_gate_b_evidence(&runner);
    let mut proof = BenchHostBanProof {
        files_scanned: 0,
        violations: Vec::new(),
        gate_b_runner: runner,
        gate_b_present,
        missing_gate_markers,
        cited_code: E0504,
    };

    for path in collect_gated_sources(bench_root) {
        proof.files_scanned += 1;
        let Ok(text) = std::fs::read_to_string(&path) else {
            proof.violations.push(HostBanViolation {
                law_id: LAW_BENCH_001,
                path: Some(path),
                message: "unreadable benchmark source".into(),
                line: 1,
                col: 1,
                code: E0504,
            });
            continue;
        };
        if let Err(mut v) = check_source_bans_host(&text) {
            v.path = Some(path);
            proof.violations.push(v);
        }
    }

    let result = law_result_from_proof(&proof);
    (proof, result)
}

fn law_result_from_proof(proof: &BenchHostBanProof) -> LawResult {
    let body_hash = format!(
        "structural:files={}:gate_b={}:code={}",
        proof.files_scanned, proof.gate_b_present, proof.cited_code
    );
    if !proof.gate_b_present {
        let missing = proof.missing_gate_markers.join(", ");
        return LawResult {
            name: LAW_BENCH_001_NAME.into(),
            description: format!("{LAW_BENCH_001}: {LAW_BENCH_001_TITLE}"),
            span: Span::default(),
            passed: false,
            tier: Some(DischargeTier::Structural),
            message: format!(
                "{LAW_BENCH_001} proof failed (E0504): Gate B evidence missing in {} (need: {missing}). \
                 Complementary harness check was removed — restore `check_algo_bans_host` in verify_super.",
                proof.gate_b_runner.display()
            ),
            counterexample: Some(Counterexample {
                bindings: BTreeMap::from([
                    ("gate".into(), "B".into()),
                    ("runner".into(), proof.gate_b_runner.display().to_string()),
                    ("missing".into(), missing),
                ]),
            }),
            code: Some(E0504),
            body_hash,
        };
    }
    if let Some(v) = proof.violations.first() {
        let path = v
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".into());
        let mut bindings = BTreeMap::from([
            ("path".into(), path.clone()),
            ("line".into(), v.line.to_string()),
            ("col".into(), v.col.to_string()),
            ("files_scanned".into(), proof.files_scanned.to_string()),
        ]);
        bindings.insert("violation".into(), v.message.clone());
        return LawResult {
            name: LAW_BENCH_001_NAME.into(),
            description: format!("{LAW_BENCH_001}: {LAW_BENCH_001_TITLE}"),
            span: Span::default(),
            passed: false,
            tier: Some(DischargeTier::Structural),
            message: format!(
                "{LAW_BENCH_001} falsified (E0504): {path}:{}:{}: {}",
                v.line, v.col, v.message
            ),
            counterexample: Some(Counterexample { bindings }),
            code: Some(E0504),
            body_hash,
        };
    }
    LawResult {
        name: LAW_BENCH_001_NAME.into(),
        description: format!("{LAW_BENCH_001}: {LAW_BENCH_001_TITLE}"),
        span: Span::default(),
        passed: true,
        tier: Some(DischargeTier::Structural),
        message: format!(
            "discharged structurally: scanned {} gated .sr file(s); no host import/call; Gate B evidence present in {}",
            proof.files_scanned,
            proof.gate_b_runner.display()
        ),
        counterexample: None,
        code: None,
        body_hash,
    }
}

/// Evidence for LAW-BENCH-002 (bench-shaped host/C API ban).
#[derive(Clone, Debug, Default)]
pub struct HostSurfaceBanProof {
    pub files_scanned: usize,
    pub violations: Vec<HostBanViolation>,
    pub gate_c_runner: PathBuf,
    pub gate_c_present: bool,
    pub missing_gate_markers: Vec<String>,
    pub cited_code: &'static str,
}

impl HostSurfaceBanProof {
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.gate_c_present
    }
}

fn check_gate_c_evidence(runner_py: &Path) -> (bool, Vec<String>) {
    let Ok(text) = std::fs::read_to_string(runner_py) else {
        return (false, vec![format!("cannot read {}", runner_py.display())]);
    };
    let mut missing = Vec::new();
    for marker in GATE_C_MARKERS {
        if !text.contains(marker) {
            missing.push((*marker).to_string());
        }
    }
    (missing.is_empty(), missing)
}

/// Return the first forbidden bench-shaped identifier stem in `src`, if any.
///
/// Stems match as case-insensitive substrings after comment stripping so that
/// `sys_http14_run` / `SUP_HTTP14_CAP` / `sup_polars13_fuse` all trip the gate.
pub fn find_bench_shaped_host_ident(src: &str) -> Option<(usize, &'static str)> {
    let cleaned = strip_comments(src);
    let lower = cleaned.to_ascii_lowercase();
    for &ident in BENCH_HOST_FORBIDDEN_IDENTS {
        if let Some(idx) = lower.find(ident) {
            return Some((idx, ident));
        }
    }
    // `algo_1N_*` whole-algo style (e.g. algo_14_run).
    let bytes = lower.as_bytes();
    let algo = b"algo_1";
    let mut i = 0;
    while i + algo.len() + 2 <= bytes.len() {
        if &bytes[i..i + algo.len()] == algo
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
            && bytes[i + algo.len()].is_ascii_digit()
            && bytes[i + algo.len() + 1] == b'_'
        {
            return Some((i, "algo_1N_*"));
        }
        i += 1;
    }
    None
}

fn collect_host_surface_sources(workspace: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let roots = [
        workspace.join("crates/super-host"),
        workspace.join("crates/super-runtime/src/c_emit.rs"),
        workspace.join("crates/super-runtime/src/c_emit"),
    ];
    for root in roots {
        if root.is_file() {
            out.push(root);
            continue;
        }
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// Resolve workspace root from a benchmarks/ path or crate manifest dir.
pub fn find_workspace_root(start: Option<&Path>) -> Option<PathBuf> {
    let mut cur = start
        .map(|p| {
            if p.is_file() {
                p.parent().unwrap_or(p).to_path_buf()
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    loop {
        if cur.join("Cargo.toml").is_file()
            && (cur.join("crates/super-host").is_dir() || cur.join("crates/super-runtime").is_dir())
        {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

/// Mechanically discharge LAW-BENCH-002 over the workspace host/C surface.
pub fn prove_law_bench_002(workspace: &Path) -> (HostSurfaceBanProof, LawResult) {
    let bench_root = workspace.join("benchmarks");
    let runner = bench_root.join("runner.py");
    let (gate_c_present, missing_gate_markers) = check_gate_c_evidence(&runner);
    let mut proof = HostSurfaceBanProof {
        files_scanned: 0,
        violations: Vec::new(),
        gate_c_runner: runner,
        gate_c_present,
        missing_gate_markers,
        cited_code: E0505,
    };

    for path in collect_host_surface_sources(workspace) {
        proof.files_scanned += 1;
        let Ok(text) = std::fs::read_to_string(&path) else {
            proof.violations.push(HostBanViolation {
                law_id: LAW_BENCH_002,
                path: Some(path),
                message: "unreadable host-surface source".into(),
                line: 1,
                col: 1,
                code: E0505,
            });
            continue;
        };
        if let Some((byte_idx, ident)) = find_bench_shaped_host_ident(&text) {
            let (line, col) = line_col_at(&text, byte_idx);
            proof.violations.push(HostBanViolation {
                law_id: LAW_BENCH_002,
                path: Some(path),
                message: format!(
                    "{LAW_BENCH_002_TITLE}: forbidden bench-shaped host/C symbol `{ident}`"
                ),
                line,
                col,
                code: E0505,
            });
        }
    }

    let body_hash = format!(
        "law-bench-002:files={}:violations={}:gate_c={}",
        proof.files_scanned,
        proof.violations.len(),
        proof.gate_c_present
    );

    if !proof.gate_c_present {
        return (
            proof.clone(),
            LawResult {
                name: LAW_BENCH_002_NAME.into(),
                description: format!("{LAW_BENCH_002}: {LAW_BENCH_002_TITLE}"),
                span: Span::default(),
                passed: false,
                tier: Some(DischargeTier::Structural),
                message: format!(
                    "{LAW_BENCH_002} Gate C missing in {}: {}",
                    proof.gate_c_runner.display(),
                    proof.missing_gate_markers.join(", ")
                ),
                counterexample: Some(Counterexample {
                    bindings: BTreeMap::from([(
                        "missing".into(),
                        proof.missing_gate_markers.join(","),
                    )]),
                }),
                code: Some(E0505),
                body_hash,
            },
        );
    }

    if let Some(v) = proof.violations.first() {
        let mut bindings = BTreeMap::new();
        if let Some(p) = &v.path {
            bindings.insert("path".into(), p.display().to_string());
        }
        bindings.insert("line".into(), v.line.to_string());
        bindings.insert("col".into(), v.col.to_string());
        bindings.insert("violation".into(), v.message.clone());
        return (
            proof.clone(),
            LawResult {
                name: LAW_BENCH_002_NAME.into(),
                description: format!("{LAW_BENCH_002}: {LAW_BENCH_002_TITLE}"),
                span: Span::default(),
                passed: false,
                tier: Some(DischargeTier::Structural),
                message: v.diagnostic_message(),
                counterexample: Some(Counterexample { bindings }),
                code: Some(E0505),
                body_hash,
            },
        );
    }

    let result = LawResult {
        name: LAW_BENCH_002_NAME.into(),
        description: format!("{LAW_BENCH_002}: {LAW_BENCH_002_TITLE}"),
        span: Span::default(),
        passed: true,
        tier: Some(DischargeTier::Structural),
        message: format!(
            "discharged structurally: scanned {} host/C file(s); no bench-shaped APIs; Gate C evidence present in {}",
            proof.files_scanned,
            proof.gate_c_runner.display()
        ),
        counterexample: None,
        code: None,
        body_hash,
    };
    (proof, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn stamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn clean_super_algo_passes() {
        let src = r#"
use std.math.core
use std.core.collections

+main() {
    xs = [1, 2, 3]
    s = 0
    for xs { s += it }
    print("CHECKSUM={s}")
}
"#;
        assert!(check_source_bans_host(src).is_ok());
    }

    #[test]
    fn host_sys_call_fails() {
        let src = r#"
use host
+main() {
    cs = host.sys_lake20_run()
    print("CHECKSUM={cs}")
}
"#;
        let err = check_source_bans_host(src).unwrap_err();
        assert_eq!(err.law_id, LAW_BENCH_001);
        assert_eq!(err.code, E0504);
    }

    #[test]
    fn host_dot_without_use_fails() {
        let src = r#"
+main() {
    cs = host.sys_sparse12_run()
    print("CHECKSUM={cs}")
}
"#;
        let err = check_source_bans_host(src).unwrap_err();
        assert!(err.message.contains("host."));
    }

    #[test]
    fn braced_use_host_fails() {
        let src = "use { host, math }\n+main() {}\n";
        assert!(check_source_bans_host(src).is_err());
    }

    #[test]
    fn comment_mention_does_not_fail() {
        let src = r#"
// historical note: used to call host.sys_foo_run — forbidden now
use std.math.core
+main() { print("ok") }
"#;
        assert!(check_source_bans_host(src).is_ok());
    }

    #[test]
    fn path_gate_skips_non_benchmarks_and_fixtures() {
        assert!(path_is_benchmark_source(Path::new(
            "/repo/benchmarks/20_lake/algo.sr"
        )));
        assert!(path_is_benchmark_source(Path::new(
            "benchmarks/laws/note.sr"
        )));
        assert!(!path_is_benchmark_source(Path::new(
            "benchmarks/tests/fixtures/bad_host.sr"
        )));
        assert!(!path_is_benchmark_source(Path::new("examples/01/main.sr")));
        assert!(!path_is_benchmark_source(Path::new("std/host.sr")));
    }

    #[test]
    fn check_benchmark_file_respects_path() {
        let dirty = "cs = host.sys_x_run()\n";
        assert!(check_benchmark_file(Path::new("examples/x.sr"), dirty).is_ok());
        assert!(check_benchmark_file(Path::new("benchmarks/01_nbody/algo.sr"), dirty).is_err());
    }

    fn write_min_gate_b(runner: &Path) {
        std::fs::write(
            runner,
            r#"
LAW_BENCH_001 = "LAW-BENCH-001"
LAW_BENCH_002 = "LAW-BENCH-002"
def check_algo_bans_host(src):
    return None
def check_host_surface_bans_bench(workspace_root=None):
    return None
def verify_super(tools, algo_dir):
    text = (algo_dir / "algo.sr").read_text()
    host_err = check_algo_bans_host(text)
    if host_err is not None:
        return False, None, host_err
    return True, "0", "ok"
def run_suite():
    surface_err = check_host_surface_bans_bench()
    if surface_err is not None:
        raise SystemExit(surface_err)
"#,
        )
        .unwrap();
    }

    #[test]
    fn law_bench_001_passes_clean_tree() {
        let root = std::env::temp_dir().join(format!("law-bench-clean-{}", stamp()));
        let algo = root.join("01_demo");
        std::fs::create_dir_all(&algo).unwrap();
        write_min_gate_b(&root.join("runner.py"));
        std::fs::write(
            algo.join("algo.sr"),
            "use std.core.collections\n+main() { print(\"CHECKSUM=1\") }\n",
        )
        .unwrap();
        let (proof, result) = prove_law_bench_001(&root);
        assert!(proof.passed(), "{:?}", proof.violations);
        assert!(result.passed, "{}", result.message);
        assert_eq!(result.tier, Some(DischargeTier::Structural));
        assert_eq!(result.name, LAW_BENCH_001_NAME);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn law_bench_001_fails_on_injected_host_algo() {
        let root = std::env::temp_dir().join(format!("law-bench-cheat-{}", stamp()));
        let algo = root.join("99_cheat");
        std::fs::create_dir_all(&algo).unwrap();
        write_min_gate_b(&root.join("runner.py"));
        std::fs::write(
            algo.join("algo.sr"),
            "use host\n+main() { cs = host.sys_lake20_run(); print(\"CHECKSUM={cs}\") }\n",
        )
        .unwrap();
        // Fixtures under tests/ must NOT by themselves fail the law:
        let fix = root.join("tests/fixtures");
        std::fs::create_dir_all(&fix).unwrap();
        std::fs::write(fix.join("also_bad.sr"), "host.sys_x_run()\n").unwrap();

        let (proof, result) = prove_law_bench_001(&root);
        assert!(!proof.passed());
        assert!(!result.passed);
        assert_eq!(result.code, Some(E0504));
        assert!(
            result.message.contains("host") || result.message.contains("LAW-BENCH-001"),
            "{}",
            result.message
        );
        let cex = result.counterexample.expect("counterexample path");
        assert!(cex.bindings.contains_key("path"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn law_bench_001_fails_when_runner_gate_missing() {
        let root = std::env::temp_dir().join(format!("law-bench-nogate-{}", stamp()));
        let algo = root.join("01_demo");
        std::fs::create_dir_all(&algo).unwrap();
        std::fs::write(root.join("runner.py"), "# emptied Gate B\nprint('hi')\n").unwrap();
        std::fs::write(algo.join("algo.sr"), "+main() { print(\"ok\") }\n").unwrap();
        let (proof, result) = prove_law_bench_001(&root);
        assert!(!proof.gate_b_present);
        assert!(!result.passed);
        assert_eq!(result.code, Some(E0504));
        assert!(result.message.contains("Gate B"), "{}", result.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn law_bench_001_passes_on_repo_benchmarks_if_present() {
        // Integration: if this checkout has benchmarks/, the real tree must pass.
        let Some(bench) = find_benchmarks_root(Some(Path::new(env!("CARGO_MANIFEST_DIR"))))
            .or_else(|| find_benchmarks_root(None))
        else {
            return;
        };
        let (proof, result) = prove_law_bench_001(&bench);
        assert!(
            result.passed,
            "repo benchmarks violated LAW-BENCH-001: {} {:?}",
            result.message, proof.violations
        );
        assert!(proof.files_scanned >= 20, "expected ≥20 algo sources");
    }

    #[test]
    fn find_bench_shaped_detects_http14_and_algo_run() {
        assert_eq!(
            find_bench_shaped_host_ident("fn sys_http14_run() {}").map(|(_, id)| id),
            Some("http14")
        );
        assert_eq!(
            find_bench_shaped_host_ident("call algo_14_fuse()").map(|(_, id)| id),
            Some("algo_1N_*")
        );
        assert!(find_bench_shaped_host_ident("sys_sha256 / sys_i64_gather_dense").is_none());
    }

    #[test]
    fn law_bench_002_passes_on_repo_host_surface_if_present() {
        let Some(ws) = find_workspace_root(Some(Path::new(env!("CARGO_MANIFEST_DIR"))))
            .or_else(|| find_workspace_root(None))
        else {
            return;
        };
        // Ensure Gate C markers exist in the real runner before asserting.
        let runner = ws.join("benchmarks/runner.py");
        if !runner.is_file() {
            return;
        }
        let (proof, result) = prove_law_bench_002(&ws);
        assert!(
            result.passed,
            "repo host surface violated LAW-BENCH-002: {} {:?}",
            result.message, proof.violations
        );
        assert!(proof.files_scanned >= 5, "expected host/c_emit sources");
    }
}
