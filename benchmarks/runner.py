#!/usr/bin/env python3
"""Super 20-algorithm cross-language benchmark harness.

Protocol: each program prints CHECKSUM=<value> (value may be on the next line).
Gate B: if Super .sr verify fails, skip the entire algorithm for all targets.
Gate B also enforces LAW-BENCH-001 (E0504): algo.sr must not `use host` or
call `host.*` — benchmarks express algorithms in Super stdlib only.
Languages: C, Super AOT/JIT, Bun, Kotlin, Lua, Python, Julia, Rust, Zig.
"""
from __future__ import annotations

import json
import math
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import traceback
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Any, Callable, Optional

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
# ---------------------------------------------------------------------------
# Timing protocol (versioned). Bump PROTOCOL_VERSION whenever aggregation,
# warm-up policy, interleave order, or estimator semantics change.
# results.json always records the protocol that produced it.
# ---------------------------------------------------------------------------
PROTOCOL_VERSION = "2.0.0"
# Defaults sized for sub-10ms workloads: 2 warm-ups + 10 timed runs.
# Override: SUPER_BENCH_WARM / SUPER_BENCH_TIMED / SUPER_BENCH_ESTIMATOR,
# or CLI --warm / --timed / --estimator.
WARMUPS = int(os.environ.get("SUPER_BENCH_WARM", "2"))
TIMED_RUNS = int(os.environ.get("SUPER_BENCH_TIMED", "10"))
ESTIMATOR = os.environ.get("SUPER_BENCH_ESTIMATOR", "median")  # median | trimmed_mean
TRIM_FRACTION = float(os.environ.get("SUPER_BENCH_TRIM", "0.10"))
CI_Z = float(os.environ.get("SUPER_BENCH_CI_Z", "1.96"))  # ~95% normal CI on mean/stderr
WALL_PRIMARY = ESTIMATOR if ESTIMATOR in ("median", "trimmed_mean") else "median"
TIMEOUT_S = 900
# JIT / `super run` can hang on large stdlib benches (e.g. 15_physics2d); keep a short
# per-run cap so one stuck VM does not freeze the whole suite for TIMEOUT_S × runs.
JIT_TIMEOUT_S = 45
CHECKSUM_RE = re.compile(r"CHECKSUM=\s*(\S+)", re.MULTILINE)
ELAPSED_RE = re.compile(r"ELAPSED_S=\s*([0-9.eE+-]+)", re.MULTILINE)

# Composite weights (CPU dropped — it duplicated Speed via thru_vs_c):
# Speed 40% · Memory 25% · Reliability 15% · Cycle 10% · LoC 10%.
SCORE_WEIGHTS = {
    "speed": 0.40,
    "memory": 0.25,
    "reliability": 0.15,
    "cycle": 0.10,
    "loc": 0.10,
}
SCORE_CATEGORIES = ("speed", "memory", "reliability", "cycle", "loc")
# Languages whose "compile" step is a no-op (edit → run is the iteration).
INTERPRETED_TARGETS = frozenset({"bun", "lua", "python", "julia", "super_jit"})

ALGOS = [
    "01_nbody",
    "02_mandelbrot_parallel",
    "03_astar_pathfinding",
    "04_json_serde",
    "05_parallel_mapreduce",
    "06_mergesort_radix",
    "07_crc32_lz4_compress",
    "08_ecs_simulation",
    "09_matmul_tiled",
    "10_sha256_merkle_tree",
    "11_symbolic_autodiff_ode",
    "12_sparse_fem_fft",
    "13_polars_data_pipeline",
    "14_http_router_throughput",
    "15_physics2d_broadphase_solver",
    "16_crypto_pq_argon2_suite",
    "17_ai_tensor_hnsw_search",
    "18_stdlib_core_algorithms",
    "19_distributed_raft_wal_kv",
    "20_end_to_end_lakehouse_service",
]

TARGETS = [
    "c",
    "super_aot",
    "super_jit",
    "bun",
    "kotlin",
    "lua",
    "python",
    "julia",
    "rust",
    "zig",
]

# Source file used for LOC per language (Super counted once; JIT/AOT share algo.sr)
LOC_SOURCES = {
    "c": "algo.c",
    "super": "algo.sr",
    "bun": "algo.js",
    "kotlin": "Algo.kt",
    "lua": "algo.lua",
    "python": "algo.py",
    "julia": "algo.jl",
    "rust": "algo.rs",
    "zig": "algo.zig",
}


def count_loc(path: Path) -> dict[str, int]:
    """Physical LOC: total lines, non-blank, non-blank-non-comment (// # --)."""
    if not path.exists():
        return {"total": 0, "non_blank": 0, "code": 0}
    total = non_blank = code = 0
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        total += 1
        line = raw.strip()
        if not line:
            continue
        non_blank += 1
        if line.startswith("//") or line.startswith("#") or line.startswith("--"):
            # keep Super comptime `#NAME =` as code
            if path.suffix == ".sr" and line.startswith("#") and not line.startswith("# "):
                if "=" in line or line.startswith("#for") or line.startswith("#{") or line.startswith("#["):
                    code += 1
                    continue
            continue
        code += 1
    return {"total": total, "non_blank": non_blank, "code": code}


def collect_loc() -> dict[str, Any]:
    per_algo: dict[str, dict[str, dict[str, int]]] = {}
    totals: dict[str, dict[str, int]] = {
        lang: {"total": 0, "non_blank": 0, "code": 0} for lang in LOC_SOURCES
    }
    for algo in ALGOS:
        per_algo[algo] = {}
        for lang, fname in LOC_SOURCES.items():
            loc = count_loc(ROOT / algo / fname)
            per_algo[algo][lang] = loc
            for k in ("total", "non_blank", "code"):
                totals[lang][k] += loc[k]
    grand = {"total": 0, "non_blank": 0, "code": 0}
    for lang in totals:
        for k in grand:
            grand[k] += totals[lang][k]
    return {"per_algo": per_algo, "by_language": totals, "grand": grand}


def which(name: str, extra: Optional[list[str]] = None) -> Optional[str]:
    path = os.environ.get("PATH", "")
    parts = []
    home = Path.home()
    # Prefer the workspace release CLI over a stale `~/.cargo/bin/super` so
    # AOT host symbols (e.g. sys_i64_gather) match the stdlib under edit.
    workspace_release = ROOT.parent / "target" / "release"
    if workspace_release.is_dir():
        parts.append(str(workspace_release))
    parts.append(str(home / ".bun" / "bin"))
    parts.append(str(home / ".cargo" / "bin"))
    parts.append(str(home / ".juliaup" / "bin"))
    # Prefer system Zig (Arch/CachyOS package, currently 0.17.x) over any
    # leftover user-local versioned tarball installs.
    for zig_hint in (
        home / ".local" / "bin",
        Path("/usr/bin"),
    ):
        if zig_hint.is_dir():
            parts.append(str(zig_hint))
    if extra:
        parts.extend(extra)
    parts.append(path)
    os.environ["PATH"] = os.pathsep.join(parts)
    return shutil.which(name)


def run_cmd(
    argv: list[str],
    *,
    cwd: Optional[Path] = None,
    timeout: float = TIMEOUT_S,
    env: Optional[dict[str, str]] = None,
    track_rss: bool = False,
) -> tuple[int, str, str, float]:
    """Run argv. When track_rss=True, also records peak VmHWM into `_LAST_RSS_KB`."""
    global _LAST_RSS_KB
    e = os.environ.copy()
    if env:
        e.update(env)
    t0 = time.perf_counter()
    try:
        if not track_rss:
            p = subprocess.run(
                argv,
                cwd=str(cwd) if cwd else None,
                capture_output=True,
                text=True,
                timeout=timeout,
                env=e,
            )
            dt = time.perf_counter() - t0
            return p.returncode, p.stdout or "", p.stderr or "", dt

        proc = subprocess.Popen(
            argv,
            cwd=str(cwd) if cwd else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=e,
        )
        peak = 0
        status_path = f"/proc/{proc.pid}/status"
        pid = proc.pid
        # Sample VmHWM while the *same* child is alive. Bound the spin so we do
        # not steal a core from the workload; stop immediately on PID reuse.
        spins = 0
        while proc.poll() is None:
            if proc.pid != pid:
                break
            try:
                with open(status_path, encoding="utf-8") as fh:
                    for line in fh:
                        if line.startswith("VmHWM:"):
                            peak = max(peak, int(line.split()[1]))
                            break
            except (FileNotFoundError, ProcessLookupError, ValueError, OSError):
                break
            if time.perf_counter() - t0 > timeout:
                proc.kill()
                break
            spins += 1
            if spins & 63 == 0:
                time.sleep(0)
        try:
            out, err = proc.communicate(timeout=5)
        except Exception:
            out, err = "", ""
        dt = time.perf_counter() - t0
        _LAST_RSS_KB = peak or None
        code = proc.returncode if proc.returncode is not None else 124
        return code, out or "", err or "", dt
    except subprocess.TimeoutExpired as ex:
        dt = time.perf_counter() - t0
        out = (ex.stdout or b"") if isinstance(ex.stdout, (bytes, bytearray)) else (ex.stdout or "")
        err = (ex.stderr or b"") if isinstance(ex.stderr, (bytes, bytearray)) else (ex.stderr or "")
        if isinstance(out, (bytes, bytearray)):
            out = out.decode("utf-8", "replace")
        if isinstance(err, (bytes, bytearray)):
            err = err.decode("utf-8", "replace")
        return 124, out, err + "\nTIMEOUT", dt
    except FileNotFoundError as ex:
        return 127, "", str(ex), 0.0


_LAST_RSS_KB: Optional[int] = None


def parse_checksum(stdout: str) -> Optional[str]:
    # Prefer single-line CHECKSUM=value
    m = CHECKSUM_RE.search(stdout)
    if m:
        return m.group(1).strip()
    # AOT-safe: print("CHECKSUM=") then print(n)
    lines = [ln.strip() for ln in stdout.splitlines() if ln.strip() != ""]
    for i, ln in enumerate(lines):
        if ln == "CHECKSUM=" and i + 1 < len(lines):
            return lines[i + 1].strip()
        if ln.startswith("CHECKSUM=") and len(ln) > len("CHECKSUM="):
            return ln.split("=", 1)[1].strip()
    return None


def parse_elapsed_s(stdout: str) -> Optional[float]:
    """Optional in-process kernel time (Julia prints ELAPSED_S=… after compute)."""
    m = ELAPSED_RE.search(stdout)
    if not m:
        return None
    try:
        v = float(m.group(1))
    except ValueError:
        return None
    return v if v > 0 else None


def trimmed_mean(samples: list[float], fraction: float = TRIM_FRACTION) -> float:
    """Drop top/bottom `fraction` (each side) then arithmetic mean of the rest."""
    if not samples:
        return 0.0
    s = sorted(samples)
    n = len(s)
    if n < 3 or fraction <= 0:
        return float(statistics.fmean(s))
    drop = int(n * fraction)
    if drop * 2 >= n:
        drop = max(0, (n - 1) // 2)
    core = s[drop : n - drop] if drop else s
    return float(statistics.fmean(core)) if core else float(statistics.fmean(s))


def stats(
    samples: list[float],
    *,
    estimator: Optional[str] = None,
) -> dict[str, float]:
    """Aggregate **timed-run** walls only (caller must not pass warmup samples).

    Primary reading is `estimator` (median default, or trimmed_mean). Always
    ships spread: stdev, min/max, stderr, and a normal CI around the mean.
    """
    est = (estimator or ESTIMATOR or "median").strip().lower()
    if not samples:
        return {
            "mean": 0.0,
            "median": 0.0,
            "trimmed_mean": 0.0,
            "primary": 0.0,
            "min": 0.0,
            "max": 0.0,
            "p95": 0.0,
            "p99": 0.0,
            "stddev": 0.0,
            "stderr": 0.0,
            "ci_lo": 0.0,
            "ci_hi": 0.0,
            "n": 0.0,
            "estimator": est,
        }
    s = sorted(float(x) for x in samples)
    n = len(s)
    p95_idx = min(n - 1, max(0, int(math.ceil(0.95 * n) - 1)))
    p99_idx = min(n - 1, max(0, int(math.ceil(0.99 * n) - 1)))
    mean = float(statistics.fmean(s))
    median = float(statistics.median(s))
    tmean = trimmed_mean(s)
    if est == "trimmed_mean":
        primary = tmean
    elif est == "mean":
        primary = mean
    else:
        primary = median
        est = "median"
    stddev = float(statistics.pstdev(s)) if n > 1 else 0.0
    stderr = stddev / math.sqrt(n) if n > 0 else 0.0
    half = CI_Z * stderr
    ci_lo = max(0.0, mean - half)
    ci_hi = mean + half
    # Ranking interval = Protocol 2.0.0 mean CI (1.96σ on the mean). A former
    # ±1σ band around the primary made sub-millisecond crowns (A*, router)
    # report "within noise" even with a clear median lead; the documented
    # within-noise rule is CI overlap, not a full-sample σ halo.
    return {
        "mean": mean,
        "median": median,
        "trimmed_mean": tmean,
        "primary": primary,
        "min": s[0],
        "max": s[-1],
        "p95": s[p95_idx],
        "p99": s[p99_idx],
        "stddev": stddev,
        "stderr": stderr,
        "ci_lo": ci_lo,
        "ci_hi": ci_hi,
        "rank_lo": ci_lo,
        "rank_hi": ci_hi,
        "n": float(n),
        "estimator": est,
    }


def wall_primary(st: Optional[dict[str, float]]) -> Optional[float]:
    """Primary wall reading from a target's stats (estimator → median → mean)."""
    if not st:
        return None
    for key in ("primary", WALL_PRIMARY, "median", "trimmed_mean", "mean"):
        v = st.get(key)
        if v is not None and float(v) > 0:
            return float(v)
    return None


def ci_interval(st: Optional[dict[str, float]]) -> Optional[tuple[float, float]]:
    """Return ranking interval (rank_lo/hi, else ci_lo/hi) when present."""
    if not st:
        return None
    for lo_k, hi_k in (("rank_lo", "rank_hi"), ("ci_lo", "ci_hi")):
        lo, hi = st.get(lo_k), st.get(hi_k)
        if lo is not None and hi is not None:
            return float(lo), float(hi)
    return None


def intervals_overlap(
    a: Optional[dict[str, float]],
    b: Optional[dict[str, float]],
) -> bool:
    """True when two languages' ranking intervals overlap (within noise)."""
    ia, ib = ci_interval(a), ci_interval(b)
    if not ia or not ib:
        return False
    return ia[0] <= ib[1] and ib[0] <= ia[1]


def ranking_within_noise(
    stats_by_target: dict[str, dict[str, float]],
) -> list[dict[str, Any]]:
    """Sort by primary wall; flag pairs whose CIs overlap as within-noise."""
    rows = []
    for t, st in stats_by_target.items():
        wp = wall_primary(st)
        if wp is None:
            continue
        rows.append({"target": t, "primary": wp, "stats": st})
    rows.sort(key=lambda r: r["primary"])
    out: list[dict[str, Any]] = []
    for i, row in enumerate(rows):
        noisy: list[str] = []
        for j, other in enumerate(rows):
            if i == j:
                continue
            if intervals_overlap(row["stats"], other["stats"]):
                noisy.append(other["target"])
        out.append(
            {
                "rank": i + 1,
                "target": row["target"],
                "primary": row["primary"],
                "ci_lo": row["stats"].get("ci_lo"),
                "ci_hi": row["stats"].get("ci_hi"),
                "stddev": row["stats"].get("stddev"),
                "within_noise_of": noisy,
                "hard_order": len(noisy) == 0,
            }
        )
    return out


def timed_samples_from_raw(raw_runs: list[dict[str, Any]]) -> list[float]:
    """Extract timed (non-warmup) walls from a raw_runs list."""
    out: list[float] = []
    for r in raw_runs:
        if r.get("warmup"):
            continue
        t = r.get("time_s")
        if isinstance(t, (int, float)) and float(t) > 0:
            out.append(float(t))
    return out


def reaggregate_raw_runs(
    raw_runs: list[dict[str, Any]],
    *,
    estimator: Optional[str] = None,
) -> dict[str, float]:
    """Recompute aggregates from stored raw runs (warmups ignored)."""
    return stats(timed_samples_from_raw(raw_runs), estimator=estimator)


def interleave_schedule(
    targets: list[str],
    *,
    warmups: Optional[int] = None,
    timed: Optional[int] = None,
    extra_warm: Optional[dict[str, int]] = None,
) -> list[tuple[str, str, int]]:
    """Round-robin (target, phase, local_index) across languages.

    Warm-ups first (still interleaved), then timed runs. Extra warm-ups
    (e.g. Kotlin JVM) only schedule that language for those slots.
    """
    warm = WARMUPS if warmups is None else int(warmups)
    n_timed = TIMED_RUNS if timed is None else int(timed)
    extra = extra_warm or {}
    sched: list[tuple[str, str, int]] = []
    max_warm = 0
    for t in targets:
        max_warm = max(max_warm, warm + int(extra.get(t, 0)))
    for i in range(max_warm):
        for t in targets:
            if i < warm + int(extra.get(t, 0)):
                sched.append((t, "warmup", i))
    for i in range(n_timed):
        for t in targets:
            sched.append((t, "timed", i))
    return sched


def protocol_block() -> dict[str, Any]:
    """Frozen protocol descriptor written into every results.json."""
    return {
        "version": PROTOCOL_VERSION,
        "warmups": WARMUPS,
        "timed_runs": TIMED_RUNS,
        "estimator": ESTIMATOR,
        "wall_primary": WALL_PRIMARY,
        "trim_fraction": TRIM_FRACTION,
        "ci_z": CI_Z,
        "interleave": "round_robin_across_targets",
        "timeout_s": TIMEOUT_S,
        "gate": "B_super_sr_gates_entire_algo",
        "timing": (
            f"protocol {PROTOCOL_VERSION}: {WARMUPS} warmup(s) marked+discarded from "
            f"aggregation, then {TIMED_RUNS} timed runs interleaved round-robin across "
            f"languages; primary wall = {WALL_PRIMARY}; spread = stdev/min/max + "
            f"{CI_Z:.2f}σ CI on mean"
        ),
        "within_noise": (
            "When two languages' CI intervals overlap on an algorithm, the ranking "
            "is reported as within noise (no hard order)."
        ),
    }


@dataclass
class Toolchain:
    name: str
    path: Optional[str]
    version: str
    available: bool


@dataclass
class TargetResult:
    target: str
    status: str  # ok | failed | skipped
    checksum: Optional[str] = None
    detail: str = ""
    times_s: list[float] = field(default_factory=list)  # timed only (compat)
    raw_runs: list[dict[str, Any]] = field(default_factory=list)  # all runs, warmup marked
    stats: dict[str, float] = field(default_factory=dict)
    speedup_vs_c: Optional[float] = None
    peak_rss_kb: Optional[int] = None
    compile_s: Optional[float] = None
    cycle_s: Optional[float] = None
    ranking_note: str = ""  # e.g. within-noise vs peers


def detect_toolchains() -> dict[str, Toolchain]:
    tools: dict[str, Toolchain] = {}

    def ver(cmd: list[str]) -> str:
        code, out, err, _ = run_cmd(cmd, timeout=30)
        text = (out or err or "").strip().splitlines()
        return text[0] if text else ("ok" if code == 0 else "unknown")

    cc = which("cc") or which("gcc") or which("clang")
    tools["cc"] = Toolchain("cc", cc, ver([cc, "--version"]) if cc else "", bool(cc))

    super_bin = which("super")
    tools["super"] = Toolchain(
        "super", super_bin, ver([super_bin, "--version"]) if super_bin else "", bool(super_bin)
    )

    bun = which("bun")
    tools["bun"] = Toolchain("bun", bun, ver([bun, "--version"]) if bun else "", bool(bun))

    kotlinc = which("kotlinc")
    java = which("java")
    kv = ""
    if kotlinc:
        kv = ver([kotlinc, "-version"])
    if java:
        kv = (kv + " / " if kv else "") + ver([java, "-version"])
    tools["kotlin"] = Toolchain("kotlin", kotlinc, kv, bool(kotlinc and java))
    tools["java"] = Toolchain("java", java, ver([java, "-version"]) if java else "", bool(java))

    luajit = which("luajit")
    lua = which("lua") or which("lua5.4")
    if luajit:
        tools["lua"] = Toolchain("luajit", luajit, ver([luajit, "-v"]), True)
    elif lua:
        tools["lua"] = Toolchain("lua", lua, ver([lua, "-v"]), True)
    else:
        tools["lua"] = Toolchain("lua", None, "", False)

    py = which("python3")
    pypy = which("pypy3")
    label = ver([py, "--version"]) if py else ""
    if pypy:
        label += f" (pypy3 present: {ver([pypy, '--version'])})"
    tools["python"] = Toolchain("python3", py, label, bool(py))

    julia = which("julia")
    tools["julia"] = Toolchain("julia", julia, ver([julia, "--version"]) if julia else "", bool(julia))

    rustc = which("rustc")
    tools["rust"] = Toolchain(
        "rustc", rustc, ver([rustc, "--version"]) if rustc else "", bool(rustc)
    )

    zig = which("zig")
    tools["zig"] = Toolchain(
        "zig", zig, ver([zig, "version"]) if zig else "", bool(zig)
    )
    return tools


def machine_info() -> dict[str, Any]:
    mem = ""
    try:
        code, out, _, _ = run_cmd(["free", "-h"], timeout=5)
        if code == 0:
            mem = out.strip().splitlines()[1] if out.strip() else ""
    except Exception:
        pass
    nproc = os.cpu_count() or 1
    return {
        "os": platform.platform(),
        "kernel": platform.release(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "python": sys.version,
        "cpus": nproc,
        "memory": mem,
        "hostname": platform.node(),
    }


class AlgoBuild:
    def __init__(self, algo_dir: Path, tools: dict[str, Toolchain], work: Path):
        self.algo_dir = algo_dir
        self.tools = tools
        self.work = work
        self.work.mkdir(parents=True, exist_ok=True)
        self.c_bin: Optional[Path] = None
        self.aot_bin: Optional[Path] = None
        self.kt_jar: Optional[Path] = None
        self.rust_bin: Optional[Path] = None
        self.zig_bin: Optional[Path] = None
        self.compile_s: dict[str, float] = {}

    def compile_c(self) -> tuple[bool, str]:
        src = self.algo_dir / "algo.c"
        if not src.exists():
            return False, "algo.c missing"
        out = self.work / "algo_c"
        cc = self.tools["cc"].path
        if not cc:
            return False, "cc not installed"
        for flags in (
            [cc, "-O3", "-march=native", "-fopenmp", "-o", str(out), str(src), "-lm"],
            [cc, "-O3", "-march=native", "-o", str(out), str(src), "-lm"],
        ):
            code, _, err, dt = run_cmd(flags, timeout=120)
            if code == 0 and out.exists():
                self.c_bin = out
                self.compile_s["c"] = float(dt)
                return True, " ".join(flags)
        return False, err[-2000:]

    def compile_super_aot(self) -> tuple[bool, str]:
        src = self.algo_dir / "algo.sr"
        if not src.exists():
            return False, "algo.sr missing"
        super_bin = self.tools["super"].path
        if not super_bin:
            return False, "super not installed"
        out = self.work / "algo_aot"
        # Peak AOT: release profile (−O3, fat LTO, −march=native, −ffast-math, unroll).
        argv = [
            super_bin,
            "build",
            "--c",
            "--release",
            "--out",
            str(out),
            str(src),
        ]
        code, out_s, err, dt = run_cmd(argv, timeout=300)
        if code == 0 and out.exists():
            self.aot_bin = out
            self.compile_s["super_aot"] = float(dt)
            return True, out_s + err
        # Sometimes binary name differs
        candidates = list(self.work.glob("algo_aot*")) + list(self.algo_dir.glob("algo"))
        for c in candidates:
            if c.is_file() and os.access(c, os.X_OK):
                self.aot_bin = c
                self.compile_s["super_aot"] = float(dt)
                return True, out_s + err
        return False, (err or out_s)[-2000:]

    def compile_super_jit(self) -> tuple[bool, str]:
        """Warm the Super JIT native cache before interleaved timing.

        Protocol 2.0.0 lists Super JIT with interpreters: compile cost is 0, so the
        edit→verify Cycle score must not fold a cold `cc` into `first_wall`. One
        discarded `run --jit` populates `~/.cache/super/jit/<hash>`; timed slots
        then measure warm native dispatch (honest Speed + Cycle split).
        """
        src = self.algo_dir / "algo.sr"
        if not src.exists():
            return False, "algo.sr missing"
        super_bin = self.tools["super"].path
        if not super_bin:
            return False, "super not installed"
        code, out_s, err, dt = run_cmd(
            [super_bin, "run", "--jit", str(src)],
            timeout=JIT_TIMEOUT_S,
        )
        # Compile phase is free for JIT; record wall only for logs / debugging.
        self.compile_s["super_jit"] = 0.0
        if code == 0 and parse_checksum(out_s) is not None:
            return True, f"jit-cache-warm dt={dt:.3f}s"
        return False, (err or out_s)[-2000:]

    def compile_kotlin(self) -> tuple[bool, str]:
        src = self.algo_dir / "Algo.kt"
        if not src.exists():
            return False, "Algo.kt missing"
        kotlinc = self.tools["kotlin"].path
        if not kotlinc or not self.tools["java"].available:
            return False, "kotlin/java not installed"
        jar = self.work / "algo.jar"
        code, out, err, dt = run_cmd(
            [kotlinc, str(src), "-include-runtime", "-d", str(jar)],
            timeout=180,
        )
        if code == 0 and jar.exists():
            self.kt_jar = jar
            self.compile_s["kotlin"] = float(dt)
            return True, out + err
        return False, (err or out)[-2000:]

    def compile_rust(self) -> tuple[bool, str]:
        src = self.algo_dir / "algo.rs"
        if not src.exists():
            return False, "algo.rs missing"
        rustc = self.tools["rust"].path
        if not rustc:
            return False, "rustc not installed"
        out = self.work / "algo_rs"
        argv = [
            rustc,
            "-C",
            "opt-level=3",
            "-C",
            "target-cpu=native",
            "-C",
            "lto=fat",
            str(src),
            "-o",
            str(out),
        ]
        code, out_s, err, dt = run_cmd(argv, timeout=180)
        if code == 0 and out.exists():
            self.rust_bin = out
            self.compile_s["rust"] = float(dt)
            return True, out_s + err
        return False, (err or out_s)[-2000:]

    def compile_zig(self) -> tuple[bool, str]:
        src = self.algo_dir / "algo.zig"
        if not src.exists():
            return False, "algo.zig missing"
        zig = self.tools["zig"].path
        if not zig:
            return False, "zig not installed"
        out = self.work / "algo_zig"
        argv = [
            zig,
            "build-exe",
            "-OReleaseFast",
            "-mcpu=native",
            str(src),
            f"-femit-bin={out}",
        ]
        code, out_s, err, dt = run_cmd(argv, timeout=180, cwd=self.work)
        if code == 0 and out.exists():
            self.zig_bin = out
            self.compile_s["zig"] = float(dt)
            return True, out_s + err
        # Zig may emit beside cwd with a different name
        for c in list(self.work.glob("algo_zig*")) + list(self.work.glob("algo")):
            if c.is_file() and os.access(c, os.X_OK) and c.suffix != ".o":
                self.zig_bin = c
                self.compile_s["zig"] = float(dt)
                return True, out_s + err
        return False, (err or out_s)[-2000:]


def _run_tracked(argv: list[str], **kwargs: Any) -> tuple[int, str, str, float]:
    """Runtime invocations track peak RSS for the composite memory score."""
    return run_cmd(argv, track_rss=True, **kwargs)


def runner_for(
    target: str, build: AlgoBuild, tools: dict[str, Toolchain]
) -> Optional[Callable[[], tuple[int, str, str, float]]]:
    algo = build.algo_dir

    if target == "c":
        if not build.c_bin:
            return None

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([str(build.c_bin)], timeout=TIMEOUT_S)

        return run

    if target == "super_jit":
        super_bin = tools["super"].path
        if not super_bin:
            return None
        src = algo / "algo.sr"

        def run() -> tuple[int, str, str, float]:
            # Prefer --jit but fall back if it errors; checksum verify catches miscompiles.
            # Short timeout: AOT is the scored Super path; a hung JIT must not stall the suite.
            code, out, err, dt = _run_tracked(
                [super_bin, "run", "--jit", str(src)], timeout=JIT_TIMEOUT_S
            )
            if code != 0 or parse_checksum(out) is None:
                return _run_tracked(
                    [super_bin, "run", str(src)], timeout=JIT_TIMEOUT_S
                )
            return code, out, err, dt

        return run

    if target == "super_aot":
        if not build.aot_bin:
            return None

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([str(build.aot_bin)], timeout=TIMEOUT_S)

        return run

    if target == "bun":
        bun = tools["bun"].path
        if not bun:
            return None
        src = algo / "algo.js"

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([bun, "run", str(src)], timeout=TIMEOUT_S)

        return run

    if target == "kotlin":
        java = tools["java"].path
        if not java or not build.kt_jar:
            return None

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([java, "-jar", str(build.kt_jar)], timeout=TIMEOUT_S)

        return run

    if target == "lua":
        lua = tools["lua"].path
        if not lua:
            return None
        src = algo / "algo.lua"

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([lua, str(src)], timeout=TIMEOUT_S)

        return run

    if target == "python":
        py = tools["python"].path
        if not py:
            return None
        src = algo / "algo.py"

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([py, str(src)], timeout=TIMEOUT_S)

        return run

    if target == "julia":
        julia = tools["julia"].path
        if not julia:
            return None
        src = algo / "algo.jl"

        # Timed metric uses in-process ELAPSED_S from algo.jl (startup excluded).
        # SUPER_BENCH_JULIA_WARM overrides in-process warmups (default 2).
        def run() -> tuple[int, str, str, float]:
            env = {"JULIA_NUM_THREADS": "auto"}
            warm = os.environ.get("SUPER_BENCH_JULIA_WARM", "2")
            env["JULIA_BENCH_WARM"] = warm
            return _run_tracked(
                [
                    julia,
                    "-O3",
                    "--check-bounds=no",
                    "--startup-file=no",
                    "--history-file=no",
                    "-t",
                    "auto",
                    str(src),
                ],
                timeout=TIMEOUT_S,
                env=env,
            )

        return run

    if target == "rust":
        if not build.rust_bin:
            return None

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([str(build.rust_bin)], timeout=TIMEOUT_S)

        return run

    if target == "zig":
        if not build.zig_bin:
            return None

        def run() -> tuple[int, str, str, float]:
            return _run_tracked([str(build.zig_bin)], timeout=TIMEOUT_S)

        return run

    return None


def cycle_cost_s(
    target: str,
    compile_s: Optional[float],
    one_run_s: Optional[float],
) -> Optional[float]:
    """Agent edit→verify cost: compile (if any) + one checksummed run.

    Interpreters and Super JIT treat compile as 0; AOT/native add measured
    compile wall. Missing one_run yields None (scored neutral later).
    """
    if one_run_s is None or one_run_s <= 0:
        return None
    build = 0.0 if target in INTERPRETED_TARGETS else float(compile_s or 0.0)
    return build + float(one_run_s)


def _sample_wall(out: str, err: str, dt: float) -> tuple[float, Optional[float]]:
    """Choose reported wall; prefer Julia ELAPSED_S floored to 5% process wall."""
    elapsed = parse_elapsed_s(out) or parse_elapsed_s(err)
    if elapsed is not None:
        return max(elapsed, 0.05 * dt), elapsed
    return float(dt), None


def time_target(
    target: str,
    run: Callable[[], tuple[int, str, str, float]],
    expected: Optional[str],
    *,
    kotlin_extra_warm: bool = False,
    compile_s: Optional[float] = None,
) -> TargetResult:
    """Time one target (block schedule). Prefer ``time_targets_round_robin`` for suites."""
    return time_targets_round_robin(
        {target: run},
        expected,
        compile_s={target: compile_s},
        extra_warm={target: 1} if kotlin_extra_warm else None,
    )[target]


def time_targets_round_robin(
    runners: dict[str, Callable[[], tuple[int, str, str, float]]],
    expected: Optional[str],
    *,
    compile_s: Optional[dict[str, Optional[float]]] = None,
    extra_warm: Optional[dict[str, int]] = None,
) -> dict[str, TargetResult]:
    """Interleave warm-up + timed runs across languages (round-robin).

    Every raw sample is kept (warmup marked). Aggregation uses timed samples
    only with the configured estimator (median / trimmed_mean).
    """
    global _LAST_RSS_KB
    targets = [t for t in TARGETS if t in runners] or list(runners.keys())
    compile_s = compile_s or {}
    extra = dict(extra_warm or {})
    state: dict[str, dict[str, Any]] = {
        t: {
            "raw": [],
            "times": [],
            "last_cs": None,
            "first_wall": None,
            "peak_rss": None,
            "failed": None,  # TargetResult on failure
        }
        for t in targets
    }
    schedule = interleave_schedule(targets, extra_warm=extra)
    print(
        f"  interleave protocol={PROTOCOL_VERSION} warm={WARMUPS} timed={TIMED_RUNS} "
        f"estimator={ESTIMATOR} slots={len(schedule)} targets={len(targets)}",
        flush=True,
    )
    for target, phase, local_i in schedule:
        if state[target]["failed"] is not None:
            continue
        run = runners[target]
        _LAST_RSS_KB = None
        code, out, err, dt = run()
        wall, elapsed = _sample_wall(out, err, dt)
        if state[target]["first_wall"] is None:
            state[target]["first_wall"] = float(dt)
        if _LAST_RSS_KB:
            state[target]["peak_rss"] = max(state[target]["peak_rss"] or 0, _LAST_RSS_KB)
        cs = parse_checksum(out) or parse_checksum(err)
        is_warm = phase == "warmup"
        raw = {
            "index": local_i,
            "phase": phase,
            "warmup": is_warm,
            "time_s": wall,
            "wall_s": float(dt),
            "elapsed_s": elapsed,
            "checksum": cs,
            "ok": code == 0 and cs is not None and (expected is None or cs == expected),
        }
        state[target]["raw"].append(raw)
        if code != 0 or cs is None:
            state[target]["failed"] = TargetResult(
                target=target,
                status="failed",
                detail=f"{phase}#{local_i} exit={code} cs={cs} err={err[-1500:]} out={out[-500:]}",
                times_s=list(state[target]["times"]),
                raw_runs=list(state[target]["raw"]),
                peak_rss_kb=state[target]["peak_rss"],
                compile_s=compile_s.get(target),
                cycle_s=cycle_cost_s(target, compile_s.get(target), state[target]["first_wall"]),
            )
            continue
        if expected is not None and cs != expected:
            state[target]["failed"] = TargetResult(
                target=target,
                status="failed",
                checksum=cs,
                detail=f"checksum mismatch: got {cs} expected {expected}",
                times_s=list(state[target]["times"]),
                raw_runs=list(state[target]["raw"]),
                peak_rss_kb=state[target]["peak_rss"],
                compile_s=compile_s.get(target),
                cycle_s=cycle_cost_s(target, compile_s.get(target), state[target]["first_wall"]),
            )
            continue
        state[target]["last_cs"] = cs
        if not is_warm:
            state[target]["times"].append(wall)

    results: dict[str, TargetResult] = {}
    ok_stats: dict[str, dict[str, float]] = {}
    for t in targets:
        st = state[t]
        if st["failed"] is not None:
            results[t] = st["failed"]
            continue
        times: list[float] = st["times"]
        agg = stats(times)
        ok_stats[t] = agg
        one_run = st["first_wall"] if st["first_wall"] is not None else (times[0] if times else None)
        results[t] = TargetResult(
            target=t,
            status="ok",
            checksum=st["last_cs"],
            detail="",
            times_s=times,
            raw_runs=list(st["raw"]),
            stats=agg,
            peak_rss_kb=st["peak_rss"],
            compile_s=compile_s.get(t),
            cycle_s=cycle_cost_s(t, compile_s.get(t), one_run),
        )
    ranks = ranking_within_noise(ok_stats)
    by_t = {r["target"]: r for r in ranks}
    for t, r in results.items():
        note = by_t.get(t)
        if not note:
            continue
        if note["within_noise_of"]:
            peers = ", ".join(note["within_noise_of"])
            r.ranking_note = f"within noise of: {peers}"
        else:
            r.ranking_note = "hard order (CI does not overlap peers)"
    return results


# LAW-BENCH-001 — mirror of crates/super-frontend/src/bench_purity.rs (E0504).
LAW_BENCH_001 = "LAW-BENCH-001"
# LAW-BENCH-002 — no bench-shaped whole-algo / fuse APIs in super-host / c_emit (E0505).
LAW_BENCH_002 = "LAW-BENCH-002"
_HOST_USE_RE = re.compile(
    r"(?m)(?:^|[^A-Za-z0-9_])use\s+(?:host\b|\{[^}]*\bhost\b)"
)
_HOST_CALL_RE = re.compile(r"(?m)(?:^|[^A-Za-z0-9_])host\s*[.\[]")
_BENCH_HOST_IDENTS = (
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
)
_BENCH_HOST_IDENT_RE = re.compile(
    r"(?i)("
    + "|".join(re.escape(x) for x in _BENCH_HOST_IDENTS)
    + r"|algo_1\d_\w+)"
)


def _strip_sr_comments(src: str) -> str:
    """Blank out // and /* */ comments so comment text cannot trip the ban."""
    out: list[str] = []
    i = 0
    n = len(src)
    while i < n:
        if src[i] == "/" and i + 1 < n and src[i + 1] == "/":
            out.append("  ")
            i += 2
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
            continue
        if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
            out.append("  ")
            i += 2
            while i + 1 < n and not (src[i] == "*" and src[i + 1] == "/"):
                out.append("\n" if src[i] == "\n" else " ")
                i += 1
            if i + 1 < n:
                out.append("  ")
                i += 2
            continue
        out.append(src[i])
        i += 1
    return "".join(out)


def check_algo_bans_host(src: str) -> Optional[str]:
    """Return an error detail if `src` violates LAW-BENCH-001, else None."""
    cleaned = _strip_sr_comments(src)
    if _HOST_USE_RE.search(cleaned):
        return (
            f"{LAW_BENCH_001} (E0504): `use host` / braced host import forbidden in "
            "benchmark algo.sr — express the algorithm in Super stdlib only"
        )
    if _HOST_CALL_RE.search(cleaned):
        return (
            f"{LAW_BENCH_001} (E0504): `host.` / host FFI call forbidden in "
            "benchmark algo.sr — no whole-algo or primitive host shims from benchmarks/"
        )
    return None


def check_host_surface_bans_bench(workspace_root: Optional[Path] = None) -> Optional[str]:
    """LAW-BENCH-002 / E0505 — forbid bench-shaped APIs in super-host / c_emit."""
    root = workspace_root or ROOT.parent
    scan_roots = [
        root / "crates" / "super-host",
        root / "crates" / "super-runtime" / "src" / "c_emit.rs",
        root / "crates" / "super-runtime" / "src" / "c_emit",
    ]
    files: list[Path] = []
    for p in scan_roots:
        if p.is_file():
            files.append(p)
        elif p.is_dir():
            files.extend(sorted(p.rglob("*.rs")))
    for path in files:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as e:
            return f"{LAW_BENCH_002} (E0505): cannot read {path}: {e}"
        # Strip // and /* */ so prose comments cannot trip the ban.
        cleaned = _strip_sr_comments(text)
        m = _BENCH_HOST_IDENT_RE.search(cleaned)
        if m:
            return (
                f"{LAW_BENCH_002} (E0505): bench-shaped host/C symbol "
                f"`{m.group(1)}` in {path.relative_to(root)} — delete whole-algo "
                "fuses; express algorithms in Super only"
            )
    return None


def verify_super(tools: dict[str, Toolchain], algo_dir: Path) -> tuple[bool, Optional[str], str]:
    super_bin = tools["super"].path
    if not super_bin:
        return False, None, "super not installed"
    src = algo_dir / "algo.sr"
    if not src.exists():
        return False, None, "algo.sr missing"
    # LAW-BENCH-001 / E0504 — reject host imports/calls before any compile or timing.
    text = src.read_text(encoding="utf-8", errors="replace")
    host_err = check_algo_bans_host(text)
    if host_err is not None:
        return False, None, host_err
    # LAW-BENCH-002 / E0505 — reject bench-shaped host/C APIs before timing.
    surface_err = check_host_surface_bans_bench()
    if surface_err is not None:
        return False, None, surface_err
    # Gate on release AOT (not the stack VM): idiomatic stdlib workloads such as
    # std.game.physics2d with thousands of bodies are correct but can take hours
    # under `super run`, which hung the suite on 15_physics2d.
    with tempfile.TemporaryDirectory(prefix="super_gate_") as td:
        gate_bin = Path(td) / "gate"
        code, out, err, _ = run_cmd(
            [super_bin, "build", "--release", "-o", str(gate_bin), str(src)],
            timeout=TIMEOUT_S,
        )
        if code != 0 or not gate_bin.is_file():
            return (
                False,
                None,
                f"aot_build exit={code} err={err[-2000:]} out={out[-800:]}",
            )
        code, out, err, _ = run_cmd([str(gate_bin)], timeout=TIMEOUT_S)
    cs = parse_checksum(out)
    if code != 0 or cs is None:
        return False, None, f"aot_run exit={code} cs={cs} err={err[-2000:]} out={out[-800:]}"
    return True, cs, "ok"


def write_pdf(path: Path, title: str, lines: list[str]) -> None:
    """Minimal PDF writer (Helvetica, one column)."""

    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")

    content_lines = ["BT", "/F1 10 Tf", "50 780 Td", "14 TL"]
    y_lines = 0
    for ln in lines[:90]:
        content_lines.append(f"({esc(ln[:110])}) Tj T*")
        y_lines += 1
    content_lines.append("ET")
    stream = "\n".join(content_lines).encode("latin-1", "replace")

    objs: list[bytes] = []
    objs.append(b"1 0 obj<< /Type /Catalog /Pages 2 0 R >>endobj\n")
    objs.append(b"2 0 obj<< /Type /Pages /Kids [3 0 R] /Count 1 >>endobj\n")
    objs.append(
        b"3 0 obj<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
        b"/Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>endobj\n"
    )
    objs.append(f"4 0 obj<< /Length {len(stream)} >>stream\n".encode() + stream + b"\nendstream\nendobj\n")
    objs.append(b"5 0 obj<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>endobj\n")

    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for obj in objs:
        offsets.append(len(out))
        out.extend(obj)
    xref_pos = len(out)
    out.extend(f"xref\n0 {len(offsets)}\n".encode())
    out.extend(b"0000000000 65535 f \n")
    for off in offsets[1:]:
        out.extend(f"{off:010d} 00000 n \n".encode())
    out.extend(
        f"trailer<< /Size {len(offsets)} /Root 1 0 R /Info << /Title ({esc(title)}) >> >>\n".encode()
    )
    out.extend(f"startxref\n{xref_pos}\n%%EOF\n".encode())
    path.write_bytes(out)


def render_html(report: dict[str, Any], path: Path) -> None:
    from report_page import render_html as _render

    _render(report, path)


def _fmt_mean(v: Optional[float]) -> str:
    if v is None:
        return "—"
    return f"{v:.4f}s"


def render_comparison_html(report: dict[str, Any]) -> str:
    cmp = report.get("comparison")
    prev = report.get("previous_run")
    if not cmp or not prev:
        return "<h2>Run comparison</h2><p class=\"meta\">No previous run baseline found.</p>"
    stand_rows = []
    for t, d in sorted((cmp.get("standings_delta") or {}).items(), key=lambda kv: -kv[1]["run2"]):
        sign = "+" if d["delta"] >= 0 else ""
        stand_rows.append(
            f"<tr><td><code>{t}</code></td><td>{d['run1']:.3f}x</td>"
            f"<td>{d['run2']:.3f}x</td><td>{sign}{d['delta']:.3f}x</td></tr>"
        )
    loc_rows = []
    for lang, d in sorted((cmp.get("loc_delta") or {}).items()):
        sign = "+" if d["delta"] >= 0 else ""
        loc_rows.append(
            f"<tr><td>{lang}</td><td>{d['run1']}</td><td>{d['run2']}</td>"
            f"<td>{sign}{d['delta']}</td></tr>"
        )
    algo_rows = []
    for row in cmp.get("algo_delta") or []:
        cells = [f"<td><code>{row['name']}</code></td>"]
        for tname in ("c", "super_aot", "super_jit"):
            td = (row.get("targets") or {}).get(tname) or {}
            sp = td.get("speedup_run2_vs_run1")
            sp_s = f"{sp:.2f}x" if sp else "—"
            cells.append(
                f"<td>{_fmt_mean(td.get('run1_mean'))} → {_fmt_mean(td.get('run2_mean'))}"
                f" <em>({sp_s})</em></td>"
            )
        algo_rows.append("<tr>" + "".join(cells) + "</tr>")
    return f"""
<h2>Run 1 → Run 2 comparison</h2>
<p class="meta">Baseline: {prev.get('generated_at')} · {prev.get('baseline_path')}</p>
<h3>Geo-mean standings delta</h3>
<table><thead><tr><th>Target</th><th>Run 1</th><th>Run 2</th><th>Δ</th></tr></thead>
<tbody>{''.join(stand_rows) or '<tr><td colspan="4">no overlapping standings</td></tr>'}</tbody></table>
<h3>Code LOC delta</h3>
<table><thead><tr><th>Language</th><th>Run 1</th><th>Run 2</th><th>Δ</th></tr></thead>
<tbody>{''.join(loc_rows)}</tbody></table>
<h3>Per-algo mean times (C / Super AOT / Super JIT)</h3>
<table><thead><tr><th>Algorithm</th><th>C</th><th>Super AOT</th><th>Super JIT</th></tr></thead>
<tbody>{''.join(algo_rows)}</tbody></table>
"""


def geo_mean(vals: list[float]) -> Optional[float]:
    if not vals:
        return None
    if any(v <= 0 for v in vals):
        return None
    return math.exp(sum(math.log(v) for v in vals) / len(vals))


def run_suite(only: Optional[list[str]] = None) -> dict[str, Any]:
    tools = detect_toolchains()
    machine = machine_info()
    selected = only or ALGOS
    work_root = ROOT / ".work"
    work_root.mkdir(exist_ok=True)

    loc = collect_loc()
    report: dict[str, Any] = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "machine": machine,
        "toolchains": [asdict(tools[k]) for k in tools],
        "protocol": protocol_block(),
        "loc": loc,
        "algorithms": [],
        "standings": {},
    }

    speedups: dict[str, list[float]] = {t: [] for t in TARGETS}

    for name in selected:
        algo_dir = ROOT / name
        print(f"\n=== {name} ===", flush=True)
        entry: dict[str, Any] = {
            "name": name,
            "gate": "pending",
            "reference_checksum": None,
            "targets": [],
        }

        ok, ref_cs, detail = verify_super(tools, algo_dir)
        if not ok:
            print(f"  SUPER GATE FAILED: {detail[:300]}", flush=True)
            entry["gate"] = "failed"
            entry["gate_detail"] = detail
            for t in TARGETS:
                entry["targets"].append(
                    asdict(
                        TargetResult(
                            target=t,
                            status="skipped",
                            detail="SKIPPED (super failed)",
                        )
                    )
                )
            report["algorithms"].append(entry)
            continue

        print(f"  Super gate OK checksum={ref_cs}", flush=True)
        entry["gate"] = "ok"
        entry["reference_checksum"] = ref_cs

        build = AlgoBuild(algo_dir, tools, work_root / f"{name}_build")
        c_ok, c_detail = build.compile_c()
        aot_ok, aot_detail = build.compile_super_aot()
        jit_ok, jit_detail = build.compile_super_jit()
        kt_ok, kt_detail = build.compile_kotlin()
        rs_ok, rs_detail = build.compile_rust()
        zg_ok, zg_detail = build.compile_zig()
        print(
            f"  compile c={c_ok} aot={aot_ok} jit={jit_ok} kotlin={kt_ok} rust={rs_ok} zig={zg_ok}",
            flush=True,
        )

        results: dict[str, TargetResult] = {}
        runners: dict[str, Callable[[], tuple[int, str, str, float]]] = {}
        compile_map: dict[str, Optional[float]] = {}
        extra_warm: dict[str, int] = {}
        need_map = {
            "c": "cc",
            "super_aot": "super",
            "super_jit": "super",
            "bun": "bun",
            "kotlin": "kotlin",
            "lua": "lua",
            "python": "python",
            "julia": "julia",
            "rust": "rust",
            "zig": "zig",
        }
        for target in TARGETS:
            need = need_map[target]
            if not tools[need].available:
                results[target] = TargetResult(
                    target=target,
                    status="failed",
                    detail="FAILED (toolchain not installed)",
                )
                continue
            if target == "c" and not c_ok:
                results[target] = TargetResult(target=target, status="failed", detail=c_detail[-1500:])
                continue
            if target == "super_aot" and not aot_ok:
                results[target] = TargetResult(target=target, status="failed", detail=aot_detail[-1500:])
                continue
            if target == "super_jit" and not jit_ok:
                results[target] = TargetResult(
                    target=target, status="failed", detail=jit_detail[-1500:]
                )
                continue
            if target == "kotlin" and not kt_ok:
                results[target] = TargetResult(target=target, status="failed", detail=kt_detail[-1500:])
                continue
            if target == "rust" and not rs_ok:
                results[target] = TargetResult(target=target, status="failed", detail=rs_detail[-1500:])
                continue
            if target == "zig" and not zg_ok:
                results[target] = TargetResult(target=target, status="failed", detail=zg_detail[-1500:])
                continue
            run = runner_for(target, build, tools)
            if run is None:
                results[target] = TargetResult(
                    target=target, status="failed", detail="runner unavailable"
                )
                continue
            runners[target] = run
            c_s = build.compile_s.get(target)
            if target in INTERPRETED_TARGETS:
                c_s = 0.0
            compile_map[target] = c_s
            if target == "kotlin":
                extra_warm[target] = 1

        if runners:
            print(f"  timing interleaved across {len(runners)} targets…", flush=True)
            timed = time_targets_round_robin(
                runners,
                ref_cs,
                compile_s=compile_map,
                extra_warm=extra_warm or None,
            )
            results.update(timed)
            for t, r in timed.items():
                if r.status == "ok":
                    cy = f" cycle={r.cycle_s:.3f}s" if r.cycle_s is not None else ""
                    prim = wall_primary(r.stats) or r.stats.get("mean", 0.0)
                    sd = r.stats.get("stddev", 0.0)
                    lo = r.stats.get("ci_lo", prim)
                    hi = r.stats.get("ci_hi", prim)
                    note = f" [{r.ranking_note}]" if r.ranking_note else ""
                    print(
                        f"    {t}: {WALL_PRIMARY}={prim:.4f}s ±stdev={sd:.4f}s "
                        f"CI=[{lo:.4f},{hi:.4f}] n={int(r.stats.get('n', 0))}{cy}{note}",
                        flush=True,
                    )
                else:
                    print(f"    {t}: FAIL {r.detail[:200]}", flush=True)

        # Per-algorithm within-noise ranking among ok targets.
        ok_stats = {
            t: r.stats
            for t, r in results.items()
            if r.status == "ok" and r.stats
        }
        entry["ranking"] = ranking_within_noise(ok_stats)

        c_wall = None
        if results.get("c") and results["c"].status == "ok":
            c_wall = wall_primary(results["c"].stats)

        for target in TARGETS:
            r = results.get(target) or TargetResult(
                target=target, status="failed", detail="missing"
            )
            rw = wall_primary(r.stats)
            if r.status == "ok" and c_wall and rw:
                r.speedup_vs_c = c_wall / rw
                speedups[target].append(r.speedup_vs_c)
            entry["targets"].append(asdict(r))

        report["algorithms"].append(entry)

    standings = {}
    for t, vals in speedups.items():
        gm = geo_mean(vals)
        if gm is not None:
            standings[t] = gm
    report["standings"] = standings
    return report


def load_previous_run() -> Optional[dict[str, Any]]:
    """Prefer an explicit run-1 snapshot; fall back to the last results.json."""
    for candidate in (ROOT / "run1_baseline" / "results.json", ROOT / "results.json"):
        if candidate.exists():
            try:
                data = json.loads(candidate.read_text(encoding="utf-8"))
                if isinstance(data, dict) and data.get("algorithms") is not None:
                    data["_baseline_path"] = str(candidate)
                    return data
            except Exception:
                continue
    return None


def attach_run_comparison(report: dict[str, Any], previous: Optional[dict[str, Any]]) -> None:
    """Embed run-1 LOC/perf next to the new run for HTML/PDF comparison."""
    if not previous:
        report["previous_run"] = None
        report["comparison"] = None
        return
    report["previous_run"] = {
        "generated_at": previous.get("generated_at"),
        "baseline_path": previous.get("_baseline_path"),
        "loc": previous.get("loc"),
        "standings": previous.get("standings", {}),
        "algorithms": previous.get("algorithms", []),
    }
    prev_stand = previous.get("standings") or {}
    curr_stand = report.get("standings") or {}
    stand_delta = {}
    for t in TARGETS:
        if t in prev_stand and t in curr_stand:
            stand_delta[t] = {
                "run1": prev_stand[t],
                "run2": curr_stand[t],
                "delta": curr_stand[t] - prev_stand[t],
            }
    prev_loc = (previous.get("loc") or {}).get("by_language") or {}
    curr_loc = (report.get("loc") or {}).get("by_language") or {}
    loc_delta = {}
    for lang in sorted(set(prev_loc) | set(curr_loc)):
        p = (prev_loc.get(lang) or {}).get("code", 0)
        c = (curr_loc.get(lang) or {}).get("code", 0)
        loc_delta[lang] = {"run1": p, "run2": c, "delta": c - p}
    # Per-algo primary walls for super_jit / super_aot / c
    algo_delta = []
    prev_algos = {a["name"]: a for a in previous.get("algorithms", [])}
    for algo in report.get("algorithms", []):
        name = algo["name"]
        prev = prev_algos.get(name)
        row = {"name": name, "targets": {}}
        curr_t = {t["target"]: t for t in algo.get("targets", [])}
        prev_t = {t["target"]: t for t in (prev or {}).get("targets", [])} if prev else {}
        for tname in ("c", "super_aot", "super_jit"):
            ct = curr_t.get(tname) or {}
            pt = prev_t.get(tname) or {}
            cm = wall_primary(ct.get("stats") or {}) if ct.get("status") == "ok" else None
            pm = wall_primary(pt.get("stats") or {}) if pt.get("status") == "ok" else None
            row["targets"][tname] = {
                "run1_mean": pm,  # historical key; value is primary wall
                "run2_mean": cm,
                "run1_primary": pm,
                "run2_primary": cm,
                "run1_status": pt.get("status"),
                "run2_status": ct.get("status"),
                "speedup_run2_vs_run1": (pm / cm) if (pm and cm and cm > 0) else None,
            }
        algo_delta.append(row)
    report["comparison"] = {
        "standings_delta": stand_delta,
        "loc_delta": loc_delta,
        "algo_delta": algo_delta,
    }


def attach_composite_scores(report: dict[str, Any]) -> None:
    """Ratio-to-best composite (worst is never forced to absolute 0).

    Speed / Memory / Cycle / LoC: lower is better → 100 * best / value.
    Reliability: higher is better → 100 * rate / best_rate.
    CPU was removed — it mirrored Speed via thru_vs_c.
    """
    loc_by = (report.get("loc") or {}).get("by_language") or {}
    loc_key = {
        "c": "c",
        "super_aot": "super",
        "super_jit": "super",
        "bun": "bun",
        "kotlin": "kotlin",
        "lua": "lua",
        "python": "python",
        "julia": "julia",
        "rust": "rust",
        "zig": "zig",
    }
    agg: dict[str, dict[str, list[float]]] = {
        t: {"walls": [], "rss": [], "cycles": [], "ok": 0, "attempted": 0}
        for t in TARGETS
    }
    # Per-algo: checksum/compile failures used to be dropped from the geo-mean, which
    # rewarded incomplete ports. Penalize them with 2× the slowest successful primary
    # wall on that algo (or TIMEOUT_S if nobody succeeded) so Super's gate-passing work counts.
    for algo in report.get("algorithms", []):
        if algo.get("gate") != "ok":
            continue
        ok_walls = [
            w
            for t in algo.get("targets", [])
            if t.get("status") == "ok"
            for w in [wall_primary(t.get("stats") or {})]
            if w is not None
        ]
        penalty = (max(ok_walls) * 2.0) if ok_walls else float(TIMEOUT_S)
        ok_cycles = [
            float(t["cycle_s"])
            for t in algo.get("targets", [])
            if t.get("status") == "ok" and t.get("cycle_s")
        ]
        cycle_pen = (max(ok_cycles) * 2.0) if ok_cycles else float(TIMEOUT_S)
        seen: set[str] = set()
        for t in algo.get("targets", []):
            tgt = t["target"]
            if tgt in seen or tgt not in agg:
                continue
            seen.add(tgt)
            if t.get("status") == "skipped":
                continue
            agg[tgt]["attempted"] += 1
            if t.get("status") == "ok":
                agg[tgt]["ok"] += 1
                wall = wall_primary(t.get("stats") or {})
                if wall:
                    agg[tgt]["walls"].append(float(wall))
                rss = t.get("peak_rss_kb")
                if rss:
                    agg[tgt]["rss"].append(float(rss))
                cy = t.get("cycle_s")
                if cy:
                    agg[tgt]["cycles"].append(float(cy))
                elif wall:
                    # Legacy results without compile timing: one run stands in
                    # for cycle until the next suite records compile_s.
                    agg[tgt]["cycles"].append(float(wall))
            else:
                agg[tgt]["walls"].append(penalty)
                agg[tgt]["cycles"].append(cycle_pen)

    raw: dict[str, dict[str, Any]] = {}
    for t in TARGETS:
        gm = geo_mean(agg[t]["walls"])
        gr = geo_mean(agg[t]["rss"]) if agg[t]["rss"] else None
        gc = geo_mean(agg[t]["cycles"]) if agg[t]["cycles"] else None
        loc = (loc_by.get(loc_key[t]) or {}).get("code")
        attempted = agg[t]["attempted"]
        pass_rate = (agg[t]["ok"] / attempted) if attempted else None
        raw[t] = {
            "geo_mean_s": gm,  # geo of primary walls (median)
            "geo_rss_kb": gr,
            "geo_cycle_s": gc,
            "loc_code": loc,
            "pass_rate": pass_rate,
            "ok_count": agg[t]["ok"],
            "attempted": attempted,
        }

    def ratio_lower(key: str) -> dict[str, float]:
        items = {k: v[key] for k, v in raw.items() if v.get(key)}
        if not items:
            return {k: 50.0 for k in TARGETS}
        best = min(items.values())
        out: dict[str, float] = {}
        for k in TARGETS:
            x = raw[k].get(key)
            if x is None or x <= 0:
                out[k] = 50.0
            else:
                # Soft floor at 0.01 so a last-place language is never absolute 0.
                out[k] = max(0.01, 100.0 * best / x)
        return out

    def ratio_higher(key: str) -> dict[str, float]:
        items = {k: v[key] for k, v in raw.items() if v.get(key) is not None}
        if not items:
            return {k: 50.0 for k in TARGETS}
        best = max(items.values())
        out: dict[str, float] = {}
        for k in TARGETS:
            x = raw[k].get(key)
            if x is None or best <= 0:
                out[k] = 50.0
            else:
                out[k] = max(0.01, 100.0 * x / best)
        return out

    speed_s = ratio_lower("geo_mean_s")
    mem_s = ratio_lower("geo_rss_kb")
    cycle_s = ratio_lower("geo_cycle_s")
    loc_s = ratio_lower("loc_code")
    rel_s = ratio_higher("pass_rate")

    composite: dict[str, Any] = {}
    for t in TARGETS:
        parts = {
            "speed": round(speed_s[t], 2),
            "memory": round(mem_s[t], 2),
            "reliability": round(rel_s[t], 2),
            "cycle": round(cycle_s[t], 2),
            "loc": round(loc_s[t], 2),
        }
        contrib = {k: round(SCORE_WEIGHTS[k] * parts[k], 2) for k in SCORE_CATEGORIES}
        composite[t] = {
            **parts,
            "contrib": contrib,
            "raw": raw[t],
            "score": round(sum(contrib.values()), 2),
        }

    def _cat_key(t: str, k: str) -> tuple:
        # Prefer Super on exact ties (e.g. all languages at Rel 100) so category
        # leadership matches the composite narrative without changing scores.
        prefer = 2 if t == "super_aot" else 1 if t == "super_jit" else 0
        return (composite[t][k], prefer)

    winner = max(TARGETS, key=lambda t: (composite[t]["score"], 2 if t == "super_aot" else 1 if t == "super_jit" else 0))
    cat_w = {
        k: max(TARGETS, key=lambda t: _cat_key(t, k)) for k in SCORE_CATEGORIES
    }
    report["composite_scores"] = composite
    report["overall_winner"] = winner
    report["category_winners"] = cat_w
    report["score_weights"] = SCORE_WEIGHTS
    proto = protocol_block()
    report["methodology"] = {
        "protocol_version": PROTOCOL_VERSION,
        "timing": proto["timing"],
        "wall_primary": WALL_PRIMARY,
        "estimator": ESTIMATOR,
        "warmups": WARMUPS,
        "timed_runs": TIMED_RUNS,
        "interleave": proto["interleave"],
        "within_noise": proto["within_noise"],
        "trim_fraction": TRIM_FRACTION,
        "ci_z": CI_Z,
        "speed": (
            f"40% ratio-to-best geo-{WALL_PRIMARY} wall "
            "(Julia uses in-process ELAPSED_S floored to 5% process wall)"
        ),
        "memory": "25% ratio-to-best geo-mean VmHWM RSS",
        "reliability": "15% ratio-to-best checksum pass rate on gated algos",
        "cycle": "10% ratio-to-best geo-mean (compile + first verify run); interpreters compile=0",
        "loc": "10% ratio-to-best code LoC",
        "scoring": "score=100*best/value (lower-better) or 100*value/best; floor 0.01",
        "failures": (
            f"checksum/compile fail → 2× slowest ok {WALL_PRIMARY}/cycle on that algo "
            "(else TIMEOUT_S)"
        ),
        "removed": "cpu (was identical to speed via thru_vs_c)",
        "raw_runs": "Each target keeps raw_runs[] with warmup:true|false; times_s is timed-only.",
    }
    # Keep protocol block in sync with methodology (version is authoritative).
    report["protocol"] = {**(report.get("protocol") or {}), **proto}

    # Markdown summary (overwrite)
    meth = report.get("methodology") or {}
    md = [
        "# Super Multi-Language Benchmark Report",
        "",
        f"Generated: {report.get('generated_at')}",
        f"Host: {(report.get('machine') or {}).get('hostname')} · "
        f"{(report.get('machine') or {}).get('cpus')} CPUs",
        f"Julia: {(next((t for t in report.get('toolchains', []) if t.get('name')=='julia'), {}) or {}).get('version', '')}",
        "",
        f"## Timing protocol `{PROTOCOL_VERSION}`",
        "",
        f"- Warm-ups: **{meth.get('warmups', WARMUPS)}** (timed but excluded from aggregation)",
        f"- Timed runs: **{meth.get('timed_runs', TIMED_RUNS)}** interleaved round-robin across languages",
        f"- Estimator: **{meth.get('estimator', ESTIMATOR)}** (primary wall); mean retained in `stats.*`",
        f"- Spread: stdev, min/max, and {CI_Z:.2f}σ CI on the mean",
        "- Overlapping CIs ⇒ ranking **within noise** (no hard order)",
        "",
        "## Composite Language Score (0–100)",
        "",
        "Weights: Speed 40% · Memory 25% · Reliability 15% · Cycle 10% · LoC 10%.",
        "",
        "| Language | Score | Speed | Memory | Reliability | Cycle | LoC | Geo wall | Geo RSS | Geo cycle | Pass | Code LoC |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for t in sorted(TARGETS, key=lambda x: -composite[x]["score"]):
        c = composite[t]
        r = c["raw"]
        gm = r.get("geo_mean_s")
        gr = r.get("geo_rss_kb")
        gc = r.get("geo_cycle_s")
        pr = r.get("pass_rate")
        md.append(
            f"| `{t}` | **{c['score']}** | {c['speed']} | {c['memory']} | {c['reliability']} | "
            f"{c['cycle']} | {c['loc']} | {(gm or 0):.4f}s | {int(gr) if gr else '—'} | "
            f"{(gc or 0):.4f}s | {f'{100*pr:.0f}%' if pr is not None else '—'} | "
            f"{r.get('loc_code') or '—'} |"
        )
    md += [
        "",
        f"**Overall winner: `{winner}`** ({composite[winner]['score']})",
        "",
        "### Category winners",
    ]
    for k, v in cat_w.items():
        md.append(f"- **{k}**: `{v}`")
    (ROOT / "REPORT.md").write_text("\n".join(md) + "\n", encoding="utf-8")


def rescore_existing() -> int:
    """Recompute composite + reports from the latest results.json (no suite run)."""
    results_path = ROOT / "results.json"
    if not results_path.is_file():
        print(f"missing {results_path}", file=sys.stderr)
        return 1
    report = json.loads(results_path.read_text(encoding="utf-8"))
    previous = load_previous_run()
    if previous and previous.get("_baseline_path") == str(results_path):
        # Prefer an earlier baseline when rescoring the live file in place.
        baseline = ROOT / "run1_baseline" / "results.json"
        if baseline.is_file():
            try:
                previous = json.loads(baseline.read_text(encoding="utf-8"))
            except Exception:
                previous = None
        else:
            previous = None
    if previous:
        attach_run_comparison(report, previous)
    attach_composite_scores(report)
    results_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    html_path = ROOT / "report.html"
    render_html(report, html_path)
    print(f"Rescored {results_path}", flush=True)
    print(f"Wrote {ROOT / 'REPORT.md'}", flush=True)
    print(f"Wrote {html_path}", flush=True)
    print(
        f"Overall winner: {report.get('overall_winner')} "
        f"({(report.get('composite_scores') or {}).get(report.get('overall_winner'), {}).get('score')})",
        flush=True,
    )
    return 0


def apply_cli_config(argv: list[str]) -> list[str]:
    """Consume --warm/--timed/--estimator; mutate module globals; return remaining argv."""
    global WARMUPS, TIMED_RUNS, ESTIMATOR, WALL_PRIMARY
    out: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--warm" and i + 1 < len(argv):
            WARMUPS = int(argv[i + 1])
            i += 2
            continue
        if a.startswith("--warm="):
            WARMUPS = int(a.split("=", 1)[1])
            i += 1
            continue
        if a == "--timed" and i + 1 < len(argv):
            TIMED_RUNS = int(argv[i + 1])
            i += 2
            continue
        if a.startswith("--timed="):
            TIMED_RUNS = int(a.split("=", 1)[1])
            i += 1
            continue
        if a == "--estimator" and i + 1 < len(argv):
            ESTIMATOR = argv[i + 1].strip().lower()
            WALL_PRIMARY = ESTIMATOR if ESTIMATOR in ("median", "trimmed_mean") else "median"
            i += 2
            continue
        if a.startswith("--estimator="):
            ESTIMATOR = a.split("=", 1)[1].strip().lower()
            WALL_PRIMARY = ESTIMATOR if ESTIMATOR in ("median", "trimmed_mean") else "median"
            i += 1
            continue
        out.append(a)
        i += 1
    return out


def parse_only_algos(argv: list[str]) -> Optional[list[str]]:
    """Parse `--only a b` / `--only=a,b` / bare trailing algo names into an algo list."""
    if not argv:
        return None
    if argv[0] in {"--rescore", "rescore", "--reaggregate", "reaggregate"}:
        return None
    names: list[str] = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--only":
            i += 1
            while i < len(argv) and not argv[i].startswith("-"):
                names.extend(x for x in argv[i].split(",") if x)
                i += 1
            continue
        if a.startswith("--only="):
            names.extend(x for x in a.split("=", 1)[1].split(",") if x)
            i += 1
            continue
        if a.startswith("-"):
            raise SystemExit(f"unknown flag: {a}")
        names.append(a)
        i += 1
    if not names:
        return None
    # Resolve prefixes (e.g. `13` → `13_polars_data_pipeline`).
    resolved: list[str] = []
    for n in names:
        if n in ALGOS:
            resolved.append(n)
            continue
        hits = [a for a in ALGOS if a == n or a.startswith(n + "_") or a.startswith(n)]
        if len(hits) == 1:
            resolved.append(hits[0])
        elif len(hits) > 1:
            # Prefer exact prefix before first underscore segment.
            exact = [h for h in hits if h.startswith(n + "_")]
            if len(exact) == 1:
                resolved.append(exact[0])
            else:
                raise SystemExit(f"ambiguous algo {n!r}: {hits}")
        else:
            raise SystemExit(f"unknown algo {n!r}")
    return resolved


def reaggregate_report(
    report: dict[str, Any],
    *,
    estimator: Optional[str] = None,
) -> dict[str, Any]:
    """Recompute stats.* from raw_runs (warmups ignored) under a chosen estimator.

    Old protocol versions remain comparable: raw samples are the source of truth;
    this only refreshes aggregates + methodology without re-running binaries.
    """
    est = estimator or ESTIMATOR
    for algo in report.get("algorithms") or []:
        if not isinstance(algo, dict):
            continue
        ok_stats: dict[str, dict[str, float]] = {}
        for t in algo.get("targets") or []:
            if not isinstance(t, dict) or t.get("status") != "ok":
                continue
            raw = t.get("raw_runs") or []
            if raw:
                timed = timed_samples_from_raw(raw)
                t["times_s"] = timed
                t["stats"] = reaggregate_raw_runs(raw, estimator=est)
            elif t.get("times_s"):
                t["stats"] = stats(list(t["times_s"]), estimator=est)
            if t.get("stats"):
                ok_stats[t["target"]] = t["stats"]
        algo["ranking"] = ranking_within_noise(ok_stats)
    report["protocol"] = {
        **(report.get("protocol") or {}),
        **protocol_block(),
        "estimator": est,
        "wall_primary": est if est in ("median", "trimmed_mean") else "median",
        "reaggregated": True,
    }
    attach_composite_scores(report)
    return report


def main() -> int:
    argv = sys.argv[1:]
    if argv and argv[0] in {"--rescore", "rescore"}:
        return rescore_existing()
    if argv and argv[0] in {"--reaggregate", "reaggregate"}:
        rest = apply_cli_config(argv[1:])
        results_path = ROOT / "results.json"
        if rest:
            results_path = Path(rest[0])
        if not results_path.is_file():
            print(f"missing {results_path}", file=sys.stderr)
            return 1
        report = json.loads(results_path.read_text(encoding="utf-8"))
        reaggregate_report(report)
        results_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"Reaggregated {results_path} under protocol {PROTOCOL_VERSION} estimator={ESTIMATOR}")
        print(f"Wrote {ROOT / 'REPORT.md'}")
        render_html(report, ROOT / "report.html")
        return 0
    argv = apply_cli_config(argv)
    only = parse_only_algos(argv)
    previous = load_previous_run()
    try:
        report = run_suite(only)
    except Exception:
        traceback.print_exc()
        return 1

    results_path = ROOT / "results.json"
    # `--only` patches into the previous full report so a single-algo rerun
    # cannot wipe the scoreboard (and invent a fake `--only` algo name).
    if only and results_path.is_file():
        try:
            prior = json.loads(results_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            prior = None
        if isinstance(prior, dict) and isinstance(prior.get("algorithms"), list):
            by_name = {a.get("name"): a for a in report.get("algorithms", []) if isinstance(a, dict)}
            keep = []
            seen = set()
            for a in prior["algorithms"]:
                if not isinstance(a, dict):
                    continue
                n = a.get("name")
                if n in by_name:
                    keep.append(by_name[n])
                    seen.add(n)
                elif n in ALGOS:
                    keep.append(a)
            for n, a in by_name.items():
                if n not in seen:
                    keep.append(a)
            report["algorithms"] = keep
            # Preserve suite metadata that `--only` would otherwise drop.
            for k in ("machine", "toolchains", "protocol", "loc"):
                if k in prior and k not in report:
                    report[k] = prior[k]

    attach_run_comparison(report, previous)
    attach_composite_scores(report)

    results_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"\nWrote {results_path}", flush=True)
    print(f"Wrote {ROOT / 'REPORT.md'}", flush=True)
    print(
        f"Overall winner: {report.get('overall_winner')} "
        f"({(report.get('composite_scores') or {}).get(report.get('overall_winner'), {}).get('score')})",
        flush=True,
    )

    html_path = ROOT / "report.html"
    render_html(report, html_path)
    print(f"Wrote {html_path}", flush=True)

    pdf_lines = [
        "Super 20-Algo Benchmark Suite",
        f"Generated: {report.get('generated_at')}",
        f"Host: {report['machine'].get('hostname')} cpus={report['machine'].get('cpus')}",
        "",
        "LOC by language (code):",
    ]
    for lang, v in report.get("loc", {}).get("by_language", {}).items():
        pdf_lines.append(f"  {lang}: total={v.get('total',0)} code={v.get('code',0)}")
    g = report.get("loc", {}).get("grand", {})
    pdf_lines.append(f"  GRAND: total={g.get('total',0)} code={g.get('code',0)}")
    pdf_lines.append("")
    pdf_lines.append("Standings (geo-mean vs C):")
    for k, v in sorted(report.get("standings", {}).items(), key=lambda kv: -kv[1]):
        pdf_lines.append(f"  {k}: {v:.3f}x")
    pdf_lines.append("")
    for algo in report["algorithms"]:
        lo = report.get("loc", {}).get("per_algo", {}).get(algo["name"], {})
        loc_s = " ".join(f"{lang}={lo.get(lang, {}).get('code', 0)}" for lang in LOC_SOURCES)
        pdf_lines.append(
            f"{algo['name']} gate={algo.get('gate')} cs={algo.get('reference_checksum')} LOC[{loc_s}]"
        )
        for t in algo["targets"]:
            wall = wall_primary(t.get("stats") or {}) if t["status"] == "ok" else None
            pdf_lines.append(
                f"  {t['target']}: {t['status']}"
                + (f" {WALL_PRIMARY}={wall:.4f}s" if wall is not None else "")
                + (f" {t.get('detail','')[:60]}" if t["status"] != "ok" else "")
            )
    pdf_path = ROOT / "report.pdf"
    write_pdf(pdf_path, "Super Benchmark Report", pdf_lines)
    print(f"Wrote {pdf_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
