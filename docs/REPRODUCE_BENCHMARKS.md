# Reproduce benchmarks on your hardware

The published Stage-1 **scoreboard is frozen** in [`REPORT.md`](REPORT.md) and [`results.json`](../results.json). Do **not** overwrite those files. This guide is for optional local reruns and **community submissions**.

Start with [`../benchmarks/REPRODUCE.md`](../benchmarks/REPRODUCE.md) (integrity smoke, checksum-only rescoring).

## Record your environment

Before any timed run, capture:

| Field | Example |
|---|---|
| CPU model | `lscpu` / `sysctl -n machdep.cpu.brand_string` |
| OS + kernel | `uname -a` |
| Compiler versions | Toolchain block in your local `results.json` |
| CPU governor | `performance` vs `powersave` (note if not pinned) |
| Git commit | `git rev-parse HEAD` |
| Harness protocol | `2.0.0` (see `benchmarks/laws/LAW-BENCH-001.md`) |

## Integrity-only (no timing)

```bash
cd benchmarks
sha256sum -c SHA256SUMS
sha256sum -c data/SHA256SUMS
python3 -m unittest discover -s . -p 'test_*.py' -q
```

## Optional single-algorithm timing (local only)

```bash
cd benchmarks
python3 runner.py --only 01_nbody
```

Compare emitted `CHECKSUM=` lines to the frozen row in `data/results.json` — mismatches indicate environment or port drift, not an automatic scoreboard update.

## Submit community results

1. Run the harness on your machine (full or subset — say which).
2. Export a JSON artifact plus `CHECKSUM=` lines and the environment table above.
3. Open a **Benchmark result** issue (`.github/ISSUE_TEMPLATE/benchmark_result.yml`).
4. Maintainers may copy approved submissions into `benchmarks/results/community/` — **never** into `docs/results.json` without a formal protocol bump.

## Site regeneration (no re-measure)

```bash
./scripts/publish_site.sh
```

Reads existing `docs/results.json`; does not rerun language binaries.