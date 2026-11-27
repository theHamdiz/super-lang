# Differential evidence pack (A11)

One-command reproduce (from repo root):

```bash
python3 benchmarks/runner.py --protocol 2.0.0
```

Contents:

| Path | Role |
|------|------|
| `../../benchmarks/` | 20 equal-algorithm kernels + multi-language harness |
| `../../benchmarks/laws/LAW-BENCH-001.md` | No host FFI in gated Super benches |
| `../../docs/suite report` | Raw suite results |
| `../../docs/REPORT.md` / `report.html` | Composite report (also served on `:8765`) |
| `checksum-equivalence.md` | VM vs JIT vs C23 policy notes |

Machine / compiler details are recorded in the suite report header and `docs/stage1_speed_walls.md`.

README performance claims must link here — not float unbound.
