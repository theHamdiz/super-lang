# Reproducing Stage-1 checksums (not timings)

Benchmark **scores are frozen** in [`../docs/REPORT.md`](../docs/REPORT.md) and [`../docs/results.json`](../docs/results.json). Do not re-run the full suite to “refresh” the public scoreboard.

## Integrity smoke (no re-measure)

```bash
cd benchmarks
sha256sum -c SHA256SUMS          # pins data/results.json
sha256sum -c data/SHA256SUMS
python3 -m unittest discover -s . -p 'test_*.py' -q
```

`python3 runner.py --rescore` recomputes composite tables from the frozen `results.json` **without** re-executing language binaries. Do not pass bare `python3 runner.py` (that starts a full timed suite).

## Optional single-algorithm re-run (writes local artifacts only)

```bash
# Example — measures one algorithm; patches into local results.json.
# Do not publish new medians over the frozen Stage-1 board.
python3 runner.py --only 01_nbody
```

Compare any emitted `CHECKSUM=` line to the value recorded for that algorithm/language in `data/results.json` (same bytes as `../docs/results.json`).

## Full harness layout

| Path | Role |
|---|---|
| `runner.py`, `report_page.py`, `test_*.py` | Orchestration and scoring (**do not edit weights or aggregation**) |
| `01_nbody` … `20_end_to_end_lakehouse_service` | Equal-algorithm ports (10 languages each) |
| `laws/LAW-BENCH-001.md` | Super sources must not `use host` in suite kernels |
| `data/results.json` | Frozen raw aggregates (copy of `docs/results.json`) |
| `data/SHA256SUMS` / `SHA256SUMS` | Integrity pins for published data files |

## Toolchain snapshot

Published tool versions for the frozen run are embedded in `data/results.json` → `toolchains` and mirrored on the [benchmarks toolchain table](https://thehamdiz.github.io/super-lang/benchmarks/#toolchain).

## Site regeneration (no re-measure)

```bash
./scripts/publish_site.sh   # HTML only; reads existing docs/results.json
```
