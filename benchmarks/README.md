# Super benchmarks

Stage-1 equal-algorithm suite (**Protocol 2.0.0**, LAW-BENCH-001). Scores are **frozen** — see [`../docs/REPORT.md`](../docs/REPORT.md).

| Artifact | Path |
|---|---|
| Harness + 20×10 ports | `01_nbody` … `20_end_to_end_lakehouse_service` |
| Runner / scoring | `runner.py`, `report_page.py` |
| Frozen raw JSON | `data/results.json` (+ `data/SHA256SUMS`) |
| Reproduction steps | [`REPRODUCE.md`](REPRODUCE.md) |
| Public site | [`../docs/`](../docs/) |

Regenerate **HTML only** (no re-measure):

```bash
./scripts/publish_site.sh
```
