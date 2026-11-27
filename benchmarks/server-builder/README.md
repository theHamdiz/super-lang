# Server builder microbenchmark

This ergonomics-round benchmark is separate from the frozen 0.1.0 composite
benchmark and does not change any published composite input.

`new.sr` uses `App.serve`; `verbose.sr` wires `Server`, `Notify`, and cleanup
explicitly. Both register the same typed `/ping` handler and compile with the
same release AOT profile. `load.py` is a dependency-free HTTP/1.1 probe.

## Stress probe

```sh
SUPER_STD="$PWD/stdlib" target/debug/super build --release benchmarks/server-builder/new.sr
SUPER_ADDR=127.0.0.1:3022 ./target/new &
pid=$!
python3 benchmarks/server-builder/load.py \
  --port 3022 --pid "$pid" --concurrency 32 --duration 5 --runs 3
kill -INT "$pid"
```

The 2026-11-26 run on Linux 7.3.5 and a Ryzen 9 9950X completed each run with
zero HTTP errors: median 14,993.65 requests/s, 2.112 ms p50, and 2.511 ms p99.
A separate five-second probe completed 78,342 requests at concurrency 128 with
zero errors and 10.402 ms p99.

These are **not release performance claims**. RSS grew from 2,104,708 KiB to
6,372,592 KiB over the three stress runs, so request-lifetime allocation is not
bounded by this evidence.

## Builder comparison

```sh
cargo run --release -p super-cli -- build --release benchmarks/server-builder/verbose.sr
cargo run --release -p super-cli -- build --release benchmarks/server-builder/new.sr

SUPER_ADDR=127.0.0.1:3021 ./target/verbose &
python3 benchmarks/server-builder/load.py --pid "$!" --duration 2 --runs 3 --concurrency 16
kill -TERM "$!"

SUPER_ADDR=127.0.0.1:3021 ./target/new &
python3 benchmarks/server-builder/load.py --pid "$!" --duration 2 --runs 3 --concurrency 16
kill -TERM "$!"
```

`results.json` records both probes. Comparison load results are medians of
three independent two-second windows; startup is the median and full range of
ten process-to-listen measurements. The probe reports process RSS, not
allocator events, so the allocation-count gate remains unverified.
