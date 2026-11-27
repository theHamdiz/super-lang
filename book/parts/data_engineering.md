# High-Throughput Data Engineering (`std.data`)

`std.data` is a zero-GC columnar analytics pipeline layered on Super’s collections, `std.core.decimal`, `std.fmt.*`, and `std.math.stats`.

## Layers

1. **`column`** — `IntCol`, `FloatCol`, `BoolCol`, `StrCol` with null bitmaps and SIMD-friendly kernels.
2. **`expr`** — filter/projection AST and Kleene three-valued logic.
3. **`frame`** — in-memory `Frame` with filter, sort, join, group_by, pivot-style helpers.
4. **`plan`** — `LazyFrame` with a result-preserving optimizer (`.lazy().collect()`).
5. **`io`** — `scan_csv` / `read_csv` / `write_csv` and `read_xlsx` / `write_xlsx` via `std.fmt.xlsx`.

## Example

```super
use std.data.io
use std.data.frame

f = read_csv("events.csv")
g = f.filter("amount > 0").group_by("region").agg_sum("amount")
write_xlsx(g, "out.xlsx")
```

## Laws

`expr.law.sr` / `plan.law.sr` lock filter commutativity and optimizer result preservation; discharge with `super verify`.

## Formats

Unified tabular I/O spans `csv`, `json`/`jsonl`, `toml`, `yaml`, `xml`, `msgpack`, `xlsx`, and SQL sinks through `std.db.sql`.
